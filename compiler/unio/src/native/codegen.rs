//! Cranelift code generation from the lowered form (docs/runtime.md 8.4).
//!
//! Every unit becomes one native function. Values have the layout of
//! `unio_runtime::value::Value`; a local of type `Int`, `Float` or `Bool` whose type the
//! signature or its binding shows is kept unboxed in a register, and operators on such values
//! are machine instructions with the overflow and zero checks of runtime section 2 (an add is
//! one add and a branch on overflow). Other values live in the frame of the function or in
//! temporaries on its stack, and operations on values that own memory call the functions of
//! `native::support`. Calls of a function with a body and no type arguments are direct calls
//! with the unboxed parameters in registers. Control flow, `try`, `on`, `using`, `break`,
//! `continue` and `return` are native branches; an unwinding operation returns status 1,
//! after which the code releases the temporaries it holds and continues at the innermost
//! handler or returns 1 itself.

use super::support::{index_steps, op_number, CURSOR_SIZE, MAGIC};
use super::units::{Body, Ref, Unit};
use crate::interp::{Interp, Intrinsic};
use crate::lower::{Block as LBlock, Code, Node, Op, PStep, Pat, Place, S};
use crate::program::Owner;
use crate::types::{Prim, Ty};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{F64, I32, I64, I8};
use cranelift_codegen::ir::{
    self, AbiParam, BlockArg, FuncRef, InstBuilder, MemFlagsData, Signature, StackSlot,
    StackSlotData, StackSlotKind, UserFuncName,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::{HashMap, HashSet};
use unio_runtime::value::layout;
use unio_runtime::value::Value as RValue;

const VSIZE: i32 = layout::SIZE as i32;

/// How code is generated: `release` optimises for speed.
pub struct Options {
    pub release: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Boxed,
    Int,
    Float,
    Bool,
}

impl Kind {
    fn of(ty: &Ty) -> Kind {
        match ty {
            Ty::Prim(Prim::Int) => Kind::Int,
            Ty::Prim(Prim::Float) => Kind::Float,
            Ty::Prim(Prim::Bool) => Kind::Bool,
            _ => Kind::Boxed,
        }
    }

    fn scalar(self) -> bool {
        self != Kind::Boxed
    }

    fn ir(self) -> ir::Type {
        match self {
            Kind::Int | Kind::Boxed => I64,
            Kind::Float => F64,
            Kind::Bool => I8,
        }
    }

    fn tag(self) -> i64 {
        match self {
            Kind::Int => layout::INT as i64,
            Kind::Float => layout::FLOAT as i64,
            Kind::Bool => layout::BOOL as i64,
            Kind::Boxed => -1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RetKind {
    Unit,
    Scalar(Kind),
    Boxed,
}

/// The direct entry of a function: how each parameter is passed (a scalar in a register or a
/// value by address, taken by the callee) and how the result comes back.
struct FnInfo {
    direct: FuncId,
    pass: Vec<Kind>,
    ret: RetKind,
    /// The kind of each parameter's slot inside the function.
    slot: Vec<Kind>,
    var: Vec<bool>,
}

/// Facts about the whole program that the code of one unit relies on.
struct Facts {
    /// Method names with a `var self` method, with a `var` parameter, and the scalar result
    /// that every method of the name has.
    var_self: HashSet<String>,
    var_param: HashSet<String>,
    method_ret: HashMap<String, Option<Kind>>,
    /// The scalar type that every field of the name has, if any.
    field_kind: HashMap<String, Option<Kind>>,
}

impl Facts {
    fn new(interp: &Interp) -> Facts {
        let p = &interp.program;
        let mut f = Facts {
            var_self: HashSet::new(),
            var_param: HashSet::new(),
            method_ret: HashMap::new(),
            field_kind: HashMap::new(),
        };
        let merge = |map: &mut HashMap<String, Option<Kind>>, name: &str, k: Kind| {
            let k = k.scalar().then_some(k);
            map.entry(name.to_string())
                .and_modify(|old| {
                    if *old != k {
                        *old = None
                    }
                })
                .or_insert(k);
        };
        for def in &p.defs {
            match &def.kind {
                crate::program::DefKind::Struct(fs) | crate::program::DefKind::Error(fs) => {
                    for field in fs {
                        merge(&mut f.field_kind, &field.name, Kind::of(&field.ty));
                    }
                }
                crate::program::DefKind::Enum(vs) => {
                    for v in vs {
                        for (n, t) in &v.fields {
                            merge(&mut f.field_kind, n, Kind::of(t));
                        }
                    }
                }
                _ => {}
            }
        }
        for def in &p.funcs {
            if def.self_param.is_none() {
                continue;
            }
            if def.self_param == Some(true) {
                f.var_self.insert(def.name.clone());
            }
            if def.params.iter().any(|q| q.var) {
                f.var_param.insert(def.name.clone());
            }
            merge(&mut f.method_ret, &def.name, Kind::of(&def.ret));
        }
        // A field holding a function may be called like a method; its result is unknown.
        let fields: Vec<String> = f.field_kind.keys().cloned().collect();
        for name in fields {
            f.method_ret.insert(name, None);
        }
        f
    }
}

/// Where a temporary value lives: a stack slot and an offset in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Addr {
    ss: StackSlot,
    off: i32,
    /// A temporary of its own (not an element of an array).
    single: bool,
}

/// Something on this function's stack that must be released when evaluation unwinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Live {
    Val(Addr),
    Cursor(StackSlot),
}

/// A value during code generation.
#[derive(Clone, Copy, Debug)]
enum V {
    I(ir::Value),
    F(ir::Value),
    B(ir::Value),
    Unit,
    /// An owned value in a temporary.
    Own(Addr),
    /// A borrowed value at an address: a slot of the frame or a literal.
    Ref(ir::Value),
}

/// The array of evaluated index steps of a place: its slot, its elements, their number.
type Steps = (StackSlot, Vec<Addr>, usize);

enum Scope {
    /// Unwinding stops here (the handlers of `try` or `on`).
    Landing { block: ir::Block, live: usize },
    /// A `using` block: its landing releases the resource; leaving it normally too.
    Using {
        landing: ir::Block,
        live: usize,
        res: Addr,
    },
    Loop {
        brk: ir::Block,
        cont: ir::Block,
        live: usize,
    },
    /// The slots a block binds, cleared when the block is left by `break` or `continue`.
    Clear(u32, u32),
}

/// Helper functions of `native::support`: name, parameters, results.
const P: ir::Type = I64;
const HELPERS: &[(&str, &[ir::Type], &[ir::Type])] = &[
    ("drop", &[P], &[]),
    ("clear", &[P, I64], &[]),
    ("clone", &[P, P], &[]),
    ("stack_exhausted", &[P], &[I32]),
    ("int_fault", &[P, I32], &[I32]),
    ("int_power", &[P, I64, I64, P], &[I32]),
    ("float_rem", &[F64, F64], &[F64]),
    ("float_pow", &[F64, F64], &[F64]),
    ("wrong_kind", &[P, I32, P], &[I32]),
    ("binary", &[P, I32, P, P, P], &[I32]),
    ("compare", &[I32, P, P], &[I32]),
    ("not", &[P, P, P], &[I32]),
    ("negate", &[P, P, P], &[I32]),
    ("call", &[P, I64, P, I64, P, P], &[I32]),
    ("call_typed", &[P, P, I64, P, P, I64, P], &[I32]),
    ("call_value", &[P, P, P, I64, P], &[I32]),
    ("take_arg", &[P, P, I64, P, I64, P, P], &[I32]),
    ("put_arg", &[P, P, I64, P, I64, P, P], &[I32]),
    ("method", &[P, P, I64, P, P, I32, P, P, P], &[I32]),
    ("field", &[P, P, P, P], &[I32]),
    ("field_ref", &[P, P, P], &[P]),
    ("field_place", &[P, P, P], &[P]),
    ("get_or", &[P, P, I64, P, P, I32, P, P], &[I32]),
    ("safe_field", &[P, P, P, P], &[I32]),
    ("index", &[P, P, P, P], &[I32]),
    ("constant", &[P, I64, P], &[I32]),
    ("fnref", &[P, I64, P], &[]),
    ("variant", &[P, P, P], &[]),
    ("construct", &[P, P, P, P], &[I32]),
    ("sequence", &[I32, P, I64, P], &[]),
    ("map", &[P, I64, P], &[]),
    ("text_new", &[P], &[]),
    ("text_literal", &[P, P, I64], &[]),
    ("text_value", &[P, P, P], &[I32]),
    ("text_int", &[P, I64], &[]),
    ("closure", &[P, P, P, P, P, P], &[]),
    ("or", &[P, P, I32, P], &[I32]),
    ("try", &[P, P, I32, P], &[I32]),
    ("match", &[P, P, I64, P, P], &[I32]),
    ("fault_node", &[P, P], &[I32]),
    ("assert_failed", &[P, P], &[I32]),
    ("fail", &[P, P], &[I32]),
    ("catch", &[P, P, I64, P, I64], &[I32]),
    ("using_open", &[P, P, P, P], &[]),
    ("using_release", &[P, P], &[I32]),
    ("using_unwind", &[P, P], &[]),
    ("cursor_init", &[P, P, P], &[I32]),
    ("cursor_next", &[P, P, P], &[I32]),
    ("cursor_drop", &[P], &[]),
    ("bind", &[P, P, I64, P, P, P], &[I32]),
    ("list_push", &[P, P], &[]),
    ("list_done", &[P, P], &[I32]),
    ("assign", &[P, P, I64, P, P, P], &[I32]),
    ("set_index", &[P, P, I64, P, P, P, P], &[I32]),
    ("concat", &[P, P, P, I64], &[I32]),
];

/// The object file of a program: its units, the embedded image, and `main`.
pub(crate) fn compile(
    interp: &Interp,
    units: &[Unit],
    image: &[u8],
    options: &Options,
) -> Result<Vec<u8>, String> {
    if !layout::holds() {
        return Err("the value layout is not the one native code assumes".into());
    }
    let mut flags = settings::builder();
    let set = |flags: &mut settings::Builder, k: &str, v: &str| {
        flags
            .set(k, v)
            .map_err(|e| format!("the code generator setting {k}: {e}"))
    };
    set(
        &mut flags,
        "opt_level",
        if options.release { "speed" } else { "none" },
    )?;
    set(&mut flags, "is_pic", "true")?;
    set(&mut flags, "enable_probestack", "true")?;
    set(&mut flags, "probestack_strategy", "inline")?;
    set(&mut flags, "preserve_frame_pointers", "true")?;
    set(&mut flags, "enable_verifier", "true")?;
    let isa = cranelift_codegen::isa::lookup_by_name("x86_64-unknown-linux-gnu")
        .map_err(|e| format!("no code generator for x86-64 Linux: {e}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| format!("the code generator: {e}"))?;
    let builder = ObjectBuilder::new(isa, "unio", cranelift_module::default_libcall_names())
        .map_err(|e| e.to_string())?;
    let mut module = ObjectModule::new(builder);
    let frontend = module.isa().frontend_config();
    let facts = Facts::new(interp);

    // Every unit is declared before any is defined, so that calls can refer to any of them.
    let mut entries = Vec::with_capacity(units.len());
    let mut infos: HashMap<usize, FnInfo> = HashMap::new();
    let mut lambda_entry: HashMap<usize, FuncId> = HashMap::new();
    let mut tables = Vec::with_capacity(units.len());
    let mut lengths = Vec::with_capacity(units.len());
    let mut unit_refs = Vec::with_capacity(units.len());
    let entry_sig = {
        let mut s = module.make_signature();
        for _ in 0..4 {
            s.params.push(AbiParam::new(P));
        }
        s.returns.push(AbiParam::new(I32));
        s
    };
    for (i, unit) in units.iter().enumerate() {
        let id = module
            .declare_function(&format!("unio_u{i}"), Linkage::Local, &entry_sig)
            .map_err(|e| e.to_string())?;
        entries.push(id);
        if let Body::Func(f, code) = &unit.body {
            let info = fn_info(interp, *f, code);
            let sig = direct_signature(&module, &info);
            let direct = module
                .declare_function(&format!("unio_d{i}"), Linkage::Local, &sig)
                .map_err(|e| e.to_string())?;
            infos.insert(*f, FnInfo { direct, ..info });
        }
        if let Body::Lambda(code) = &unit.body {
            lambda_entry.insert(std::rc::Rc::as_ptr(code) as usize, id);
        }
        let mut refs = Vec::new();
        let mut lambdas = Vec::new();
        super::units::refs(unit, &mut refs, &mut lambdas);
        let table = module
            .declare_data(&format!("unio_r{i}"), Linkage::Local, true, false)
            .map_err(|e| e.to_string())?;
        let mut data = DataDescription::new();
        data.define_zeroinit((refs.len() * 8).max(8));
        data.set_align(8);
        module
            .define_data(table, &data)
            .map_err(|e| e.to_string())?;
        tables.push(table);
        lengths.push(refs.len() as u64);
        unit_refs.push(refs);
    }

    let mut shared = Shared {
        module,
        helpers: HashMap::new(),
        literals: HashMap::new(),
        infos,
        lambda_entry,
        facts,
        interp,
        fbctx: FunctionBuilderContext::new(),
        frontend,
    };
    for (i, unit) in units.iter().enumerate() {
        let index: HashMap<usize, usize> = unit_refs[i]
            .iter()
            .enumerate()
            .map(|(j, r)| (r.address(), j))
            .collect();
        let ctx = UnitCtx {
            module: unit.module,
            table: tables[i],
            index,
        };
        match &unit.body {
            Body::Func(f, code) => {
                shared.define_direct(*f, code, &ctx)?;
                shared.define_wrapper(entries[i], *f)?;
            }
            Body::Expr(code) => {
                shared.define_expr(entries[i], &code.body, code.slots, &ctx)?;
            }
            Body::Lambda(code) => {
                shared.define_expr(entries[i], &code.body, code.slots, &ctx)?;
            }
        }
    }
    shared.define_image(image, &entries, &tables, &lengths)?;
    let product = shared.module.finish();
    product.emit().map_err(|e| e.to_string())
}

fn fn_info(interp: &Interp, f: usize, code: &Code) -> FnInfo {
    let def = &interp.program.funcs[f];
    let mut pass = Vec::new();
    let mut slot = Vec::new();
    let mut var = Vec::new();
    if let Some(var_self) = def.self_param {
        let k = match def.owner {
            Owner::Impl(i) if !var_self => Kind::of(&interp.program.impls[i].target),
            _ => Kind::Boxed,
        };
        pass.push(k);
        slot.push(k);
        var.push(var_self);
    }
    for (i, p) in def.params.iter().enumerate() {
        let k = if p.var { Kind::Boxed } else { Kind::of(&p.ty) };
        let has_default = code
            .params
            .get(i + usize::from(def.self_param.is_some()))
            .is_some_and(|c| c.default.is_some());
        pass.push(if has_default { Kind::Boxed } else { k });
        slot.push(k);
        var.push(p.var);
    }
    let ret = match &def.ret {
        Ty::Unit => RetKind::Unit,
        t if Kind::of(t).scalar() => RetKind::Scalar(Kind::of(t)),
        _ => RetKind::Boxed,
    };
    FnInfo {
        direct: FuncId::from_u32(0),
        pass,
        ret,
        slot,
        var,
    }
}

fn direct_signature(module: &ObjectModule, info: &FnInfo) -> Signature {
    let mut s = module.make_signature();
    for _ in 0..3 {
        s.params.push(AbiParam::new(P));
    }
    for k in &info.pass {
        s.params.push(AbiParam::new(k.ir()));
    }
    s.returns.push(AbiParam::new(I32));
    if let RetKind::Scalar(k) = info.ret {
        s.returns.push(AbiParam::new(k.ir()));
    }
    s
}

struct Shared<'a> {
    module: ObjectModule,
    helpers: HashMap<&'static str, FuncId>,
    literals: HashMap<Vec<u8>, DataId>,
    infos: HashMap<usize, FnInfo>,
    lambda_entry: HashMap<usize, FuncId>,
    facts: Facts,
    interp: &'a Interp,
    fbctx: FunctionBuilderContext,
    frontend: cranelift_codegen::isa::TargetFrontendConfig,
}

struct UnitCtx {
    module: usize,
    table: DataId,
    index: HashMap<usize, usize>,
}

impl Shared<'_> {
    fn helper(&mut self, name: &'static str) -> Result<FuncId, String> {
        if let Some(id) = self.helpers.get(name) {
            return Ok(*id);
        }
        let (_, params, rets) = HELPERS
            .iter()
            .find(|(n, _, _)| *n == name)
            .ok_or_else(|| format!("internal: no helper {name}"))?;
        let mut s = self.module.make_signature();
        for p in *params {
            s.params.push(AbiParam::new(*p));
        }
        for r in *rets {
            s.returns.push(AbiParam::new(*r));
        }
        let id = self
            .module
            .declare_function(&format!("unio_rt_{name}"), Linkage::Import, &s)
            .map_err(|e| e.to_string())?;
        self.helpers.insert(name, id);
        Ok(id)
    }

    fn literal(&mut self, bytes: &[u8]) -> Result<DataId, String> {
        if let Some(id) = self.literals.get(bytes) {
            return Ok(*id);
        }
        let id = self
            .module
            .declare_data(
                &format!("unio_l{}", self.literals.len()),
                Linkage::Local,
                false,
                false,
            )
            .map_err(|e| e.to_string())?;
        let mut data = DataDescription::new();
        let mut contents = bytes.to_vec();
        contents.push(0);
        data.define(contents.into_boxed_slice());
        self.module
            .define_data(id, &data)
            .map_err(|e| e.to_string())?;
        self.literals.insert(bytes.to_vec(), id);
        Ok(id)
    }

    fn finish_function(
        &mut self,
        id: FuncId,
        ctx: &mut cranelift_codegen::Context,
    ) -> Result<(), String> {
        self.module
            .define_function(id, ctx)
            .map_err(|e| format!("internal: code generation failed: {e:?}"))
    }

    /// The direct entry of a function.
    fn define_direct(&mut self, f: usize, code: &Code, unit: &UnitCtx) -> Result<(), String> {
        let info = &self.infos[&f];
        let (direct, pass, ret, slot_kinds, var) = (
            info.direct,
            info.pass.clone(),
            info.ret,
            info.slot.clone(),
            info.var.clone(),
        );
        let mut ctx = self.module.make_context();
        ctx.func.signature = direct_signature(&self.module, &self.infos[&f]);
        ctx.func.name = UserFuncName::user(0, direct.as_u32());
        let mut fbctx = std::mem::take(&mut self.fbctx);
        let result = {
            let b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
            let frontend = self.frontend;
            let mut g = Gen::new(self, b, unit, code.slots);
            g.ret = ret;
            let mut r = g.direct(f, code, &pass, &slot_kinds, &var);
            if r.is_ok() && g.broken {
                r = Err(g.unsupported("internal: a temporary released twice"));
            }
            if r.is_ok() {
                g.b.finalize(frontend);
            }
            r
        };
        self.fbctx = fbctx;
        result?;
        self.finish_function(direct, &mut ctx)
    }

    /// The entry of a function from the interpreter: arguments in a frame of values.
    fn define_wrapper(&mut self, id: FuncId, f: usize) -> Result<(), String> {
        let info = &self.infos[&f];
        let (direct, pass, ret) = (info.direct, info.pass.clone(), info.ret);
        let mut ctx = self.module.make_context();
        for _ in 0..4 {
            ctx.func.signature.params.push(AbiParam::new(P));
        }
        ctx.func.signature.returns.push(AbiParam::new(I32));
        ctx.func.name = UserFuncName::user(0, id.as_u32());
        let mut fbctx = std::mem::take(&mut self.fbctx);
        let result: Result<(), String> = {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let params = b.block_params(entry).to_vec();
            let (cx, frame, out, types) = (params[0], params[1], params[2], params[3]);
            let mut args = vec![cx, out, types];
            let wrong = b.create_block();
            b.set_cold_block(wrong);
            b.append_block_param(wrong, P);
            for (i, k) in pass.iter().enumerate() {
                let at = b.ins().iadd_imm_s(frame, i as i64 * VSIZE as i64);
                if k.scalar() {
                    let tag = b.ins().load(I8, MemFlagsData::trusted(), at, 0);
                    let ok = b.ins().icmp_imm_s(IntCC::Equal, tag, k.tag());
                    let next = b.create_block();
                    b.ins().brif(ok, next, &[], wrong, &[BlockArg::Value(at)]);
                    b.switch_to_block(next);
                    let off = if *k == Kind::Bool {
                        layout::BOOL_PAYLOAD
                    } else {
                        layout::PAYLOAD
                    };
                    args.push(b.ins().load(k.ir(), MemFlagsData::trusted(), at, off));
                } else {
                    args.push(at);
                }
            }
            let callee = self.module.declare_func_in_func(direct, b.func);
            let call = b.ins().call(callee, &args);
            let results = b.inst_results(call).to_vec();
            let status = results[0];
            match ret {
                RetKind::Scalar(k) => {
                    let ok = b.create_block();
                    let done = b.create_block();
                    b.append_block_param(done, I32);
                    b.ins()
                        .brif(status, done, &[BlockArg::Value(status)], ok, &[]);
                    b.switch_to_block(ok);
                    store_scalar(&mut b, out, k, results[1]);
                    b.ins().jump(done, &[BlockArg::Value(status)]);
                    b.switch_to_block(done);
                    let s = b.block_params(done)[0];
                    b.ins().return_(&[s]);
                }
                RetKind::Unit => {
                    let tag = b.ins().iconst(I8, layout::UNIT as i64);
                    b.ins().store(MemFlagsData::trusted(), tag, out, 0);
                    b.ins().return_(&[status]);
                }
                RetKind::Boxed => {
                    b.ins().return_(&[status]);
                }
            }
            b.switch_to_block(wrong);
            let at = b.block_params(wrong)[0];
            let helper = self.helper("wrong_kind")?;
            let h = self.module.declare_func_in_func(helper, b.func);
            let which = b.ins().iconst(I32, 1);
            let call = b.ins().call(h, &[cx, which, at]);
            let s = b.inst_results(call)[0];
            b.ins().return_(&[s]);
            b.seal_all_blocks();
            b.finalize(self.frontend);
            Ok(())
        };
        self.fbctx = fbctx;
        result?;
        self.finish_function(id, &mut ctx)
    }

    /// The entry of a constant, a default, or a lambda: its frame comes from the caller.
    fn define_expr(
        &mut self,
        id: FuncId,
        body: &Node,
        slots: usize,
        unit: &UnitCtx,
    ) -> Result<(), String> {
        let mut ctx = self.module.make_context();
        for _ in 0..4 {
            ctx.func.signature.params.push(AbiParam::new(P));
        }
        ctx.func.signature.returns.push(AbiParam::new(I32));
        ctx.func.name = UserFuncName::user(0, id.as_u32());
        let mut fbctx = std::mem::take(&mut self.fbctx);
        let result = {
            let b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
            let frontend = self.frontend;
            let mut g = Gen::new(self, b, unit, slots);
            let mut r = g.expr_unit(body);
            if r.is_ok() && g.broken {
                r = Err(g.unsupported("internal: a temporary released twice"));
            }
            if r.is_ok() {
                g.b.finalize(frontend);
            }
            r
        };
        self.fbctx = fbctx;
        result?;
        self.finish_function(id, &mut ctx)
    }

    fn define_image(
        &mut self,
        image: &[u8],
        entries: &[FuncId],
        tables: &[DataId],
        lengths: &[u64],
    ) -> Result<(), String> {
        let err = |e: cranelift_module::ModuleError| e.to_string();
        let m = &mut self.module;
        let data_id = m
            .declare_data("unio_program", Linkage::Local, false, false)
            .map_err(err)?;
        let mut d = DataDescription::new();
        d.define(image.to_vec().into_boxed_slice());
        m.define_data(data_id, &d).map_err(err)?;

        let n = entries.len();
        let entries_id = m
            .declare_data("unio_entries", Linkage::Local, false, false)
            .map_err(err)?;
        let mut d = DataDescription::new();
        d.define(vec![0u8; (n * 8).max(8)].into_boxed_slice());
        d.set_align(8);
        for (i, e) in entries.iter().enumerate() {
            let r = m.declare_func_in_data(*e, &mut d);
            d.write_function_addr((i * 8) as u32, r);
        }
        m.define_data(entries_id, &d).map_err(err)?;

        let tables_id = m
            .declare_data("unio_tables", Linkage::Local, false, false)
            .map_err(err)?;
        let mut d = DataDescription::new();
        d.define(vec![0u8; (n * 8).max(8)].into_boxed_slice());
        d.set_align(8);
        for (i, t) in tables.iter().enumerate() {
            let g = m.declare_data_in_data(*t, &mut d);
            d.write_data_addr((i * 8) as u32, g, 0);
        }
        m.define_data(tables_id, &d).map_err(err)?;

        let lengths_id = m
            .declare_data("unio_lengths", Linkage::Local, false, false)
            .map_err(err)?;
        let mut d = DataDescription::new();
        let mut bytes = Vec::with_capacity(n * 8);
        for l in lengths {
            bytes.extend_from_slice(&l.to_le_bytes());
        }
        bytes.resize(bytes.len().max(8), 0);
        d.define(bytes.into_boxed_slice());
        d.set_align(8);
        m.define_data(lengths_id, &d).map_err(err)?;

        // struct Image { magic, value_size, data, data_len, units, entries, tables, lengths }
        let image_id = m
            .declare_data("unio_image", Linkage::Local, false, false)
            .map_err(err)?;
        let mut d = DataDescription::new();
        let mut bytes = vec![0u8; 64];
        bytes[0..8].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[8..16].copy_from_slice(&(layout::SIZE as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&(image.len() as u64).to_le_bytes());
        bytes[32..40].copy_from_slice(&(n as u64).to_le_bytes());
        d.define(bytes.into_boxed_slice());
        d.set_align(8);
        for (off, id) in [
            (16, data_id),
            (40, entries_id),
            (48, tables_id),
            (56, lengths_id),
        ] {
            let g = m.declare_data_in_data(id, &mut d);
            d.write_data_addr(off, g, 0);
        }
        m.define_data(image_id, &d).map_err(err)?;

        // int main(int argc, char **argv) { return unio_native_main(argc, argv, &unio_image); }
        let mut sig = m.make_signature();
        sig.params.push(AbiParam::new(I32));
        sig.params.push(AbiParam::new(P));
        sig.returns.push(AbiParam::new(I32));
        let main = m
            .declare_function("main", Linkage::Export, &sig)
            .map_err(err)?;
        let mut start_sig = sig.clone();
        start_sig.params.push(AbiParam::new(P));
        let start = m
            .declare_function("unio_native_main", Linkage::Import, &start_sig)
            .map_err(err)?;
        let mut ctx = m.make_context();
        ctx.func.signature = sig;
        ctx.func.name = UserFuncName::user(0, main.as_u32());
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut self.fbctx);
            let block = b.create_block();
            b.append_block_params_for_function_params(block);
            b.switch_to_block(block);
            let p = b.block_params(block).to_vec();
            let gv = m.declare_data_in_func(image_id, b.func);
            let addr = b.ins().symbol_value(P, gv);
            let s = m.declare_func_in_func(start, b.func);
            let call = b.ins().call(s, &[p[0], p[1], addr]);
            let r = b.inst_results(call)[0];
            b.ins().return_(&[r]);
            b.seal_all_blocks();
            b.finalize(self.frontend);
        }
        m.define_function(main, &mut ctx)
            .map_err(|e| format!("{e:?}"))?;
        Ok(())
    }
}

fn store_scalar(b: &mut FunctionBuilder, at: ir::Value, k: Kind, v: ir::Value) {
    let tag = b.ins().iconst(I8, k.tag());
    b.ins().store(MemFlagsData::trusted(), tag, at, 0);
    let off = if k == Kind::Bool {
        layout::BOOL_PAYLOAD
    } else {
        layout::PAYLOAD
    };
    b.ins().store(MemFlagsData::trusted(), v, at, off);
}

/// The code generator of one function.
struct Gen<'s, 'a, 'b> {
    s: &'s mut Shared<'a>,
    b: FunctionBuilder<'b>,
    unit: &'s UnitCtx,
    funcs: HashMap<FuncId, FuncRef>,
    cx: ir::Value,
    types: ir::Value,
    out: ir::Value,
    /// The frame: a stack slot of the function, or the frame its caller passed.
    frame_ss: Option<StackSlot>,
    frame_param: Option<ir::Value>,
    nslots: usize,
    kinds: Vec<Kind>,
    vars: Vec<Option<Variable>>,
    /// Slots that an argument may change during an expression (a `var` argument or a method
    /// that changes `self`): they are copied when read, never borrowed.
    hazard: HashSet<u32>,
    live: Vec<Live>,
    scopes: Vec<Scope>,
    free: HashMap<i32, Vec<StackSlot>>,
    unwind_exit: ir::Block,
    ok_exit: ir::Block,
    /// The address of this call's frame on the runtime's call stack, for the current line.
    site: Option<ir::Value>,
    ret: RetKind,
    ret_var: Option<Variable>,
    line: u32,
    /// Unwind blocks, filled at the end: what they release and where they continue.
    deferred: Vec<(ir::Block, Vec<Live>, Option<Addr>, ir::Block)>,
    table_gv: Option<ir::GlobalValue>,
    /// An internal inconsistency of the generator was found; the unit is not compiled.
    broken: bool,
}

