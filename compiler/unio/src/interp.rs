//! The interpreter of sprint S5 (docs/runtime.md). It runs a checked program in the lowered
//! form of [`crate::lower`]: local names are slots of a frame, calls know their function and
//! the parameter of each argument, and literals are values. It uses the resolved declarations of
//! the checker (`Program`) for methods, constants, and defaults, and the value model, faults,
//! and external functions of `unio-runtime`. Evaluation is strict and left to right (section 1).
//!
//! The frequent paths are kept short (bench/README.md): calls put their slots on one stack of
//! values shared with the caller, method and field lookups remember what they found at each
//! place in the code, integer expressions of locals and literals are computed without building
//! values, and a few small prelude functions run natively ([`Intrinsic`]) with the results of
//! their Unio definitions.

use crate::ast::*;
use crate::lower::{
    Block, CallCode, Code, ConstructCode, ExprCode, LambdaCode, ListPat, Lowering, MethodArg,
    MethodCode, NameFallback, Node, Op, PStep, ParamCode, Part, Pat, Place, Slot, TypeBase,
    TypeCode, S,
};
use crate::program::{DefKind, Owner, Program, Symbol};
use crate::types::Ty;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use unio_runtime::fault::{self as rf, Fault, FaultKind, Outcome, Unwind};
use unio_runtime::runtime::Runtime;
use unio_runtime::value::{
    values_equal, Callable, EnumValue, OrderedMap, OrderedSet, StructValue, Value,
};

pub(crate) type ModId = usize;
pub(crate) type FnId = usize;
pub(crate) type DefId = usize;

/// A type known at run time: needed only where a value must be built from its type
/// (`std.json.from_value`).
#[derive(Clone, Debug)]
pub enum RtTy {
    Unknown,
    Int,
    Float,
    Bool,
    Text,
    Bytes,
    Path,
    List(Box<RtTy>),
    Map(Box<RtTy>, Box<RtTy>),
    Set(Box<RtTy>),
    Tuple(Vec<RtTy>),
    Named(DefId, Vec<RtTy>),
}

pub(crate) type Types = Rc<HashMap<String, RtTy>>;

/// A hasher for the interpreter's own tables, keyed by small integers, addresses in the tree,
/// and names (the multiplicative hash of rustc, FxHash). Values of programs are hashed by
/// `unio-runtime`, whose hash does not depend on the run.
#[derive(Default, Clone, Copy)]
pub(crate) struct Fx(u64);

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i));
    }
    fn write_u8(&mut self, i: u8) {
        self.write_u64(u64::from(i));
    }
}

pub(crate) type FxMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<Fx>>;

/// The frame of a running function: its slots, its module, and its type arguments.
pub struct Env {
    vars: Slots,
    module: ModId,
    types: Types,
}

/// The slots of the running function: a window from `base` into a stack of values that the
/// calls made from it share, so that a call pushes its slots and truncates them afterwards
/// instead of allocating a frame.
pub(crate) struct Slots {
    stack: Vec<Value>,
    base: usize,
}

impl Slots {
    fn new(values: Vec<Value>) -> Slots {
        Slots {
            stack: values,
            base: 0,
        }
    }
}

impl std::ops::Index<usize> for Slots {
    type Output = Value;
    #[inline]
    fn index(&self, slot: usize) -> &Value {
        &self.stack[self.base + slot]
    }
}

impl std::ops::IndexMut<usize> for Slots {
    #[inline]
    fn index_mut(&mut self, slot: usize) -> &mut Value {
        &mut self.stack[self.base + slot]
    }
}

impl std::ops::Index<std::ops::Range<usize>> for Slots {
    type Output = [Value];
    fn index(&self, slots: std::ops::Range<usize>) -> &[Value] {
        &self.stack[self.base + slots.start..self.base + slots.end]
    }
}

impl std::ops::IndexMut<std::ops::Range<usize>> for Slots {
    fn index_mut(&mut self, slots: std::ops::Range<usize>) -> &mut [Value] {
        &mut self.stack[self.base + slots.start..self.base + slots.end]
    }
}

/// What a name written as a callee or a value means outside the local names.
#[derive(Clone, Copy)]
pub(crate) enum Global {
    Func(FnId),
    Def(DefId),
    Const(usize),
    /// A variant written by its short name.
    Variant(DefId),
    Unknown,
}

/// The names of one variant of an enum, shared by all its values.
pub(crate) struct Shape {
    variant: Rc<str>,
    fields: Vec<Rc<str>>,
}

pub(crate) enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

type Exec = Result<Flow, Unwind>;

/// The frame of a method call and the places that its `var` arguments go back to.
type MethodFrame<'c> = (Vec<Value>, Vec<(usize, &'c Place)>);

#[derive(Clone)]
pub(crate) enum Step {
    Field(Rc<str>),
    Index(Value),
}

pub(crate) fn take(v: &mut Value) -> Value {
    std::mem::replace(v, Value::Unit)
}

pub(crate) fn internal(rt: &Runtime, message: impl std::fmt::Display) -> Unwind {
    rt.fault(
        FaultKind::InvalidArgument,
        format_args!("internal: {message}"),
    )
}

pub(crate) fn traced(rt: &Runtime, fault: Fault) -> Unwind {
    rt.traced(fault)
}

/// The interpreter: the checked program and the tables derived from it.
pub struct Interp {
    pub(crate) me: Weak<Interp>,
    pub program: Program,
    pub(crate) methods: FxMap<String, FxMap<String, FnId>>,
    /// Trace label `module.function` of each function.
    pub(crate) labels: Vec<Rc<str>>,
    /// Binding-table name of each function, used for externals.
    pub(crate) external_names: Vec<String>,
    pub(crate) variants: FxMap<String, Vec<DefId>>,
    /// The value of each top-level constant, evaluated at its first use.
    pub(crate) consts: Vec<std::cell::OnceCell<Value>>,
    pub(crate) files: Vec<Rc<str>>,
    /// The declaration of each function: its item and member (`signatures` by function).
    pub(crate) sig_of: Vec<Option<(usize, Option<usize>)>>,
    /// The bound external of each function without a body.
    pub(crate) externs: Vec<Option<unio_runtime::externs::ExternFn>>,
    /// The item of each struct or error declaration.
    pub(crate) decl_of: Vec<Option<usize>>,
    /// The type name of each declaration, shared by its values.
    pub(crate) def_names: Vec<Rc<str>>,
    /// The variants of each enum.
    pub(crate) shapes: Vec<Vec<Shape>>,
    /// The field names of each struct or error.
    pub(crate) field_names: Vec<Vec<Rc<str>>>,
    pub(crate) empty_types: Types,
    /// Names shared by the values and the lowered code that use them.
    pub(crate) names: RefCell<FxMap<String, Rc<str>>>,
    /// The lowered body of each function, made at its first call.
    pub(crate) codes: Vec<std::cell::OnceCell<Rc<Code>>>,
    /// Storage of frames that ended, for the next calls.
    pub(crate) pool: RefCell<Vec<Vec<Value>>>,
    /// Library functions of the prelude that the interpreter runs natively, with the same
    /// result as their Unio definitions (see [`Intrinsic`]).
    pub(crate) intrinsics: Vec<Option<Intrinsic>>,
    /// Whether a function has a `var` parameter other than `self`.
    pub(crate) var_params: Vec<bool>,
    /// 1 for the JSON conversions that the interpreter provides, 0 for other functions.
    pub(crate) externs_json: Vec<u8>,
    /// Lowered constants (by `(usize::MAX, constant)`) and defaults of externals (by function
    /// and parameter).
    pub(crate) default_codes: RefCell<FxMap<(usize, usize), Rc<ExprCode>>>,
    /// Lowered defaults of fields, by declaration and field.
    pub(crate) field_codes: RefCell<FxMap<(usize, usize), Rc<ExprCode>>>,
    /// The native code of every lowered body, in an executable made by `unio build`; the
    /// interpreter then only dispatches and never evaluates lowered code (runtime 8.4).
    pub(crate) native: std::cell::OnceCell<crate::native::support::Hooks>,
    /// The front end that lowers bodies at their first use; none in an executable.
    front: Option<&'static FrontEnd>,
}

/// What the interpreter needs of the front end: the declarations in the syntax trees, and
/// the lowering of a body at its first use. An executable made by `unio build` has neither,
/// since the build lowered every body (docs/runtime.md 8.4), so that it does not link them.
pub(crate) struct FrontEnd {
    declarations: fn(&Program) -> Declarations,
    code: fn(&Interp, FnId) -> Option<Rc<Code>>,
    constant: fn(&Interp, usize) -> Option<Rc<ExprCode>>,
    field_default: fn(&Interp, DefId, usize) -> Option<Rc<ExprCode>>,
    extern_default: fn(&Interp, FnId, usize) -> Option<Rc<ExprCode>>,
}

/// The item and member that declare each function, and the item of each struct or error.
type Declarations = (Vec<Option<(usize, Option<usize>)>>, Vec<Option<usize>>);

static FRONT_END: FrontEnd = FrontEnd {
    declarations,
    code: Interp::lower_code,
    constant: Interp::lower_const,
    field_default: Interp::lower_field_default,
    extern_default: Interp::lower_extern_default,
};

/// Where the syntax trees declare the functions and the structs and errors of the program.
fn declarations(program: &Program) -> Declarations {
    let mut signatures = FxMap::default();
    let mut decls = FxMap::default();
    for (m, module) in program.modules.iter().enumerate() {
        for (i, item) in module.analysis.file.items.iter().enumerate() {
            match item {
                Item::Declaration(d) => match &d.kind {
                    DeclarationKind::Function(f) => {
                        signatures.insert((m, f.signature.name.span.start.offset), (i, None));
                    }
                    DeclarationKind::External(s) => {
                        signatures.insert((m, s.name.span.start.offset), (i, None));
                    }
                    DeclarationKind::Struct(s) => {
                        decls.insert((m, s.name.text.clone()), i);
                    }
                    DeclarationKind::Error(e) => {
                        decls.insert((m, e.name.text.clone()), i);
                    }
                    DeclarationKind::Interface(x) => {
                        for (j, (s, _)) in x.members.iter().enumerate() {
                            signatures.insert((m, s.name.span.start.offset), (i, Some(j)));
                        }
                    }
                    _ => {}
                },
                Item::Extend(e) => {
                    for (j, member) in e.members.iter().enumerate() {
                        signatures
                            .insert((m, member.signature.name.span.start.offset), (i, Some(j)));
                    }
                }
                _ => {}
            }
        }
    }
    let sig_of = program
        .funcs
        .iter()
        .map(|def| {
            signatures
                .get(&(def.module, def.span.start.offset))
                .copied()
        })
        .collect();
    let decl_of = program
        .defs
        .iter()
        .map(|def| decls.get(&(def.module, def.name.clone())).copied())
        .collect();
    (sig_of, decl_of)
}

impl Interp {
    /// The interpreter of a checked program, which lowers each body at its first use.
    pub fn new(program: Program) -> Rc<Interp> {
        Interp::with(program, Some(&FRONT_END))
    }

    /// The interpreter of a program read from the image of an executable, whose bodies are all
    /// lowered and installed before it runs.
    pub(crate) fn prepared(program: Program) -> Rc<Interp> {
        Interp::with(program, None)
    }