impl<'s, 'a, 'b> Gen<'s, 'a, 'b> {
    fn new(
        s: &'s mut Shared<'a>,
        mut b: FunctionBuilder<'b>,
        unit: &'s UnitCtx,
        nslots: usize,
    ) -> Gen<'s, 'a, 'b> {
        let unwind_exit = b.create_block();
        let ok_exit = b.create_block();
        let dummy = ir::Value::from_u32(0);
        Gen {
            s,
            b,
            unit,
            funcs: HashMap::new(),
            cx: dummy,
            types: dummy,
            out: dummy,
            frame_ss: None,
            frame_param: None,
            nslots,
            kinds: vec![Kind::Boxed; nslots],
            vars: vec![None; nslots],
            hazard: HashSet::new(),
            live: Vec::new(),
            scopes: Vec::new(),
            free: HashMap::new(),
            unwind_exit,
            ok_exit,
            site: None,
            ret: RetKind::Boxed,
            ret_var: None,
            line: 0,
            deferred: Vec::new(),
            table_gv: None,
            broken: false,
        }
    }

    /// Fills the unwind blocks: release, then continue at the target.
    fn finish_deferred(&mut self) -> Result<(), String> {
        while let Some((block, items, extra, target)) = self.deferred.pop() {
            self.b.switch_to_block(block);
            for item in items.iter().rev() {
                match item {
                    Live::Val(a) => {
                        let at = self.addr(*a);
                        self.emit_drop(at)?;
                    }
                    Live::Cursor(ss) => {
                        let at = self.b.ins().stack_addr(P, *ss, 0);
                        self.h("cursor_drop", &[at])?;
                    }
                }
            }
            if let Some(a) = extra {
                let at = self.addr(a);
                self.emit_drop(at)?;
            }
            self.b.ins().jump(target, &[]);
        }
        Ok(())
    }

    fn unsupported(&self, what: &str) -> String {
        let file = &self.s.interp.files[self.unit.module];
        format!(
            "{file}:{}: not yet compiled natively: {what}",
            self.line.max(1)
        )
    }

    // ---------------------------------------------------------------- calls of helpers

    fn h(&mut self, name: &'static str, args: &[ir::Value]) -> Result<Option<ir::Value>, String> {
        let id = self.s.helper(name)?;
        let r = self.fref(id);
        let call = self.b.ins().call(r, args);
        Ok(self.b.inst_results(call).first().copied())
    }

    fn hs(&mut self, name: &'static str, args: &[ir::Value]) -> Result<ir::Value, String> {
        self.h(name, args)?
            .ok_or_else(|| format!("internal: helper {name} has no status"))
    }

    fn fref(&mut self, id: FuncId) -> FuncRef {
        if let Some(r) = self.funcs.get(&id) {
            return *r;
        }
        let r = self.s.module.declare_func_in_func(id, self.b.func);
        self.funcs.insert(id, r);
        r
    }

    fn iconst(&mut self, ty: ir::Type, v: i64) -> ir::Value {
        self.b.ins().iconst(ty, v)
    }

    fn null(&mut self) -> ir::Value {
        self.iconst(P, 0)
    }

    /// The runtime address of reference `r` of this unit.
    fn refp(&mut self, r: Ref) -> Result<ir::Value, String> {
        let Some(&i) = self.unit.index.get(&r.address()) else {
            return Err(self.unsupported("a reference outside its unit"));
        };
        let gv = match self.table_gv {
            Some(gv) => gv,
            None => {
                let gv = self
                    .s
                    .module
                    .declare_data_in_func(self.unit.table, self.b.func);
                self.table_gv = Some(gv);
                gv
            }
        };
        let base = self.b.ins().symbol_value(P, gv);
        Ok(self
            .b
            .ins()
            .load(P, MemFlagsData::trusted(), base, (i * 8) as i32))
    }

    fn node_ref(&mut self, n: &Node) -> Result<ir::Value, String> {
        self.refp(Ref::Node(n))
    }

    fn stmt_ref(&mut self, s: &S) -> Result<ir::Value, String> {
        self.refp(Ref::Stmt(s))
    }

    // ---------------------------------------------------------------- frame and temporaries

    fn slot_addr(&mut self, slot: u32) -> ir::Value {
        let off = slot as i32 * VSIZE;
        match (self.frame_ss, self.frame_param) {
            (Some(ss), _) => self.b.ins().stack_addr(P, ss, off),
            (None, Some(p)) => self.b.ins().iadd_imm_s(p, off as i64),
            _ => self.null(),
        }
    }

    fn frame_ptr(&mut self) -> ir::Value {
        match (self.frame_ss, self.frame_param) {
            (Some(ss), _) => self.b.ins().stack_addr(P, ss, 0),
            (None, Some(p)) => p,
            _ => self.null(),
        }
    }

    fn nslots_v(&mut self) -> ir::Value {
        let n = if self.frame_ss.is_some() || self.frame_param.is_some() {
            self.nslots as i64
        } else {
            0
        };
        self.iconst(I64, n)
    }

    fn stack(&mut self, values: usize) -> StackSlot {
        let size = (values.max(1) as i32) * VSIZE;
        if let Some(ss) = self.free.get_mut(&size).and_then(|v| v.pop()) {
            return ss;
        }
        self.b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size as u32,
            3,
        ))
    }

    fn release_stack(&mut self, ss: StackSlot, values: usize) {
        let size = (values.max(1) as i32) * VSIZE;
        let pool = self.free.entry(size).or_default();
        // A temporary given back twice would be handed out twice: refuse to generate code.
        if pool.contains(&ss) {
            self.broken = true;
        }
        pool.push(ss);
    }

    fn temp(&mut self) -> Addr {
        Addr {
            ss: self.stack(1),
            off: 0,
            single: true,
        }
    }

    fn addr(&mut self, a: Addr) -> ir::Value {
        self.b.ins().stack_addr(P, a.ss, a.off)
    }

    fn push_live(&mut self, a: Addr) {
        self.live.push(Live::Val(a));
    }

    fn unlive(&mut self, a: Addr) {
        if let Some(i) = self.live.iter().rposition(|l| *l == Live::Val(a)) {
            self.live.remove(i);
        }
    }

    /// A temporary whose value a helper took or moved elsewhere.
    fn consumed(&mut self, a: Addr) {
        self.unlive(a);
        if a.single {
            self.release_stack(a.ss, 1);
        }
    }

    fn tag(&mut self, at: ir::Value) -> ir::Value {
        self.b.ins().load(I8, MemFlagsData::trusted(), at, 0)
    }

    /// Releases the value at an address if it owns memory.
    fn emit_drop(&mut self, at: ir::Value) -> Result<(), String> {
        let tag = self.tag(at);
        let owns =
            self.b
                .ins()
                .icmp_imm_s(IntCC::UnsignedGreaterThan, tag, layout::LAST_SCALAR as i64);
        let drop = self.b.create_block();
        let next = self.b.create_block();
        self.b.set_cold_block(drop);
        self.b.ins().brif(owns, drop, &[], next, &[]);
        self.b.switch_to_block(drop);
        self.h("drop", &[at])?;
        self.b.ins().jump(next, &[]);
        self.b.switch_to_block(next);
        Ok(())
    }

    fn set_unit(&mut self, at: ir::Value) {
        let z = self.iconst(I8, layout::UNIT as i64);
        self.b.ins().store(MemFlagsData::trusted(), z, at, 0);
    }

    fn copy_value(&mut self, from: ir::Value, to: ir::Value) {
        for i in 0..(VSIZE / 8) {
            let w = self.b.ins().load(I64, MemFlagsData::trusted(), from, i * 8);
            self.b.ins().store(MemFlagsData::trusted(), w, to, i * 8);
        }
    }

    /// Copies the value at `from` (borrowed) to `to` (holding nothing).
    fn emit_clone(&mut self, from: ir::Value, to: ir::Value) -> Result<(), String> {
        let tag = self.tag(from);
        let owns =
            self.b
                .ins()
                .icmp_imm_s(IntCC::UnsignedGreaterThan, tag, layout::LAST_SCALAR as i64);
        let slow = self.b.create_block();
        let fast = self.b.create_block();
        let next = self.b.create_block();
        self.b.ins().brif(owns, slow, &[], fast, &[]);
        self.b.switch_to_block(fast);
        for i in 0..2 {
            let w = self.b.ins().load(I64, MemFlagsData::trusted(), from, i * 8);
            self.b.ins().store(MemFlagsData::trusted(), w, to, i * 8);
        }
        self.b.ins().jump(next, &[]);
        self.b.switch_to_block(slow);
        self.h("clone", &[from, to])?;
        self.b.ins().jump(next, &[]);
        self.b.switch_to_block(next);
        Ok(())
    }

    /// Writes a value into memory that holds nothing; an owned temporary is moved.
    fn store_into(&mut self, v: V, to: ir::Value) -> Result<(), String> {
        match v {
            V::I(x) => store_scalar(&mut self.b, to, Kind::Int, x),
            V::F(x) => store_scalar(&mut self.b, to, Kind::Float, x),
            V::B(x) => store_scalar(&mut self.b, to, Kind::Bool, x),
            V::Unit => self.set_unit(to),
            V::Own(a) => {
                let from = self.addr(a);
                self.copy_value(from, to);
                self.consumed(a);
            }
            V::Ref(p) => self.emit_clone(p, to)?,
        }
        Ok(())
    }

    /// The value as an owned temporary.
    fn own(&mut self, v: V) -> Result<Addr, String> {
        if let V::Own(a) = v {
            return Ok(a);
        }
        let a = self.temp();
        let at = self.addr(a);
        self.store_into(v, at)?;
        self.push_live(a);
        Ok(a)
    }

    /// The address of the value, borrowed until [`Gen::done`].
    fn ptr(&mut self, v: V) -> Result<(ir::Value, V), String> {
        match v {
            V::Ref(p) => Ok((p, v)),
            V::Own(a) => {
                let p = self.addr(a);
                Ok((p, v))
            }
            other => {
                let a = self.own(other)?;
                let p = self.addr(a);
                Ok((p, V::Own(a)))
            }
        }
    }

    /// Ends the use of a value: an owned temporary is released.
    fn done(&mut self, v: V) -> Result<(), String> {
        if let V::Own(a) = v {
            let at = self.addr(a);
            self.emit_drop(at)?;
            self.consumed(a);
        }
        Ok(())
    }

    /// Moves an owned value into a helper that takes it: its address.
    fn give(&mut self, v: V) -> Result<(ir::Value, Addr), String> {
        let a = self.own(v)?;
        let p = self.addr(a);
        Ok((p, a))
    }

    /// An array of `n` values on the stack, filled by evaluating the nodes in order.
    fn array(&mut self, nodes: &[&Node]) -> Result<(ir::Value, StackSlot, Vec<Addr>), String> {
        let ss = self.stack(nodes.len());
        let mut addrs = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            let v = self.expr(n)?;
            let a = Addr {
                ss,
                off: i as i32 * VSIZE,
                single: false,
            };
            let at = self.addr(a);
            self.store_into(v, at)?;
            self.push_live(a);
            addrs.push(a);
        }
        let p = self.b.ins().stack_addr(P, ss, 0);
        Ok((p, ss, addrs))
    }

    fn array_taken(&mut self, ss: StackSlot, addrs: &[Addr], n: usize) {
        for a in addrs {
            self.unlive(*a);
        }
        self.release_stack(ss, n);
    }

    // ---------------------------------------------------------------- unwinding

    /// The innermost place where unwinding continues, below scope `limit`.
    fn target(&self, limit: usize) -> (ir::Block, usize) {
        for s in self.scopes[..limit].iter().rev() {
            match s {
                Scope::Landing { block, live } => return (*block, *live),
                Scope::Using { landing, live, .. } => return (*landing, *live),
                _ => {}
            }
        }
        (self.unwind_exit, 0)
    }

    /// A block that releases what is live above the target and continues there.
    fn unwind_from(
        &mut self,
        limit: usize,
        excluded: &[Addr],
        extra: Option<Addr>,
    ) -> Result<ir::Block, String> {
        let (target, keep) = self.target(limit);
        let items: Vec<Live> = self.live[keep.min(self.live.len())..]
            .iter()
            .filter(|l| !matches!(l, Live::Val(a) if excluded.contains(a)))
            .copied()
            .collect();
        if items.is_empty() && extra.is_none() {
            return Ok(target);
        }
        let block = self.b.create_block();
        self.b.set_cold_block(block);
        self.deferred.push((block, items, extra, target));
        Ok(block)
    }

    /// Continues when `status` is 0 and unwinds otherwise.
    fn check(&mut self, status: ir::Value) -> Result<(), String> {
        let bad = self.unwind_from(self.scopes.len(), &[], None)?;
        let ok = self.b.create_block();
        self.b.ins().brif(status, bad, &[], ok, &[]);
        self.b.switch_to_block(ok);
        Ok(())
    }

    /// Unwinds unconditionally (the pending reason is set); code after it is unreachable.
    fn unwind_now(&mut self) -> Result<(), String> {
        let bad = self.unwind_from(self.scopes.len(), &[], None)?;
        self.b.ins().jump(bad, &[]);
        let dead = self.b.create_block();
        self.b.switch_to_block(dead);
        Ok(())
    }

    // ---------------------------------------------------------------- conversions

    fn scalar(&mut self, v: V, k: Kind) -> Result<ir::Value, String> {
        match (v, k) {
            (V::I(x), Kind::Int) | (V::F(x), Kind::Float) | (V::B(x), Kind::Bool) => Ok(x),
            (V::I(x), Kind::Float) => Ok(self.b.ins().fcvt_from_sint(F64, x)),
            (V::Own(_) | V::Ref(_), _) => {
                let (p, v) = self.ptr(v)?;
                let tag = self.tag(p);
                let ok = self.b.ins().icmp_imm_s(IntCC::Equal, tag, k.tag());
                let good = self.b.create_block();
                let bad = self.b.create_block();
                self.b.set_cold_block(bad);
                self.b.ins().brif(ok, good, &[], bad, &[]);
                self.b.switch_to_block(bad);
                let which = self.iconst(
                    I32,
                    match k {
                        Kind::Int => 1,
                        Kind::Float => 2,
                        _ => 3,
                    },
                );
                let cx = self.cx;
                let st = self.hs("wrong_kind", &[cx, which, p])?;
                let _ = st;
                let target = self.unwind_from(self.scopes.len(), &[], None)?;
                self.b.ins().jump(target, &[]);
                self.b.switch_to_block(good);
                let off = if k == Kind::Bool {
                    layout::BOOL_PAYLOAD
                } else {
                    layout::PAYLOAD
                };
                let x = self.b.ins().load(k.ir(), MemFlagsData::trusted(), p, off);
                // A scalar owns no memory: the temporary is only given back.
                if let V::Own(a) = v {
                    self.consumed(a);
                }
                Ok(x)
            }
            _ => Err(self.unsupported("a value of an unexpected kind")),
        }
    }

    /// A condition: a `Bool` in a register.
    fn truth_of(&mut self, v: V) -> Result<ir::Value, String> {
        match v {
            V::B(x) => Ok(x),
            V::Own(_) | V::Ref(_) => {
                let (p, v) = self.ptr(v)?;
                let tag = self.tag(p);
                let ok = self
                    .b
                    .ins()
                    .icmp_imm_s(IntCC::Equal, tag, layout::BOOL as i64);
                let good = self.b.create_block();
                let bad = self.b.create_block();
                self.b.set_cold_block(bad);
                self.b.ins().brif(ok, good, &[], bad, &[]);
                self.b.switch_to_block(bad);
                let which = self.iconst(I32, 0);
                let cx = self.cx;
                self.hs("wrong_kind", &[cx, which, p])?;
                let target = self.unwind_from(self.scopes.len(), &[], None)?;
                self.b.ins().jump(target, &[]);
                self.b.switch_to_block(good);
                let x = self
                    .b
                    .ins()
                    .load(I8, MemFlagsData::trusted(), p, layout::BOOL_PAYLOAD);
                if let V::Own(a) = v {
                    self.consumed(a);
                }
                Ok(x)
            }
            other => {
                let (p, v) = self.ptr(other)?;
                let which = self.iconst(I32, 0);
                let cx = self.cx;
                self.hs("wrong_kind", &[cx, which, p])?;
                self.done(v)?;
                self.unwind_now()?;
                Ok(self.iconst(I8, 0))
            }
        }
    }

    // ---------------------------------------------------------------- static kinds

    fn kind(&self, n: &Node) -> Kind {
        let interp = self.s.interp;
        match n {
            Node::Value(RValue::Int(_)) => Kind::Int,
            Node::Value(RValue::Float(_)) => Kind::Float,
            Node::Value(RValue::Bool(_)) => Kind::Bool,
            Node::Local(s) => self.kinds[*s as usize],
            Node::Binary(op, l, r) => {
                let (kl, kr) = (self.kind(l), self.kind(r));
                match op {
                    Op::Add
                    | Op::Subtract
                    | Op::Multiply
                    | Op::Divide
                    | Op::Remainder
                    | Op::Power => {
                        if kl == kr && matches!(kl, Kind::Int | Kind::Float) {
                            kl
                        } else {
                            Kind::Boxed
                        }
                    }
                    Op::In | Op::NotIn => Kind::Boxed,
                    _ => {
                        if kl == kr && kl.scalar() {
                            Kind::Bool
                        } else {
                            Kind::Boxed
                        }
                    }
                }
            }
            Node::And(..) | Node::Is(..) => Kind::Bool,
            Node::Not(v) if self.kind(v) == Kind::Bool => Kind::Bool,
            Node::Or(l, r, _) if self.kind(l) == Kind::Bool && self.kind(r) == Kind::Bool => {
                Kind::Bool
            }
            Node::Negate(v) => match self.kind(v) {
                k @ (Kind::Int | Kind::Float) => k,
                _ => Kind::Boxed,
            },
            Node::Conditional(parts) => {
                let (kt, ko) = (self.kind(&parts[1]), self.kind(&parts[2]));
                if kt == ko {
                    kt
                } else {
                    Kind::Boxed
                }
            }
            Node::Call(call) if call.types.is_empty() => {
                let def = &interp.program.funcs[call.f];
                if def.generics.is_empty() {
                    Kind::of(&def.ret)
                } else {
                    Kind::Boxed
                }
            }
            Node::Method(m) => self
                .s
                .facts
                .method_ret
                .get(&m.name)
                .copied()
                .flatten()
                .unwrap_or(Kind::Boxed),
            Node::Field(_, name, _) => self
                .s
                .facts
                .field_kind
                .get(&**name)
                .copied()
                .flatten()
                .unwrap_or(Kind::Boxed),
            _ => Kind::Boxed,
        }
    }

    /// Whether a call is of the prelude's `range(start, end)`.
    fn is_range(&self, n: &Node) -> Option<(usize, usize)> {
        let Node::Call(call) = n else { return None };
        if !matches!(self.s.interp.intrinsics[call.f], Some(Intrinsic::Range(_))) {
            return None;
        }
        if call.args.len() != 2 || !call.types.is_empty() {
            return None;
        }
        let start = call.args.iter().position(|a| a.target == 0)?;
        let end = call.args.iter().position(|a| a.target == 1)?;
        Some((start, end))
    }

    /// The slots of a function that are kept unboxed.
    fn infer(&mut self, block: &LBlock) {
        for s in &block.stmts {
            self.infer_s(&s.kind);
        }
    }

    fn infer_s(&mut self, s: &S) {
        match s {
            S::Let(slot, n) => {
                let k = self.kind(n);
                let slot = *slot as usize;
                if k.scalar() && !self.hazard.contains(&(slot as u32)) {
                    self.kinds[slot] = k;
                }
            }
            S::Handled { inner, block, .. } => {
                self.infer_s(inner);
                self.infer(block);
            }
            S::If(branches, otherwise) => {
                for (_, b) in branches {
                    self.infer(b);
                }
                if let Some(b) = otherwise {
                    self.infer(b);
                }
            }
            S::While(_, b) | S::Using(_, _, b) => self.infer(b),
            S::For(names, n, b) => {
                if let ([slot], Some(_)) = (&names[..], self.is_range(n)) {
                    if !self.hazard.contains(slot) {
                        self.kinds[*slot as usize] = Kind::Int;
                    }
                }
                self.infer(b);
            }
            S::Match(_, cases) => {
                for c in cases {
                    self.infer(&c.body);
                }
            }
            S::Try(b, hs) => {
                self.infer(b);
                for h in hs {
                    self.infer(&h.body);
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- hazards

    /// The roots of places that an expression may change through `var`.
    fn var_roots(&self, n: &Node, out: &mut Vec<u32>) {
        let interp = self.s.interp;
        let facts = &self.s.facts;
        let mut stack = vec![n];
        while let Some(n) = stack.pop() {
            match n {
                Node::Call(call) => {
                    let def = &interp.program.funcs[call.f];
                    let offset = usize::from(def.self_param.is_some());
                    for a in &call.args {
                        let var = if a.target < offset {
                            def.self_param == Some(true)
                        } else {
                            def.params.get(a.target - offset).is_some_and(|p| p.var)
                        };
                        if let (true, Some(p)) = (var, &a.place) {
                            out.push(p.root);
                        }
                        stack.push(&a.value);
                    }
                }
                Node::Method(m) => {
                    if facts.var_self.contains(&m.name) {
                        if let Some(p) = &m.place {
                            out.push(p.root);
                        }
                    }
                    if facts.var_param.contains(&m.name) {
                        for a in &m.args {
                            if let Some(p) = &a.place {
                                out.push(p.root);
                            }
                        }
                    }
                    stack.push(&m.receiver);
                    for a in &m.args {
                        stack.push(&a.value);
                    }
                }
                Node::Text(parts) => {
                    for p in parts {
                        if let crate::lower::Part::Value(v) = p {
                            stack.push(v);
                        }
                    }
                }
                Node::Tuple(items) | Node::List(items) => stack.extend(items.iter()),
                Node::Map(entries) => {
                    for (k, v) in entries {
                        stack.push(k);
                        stack.push(v);
                    }
                }
                Node::Comprehension(c) => {
                    stack.push(&c.iterable);
                    stack.push(&c.element);
                    if let Some(x) = &c.condition {
                        stack.push(x);
                    }
                }
                Node::Is(v, _) | Node::Not(v) | Node::Negate(v) | Node::Try(v, _) => stack.push(v),
                Node::Field(t, _, _) | Node::SafeField(t, _) => stack.push(t),
                Node::Conditional(p) => stack.extend(p.iter()),
                Node::And(l, r) | Node::Or(l, r, _) | Node::Binary(_, l, r) | Node::Index(l, r) => {
                    stack.push(l);
                    stack.push(r);
                }
                Node::CallValue(c, args) => {
                    stack.push(c);
                    stack.extend(args.iter());
                }
                Node::Construct(c) => stack.extend(c.args.iter().map(|(_, v)| v)),
                _ => {}
            }
        }
    }

    fn collect_hazards(&mut self, block: &LBlock) {
        let mut roots = Vec::new();
        let mut nodes: Vec<&Node> = Vec::new();
        collect_nodes_block(block, &mut nodes);
        for n in nodes {
            self.var_roots(n, &mut roots);
        }
        self.hazard.extend(roots);
    }

    // ---------------------------------------------------------------- units

    /// The body of a function with its direct signature.
    fn direct(
        &mut self,
        f: usize,
        code: &Code,
        pass: &[Kind],
        slot_kinds: &[Kind],
        var: &[bool],
    ) -> Result<(), String> {
        let interp = self.s.interp;
        let def = &interp.program.funcs[f];
        self.line = def.span.start.line as u32;
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let params = self.b.block_params(entry).to_vec();
        self.cx = params[0];
        self.out = params[1];
        self.types = params[2];
        let args = &params[3..];

        // Kinds: the parameters from the signature, the rest from their bindings.
        for (i, k) in slot_kinds.iter().enumerate() {
            if i < self.kinds.len() {
                self.kinds[i] = *k;
            }
        }
        self.collect_hazards(&code.body);
        for p in &code.params {
            if let Some(d) = &p.default {
                let mut roots = Vec::new();
                self.var_roots(d, &mut roots);
                self.hazard.extend(roots);
            }
        }
        for h in self.hazard.clone() {
            if let Some(k) = self.kinds.get_mut(h as usize) {
                if (h as usize) >= slot_kinds.len() {
                    *k = Kind::Boxed;
                }
            }
        }
        self.infer(&code.body);
        for (i, k) in self.kinds.clone().iter().enumerate() {
            if k.scalar() {
                let v = self.b.declare_var(k.ir());
                let zero = match k {
                    Kind::Float => self.b.ins().f64const(0.0),
                    _ => self.b.ins().iconst(k.ir(), 0),
                };
                self.b.def_var(v, zero);
                self.vars[i] = Some(v);
            }
        }
        if let RetKind::Scalar(k) = self.ret {
            let v = self.b.declare_var(k.ir());
            let zero = match k {
                Kind::Float => self.b.ins().f64const(0.0),
                _ => self.b.ins().iconst(k.ir(), 0),
            };
            self.b.def_var(v, zero);
            self.ret_var = Some(v);
        }

        // The call frame on the runtime's stack: depth below the limit, then (site, line).
        let cx = self.cx;
        let depth_ptr = self.b.ins().load(P, MemFlagsData::trusted(), cx, 8);
        let depth = self
            .b
            .ins()
            .load(I64, MemFlagsData::trusted(), depth_ptr, 0);
        let limit = self.b.ins().load(I64, MemFlagsData::trusted(), cx, 16);
        let over = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, depth, limit);
        let exhausted = self.b.create_block();
        let body = self.b.create_block();
        self.b.set_cold_block(exhausted);
        self.b.ins().brif(over, exhausted, &[], body, &[]);
        self.b.switch_to_block(exhausted);
        // The callee takes its arguments by value: they are released here.
        for (i, k) in pass.iter().enumerate() {
            if !k.scalar() && !var[i] {
                self.emit_drop(args[i])?;
            }
        }
        self.h("stack_exhausted", &[cx])?;
        let one = self.iconst(I32, 1);
        let mut rets = vec![one];
        if let RetKind::Scalar(k) = self.ret {
            rets.push(match k {
                Kind::Float => self.b.ins().f64const(0.0),
                _ => self.b.ins().iconst(k.ir(), 0),
            });
        }
        self.b.ins().return_(&rets);
        self.b.switch_to_block(body);
        let frames = self.b.ins().load(P, MemFlagsData::trusted(), cx, 0);
        let offset = self.b.ins().ishl_imm_s(depth, 3);
        let site = self.b.ins().iadd(frames, offset);
        let id = self.iconst(I32, f as i64);
        self.b.ins().store(MemFlagsData::trusted(), id, site, 0);
        let line = self.iconst(I32, def.span.start.line as i64);
        self.b.ins().store(MemFlagsData::trusted(), line, site, 4);
        let next = self.b.ins().iadd_imm_s(depth, 1);
        self.b
            .ins()
            .store(MemFlagsData::trusted(), next, depth_ptr, 0);
        self.site = Some(site);

        // The frame of boxed slots.
        if self.kinds.iter().any(|k| !k.scalar()) || pass.iter().any(|k| !k.scalar()) {
            let ss = self.b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                (self.nslots as u32) * VSIZE as u32,
                3,
            ));
            self.frame_ss = Some(ss);
            for i in 0..self.nslots {
                let at = self.slot_addr(i as u32);
                self.set_unit(at);
            }
        }
        // Parameters: scalars into registers, values moved into the frame.
        for (i, k) in pass.iter().enumerate() {
            if k.scalar() {
                if let Some(v) = self.vars[i] {
                    self.b.def_var(v, args[i]);
                }
            } else {
                let at = self.slot_addr(i as u32);
                self.copy_value(args[i], at);
                if !var[i] {
                    self.set_unit(args[i]);
                }
            }
        }
        // Defaults, which see the parameters before them (runtime 1.3).
        for (i, p) in code.params.iter().enumerate() {
            if let Some(d) = &p.default {
                let at = self.slot_addr(i as u32);
                let tag = self.tag(at);
                let missing = self
                    .b
                    .ins()
                    .icmp_imm_s(IntCC::Equal, tag, layout::UNIT as i64);
                let fill = self.b.create_block();
                let after = self.b.create_block();
                self.b.ins().brif(missing, fill, &[], after, &[]);
                self.b.switch_to_block(fill);
                let v = self.expr(d)?;
                let at = self.slot_addr(i as u32);
                self.store_into(v, at)?;
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(after);
            }
            if p.default.is_some() && self.kinds[i].scalar() {
                let at = self.slot_addr(i as u32);
                let k = self.kinds[i];
                let x = self.scalar(V::Ref(at), k)?;
                if let Some(v) = self.vars[i] {
                    self.b.def_var(v, x);
                }
            }
        }

        self.block(&code.body)?;
        // Falling off the end returns Unit.
        if self.ret == RetKind::Boxed {
            let out = self.out;
            self.set_unit(out);
        }
        let ok = self.ok_exit;
        self.b.ins().jump(ok, &[]);

        for (exit, status) in [(self.ok_exit, 0), (self.unwind_exit, 1)] {
            self.b.switch_to_block(exit);
            // `var` parameters go back to the caller's places, then the frame is released.
            for (i, k) in pass.iter().enumerate() {
                if var[i] && !k.scalar() {
                    let at = self.slot_addr(i as u32);
                    self.copy_value(at, args[i]);
                    self.set_unit(at);
                }
            }
            self.release_frame()?;
            self.b
                .ins()
                .store(MemFlagsData::trusted(), depth, depth_ptr, 0);
            let st = self.iconst(I32, status);
            let mut rets = vec![st];
            if let RetKind::Scalar(k) = self.ret {
                let v = match (status, self.ret_var) {
                    (0, Some(var)) => self.b.use_var(var),
                    _ => match k {
                        Kind::Float => self.b.ins().f64const(0.0),
                        _ => self.b.ins().iconst(k.ir(), 0),
                    },
                };
                rets.push(v);
            }
            self.b.ins().return_(&rets);
        }
        self.finish_deferred()?;
        self.b.seal_all_blocks();
        Ok(())
    }

    fn release_frame(&mut self) -> Result<(), String> {
        if self.frame_ss.is_none() {
            return Ok(());
        }
        let boxed: Vec<u32> = (0..self.nslots as u32)
            .filter(|i| !self.kinds[*i as usize].scalar())
            .collect();
        if boxed.len() > 6 {
            let p = self.frame_ptr();
            let n = self.iconst(I64, self.nslots as i64);
            self.h("clear", &[p, n])?;
        } else {
            for i in boxed {
                let at = self.slot_addr(i);
                self.emit_drop(at)?;
            }
        }
        Ok(())
    }

    /// A constant, a default, or a lambda body: its value into the result.
    fn expr_unit(&mut self, body: &Node) -> Result<(), String> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let params = self.b.block_params(entry).to_vec();
        self.cx = params[0];
        self.frame_param = Some(params[1]);
        self.out = params[2];
        self.types = params[3];
        let mut roots = Vec::new();
        self.var_roots(body, &mut roots);
        self.hazard.extend(roots);
        let v = self.expr(body)?;
        let out = self.out;
        self.store_into(v, out)?;
        let zero = self.iconst(I32, 0);
        self.b.ins().return_(&[zero]);
        let exit = self.unwind_exit;
        self.b.switch_to_block(exit);
        let one = self.iconst(I32, 1);
        self.b.ins().return_(&[one]);
        let ok = self.ok_exit;
        self.b.switch_to_block(ok);
        let zero = self.iconst(I32, 0);
        self.b.ins().return_(&[zero]);
        self.finish_deferred()?;
        self.b.seal_all_blocks();
        Ok(())
    }

    // ---------------------------------------------------------------- statements

    fn block(&mut self, block: &LBlock) -> Result<(), String> {
        self.scopes.push(Scope::Clear(block.clear.0, block.clear.1));
        for s in &block.stmts {
            self.line = s.line;
            if let Some(site) = self.site {
                let line = self.iconst(I32, s.line as i64);
                self.b.ins().store(MemFlagsData::trusted(), line, site, 4);
            }
            self.stmt(&s.kind)?;
        }
        self.scopes.pop();
        self.clear(block.clear.0, block.clear.1)
    }

    fn clear(&mut self, start: u32, end: u32) -> Result<(), String> {
        let boxed: Vec<u32> = (start..end)
            .filter(|i| (*i as usize) < self.kinds.len() && !self.kinds[*i as usize].scalar())
            .collect();
        if boxed.is_empty() {
            return Ok(());
        }
        if boxed.len() > 4 {
            let p = self.slot_addr(start);
            let n = self.iconst(I64, (end - start) as i64);
            self.h("clear", &[p, n])?;
            return Ok(());
        }
        for i in boxed {
            let at = self.slot_addr(i);
            self.emit_drop(at)?;
            self.set_unit(at);
        }
        Ok(())
    }

    /// Writes a value into a slot of the frame, releasing what it held.
    fn set_slot(&mut self, slot: u32, v: V) -> Result<(), String> {
        let k = self.kinds[slot as usize];
        if k.scalar() {
            let x = self.scalar(v, k)?;
            if let Some(var) = self.vars[slot as usize] {
                self.b.def_var(var, x);
            }
            return Ok(());
        }
        // A borrowed value is copied first: it may be the slot itself.
        let v = match v {
            V::Ref(_) => V::Own(self.own(v)?),
            other => other,
        };
        let at = self.slot_addr(slot);
        self.emit_drop(at)?;
        let at = self.slot_addr(slot);
        self.store_into(v, at)
    }

    fn stmt(&mut self, s: &S) -> Result<(), String> {
        match s {
            S::Let(slot, n) => {
                let v = self.expr(n)?;
                self.set_slot(*slot, v)
            }
            S::Assign(place, n) => self.assign(s, place, n),
            S::SetIndex(place, key, value) => {
                let (steps, sss) = self.index_steps(place)?;
                let k = self.expr(key)?;
                let (kp, ka) = self.give(k)?;
                let v = self.expr(value)?;
                let (vp, va) = self.give(v)?;
                let (cx, frame, n, sr) = (
                    self.cx,
                    self.frame_ptr(),
                    self.nslots_v(),
                    self.stmt_ref(s)?,
                );
                let st = self.hs("set_index", &[cx, frame, n, sr, steps, kp, vp])?;
                self.consumed(ka);
                self.consumed(va);
                self.steps_taken(sss);
                self.check(st)
            }
            S::Concat(slot, parts, general) => {
                // `x = x + a` is a concatenation only for a text; a number takes the
                // general form, as in the interpreter.
                if self.kinds[*slot as usize].scalar() {
                    let v = self.expr(general)?;
                    return self.set_slot(*slot, v);
                }
                let at = self.slot_addr(*slot);
                let tag = self.tag(at);
                let is_text = self
                    .b
                    .ins()
                    .icmp_imm_s(IntCC::Equal, tag, layout::TEXT as i64);
                let fast = self.b.create_block();
                let slow = self.b.create_block();
                let after = self.b.create_block();
                self.b.ins().brif(is_text, fast, &[], slow, &[]);
                self.b.switch_to_block(slow);
                let v = self.expr(general)?;
                self.set_slot(*slot, v)?;
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(fast);
                let nodes: Vec<&Node> = parts.iter().collect();
                let (p, ss, addrs) = self.array(&nodes)?;
                let at = self.slot_addr(*slot);
                let n = self.iconst(I64, parts.len() as i64);
                let cx = self.cx;
                let st = self.hs("concat", &[cx, at, p, n])?;
                self.array_taken(ss, &addrs, parts.len());
                self.check(st)?;
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(after);
                Ok(())
            }
            S::Return(e) => self.ret_stmt(e.as_ref()),
            S::Fail(e) => {
                let v = self.expr(e)?;
                let (p, a) = self.give(v)?;
                let cx = self.cx;
                self.hs("fail", &[cx, p])?;
                self.consumed(a);
                self.unwind_now()
            }
            S::Assert(e, _) => {
                let c = self.truth(e)?;
                let fail = self.b.create_block();
                let ok = self.b.create_block();
                self.b.set_cold_block(fail);
                self.b.ins().brif(c, ok, &[], fail, &[]);
                self.b.switch_to_block(fail);
                let (cx, sr) = (self.cx, self.stmt_ref(s)?);
                self.hs("assert_failed", &[cx, sr])?;
                let t = self.unwind_from(self.scopes.len(), &[], None)?;
                self.b.ins().jump(t, &[]);
                self.b.switch_to_block(ok);
                Ok(())
            }
            S::Break | S::Continue => {
                let Some(i) = self
                    .scopes
                    .iter()
                    .rposition(|s| matches!(s, Scope::Loop { .. }))
                else {
                    return Err(self.unsupported("break outside a loop"));
                };
                let (target, live) = match self.scopes[i] {
                    Scope::Loop { brk, cont, live } => {
                        (if matches!(s, S::Break) { brk } else { cont }, live)
                    }
                    _ => unreachable!("a loop"),
                };
                self.leave(i + 1, live, None, true)?;
                self.b.ins().jump(target, &[]);
                let dead = self.b.create_block();
                self.b.switch_to_block(dead);
                Ok(())
            }
            S::Pass => Ok(()),
            S::Expr(e) => {
                let v = self.expr(e)?;
                self.done(v)
            }
            S::Handled { inner, block, .. } => {
                let landing = self.b.create_block();
                let after = self.b.create_block();
                let live = self.live.len();
                self.scopes.push(Scope::Landing {
                    block: landing,
                    live,
                });
                self.stmt(inner)?;
                self.scopes.pop();
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(landing);
                let (cx, frame, n, sr) = (
                    self.cx,
                    self.frame_ptr(),
                    self.nslots_v(),
                    self.stmt_ref(s)?,
                );
                let zero = self.iconst(I64, 0);
                let caught = self.hs("catch", &[cx, frame, n, sr, zero])?;
                let handle = self.b.create_block();
                let pass = self.unwind_from(self.scopes.len(), &[], None)?;
                self.b.ins().brif(caught, handle, &[], pass, &[]);
                self.b.switch_to_block(handle);
                self.block(block)?;
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(after);
                Ok(())
            }
            S::If(branches, otherwise) => {
                let after = self.b.create_block();
                for (c, body) in branches {
                    let cond = self.truth(c)?;
                    let then = self.b.create_block();
                    let next = self.b.create_block();
                    self.b.ins().brif(cond, then, &[], next, &[]);
                    self.b.switch_to_block(then);
                    self.block(body)?;
                    self.b.ins().jump(after, &[]);
                    self.b.switch_to_block(next);
                }
                if let Some(b) = otherwise {
                    self.block(b)?;
                }
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(after);
                Ok(())
            }
            S::While(c, body) => {
                let header = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let cond = self.truth(c)?;
                let inside = self.b.create_block();
                self.b.ins().brif(cond, inside, &[], exit, &[]);
                self.b.switch_to_block(inside);
                let live = self.live.len();
                self.scopes.push(Scope::Loop {
                    brk: exit,
                    cont: header,
                    live,
                });
                self.block(body)?;
                self.scopes.pop();
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(exit);
                Ok(())
            }
            S::For(names, iterable, body) => self.for_stmt(s, names, iterable, body),
            S::Match(subject, cases) => {
                let v = self.expr(subject)?;
                let (p, v) = self.ptr(v)?;
                let after = self.b.create_block();
                for case in cases {
                    let (cx, frame, n) = (self.cx, self.frame_ptr(), self.nslots_v());
                    let pr = self.refp(Ref::Pat(&case.pattern))?;
                    let r = self.hs("match", &[cx, frame, n, pr, p])?;
                    let yes = self.b.create_block();
                    let no = self.b.create_block();
                    let fail = self.unwind_from(self.scopes.len(), &[], None)?;
                    let two = self.b.ins().icmp_imm_s(IntCC::Equal, r, 2);
                    let decided = self.b.create_block();
                    self.b.ins().brif(two, fail, &[], decided, &[]);
                    self.b.switch_to_block(decided);
                    self.b.ins().brif(r, yes, &[], no, &[]);
                    self.b.switch_to_block(yes);
                    if let Some(g) = &case.guard {
                        let c = self.truth(g)?;
                        let body = self.b.create_block();
                        self.b.ins().brif(c, body, &[], no, &[]);
                        self.b.switch_to_block(body);
                    }
                    self.block(&case.body)?;
                    self.b.ins().jump(after, &[]);
                    self.b.switch_to_block(no);
                }
                let cx = self.cx;
                let null = self.null();
                self.hs("fault_node", &[cx, null])?;
                let t = self.unwind_from(self.scopes.len(), &[], None)?;
                self.b.ins().jump(t, &[]);
                self.b.switch_to_block(after);
                self.done(v)
            }
            S::Try(body, handlers) => {
                let landing = self.b.create_block();
                let after = self.b.create_block();
                let live = self.live.len();
                self.scopes.push(Scope::Landing {
                    block: landing,
                    live,
                });
                self.block(body)?;
                self.scopes.pop();
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(landing);
                for (i, h) in handlers.iter().enumerate() {
                    let (cx, frame, n, sr) = (
                        self.cx,
                        self.frame_ptr(),
                        self.nslots_v(),
                        self.stmt_ref(s)?,
                    );
                    let idx = self.iconst(I64, i as i64);
                    let caught = self.hs("catch", &[cx, frame, n, sr, idx])?;
                    let handle = self.b.create_block();
                    let next = self.b.create_block();
                    self.b.ins().brif(caught, handle, &[], next, &[]);
                    self.b.switch_to_block(handle);
                    self.block(&h.body)?;
                    self.b.ins().jump(after, &[]);
                    self.b.switch_to_block(next);
                }
                let t = self.unwind_from(self.scopes.len(), &[], None)?;
                self.b.ins().jump(t, &[]);
                self.b.switch_to_block(after);
                Ok(())
            }
            S::Using(slot, value, body) => {
                if self.kinds[*slot as usize].scalar() {
                    return Err(self.unsupported("a using block on a number"));
                }
                let v = self.expr(value)?;
                let (vp, va) = self.give(v)?;
                let res = self.temp();
                let rp = self.addr(res);
                let at = self.slot_addr(*slot);
                let cx = self.cx;
                self.h("using_open", &[cx, vp, at, rp])?;
                self.consumed(va);
                self.push_live(res);
                let landing = self.b.create_block();
                let live = self.live.len();
                self.scopes.push(Scope::Using { landing, live, res });
                self.block(body)?;
                self.scopes.pop();
                // The block ended normally: the name is cleared, then the resource released.
                let at = self.slot_addr(*slot);
                self.emit_drop(at)?;
                self.set_unit(at);
                let rp = self.addr(res);
                let cx = self.cx;
                let st = self.hs("using_release", &[cx, rp])?;
                self.unlive(res);
                self.check(st)?;
                let after = self.b.create_block();
                self.b.ins().jump(after, &[]);
                // Unwinding through the block: release, then continue unwinding.
                self.b.switch_to_block(landing);
                self.push_live(res);
                let at = self.slot_addr(*slot);
                self.emit_drop(at)?;
                self.set_unit(at);
                let rp = self.addr(res);
                let cx = self.cx;
                self.h("using_unwind", &[cx, rp])?;
                self.unlive(res);
                let t = self.unwind_from(self.scopes.len(), &[], None)?;
                self.b.ins().jump(t, &[]);
                self.b.switch_to_block(after);
                self.release_stack(res.ss, 1);
                Ok(())
            }
        }
    }

    /// Leaves the scopes above `limit` for `break`, `continue` or `return`: open `using`
    /// blocks release their resources (a failure unwinds from there), blocks clear their
    /// slots, and what is live above `keep` is released.
    fn leave(
        &mut self,
        limit: usize,
        keep: usize,
        ret: Option<Addr>,
        clear: bool,
    ) -> Result<(), String> {
        let mut released: Vec<Addr> = Vec::new();
        let mut i = self.scopes.len();
        while i > limit {
            i -= 1;
            match self.scopes[i] {
                Scope::Clear(a, b) if clear => self.clear(a, b)?,
                Scope::Using { res, .. } => {
                    released.push(res);
                    let rp = self.addr(res);
                    let cx = self.cx;
                    let st = self.hs("using_release", &[cx, rp])?;
                    let bad = self.unwind_from(i, &released, ret)?;
                    let ok = self.b.create_block();
                    self.b.ins().brif(st, bad, &[], ok, &[]);
                    self.b.switch_to_block(ok);
                }
                _ => {}
            }
        }
        let items: Vec<Live> = self.live[keep.min(self.live.len())..]
            .iter()
            .filter(|l| !matches!(l, Live::Val(a) if released.contains(a)))
            .copied()
            .collect();
        for item in items.iter().rev() {
            match item {
                Live::Val(a) => {
                    let at = self.addr(*a);
                    self.emit_drop(at)?;
                }
                Live::Cursor(ss) => {
                    let at = self.b.ins().stack_addr(P, *ss, 0);
                    self.h("cursor_drop", &[at])?;
                }
            }
        }
        Ok(())
    }

    fn ret_stmt(&mut self, e: Option<&Node>) -> Result<(), String> {
        if self.site.is_none() {
            return Err(self.unsupported("return outside a function"));
        }
        let ok = self.ok_exit;
        match (self.ret, e) {
            (RetKind::Scalar(k), Some(e)) => {
                let v = self.expr(e)?;
                let x = self.scalar(v, k)?;
                if let Some(var) = self.ret_var {
                    self.b.def_var(var, x);
                }
                self.leave(0, 0, None, false)?;
            }
            (RetKind::Boxed, Some(e)) => {
                let v = self.expr(e)?;
                let a = self.own(v)?;
                self.unlive(a);
                self.leave(0, 0, Some(a), false)?;
                let from = self.addr(a);
                let out = self.out;
                self.copy_value(from, out);
                self.release_stack(a.ss, 1);
            }
            (RetKind::Boxed, None) => {
                self.leave(0, 0, None, false)?;
                let out = self.out;
                self.set_unit(out);
            }
            (_, Some(e)) => {
                let v = self.expr(e)?;
                self.done(v)?;
                self.leave(0, 0, None, false)?;
            }
            (_, None) => self.leave(0, 0, None, false)?,
        }
        self.b.ins().jump(ok, &[]);
        let dead = self.b.create_block();
        self.b.switch_to_block(dead);
        Ok(())
    }

    /// Evaluates the index steps of a place in order into an array; a null address without
    /// index steps.
    fn index_steps(&mut self, place: &Place) -> Result<(ir::Value, Option<Steps>), String> {
        let nodes: Vec<&Node> = place
            .steps
            .iter()
            .filter_map(|s| match s {
                PStep::Index(n) => Some(&**n),
                PStep::Field(_) => None,
            })
            .collect();
        if nodes.is_empty() {
            return Ok((self.null(), None));
        }
        let (p, ss, addrs) = self.array(&nodes)?;
        Ok((p, Some((ss, addrs, nodes.len()))))
    }

    fn steps_taken(&mut self, s: Option<Steps>) {
        if let Some((ss, addrs, n)) = s {
            self.array_taken(ss, &addrs, n);
        }
    }

    fn assign(&mut self, s: &S, place: &Place, n: &Node) -> Result<(), String> {
        let root = place.root;
        if place.steps.is_empty() {
            if self.kinds[root as usize].scalar() {
                let v = self.expr(n)?;
                return self.set_slot(root, v);
            }
            let v = self.expr(n)?;
            let a = self.own(v)?;
            // A name that holds a resource changes the resource's value (runtime 5).
            let at = self.slot_addr(root);
            let tag = self.tag(at);
            let is_resource = self
                .b
                .ins()
                .icmp_imm_s(IntCC::Equal, tag, layout::RESOURCE as i64);
            let general = self.b.create_block();
            let plain = self.b.create_block();
            let after = self.b.create_block();
            self.b.set_cold_block(general);
            self.b.ins().brif(is_resource, general, &[], plain, &[]);
            self.b.switch_to_block(plain);
            let at = self.slot_addr(root);
            self.emit_drop(at)?;
            let (from, at) = (self.addr(a), self.slot_addr(root));
            self.copy_value(from, at);
            self.b.ins().jump(after, &[]);
            self.b.switch_to_block(general);
            let vp = self.addr(a);
            let (cx, frame, nn, sr, null) = (
                self.cx,
                self.frame_ptr(),
                self.nslots_v(),
                self.stmt_ref(s)?,
                self.null(),
            );
            let st = self.hs("assign", &[cx, frame, nn, sr, null, vp])?;
            self.unlive(a);
            self.check(st)?;
            self.b.ins().jump(after, &[]);
            self.b.switch_to_block(after);
            self.consumed(a);
            return Ok(());
        }
        if place.steps.iter().all(|x| matches!(x, PStep::Field(_)))
            && !self.kinds[root as usize].scalar()
        {
            // Fields only: the value, then the field's place found in the frame's value, and
            // the value stored there; a name that holds a resource takes the general way.
            let v = self.expr(n)?;
            let a = self.own(v)?;
            let at = self.slot_addr(root);
            let tag = self.tag(at);
            let is_resource = self
                .b
                .ins()
                .icmp_imm_s(IntCC::Equal, tag, layout::RESOURCE as i64);
            let general = self.b.create_block();
            let plain = self.b.create_block();
            let after = self.b.create_block();
            self.b.set_cold_block(general);
            self.b.ins().brif(is_resource, general, &[], plain, &[]);
            self.b.switch_to_block(plain);
            let (cx, sr) = (self.cx, self.stmt_ref(s)?);
            let at = self.slot_addr(root);
            let target = self.hs("field_place", &[cx, at, sr])?;
            let bad = self.unwind_from(self.scopes.len(), &[], None)?;
            let found = self.b.create_block();
            self.b.ins().brif(target, found, &[], bad, &[]);
            self.b.switch_to_block(found);
            self.emit_drop(target)?;
            let from = self.addr(a);
            self.copy_value(from, target);
            self.b.ins().jump(after, &[]);
            self.b.switch_to_block(general);
            let vp = self.addr(a);
            let (cx, frame, nn, sr, null) = (
                self.cx,
                self.frame_ptr(),
                self.nslots_v(),
                self.stmt_ref(s)?,
                self.null(),
            );
            let st = self.hs("assign", &[cx, frame, nn, sr, null, vp])?;
            self.unlive(a);
            self.check(st)?;
            self.b.ins().jump(after, &[]);
            self.b.switch_to_block(after);
            self.consumed(a);
            return Ok(());
        }
        let (steps, sss) = self.index_steps(place)?;
        let v = self.expr(n)?;
        let (vp, va) = self.give(v)?;
        let (cx, frame, nn, sr) = (
            self.cx,
            self.frame_ptr(),
            self.nslots_v(),
            self.stmt_ref(s)?,
        );
        let st = self.hs("assign", &[cx, frame, nn, sr, steps, vp])?;
        self.consumed(va);
        self.steps_taken(sss);
        self.check(st)
    }

    fn for_stmt(
        &mut self,
        s: &S,
        names: &[u32],
        iterable: &Node,
        body: &LBlock,
    ) -> Result<(), String> {
        if let (Some((si, ei)), [slot]) = (self.is_range(iterable), names) {
            if self.kinds[*slot as usize] == Kind::Int {
                let Node::Call(call) = iterable else {
                    unreachable!("a call of range")
                };
                // Arguments in written order, as for any call.
                let mut vals = [None, None];
                for (i, a) in call.args.iter().enumerate() {
                    let v = self.expr(&a.value)?;
                    let x = self.scalar(v, Kind::Int)?;
                    if i == si {
                        vals[0] = Some(x);
                    } else if i == ei {
                        vals[1] = Some(x);
                    }
                }
                let (Some(start), Some(end)) = (vals[0], vals[1]) else {
                    return Err(self.unsupported("a range"));
                };
                let counter = self.b.declare_var(I64);
                self.b.def_var(counter, start);
                let header = self.b.create_block();
                let latch = self.b.create_block();
                let exit = self.b.create_block();
                let inside = self.b.create_block();
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let c = self.b.use_var(counter);
                let more = self.b.ins().icmp(IntCC::SignedLessThan, c, end);
                self.b.ins().brif(more, inside, &[], exit, &[]);
                self.b.switch_to_block(inside);
                if let Some(var) = self.vars[*slot as usize] {
                    self.b.def_var(var, c);
                }
                let live = self.live.len();
                self.scopes.push(Scope::Loop {
                    brk: exit,
                    cont: latch,
                    live,
                });
                self.block(body)?;
                self.scopes.pop();
                self.b.ins().jump(latch, &[]);
                self.b.switch_to_block(latch);
                let c = self.b.use_var(counter);
                let n = self.b.ins().iadd_imm_s(c, 1);
                self.b.def_var(counter, n);
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(exit);
                return Ok(());
            }
        }
        let v = self.expr(iterable)?;
        let (src, sa) = self.give(v)?;
        let cur = self.b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            CURSOR_SIZE as u32,
            3,
        ));
        let cp = self.b.ins().stack_addr(P, cur, 0);
        let cx = self.cx;
        let st = self.hs("cursor_init", &[cx, src, cp])?;
        self.consumed(sa);
        self.check(st)?;
        self.live.push(Live::Cursor(cur));
        let header = self.b.create_block();
        let exit = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        let item = self.temp();
        let ip = self.addr(item);
        let cp = self.b.ins().stack_addr(P, cur, 0);
        let cx = self.cx;
        let r = self.hs("cursor_next", &[cx, cp, ip])?;
        let fail = self.unwind_from(self.scopes.len(), &[], None)?;
        let two = self.b.ins().icmp_imm_s(IntCC::Equal, r, 2);
        let decided = self.b.create_block();
        let inside = self.b.create_block();
        self.b.ins().brif(two, fail, &[], decided, &[]);
        self.b.switch_to_block(decided);
        self.b.ins().brif(r, inside, &[], done, &[]);
        self.b.switch_to_block(inside);
        self.push_live(item);
        match names {
            [slot] if !self.kinds[*slot as usize].scalar() => {
                self.set_slot(*slot, V::Own(item))?;
            }
            _ => {
                let (cx, frame, n, sr, null) = (
                    self.cx,
                    self.frame_ptr(),
                    self.nslots_v(),
                    self.stmt_ref(s)?,
                    self.null(),
                );
                let ip = self.addr(item);
                let st = self.hs("bind", &[cx, frame, n, sr, null, ip])?;
                self.consumed(item);
                self.check(st)?;
            }
        }
        let live = self.live.len();
        self.scopes.push(Scope::Loop {
            brk: exit,
            cont: header,
            live,
        });
        self.block(body)?;
        self.scopes.pop();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(exit);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        let cp = self.b.ins().stack_addr(P, cur, 0);
        self.h("cursor_drop", &[cp])?;
        if let Some(i) = self.live.iter().rposition(|l| *l == Live::Cursor(cur)) {
            self.live.remove(i);
        }
        Ok(())
    }

    // ---------------------------------------------------------------- expressions

    fn truth(&mut self, n: &Node) -> Result<ir::Value, String> {
        match n {
            Node::And(l, r) => {
                let a = self.truth(l)?;
                let rhs = self.b.create_block();
                let after = self.b.create_block();
                self.b.append_block_param(after, I8);
                self.b.ins().brif(a, rhs, &[], after, &[BlockArg::Value(a)]);
                self.b.switch_to_block(rhs);
                let c = self.truth(r)?;
                self.b.ins().jump(after, &[BlockArg::Value(c)]);
                self.b.switch_to_block(after);
                Ok(self.b.block_params(after)[0])
            }
            Node::Or(l, r, _) if gives_bool(l) || self.kind(l) == Kind::Bool => {
                let a = self.truth(l)?;
                let rhs = self.b.create_block();
                let after = self.b.create_block();
                self.b.append_block_param(after, I8);
                self.b.ins().brif(a, after, &[BlockArg::Value(a)], rhs, &[]);
                self.b.switch_to_block(rhs);
                let c = self.truth(r)?;
                self.b.ins().jump(after, &[BlockArg::Value(c)]);
                self.b.switch_to_block(after);
                Ok(self.b.block_params(after)[0])
            }
            Node::Not(v) => {
                let a = self.truth(v)?;
                Ok(self.b.ins().icmp_imm_s(IntCC::Equal, a, 0))
            }
            _ => {
                let v = self.expr(n)?;
                self.truth_of(v)
            }
        }
    }

    fn expr(&mut self, n: &Node) -> Result<V, String> {
        match n {
            Node::Value(v) => Ok(match v {
                RValue::Int(i) => V::I(self.iconst(I64, *i)),
                RValue::Float(x) => V::F(self.b.ins().f64const(*x)),
                RValue::Bool(b) => V::B(self.iconst(I8, i64::from(*b))),
                RValue::Unit => V::Unit,
                other => V::Ref(self.refp(Ref::Value(other))?),
            }),
            Node::Local(slot) => {
                let k = self.kinds[*slot as usize];
                if let (true, Some(var)) = (k.scalar(), self.vars[*slot as usize]) {
                    let x = self.b.use_var(var);
                    return Ok(match k {
                        Kind::Int => V::I(x),
                        Kind::Float => V::F(x),
                        _ => V::B(x),
                    });
                }
                let at = self.slot_addr(*slot);
                if self.hazard.contains(slot) {
                    Ok(V::Own(self.own(V::Ref(at))?))
                } else {
                    Ok(V::Ref(at))
                }
            }
            Node::Const(c) => {
                let out = self.temp();
                let op = self.addr(out);
                let (cx, k) = (self.cx, self.iconst(I64, *c as i64));
                let st = self.hs("constant", &[cx, k, op])?;
                self.check(st)?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::FnRef(f) => {
                let out = self.temp();
                let op = self.addr(out);
                let (cx, k) = (self.cx, self.iconst(I64, *f as i64));
                self.h("fnref", &[cx, k, op])?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Variant(..) => {
                let out = self.temp();
                let op = self.addr(out);
                let (cx, nr) = (self.cx, self.node_ref(n)?);
                self.h("variant", &[cx, nr, op])?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Text(parts) => {
                let acc = self.temp();
                let ap = self.addr(acc);
                self.h("text_new", &[ap])?;
                self.push_live(acc);
                for p in parts {
                    match p {
                        crate::lower::Part::Literal(s) => {
                            if s.is_empty() {
                                continue;
                            }
                            let id = self.s.literal(s.as_bytes())?;
                            let gv = self.s.module.declare_data_in_func(id, self.b.func);
                            let lp = self.b.ins().symbol_value(P, gv);
                            let len = self.iconst(I64, s.len() as i64);
                            let ap = self.addr(acc);
                            self.h("text_literal", &[ap, lp, len])?;
                        }
                        crate::lower::Part::Value(e) => {
                            let v = self.expr(e)?;
                            if let V::I(x) = v {
                                let ap = self.addr(acc);
                                self.h("text_int", &[ap, x])?;
                                continue;
                            }
                            let (p, v) = self.ptr(v)?;
                            let ap = self.addr(acc);
                            let cx = self.cx;
                            let st = self.hs("text_value", &[cx, ap, p])?;
                            self.done(v)?;
                            self.check(st)?;
                        }
                    }
                }
                Ok(V::Own(acc))
            }
            Node::Tuple(items) | Node::List(items) => {
                let which = self.iconst(I32, i64::from(matches!(n, Node::List(_))));
                let nodes: Vec<&Node> = items.iter().collect();
                let (p, ss, addrs) = self.array(&nodes)?;
                let out = self.temp();
                let op = self.addr(out);
                let len = self.iconst(I64, items.len() as i64);
                self.h("sequence", &[which, p, len, op])?;
                self.array_taken(ss, &addrs, items.len());
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Map(entries) => {
                let nodes: Vec<&Node> = entries.iter().flat_map(|(k, v)| [k, v]).collect();
                let (p, ss, addrs) = self.array(&nodes)?;
                let out = self.temp();
                let op = self.addr(out);
                let len = self.iconst(I64, entries.len() as i64);
                self.h("map", &[p, len, op])?;
                self.array_taken(ss, &addrs, nodes.len());
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Comprehension(c) => self.comprehension(n, c),
            Node::Lambda(code) => {
                let entry = *self
                    .s
                    .lambda_entry
                    .get(&(std::rc::Rc::as_ptr(code) as usize))
                    .ok_or_else(|| self.unsupported("a lambda without a unit"))?;
                let locals: Vec<Node> = code
                    .captures
                    .iter()
                    .map(|(outer, _)| Node::Local(*outer))
                    .collect();
                let nodes: Vec<&Node> = locals.iter().collect();
                let (p, ss, addrs) = if nodes.is_empty() {
                    let p = self.null();
                    (p, None, Vec::new())
                } else {
                    let (p, ss, addrs) = self.array(&nodes)?;
                    (p, Some(ss), addrs)
                };
                let r = self.fref(entry);
                let fp = self.b.ins().func_addr(P, r);
                let out = self.temp();
                let op = self.addr(out);
                let (cx, nr, types) = (self.cx, self.node_ref(n)?, self.types);
                self.h("closure", &[cx, nr, fp, p, types, op])?;
                if let Some(ss) = ss {
                    self.array_taken(ss, &addrs, nodes.len());
                }
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Is(value, pat) => {
                if let (Pat::Wildcard, _) = (&**pat, ()) {
                    let v = self.expr(value)?;
                    self.done(v)?;
                    return Ok(V::B(self.iconst(I8, 1)));
                }
                let v = self.expr(value)?;
                let (p, v) = self.ptr(v)?;
                let (cx, frame, nn) = (self.cx, self.frame_ptr(), self.nslots_v());
                let pr = self.refp(Ref::Pat(&**pat))?;
                let r = self.hs("match", &[cx, frame, nn, pr, p])?;
                self.done(v)?;
                let fail = self.unwind_from(self.scopes.len(), &[], None)?;
                let two = self.b.ins().icmp_imm_s(IntCC::Equal, r, 2);
                let ok = self.b.create_block();
                self.b.ins().brif(two, fail, &[], ok, &[]);
                self.b.switch_to_block(ok);
                let x = self.b.ins().icmp_imm_s(IntCC::Equal, r, 1);
                Ok(V::B(x))
            }
            Node::Conditional(parts) => {
                let [c, t, o] = &**parts;
                let cond = self.truth(c)?;
                let k = self.kind(n);
                let then = self.b.create_block();
                let other = self.b.create_block();
                let after = self.b.create_block();
                self.b.ins().brif(cond, then, &[], other, &[]);
                if k.scalar() {
                    self.b.append_block_param(after, k.ir());
                    for (block, e) in [(then, t), (other, o)] {
                        self.b.switch_to_block(block);
                        let v = self.expr(e)?;
                        let x = self.scalar(v, k)?;
                        self.b.ins().jump(after, &[BlockArg::Value(x)]);
                    }
                    self.b.switch_to_block(after);
                    let x = self.b.block_params(after)[0];
                    return Ok(match k {
                        Kind::Int => V::I(x),
                        Kind::Float => V::F(x),
                        _ => V::B(x),
                    });
                }
                let res = self.temp();
                for (block, e) in [(then, t), (other, o)] {
                    self.b.switch_to_block(block);
                    let v = self.expr(e)?;
                    let at = self.addr(res);
                    self.store_into(v, at)?;
                    self.b.ins().jump(after, &[]);
                }
                self.b.switch_to_block(after);
                self.push_live(res);
                Ok(V::Own(res))
            }
            Node::And(..) => Ok(V::B(self.truth(n)?)),
            Node::Or(l, r, yields) => {
                if gives_bool(l) || self.kind(l) == Kind::Bool {
                    return Ok(V::B(self.truth(n)?));
                }
                if let (Node::Method(m), false) = (&**l, *yields) {
                    if m.name == "get" && m.args.len() == 1 && m.args[0].name.is_none() {
                        return self.get_or(l, m, r);
                    }
                }
                let lv = self.expr(l)?;
                let (lp, la) = self.give(lv)?;
                let res = self.temp();
                let rp = self.addr(res);
                let (cx, y) = (self.cx, self.iconst(I32, i64::from(*yields)));
                let st = self.hs("or", &[cx, lp, y, rp])?;
                self.consumed(la);
                let right = self.b.create_block();
                let after = self.b.create_block();
                let decided = self.b.create_block();
                let fail = self.unwind_from(self.scopes.len(), &[], None)?;
                let two = self.b.ins().icmp_imm_s(IntCC::Equal, st, 2);
                self.b.ins().brif(two, fail, &[], decided, &[]);
                self.b.switch_to_block(decided);
                self.b.ins().brif(st, right, &[], after, &[]);
                self.b.switch_to_block(right);
                let v = self.expr(r)?;
                let at = self.addr(res);
                self.store_into(v, at)?;
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(after);
                self.push_live(res);
                Ok(V::Own(res))
            }
            Node::Not(v) => {
                if self.kind(v) == Kind::Bool {
                    return Ok(V::B(self.truth(n)?));
                }
                let x = self.expr(v)?;
                self.unary("not", x)
            }
            Node::Negate(v) => {
                let x = self.expr(v)?;
                match x {
                    V::I(i) => {
                        let zero = self.iconst(I64, 0);
                        let (r, of) = self.b.ins().ssub_overflow(zero, i);
                        self.overflow_check(of, 1)?;
                        Ok(V::I(r))
                    }
                    V::F(f) => Ok(V::F(self.b.ins().fneg(f))),
                    other => self.unary("negate", other),
                }
            }
            Node::Binary(op, l, r) => self.binary(*op, l, r),
            Node::Try(v, is_value) => {
                let x = self.expr(v)?;
                let (p, a) = self.give(x)?;
                let out = self.temp();
                let op = self.addr(out);
                let (cx, iv) = (self.cx, self.iconst(I32, i64::from(*is_value)));
                let st = self.hs("try", &[cx, p, iv, op])?;
                self.consumed(a);
                self.check(st)?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Call(_) => self.call(n),
            Node::CallValue(callee, args) => {
                let f = self.expr(callee)?;
                let (fp, fa) = self.give(f)?;
                let nodes: Vec<&Node> = args.iter().collect();
                let (p, ss, addrs) = self.array(&nodes)?;
                let out = self.temp();
                let op = self.addr(out);
                let (cx, len) = (self.cx, self.iconst(I64, args.len() as i64));
                let st = self.hs("call_value", &[cx, fp, p, len, op])?;
                self.consumed(fa);
                self.array_taken(ss, &addrs, args.len());
                self.check(st)?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Construct(c) => {
                let nodes: Vec<&Node> = c.args.iter().map(|(_, v)| v).collect();
                let (p, ss, addrs) = self.array(&nodes)?;
                let out = self.temp();
                let op = self.addr(out);
                let (cx, nr) = (self.cx, self.node_ref(n)?);
                let st = self.hs("construct", &[cx, nr, p, op])?;
                self.array_taken(ss, &addrs, nodes.len());
                self.check(st)?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Method(m) => self.method(n, m),
            Node::Index(t, i) => {
                let tv = self.expr(t)?;
                let iv = self.expr(i)?;
                let (tp, tv) = self.ptr(tv)?;
                let (ip, iv) = self.ptr(iv)?;
                let out = self.temp();
                let op = self.addr(out);
                let cx = self.cx;
                let st = self.hs("index", &[cx, tp, ip, op])?;
                self.done(iv)?;
                self.done(tv)?;
                self.check(st)?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Field(t, _, _) => {
                let tv = self.expr(t)?;
                if let V::Ref(tp) = tv {
                    // A field of a value the frame holds is read in place.
                    let (cx, nr) = (self.cx, self.node_ref(n)?);
                    let p = self.hs("field_ref", &[cx, nr, tp])?;
                    let bad = self.unwind_from(self.scopes.len(), &[], None)?;
                    let ok = self.b.create_block();
                    self.b.ins().brif(p, ok, &[], bad, &[]);
                    self.b.switch_to_block(ok);
                    return Ok(V::Ref(p));
                }
                let (tp, tv) = self.ptr(tv)?;
                let out = self.temp();
                let op = self.addr(out);
                let (cx, nr) = (self.cx, self.node_ref(n)?);
                let st = self.hs("field", &[cx, nr, tp, op])?;
                self.done(tv)?;
                self.check(st)?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::SafeField(t, _) => {
                let tv = self.expr(t)?;
                let (tp, ta) = self.give(tv)?;
                let out = self.temp();
                let op = self.addr(out);
                let (cx, nr) = (self.cx, self.node_ref(n)?);
                let st = self.hs("safe_field", &[cx, nr, tp, op])?;
                self.consumed(ta);
                self.check(st)?;
                self.push_live(out);
                Ok(V::Own(out))
            }
            Node::Fault(_) => {
                let (cx, nr) = (self.cx, self.node_ref(n)?);
                self.hs("fault_node", &[cx, nr])?;
                self.unwind_now()?;
                Ok(V::Unit)
            }
        }
    }

    /// `items.get(i) or right`: the list's item without an `Option` (runtime 8.1).
    fn get_or(&mut self, l: &Node, m: &crate::lower::MethodCode, r: &Node) -> Result<V, String> {
        let mut roots = Vec::new();
        self.var_roots(&m.args[0].value, &mut roots);
        let (recv, owned) = match &m.receiver {
            Node::Local(slot) if !self.kinds[*slot as usize].scalar() && !roots.contains(slot) => {
                (Err(self.slot_addr(*slot)), false)
            }
            other => {
                let v = self.expr(other)?;
                match v {
                    V::Ref(p) if roots.is_empty() => (Err(p), false),
                    v => (Ok(self.own(v)?), true),
                }
            }
        };
        let i = self.expr(&m.args[0].value)?;
        let (ip, ia) = self.give(i)?;
        let rp = match recv {
            Ok(a) => self.addr(a),
            Err(p) => p,
        };
        let res = self.temp();
        let op = self.addr(res);
        let (cx, frame, nn, nr) = (
            self.cx,
            self.frame_ptr(),
            self.nslots_v(),
            self.node_ref(l)?,
        );
        let ow = self.iconst(I32, i64::from(owned));
        let st = self.hs("get_or", &[cx, frame, nn, nr, rp, ow, ip, op])?;
        if let Ok(a) = recv {
            self.consumed(a);
        }
        self.consumed(ia);
        let right = self.b.create_block();
        let after = self.b.create_block();
        let decided = self.b.create_block();
        let fail = self.unwind_from(self.scopes.len(), &[], None)?;
        let two = self.b.ins().icmp_imm_s(IntCC::Equal, st, 2);
        self.b.ins().brif(two, fail, &[], decided, &[]);
        self.b.switch_to_block(decided);
        self.b.ins().brif(st, right, &[], after, &[]);
        self.b.switch_to_block(right);
        let v = self.expr(r)?;
        let at = self.addr(res);
        self.store_into(v, at)?;
        self.b.ins().jump(after, &[]);
        self.b.switch_to_block(after);
        self.push_live(res);
        Ok(V::Own(res))
    }

    fn unary(&mut self, name: &'static str, x: V) -> Result<V, String> {
        let (p, a) = self.give(x)?;
        let out = self.temp();
        let op = self.addr(out);
        let cx = self.cx;
        let st = self.hs(name, &[cx, p, op])?;
        self.consumed(a);
        self.check(st)?;
        self.push_live(out);
        Ok(V::Own(out))
    }

    fn overflow_check(&mut self, of: ir::Value, which: i64) -> Result<(), String> {
        let bad = self.b.create_block();
        let ok = self.b.create_block();
        self.b.set_cold_block(bad);
        self.b.ins().brif(of, bad, &[], ok, &[]);
        self.b.switch_to_block(bad);
        let (cx, w) = (self.cx, self.iconst(I32, which));
        self.hs("int_fault", &[cx, w])?;
        let t = self.unwind_from(self.scopes.len(), &[], None)?;
        self.b.ins().jump(t, &[]);
        self.b.switch_to_block(ok);
        Ok(())
    }

    fn int_fault_if(&mut self, cond: ir::Value, which: i64) -> Result<(), String> {
        self.overflow_check(cond, which)
    }

    /// An operator on two integers in registers.
    fn int_op(
        &mut self,
        op: Op,
        a: ir::Value,
        b: ir::Value,
        b_const: Option<i64>,
    ) -> Result<V, String> {
        Ok(match op {
            Op::Add => {
                let (r, of) = self.b.ins().sadd_overflow(a, b);
                self.overflow_check(of, 0)?;
                V::I(r)
            }
            Op::Subtract => {
                let (r, of) = self.b.ins().ssub_overflow(a, b);
                self.overflow_check(of, 1)?;
                V::I(r)
            }
            Op::Multiply => {
                let (r, of) = self.b.ins().smul_overflow(a, b);
                self.overflow_check(of, 2)?;
                V::I(r)
            }
            Op::Divide => {
                if b_const.is_none_or(|c| c == 0) {
                    let zero = self.b.ins().icmp_imm_s(IntCC::Equal, b, 0);
                    self.int_fault_if(zero, 4)?;
                }
                if b_const.is_none_or(|c| c == -1) {
                    let m1 = self.b.ins().icmp_imm_s(IntCC::Equal, b, -1);
                    let min = self.b.ins().icmp_imm_s(IntCC::Equal, a, i64::MIN);
                    let both = self.b.ins().band(m1, min);
                    self.int_fault_if(both, 3)?;
                }
                V::I(self.b.ins().sdiv(a, b))
            }
            Op::Remainder => {
                if b_const.is_none_or(|c| c == 0) {
                    let zero = self.b.ins().icmp_imm_s(IntCC::Equal, b, 0);
                    self.int_fault_if(zero, 5)?;
                }
                if b_const.is_none_or(|c| c == -1) {
                    // x % -1 is 0, and MINIMUM_INT % -1 must not trap.
                    let m1 = self.b.ins().icmp_imm_s(IntCC::Equal, b, -1);
                    let one = self.iconst(I64, 1);
                    let safe = self.b.ins().select(m1, one, b);
                    let r = self.b.ins().srem(a, safe);
                    let zero = self.iconst(I64, 0);
                    V::I(self.b.ins().select(m1, zero, r))
                } else {
                    V::I(self.b.ins().srem(a, b))
                }
            }
            Op::Power => {
                let slot = self.b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let p = self.b.ins().stack_addr(P, slot, 0);
                let cx = self.cx;
                let st = self.hs("int_power", &[cx, a, b, p])?;
                self.check(st)?;
                V::I(self.b.ins().load(I64, MemFlagsData::trusted(), p, 0))
            }
            Op::Equal
            | Op::NotEqual
            | Op::Less
            | Op::LessEqual
            | Op::Greater
            | Op::GreaterEqual => {
                let cc = match op {
                    Op::Equal => IntCC::Equal,
                    Op::NotEqual => IntCC::NotEqual,
                    Op::Less => IntCC::SignedLessThan,
                    Op::LessEqual => IntCC::SignedLessThanOrEqual,
                    Op::Greater => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                V::B(self.b.ins().icmp(cc, a, b))
            }
            Op::In | Op::NotIn => return Err(self.unsupported("in on Int")),
        })
    }

    fn float_op(&mut self, op: Op, a: ir::Value, b: ir::Value) -> Result<V, String> {
        Ok(match op {
            Op::Add => V::F(self.b.ins().fadd(a, b)),
            Op::Subtract => V::F(self.b.ins().fsub(a, b)),
            Op::Multiply => V::F(self.b.ins().fmul(a, b)),
            Op::Divide => V::F(self.b.ins().fdiv(a, b)),
            Op::Remainder => V::F(self.h("float_rem", &[a, b])?.ok_or("internal: float_rem")?),
            Op::Power => V::F(self.h("float_pow", &[a, b])?.ok_or("internal: float_pow")?),
            // IEEE 754 comparisons: with NaN every ordering is false (runtime 2).
            Op::Equal => V::B(self.b.ins().fcmp(FloatCC::Equal, a, b)),
            Op::NotEqual => V::B(self.b.ins().fcmp(FloatCC::NotEqual, a, b)),
            Op::Less => V::B(self.b.ins().fcmp(FloatCC::LessThan, a, b)),
            Op::LessEqual => V::B(self.b.ins().fcmp(FloatCC::LessThanOrEqual, a, b)),
            Op::Greater => V::B(self.b.ins().fcmp(FloatCC::GreaterThan, a, b)),
            Op::GreaterEqual => V::B(self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, a, b)),
            Op::In | Op::NotIn => return Err(self.unsupported("in on Float")),
        })
    }

    fn binary(&mut self, op: Op, l: &Node, r: &Node) -> Result<V, String> {
        let (kl, kr) = (self.kind(l), self.kind(r));
        let comparison = matches!(
            op,
            Op::Equal | Op::NotEqual | Op::Less | Op::LessEqual | Op::Greater | Op::GreaterEqual
        );
        if kl == Kind::Int && kr == Kind::Int && !matches!(op, Op::In | Op::NotIn) {
            let a = self.expr(l)?;
            let a = self.scalar(a, Kind::Int)?;
            let b_const = match r {
                Node::Value(RValue::Int(c)) => Some(*c),
                _ => None,
            };
            let b = self.expr(r)?;
            let b = self.scalar(b, Kind::Int)?;
            return self.int_op(op, a, b, b_const);
        }
        if kl == Kind::Float && kr == Kind::Float && !matches!(op, Op::In | Op::NotIn) {
            let a = self.expr(l)?;
            let a = self.scalar(a, Kind::Float)?;
            let b = self.expr(r)?;
            let b = self.scalar(b, Kind::Float)?;
            return self.float_op(op, a, b);
        }
        if kl == Kind::Bool && kr == Kind::Bool && matches!(op, Op::Equal | Op::NotEqual) {
            let a = self.expr(l)?;
            let a = self.scalar(a, Kind::Bool)?;
            let b = self.expr(r)?;
            let b = self.scalar(b, Kind::Bool)?;
            let cc = if op == Op::Equal {
                IntCC::Equal
            } else {
                IntCC::NotEqual
            };
            return Ok(V::B(self.b.ins().icmp(cc, a, b)));
        }
        let a = self.expr(l)?;
        let b = self.expr(r)?;
        let (pa, a) = self.ptr(a)?;
        let (pb, b) = self.ptr(b)?;
        let res = self.temp();
        let after = self.b.create_block();
        let boolean = comparison || matches!(op, Op::In | Op::NotIn);
        if boolean {
            self.b.append_block_param(after, I8);
        }
        let live_before = self.live.clone();
        let opn = self.iconst(I32, op_number(op) as i64);
        if boolean {
            // Values whose comparison no program defines are compared where they are.
            let r = self.hs("compare", &[opn, pa, pb])?;
            let unknown = self.b.ins().icmp_imm_s(IntCC::Equal, r, 2);
            let known = self.b.create_block();
            let slow = self.b.create_block();
            self.b.ins().brif(unknown, slow, &[], known, &[]);
            self.b.switch_to_block(known);
            for x in [a, b] {
                if let V::Own(t) = x {
                    let at = self.addr(t);
                    self.emit_drop(at)?;
                }
            }
            let x = self.b.ins().ireduce(I8, r);
            self.b.ins().jump(after, &[BlockArg::Value(x)]);
            self.b.switch_to_block(slow);
        } else if matches!(op, Op::Add | Op::Subtract | Op::Multiply) {
            // Integers whose type the code does not know: a test of both tags, then the
            // instruction.
            let ta = self.tag(pa);
            let tb = self.tag(pb);
            let ia = self
                .b
                .ins()
                .icmp_imm_s(IntCC::Equal, ta, layout::INT as i64);
            let ib = self
                .b
                .ins()
                .icmp_imm_s(IntCC::Equal, tb, layout::INT as i64);
            let both = self.b.ins().band(ia, ib);
            let fast = self.b.create_block();
            let slow = self.b.create_block();
            self.b.ins().brif(both, fast, &[], slow, &[]);
            self.b.switch_to_block(fast);
            let x = self
                .b
                .ins()
                .load(I64, MemFlagsData::trusted(), pa, layout::PAYLOAD);
            let y = self
                .b
                .ins()
                .load(I64, MemFlagsData::trusted(), pb, layout::PAYLOAD);
            let v = self.int_op(op, x, y, None)?;
            let at = self.addr(res);
            self.store_into(v, at)?;
            // The operands are integers: their temporaries hold nothing to release.
            self.b.ins().jump(after, &[]);
            self.b.switch_to_block(slow);
        }
        // The runtime takes both operands: borrowed ones are copied first.
        let mut copies = Vec::new();
        let mut give = |g: &mut Self, v: V, p: ir::Value| -> Result<Addr, String> {
            Ok(match v {
                V::Own(x) => x,
                _ => {
                    let t = g.temp();
                    let at = g.addr(t);
                    g.emit_clone(p, at)?;
                    g.push_live(t);
                    copies.push(t);
                    t
                }
            })
        };
        let ga = give(self, a, pa)?;
        let gb = give(self, b, pb)?;
        let (pa2, pb2, ro) = (self.addr(ga), self.addr(gb), self.addr(res));
        let cx = self.cx;
        let st = self.hs("binary", &[cx, opn, pa2, pb2, ro])?;
        self.unlive(ga);
        self.unlive(gb);
        self.check(st)?;
        if boolean {
            self.push_live(res);
            let x = self.truth_of(V::Own(res))?;
            self.b.ins().jump(after, &[BlockArg::Value(x)]);
        } else {
            self.b.ins().jump(after, &[]);
        }
        self.b.switch_to_block(after);
        // Every path ends with the operands released and the result in `res`.
        self.live = live_before;
        for x in [a, b] {
            if let V::Own(t) = x {
                self.consumed(t);
            }
        }
        for t in copies {
            self.release_stack(t.ss, 1);
        }
        if boolean {
            // `res` went back to the pool when the slow path read its Bool.
            return Ok(V::B(self.b.block_params(after)[0]));
        }
        self.push_live(res);
        Ok(V::Own(res))
    }

    fn comprehension(
        &mut self,
        n: &Node,
        c: &crate::lower::ComprehensionCode,
    ) -> Result<V, String> {
        let acc = self.temp();
        let ap = self.addr(acc);
        let (which, null, zero) = (self.iconst(I32, 1), self.null(), self.iconst(I64, 0));
        self.h("sequence", &[which, null, zero, ap])?;
        self.push_live(acc);
        let v = self.expr(&c.iterable)?;
        let (src, sa) = self.give(v)?;
        let cur = self.b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            CURSOR_SIZE as u32,
            3,
        ));
        let cp = self.b.ins().stack_addr(P, cur, 0);
        let cx = self.cx;
        let st = self.hs("cursor_init", &[cx, src, cp])?;
        self.consumed(sa);
        self.check(st)?;
        self.live.push(Live::Cursor(cur));
        let header = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        let item = self.temp();
        let ip = self.addr(item);
        let cp = self.b.ins().stack_addr(P, cur, 0);
        let cx = self.cx;
        let r = self.hs("cursor_next", &[cx, cp, ip])?;
        let fail = self.unwind_from(self.scopes.len(), &[], None)?;
        let two = self.b.ins().icmp_imm_s(IntCC::Equal, r, 2);
        let decided = self.b.create_block();
        let inside = self.b.create_block();
        self.b.ins().brif(two, fail, &[], decided, &[]);
        self.b.switch_to_block(decided);
        self.b.ins().brif(r, inside, &[], done, &[]);
        self.b.switch_to_block(inside);
        let (cx, frame, nn, null, nr) = (
            self.cx,
            self.frame_ptr(),
            self.nslots_v(),
            self.null(),
            self.node_ref(n)?,
        );
        let ip = self.addr(item);
        let st = self.hs("bind", &[cx, frame, nn, null, nr, ip])?;
        self.release_stack(item.ss, 1);
        self.check(st)?;
        if let Some(cond) = &c.condition {
            let keep = self.truth(cond)?;
            let add = self.b.create_block();
            self.b.ins().brif(keep, add, &[], header, &[]);
            self.b.switch_to_block(add);
        }
        let v = self.expr(&c.element)?;
        let (vp, va) = self.give(v)?;
        let ap = self.addr(acc);
        self.h("list_push", &[ap, vp])?;
        self.consumed(va);
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(done);
        let cp = self.b.ins().stack_addr(P, cur, 0);
        self.h("cursor_drop", &[cp])?;
        if let Some(i) = self.live.iter().rposition(|l| *l == Live::Cursor(cur)) {
            self.live.remove(i);
        }
        for slot in &c.names {
            if (*slot as usize) < self.kinds.len() && !self.kinds[*slot as usize].scalar() {
                let at = self.slot_addr(*slot);
                self.emit_drop(at)?;
                self.set_unit(at);
            }
        }
        let (cx, ap) = (self.cx, self.addr(acc));
        let st = self.hs("list_done", &[cx, ap])?;
        self.check(st)?;
        Ok(V::Own(acc))
    }

    fn method(&mut self, n: &Node, m: &crate::lower::MethodCode) -> Result<V, String> {
        let facts = &self.s.facts;
        let var_self = facts.var_self.contains(&m.name);
        let var_param = facts.var_param.contains(&m.name);
        if var_param
            && m.args
                .iter()
                .any(|a| a.place.as_ref().is_some_and(|p| index_steps(p) > 0))
        {
            return Err(
                self.unsupported(&format!("an indexed place as a var argument of {}", m.name))
            );
        }
        let place_index = m.place.as_ref().map_or(0, index_steps);
        if var_self && place_index > 0 {
            let impure = m.place.as_ref().is_some_and(|p| {
                p.steps.iter().any(|s| match s {
                    PStep::Index(n) => !pure(n),
                    PStep::Field(_) => false,
                })
            });
            if impure {
                return Err(self.unsupported(&format!(
                    "{} on an indexed place whose index has effects",
                    m.name
                )));
            }
        }
        // The receiver: a local name is read in place after the arguments, as the
        // interpreter does; an argument that may change that name is not compiled yet.
        let (recv, owned) = match &m.receiver {
            Node::Local(slot) if !self.kinds[*slot as usize].scalar() => {
                let mut roots = Vec::new();
                for a in &m.args {
                    self.var_roots(&a.value, &mut roots);
                }
                if roots.contains(slot) {
                    return Err(self.unsupported(&format!(
                        "an argument of {} that changes its receiver",
                        m.name
                    )));
                }
                (None, false)
            }
            other => {
                let v = self.expr(other)?;
                let mut roots = Vec::new();
                for a in &m.args {
                    self.var_roots(&a.value, &mut roots);
                }
                match v {
                    V::Ref(p) if roots.is_empty() => (Some(Err(p)), false),
                    v => (Some(Ok(self.own(v)?)), true),
                }
            }
        };
        let nodes: Vec<&Node> = m.args.iter().map(|a| &a.value).collect();
        let (ap, ass, aaddrs) = if nodes.is_empty() {
            (self.null(), None, Vec::new())
        } else {
            let (p, ss, addrs) = self.array(&nodes)?;
            (p, Some(ss), addrs)
        };
        let (steps, sss) = match (&m.place, var_self && place_index > 0) {
            (Some(place), true) => self.index_steps(place)?,
            _ => (self.null(), None),
        };
        let rp = match (recv, &m.receiver) {
            (Some(Ok(a)), _) => self.addr(a),
            (Some(Err(p)), _) => p,
            (None, Node::Local(slot)) => self.slot_addr(*slot),
            _ => self.null(),
        };
        let out = self.temp();
        let op = self.addr(out);
        let (cx, frame, nn, nr) = (
            self.cx,
            self.frame_ptr(),
            self.nslots_v(),
            self.node_ref(n)?,
        );
        let ow = self.iconst(I32, i64::from(owned));
        let st = self.hs("method", &[cx, frame, nn, nr, rp, ow, ap, steps, op])?;
        if let Some(Ok(a)) = recv {
            self.consumed(a);
        }
        if let Some(ss) = ass {
            self.array_taken(ss, &aaddrs, nodes.len());
        }
        self.steps_taken(sss);
        self.check(st)?;
        self.push_live(out);
        Ok(V::Own(out))
    }

    fn call(&mut self, n: &Node) -> Result<V, String> {
        let Node::Call(call) = n else {
            return Err(self.unsupported("a call"));
        };
        let interp = self.s.interp;
        let def = &interp.program.funcs[call.f];
        let offset = usize::from(def.self_param.is_some());
        let direct = def.body.is_some()
            && call.types.is_empty()
            && interp.intrinsics[call.f].is_none()
            && self.s.infos.contains_key(&call.f);
        let is_var = |target: usize| {
            if target < offset {
                def.self_param == Some(true)
            } else {
                def.params.get(target - offset).is_some_and(|p| p.var)
            }
        };
        // Arguments in written order; a `var` argument is moved out of its place.
        let mut values: Vec<Option<V>> = vec![None; call.count];
        let mut taken: Vec<(usize, usize, Option<Variable>)> = Vec::new();
        for (ai, a) in call.args.iter().enumerate() {
            match (&a.place, is_var(a.target)) {
                (Some(place), true) => {
                    let (t, flag) = self.take_place(n, ai, place)?;
                    values[a.target] = Some(V::Own(t));
                    taken.push((ai, a.target, flag));
                }
                _ => {
                    let v = self.expr(&a.value)?;
                    // A later argument may run code; a borrowed value is copied now.
                    let v = match v {
                        V::Ref(_) => V::Own(self.own(v)?),
                        other => other,
                    };
                    values[a.target] = Some(v);
                }
            }
        }
        if direct {
            let info = &self.s.infos[&call.f];
            let (callee, pass, ret) = (info.direct, info.pass.clone(), info.ret);
            let out = self.temp();
            let op = self.addr(out);
            let null = self.null();
            let mut args = vec![self.cx, op, null];
            let mut given = Vec::new();
            let mut back: Vec<(usize, Addr, Option<Variable>)> = Vec::new();
            for (i, k) in pass.iter().enumerate() {
                let v = values[i].take().unwrap_or(V::Unit);
                if k.scalar() {
                    let x = self.scalar(v, *k)?;
                    args.push(x);
                } else {
                    let a = self.own(v)?;
                    let p = self.addr(a);
                    args.push(p);
                    match taken.iter().find(|t| t.1 == i) {
                        Some((ai, _, flag)) => back.push((*ai, a, *flag)),
                        None => given.push(a),
                    }
                }
            }
            let r = self.fref(callee);
            let inst = self.b.ins().call(r, &args);
            let results = self.b.inst_results(inst).to_vec();
            for a in given {
                self.consumed(a);
            }
            let status = results[0];
            let boxed_out = matches!(ret, RetKind::Boxed).then_some(out);
            self.put_back(n, &back, status, boxed_out)?;
            for (_, a, _) in &back {
                self.release_stack(a.ss, 1);
            }
            self.check(status)?;
            return Ok(match ret {
                RetKind::Boxed => {
                    self.push_live(out);
                    V::Own(out)
                }
                other => {
                    self.release_stack(out.ss, 1);
                    match other {
                        RetKind::Scalar(Kind::Int) => V::I(results[1]),
                        RetKind::Scalar(Kind::Float) => V::F(results[1]),
                        RetKind::Scalar(_) => V::B(results[1]),
                        _ => V::Unit,
                    }
                }
            });
        }
        // Through the runtime: the arguments in parameter order in an array.
        let count = call.count;
        let ss = self.stack(count);
        let mut addrs = Vec::new();
        for (i, value) in values.iter_mut().enumerate() {
            let a = Addr {
                ss,
                off: i as i32 * VSIZE,
                single: false,
            };
            let at = self.addr(a);
            match value.take() {
                Some(v) => self.store_into(v, at)?,
                None => self.set_unit(at),
            }
            self.push_live(a);
            addrs.push(a);
        }
        let p = self.b.ins().stack_addr(P, ss, 0);
        let out = self.temp();
        let op = self.addr(out);
        let cx = self.cx;
        let len = self.iconst(I64, count as i64);
        let st = if call.types.is_empty() {
            let (f, null) = (self.iconst(I64, call.f as i64), self.null());
            self.hs("call", &[cx, f, p, len, null, op])?
        } else {
            let (nr, module, types) = (
                self.node_ref(n)?,
                self.iconst(I64, self.unit.module as i64),
                self.types,
            );
            self.hs("call_typed", &[cx, nr, module, types, p, len, op])?
        };
        // `var` arguments go back; the rest of the array is released.
        let back: Vec<(usize, Addr, Option<Variable>)> = taken
            .iter()
            .map(|(ai, t, flag)| (*ai, addrs[*t], *flag))
            .collect();
        self.put_back(n, &back, st, Some(out))?;
        for a in &addrs {
            if !back.iter().any(|(_, b, _)| b == a) {
                let at = self.addr(*a);
                self.emit_drop(at)?;
                self.unlive(*a);
            }
        }
        self.release_stack(ss, count);
        self.check(st)?;
        self.push_live(out);
        Ok(V::Own(out))
    }

    /// Moves the value of a `var` argument's place into a new temporary. A plain name is
    /// moved inline unless it holds a resource; the returned variable says which way was taken.
    fn take_place(
        &mut self,
        n: &Node,
        arg: usize,
        place: &Place,
    ) -> Result<(Addr, Option<Variable>), String> {
        if self.kinds[place.root as usize].scalar() {
            return Err(self.unsupported("a number as a var argument"));
        }
        let a = self.temp();
        let mut flag = None;
        let after = self.b.create_block();
        if place.steps.is_empty() {
            let inline = self.b.declare_var(I8);
            let at = self.slot_addr(place.root);
            let tag = self.tag(at);
            let is_resource = self
                .b
                .ins()
                .icmp_imm_s(IntCC::Equal, tag, layout::RESOURCE as i64);
            let plain = self.b.create_block();
            let general = self.b.create_block();
            self.b.set_cold_block(general);
            self.b.ins().brif(is_resource, general, &[], plain, &[]);
            self.b.switch_to_block(plain);
            let at = self.slot_addr(place.root);
            let to = self.addr(a);
            self.copy_value(at, to);
            self.set_unit(at);
            let one = self.iconst(I8, 1);
            self.b.def_var(inline, one);
            self.b.ins().jump(after, &[]);
            self.b.switch_to_block(general);
            let zero = self.iconst(I8, 0);
            self.b.def_var(inline, zero);
            flag = Some(inline);
        }
        let (steps, sss) = self.index_steps(place)?;
        let to = self.addr(a);
        let (cx, frame, nn, nr, ai) = (
            self.cx,
            self.frame_ptr(),
            self.nslots_v(),
            self.node_ref(n)?,
            self.iconst(I64, arg as i64),
        );
        let st = self.hs("take_arg", &[cx, frame, nn, nr, ai, steps, to])?;
        self.steps_taken(sss);
        self.check(st)?;
        self.b.ins().jump(after, &[]);
        self.b.switch_to_block(after);
        self.push_live(a);
        Ok((a, flag))
    }

    /// Puts `var` arguments (taken from their temporaries) back into their places after a
    /// call whose status is `status`, evaluating the places' indices again as the interpreter
    /// does; when that fails, the call's result is released and the failure unwinds.
    fn put_back(
        &mut self,
        n: &Node,
        back: &[(usize, Addr, Option<Variable>)],
        status: ir::Value,
        out: Option<Addr>,
    ) -> Result<(), String> {
        let Node::Call(call) = n else { return Ok(()) };
        for (ai, a, flag) in back {
            let place = call.args[*ai]
                .place
                .as_ref()
                .ok_or("internal: a var place")?;
            let after = self.b.create_block();
            if let Some(flag) = flag {
                let inline = self.b.use_var(*flag);
                let plain = self.b.create_block();
                let general = self.b.create_block();
                self.b.ins().brif(inline, plain, &[], general, &[]);
                self.b.switch_to_block(plain);
                let at = self.slot_addr(place.root);
                self.emit_drop(at)?;
                let (from, at) = (self.addr(*a), self.slot_addr(place.root));
                self.copy_value(from, at);
                self.b.ins().jump(after, &[]);
                self.b.switch_to_block(general);
            }
            let (steps, sss) = self.index_steps(place)?;
            let vp = self.addr(*a);
            let (cx, frame, nn, nr, aiv) = (
                self.cx,
                self.frame_ptr(),
                self.nslots_v(),
                self.node_ref(n)?,
                self.iconst(I64, *ai as i64),
            );
            let st = self.hs("put_arg", &[cx, frame, nn, nr, aiv, steps, vp])?;
            self.unlive(*a);
            self.steps_taken(sss);
            // A failed put wins over the call's own outcome.
            let bad = self.b.create_block();
            let ok = self.b.create_block();
            self.b.set_cold_block(bad);
            self.b.ins().brif(st, bad, &[], ok, &[]);
            self.b.switch_to_block(bad);
            if let Some(o) = out {
                let release = self.b.create_block();
                let skip = self.b.create_block();
                self.b.ins().brif(status, skip, &[], release, &[]);
                self.b.switch_to_block(release);
                let at = self.addr(o);
                self.emit_drop(at)?;
                self.b.ins().jump(skip, &[]);
                self.b.switch_to_block(skip);
            }
            let t = self.unwind_from(self.scopes.len(), &[], None)?;
            self.b.ins().jump(t, &[]);
            self.b.switch_to_block(ok);
            self.b.ins().jump(after, &[]);
            self.b.switch_to_block(after);
        }
        Ok(())
    }
}

/// Whether evaluating a node twice is not observable.
fn pure(n: &Node) -> bool {
    match n {
        Node::Value(_) | Node::Local(_) | Node::Const(_) => true,
        Node::Binary(_, l, r) | Node::Index(l, r) | Node::And(l, r) => pure(l) && pure(r),
        Node::Field(t, _, _) | Node::Negate(t) | Node::Not(t) => pure(t),
        _ => false,
    }
}

/// Whether a node always gives a `Bool`, so that `or` with it on the left is boolean.
fn gives_bool(node: &Node) -> bool {
    match node {
        Node::Binary(op, _, _) => !matches!(
            op,
            Op::Add | Op::Subtract | Op::Multiply | Op::Divide | Op::Remainder | Op::Power
        ),
        Node::And(..) | Node::Not(_) | Node::Is(..) | Node::Value(RValue::Bool(_)) => true,
        Node::Or(left, _, _) => gives_bool(left),
        _ => false,
    }
}

fn collect_nodes_block<'n>(b: &'n LBlock, out: &mut Vec<&'n Node>) {
    for s in &b.stmts {
        collect_nodes_s(&s.kind, out);
    }
}

fn collect_nodes_s<'n>(s: &'n S, out: &mut Vec<&'n Node>) {
    match s {
        S::Let(_, n) | S::Fail(n) | S::Assert(n, _) | S::Expr(n) => out.push(n),
        S::Return(n) => out.extend(n.iter()),
        S::Assign(p, n) => {
            place_nodes(p, out);
            out.push(n);
        }
        S::SetIndex(p, k, v) => {
            place_nodes(p, out);
            out.push(k);
            out.push(v);
        }
        S::Concat(_, parts, g) => {
            out.extend(parts.iter());
            out.push(g);
        }
        S::Break | S::Continue | S::Pass => {}
        S::Handled { inner, block, .. } => {
            collect_nodes_s(inner, out);
            collect_nodes_block(block, out);
        }
        S::If(bs, o) => {
            for (c, b) in bs {
                out.push(c);
                collect_nodes_block(b, out);
            }
            if let Some(b) = o {
                collect_nodes_block(b, out);
            }
        }
        S::While(c, b) | S::For(_, c, b) | S::Using(_, c, b) => {
            out.push(c);
            collect_nodes_block(b, out);
        }
        S::Match(n, cases) => {
            out.push(n);
            for c in cases {
                out.extend(c.guard.iter());
                collect_nodes_block(&c.body, out);
            }
        }
        S::Try(b, hs) => {
            collect_nodes_block(b, out);
            for h in hs {
                collect_nodes_block(&h.body, out);
            }
        }
    }
}

fn place_nodes<'n>(p: &'n Place, out: &mut Vec<&'n Node>) {
    for s in &p.steps {
        if let PStep::Index(n) = s {
            out.push(n);
        }
    }
}