    fn with(program: Program, front: Option<&'static FrontEnd>) -> Rc<Interp> {
        let mut methods: FxMap<String, FxMap<String, FnId>> = FxMap::default();
        for imp in &program.impls {
            let Some(key) = ty_key(&program, &imp.target) else {
                continue;
            };
            for &f in &imp.methods {
                methods
                    .entry(key.clone())
                    .or_default()
                    .insert(program.funcs[f].name.clone(), f);
            }
        }
        for imp in &program.impls {
            let Some(key) = ty_key(&program, &imp.target) else {
                continue;
            };
            if let Some(Ty::Named(i, _)) = &imp.interface {
                if let DefKind::Interface(fs) = &program.defs[*i].kind {
                    for &f in fs {
                        if program.funcs[f].body.is_some() {
                            methods
                                .entry(key.clone())
                                .or_default()
                                .entry(program.funcs[f].name.clone())
                                .or_insert(f);
                        }
                    }
                }
            }
        }
        let (sig_of, decl_of) = match front {
            Some(front) => (front.declarations)(&program),
            None => (
                vec![None; program.funcs.len()],
                vec![None; program.defs.len()],
            ),
        };
        let mut variants: FxMap<String, Vec<DefId>> = FxMap::default();
        for (d, def) in program.defs.iter().enumerate() {
            if let DefKind::Enum(vs) = &def.kind {
                for v in vs {
                    variants.entry(v.name.clone()).or_default().push(d);
                }
            }
        }
        let files = (0..program.modules.len())
            .map(|m| Rc::from(program.trace_path(m)))
            .collect();
        let labels: Vec<Rc<str>> = program
            .funcs
            .iter()
            .map(|f| Rc::from(format!("{}.{}", program.modules[f.module].name, f.name)))
            .collect();
        let external_names: Vec<String> = (0..program.funcs.len())
            .map(|f| Interp::external_name(&program, f))
            .collect();
        let externs = program
            .funcs
            .iter()
            .zip(&external_names)
            .map(|(def, name)| match def.body {
                Some(_) => None,
                None => unio_runtime::externs::lookup(name),
            })
            .collect();
        let mut names: FxMap<String, Rc<str>> = FxMap::default();
        let mut shared = |name: &str| -> Rc<str> {
            if let Some(n) = names.get(name) {
                return Rc::clone(n);
            }
            let n = unio_runtime::value::names::get(name).unwrap_or_else(|| Rc::from(name));
            names.insert(name.to_string(), Rc::clone(&n));
            n
        };
        // Values of a declared type carry its identity, not its short name (`type_identity`).
        let def_names: Vec<Rc<str>> = (0..program.defs.len())
            .map(|d| shared(&type_identity(&program, d)))
            .collect();
        let shapes: Vec<Vec<Shape>> = program
            .defs
            .iter()
            .map(|def| match &def.kind {
                DefKind::Enum(vs) => vs
                    .iter()
                    .map(|v| Shape {
                        variant: shared(&v.name),
                        fields: v.fields.iter().map(|(n, _)| shared(n)).collect(),
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect();
        let field_names: Vec<Vec<Rc<str>>> = program
            .defs
            .iter()
            .map(|def| match &def.kind {
                DefKind::Struct(fs) | DefKind::Error(fs) => {
                    fs.iter().map(|f| shared(&f.name)).collect()
                }
                _ => Vec::new(),
            })
            .collect();
        for name in program
            .defs
            .iter()
            .flat_map(|def| match &def.kind {
                DefKind::Enum(vs) => vs
                    .iter()
                    .flat_map(|v| std::iter::once(&v.name).chain(v.fields.iter().map(|(n, _)| n)))
                    .collect::<Vec<_>>(),
                DefKind::Struct(fs) | DefKind::Error(fs) => fs.iter().map(|f| &f.name).collect(),
                _ => Vec::new(),
            })
            .chain(program.defs.iter().map(|def| &def.name))
        {
            shared(name);
        }
        let codes = program
            .funcs
            .iter()
            .map(|_| std::cell::OnceCell::new())
            .collect();
        let range_def = program.defs.iter().position(|d| {
            d.name == "Range" && program.modules[d.module].name == "prelude:core/prelude"
        });
        let intrinsics = labels
            .iter()
            .map(|label: &Rc<str>| Intrinsic::of(label, range_def))
            .collect();
        let var_params = program
            .funcs
            .iter()
            .map(|f| f.params.iter().any(|p| p.var))
            .collect();
        let consts = program
            .consts
            .iter()
            .map(|_| std::cell::OnceCell::new())
            .collect();
        Rc::new_cyclic(|me| Interp {
            front,
            var_params,
            intrinsics,
            sig_of,
            externs,
            decl_of,
            def_names,
            shapes,
            field_names,
            empty_types: Rc::new(HashMap::new()),
            names: RefCell::new(names),
            codes,
            pool: RefCell::new(Vec::new()),
            externs_json: external_names
                .iter()
                .map(|n| u8::from(n == "std.json.to_value" || n == "std.json.from_value"))
                .collect(),
            default_codes: RefCell::new(FxMap::default()),
            field_codes: RefCell::new(FxMap::default()),
            native: std::cell::OnceCell::new(),
            labels,
            external_names,
            me: me.clone(),
            program,
            methods,
            variants,
            consts,
            files,
        })
    }

    /// The function and file of every function, for the call stack of a runtime.
    pub(crate) fn sites(&self) -> Vec<(Rc<str>, Rc<str>)> {
        self.program
            .funcs
            .iter()
            .enumerate()
            .map(|(f, def)| {
                (
                    Rc::clone(&self.labels[f]),
                    Rc::clone(&self.files[def.module]),
                )
            })
            .collect()
    }

    pub(crate) fn rc(&self) -> Rc<Interp> {
        self.me.upgrade().expect("the interpreter is alive")
    }

    /// The shared name for a text: values and lowered code use one allocation per name.
    pub(crate) fn shared(&self, name: &str) -> Rc<str> {
        if let Some(n) = self.names.borrow().get(name) {
            return Rc::clone(n);
        }
        let n: Rc<str> = Rc::from(name);
        self.names
            .borrow_mut()
            .insert(name.to_string(), Rc::clone(&n));
        n
    }

    /// What a name means outside the local names of a module (a callee or a name used as a
    /// value): a variant of the module first, which hides a prelude name, then a declaration,
    /// then a variant of any visible enum.
    pub(crate) fn global(&self, module: ModId, name: &str) -> Global {
        if let Some(d) = self.local_variant(module, name) {
            return Global::Variant(d);
        }
        match self.program.lookup(module, name) {
            Some((_, Symbol::Func(f))) => Global::Func(f),
            Some((_, Symbol::Def(d))) => Global::Def(d),
            Some((_, Symbol::Const(c))) => Global::Const(c),
            None => match self.variant(module, name) {
                Some(d) => Global::Variant(d),
                None => Global::Unknown,
            },
        }
    }

    pub(crate) fn signature(&self, f: FnId) -> Option<(&Signature, Option<&crate::ast::Block>)> {
        let def = &self.program.funcs[f];
        let (item, member) = self.sig_of[f]?;
        let items = &self.program.modules[def.module].analysis.file.items;
        match (&items[item], member) {
            (Item::Declaration(d), None) => match &d.kind {
                DeclarationKind::Function(func) => Some((&func.signature, Some(&func.body))),
                DeclarationKind::External(s) => Some((s, None)),
                _ => None,
            },
            (Item::Declaration(d), Some(j)) => match &d.kind {
                DeclarationKind::Interface(x) => {
                    let (s, b) = &x.members[j];
                    Some((s, b.as_ref()))
                }
                _ => None,
            },
            (Item::Extend(e), Some(j)) => {
                let member = &e.members[j];
                Some((&member.signature, member.body.as_ref()))
            }
            _ => None,
        }
    }

    pub(crate) fn fields_of(&self, d: DefId) -> Option<&Vec<Field>> {
        let def = &self.program.defs[d];
        let i = self.decl_of[d]?;
        match &self.program.modules[def.module].analysis.file.items[i] {
            Item::Declaration(decl) => match &decl.kind {
                DeclarationKind::Struct(s) => s.fields.as_ref(),
                DeclarationKind::Error(e) => e.fields.as_ref(),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn source_text(&self, module: ModId, span: crate::diag::Span) -> String {
        let source = &self.program.modules[module].analysis.source;
        source
            .get(span.start.offset..span.end.offset)
            .unwrap_or("")
            .to_string()
    }

    // ---------------------------------------------------------------- methods

    pub(crate) fn method(&self, key: &str, name: &str) -> Option<FnId> {
        self.methods.get(key)?.get(name).copied()
    }

    /// The method `name` of the type of a value, found without building the name of the type.
    pub(crate) fn method_of(&self, value: &Value, name: &str) -> Option<FnId> {
        match value {
            Value::Struct(s) => self.method(&s.type_name, name),
            Value::Enum(e) => self.method(&e.type_name, name),
            Value::Resource(cell) => self.method_of(&cell.value.borrow(), name),
            other => self.method(static_key(other), name),
        }
    }

    /// External name of a function without a body (unio-runtime README, binding table).
    pub(crate) fn external_name(program: &Program, f: FnId) -> String {
        let def = &program.funcs[f];
        match def.owner {
            Owner::Impl(i) => {
                let key = ty_key(program, &program.impls[i].target).unwrap_or_default();
                format!("{key}.{}", def.name)
            }
            _ => {
                let module = &program.modules[def.module];
                if module.prelude {
                    def.name.clone()
                } else {
                    format!("{}.{}", module.name, def.name)
                }
            }
        }
    }

    // ---------------------------------------------------------------- lowered code

    /// The lowered body of a function, made the first time it is called.
    pub(crate) fn code(&self, f: FnId) -> Option<&Code> {
        if let Some(code) = self.codes[f].get() {
            return Some(code);
        }
        let code = (self.front?.code)(self, f)?;
        let _ = self.codes[f].set(code);
        self.codes[f].get().map(|c| &**c)
    }

    fn lower_code(&self, f: FnId) -> Option<Rc<Code>> {
        let (sig, body) = self.signature(f)?;
        let body = body?;
        let mut lower = Lowering::new(self, self.program.funcs[f].module);
        let mut params = Vec::with_capacity(sig.params.len());
        for param in &sig.params {
            let (name, var, default) = match param {
                Param::SelfParam { var, .. } => ("self", *var, None),
                Param::Named { name, default, .. } => (name.text.as_str(), false, default.as_ref()),
                Param::Plain {
                    var, name, default, ..
                } => (name.text.as_str(), *var, default.as_ref()),
            };
            // A default sees the parameters before it (runtime 1.3).
            let default = default.map(|e| lower.expr(e));
            lower.bind(name);
            params.push(ParamCode { var, default });
        }
        let body = lower.block(body);
        Some(Rc::new(Code {
            slots: lower.slots(),
            defaults: params.iter().any(|p| p.default.is_some()),
            params,
            body,
        }))
    }

    /// A lowered expression evaluated alone in the module: a constant or a default of an
    /// external or of a field, kept by `key` in `cache`.
    pub(crate) fn expr_code(
        &self,
        cache: &RefCell<FxMap<(usize, usize), Rc<ExprCode>>>,
        key: (usize, usize),
        module: ModId,
        expr: &Expr,
    ) -> Rc<ExprCode> {
        if let Some(code) = cache.borrow().get(&key) {
            return Rc::clone(code);
        }
        let mut lower = Lowering::new(self, module);
        let body = lower.expr(expr);
        let code = Rc::new(ExprCode {
            slots: lower.slots(),
            body,
        });
        cache.borrow_mut().insert(key, Rc::clone(&code));
        code
    }

    pub(crate) fn eval_code(&self, rt: &mut Runtime, module: ModId, code: &ExprCode) -> Outcome {
        if let Some(hooks) = self.native.get() {
            return hooks.eval_expr(self, rt, code);
        }
        let mut env = Env {
            vars: Slots::new(vec![Value::Unit; code.slots]),
            module,
            types: Rc::clone(&self.empty_types),
        };
        self.eval(rt, &mut env, &code.body)
    }

    // ---------------------------------------------------------------- calls

    /// Calls a function with its arguments in parameter order (`self` first); `var`
    /// parameters are written back into `args`.
    pub fn call_fn(&self, rt: &mut Runtime, f: FnId, args: &mut [Value], types: Types) -> Outcome {
        if self.program.funcs[f].body.is_none() {
            return self.call_external(rt, f, args, &types);
        }
        let mut frame: Vec<Value> = args.iter_mut().map(take).collect();
        let result = self.invoke(rt, f, &mut frame, types);
        if let Some(code) = self.codes[f].get() {
            for (i, p) in code.params.iter().enumerate() {
                if p.var && i < args.len() && i < frame.len() {
                    args[i] = take(&mut frame[i]);
                }
            }
        }
        result
    }

    /// Runs a function whose arguments are in the first slots of `frame`; `Unit` marks a
    /// missing argument. After the call, the slots of `var` parameters hold their values.
    pub(crate) fn invoke(
        &self,
        rt: &mut Runtime,
        f: FnId,
        frame: &mut Vec<Value>,
        types: Types,
    ) -> Outcome {
        let def = &self.program.funcs[f];
        if def.body.is_none() {
            return self.call_external(rt, f, frame, &types);
        }
        if let Some(intrinsic) = self.intrinsics[f] {
            if let Some(result) = self.intrinsic(rt, intrinsic, frame) {
                return result;
            }
        }
        if let Some(hooks) = self.native.get() {
            return hooks.invoke(self, rt, f, frame, &types);
        }
        let Some(code) = self.code(f) else {
            return Err(internal(
                rt,
                format_args!("no declaration for {}", def.name),
            ));
        };
        let module = def.module;
        let base = rt.site_base(|| self.sites());
        rt.enter_site(base + f as u32, def.span.start.line as u32)?;
        let mut env = Env {
            vars: Slots::new(std::mem::take(frame)),
            module,
            types,
        };
        while env.vars.stack.len() < code.slots {
            env.vars.stack.push(Value::Unit);
        }
        for (i, p) in code.params.iter().enumerate().filter(|_| code.defaults) {
            if let (Value::Unit, Some(default)) = (&env.vars[i], &p.default) {
                match self.eval(rt, &mut env, default) {
                    Ok(v) => env.vars[i] = v,
                    Err(e) => {
                        rt.leave();
                        *frame = env.vars.stack;
                        return Err(e);
                    }
                }
            }
        }
        let result = self.run_block(rt, &mut env, &code.body);
        rt.leave();
        *frame = env.vars.stack;
        match result? {
            Flow::Return(v) => Ok(v),
            _ => Ok(Value::Unit),
        }
    }

    /// Runs a library function natively; `None` when the arguments need its Unio definition
    /// (for example `contains` on items whose equality a program defines).
    pub(crate) fn intrinsic(
        &self,
        rt: &mut Runtime,
        intrinsic: Intrinsic,
        frame: &mut [Value],
    ) -> Option<Outcome> {
        let arg = |i: usize| frame.get(i);
        Some(Ok(match (intrinsic, arg(0)?) {
            (Intrinsic::IsEmpty, Value::Text(t)) => Value::Bool(t.is_empty()),
            (Intrinsic::IsEmpty, Value::List(l)) => Value::Bool(l.is_empty()),
            (Intrinsic::IsEmpty, Value::Map(m)) => Value::Bool(m.is_empty()),
            (Intrinsic::IsEmpty, Value::Set(s)) => Value::Bool(s.is_empty()),
            (Intrinsic::IsEmpty, Value::Bytes(b)) => Value::Bool(b.is_empty()),
            // A sign, then ASCII digits, without overflow: what `i64::from_str` accepts.
            (Intrinsic::ToInt, Value::Text(t)) => {
                Value::option(t.as_str().parse::<i64>().ok().map(Value::Int))
            }
            (Intrinsic::First, Value::List(l)) => Value::option(l.first().cloned()),
            (Intrinsic::Last, Value::List(l)) => Value::option(l.last().cloned()),
            (Intrinsic::Enumerate, Value::List(l)) => {
                let start = match arg(1)? {
                    Value::Int(i) => *i,
                    Value::Unit => 0,
                    _ => return None,
                };
                let mut pairs = Vec::with_capacity(l.len());
                for (i, item) in l.iter().enumerate() {
                    let index = match rf::int_add(start, i as i64) {
                        Ok(index) => index,
                        Err(fault) => return Some(Err(traced(rt, fault))),
                    };
                    pairs.push(Value::tuple(vec![Value::Int(index), item.clone()]));
                }
                if let Err(e) = rt.check_heap() {
                    return Some(Err(e));
                }
                Value::list(pairs)
            }
            (Intrinsic::Contains, Value::List(l)) => match arg(1)? {
                item @ (Value::Text(_) | Value::Int(_) | Value::Bool(_)) => {
                    Value::Bool(l.iter().any(|x| values_equal(x, item)))
                }
                _ => return None,
            },
            (Intrinsic::Contains, Value::Map(m)) => Value::Bool(m.get(arg(1)?).is_some()),
            (Intrinsic::Keys, Value::Map(m)) => {
                Value::list(m.entries().map(|(k, _)| k.clone()).collect())
            }
            (Intrinsic::Values, Value::Map(m)) => {
                Value::list(m.entries().map(|(_, v)| v.clone()).collect())
            }
            (Intrinsic::AppendAll, Value::List(_)) => {
                let Some(Value::List(items)) = frame.get(1).cloned() else {
                    return None;
                };
                let Value::List(l) = &mut frame[0] else {
                    return None;
                };
                Rc::make_mut(l).extend(items.iter().cloned());
                Value::Unit
            }
            (Intrinsic::Range(d), Value::Int(start)) => {
                let Some(Value::Int(end)) = arg(1) else {
                    return None;
                };
                let values = vec![Some(Value::Int(*start)), Some(Value::Int(*end))];
                return Some(self.finish_struct(rt, d, values));
            }
            _ => return None,
        }))
    }

    pub(crate) fn call_external(
        &self,
        rt: &mut Runtime,
        f: FnId,
        args: &mut [Value],
        types: &Types,
    ) -> Outcome {
        let def = &self.program.funcs[f];
        if !def.params.iter().any(|p| p.has_default) {
            if let (0, Some(function)) = (self.externs_json[f], self.externs[f]) {
                return function(rt, args);
            }
        }
        // Defaults of externals are evaluated here, in the declaring module.
        let offset = usize::from(def.self_param.is_some());
        for (i, param) in def.params.iter().enumerate() {
            if !param.has_default || !matches!(args.get(offset + i), Some(Value::Unit)) {
                continue;
            }
            let Some(code) = self.extern_default_code(f, i) else {
                return Err(internal(
                    rt,
                    format_args!("no declaration for {}", def.name),
                ));
            };
            args[offset + i] = self.eval_code(rt, def.module, &code)?;
        }
        if self.externs_json[f] == 0 {
            return match self.externs[f] {
                Some(function) => function(rt, args),
                None => unio_runtime::externs::call(rt, &self.external_names[f], args),
            };
        }
        match self.external_names[f].as_str() {
            "std.json.to_value" => {
                let v = args.first().cloned().unwrap_or(Value::Unit);
                self.encode_json(rt, &v)
            }
            "std.json.from_value" => {
                let ty = types.values().next().cloned().unwrap_or(RtTy::Unknown);
                let v = args.first().cloned().unwrap_or(Value::Unit);
                self.decode_json(rt, &ty, &v, "")
            }
            name => match self.externs[f] {
                Some(function) => function(rt, args),
                None => unio_runtime::externs::call(rt, name, args),
            },
        }
    }

    pub(crate) fn generic_types(
        &self,
        env: &Env,
        f: FnId,
        types: &[TypeCode],
    ) -> Result<Types, Unwind> {
        self.generic_types_in(&env.types, f, types)
    }

    /// The type arguments of a call to `f`, with the names of the caller's type parameters
    /// (`env_types`).
    pub(crate) fn generic_types_in(
        &self,
        env_types: &Types,
        f: FnId,
        types: &[TypeCode],
    ) -> Result<Types, Unwind> {
        if types.is_empty() || self.program.funcs[f].generics.is_empty() {
            return Ok(Rc::clone(&self.empty_types));
        }
        let mut map = HashMap::new();
        for (p, t) in self.program.funcs[f].generics.iter().zip(types) {
            map.insert(
                self.program.params[*p].name.clone(),
                self.rt_type(env_types, t)?,
            );
        }
        Ok(Rc::new(map))
    }

    /// The frame of a call to `f`: arguments in parameter order, `Unit` where missing, and
    /// the places that `var` arguments come from and go back to.
    #[inline]
    pub(crate) fn new_frame(&self, f: FnId, count: usize) -> Vec<Value> {
        let slots = self.codes[f].get().map_or(count, |c| c.slots.max(count));
        let mut frame = self.pool.borrow_mut().pop().unwrap_or_default();
        frame.reserve(slots);
        for _ in 0..count {
            frame.push(Value::Unit);
        }
        frame
    }

    /// Returns the storage of a frame for the next call, so that calls do not allocate.
    pub(crate) fn recycle(&self, mut frame: Vec<Value>) {
        frame.clear();
        let mut pool = self.pool.borrow_mut();
        if pool.len() < 256 && frame.capacity() <= 1024 {
            pool.push(frame);
        }
    }

    pub(crate) fn call_known(&self, rt: &mut Runtime, env: &mut Env, call: &CallCode) -> Outcome {
        if call.types.is_empty() {
            if let Some(code) = self.codes[call.f].get() {
                if !code.defaults && self.intrinsics[call.f].is_none() {
                    return self.call_lowered(rt, env, call, code);
                }
            }
        }
        let mut frame = self.new_frame(call.f, call.count);
        let mut places: Vec<(usize, &Place)> = Vec::new();
        for arg in &call.args {
            frame[arg.target] = match &arg.place {
                Some(place) => {
                    places.push((arg.target, place));
                    self.take_place(rt, env, place)?
                }
                None => self.eval(rt, env, &arg.value)?,
            };
        }
        let types = self.generic_types(env, call.f, &call.types)?;
        let result = self.invoke(rt, call.f, &mut frame, types);
        for (i, place) in places {
            let v = take(&mut frame[i]);
            self.put_place(rt, env, place, v)?;
        }
        self.recycle(frame);
        result
    }

    /// A call of a function whose body is lowered, without defaults to fill or type
    /// arguments: the arguments go straight into the callee's slots on the shared stack.
    #[inline]
    pub(crate) fn call_lowered(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        call: &CallCode,
        code: &Code,
    ) -> Outcome {
        let base = env.vars.stack.len();
        env.vars
            .stack
            .resize_with(base + code.slots.max(call.count), || Value::Unit);
        let mut places = false;
        for arg in &call.args {
            let value = match &arg.place {
                Some(place) => {
                    places = true;
                    self.take_place(rt, env, place)
                }
                None => self.eval(rt, env, &arg.value),
            };
            match value {
                Ok(v) => env.vars.stack[base + arg.target] = v,
                Err(e) => {
                    env.vars.stack.truncate(base);
                    return Err(e);
                }
            }
        }
        let result = self.run_on_stack(rt, env, call.f, code, base);
        if places {
            for arg in &call.args {
                if let Some(place) = &arg.place {
                    let v = take(&mut env.vars.stack[base + arg.target]);
                    if let Err(e) = self.put_place(rt, env, place, v) {
                        env.vars.stack.truncate(base);
                        return Err(e);
                    }
                }
            }
        }
        env.vars.stack.truncate(base);
        result
    }

    /// Runs the lowered body of `f` with its slots at `base` of the shared stack, which the
    /// caller has filled with the arguments; the slots stay for the caller to read `var`
    /// parameters back and to truncate.
    #[inline]
    pub(crate) fn run_on_stack(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        f: FnId,
        code: &Code,
        base: usize,
    ) -> Outcome {
        let def = &self.program.funcs[f];
        let sites = rt.site_base(|| self.sites());
        rt.enter_site(sites + f as u32, def.span.start.line as u32)?;
        let caller_base = std::mem::replace(&mut env.vars.base, base);
        let caller_module = std::mem::replace(&mut env.module, def.module);
        let caller_types = std::mem::replace(&mut env.types, Rc::clone(&self.empty_types));
        let result = self.run_block(rt, env, &code.body);
        env.vars.base = caller_base;
        env.module = caller_module;
        env.types = caller_types;
        rt.leave();
        match result? {
            Flow::Return(v) => Ok(v),
            _ => Ok(Value::Unit),
        }
    }

    /// The arguments of a method call matched to the parameters of the method found.
    pub(crate) fn method_frame<'c>(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        f: FnId,
        args: &'c [MethodArg],
    ) -> Result<MethodFrame<'c>, Unwind> {
        let def = &self.program.funcs[f];
        let count = 1 + def.params.len();
        let mut frame = self.new_frame(f, count);
        let mut places = Vec::new();
        let mut position = 1;
        for arg in args {
            let target = match &arg.name {
                Some(n) => match def.params.iter().position(|p| p.name == *n) {
                    Some(i) => i + 1,
                    None => return Err(internal(rt, format_args!("no parameter {n}"))),
                },
                None => {
                    position += 1;
                    position - 1
                }
            };
            if target >= count {
                return Err(internal(rt, "too many arguments"));
            }
            frame[target] = match &arg.place {
                Some(place) if def.params[target - 1].var => {
                    places.push((target, place));
                    self.take_place(rt, env, place)?
                }
                _ => self.eval(rt, env, &arg.value)?,
            };
        }
        Ok((frame, places))
    }

    pub(crate) fn method_call(&self, rt: &mut Runtime, env: &mut Env, m: &MethodCode) -> Outcome {
        if let Node::Local(slot) = m.receiver {
            if let Some(result) = self.local_method_call(rt, env, m, slot as usize) {
                return result;
            }
        }
        let receiver = self.eval(rt, env, &m.receiver)?;
        self.method_call_on(rt, env, m, receiver)
    }

    /// A method call on a local name with positional arguments and no `var` parameters
    /// besides `self`: the method is found from the value in place, and a `var self` method
    /// moves the value into the call and back without copying. `None` leaves the call to the
    /// general path, before anything is evaluated.
    #[inline]
    pub(crate) fn local_method_call(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        m: &MethodCode,
        slot: usize,
    ) -> Option<Outcome> {
        let receiver = &env.vars[slot];
        if matches!(receiver, Value::Resource(_)) || !m.positional {
            return None;
        }
        let f = self.cached_method(m, receiver)?;
        let def = &self.program.funcs[f];
        if self.var_params[f] || m.args.len() > def.params.len() {
            return None;
        }
        let in_place = def.self_param == Some(true);
        if in_place && !matches!(&m.place, Some(p) if p.steps.is_empty() && p.root as usize == slot)
        {
            return None;
        }
        let count = 1 + def.params.len();
        let code = match def.body {
            Some(_) => match self.codes[f].get() {
                Some(code) if !code.defaults && self.intrinsics[f].is_none() => Some(&**code),
                _ => None,
            },
            None => None,
        };
        let external = match (def.body.is_none(), self.externs[f]) {
            (true, Some(function))
                if self.externs_json[f] == 0 && m.args.len() == def.params.len() =>
            {
                Some(function)
            }
            _ => None,
        };
        if code.is_some() || external.is_some() {
            // The callee's slots go on the shared stack: arguments first, then the receiver,
            // which a `var self` method takes from its name only after the arguments.
            let base = env.vars.stack.len();
            let slots = code.map_or(count, |c| c.slots.max(count));
            env.vars.stack.resize_with(base + slots, || Value::Unit);
            for (i, arg) in m.args.iter().enumerate() {
                match self.eval(rt, env, &arg.value) {
                    Ok(v) => env.vars.stack[base + 1 + i] = v,
                    Err(e) => {
                        env.vars.stack.truncate(base);
                        return Some(Err(e));
                    }
                }
            }
            env.vars.stack[base] = if in_place {
                take(&mut env.vars[slot])
            } else {
                env.vars[slot].clone()
            };
            let result = match (code, external) {
                (Some(code), _) => self.run_on_stack(rt, env, f, code, base),
                (None, Some(function)) => function(rt, &mut env.vars.stack[base..base + count]),
                (None, None) => unreachable!("a lowered body or an external"),
            };
            if in_place {
                env.vars[slot] = take(&mut env.vars.stack[base]);
            }
            env.vars.stack.truncate(base);
            return Some(result);
        }
        let mut frame = self.new_frame(f, count);
        for (i, arg) in m.args.iter().enumerate() {
            match self.eval(rt, env, &arg.value) {
                Ok(v) => frame[1 + i] = v,
                Err(e) => {
                    self.recycle(frame);
                    return Some(Err(e));
                }
            }
        }
        let types = Rc::clone(&self.empty_types);
        let result = if in_place {
            frame[0] = take(&mut env.vars[slot]);
            let result = self.invoke(rt, f, &mut frame, types);
            env.vars[slot] = take(&mut frame[0]);
            result
        } else {
            frame[0] = env.vars[slot].clone();
            self.invoke(rt, f, &mut frame, types)
        };
        self.recycle(frame);
        Some(result)
    }

    pub(crate) fn method_call_on(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        m: &MethodCode,
        receiver: Value,
    ) -> Outcome {
        let Some(f) = self.cached_method(m, &receiver) else {
            // A field that holds a function value: `self.steps.write_data(...)` is not a method.
            if let Ok(Value::Function(_)) = field_of(rt, &receiver, &m.name) {
                let function = field_of(rt, &receiver, &m.name)?;
                let args = self.positional(rt, env, &m.args)?;
                return self.call_value(rt, &function, args);
            }
            let args = self.positional(rt, env, &m.args)?;
            return self.call_method_value(rt, receiver, &m.name, args);
        };
        let var_self = self.program.funcs[f].self_param == Some(true);
        if !var_self && m.positional && !matches!(receiver, Value::Resource(_)) {
            if let Some(result) = self.external_method(rt, env, f, m, receiver.clone()) {
                return result;
            }
        }
        let empty = || Rc::clone(&self.empty_types);
        if let (true, Some(place), false) =
            (var_self, &m.place, matches!(receiver, Value::Resource(_)))
        {
            drop(receiver);
            let (mut frame, places) = self.method_frame(rt, env, f, &m.args)?;
            let steps = self.steps(rt, env, place)?;
            let result = self.with_resolved(rt, env, place.root, &steps, |interp, rt, slot| {
                frame[0] = take(slot);
                let r = interp.invoke(rt, f, &mut frame, Rc::clone(&interp.empty_types));
                *slot = take(&mut frame[0]);
                Ok(r)
            })?;
            for (i, place) in places {
                let v = take(&mut frame[i]);
                self.put_place(rt, env, place, v)?;
            }
            self.recycle(frame);
            return result;
        }
        let (mut frame, places) = self.method_frame(rt, env, f, &m.args)?;
        let result = if let Value::Resource(cell) = &receiver {
            if var_self {
                frame[0] = take(&mut cell.value.borrow_mut());
                let r = self.invoke(rt, f, &mut frame, empty());
                *cell.value.borrow_mut() = take(&mut frame[0]);
                r
            } else {
                frame[0] = cell.value.borrow().clone();
                self.invoke(rt, f, &mut frame, empty())
            }
        } else {
            frame[0] = receiver;
            self.invoke(rt, f, &mut frame, empty())
        };
        for (i, place) in places {
            let v = take(&mut frame[i]);
            self.put_place(rt, env, place, v)?;
        }
        self.recycle(frame);
        result
    }

    /// A method bound to an external function with every argument given positionally and at
    /// most three of them: the arguments are passed in a small array, without a frame.
    #[inline]
    pub(crate) fn external_method(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        f: FnId,
        m: &MethodCode,
        receiver: Value,
    ) -> Option<Outcome> {
        let def = &self.program.funcs[f];
        let function = self.externs[f]?;
        if def.body.is_some()
            || self.externs_json[f] != 0
            || self.var_params[f]
            || m.args.len() != def.params.len()
            || m.args.len() > 3
        {
            return None;
        }
        let mut args = [receiver, Value::Unit, Value::Unit, Value::Unit];
        for (i, arg) in m.args.iter().enumerate() {
            match self.eval(rt, env, &arg.value) {
                Ok(v) => args[1 + i] = v,
                Err(e) => return Some(Err(e)),
            }
        }
        Some(function(rt, &mut args[..1 + m.args.len()]))
    }

    /// The method of a call site for the type of the receiver, remembered for the last type.
    pub(crate) fn cached_method(&self, m: &MethodCode, receiver: &Value) -> Option<FnId> {
        let key = match receiver {
            Value::Struct(s) => Rc::as_ptr(&s.type_name) as *const u8 as usize,
            Value::Enum(e) => Rc::as_ptr(&e.type_name) as *const u8 as usize,
            Value::Resource(_) => 0,
            other => static_key(other).as_ptr() as usize,
        };
        let (k, f) = m.cache.get();
        if key != 0 && k == key {
            return Some(f);
        }
        let f = self.method_of(receiver, &m.name)?;
        if key != 0 {
            // The identity stays alive while its address is cached, so no other type's name
            // can take that address (the externals make names of their own).
            *m.pin.borrow_mut() = match receiver {
                Value::Struct(s) => Some(Rc::clone(&s.type_name)),
                Value::Enum(e) => Some(Rc::clone(&e.type_name)),
                _ => None,
            };
            m.cache.set((key, f));
        }
        Some(f)
    }

    pub(crate) fn positional(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        args: &[MethodArg],
    ) -> Result<Vec<Value>, Unwind> {
        args.iter().map(|a| self.eval(rt, env, &a.value)).collect()
    }

    /// Calls a method on a value that is not a place: the receiver is not written back.
    pub(crate) fn call_method_value(
        &self,
        rt: &mut Runtime,
        receiver: Value,
        name: &str,
        args: Vec<Value>,
    ) -> Outcome {
        let (result, _) = self.call_method_owned(rt, receiver, name, args)?;
        result
    }

    /// Calls a method and returns its result together with the receiver after the call.
    pub(crate) fn call_method_owned(
        &self,
        rt: &mut Runtime,
        receiver: Value,
        name: &str,
        rest: Vec<Value>,
    ) -> Result<(Outcome, Value), Unwind> {
        if let Value::Resource(cell) = &receiver {
            let var_self = self
                .method_of(&cell.value.borrow(), name)
                .as_ref()
                .is_some_and(|f| self.program.funcs[*f].self_param == Some(true));
            if !var_self {
                let inner = cell.value.borrow().clone();
                let (result, _) = self.call_method_owned(rt, inner, name, rest)?;
                return Ok((result, receiver));
            }
            let inner = take(&mut cell.value.borrow_mut());
            let (result, after) = self.call_method_owned(rt, inner, name, rest)?;
            *cell.value.borrow_mut() = after;
            return Ok((result, receiver));
        }
        match self.method_of(&receiver, name) {
            Some(f) => {
                let total = 1 + self.program.funcs[f].params.len();
                let mut args = self.new_frame(f, 0);
                args.push(receiver);
                args.extend(rest);
                while args.len() < total {
                    args.push(Value::Unit);
                }
                let result = self.invoke(rt, f, &mut args, Rc::clone(&self.empty_types));
                let after = take(&mut args[0]);
                self.recycle(args);
                Ok((result, after))
            }
            None => {
                let result = match (name, &receiver) {
                    ("to_text", _) => self.display(rt, &receiver).map(Value::text),
                    ("diagnostic_code", Value::Struct(s)) => {
                        Ok(Value::text(self.error_code(&s.type_name)))
                    }
                    ("equals", _) => Ok(Value::Bool(values_equal(
                        &receiver,
                        rest.first().unwrap_or(&Value::Unit),
                    ))),
                    _ => Err(internal(
                        rt,
                        format!("no method {name} for {}", value_key(&receiver)),
                    )),
                };
                Ok((result, receiver))
            }
        }
    }

    pub(crate) fn call_value(
        &self,
        rt: &mut Runtime,
        function: &Value,
        mut args: Vec<Value>,
    ) -> Outcome {
        match function {
            Value::Function(f) => f.call(rt, &mut args),
            other => Err(internal(
                rt,
                format_args!("{} is not a function", other.kind()),
            )),
        }
    }

    /// The fully qualified identifier of an error from the identity of its type.
    pub(crate) fn error_code(&self, type_name: &str) -> String {
        for (d, def) in self.program.defs.iter().enumerate() {
            if &*self.def_names[d] == type_name && matches!(def.kind, DefKind::Error(_)) {
                let module = &self.program.modules[def.module].name;
                return format!("{module}.{}", def.name);
            }
        }
        type_name.to_string()
    }

    // ---------------------------------------------------------------- places

    /// The steps of a place with its indices evaluated.
    pub(crate) fn steps(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        place: &Place,
    ) -> Result<Vec<Step>, Unwind> {
        let mut steps = Vec::with_capacity(place.steps.len());
        for step in &place.steps {
            steps.push(match step {
                PStep::Field(n) => Step::Field(Rc::clone(n)),
                PStep::Index(e) => Step::Index(self.eval(rt, env, e)?),
            });
        }
        Ok(steps)
    }

    /// Runs `f` on the place `root` + `steps`, whose indices are already evaluated.
    pub(crate) fn with_resolved<R>(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        root: Slot,
        steps: &[Step],
        f: impl FnOnce(&Interp, &mut Runtime, &mut Value) -> Result<R, Unwind>,
    ) -> Result<R, Unwind> {
        let root = root as usize;
        let mut value = take(&mut env.vars[root]);
        let result = if let Value::Resource(cell) = &value {
            let cell = Rc::clone(cell);
            let mut inner = take(&mut cell.value.borrow_mut());
            let r = navigate(rt, &mut inner, steps).and_then(|target| f(self, rt, target));
            *cell.value.borrow_mut() = inner;
            r
        } else {
            navigate(rt, &mut value, steps).and_then(|target| f(self, rt, target))
        };
        env.vars[root] = value;
        result
    }

    pub(crate) fn take_place(&self, rt: &mut Runtime, env: &mut Env, place: &Place) -> Outcome {
        if place.steps.is_empty() && !matches!(env.vars[place.root as usize], Value::Resource(_)) {
            return Ok(take(&mut env.vars[place.root as usize]));
        }
        let steps = self.steps(rt, env, place)?;
        self.with_resolved(rt, env, place.root, &steps, |_, _, v| Ok(take(v)))
    }

    pub(crate) fn put_place(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        place: &Place,
        value: Value,
    ) -> Result<(), Unwind> {
        if place.steps.is_empty() && !matches!(env.vars[place.root as usize], Value::Resource(_)) {
            env.vars[place.root as usize] = value;
            return Ok(());
        }
        let steps = self.steps(rt, env, place)?;
        self.with_resolved(rt, env, place.root, &steps, |_, _, slot| {
            *slot = value;
            Ok(())
        })
    }

    // ---------------------------------------------------------------- statements

    pub(crate) fn run_block(&self, rt: &mut Runtime, env: &mut Env, block: &Block) -> Exec {
        for statement in &block.stmts {
            rt.at_line(statement.line);
            match self.run(rt, env, &statement.kind) {
                Ok(Flow::Normal) => {}
                flow => {
                    clear_block(env, block);
                    return flow;
                }
            }
        }
        clear_block(env, block);
        Ok(Flow::Normal)
    }

    pub(crate) fn run(&self, rt: &mut Runtime, env: &mut Env, statement: &S) -> Exec {
        match statement {
            S::Let(slot, value) => {
                env.vars[*slot as usize] = self.eval(rt, env, value)?;
                Ok(Flow::Normal)
            }
            S::Assign(place, value) => {
                if place.steps.is_empty()
                    && !matches!(env.vars[place.root as usize], Value::Resource(_))
                {
                    env.vars[place.root as usize] = self.eval(rt, env, value)?;
                    return Ok(Flow::Normal);
                }
                if place.steps.iter().all(|s| matches!(s, PStep::Field(_)))
                    && !matches!(env.vars[place.root as usize], Value::Resource(_))
                {
                    // Fields only: nothing to evaluate in the place, and no steps to build.
                    let value = self.eval(rt, env, value)?;
                    let target =
                        navigate_fields(rt, &mut env.vars[place.root as usize], &place.steps)?;
                    *target = value;
                    return Ok(Flow::Normal);
                }
                // The place and its indices are resolved first, then the value (runtime 1.1).
                let steps = self.steps(rt, env, place)?;
                let value = self.eval(rt, env, value)?;
                self.with_resolved(rt, env, place.root, &steps, |_, _, slot| {
                    *slot = value;
                    Ok(())
                })?;
                Ok(Flow::Normal)
            }
            S::SetIndex(place, key, value) => {
                let steps = self.steps(rt, env, place)?;
                let key = self.eval(rt, env, key)?;
                let mut value = Some(self.eval(rt, env, value)?);
                self.with_resolved(
                    rt,
                    env,
                    place.root,
                    &steps,
                    |_, rt, container| match container {
                        Value::Map(m) => {
                            Rc::make_mut(m).put(key, value.take().unwrap_or(Value::Unit));
                            Ok(())
                        }
                        Value::List(l) => {
                            let list = Rc::make_mut(l);
                            let i = match key {
                                Value::Int(i) => i,
                                _ => return Err(internal(rt, "list index is not Int")),
                            };
                            match usize::try_from(i).ok().and_then(|i| list.get_mut(i)) {
                                Some(slot) => {
                                    *slot = value.take().unwrap_or(Value::Unit);
                                    Ok(())
                                }
                                None => Err(rt.fault(
                                    FaultKind::IndexOutOfRange,
                                    format_args!("index {i} is outside 0..{}", list.len()),
                                )),
                            }
                        }
                        _ => Err(internal(
                            rt,
                            "indexed assignment to a value that is not a list or map",
                        )),
                    },
                )?;
                Ok(Flow::Normal)
            }
            S::Concat(slot, parts, general) => {
                let slot = *slot as usize;
                if !matches!(env.vars[slot], Value::Text(_)) {
                    env.vars[slot] = self.eval(rt, env, general)?;
                    return Ok(Flow::Normal);
                }
                // The parts do not use the name, and text + text cannot fail, so evaluating
                // them before taking the text out is not observable.
                if let [part] = &parts[..] {
                    let value = self.eval(rt, env, part)?;
                    let Value::Text(text) = take(&mut env.vars[slot]) else {
                        return Err(internal(rt, "concatenation"));
                    };
                    return match value {
                        Value::Text(t) => {
                            env.vars[slot] = Value::Text(text.append(t.as_str()));
                            Ok(Flow::Normal)
                        }
                        other => {
                            env.vars[slot] = Value::Text(text);
                            Err(internal(rt, format_args!("text + {}", other.kind())))
                        }
                    };
                }
                let mut values = Vec::with_capacity(parts.len());
                for part in parts {
                    values.push(self.eval(rt, env, part)?);
                }
                let Value::Text(mut text) = take(&mut env.vars[slot]) else {
                    return Err(internal(rt, "concatenation"));
                };
                for v in values {
                    match v {
                        Value::Text(t) => text = text.append(t.as_str()),
                        other => {
                            env.vars[slot] = Value::Text(text);
                            return Err(internal(rt, format_args!("text + {}", other.kind())));
                        }
                    }
                }
                env.vars[slot] = Value::Text(text);
                Ok(Flow::Normal)
            }
            S::Return(e) => Ok(Flow::Return(match e {
                Some(e) => self.eval(rt, env, e)?,
                None => Value::Unit,
            })),
            S::Fail(e) => Err(Unwind::Error(Box::new(self.eval(rt, env, e)?))),
            S::Assert(e, text) => {
                if self.truth(rt, env, e)? {
                    Ok(Flow::Normal)
                } else {
                    Err(rt.fault(
                        FaultKind::AssertionFailed,
                        format_args!("assertion failed: {text}"),
                    ))
                }
            }
            S::Break => Ok(Flow::Break),
            S::Continue => Ok(Flow::Continue),
            S::Pass => Ok(Flow::Normal),
            S::Expr(e) => {
                self.eval(rt, env, e)?;
                Ok(Flow::Normal)
            }
            S::Handled {
                inner,
                ty,
                bind,
                block,
            } => match self.run(rt, env, inner) {
                Err(Unwind::Error(error)) if handles(ty, &error) => {
                    if let Some(slot) = bind {
                        env.vars[*slot as usize] = *error;
                    }
                    self.run_block(rt, env, block)
                }
                other => other,
            },
            S::If(branches, otherwise) => {
                for (condition, body) in branches {
                    if self.truth(rt, env, condition)? {
                        return self.run_block(rt, env, body);
                    }
                }
                match otherwise {
                    Some(body) => self.run_block(rt, env, body),
                    None => Ok(Flow::Normal),
                }
            }
            S::While(condition, body) => loop {
                if !self.truth(rt, env, condition)? {
                    return Ok(Flow::Normal);
                }
                match self.run_block(rt, env, body)? {
                    Flow::Break => return Ok(Flow::Normal),
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                    _ => {}
                }
            },
            S::For(names, iterable, body) => {
                let source = self.eval(rt, env, iterable)?;
                let mut cursor = self.cursor(rt, source)?;
                loop {
                    let Some(item) = self.next(rt, &mut cursor)? else {
                        return Ok(Flow::Normal);
                    };
                    bind_names(rt, &mut env.vars, names, item)?;
                    match self.run_block(rt, env, body)? {
                        Flow::Break => return Ok(Flow::Normal),
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            S::Match(subject, cases) => {
                let value = self.eval(rt, env, subject)?;
                for case in cases {
                    if self.matches(rt, &mut env.vars, &case.pattern, &value)?
                        && match &case.guard {
                            Some(g) => self.truth(rt, env, g)?,
                            None => true,
                        }
                    {
                        return self.run_block(rt, env, &case.body);
                    }
                }
                Err(rt.fault(FaultKind::InvalidArgument, "internal: no case matched"))
            }
            S::Try(body, handlers) => match self.run_block(rt, env, body) {
                Err(Unwind::Error(error)) => {
                    for handler in handlers {
                        if handles(&handler.ty, &error) {
                            if let Some(slot) = handler.bind {
                                env.vars[slot as usize] = *error;
                            }
                            return self.run_block(rt, env, &handler.body);
                        }
                    }
                    Err(Unwind::Error(error))
                }
                other => other,
            },
            S::Using(slot, value, body) => {
                let v = self.eval(rt, env, value)?;
                let resource = match v {
                    Value::Resource(_) => v,
                    other => rt.new_resource(other),
                };
                env.vars[*slot as usize] = resource.clone();
                let flow = self.run_block(rt, env, body);
                env.vars[*slot as usize] = Value::Unit;
                if matches!(flow, Err(Unwind::Exhausted(_))) {
                    return flow;
                }
                let released = self.release(rt, &resource);
                match (flow, released) {
                    // A release that exhausted the reserve or the heap ends the program with
                    // code 102; the remaining releases are skipped (C-18).
                    (_, Err(exhausted @ Unwind::Exhausted(_))) => Err(exhausted),
                    (Err(e), Err(Unwind::Fault(f))) => {
                        rt.write_fault(&f);
                        Err(e)
                    }
                    (Ok(_), Err(e)) => Err(e),
                    (flow, _) => flow,
                }
            }
        }
    }

    pub(crate) fn release(&self, rt: &mut Runtime, resource: &Value) -> Result<(), Unwind> {
        if let Value::Resource(cell) = resource {
            if cell.released.replace(true) {
                return Ok(());
            }
        }
        let has_release = self.method_of(resource, "release").is_some();
        if !has_release {
            return Ok(());
        }
        let result = self
            .call_method_owned(rt, resource.clone(), "release", Vec::new())
            .and_then(|(result, _)| result.map(|_| ()));
        if let Err(exhausted @ Unwind::Exhausted(_)) = result {
            return Err(exhausted);
        }
        // On the path of a fault, a release that overflowed the reserve on a full heap is
        // exhaustion even when it reached no check of the heap itself (C-17, C-18).
        rt.check_heap()?;
        result
    }

    // ---------------------------------------------------------------- iteration

    pub(crate) fn cursor(&self, rt: &mut Runtime, source: Value) -> Result<Cursor, Unwind> {
        Ok(match source {
            Value::List(l) | Value::Tuple(l) => Cursor::List(l, 0),
            Value::Set(s) => Cursor::Items(s.items().cloned().collect::<Vec<_>>().into_iter()),
            Value::Map(m) => Cursor::Items(
                m.entries()
                    .map(|(k, _)| k.clone())
                    .collect::<Vec<_>>()
                    .into_iter(),
            ),
            Value::Text(t) => Cursor::Text(t, 0),
            Value::Struct(s) if &*s.type_name == "Range" => {
                let start = match s.fields.iter().find(|(n, _)| &**n == "start") {
                    Some((_, Value::Int(i))) => *i,
                    _ => 0,
                };
                let end = match s.fields.iter().find(|(n, _)| &**n == "end") {
                    Some((_, Value::Int(i))) => *i,
                    _ => 0,
                };
                Cursor::Range(start, end)
            }
            other => {
                if self.method_of(&other, "next").is_some() {
                    Cursor::Iterator(other)
                } else {
                    let it = self.call_method_value(rt, other, "iterator", Vec::new())?;
                    Cursor::Iterator(it)
                }
            }
        })
    }

    pub(crate) fn next(
        &self,
        rt: &mut Runtime,
        cursor: &mut Cursor,
    ) -> Result<Option<Value>, Unwind> {
        match cursor {
            Cursor::List(items, i) => {
                let item = items.get(*i).cloned();
                *i += 1;
                Ok(item)
            }
            Cursor::Text(t, i) => {
                let item = t.char_at(*i).map(Value::Text);
                *i += 1;
                Ok(item)
            }
            Cursor::Items(it) => Ok(it.next()),
            Cursor::Range(current, end) => {
                if *current >= *end {
                    Ok(None)
                } else {
                    let v = *current;
                    *current += 1;
                    Ok(Some(Value::Int(v)))
                }
            }
            Cursor::Iterator(it) => {
                let receiver = take(it);
                let (result, after) = self.call_method_owned(rt, receiver, "next", Vec::new())?;
                *it = after;
                match result? {
                    Value::Enum(e) if &*e.variant == "some" => {
                        Ok(e.fields.first().map(|(_, v)| v.clone()))
                    }
                    _ => Ok(None),
                }
            }
        }
    }

    // ---------------------------------------------------------------- expressions

    pub(crate) fn truth(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        node: &Node,
    ) -> Result<bool, Unwind> {
        match node {
            Node::Binary(op, left, right) => {
                if let Some(Value::Bool(b)) = fast_binary(env, *op, left, right) {
                    return Ok(b);
                }
                if let (Some(a), Some(b)) = (peek(env, left), peek(env, right)) {
                    if let Some(result) = compare_simple(*op, a, b) {
                        return Ok(result);
                    }
                }
            }
            Node::And(left, right) => {
                return Ok(self.truth(rt, env, left)? && self.truth(rt, env, right)?)
            }
            Node::Not(operand) => return Ok(!self.truth(rt, env, operand)?),
            Node::Value(Value::Bool(b)) => return Ok(*b),
            Node::Or(left, right, _) if gives_bool(left) => {
                return Ok(self.truth(rt, env, left)? || self.truth(rt, env, right)?)
            }
            _ => {}
        }
        match self.eval(rt, env, node)? {
            Value::Bool(b) => Ok(b),
            other => Err(internal(
                rt,
                format_args!("condition is a {}", other.kind()),
            )),
        }
    }

    /// Evaluates an expression. The common nodes are handled here and the rest in
    /// [`Interp::eval_node`], so that the frequent paths stay short.
    #[inline]
    pub(crate) fn eval(&self, rt: &mut Runtime, env: &mut Env, node: &Node) -> Outcome {
        match node {
            Node::Local(slot) => Ok(env.vars[*slot as usize].clone()),
            Node::Value(v) => Ok(v.clone()),
            Node::Binary(op, left, right) => self.binary(rt, env, *op, left, right),
            Node::Call(call) => self.call_known(rt, env, call),
            Node::Method(m) => self.method_call(rt, env, m),
            Node::Field(target, name, at) => match **target {
                Node::Local(slot) => field_cached(rt, &env.vars[slot as usize], name, at),
                _ => self.eval_node(rt, env, node),
            },
            _ => self.eval_node(rt, env, node),
        }
    }

    /// A binary operator: integers that need no other evaluation first, then the general case.
    #[inline(never)]
    pub(crate) fn binary(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        op: Op,
        left: &Node,
        right: &Node,
    ) -> Outcome {
        if let Some(v) = fast_binary(env, op, left, right) {
            return Ok(v);
        }
        if let (Some(a), Some(b)) = (peek(env, left), peek(env, right)) {
            if let (Value::Int(a), Value::Int(b)) = (a, b) {
                return int_op(rt, op, *a, *b);
            }
            if let Some(result) = compare_simple(op, a, b) {
                return Ok(Value::Bool(result));
            }
        }
        let l = self.eval(rt, env, left)?;
        if let (Value::Int(a), Some(Value::Int(b))) = (&l, peek(env, right)) {
            return int_op(rt, op, *a, *b);
        }
        let r = self.eval(rt, env, right)?;
        if let (Value::Int(a), Value::Int(b)) = (&l, &r) {
            return int_op(rt, op, *a, *b);
        }
        self.apply(rt, op, l, r)
    }

    #[inline(never)]
    pub(crate) fn eval_node(&self, rt: &mut Runtime, env: &mut Env, node: &Node) -> Outcome {
        match node {
            Node::Value(v) => Ok(v.clone()),
            Node::Local(slot) => Ok(env.vars[*slot as usize].clone()),
            Node::Binary(op, left, right) => {
                if let (Some(a), Some(b)) = (peek(env, left), peek(env, right)) {
                    if let (Value::Int(a), Value::Int(b)) = (a, b) {
                        return int_op(rt, *op, *a, *b);
                    }
                    if let Some(result) = compare_simple(*op, a, b) {
                        return Ok(Value::Bool(result));
                    }
                }
                let l = self.eval(rt, env, left)?;
                if let (Value::Int(a), Some(Value::Int(b))) = (&l, peek(env, right)) {
                    return int_op(rt, *op, *a, *b);
                }
                let r = self.eval(rt, env, right)?;
                if let (Value::Int(a), Value::Int(b)) = (&l, &r) {
                    return int_op(rt, *op, *a, *b);
                }
                self.apply(rt, *op, l, r)
            }
            Node::Call(call) => self.call_known(rt, env, call),
            Node::Method(m) => self.method_call(rt, env, m),
            Node::Field(target, name, at) => {
                // A local struct is read in place, without copying the struct.
                if let Node::Local(slot) = **target {
                    return field_cached(rt, &env.vars[slot as usize], name, at);
                }
                let value = self.eval(rt, env, target)?;
                field_cached(rt, &value, name, at)
            }
            Node::Index(target, index) => {
                if let Node::Local(slot) = **target {
                    let i = self.eval(rt, env, index)?;
                    return index_value(rt, &env.vars[slot as usize], &i);
                }
                let t = self.eval(rt, env, target)?;
                let i = self.eval(rt, env, index)?;
                index_value(rt, &t, &i)
            }
            Node::And(left, right) => Ok(Value::Bool(
                self.truth(rt, env, left)? && self.truth(rt, env, right)?,
            )),
            Node::Not(operand) => match self.eval(rt, env, operand)? {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                v => Err(internal(rt, format_args!("unary not on {}", v.kind()))),
            },
            Node::Const(c) => self.constant(rt, *c),
            Node::FnRef(f) => Ok(Value::Function(Rc::new(FnRef {
                interp: self.rc(),
                f: *f,
            }))),
            Node::Variant(d, name) => Ok(self.make_variant(*d, name, Vec::new())),
            Node::Text(parts) => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        Part::Literal(s) => out.push_str(s),
                        Part::Value(e) => match self.eval(rt, env, e)? {
                            Value::Text(t) => out.push_str(t.as_str()),
                            Value::Int(i) => {
                                use std::fmt::Write;
                                let _ = write!(out, "{i}");
                            }
                            v => out.push_str(&self.display(rt, &v)?),
                        },
                    }
                }
                Ok(Value::text(out))
            }
            Node::Tuple(items) => Ok(Value::tuple(self.eval_all(rt, env, items)?)),
            Node::List(items) => Ok(Value::list(self.eval_all(rt, env, items)?)),
            Node::Map(entries) => {
                let mut map = OrderedMap::new();
                for (k, v) in entries {
                    let key = self.eval(rt, env, k)?;
                    let value = self.eval(rt, env, v)?;
                    map.put(key, value);
                }
                Ok(Value::map(map))
            }
            Node::Comprehension(c) => {
                let source = self.eval(rt, env, &c.iterable)?;
                let mut cursor = self.cursor(rt, source)?;
                let mut out = Vec::new();
                while let Some(item) = self.next(rt, &mut cursor)? {
                    bind_names(rt, &mut env.vars, &c.names, item)?;
                    let keep = match &c.condition {
                        Some(condition) => self.truth(rt, env, condition)?,
                        None => true,
                    };
                    if keep {
                        out.push(self.eval(rt, env, &c.element)?);
                    }
                }
                for slot in &c.names {
                    env.vars[*slot as usize] = Value::Unit;
                }
                rt.check_heap()?;
                Ok(Value::list(out))
            }
            Node::Lambda(code) => Ok(Value::Function(Rc::new(Closure {
                interp: self.rc(),
                captured: code
                    .captures
                    .iter()
                    .map(|(outer, _)| env.vars[*outer as usize].clone())
                    .collect(),
                code: Rc::clone(code),
                module: env.module,
                types: Rc::clone(&env.types),
            }))),
            Node::Is(value, pattern) => {
                let v = self.eval(rt, env, value)?;
                Ok(Value::Bool(self.matches(rt, &mut env.vars, pattern, &v)?))
            }
            Node::Conditional(parts) => {
                let [condition, then, otherwise] = &**parts;
                if self.truth(rt, env, condition)? {
                    self.eval(rt, env, then)
                } else {
                    self.eval(rt, env, otherwise)
                }
            }
            Node::Or(left, right, yields_option) => {
                // `items.get(i) or default` on a list: the item without building an Option.
                if let (Node::Method(m), false) = (&**left, *yields_option) {
                    if m.name == "get" && m.args.len() == 1 && m.args[0].name.is_none() {
                        let receiver = self.eval(rt, env, &m.receiver)?;
                        if let Value::List(items) = &receiver {
                            if let Value::Int(i) = self.eval(rt, env, &m.args[0].value)? {
                                return match usize::try_from(i).ok().and_then(|i| items.get(i)) {
                                    Some(item) => Ok(item.clone()),
                                    None => self.eval(rt, env, right),
                                };
                            }
                            return Err(internal(rt, "List.get with an index that is not Int"));
                        }
                        let l = self.method_call_on(rt, env, m, receiver)?;
                        return self.or_value(rt, env, l, right, false);
                    }
                }
                let l = self.eval(rt, env, left)?;
                self.or_value(rt, env, l, right, *yields_option)
            }
            Node::Negate(operand) => match self.eval(rt, env, operand)? {
                Value::Int(i) => rf::int_subtract(0, i)
                    .map(Value::Int)
                    .map_err(|f| traced(rt, f)),
                Value::Float(f) => Ok(Value::Float(-f)),
                v => Err(internal(rt, format_args!("unary - on {}", v.kind()))),
            },
            Node::Try(operand, is_value) => match self.eval(rt, env, operand)? {
                Value::Enum(e) if &*e.type_name == "Result" && *is_value => {
                    let inner = e
                        .fields
                        .first()
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Unit);
                    if &*e.variant == "ok" {
                        Ok(inner)
                    } else {
                        Err(Unwind::Error(Box::new(inner)))
                    }
                }
                v => Ok(v),
            },
            Node::CallValue(callee, args) => {
                let function = self.eval(rt, env, callee)?;
                let args = self.eval_all(rt, env, args)?;
                self.call_value(rt, &function, args)
            }
            Node::Construct(c) => self.construct(rt, env, c),
            Node::SafeField(target, name) => match self.eval(rt, env, target)? {
                Value::Enum(e) if &*e.type_name == "Option" => {
                    if &*e.variant == "none" {
                        Ok(Value::none())
                    } else {
                        let inner = e
                            .fields
                            .first()
                            .map(|(_, v)| v.clone())
                            .unwrap_or(Value::Unit);
                        Ok(Value::some(field_of(rt, &inner, name)?))
                    }
                }
                other => field_of(rt, &other, name),
            },
            Node::Fault(message) => Err(rt.fault(FaultKind::InvalidArgument, message)),
        }
    }

    /// `l or right` once `l` is known: `right` only for `false` and `none` (runtime 1.2).
    pub(crate) fn or_value(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        l: Value,
        right: &Node,
        yields_option: bool,
    ) -> Outcome {
        match l {
            Value::Bool(true) => Ok(Value::Bool(true)),
            Value::Bool(false) => self.eval(rt, env, right),
            Value::Enum(e) if &*e.type_name == "Option" => {
                if &*e.variant == "none" {
                    self.eval(rt, env, right)
                } else if yields_option {
                    Ok(Value::Enum(e))
                } else {
                    Ok(e.fields
                        .first()
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Unit))
                }
            }
            other => Err(internal(rt, format_args!("or on {}", other.kind()))),
        }
    }

    pub(crate) fn eval_all(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        items: &[Node],
    ) -> Result<Vec<Value>, Unwind> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(self.eval(rt, env, item)?);
        }
        Ok(out)
    }

    /// A lowered expression already made: by the interpreter at its first use, or read from
    /// the program image of an executable (docs/runtime.md 8.4).
    fn lowered(
        cache: &RefCell<FxMap<(usize, usize), Rc<ExprCode>>>,
        key: (usize, usize),
    ) -> Option<Rc<ExprCode>> {
        cache.borrow().get(&key).cloned()
    }

    /// The lowered value of a constant.
    pub(crate) fn const_code(&self, c: usize) -> Option<Rc<ExprCode>> {
        if let Some(code) = Self::lowered(&self.default_codes, (usize::MAX, c)) {
            return Some(code);
        }
        (self.front?.constant)(self, c)
    }

    fn lower_const(&self, c: usize) -> Option<Rc<ExprCode>> {
        let def = &self.program.consts[c];
        let expr = match &self.program.modules[def.module].analysis.file.items[def.item] {
            Item::Declaration(d) => match &d.kind {
                DeclarationKind::Constant(k) => &k.value,
                _ => return None,
            },
            _ => return None,
        };
        Some(self.expr_code(&self.default_codes, (usize::MAX, c), def.module, expr))
    }

    /// The lowered default of field `i` of a struct or an error.
    pub(crate) fn field_default_code(&self, d: DefId, i: usize) -> Option<Rc<ExprCode>> {
        if let Some(code) = Self::lowered(&self.field_codes, (d, i)) {
            return Some(code);
        }
        (self.front?.field_default)(self, d, i)
    }

    fn lower_field_default(&self, d: DefId, i: usize) -> Option<Rc<ExprCode>> {
        let def = &self.program.defs[d];
        let fields = match &def.kind {
            DefKind::Struct(f) | DefKind::Error(f) => f,
            _ => return None,
        };
        let field = fields.get(i)?;
        let e = self
            .fields_of(d)?
            .iter()
            .find(|f| f.name.text == field.name)?
            .default
            .as_ref()?;
        Some(self.expr_code(&self.field_codes, (d, i), def.module, e))
    }

    /// The lowered default of parameter `i` (after `self`) of an external function.
    pub(crate) fn extern_default_code(&self, f: FnId, i: usize) -> Option<Rc<ExprCode>> {
        if let Some(code) = Self::lowered(&self.default_codes, (f, i)) {
            return Some(code);
        }
        (self.front?.extern_default)(self, f, i)
    }

    fn lower_extern_default(&self, f: FnId, i: usize) -> Option<Rc<ExprCode>> {
        let def = &self.program.funcs[f];
        let (sig, _) = self.signature(f)?;
        let param = sig
            .params
            .iter()
            .filter(|p| !matches!(p, Param::SelfParam { .. }))
            .nth(i)?;
        let e = match param {
            Param::Named { default, .. } | Param::Plain { default, .. } => default.as_ref()?,
            Param::SelfParam { .. } => return None,
        };
        Some(self.expr_code(&self.default_codes, (f, i), def.module, e))
    }

    pub(crate) fn constant(&self, rt: &mut Runtime, c: usize) -> Outcome {
        if let Some(v) = self.consts[c].get() {
            return Ok(v.clone());
        }
        let def = &self.program.consts[c];
        let Some(code) = self.const_code(c) else {
            return Err(internal(rt, "constant"));
        };
        let value = self.eval_code(rt, def.module, &code)?;
        let _ = self.consts[c].set(value.clone());
        Ok(value)
    }

    pub(crate) fn construct(&self, rt: &mut Runtime, env: &mut Env, c: &ConstructCode) -> Outcome {
        match &c.variant {
            Some(variant) if c.fields == 1 && c.args.len() == 1 && c.args[0].0 == 0 => {
                let value = self.eval(rt, env, &c.args[0].1)?;
                let shapes = &self.shapes[c.def];
                let shape = shapes
                    .iter()
                    .find(|v| Rc::ptr_eq(&v.variant, variant))
                    .or_else(|| shapes.iter().find(|v| v.variant == *variant));
                Ok(match shape {
                    Some(shape) if shape.fields.len() == 1 => Value::Enum(Rc::new(EnumValue {
                        type_name: Rc::clone(&self.def_names[c.def]),
                        variant: Rc::clone(&shape.variant),
                        fields: unio_runtime::value::Fields::One([(
                            Rc::clone(&shape.fields[0]),
                            value,
                        )]),
                    })),
                    _ => self.make_variant(c.def, variant, vec![value]),
                })
            }
            Some(variant) => {
                let mut values = vec![Value::Unit; c.fields];
                for (i, node) in &c.args {
                    let value = self.eval(rt, env, node)?;
                    if *i < values.len() {
                        values[*i] = value;
                    }
                }
                Ok(self.make_variant(c.def, variant, values))
            }
            None => {
                let mut values: Vec<Option<Value>> = vec![None; c.fields];
                for (i, node) in &c.args {
                    let value = self.eval(rt, env, node)?;
                    if *i < values.len() {
                        values[*i] = Some(value);
                    }
                }
                self.finish_struct(rt, c.def, values)
            }
        }
    }
    /// A variant declared in the module, which hides a prelude name (semantics 1.4).
    pub(crate) fn local_variant(&self, module: ModId, name: &str) -> Option<DefId> {
        self.variants
            .get(name)?
            .iter()
            .find(|d| self.program.defs[**d].module == module)
            .copied()
    }

    pub(crate) fn variant(&self, module: ModId, name: &str) -> Option<DefId> {
        let candidates = self.variants.get(name)?;
        candidates
            .iter()
            .find(|d| self.program.defs[**d].module == module)
            .or_else(|| {
                candidates
                    .iter()
                    .find(|d| self.program.modules[self.program.defs[**d].module].prelude)
            })
            .or_else(|| candidates.first())
            .copied()
    }

    pub(crate) fn make_variant(&self, d: DefId, variant: &str, values: Vec<Value>) -> Value {
        // Names are shared, so the same address finds the variant without comparing texts.
        let shapes = &self.shapes[d];
        let shape = shapes
            .iter()
            .find(|v| std::ptr::eq(v.variant.as_ptr(), variant.as_ptr()))
            .or_else(|| shapes.iter().find(|v| &*v.variant == variant));
        let fields = match shape {
            Some(shape) if shape.fields.len() == 1 && values.len() == 1 => {
                let value = values.into_iter().next().unwrap_or(Value::Unit);
                unio_runtime::value::Fields::One([(Rc::clone(&shape.fields[0]), value)])
            }
            Some(shape) => shape.fields.iter().cloned().zip(values).collect(),
            None => unio_runtime::value::Fields::Zero,
        };
        Value::Enum(Rc::new(EnumValue {
            type_name: Rc::clone(&self.def_names[d]),
            variant: match shape {
                Some(shape) => Rc::clone(&shape.variant),
                None => Rc::from(variant),
            },
            fields,
        }))
    }

    /// Fills defaults (evaluated freshly in the declaring module) and builds the value.
    pub(crate) fn finish_struct(
        &self,
        rt: &mut Runtime,
        d: DefId,
        mut values: Vec<Option<Value>>,
    ) -> Outcome {
        let def = &self.program.defs[d];
        let fields = match &def.kind {
            DefKind::Struct(f) | DefKind::Error(f) => f,
            _ => return Err(internal(rt, "struct")),
        };
        let mut out = Vec::with_capacity(fields.len());
        for (i, field) in fields.iter().enumerate() {
            let value = match values[i].take() {
                Some(v) => v,
                None => match field
                    .has_default
                    .then(|| self.field_default_code(d, i))
                    .flatten()
                {
                    Some(code) => self.eval_code(rt, def.module, &code)?,
                    None => Value::Unit,
                },
            };
            out.push((Rc::clone(&self.field_names[d][i]), value));
        }
        Ok(Value::Struct(Rc::new(StructValue {
            type_name: Rc::clone(&self.def_names[d]),
            fields: out,
        })))
    }

    // ---------------------------------------------------------------- operators

    pub(crate) fn apply(&self, rt: &mut Runtime, op: Op, l: Value, r: Value) -> Outcome {
        use Value::*;
        let t = |rt: &Runtime, x: Result<i64, Fault>| x.map(Int).map_err(|f| traced(rt, f));
        Ok(match (op, &l, &r) {
            (Op::Add, Int(a), Int(b)) => return t(rt, rf::int_add(*a, *b)),
            (Op::Subtract, Int(a), Int(b)) => return t(rt, rf::int_subtract(*a, *b)),
            (Op::Multiply, Int(a), Int(b)) => return t(rt, rf::int_multiply(*a, *b)),
            (Op::Divide, Int(a), Int(b)) => return t(rt, rf::int_divide(*a, *b)),
            (Op::Remainder, Int(a), Int(b)) => return t(rt, rf::int_remainder(*a, *b)),
            (Op::Power, Int(a), Int(b)) => return t(rt, rf::int_power(*a, *b)),
            (Op::Less, Int(a), Int(b)) => Bool(a < b),
            (Op::LessEqual, Int(a), Int(b)) => Bool(a <= b),
            (Op::Greater, Int(a), Int(b)) => Bool(a > b),
            (Op::GreaterEqual, Int(a), Int(b)) => Bool(a >= b),
            (Op::Equal, Int(a), Int(b)) => Bool(a == b),
            (Op::NotEqual, Int(a), Int(b)) => Bool(a != b),
            (Op::Equal, Text(a), Text(b)) => Bool(a == b),
            (Op::NotEqual, Text(a), Text(b)) => Bool(a != b),
            (Op::Add, Float(a), Float(b)) => Float(a + b),
            (Op::Subtract, Float(a), Float(b)) => Float(a - b),
            (Op::Multiply, Float(a), Float(b)) => Float(a * b),
            (Op::Divide, Float(a), Float(b)) => Float(a / b),
            (Op::Remainder, Float(a), Float(b)) => Float(a % b),
            (Op::Power, Float(a), Float(b)) => Float(a.powf(*b)),
            (Op::Add, Text(_), Text(_)) => {
                let (Text(a), Text(b)) = (l, r) else {
                    return Err(internal(rt, "text + text"));
                };
                // A text that nothing else holds grows in place.
                Text(a.append(b.as_str()))
            }
            (Op::Equal, _, _) => Bool(self.equal(rt, &l, &r)?),
            (Op::NotEqual, _, _) => Bool(!self.equal(rt, &l, &r)?),
            (Op::Less | Op::LessEqual | Op::Greater | Op::GreaterEqual, _, _) => {
                let ord = self.compare(rt, &l, &r)?;
                Bool(match (op, ord) {
                    (_, None) => false,
                    (Op::Less, Some(o)) => o.is_lt(),
                    (Op::LessEqual, Some(o)) => o.is_le(),
                    (Op::Greater, Some(o)) => o.is_gt(),
                    (_, Some(o)) => o.is_ge(),
                })
            }
            (Op::In | Op::NotIn, Text(_) | Int(_), List(items))
                if items.iter().all(|x| matches!(x, Text(_) | Int(_))) =>
            {
                // List.contains compares with ==, which for texts and integers is equality
                // of the values themselves.
                let found = items.iter().any(|x| values_equal(x, &l));
                Bool(found == (op == Op::In))
            }
            (Op::In, _, _) => return self.call_method_value(rt, r, "contains", vec![l]),
            (Op::NotIn, _, _) => {
                let v = self.call_method_value(rt, r, "contains", vec![l])?;
                Bool(!matches!(v, Bool(true)))
            }
            _ => {
                let method = match op {
                    Op::Add => "add",
                    Op::Subtract => "subtract",
                    Op::Multiply => "multiply",
                    Op::Divide => "divide",
                    _ => {
                        return Err(internal(
                            rt,
                            format!("operator {} on {}", op.text(), l.kind()),
                        ))
                    }
                };
                return self.call_method_value(rt, l, method, vec![r]);
            }
        })
    }

    // ---------------------------------------------------------------- patterns

    pub(crate) fn matches<V: std::ops::IndexMut<usize, Output = Value> + ?Sized>(
        &self,
        rt: &mut Runtime,
        env: &mut V,
        pattern: &Pat,
        v: &Value,
    ) -> Result<bool, Unwind> {
        let inner;
        let v = match v {
            Value::Resource(cell) => {
                inner = cell.value.borrow().clone();
                &inner
            }
            other => other,
        };
        match pattern {
            Pat::Wildcard => Ok(true),
            Pat::Int(i) => Ok(matches!(v, Value::Int(x) if Some(*x) == *i)),
            Pat::Bool(b) => Ok(matches!(v, Value::Bool(c) if c == b)),
            Pat::Text(s) => Ok(matches!(v, Value::Text(t) if t.as_str() == s)),
            Pat::Name {
                last,
                fallback,
                known,
                pin,
            } => {
                match v {
                    Value::Enum(e) => {
                        // Names are shared, so the same address is the same variant.
                        if Rc::ptr_eq(&e.variant, last) {
                            return Ok(true);
                        }
                        let ty = Rc::as_ptr(&e.type_name) as *const u8 as usize;
                        if known.get() == ty {
                            return Ok(*e.variant == **last);
                        }
                        if self.enum_has_variant(&e.type_name, last) {
                            *pin.borrow_mut() = Some(Rc::clone(&e.type_name));
                            known.set(ty);
                            return Ok(*e.variant == **last);
                        }
                    }
                    Value::Struct(s)
                        if Rc::ptr_eq(&s.type_name, last) || *s.type_name == **last =>
                    {
                        return Ok(true)
                    }
                    _ => {}
                }
                match fallback {
                    NameFallback::Bind(slot) => {
                        env[*slot as usize] = v.clone();
                        Ok(true)
                    }
                    NameFallback::Const(c) => {
                        let k = self.constant(rt, *c)?;
                        Ok(values_equal(&k, v))
                    }
                    NameFallback::No => Ok(false),
                }
            }
            Pat::Constructor { last, args } => {
                let fields: &[(Rc<str>, Value)] = match v {
                    Value::Enum(e) if Rc::ptr_eq(&e.variant, last) || *e.variant == **last => {
                        &e.fields
                    }
                    Value::Struct(s)
                        if Rc::ptr_eq(&s.type_name, last) || *s.type_name == **last =>
                    {
                        &s.fields
                    }
                    _ => return Ok(false),
                };
                let mut position = 0;
                for (n, p) in args {
                    let value = match n {
                        Some(n) => fields.iter().find(|(f, _)| **f == **n).map(|(_, x)| x),
                        None => {
                            position += 1;
                            fields.get(position - 1).map(|(_, x)| x)
                        }
                    };
                    let Some(value) = value else { return Ok(false) };
                    if !self.matches(rt, env, p, value)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Pat::List {
                elements,
                fixed,
                rest,
            } => {
                let Value::List(items) = v else {
                    return Ok(false);
                };
                let fixed = *fixed;
                if !rest && items.len() != fixed || items.len() < fixed {
                    return Ok(false);
                }
                let mut i = 0;
                for element in elements {
                    match element {
                        ListPat::Pattern(p) => {
                            if !self.matches(rt, env, p, &items[i])? {
                                return Ok(false);
                            }
                            i += 1;
                        }
                        ListPat::Rest(slot) => {
                            let count = items.len() - fixed;
                            env[*slot as usize] = Value::list(items[i..i + count].to_vec());
                            i += count;
                        }
                    }
                }
                Ok(true)
            }
            Pat::Tuple(ps) => {
                let Value::Tuple(items) = v else {
                    return Ok(false);
                };
                if items.len() != ps.len() {
                    return Ok(false);
                }
                for (p, x) in ps.iter().zip(items.iter()) {
                    if !self.matches(rt, env, p, x)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Pat::Bind(pattern, slot) => {
                if self.matches(rt, env, pattern, v)? {
                    env[*slot as usize] = v.clone();
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Pat::Or(ps) => {
                for p in ps {
                    if self.matches(rt, env, p, v)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
    pub(crate) fn equal(&self, rt: &mut Runtime, l: &Value, r: &Value) -> Result<bool, Unwind> {
        if let (Value::Struct(_) | Value::Enum(_), true) = (l, true) {
            if let Some(&f) = self.method_of(l, "equals").as_ref() {
                if self.program.funcs[f].body.is_some() {
                    let v = self.call_method_value(rt, l.clone(), "equals", vec![r.clone()])?;
                    return Ok(matches!(v, Value::Bool(true)));
                }
            }
        }
        Ok(values_equal(l, r))
    }

    pub(crate) fn compare(
        &self,
        rt: &mut Runtime,
        l: &Value,
        r: &Value,
    ) -> Result<Option<std::cmp::Ordering>, Unwind> {
        use Value::*;
        Ok(match (l, r) {
            (Int(a), Int(b)) => Some(a.cmp(b)),
            (Float(a), Float(b)) => a.partial_cmp(b),
            (Text(a), Text(b)) => Some(a.as_str().cmp(b.as_str())),
            (Bool(a), Bool(b)) => Some(a.cmp(b)),
            (Path(a), Path(b)) => Some(a.as_str().cmp(b.as_str())),
            _ => match self.call_method_value(rt, l.clone(), "compare", vec![r.clone()])? {
                Int(c) => Some(c.cmp(&0)),
                _ => None,
            },
        })
    }

    /// The text of a value for interpolation (`Display`).
    pub fn display(&self, rt: &mut Runtime, v: &Value) -> Result<String, Unwind> {
        Ok(match v {
            Value::Text(t) => t.as_str().to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Path(p) => p.as_str().to_string(),
            // The same text as the externals Int.to_text and Float.to_text.
            Value::Int(i) => i.to_string(),
            Value::Float(x) => unio_runtime::externs::float_text::float_to_text(*x),
            Value::Resource(cell) => {
                let inner = cell.value.borrow().clone();
                self.display(rt, &inner)?
            }
            Value::Struct(s) => {
                if self.method(&s.type_name, "to_text").is_some() {
                    match self.call_method_value(rt, v.clone(), "to_text", Vec::new())? {
                        Value::Text(t) => t.as_str().to_string(),
                        _ => String::new(),
                    }
                } else {
                    let mut parts = Vec::new();
                    for (n, f) in &s.fields {
                        parts.push(format!("{n} = {}", self.display(rt, f)?));
                    }
                    format!("{}({})", short_type_name(&s.type_name), parts.join(", "))
                }
            }
            Value::Enum(e) => {
                if self.method(&e.type_name, "to_text").is_some() {
                    match self.call_method_value(rt, v.clone(), "to_text", Vec::new())? {
                        Value::Text(t) => t.as_str().to_string(),
                        _ => String::new(),
                    }
                } else if e.fields.is_empty() {
                    e.variant.to_string()
                } else {
                    let mut parts = Vec::new();
                    for (_, f) in &e.fields {
                        parts.push(self.display(rt, f)?);
                    }
                    format!("{}({})", e.variant, parts.join(", "))
                }
            }
            other => format!("{other:?}"),
        })
    }

    pub(crate) fn enum_has_variant(&self, type_name: &str, variant: &str) -> bool {
        self.variants
            .get(variant)
            .map(|ds| ds.iter().any(|d| &*self.def_names[*d] == type_name))
            .unwrap_or(false)
    }

    // ---------------------------------------------------------------- JSON (S-5, S-17)

    pub(crate) fn rt_type(&self, env_types: &Types, ty: &TypeCode) -> Result<RtTy, Unwind> {
        let args: Vec<RtTy> = ty
            .args
            .iter()
            .map(|a| self.rt_type(env_types, a))
            .collect::<Result<_, _>>()?;
        if let Some(t) = ty.generic.as_ref().and_then(|name| env_types.get(name)) {
            return Ok(t.clone());
        }
        let arg = |i: usize| Box::new(args.get(i).cloned().unwrap_or(RtTy::Unknown));
        Ok(match ty.base {
            TypeBase::Int => RtTy::Int,
            TypeBase::Float => RtTy::Float,
            TypeBase::Bool => RtTy::Bool,
            TypeBase::Text => RtTy::Text,
            TypeBase::Bytes => RtTy::Bytes,
            TypeBase::Path => RtTy::Path,
            TypeBase::List => RtTy::List(arg(0)),
            TypeBase::Map => RtTy::Map(arg(0), arg(1)),
            TypeBase::Set => RtTy::Set(arg(0)),
            TypeBase::Tuple => RtTy::Tuple(args),
            TypeBase::Def(d) => RtTy::Named(d, args),
            TypeBase::Unknown => RtTy::Unknown,
        })
    }

    pub(crate) fn ty_to_rt(&self, ty: &Ty, subst: &HashMap<usize, RtTy>) -> RtTy {
        use crate::types::Prim;
        match ty {
            Ty::Prim(Prim::Int) => RtTy::Int,
            Ty::Prim(Prim::Float) => RtTy::Float,
            Ty::Prim(Prim::Bool) => RtTy::Bool,
            Ty::Prim(Prim::Text) => RtTy::Text,
            Ty::Prim(Prim::Bytes) => RtTy::Bytes,
            Ty::Prim(Prim::Path) => RtTy::Path,
            Ty::List(t) => RtTy::List(Box::new(self.ty_to_rt(t, subst))),
            Ty::Set(t) => RtTy::Set(Box::new(self.ty_to_rt(t, subst))),
            Ty::Map(k, v) => RtTy::Map(
                Box::new(self.ty_to_rt(k, subst)),
                Box::new(self.ty_to_rt(v, subst)),
            ),
            Ty::Tuple(ts) => RtTy::Tuple(ts.iter().map(|t| self.ty_to_rt(t, subst)).collect()),
            Ty::Named(d, args) => {
                RtTy::Named(*d, args.iter().map(|t| self.ty_to_rt(t, subst)).collect())
            }
            Ty::Param(p) => subst.get(p).cloned().unwrap_or(RtTy::Unknown),
            _ => RtTy::Unknown,
        }
    }

    pub(crate) fn json(&self, variant: &str, field: Option<(&str, Value)>) -> Value {
        Value::Enum(Rc::new(EnumValue {
            type_name: Rc::from("Value"),
            variant: Rc::from(variant),
            fields: match field {
                Some((n, v)) => unio_runtime::value::Fields::One([(Rc::from(n), v)]),
                None => unio_runtime::value::Fields::Zero,
            },
        }))
    }

    pub(crate) fn encode_json(&self, rt: &mut Runtime, v: &Value) -> Outcome {
        Ok(match v {
            Value::Int(i) => self.json("Int", Some(("value", Value::Int(*i)))),
            Value::Float(f) => self.json("Float", Some(("value", Value::Float(*f)))),
            Value::Bool(b) => self.json("Bool", Some(("value", Value::Bool(*b)))),
            Value::Text(t) | Value::Path(t) => {
                self.json("Text", Some(("value", Value::Text(t.clone()))))
            }
            Value::List(l) | Value::Tuple(l) => {
                let items = l
                    .iter()
                    .map(|x| self.encode_json(rt, x))
                    .collect::<Result<Vec<_>, _>>()?;
                self.json("Array", Some(("items", Value::list(items))))
            }
            Value::Set(s) => {
                let items = s
                    .items()
                    .map(|x| self.encode_json(rt, x))
                    .collect::<Result<Vec<_>, _>>()?;
                self.json("Array", Some(("items", Value::list(items))))
            }
            Value::Map(m) => {
                let mut out = OrderedMap::new();
                for (k, x) in m.entries() {
                    let key = self.display(rt, k)?;
                    out.put(Value::text(key), self.encode_json(rt, x)?);
                }
                self.json("Object", Some(("fields", Value::map(out))))
            }
            Value::Struct(s) => {
                let mut out = OrderedMap::new();
                for (n, x) in &s.fields {
                    out.put(Value::text(n.to_string()), self.encode_json(rt, x)?);
                }
                self.json("Object", Some(("fields", Value::map(out))))
            }
            Value::Enum(e)
                if &*e.type_name == "Value" && self.enum_has_variant("Value", "Object") =>
            {
                v.clone()
            }
            Value::Enum(e) if &*e.type_name == "Option" => {
                if &*e.variant == "none" {
                    self.json("Null", None)
                } else {
                    let inner = e
                        .fields
                        .first()
                        .map(|(_, x)| x.clone())
                        .unwrap_or(Value::Unit);
                    self.encode_json(rt, &inner)?
                }
            }
            Value::Enum(e) => {
                if e.fields.is_empty() {
                    self.json("Text", Some(("value", Value::text(e.variant.to_string()))))
                } else {
                    let mut fields = OrderedMap::new();
                    for (n, x) in &e.fields {
                        fields.put(Value::text(n.to_string()), self.encode_json(rt, x)?);
                    }
                    let mut outer = OrderedMap::new();
                    outer.put(
                        Value::text(e.variant.to_string()),
                        self.json("Object", Some(("fields", Value::map(fields)))),
                    );
                    self.json("Object", Some(("fields", Value::map(outer))))
                }
            }
            Value::Unit => self.json("Null", None),
            other => {
                return Err(internal(
                    rt,
                    format_args!("{} has no JSON form", other.kind()),
                ))
            }
        })
    }

    pub(crate) fn decode_error(&self, path: &str, message: impl Into<String>) -> Unwind {
        Unwind::Error(Box::new(Value::record(
            "DecodeError",
            vec![
                ("path", Value::text(path.to_string())),
                ("message", Value::text(message.into())),
            ],
        )))
    }

    pub(crate) fn decode_json(
        &self,
        rt: &mut Runtime,
        ty: &RtTy,
        v: &Value,
        path: &str,
    ) -> Outcome {
        let (variant, payload) = match v {
            Value::Enum(e) if &*e.type_name == "Value" => (
                e.variant.to_string(),
                e.fields.first().map(|(_, x)| x.clone()),
            ),
            _ => return Err(self.decode_error(path, "not a JSON value")),
        };
        let wrong = |expected: &str| {
            self.decode_error(
                path,
                format!("expected {expected}, found {}", variant.to_lowercase()),
            )
        };
        match ty {
            RtTy::Unknown => Ok(v.clone()),
            RtTy::Int => match payload {
                Some(Value::Int(i)) if variant == "Int" => Ok(Value::Int(i)),
                _ => Err(wrong("an integer")),
            },
            RtTy::Float => match payload {
                Some(Value::Float(f)) => Ok(Value::Float(f)),
                Some(Value::Int(i)) if variant == "Int" && (i as f64) as i64 == i => {
                    Ok(Value::Float(i as f64))
                }
                _ => Err(wrong("a number")),
            },
            RtTy::Bool => match payload {
                Some(Value::Bool(b)) => Ok(Value::Bool(b)),
                _ => Err(wrong("a boolean")),
            },
            RtTy::Text | RtTy::Path => match payload {
                Some(Value::Text(t)) if variant == "Text" => Ok(if matches!(ty, RtTy::Path) {
                    Value::Path(t)
                } else {
                    Value::Text(t)
                }),
                _ => Err(wrong("text")),
            },
            RtTy::Bytes => Err(self.decode_error(path, "Bytes have no JSON form")),
            RtTy::List(t) | RtTy::Set(t) => match payload {
                Some(Value::List(items)) if variant == "Array" => {
                    let mut out = Vec::new();
                    for (i, x) in items.iter().enumerate() {
                        out.push(self.decode_json(rt, t, x, &format!("{path}/{i}"))?);
                    }
                    if matches!(ty, RtTy::Set(_)) {
                        let mut s = OrderedSet::new();
                        for x in out {
                            s.insert(x);
                        }
                        Ok(Value::set(s))
                    } else {
                        Ok(Value::list(out))
                    }
                }
                _ => Err(wrong("an array")),
            },
            RtTy::Tuple(ts) => match payload {
                Some(Value::List(items)) if variant == "Array" && items.len() == ts.len() => {
                    let mut out = Vec::new();
                    for (i, (t, x)) in ts.iter().zip(items.iter()).enumerate() {
                        out.push(self.decode_json(rt, t, x, &format!("{path}/{i}"))?);
                    }
                    Ok(Value::tuple(out))
                }
                _ => Err(wrong("an array of the tuple's length")),
            },
            RtTy::Map(_, vt) => match payload {
                Some(Value::Map(m)) if variant == "Object" => {
                    let mut out = OrderedMap::new();
                    for (k, x) in m.entries() {
                        let key = self.display(rt, k)?;
                        let value =
                            self.decode_json(rt, vt, x, &format!("{path}/{}", pointer(&key)))?;
                        out.put(k.clone(), value);
                    }
                    Ok(Value::map(out))
                }
                _ => Err(wrong("an object")),
            },
            RtTy::Named(d, args) => {
                let def = &self.program.defs[*d];
                let subst: HashMap<usize, RtTy> = def
                    .generics
                    .iter()
                    .copied()
                    .zip(args.iter().cloned())
                    .collect();
                match &def.kind {
                    DefKind::Enum(_)
                        if def.name == "Value" && self.enum_has_variant("Value", "Object") =>
                    {
                        Ok(v.clone())
                    }
                    DefKind::Enum(_) if def.name == "Option" => {
                        if variant == "Null" {
                            Ok(Value::none())
                        } else {
                            let inner = args.first().cloned().unwrap_or(RtTy::Unknown);
                            Ok(Value::some(self.decode_json(rt, &inner, v, path)?))
                        }
                    }
                    DefKind::Enum(vs) => match (variant.as_str(), payload) {
                        ("Text", Some(Value::Text(t))) => {
                            match vs
                                .iter()
                                .find(|x| x.name == t.as_str() && x.fields.is_empty())
                            {
                                Some(x) => Ok(self.make_variant(*d, &x.name, Vec::new())),
                                None => Err(self
                                    .decode_error(path, format!("unknown variant {}", t.as_str()))),
                            }
                        }
                        ("Object", Some(Value::Map(m))) if m.len() == 1 => {
                            let (k, inner) = m
                                .entries()
                                .next()
                                .map(|(k, x)| (k.clone(), x.clone()))
                                .unwrap_or((Value::Unit, Value::Unit));
                            let name = self.display(rt, &k)?;
                            let Some(x) = vs.iter().find(|x| x.name == name) else {
                                return Err(
                                    self.decode_error(path, format!("unknown variant {name}"))
                                );
                            };
                            let fields = match &inner {
                                Value::Enum(e) if &*e.variant == "Object" => match e.fields.first()
                                {
                                    Some((_, Value::Map(f))) => f.clone(),
                                    _ => return Err(wrong("an object")),
                                },
                                _ => return Err(wrong("an object")),
                            };
                            let mut values = Vec::new();
                            for (n, t) in &x.fields {
                                let sub = format!("{path}/{}/{}", pointer(&name), pointer(n));
                                match fields.get(&Value::text(n.clone())) {
                                    Some(fv) => values.push(self.decode_json(
                                        rt,
                                        &self.ty_to_rt(t, &subst),
                                        fv,
                                        &sub,
                                    )?),
                                    None => return Err(self.decode_error(&sub, "missing field")),
                                }
                            }
                            Ok(self.make_variant(*d, &x.name, values))
                        }
                        _ => Err(wrong("a variant name or an object with one variant")),
                    },
                    DefKind::Struct(fs) | DefKind::Error(fs) => {
                        // S-17: decoding never sets a private field, even where the check's
                        // E0046 was bypassed.
                        if let Some(f) = fs.iter().find(|f| !f.public) {
                            return Err(self.decode_error(
                                path,
                                format!(
                                    "{} has the private field '{}', which JSON never sets",
                                    def.name, f.name
                                ),
                            ));
                        }
                        let Some(Value::Map(m)) = payload.filter(|_| variant == "Object") else {
                            return Err(wrong("an object"));
                        };
                        for (k, _) in m.entries() {
                            let key = self.display(rt, k)?;
                            if !fs.iter().any(|f| f.name == key) {
                                return Err(self.decode_error(
                                    &format!("{path}/{}", pointer(&key)),
                                    "unknown field",
                                ));
                            }
                        }
                        let mut values = Vec::new();
                        for f in fs {
                            let sub = format!("{path}/{}", pointer(&f.name));
                            match m.get(&Value::text(f.name.clone())) {
                                Some(fv) => values.push(Some(self.decode_json(
                                    rt,
                                    &self.ty_to_rt(&f.ty, &subst),
                                    fv,
                                    &sub,
                                )?)),
                                None if f.has_default => values.push(None),
                                None => return Err(self.decode_error(&sub, "missing field")),
                            }
                        }
                        self.finish_struct(rt, *d, values)
                    }
                    _ => Err(self.decode_error(path, format!("{} cannot be decoded", def.name))),
                }
            }
        }
    }
}

pub(crate) enum Cursor {
    List(Rc<unio_runtime::heap::Counted<Vec<Value>>>, usize),
    Text(unio_runtime::value::Text, usize),
    Items(std::vec::IntoIter<Value>),
    Range(i64, i64),
    Iterator(Value),
}

/// A named function used as a value.
pub(crate) struct FnRef {
    pub(crate) interp: Rc<Interp>,
    pub(crate) f: FnId,
}

impl Callable for FnRef {
    fn call(&self, runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
        let total = self.interp.program.funcs[self.f].params.len()
            + usize::from(self.interp.program.funcs[self.f].self_param.is_some());
        let mut args: Vec<Value> = arguments.iter_mut().map(take).collect();
        while args.len() < total {
            args.push(Value::Unit);
        }
        let result = self.interp.call_fn(
            runtime,
            self.f,
            &mut args,
            Rc::clone(&self.interp.empty_types),
        );
        for (slot, v) in arguments.iter_mut().zip(args) {
            *slot = v;
        }
        result
    }
    fn name(&self) -> &str {
        &self.interp.program.funcs[self.f].name
    }
}

/// An anonymous function with the values it captured (L-11a).
pub(crate) struct Closure {
    interp: Rc<Interp>,
    code: Rc<LambdaCode>,
    captured: Vec<Value>,
    module: ModId,
    types: Types,
}

impl Callable for Closure {
    fn call(&self, runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
        let mut vars = vec![Value::Unit; self.code.slots];
        for (slot, value) in vars.iter_mut().zip(arguments.iter().take(self.code.params)) {
            *slot = value.clone();
        }
        for ((_, inner), value) in self.code.captures.iter().zip(&self.captured) {
            vars[*inner as usize] = value.clone();
        }
        let mut env = Env {
            vars: Slots::new(vars),
            module: self.module,
            types: Rc::clone(&self.types),
        };
        self.interp.eval(runtime, &mut env, &self.code.body)
    }
    fn name(&self) -> &str {
        "function"
    }
}

// ---------------------------------------------------------------- helpers

pub(crate) fn ty_key(program: &Program, ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Prim(p) => p.name().to_string(),
        Ty::List(_) => "List".into(),
        Ty::Map(..) => "Map".into(),
        Ty::Set(_) => "Set".into(),
        Ty::Tuple(_) => "Tuple".into(),
        Ty::Named(d, _) => type_identity(program, *d),
        _ => return None,
    })
}

/// The identity of a declared type at run time, which its values carry and by which its methods,
/// handlers and patterns are found: the name of a type of the standard library, whose names are
/// unique there and which the externals use, and `module.Name` for a type of a package, so that
/// types of one name in two modules stay apart. Should two packages of one
/// program have modules of the same name, the module's number tells them apart.
pub(crate) fn type_identity(program: &Program, d: DefId) -> String {
    let def = &program.defs[d];
    let module = &program.modules[def.module];
    if module.stdlib {
        return def.name.clone();
    }
    let clash = program
        .modules
        .iter()
        .enumerate()
        .any(|(m, other)| m != def.module && !other.stdlib && other.name == module.name);
    if clash {
        format!("{}@{}.{}", module.name, def.module, def.name)
    } else {
        format!("{}.{}", module.name, def.name)
    }
}

/// The name of a type as a program writes it, from its identity: `Node` for `trees.Node`.
pub(crate) fn short_type_name(identity: &str) -> &str {
    identity.rsplit('.').next().unwrap_or(identity)
}

/// The type name of a value that is not a struct, an enum, or a resource.
pub(crate) fn static_key(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "Int",
        Value::Float(_) => "Float",
        Value::Bool(_) => "Bool",
        Value::Text(_) => "Text",
        Value::Bytes(_) => "Bytes",
        Value::Path(_) => "Path",
        Value::List(_) => "List",
        Value::Tuple(_) => "Tuple",
        Value::Map(_) => "Map",
        Value::Set(_) => "Set",
        Value::Function(_) => "Function",
        Value::Unit => "Unit",
        Value::Struct(_) | Value::Enum(_) | Value::Resource(_) => "",
    }
}

pub(crate) fn value_key(v: &Value) -> String {
    match v {
        Value::Int(_) => "Int".into(),
        Value::Float(_) => "Float".into(),
        Value::Bool(_) => "Bool".into(),
        Value::Text(_) => "Text".into(),
        Value::Bytes(_) => "Bytes".into(),
        Value::Path(_) => "Path".into(),
        Value::List(_) => "List".into(),
        Value::Tuple(_) => "Tuple".into(),
        Value::Map(_) => "Map".into(),
        Value::Set(_) => "Set".into(),
        Value::Struct(s) => short_type_name(&s.type_name).to_string(),
        Value::Enum(e) => short_type_name(&e.type_name).to_string(),
        Value::Resource(cell) => value_key(&cell.value.borrow()),
        Value::Function(_) => "Function".into(),
        Value::Unit => "Unit".into(),
    }
}

pub(crate) fn parse_int(text: &str) -> Option<i64> {
    let t = text.replace('_', "");
    if let Some(h) = t.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()
    } else if let Some(b) = t.strip_prefix("0b") {
        i64::from_str_radix(b, 2).ok()
    } else {
        t.parse().ok()
    }
}

pub(crate) fn pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// A local or a literal, read without copying it.
#[inline]
pub(crate) fn peek<'e>(env: &'e Env, node: &'e Node) -> Option<&'e Value> {
    match node {
        Node::Local(slot) => Some(&env.vars[*slot as usize]),
        Node::Value(v) => Some(v),
        _ => None,
    }
}

/// Whether a node always gives a `Bool`, so that `or` with it on the left is boolean.
pub(crate) fn gives_bool(node: &Node) -> bool {
    match node {
        Node::Binary(op, _, _) => !matches!(
            op,
            Op::Add | Op::Subtract | Op::Multiply | Op::Divide | Op::Remainder | Op::Power
        ),
        Node::And(..) | Node::Not(_) | Node::Is(..) | Node::Value(Value::Bool(_)) => true,
        Node::Or(left, _, _) => gives_bool(left),
        _ => false,
    }
}

/// An integer expression made only of locals, literals, and operators, when every step
/// succeeds; `None` sends it to the general path, which evaluates it again and reports the
/// fault. Evaluating such an expression has no effects, so evaluating it twice is not observable.
#[inline]
pub(crate) fn fast_int(env: &Env, node: &Node) -> Option<i64> {
    match node {
        Node::Local(slot) => match env.vars[*slot as usize] {
            Value::Int(i) => Some(i),
            _ => None,
        },
        Node::Value(Value::Int(i)) => Some(*i),
        Node::Field(target, name, at) => {
            let Node::Local(slot) = **target else {
                return None;
            };
            let Value::Struct(s) = &env.vars[slot as usize] else {
                return None;
            };
            match s.fields.get(at.get() as usize) {
                Some((n, Value::Int(i))) if Rc::ptr_eq(n, name) => Some(*i),
                _ => None,
            }
        }
        Node::Binary(op, l, r) => {
            let a = fast_int(env, l)?;
            let b = fast_int(env, r)?;
            match op {
                Op::Add => a.checked_add(b),
                Op::Subtract => a.checked_sub(b),
                Op::Multiply => a.checked_mul(b),
                Op::Divide if b != 0 => a.checked_div(b),
                Op::Remainder if b != 0 => Some(a.checked_rem(b).unwrap_or(0)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The value of `left op right` when both sides are integer expressions of [`fast_int`].
#[inline]
pub(crate) fn fast_binary(env: &Env, op: Op, left: &Node, right: &Node) -> Option<Value> {
    let a = fast_int(env, left)?;
    let b = fast_int(env, right)?;
    Some(match op {
        Op::Add => Value::Int(a.checked_add(b)?),
        Op::Subtract => Value::Int(a.checked_sub(b)?),
        Op::Multiply => Value::Int(a.checked_mul(b)?),
        Op::Divide if b != 0 => Value::Int(a.checked_div(b)?),
        Op::Remainder if b != 0 => Value::Int(a.checked_rem(b).unwrap_or(0)),
        Op::Equal => Value::Bool(a == b),
        Op::NotEqual => Value::Bool(a != b),
        Op::Less => Value::Bool(a < b),
        Op::LessEqual => Value::Bool(a <= b),
        Op::Greater => Value::Bool(a > b),
        Op::GreaterEqual => Value::Bool(a >= b),
        _ => return None,
    })
}

/// An operator on two integers.
#[inline]
pub(crate) fn int_op(rt: &Runtime, op: Op, a: i64, b: i64) -> Outcome {
    let t = |x: Result<i64, Fault>| x.map(Value::Int).map_err(|f| traced(rt, f));
    Ok(Value::Bool(match op {
        Op::Add => return t(rf::int_add(a, b)),
        Op::Subtract => return t(rf::int_subtract(a, b)),
        Op::Multiply => return t(rf::int_multiply(a, b)),
        Op::Divide => return t(rf::int_divide(a, b)),
        Op::Remainder => return t(rf::int_remainder(a, b)),
        Op::Power => return t(rf::int_power(a, b)),
        Op::Equal => a == b,
        Op::NotEqual => a != b,
        Op::Less => a < b,
        Op::LessEqual => a <= b,
        Op::Greater => a > b,
        Op::GreaterEqual => a >= b,
        Op::In | Op::NotIn => return Err(internal(rt, "in on Int")),
    }))
}

/// A comparison of integers, texts, or booleans, which have no methods that change it.
#[inline]
pub(crate) fn compare_simple(op: Op, a: &Value, b: &Value) -> Option<bool> {
    use std::cmp::Ordering;
    let ord: Ordering = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Text(x), Value::Text(y)) => match op {
            Op::Equal => return Some(x == y),
            Op::NotEqual => return Some(x != y),
            _ => x.as_str().cmp(y.as_str()),
        },
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => return None,
    };
    Some(match op {
        Op::Equal => ord.is_eq(),
        Op::NotEqual => ord.is_ne(),
        Op::Less => ord.is_lt(),
        Op::LessEqual => ord.is_le(),
        Op::Greater => ord.is_gt(),
        Op::GreaterEqual => ord.is_ge(),
        _ => return None,
    })
}

/// Clears the slots a block bound, so that their values are freed and unshared as early as the
/// scopes of the language allow.
#[inline]
pub(crate) fn clear_block(env: &mut Env, block: &Block) {
    let (start, end) = block.clear;
    for v in &mut env.vars[start as usize..end as usize] {
        *v = Value::Unit;
    }
}

/// Binds the item of an iteration to one name, or a tuple to several.
pub(crate) fn bind_names<V: std::ops::IndexMut<usize, Output = Value> + ?Sized>(
    rt: &Runtime,
    env: &mut V,
    names: &[Slot],
    item: Value,
) -> Result<(), Unwind> {
    if let [slot] = names {
        env[*slot as usize] = item;
        return Ok(());
    }
    match item {
        Value::Tuple(t) => {
            for (slot, v) in names.iter().zip(t.iter()) {
                env[*slot as usize] = v.clone();
            }
            Ok(())
        }
        other => Err(internal(
            rt,
            format!("cannot destructure a {}", other.kind()),
        )),
    }
}

/// Whether a handler for the error type named `ty` receives the error.
pub(crate) fn handles(ty: &str, error: &Value) -> bool {
    if ty == "Error" {
        return true;
    }
    match error {
        Value::Struct(s) => &*s.type_name == ty,
        Value::Enum(e) => &*e.type_name == ty,
        _ => false,
    }
}

/// The field at the end of a path of field names, copying shared levels on the way.
pub(crate) fn navigate_fields<'a>(
    rt: &Runtime,
    v: &'a mut Value,
    steps: &[PStep],
) -> Result<&'a mut Value, Unwind> {
    let Some((PStep::Field(n), rest)) = steps.split_first() else {
        return Ok(v);
    };
    let next: &'a mut Value = match v {
        Value::Struct(s) => {
            let s = Rc::make_mut(s);
            match field_index(&s.fields, n).map(|i| &mut s.fields[i]) {
                Some((_, x)) => x,
                None => return Err(internal(rt, format_args!("no field {n}"))),
            }
        }
        other => {
            return Err(internal(
                rt,
                format!("cannot change a part of a {}", other.kind()),
            ))
        }
    };
    navigate_fields(rt, next, rest)
}

/// The position of a field: by the shared name first, then by its text.
#[inline]
pub(crate) fn field_index(fields: &[(Rc<str>, Value)], name: &Rc<str>) -> Option<usize> {
    fields
        .iter()
        .position(|(k, _)| Rc::ptr_eq(k, name))
        .or_else(|| fields.iter().position(|(k, _)| **k == **name))
}

pub(crate) fn navigate<'a>(
    rt: &Runtime,
    v: &'a mut Value,
    steps: &[Step],
) -> Result<&'a mut Value, Unwind> {
    let Some((first, rest)) = steps.split_first() else {
        return Ok(v);
    };
    let next: &'a mut Value = match (v, first) {
        (Value::Struct(s), Step::Field(n)) => {
            let s = Rc::make_mut(s);
            match field_index(&s.fields, n).map(|i| &mut s.fields[i]) {
                Some((_, x)) => x,
                None => return Err(internal(rt, format_args!("no field {n}"))),
            }
        }
        (Value::List(l), Step::Index(Value::Int(i))) => {
            let len = l.len();
            let l = Rc::make_mut(l);
            match usize::try_from(*i).ok().and_then(|i| l.get_mut(i)) {
                Some(x) => x,
                None => {
                    return Err(rt.fault(
                        FaultKind::IndexOutOfRange,
                        format_args!("index {i} is outside 0..{len}"),
                    ))
                }
            }
        }
        (Value::Resource(cell), _) => {
            // A resource field reached through a struct; change it inside its cell.
            let _ = cell;
            return Err(internal(rt, "nested resource place"));
        }
        (other, _) => {
            return Err(internal(
                rt,
                format!("cannot change a part of a {}", other.kind()),
            ))
        }
    };
    navigate(rt, next, rest)
}

/// A field read at a place in the code that remembers the position the field had last time:
/// values of one type keep their fields in declaration order, and field names are shared, so a
/// pointer comparison confirms the position.
#[inline]
pub(crate) fn field_cached(rt: &Runtime, v: &Value, name: &Rc<str>, at: &Cell<u32>) -> Outcome {
    let fields = match v {
        Value::Struct(s) => &s.fields[..],
        Value::Enum(e) => &e.fields[..],
        _ => return field_of(rt, v, name),
    };
    if let Some((n, x)) = fields.get(at.get() as usize) {
        if Rc::ptr_eq(n, name) {
            return Ok(x.clone());
        }
    }
    match field_index(fields, name) {
        Some(i) => {
            at.set(i as u32);
            Ok(fields[i].1.clone())
        }
        None => field_of(rt, v, name),
    }
}

pub(crate) fn field_of(rt: &Runtime, v: &Value, name: &str) -> Outcome {
    match v {
        Value::Struct(s) => s
            .fields
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, x)| x.clone())
            .ok_or_else(|| internal(rt, format_args!("no field {name} in {}", s.type_name))),
        Value::Enum(e) => e
            .fields
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, x)| x.clone())
            .ok_or_else(|| internal(rt, format_args!("no field {name} in {}", e.variant))),
        Value::Resource(cell) => field_of(rt, &cell.value.borrow(), name),
        other => Err(internal(
            rt,
            format!("no field {name} in a {}", other.kind()),
        )),
    }
}

pub(crate) fn index_value(rt: &Runtime, t: &Value, i: &Value) -> Outcome {
    match (t, i) {
        (Value::List(l), Value::Int(i)) => usize::try_from(*i)
            .ok()
            .and_then(|i| l.get(i))
            .cloned()
            .ok_or_else(|| {
                rt.fault(
                    FaultKind::IndexOutOfRange,
                    format_args!("index {i} is outside 0..{}", l.len()),
                )
            }),
        (Value::Map(m), key) => m
            .get(key)
            .cloned()
            .ok_or_else(|| rt.fault(FaultKind::MissingKey, "the map has no such key")),
        (other, _) => Err(internal(
            rt,
            format_args!("cannot index a {}", other.kind()),
        )),
    }
}

/// Text of a test description.
pub fn description(parts: &[TextPart]) -> String {
    parts
        .iter()
        .map(|p| match p {
            TextPart::Literal(s) => s.clone(),
            TextPart::Interpolation(_) => "{…}".into(),
        })
        .collect()
}

// ---------------------------------------------------------------- programs and tests

/// Runs `public function main()` of the first module (`unio run`).
pub fn run_main(interp: &Rc<Interp>, rt: &mut Runtime, module: ModId) -> i32 {
    let main =
        interp.program.funcs.iter().position(|f| {
            f.module == module && f.name == "main" && matches!(f.owner, Owner::Module)
        });
    let Some(main) = main else {
        rt.write_error("unio: the module has no public function main\n");
        return 2;
    };
    let mut args = Vec::new();
    match interp.call_fn(rt, main, &mut args, Rc::clone(&interp.empty_types)) {
        Ok(v) => match rf::main_exit_code(&v) {
            Ok(code) => code,
            Err(fault) => end(rt, Unwind::Fault(Box::new(fault))),
        },
        Err(e) => end(rt, e),
    }
}

pub(crate) fn end(rt: &mut Runtime, e: Unwind) -> i32 {
    match e {
        // The fault was built on the reserve for `release`, and the releases ran from it; if
        // they overflowed it on a full heap, the program ends as exhausted (C-17, C-18).
        Unwind::Fault(_) if rt.check_heap().is_err() => {
            rt.write_error(rf::Exhaustion::Heap.message());
            rf::EXIT_EXHAUSTED
        }
        Unwind::Fault(f) => {
            rt.write_fault(&f);
            rf::EXIT_FAULT
        }
        Unwind::Exhausted(x) => {
            rt.write_error(x.message());
            rf::EXIT_EXHAUSTED
        }
        Unwind::Error(_) => {
            rt.write_error("fault: an error reached the top of the program\n");
            rf::EXIT_FAULT
        }
    }
}

/// The result of one test block (T-4).
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
    pub output: String,
    pub millis: u128,
}

/// Runs one test block of a module in a fresh runtime with captured output.
pub fn run_test(interp: &Rc<Interp>, module: ModId, item: usize) -> TestResult {
    let Item::Test(test) = &interp.program.modules[module].analysis.file.items[item] else {
        unreachable!("a test item")
    };
    let name = description(&test.description);
    let (mut rt, out, err) = Runtime::captured(Vec::new());
    let started = std::time::Instant::now();
    let label = format!("test \"{name}\"");
    let mut lower = Lowering::new(interp, module);
    let body = lower.block(&test.body);
    let mut env = Env {
        vars: Slots::new(vec![Value::Unit; lower.slots()]),
        module,
        types: Rc::clone(&interp.empty_types),
    };
    let result = rt
        .enter(
            label.as_str(),
            Rc::clone(&interp.files[module]),
            test.span.start.line as u32,
        )
        .and_then(|_| interp.run_block(&mut rt, &mut env, &body));
    let millis = started.elapsed().as_millis();
    // The test has ended, and its releases ran from the reserve (C-17). The result is the
    // harness's report: its text is made outside the reserve, as the captured output is.
    let result = match result {
        Err(Unwind::Fault(_)) if rt.check_heap().is_err() => {
            Err(Unwind::Exhausted(rf::Exhaustion::Heap))
        }
        other => other,
    };
    drop(env);
    let (passed, message) = match result {
        Ok(_) => (true, String::new()),
        Err(Unwind::Fault(f)) => (false, unio_runtime::reserve::outside(|| f.render())),
        Err(Unwind::Exhausted(x)) => (false, x.message().to_string()),
        Err(Unwind::Error(e)) => {
            let text = interp.display(&mut rt, &e).unwrap_or_default();
            (false, format!("an error was not handled: {text}\n"))
        }
    };
    let mut output = String::from_utf8_lossy(&out.borrow()).into_owned();
    output.push_str(&String::from_utf8_lossy(&err.borrow()));
    TestResult {
        name,
        passed,
        message,
        output,
        millis,
    }
}

/// A library function of the prelude that the interpreter runs natively. Each gives the result
/// of its Unio definition in `stdlib/`, which stays the specification and runs where an
/// intrinsic declines (for example `contains` on structs with their own equality).
#[derive(Clone, Copy)]
pub(crate) enum Intrinsic {
    IsEmpty,
    ToInt,
    First,
    Last,
    Enumerate,
    Contains,
    Keys,
    Values,
    AppendAll,
    Range(DefId),
}

impl Intrinsic {
    fn of(label: &str, range: Option<DefId>) -> Option<Intrinsic> {
        let (module, name) = label.rsplit_once('.')?;
        let kind = module.strip_prefix("prelude:alloc/builtin/");
        Some(match (kind, name) {
            (Some("text" | "list" | "map" | "set" | "bytes"), "is_empty") => Intrinsic::IsEmpty,
            (Some("text"), "to_int") => Intrinsic::ToInt,
            (Some("list"), "first") => Intrinsic::First,
            (Some("list"), "last") => Intrinsic::Last,
            (Some("list"), "enumerate") => Intrinsic::Enumerate,
            (Some("list" | "map"), "contains") => Intrinsic::Contains,
            (Some("map"), "keys") => Intrinsic::Keys,
            (Some("map"), "values") => Intrinsic::Values,
            (Some("list"), "append_all") => Intrinsic::AppendAll,
            (None, "range") if module == "prelude:core/prelude" => Intrinsic::Range(range?),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The peak use of the reserve for `release` (C-17, plan section 12 condition 1) in its
    /// three measured cases, each on a heap the test hook filled to the limit, so that the whole
    /// path of the fault runs from the reserve: deep unwinding (a `using` block in each of 9,000
    /// nested calls, each `release` building texts), repeated faults (on the same stack, every
    /// `release` meets an error it handles, and the innermost ten fault after it, each with a
    /// trace of about 9,000 frames), and a diagnostic with a long trace (a fault 9,990 calls
    /// deep, under a long function name and file path). Each peak is reported and must fit in
    /// the reserve; the figures are recorded in docs/runtime.md 3.6.
    #[test]
    fn the_reserve_peak_is_measured() {
        use unio_runtime::reserve;
        const RESOURCES: &str = "error Oops:
    reason as Text

function risky(n as Int) returns Int fails with Oops:
    if n % 2 == 0:
        fail Oops(reason = \"even {n}\")
    return n

struct Res:
    name as Text
    depth as Int

extend Res with Resource:
    function release(var self):
        let words = [\"{self.name} {self.depth} {i}\" for i in range(0, 20)]
        let value = try risky(self.depth) on Oops as e:
            if FAULTING and self.depth < 20 and e.reason.length() > 0:
                print(words[100])
            return
        assert value == self.depth

function nest(depth as Int) returns Int:
    using r = Res(name = \"level\", depth = depth):
        if depth == 0:
            return 1 / depth
        return nest(depth - 1) + 1

public function main() returns Int:
    return nest(9000)
";
        let cases = [
            (
                "deep unwinding",
                format!("let FAULTING as Bool = false\n\n{RESOURCES}"),
            ),
            (
                "repeated faults",
                format!("let FAULTING as Bool = true\n\n{RESOURCES}"),
            ),
            (
                "long trace",
                "function a_function_with_a_rather_long_name_to_make_every_line_of_the_trace_long(n as Int) returns Int:
    if n == 0:
        return 1 / n
    return a_function_with_a_rather_long_name_to_make_every_line_of_the_trace_long(n - 1) + 1

public function main() returns Int:
    return a_function_with_a_rather_long_name_to_make_every_line_of_the_trace_long(9990)
"
                .to_string(),
            ),
        ];
        let dir = std::env::temp_dir()
            .join(format!("unio-reserve-peak-{}", std::process::id()))
            .join("a-directory-with-a-long-name-so-that-the-file-of-each-frame-is-long");
        std::fs::create_dir_all(&dir).unwrap();
        let mut report = Vec::new();
        for (name, source) in cases {
            let file = dir.join(format!("{}.unio", name.replace(' ', "_")));
            std::fs::write(&file, source).unwrap();
            let measured = std::thread::Builder::new()
                .stack_size(1 << 30)
                .spawn(move || {
                    let program =
                        crate::load_for_run(&[file], crate::effects::Profile::Application)
                            .unwrap_or_else(|a| {
                                panic!("{}", a[0].render_text(crate::diag::Lang::En))
                            });
                    let module = program.modules.iter().position(|m| m.report).unwrap();
                    let interp = Interp::new(program);
                    let (mut rt, _out, err) = Runtime::captured(Vec::new());
                    rt.heap_limit = Some(reserve::MINIMUM_HEAP_LIMIT);
                    unio_runtime::heap::fill_to(reserve::MINIMUM_HEAP_LIMIT);
                    reserve::reset_peak();
                    let code = run_main(&interp, &mut rt, module);
                    let peak = reserve::peak();
                    unio_runtime::heap::clear_ballast();
                    let err = String::from_utf8_lossy(&err.borrow()).into_owned();
                    (code, peak, err.lines().count(), err.len())
                })
                .unwrap()
                .join()
                .unwrap();
            let (code, peak, lines, bytes) = measured;
            println!(
                "reserve peak, {name}: {peak} bytes of {} ({:.1}%), margin {} bytes; \
                 standard error {lines} lines, {bytes} bytes",
                reserve::USABLE_BYTES,
                100.0 * peak as f64 / reserve::USABLE_BYTES as f64,
                reserve::USABLE_BYTES.saturating_sub(peak)
            );
            report.push((name, code, peak, lines));
        }
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        for (name, code, peak, lines) in report {
            assert_eq!(
                code, 101,
                "{name}: the fault ends the program, not exhaustion"
            );
            assert!(
                lines > 9000,
                "{name}: the whole trace was written ({lines} lines)"
            );
            assert!(peak > 0, "{name}: the path ran from the reserve");
            assert!(
                peak <= reserve::USABLE_BYTES,
                "{name}: the peak of {peak} bytes exceeds the reserve"
            );
        }
    }

    /// Every external function the standard library declares has an implementation in the
    /// runtime, so a checked program never reaches "the runtime has no external function"
    /// (S-7; outside the library an external function is E0086).
    #[test]
    fn every_external_of_the_standard_library_is_bound() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
        let mut files = Vec::new();
        for layer in ["core", "alloc", "std"] {
            files.extend(crate::vfs::unio_files_in(&root.join(layer)));
            files.extend(crate::vfs::unio_files_in(&root.join(layer).join("builtin")));
        }
        let unbound = crate::on_front_end_stack(move || {
            let program = crate::load_checked(&files, crate::effects::Profile::Application);
            let interp = Interp::new(program);
            let program = &interp.program;
            let externals: Vec<FnId> = (0..program.funcs.len())
                .filter(|&f| {
                    let def = &program.funcs[f];
                    def.body.is_none()
                        && !matches!(def.owner, Owner::Interface(_))
                        && program.modules[def.module].stdlib
                })
                .collect();
            let unbound: Vec<String> = externals
                .iter()
                .filter(|&&f| interp.externs[f].is_none() && interp.externs_json[f] == 0)
                .map(|&f| interp.external_names[f].clone())
                .collect();
            (externals.len(), unbound)
        });
        let (externals, unbound) = unbound;
        assert!(
            externals > 50,
            "the library declares its externals: {externals}"
        );
        assert!(unbound.is_empty(), "no runtime binding: {unbound:?}");
    }
}
