//! The interpreter of sprint S5 (docs/runtime.md). It evaluates a checked program directly on
//! its syntax tree, using the resolved declarations of the checker (`Program`) for names,
//! methods, constants, and defaults, and the value model, faults, and external functions of
//! `unio-runtime`. Evaluation is strict and left to right (section 1).
//!
//! The "IR" of runtime.md 8.1 is, in this first interpreter, the checked tree together with the
//! tables built in [`Interp::new`]: methods by type and name, signatures and declarations by
//! position, variants by name. Lowering to a separate representation is left to the native
//! code generator of S6, which needs it anyway.

use crate::ast::*;
use crate::program::{DefKind, Owner, Program, Symbol};
use crate::types::Ty;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use unio_runtime::fault::{self as rf, Fault, FaultKind, Outcome, Unwind};
use unio_runtime::runtime::Runtime;
use unio_runtime::value::{
    values_equal, Callable, EnumValue, OrderedMap, OrderedSet, StructValue, Value,
};

type ModId = usize;
type FnId = usize;
type DefId = usize;

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

type Types = Rc<HashMap<String, RtTy>>;

#[derive(Clone)]
pub struct Env {
    vars: Vec<(Rc<str>, Value)>,
    module: ModId,
    types: Types,
}

impl Env {
    fn find(&self, name: &str) -> Option<usize> {
        self.vars.iter().rposition(|(n, _)| &**n == name)
    }
    fn get(&self, name: &str) -> Option<&Value> {
        self.find(name).map(|i| &self.vars[i].1)
    }
    fn bind(&mut self, name: &str, value: Value) {
        self.vars.push((Rc::from(name), value));
    }
}

enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

type Exec = Result<Flow, Unwind>;

/// Arguments in parameter order, and the places that `var` arguments are written back to.
type BoundArgs = (Vec<Value>, Vec<(usize, Expr)>);

#[derive(Clone)]
enum Step {
    Field(Rc<str>),
    Index(Value),
}

fn take(v: &mut Value) -> Value {
    std::mem::replace(v, Value::Unit)
}

fn internal(rt: &Runtime, message: impl Into<String>) -> Unwind {
    rt.fault(
        FaultKind::InvalidArgument,
        format!("internal: {}", message.into()),
    )
}

fn traced(rt: &Runtime, fault: Fault) -> Unwind {
    rt.fault(fault.kind, fault.message)
}

/// The interpreter: the checked program and the tables derived from it.
pub struct Interp {
    me: Weak<Interp>,
    pub program: Program,
    methods: HashMap<String, HashMap<String, FnId>>,
    /// Trace label `module.function` of each function.
    labels: Vec<Rc<str>>,
    /// Binding-table name of each function, used for externals.
    external_names: Vec<String>,
    signatures: HashMap<(ModId, usize), (usize, Option<usize>)>,
    decls: HashMap<(ModId, String), usize>,
    variants: HashMap<String, Vec<DefId>>,
    consts: RefCell<HashMap<usize, Value>>,
    lambdas: RefCell<HashMap<usize, Rc<Expr>>>,
    files: Vec<Rc<str>>,
}

impl Interp {
    pub fn new(program: Program) -> Rc<Interp> {
        let mut methods = HashMap::new();
        for imp in &program.impls {
            let Some(key) = ty_key(&program, &imp.target) else {
                continue;
            };
            for &f in &imp.methods {
                methods
                    .entry(key.clone())
                    .or_insert_with(HashMap::new)
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
                                .or_insert_with(HashMap::new)
                                .entry(program.funcs[f].name.clone())
                                .or_insert(f);
                        }
                    }
                }
            }
        }
        let mut signatures = HashMap::new();
        let mut decls = HashMap::new();
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
        let mut variants: HashMap<String, Vec<DefId>> = HashMap::new();
        for (d, def) in program.defs.iter().enumerate() {
            if let DefKind::Enum(vs) = &def.kind {
                for v in vs {
                    variants.entry(v.name.clone()).or_default().push(d);
                }
            }
        }
        let files = program
            .modules
            .iter()
            .map(|m| Rc::from(m.path.display().to_string()))
            .collect();
        let labels = program
            .funcs
            .iter()
            .map(|f| Rc::from(format!("{}.{}", program.modules[f.module].name, f.name)))
            .collect();
        let external_names = (0..program.funcs.len())
            .map(|f| Interp::external_name(&program, f))
            .collect();
        Rc::new_cyclic(|me| Interp {
            labels,
            external_names,
            me: me.clone(),
            program,
            methods,
            signatures,
            decls,
            variants,
            consts: RefCell::new(HashMap::new()),
            lambdas: RefCell::new(HashMap::new()),
            files,
        })
    }

    fn rc(&self) -> Rc<Interp> {
        self.me.upgrade().expect("the interpreter is alive")
    }

    fn signature(&self, f: FnId) -> Option<(&Signature, Option<&Block>)> {
        let def = &self.program.funcs[f];
        let &(item, member) = self.signatures.get(&(def.module, def.span.start.offset))?;
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

    fn fields_of(&self, d: DefId) -> Option<&Vec<Field>> {
        let def = &self.program.defs[d];
        let &i = self.decls.get(&(def.module, def.name.clone()))?;
        match &self.program.modules[def.module].analysis.file.items[i] {
            Item::Declaration(decl) => match &decl.kind {
                DeclarationKind::Struct(s) => s.fields.as_ref(),
                DeclarationKind::Error(e) => e.fields.as_ref(),
                _ => None,
            },
            _ => None,
        }
    }

    fn env(&self, module: ModId) -> Env {
        Env {
            vars: Vec::new(),
            module,
            types: Rc::new(HashMap::new()),
        }
    }

    fn source_text(&self, module: ModId, span: crate::diag::Span) -> String {
        let source = &self.program.modules[module].analysis.source;
        source
            .get(span.start.offset..span.end.offset)
            .unwrap_or("")
            .to_string()
    }

    // ---------------------------------------------------------------- calls

    fn method(&self, key: &str, name: &str) -> Option<FnId> {
        self.methods.get(key)?.get(name).copied()
    }

    /// External name of a function without a body (unio-runtime README, binding table).
    fn external_name(program: &Program, f: FnId) -> String {
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

    /// Calls a function with its arguments in parameter order (`self` first); `var`
    /// parameters are written back into `args`.
    pub fn call_fn(&self, rt: &mut Runtime, f: FnId, args: &mut [Value], types: Types) -> Outcome {
        let def = &self.program.funcs[f];
        let Some((sig, body)) = self.signature(f) else {
            return Err(internal(rt, format!("no declaration for {}", def.name)));
        };
        let Some(body) = body else {
            let name = self.external_names[f].as_str();
            // Defaults of externals are evaluated here, in the declaring module.
            let mut env = self.env(def.module);
            let offset = usize::from(def.self_param.is_some());
            for (i, param) in sig
                .params
                .iter()
                .filter(|p| !matches!(p, Param::SelfParam { .. }))
                .enumerate()
            {
                let default = match param {
                    Param::Named { default, .. } | Param::Plain { default, .. } => default.as_ref(),
                    Param::SelfParam { .. } => None,
                };
                if let (Some(slot), Some(e)) = (args.get(offset + i), default) {
                    if matches!(slot, Value::Unit) {
                        let v = self.eval(rt, &mut env, e)?;
                        args[offset + i] = v;
                    }
                }
            }
            return match name {
                "std.json.to_value" => {
                    let v = args.first().cloned().unwrap_or(Value::Unit);
                    self.encode_json(rt, &v)
                }
                "std.json.from_value" => {
                    let ty = types.values().next().cloned().unwrap_or(RtTy::Unknown);
                    let v = args.first().cloned().unwrap_or(Value::Unit);
                    self.decode_json(rt, &ty, &v, "")
                }
                _ => unio_runtime::externs::call(rt, name, args),
            };
        };
        let module = def.module;
        let line = def.span.start.line as u32;
        rt.enter(
            Rc::clone(&self.labels[f]),
            Rc::clone(&self.files[module]),
            line,
        )?;
        let mut env = Env {
            vars: Vec::new(),
            module,
            types: types.clone(),
        };
        let mut var_slots: Vec<(usize, usize)> = Vec::new();
        for (index, param) in sig.params.iter().enumerate() {
            let (name, var, default) = match param {
                Param::SelfParam { var, .. } => ("self", *var, None),
                Param::Named { name, default, .. } => (name.text.as_str(), false, default.as_ref()),
                Param::Plain {
                    var, name, default, ..
                } => (name.text.as_str(), *var, default.as_ref()),
            };
            let provided = args.get_mut(index).map(take);
            let value = match provided {
                Some(Value::Unit) | None => match default {
                    Some(expr) => match self.eval(rt, &mut env, expr) {
                        Ok(v) => v,
                        Err(e) => {
                            rt.leave();
                            return Err(e);
                        }
                    },
                    None => Value::Unit,
                },
                Some(v) => v,
            };
            if var {
                var_slots.push((index, env.vars.len()));
            }
            env.bind(name, value);
        }
        let result = self.exec_block(rt, &mut env, body);
        for (arg, slot) in var_slots {
            if arg < args.len() {
                args[arg] = take(&mut env.vars[slot].1);
            }
        }
        rt.leave();
        match result? {
            Flow::Return(v) => Ok(v),
            _ => Ok(Value::Unit),
        }
    }

    fn generic_types(&self, env: &Env, f: FnId, types: &[Type]) -> Result<Types, Unwind> {
        let mut map = HashMap::new();
        for (p, t) in self.program.funcs[f].generics.iter().zip(types) {
            map.insert(self.program.params[*p].name.clone(), self.rt_type(env, t)?);
        }
        Ok(Rc::new(map))
    }

    /// Binds the arguments of a call to the parameters of `f`: positional first, then named
    /// by name. Missing ones are `Unit`, which the callee replaces by the default.
    fn bind_args(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        f: FnId,
        receiver: Option<Value>,
        arguments: &[Argument],
    ) -> Result<BoundArgs, Unwind> {
        let def = &self.program.funcs[f];
        let offset = usize::from(def.self_param.is_some());
        let count = offset + def.params.len();
        let mut args = vec![Value::Unit; count];
        let mut places = Vec::new();
        if let Some(r) = receiver {
            if !args.is_empty() {
                args[0] = r;
            }
        }
        let mut position = offset;
        for argument in arguments {
            let target = match &argument.name {
                Some(n) => match def.params.iter().position(|p| p.name == n.text) {
                    Some(i) => i + offset,
                    None => return Err(internal(rt, format!("no parameter {}", n.text))),
                },
                None => {
                    let p = position;
                    position += 1;
                    p
                }
            };
            if target >= count {
                return Err(internal(rt, "too many arguments"));
            }
            let var = def.params[target - offset].var;
            if var && is_place(&argument.value) {
                args[target] = self.take_place(rt, env, &argument.value)?;
                places.push((target, argument.value.clone()));
            } else {
                args[target] = self.eval(rt, env, &argument.value)?;
            }
        }
        Ok((args, places))
    }

    fn call_bound(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        f: FnId,
        receiver: Option<Value>,
        arguments: &[Argument],
        types: Types,
    ) -> Outcome {
        let (mut args, places) = self.bind_args(rt, env, f, receiver, arguments)?;
        let result = self.call_fn(rt, f, &mut args, types);
        for (i, place) in places {
            self.put_place(rt, env, &place, take(&mut args[i]))?;
        }
        result
    }

    /// Calls a method on a value that is not a place: the receiver is not written back.
    fn call_method_value(
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
    fn call_method_owned(
        &self,
        rt: &mut Runtime,
        receiver: Value,
        name: &str,
        rest: Vec<Value>,
    ) -> Result<(Outcome, Value), Unwind> {
        if let Value::Resource(cell) = &receiver {
            let key = value_key(&cell.value.borrow());
            let var_self = self
                .method(&key, name)
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
        let key = value_key(&receiver);
        match self.method(&key, name) {
            Some(f) => {
                let mut args = Vec::with_capacity(rest.len() + 1);
                args.push(receiver);
                args.extend(rest);
                let total = 1 + self.program.funcs[f].params.len();
                while args.len() < total {
                    args.push(Value::Unit);
                }
                let result = self.call_fn(rt, f, &mut args, Rc::new(HashMap::new()));
                Ok((result, take(&mut args[0])))
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
                    _ => Err(internal(rt, format!("no method {name} for {key}"))),
                };
                Ok((result, receiver))
            }
        }
    }

    fn call_value(&self, rt: &mut Runtime, function: &Value, mut args: Vec<Value>) -> Outcome {
        match function {
            Value::Function(f) => f.call(rt, &mut args),
            other => Err(internal(rt, format!("{} is not a function", other.kind()))),
        }
    }

    fn error_code(&self, type_name: &str) -> String {
        for def in &self.program.defs {
            if def.name == type_name && matches!(def.kind, DefKind::Error(_)) {
                let module = &self.program.modules[def.module].name;
                return format!("{module}.{type_name}");
            }
        }
        type_name.to_string()
    }

    // ---------------------------------------------------------------- places

    fn place(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        expr: &Expr,
    ) -> Result<(usize, Vec<Step>), Unwind> {
        match &expr.kind {
            ExprKind::Paren(e) => self.place(rt, env, e),
            ExprKind::Name(n) => env
                .find(n)
                .map(|i| (i, Vec::new()))
                .ok_or_else(|| internal(rt, format!("{n} is not a local name"))),
            ExprKind::SelfValue => env
                .find("self")
                .map(|i| (i, Vec::new()))
                .ok_or_else(|| internal(rt, "self outside a method")),
            ExprKind::Field { target, name } => {
                let (root, mut steps) = self.place(rt, env, target)?;
                steps.push(Step::Field(Rc::from(name.text.as_str())));
                Ok((root, steps))
            }
            ExprKind::Index { target, index } => {
                let (root, mut steps) = self.place(rt, env, target)?;
                let i = self.eval(rt, env, index)?;
                steps.push(Step::Index(i));
                Ok((root, steps))
            }
            _ => Err(internal(rt, "not a place")),
        }
    }

    /// Applies `f` to the value at a place, inside resources and with copy-on-write.
    fn with_place<R>(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        expr: &Expr,
        f: impl FnOnce(&Interp, &mut Runtime, &mut Value) -> Result<R, Unwind>,
    ) -> Result<R, Unwind> {
        let (root, steps) = self.place(rt, env, expr)?;
        self.with_resolved(rt, env, root, &steps, f)
    }

    /// Runs `f` on the place `root` + `steps`, whose indices are already evaluated.
    fn with_resolved<R>(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        root: usize,
        steps: &[Step],
        f: impl FnOnce(&Interp, &mut Runtime, &mut Value) -> Result<R, Unwind>,
    ) -> Result<R, Unwind> {
        let mut value = take(&mut env.vars[root].1);
        let result = if let Value::Resource(cell) = &value {
            let cell = Rc::clone(cell);
            let mut inner = take(&mut cell.value.borrow_mut());
            let r = navigate(rt, &mut inner, steps).and_then(|target| f(self, rt, target));
            *cell.value.borrow_mut() = inner;
            r
        } else {
            navigate(rt, &mut value, steps).and_then(|target| f(self, rt, target))
        };
        env.vars[root].1 = value;
        result
    }

    fn take_place(&self, rt: &mut Runtime, env: &mut Env, expr: &Expr) -> Outcome {
        self.with_place(rt, env, expr, |_, _, v| Ok(take(v)))
    }

    fn put_place(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        expr: &Expr,
        value: Value,
    ) -> Result<(), Unwind> {
        self.assign(rt, env, expr, |_, _, _| Ok(value))
    }

    /// Assigns to a place: the place and its indices are resolved first, then `value` is
    /// evaluated (runtime 1.1), then the value is stored.
    fn assign(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        expr: &Expr,
        value: impl FnOnce(&Interp, &mut Runtime, &mut Env) -> Outcome,
    ) -> Result<(), Unwind> {
        // Map element assignment: m[k] = v.
        if let ExprKind::Index { target, index } = &expr.kind {
            let (root, steps) = self.place(rt, env, target)?;
            let key = self.eval(rt, env, index)?;
            let mut value = Some(value(self, rt, env)?);
            return self.with_resolved(rt, env, root, &steps, |_, rt, container| match container {
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
                            format!("index {i} is outside 0..{}", list.len()),
                        )),
                    }
                }
                _ => Err(internal(
                    rt,
                    "indexed assignment to a value that is not a list or map",
                )),
            });
        }
        let (root, steps) = self.place(rt, env, expr)?;
        let value = value(self, rt, env)?;
        self.with_resolved(rt, env, root, &steps, |_, _, slot| {
            *slot = value;
            Ok(())
        })
    }

    // ---------------------------------------------------------------- statements

    fn exec_block(&self, rt: &mut Runtime, env: &mut Env, block: &Block) -> Exec {
        let mark = env.vars.len();
        let mut flow = Ok(Flow::Normal);
        for statement in block {
            rt.at_line(statement.span.start.line as u32);
            flow = self.exec(rt, env, statement);
            match &flow {
                Ok(Flow::Normal) => {}
                _ => break,
            }
        }
        env.vars.truncate(mark);
        flow
    }

    fn exec(&self, rt: &mut Runtime, env: &mut Env, statement: &Statement) -> Exec {
        match &statement.kind {
            StatementKind::Simple {
                statement: simple,
                handler,
            } => {
                let result = self.exec_simple(rt, env, simple, statement.span);
                match (result, handler) {
                    (Err(Unwind::Error(error)), Some(block)) => {
                        let (ty, name) =
                            find_handler(simple).ok_or_else(|| internal(rt, "inline handler"))?;
                        if self.handles(env, ty, &error) {
                            let mark = env.vars.len();
                            if let Some(n) = name {
                                env.bind(&n.text, error);
                            }
                            let flow = self.exec_block(rt, env, block);
                            env.vars.truncate(mark);
                            flow
                        } else {
                            Err(Unwind::Error(error))
                        }
                    }
                    (result, _) => result,
                }
            }
            StatementKind::If {
                branches,
                otherwise,
            } => {
                for (condition, body) in branches {
                    let mark = env.vars.len();
                    if self.condition(rt, env, condition)? {
                        let flow = self.exec_block(rt, env, body);
                        env.vars.truncate(mark);
                        return flow;
                    }
                    env.vars.truncate(mark);
                }
                match otherwise {
                    Some(body) => self.exec_block(rt, env, body),
                    None => Ok(Flow::Normal),
                }
            }
            StatementKind::While { condition, body } => loop {
                let mark = env.vars.len();
                if !self.condition(rt, env, condition)? {
                    env.vars.truncate(mark);
                    return Ok(Flow::Normal);
                }
                let flow = self.exec_block(rt, env, body)?;
                env.vars.truncate(mark);
                match flow {
                    Flow::Break => return Ok(Flow::Normal),
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                    _ => {}
                }
            },
            StatementKind::For {
                names,
                iterable,
                body,
            } => {
                let source = self.eval(rt, env, iterable)?;
                let mut cursor = self.cursor(rt, source)?;
                loop {
                    let Some(item) = self.next(rt, &mut cursor)? else {
                        return Ok(Flow::Normal);
                    };
                    let mark = env.vars.len();
                    bind_names(rt, env, names, item)?;
                    let flow = self.exec_block(rt, env, body);
                    env.vars.truncate(mark);
                    match flow? {
                        Flow::Break => return Ok(Flow::Normal),
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            StatementKind::Match { subject, cases } => {
                let value = self.eval(rt, env, subject)?;
                for case in cases {
                    let mark = env.vars.len();
                    if self.matches(rt, env, &case.pattern, &value)?
                        && match &case.guard {
                            Some(g) => self.truth(rt, env, g)?,
                            None => true,
                        }
                    {
                        let flow = self.exec_block(rt, env, &case.body);
                        env.vars.truncate(mark);
                        return flow;
                    }
                    env.vars.truncate(mark);
                }
                Err(rt.fault(FaultKind::InvalidArgument, "internal: no case matched"))
            }
            StatementKind::Try { body, handlers } => match self.exec_block(rt, env, body) {
                Err(Unwind::Error(error)) => {
                    for handler in handlers {
                        if self.handles(env, &handler.ty, &error) {
                            let mark = env.vars.len();
                            if let Some(n) = &handler.name {
                                env.bind(&n.text, error);
                            }
                            let flow = self.exec_block(rt, env, &handler.body);
                            env.vars.truncate(mark);
                            return flow;
                        }
                    }
                    Err(Unwind::Error(error))
                }
                other => other,
            },
            StatementKind::Using { name, value, body } => {
                let v = self.eval(rt, env, value)?;
                let resource = match v {
                    Value::Resource(_) => v,
                    other => rt.new_resource(other),
                };
                let mark = env.vars.len();
                env.bind(&name.text, resource.clone());
                let flow = self.exec_block(rt, env, body);
                env.vars.truncate(mark);
                if matches!(flow, Err(Unwind::Exhausted(_))) {
                    return flow;
                }
                let released = self.release(rt, &resource);
                match (flow, released) {
                    (Err(e), Err(Unwind::Fault(f))) => {
                        rt.write_error(&f.render());
                        Err(e)
                    }
                    (Ok(_), Err(e)) => Err(e),
                    (flow, _) => flow,
                }
            }
        }
    }

    fn release(&self, rt: &mut Runtime, resource: &Value) -> Result<(), Unwind> {
        if let Value::Resource(cell) = resource {
            if cell.released.replace(true) {
                return Ok(());
            }
        }
        let has_release = self.method(&value_key(resource), "release").is_some();
        if !has_release {
            return Ok(());
        }
        let (result, _) = self.call_method_owned(rt, resource.clone(), "release", Vec::new())?;
        result.map(|_| ())
    }

    fn exec_simple(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        simple: &Simple,
        span: crate::diag::Span,
    ) -> Exec {
        match simple {
            Simple::Let { name, value, .. } => {
                let v = self.eval(rt, env, value)?;
                env.bind(&name.text, v);
                Ok(Flow::Normal)
            }
            Simple::Assign { target, value } => {
                self.assign(rt, env, target, |interp, rt, env| {
                    interp.eval(rt, env, value)
                })?;
                Ok(Flow::Normal)
            }
            Simple::Return(e) => Ok(Flow::Return(match e {
                Some(e) => self.eval(rt, env, e)?,
                None => Value::Unit,
            })),
            Simple::Fail(e) => Err(Unwind::Error(self.eval(rt, env, e)?)),
            Simple::Assert(e) => {
                if self.truth(rt, env, e)? {
                    Ok(Flow::Normal)
                } else {
                    let text = self.source_text(env.module, e.span);
                    let _ = span;
                    Err(rt.fault(
                        FaultKind::AssertionFailed,
                        format!("assertion failed: {text}"),
                    ))
                }
            }
            Simple::Break => Ok(Flow::Break),
            Simple::Continue => Ok(Flow::Continue),
            Simple::Pass => Ok(Flow::Normal),
            Simple::Expr(e) => {
                self.eval(rt, env, e)?;
                Ok(Flow::Normal)
            }
        }
    }

    fn handles(&self, env: &Env, ty: &Type, error: &Value) -> bool {
        let name = ty.path.last().map(|n| n.text.as_str()).unwrap_or("");
        if name == "Error" {
            return true;
        }
        match error {
            Value::Struct(s) => &*s.type_name == name,
            Value::Enum(e) => &*e.type_name == name,
            _ => {
                let _ = env;
                false
            }
        }
    }

    // ---------------------------------------------------------------- iteration

    fn cursor(&self, rt: &mut Runtime, source: Value) -> Result<Cursor, Unwind> {
        Ok(match source {
            Value::List(l) | Value::Tuple(l) => {
                Cursor::Items(l.iter().cloned().collect::<Vec<_>>().into_iter())
            }
            Value::Set(s) => Cursor::Items(s.items().cloned().collect::<Vec<_>>().into_iter()),
            Value::Map(m) => Cursor::Items(
                m.entries()
                    .map(|(k, _)| k.clone())
                    .collect::<Vec<_>>()
                    .into_iter(),
            ),
            Value::Text(t) => Cursor::Items(
                t.as_str()
                    .chars()
                    .map(|c| Value::text(c.to_string()))
                    .collect::<Vec<_>>()
                    .into_iter(),
            ),
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
                let key = value_key(&other);
                if self.method(&key, "next").is_some() {
                    Cursor::Iterator(other)
                } else {
                    let it = self.call_method_value(rt, other, "iterator", Vec::new())?;
                    Cursor::Iterator(it)
                }
            }
        })
    }

    fn next(&self, rt: &mut Runtime, cursor: &mut Cursor) -> Result<Option<Value>, Unwind> {
        match cursor {
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

    fn truth(&self, rt: &mut Runtime, env: &mut Env, expr: &Expr) -> Result<bool, Unwind> {
        match self.eval(rt, env, expr)? {
            Value::Bool(b) => Ok(b),
            other => Err(internal(rt, format!("condition is a {}", other.kind()))),
        }
    }

    /// A condition of `if` or `while`: `is` tests and the left operands of `and` bind their
    /// names in the guarded branch (L-33).
    fn condition(&self, rt: &mut Runtime, env: &mut Env, expr: &Expr) -> Result<bool, Unwind> {
        match &expr.kind {
            ExprKind::Paren(e) => self.condition(rt, env, e),
            ExprKind::Is { value, pattern } => {
                let v = self.eval(rt, env, value)?;
                self.matches(rt, env, pattern, &v)
            }
            ExprKind::Binary {
                op: "and",
                left,
                right,
            } => Ok(self.condition(rt, env, left)? && self.condition(rt, env, right)?),
            _ => self.truth(rt, env, expr),
        }
    }

    pub fn eval(&self, rt: &mut Runtime, env: &mut Env, expr: &Expr) -> Outcome {
        match &expr.kind {
            ExprKind::Int(text) => parse_int(text)
                .map(Value::Int)
                .ok_or_else(|| internal(rt, "integer literal")),
            ExprKind::Float(text) => text
                .replace('_', "")
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| internal(rt, "float literal")),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Text(parts) => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        TextPart::Literal(s) => out.push_str(s),
                        TextPart::Interpolation(e) => {
                            let v = self.eval(rt, env, e)?;
                            out.push_str(&self.display(rt, &v)?);
                        }
                    }
                }
                Ok(Value::text(out))
            }
            ExprKind::SelfValue => env.get("self").cloned().ok_or_else(|| internal(rt, "self")),
            ExprKind::Name(n) => self.name_value(rt, env, n),
            ExprKind::Paren(e) => self.eval(rt, env, e),
            ExprKind::Tuple(items) => Ok(Value::tuple(self.eval_all(rt, env, items)?)),
            ExprKind::List(items) => Ok(Value::list(self.eval_all(rt, env, items)?)),
            ExprKind::Map(entries) => {
                let mut map = OrderedMap::new();
                for (k, v) in entries {
                    let key = self.eval(rt, env, k)?;
                    let value = self.eval(rt, env, v)?;
                    map.put(key, value);
                }
                Ok(Value::map(map))
            }
            ExprKind::Comprehension {
                element,
                names,
                iterable,
                condition,
            } => {
                let source = self.eval(rt, env, iterable)?;
                let mut cursor = self.cursor(rt, source)?;
                let mut out = Vec::new();
                while let Some(item) = self.next(rt, &mut cursor)? {
                    let mark = env.vars.len();
                    bind_names(rt, env, names, item)?;
                    let keep = match condition {
                        Some(c) => self.truth(rt, env, c)?,
                        None => true,
                    };
                    if keep {
                        out.push(self.eval(rt, env, element)?);
                    }
                    env.vars.truncate(mark);
                }
                rt.check_heap()?;
                Ok(Value::list(out))
            }
            ExprKind::Lambda { params, body, .. } => {
                let key = &**body as *const Expr as usize;
                let body = Rc::clone(
                    self.lambdas
                        .borrow_mut()
                        .entry(key)
                        .or_insert_with(|| Rc::new((**body).clone())),
                );
                Ok(Value::Function(Rc::new(Closure {
                    interp: self.rc(),
                    params: params
                        .iter()
                        .map(|p| Rc::from(p.name.text.as_str()))
                        .collect(),
                    body,
                    env: env.clone(),
                })))
            }
            ExprKind::Is { value, pattern } => {
                let v = self.eval(rt, env, value)?;
                let mark = env.vars.len();
                let r = self.matches(rt, env, pattern, &v)?;
                env.vars.truncate(mark);
                Ok(Value::Bool(r))
            }
            ExprKind::Conditional {
                then,
                condition,
                otherwise,
            } => {
                if self.truth(rt, env, condition)? {
                    self.eval(rt, env, then)
                } else {
                    self.eval(rt, env, otherwise)
                }
            }
            ExprKind::Binary { op, left, right } => self.binary(rt, env, op, left, right),
            ExprKind::Unary { op, operand } => {
                let v = self.eval(rt, env, operand)?;
                match (*op, v) {
                    ("not", Value::Bool(b)) => Ok(Value::Bool(!b)),
                    ("-", Value::Int(i)) => rf::int_subtract(0, i)
                        .map(Value::Int)
                        .map_err(|f| traced(rt, f)),
                    ("-", Value::Float(f)) => Ok(Value::Float(-f)),
                    (op, v) => Err(internal(rt, format!("unary {op} on {}", v.kind()))),
                }
            }
            ExprKind::Try { operand, .. } => match self.eval(rt, env, operand)? {
                Value::Enum(e) if &*e.type_name == "Result" && operand_is_value(operand) => {
                    let inner = e
                        .fields
                        .first()
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Unit);
                    if &*e.variant == "ok" {
                        Ok(inner)
                    } else {
                        Err(Unwind::Error(inner))
                    }
                }
                v => Ok(v),
            },
            ExprKind::Call { callee, arguments } => self.call(rt, env, callee, &[], arguments),
            ExprKind::GenericCall {
                callee,
                types,
                arguments,
            } => self.call(rt, env, callee, types, arguments),
            ExprKind::Index { target, index } => {
                let t = self.eval(rt, env, target)?;
                let i = self.eval(rt, env, index)?;
                index_value(rt, &t, &i)
            }
            ExprKind::Field { target, name } => self.field(rt, env, target, &name.text),
            ExprKind::SafeField { target, name } => match self.eval(rt, env, target)? {
                Value::Enum(e) if &*e.type_name == "Option" => {
                    if &*e.variant == "none" {
                        Ok(Value::none())
                    } else {
                        let inner = e
                            .fields
                            .first()
                            .map(|(_, v)| v.clone())
                            .unwrap_or(Value::Unit);
                        Ok(Value::some(field_of(rt, &inner, &name.text)?))
                    }
                }
                other => field_of(rt, &other, &name.text),
            },
            ExprKind::Error => Err(internal(rt, "expression with a syntax error")),
        }
    }

    fn eval_all(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        items: &[Expr],
    ) -> Result<Vec<Value>, Unwind> {
        items.iter().map(|e| self.eval(rt, env, e)).collect()
    }

    fn name_value(&self, rt: &mut Runtime, env: &mut Env, name: &str) -> Outcome {
        if let Some(v) = env.get(name) {
            return Ok(v.clone());
        }
        if let Some(d) = self.local_variant(env.module, name) {
            return Ok(self.make_variant(d, name, Vec::new()));
        }
        match self.program.lookup(env.module, name) {
            Some((_, Symbol::Const(c))) => self.constant(rt, c),
            Some((_, Symbol::Func(f))) => Ok(Value::Function(Rc::new(FnRef {
                interp: self.rc(),
                f,
            }))),
            _ => match self.variant(env.module, name) {
                Some(d) => Ok(self.make_variant(d, name, Vec::new())),
                None => Err(internal(rt, format!("unknown name {name}"))),
            },
        }
    }

    fn constant(&self, rt: &mut Runtime, c: usize) -> Outcome {
        if let Some(v) = self.consts.borrow().get(&c) {
            return Ok(v.clone());
        }
        let def = &self.program.consts[c];
        let expr = match &self.program.modules[def.module].analysis.file.items[def.item] {
            Item::Declaration(d) => match &d.kind {
                DeclarationKind::Constant(k) => &k.value,
                _ => return Err(internal(rt, "constant")),
            },
            _ => return Err(internal(rt, "constant")),
        };
        let mut env = self.env(def.module);
        let value = self.eval(rt, &mut env, expr)?;
        self.consts.borrow_mut().insert(c, value.clone());
        Ok(value)
    }

    /// A variant declared in the module, which hides a prelude name (semantics 1.4).
    fn local_variant(&self, module: ModId, name: &str) -> Option<DefId> {
        self.variants
            .get(name)?
            .iter()
            .find(|d| self.program.defs[**d].module == module)
            .copied()
    }

    fn variant(&self, module: ModId, name: &str) -> Option<DefId> {
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

    fn make_variant(&self, d: DefId, variant: &str, values: Vec<Value>) -> Value {
        let names: Vec<String> = match &self.program.defs[d].kind {
            DefKind::Enum(vs) => vs
                .iter()
                .find(|v| v.name == variant)
                .map(|v| v.fields.iter().map(|(n, _)| n.clone()).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let fields = names
            .iter()
            .map(|n| Rc::from(n.as_str()))
            .zip(values)
            .collect();
        Value::Enum(Rc::new(EnumValue {
            type_name: Rc::from(self.program.defs[d].name.as_str()),
            variant: Rc::from(variant),
            fields,
        }))
    }

    fn field(&self, rt: &mut Runtime, env: &mut Env, target: &Expr, name: &str) -> Outcome {
        if let ExprKind::Name(n) = &target.kind {
            if env.get(n).is_none() {
                if let Some(m) = self.program.module_alias(env.module, n) {
                    return match self.program.lookup(m, name) {
                        Some((_, Symbol::Const(c))) => self.constant(rt, c),
                        Some((_, Symbol::Func(f))) => Ok(Value::Function(Rc::new(FnRef {
                            interp: self.rc(),
                            f,
                        }))),
                        _ => Err(internal(rt, format!("unknown {n}.{name}"))),
                    };
                }
                if let Some((_, Symbol::Def(d))) = self.program.lookup(env.module, n) {
                    return Ok(self.make_variant(d, name, Vec::new()));
                }
            }
        }
        let value = self.eval(rt, env, target)?;
        field_of(rt, &value, name)
    }

    fn call(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        callee: &Expr,
        types: &[Type],
        arguments: &[Argument],
    ) -> Outcome {
        match &callee.kind {
            ExprKind::Name(n)
                if env.get(n).is_none() && self.local_variant(env.module, n).is_some() =>
            {
                let d = self.local_variant(env.module, n).unwrap_or(0);
                self.construct(rt, env, d, Some(n), arguments)
            }
            ExprKind::Name(n) if env.get(n).is_none() => match self.program.lookup(env.module, n) {
                Some((_, Symbol::Func(f))) => {
                    let t = self.generic_types(env, f, types)?;
                    self.call_bound(rt, env, f, None, arguments, t)
                }
                Some((_, Symbol::Def(d))) => self.construct(rt, env, d, None, arguments),
                Some((_, Symbol::Const(c))) => {
                    let function = self.constant(rt, c)?;
                    let args = self.positional(rt, env, arguments)?;
                    self.call_value(rt, &function, args)
                }
                None => match self.variant(env.module, n) {
                    Some(d) => self.construct(rt, env, d, Some(n), arguments),
                    None => Err(internal(rt, format!("unknown function {n}"))),
                },
            },
            ExprKind::Field { target, name } => {
                if let ExprKind::Name(n) = &target.kind {
                    if env.get(n).is_none() {
                        if let Some(m) = self.program.module_alias(env.module, n) {
                            return match self.program.lookup(m, &name.text) {
                                Some((_, Symbol::Func(f))) => {
                                    let t = self.generic_types(env, f, types)?;
                                    self.call_bound(rt, env, f, None, arguments, t)
                                }
                                Some((_, Symbol::Def(d))) => {
                                    self.construct(rt, env, d, None, arguments)
                                }
                                _ => Err(internal(rt, format!("unknown {n}.{}", name.text))),
                            };
                        }
                        if let Some((_, Symbol::Def(d))) = self.program.lookup(env.module, n) {
                            if matches!(self.program.defs[d].kind, DefKind::Enum(_)) {
                                return self.construct(rt, env, d, Some(&name.text), arguments);
                            }
                            let key = self.program.defs[d].name.clone();
                            return match self.method(&key, &name.text) {
                                Some(f) => self.call_bound(
                                    rt,
                                    env,
                                    f,
                                    None,
                                    arguments,
                                    Rc::new(HashMap::new()),
                                ),
                                None => Err(internal(rt, format!("no function {n}.{}", name.text))),
                            };
                        }
                    }
                }
                self.method_call(rt, env, target, &name.text, arguments)
            }
            _ => {
                let function = self.eval(rt, env, callee)?;
                let args = self.positional(rt, env, arguments)?;
                self.call_value(rt, &function, args)
            }
        }
    }

    fn positional(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        arguments: &[Argument],
    ) -> Result<Vec<Value>, Unwind> {
        arguments
            .iter()
            .map(|a| self.eval(rt, env, &a.value))
            .collect()
    }

    fn method_call(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        target: &Expr,
        name: &str,
        arguments: &[Argument],
    ) -> Outcome {
        // A field that holds a function value: `self.steps.write_data(...)` is not a method.
        let receiver = self.eval(rt, env, target)?;
        let key = value_key(&receiver);
        let Some(f) = self.method(&key, name) else {
            if let Ok(Value::Function(_)) = field_of(rt, &receiver, name) {
                let function = field_of(rt, &receiver, name)?;
                let args = self.positional(rt, env, arguments)?;
                return self.call_value(rt, &function, args);
            }
            let args = self.positional(rt, env, arguments)?;
            return self.call_method_value(rt, receiver, name, args);
        };
        let var_self = self.program.funcs[f].self_param == Some(true);
        if var_self && is_place(target) && !matches!(receiver, Value::Resource(_)) {
            drop(receiver);
            let (mut args, places) = self.bind_args(rt, env, f, None, arguments)?;
            let result = self.with_place(rt, env, target, |interp, rt, slot| {
                args[0] = take(slot);
                let r = interp.call_fn(rt, f, &mut args, Rc::new(HashMap::new()));
                *slot = take(&mut args[0]);
                Ok(r)
            })?;
            for (i, place) in places {
                self.put_place(rt, env, &place, take(&mut args[i]))?;
            }
            return result;
        }
        if let Value::Resource(cell) = &receiver {
            let (mut args, places) = self.bind_args(rt, env, f, None, arguments)?;
            if !var_self {
                args[0] = cell.value.borrow().clone();
                let result = self.call_fn(rt, f, &mut args, Rc::new(HashMap::new()));
                for (i, place) in places {
                    self.put_place(rt, env, &place, take(&mut args[i]))?;
                }
                return result;
            }
            args[0] = take(&mut cell.value.borrow_mut());
            let result = self.call_fn(rt, f, &mut args, Rc::new(HashMap::new()));
            *cell.value.borrow_mut() = take(&mut args[0]);
            for (i, place) in places {
                self.put_place(rt, env, &place, take(&mut args[i]))?;
            }
            return result;
        }
        self.call_bound(
            rt,
            env,
            f,
            Some(receiver),
            arguments,
            Rc::new(HashMap::new()),
        )
    }

    fn construct(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        d: DefId,
        variant: Option<&str>,
        arguments: &[Argument],
    ) -> Outcome {
        let def = &self.program.defs[d];
        match (&def.kind, variant) {
            (DefKind::Enum(vs), Some(v)) => {
                let Some(vd) = vs.iter().find(|x| x.name == v) else {
                    return Err(internal(rt, format!("no variant {v}")));
                };
                let mut values = vec![Value::Unit; vd.fields.len()];
                let mut position = 0;
                for a in arguments {
                    let i = match &a.name {
                        Some(n) => vd
                            .fields
                            .iter()
                            .position(|(f, _)| *f == n.text)
                            .unwrap_or(0),
                        None => {
                            position += 1;
                            position - 1
                        }
                    };
                    let value = self.eval(rt, env, &a.value)?;
                    if i < values.len() {
                        values[i] = value;
                    }
                }
                Ok(self.make_variant(d, v, values))
            }
            (DefKind::Struct(fields) | DefKind::Error(fields), _) => {
                let mut values: Vec<Option<Value>> = vec![None; fields.len()];
                let mut position = 0;
                for a in arguments {
                    let i = match &a.name {
                        Some(n) => fields.iter().position(|f| f.name == n.text).unwrap_or(0),
                        None => {
                            position += 1;
                            position - 1
                        }
                    };
                    let value = self.eval(rt, env, &a.value)?;
                    if i < values.len() {
                        values[i] = Some(value);
                    }
                }
                self.finish_struct(rt, d, values)
            }
            _ => Err(internal(rt, format!("{} cannot be constructed", def.name))),
        }
    }

    /// Fills defaults (evaluated freshly in the declaring module) and builds the value.
    fn finish_struct(&self, rt: &mut Runtime, d: DefId, mut values: Vec<Option<Value>>) -> Outcome {
        let def = &self.program.defs[d];
        let fields = match &def.kind {
            DefKind::Struct(f) | DefKind::Error(f) => f,
            _ => return Err(internal(rt, "struct")),
        };
        let ast = self.fields_of(d);
        let mut out = Vec::with_capacity(fields.len());
        for (i, field) in fields.iter().enumerate() {
            let value = match values[i].take() {
                Some(v) => v,
                None => {
                    let default = ast
                        .and_then(|a| a.iter().find(|f| f.name.text == field.name))
                        .and_then(|f| f.default.as_ref());
                    match default {
                        Some(e) => {
                            let mut env = self.env(def.module);
                            self.eval(rt, &mut env, e)?
                        }
                        None => Value::Unit,
                    }
                }
            };
            out.push((Rc::from(field.name.as_str()), value));
        }
        Ok(Value::Struct(Rc::new(StructValue {
            type_name: Rc::from(def.name.as_str()),
            fields: out,
        })))
    }

    // ---------------------------------------------------------------- operators

    fn binary(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        op: &str,
        left: &Expr,
        right: &Expr,
    ) -> Outcome {
        match op {
            "and" => {
                return Ok(Value::Bool(
                    self.truth(rt, env, left)? && self.truth(rt, env, right)?,
                ));
            }
            "or" => {
                let l = self.eval(rt, env, left)?;
                return match l {
                    Value::Bool(true) => Ok(Value::Bool(true)),
                    Value::Bool(false) => self.eval(rt, env, right),
                    Value::Enum(e) if &*e.type_name == "Option" => {
                        if &*e.variant == "none" {
                            self.eval(rt, env, right)
                        } else if yields_option(right) {
                            Ok(Value::Enum(e))
                        } else {
                            Ok(e.fields
                                .first()
                                .map(|(_, v)| v.clone())
                                .unwrap_or(Value::Unit))
                        }
                    }
                    other => Err(internal(rt, format!("or on {}", other.kind()))),
                };
            }
            _ => {}
        }
        let l = self.eval(rt, env, left)?;
        let r = self.eval(rt, env, right)?;
        self.apply(rt, op, l, r)
    }

    fn apply(&self, rt: &mut Runtime, op: &str, l: Value, r: Value) -> Outcome {
        use Value::*;
        let t = |rt: &Runtime, x: Result<i64, Fault>| x.map(Int).map_err(|f| traced(rt, f));
        Ok(match (op, &l, &r) {
            ("+", Int(a), Int(b)) => return t(rt, rf::int_add(*a, *b)),
            ("-", Int(a), Int(b)) => return t(rt, rf::int_subtract(*a, *b)),
            ("*", Int(a), Int(b)) => return t(rt, rf::int_multiply(*a, *b)),
            ("/", Int(a), Int(b)) => return t(rt, rf::int_divide(*a, *b)),
            ("%", Int(a), Int(b)) => return t(rt, rf::int_remainder(*a, *b)),
            ("**", Int(a), Int(b)) => return t(rt, rf::int_power(*a, *b)),
            ("+", Float(a), Float(b)) => Float(a + b),
            ("-", Float(a), Float(b)) => Float(a - b),
            ("*", Float(a), Float(b)) => Float(a * b),
            ("/", Float(a), Float(b)) => Float(a / b),
            ("%", Float(a), Float(b)) => Float(a % b),
            ("**", Float(a), Float(b)) => Float(a.powf(*b)),
            ("+", Text(a), Text(b)) => Value::text(format!("{}{}", a.as_str(), b.as_str())),
            ("==", _, _) => Bool(self.equal(rt, &l, &r)?),
            ("!=", _, _) => Bool(!self.equal(rt, &l, &r)?),
            ("<" | "<=" | ">" | ">=", _, _) => {
                let ord = self.compare(rt, &l, &r)?;
                Bool(match (op, ord) {
                    (_, None) => false,
                    ("<", Some(o)) => o.is_lt(),
                    ("<=", Some(o)) => o.is_le(),
                    (">", Some(o)) => o.is_gt(),
                    (_, Some(o)) => o.is_ge(),
                })
            }
            ("in", _, _) => return self.call_method_value(rt, r, "contains", vec![l]),
            ("not in", _, _) => {
                let v = self.call_method_value(rt, r, "contains", vec![l])?;
                Bool(!matches!(v, Bool(true)))
            }
            _ => {
                let method = match op {
                    "+" => "add",
                    "-" => "subtract",
                    "*" => "multiply",
                    "/" => "divide",
                    _ => return Err(internal(rt, format!("operator {op} on {}", l.kind()))),
                };
                return self.call_method_value(rt, l, method, vec![r]);
            }
        })
    }

    fn equal(&self, rt: &mut Runtime, l: &Value, r: &Value) -> Result<bool, Unwind> {
        if let (Value::Struct(_) | Value::Enum(_), true) = (l, true) {
            let key = value_key(l);
            if let Some(&f) = self.method(&key, "equals").as_ref() {
                if self.program.funcs[f].body.is_some() {
                    let v = self.call_method_value(rt, l.clone(), "equals", vec![r.clone()])?;
                    return Ok(matches!(v, Value::Bool(true)));
                }
            }
        }
        Ok(values_equal(l, r))
    }

    fn compare(
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
            Value::Int(_) | Value::Float(_) => {
                let name = if matches!(v, Value::Int(_)) {
                    "Int.to_text"
                } else {
                    "Float.to_text"
                };
                let mut args = vec![v.clone()];
                match unio_runtime::externs::call(rt, name, &mut args)? {
                    Value::Text(t) => t.as_str().to_string(),
                    _ => String::new(),
                }
            }
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
                    format!("{}({})", s.type_name, parts.join(", "))
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

    // ---------------------------------------------------------------- patterns

    fn matches(
        &self,
        rt: &mut Runtime,
        env: &mut Env,
        pattern: &Pattern,
        v: &Value,
    ) -> Result<bool, Unwind> {
        let v = match v {
            Value::Resource(cell) => cell.value.borrow().clone(),
            other => other.clone(),
        };
        match &pattern.kind {
            PatternKind::Wildcard => Ok(true),
            PatternKind::Literal(lit) => Ok(match (lit, &v) {
                (Literal::Int(t), Value::Int(i)) => parse_int(t) == Some(*i),
                (Literal::Bool(b), Value::Bool(c)) => b == c,
                (Literal::Text(parts), Value::Text(t)) => {
                    let mut s = String::new();
                    for p in parts {
                        if let TextPart::Literal(l) = p {
                            s.push_str(l);
                        }
                    }
                    s == t.as_str()
                }
                _ => false,
            }),
            PatternKind::Name(names) => {
                let last = &names.last().map(|n| n.text.as_str()).unwrap_or("");
                match &v {
                    Value::Enum(e) if self.enum_has_variant(&e.type_name, last) => {
                        return Ok(&*e.variant == *last)
                    }
                    Value::Struct(s) if &*s.type_name == *last => return Ok(true),
                    _ => {}
                }
                if names.len() == 1
                    && !starts_upper(last)
                    && self.variant(env.module, last).is_none()
                {
                    if let Some((_, Symbol::Const(c))) = self.program.lookup(env.module, last) {
                        let k = self.constant(rt, c)?;
                        return Ok(values_equal(&k, &v));
                    }
                    env.bind(last, v);
                    return Ok(true);
                }
                if let Some((_, Symbol::Const(c))) = self.program.lookup(env.module, last) {
                    let k = self.constant(rt, c)?;
                    return Ok(values_equal(&k, &v));
                }
                Ok(false)
            }
            PatternKind::Constructor { name, arguments } => {
                let last = name.last().map(|n| n.text.as_str()).unwrap_or("");
                let fields: Vec<(Rc<str>, Value)> = match &v {
                    Value::Enum(e) if &*e.variant == last => e.fields.clone(),
                    Value::Struct(s) if &*s.type_name == last => s.fields.clone(),
                    _ => return Ok(false),
                };
                let mut position = 0;
                for (n, p) in arguments {
                    let value = match n {
                        Some(n) => fields
                            .iter()
                            .find(|(f, _)| **f == *n.text)
                            .map(|(_, x)| x.clone()),
                        None => {
                            position += 1;
                            fields.get(position - 1).map(|(_, x)| x.clone())
                        }
                    };
                    let Some(value) = value else { return Ok(false) };
                    if !self.matches(rt, env, p, &value)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            PatternKind::List(elements) => {
                let items: Vec<Value> = match &v {
                    Value::List(l) => l.iter().cloned().collect(),
                    _ => return Ok(false),
                };
                let rest = elements
                    .iter()
                    .position(|e| matches!(e, ListElement::Rest(_)));
                let fixed = elements.len() - usize::from(rest.is_some());
                if rest.is_none() && items.len() != fixed || items.len() < fixed {
                    return Ok(false);
                }
                let mut i = 0;
                for element in elements {
                    match element {
                        ListElement::Pattern(p) => {
                            if !self.matches(rt, env, p, &items[i])? {
                                return Ok(false);
                            }
                            i += 1;
                        }
                        ListElement::Rest(n) => {
                            let count = items.len() - fixed;
                            env.bind(&n.text, Value::list(items[i..i + count].to_vec()));
                            i += count;
                        }
                    }
                }
                Ok(true)
            }
            PatternKind::Tuple(ps) => {
                let items: Vec<Value> = match &v {
                    Value::Tuple(l) => l.iter().cloned().collect(),
                    _ => return Ok(false),
                };
                if items.len() != ps.len() {
                    return Ok(false);
                }
                for (p, x) in ps.iter().zip(items) {
                    if !self.matches(rt, env, p, &x)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            PatternKind::Bind { pattern, name } => {
                if self.matches(rt, env, pattern, &v)? {
                    env.bind(&name.text, v);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            PatternKind::Or(ps) => {
                for p in ps {
                    let mark = env.vars.len();
                    if self.matches(rt, env, p, &v)? {
                        return Ok(true);
                    }
                    env.vars.truncate(mark);
                }
                Ok(false)
            }
        }
    }

    fn enum_has_variant(&self, type_name: &str, variant: &str) -> bool {
        self.variants
            .get(variant)
            .map(|ds| ds.iter().any(|d| self.program.defs[*d].name == type_name))
            .unwrap_or(false)
    }

    // ---------------------------------------------------------------- JSON (S-5, S-17)

    fn rt_type(&self, env: &Env, ty: &Type) -> Result<RtTy, Unwind> {
        let name = ty.path.last().map(|n| n.text.as_str()).unwrap_or("");
        let args: Vec<RtTy> = ty
            .arguments
            .iter()
            .map(|a| match a {
                TypeArgument::Type(t) => self.rt_type(env, t),
                _ => Ok(RtTy::Unknown),
            })
            .collect::<Result<_, _>>()?;
        let arg = |i: usize| Box::new(args.get(i).cloned().unwrap_or(RtTy::Unknown));
        if ty.path.len() == 1 {
            if let Some(t) = env.types.get(name) {
                return Ok(t.clone());
            }
            match name {
                "Int" => return Ok(RtTy::Int),
                "Float" => return Ok(RtTy::Float),
                "Bool" => return Ok(RtTy::Bool),
                "Text" => return Ok(RtTy::Text),
                "Bytes" => return Ok(RtTy::Bytes),
                "Path" => return Ok(RtTy::Path),
                "List" => return Ok(RtTy::List(arg(0))),
                "Map" => return Ok(RtTy::Map(arg(0), arg(1))),
                "Set" => return Ok(RtTy::Set(arg(0))),
                "Tuple" => return Ok(RtTy::Tuple(args)),
                _ => {}
            }
        }
        let module = if ty.path.len() > 1 {
            self.program
                .module_alias(env.module, &ty.path[0].text)
                .unwrap_or(env.module)
        } else {
            env.module
        };
        match self.program.lookup(module, name) {
            Some((_, Symbol::Def(d))) => Ok(RtTy::Named(d, args)),
            _ => Ok(RtTy::Unknown),
        }
    }

    fn ty_to_rt(&self, ty: &Ty, subst: &HashMap<usize, RtTy>) -> RtTy {
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

    fn json(&self, variant: &str, field: Option<(&str, Value)>) -> Value {
        Value::Enum(Rc::new(EnumValue {
            type_name: Rc::from("Value"),
            variant: Rc::from(variant),
            fields: field
                .map(|(n, v)| vec![(Rc::from(n), v)])
                .unwrap_or_default(),
        }))
    }

    fn encode_json(&self, rt: &mut Runtime, v: &Value) -> Outcome {
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
            other => return Err(internal(rt, format!("{} has no JSON form", other.kind()))),
        })
    }

    fn decode_error(&self, path: &str, message: impl Into<String>) -> Unwind {
        Unwind::Error(Value::record(
            "DecodeError",
            vec![
                ("path", Value::text(path.to_string())),
                ("message", Value::text(message.into())),
            ],
        ))
    }

    fn decode_json(&self, rt: &mut Runtime, ty: &RtTy, v: &Value, path: &str) -> Outcome {
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

enum Cursor {
    Items(std::vec::IntoIter<Value>),
    Range(i64, i64),
    Iterator(Value),
}

/// A named function used as a value.
struct FnRef {
    interp: Rc<Interp>,
    f: FnId,
}

impl Callable for FnRef {
    fn call(&self, runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
        let total = self.interp.program.funcs[self.f].params.len()
            + usize::from(self.interp.program.funcs[self.f].self_param.is_some());
        let mut args: Vec<Value> = arguments.iter_mut().map(take).collect();
        while args.len() < total {
            args.push(Value::Unit);
        }
        let result = self
            .interp
            .call_fn(runtime, self.f, &mut args, Rc::new(HashMap::new()));
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
struct Closure {
    interp: Rc<Interp>,
    params: Vec<Rc<str>>,
    body: Rc<Expr>,
    env: Env,
}

impl Callable for Closure {
    fn call(&self, runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
        let mut env = self.env.clone();
        for (name, value) in self.params.iter().zip(arguments.iter()) {
            env.vars.push((Rc::clone(name), value.clone()));
        }
        self.interp.eval(runtime, &mut env, &self.body)
    }
    fn name(&self) -> &str {
        "function"
    }
}

// ---------------------------------------------------------------- helpers

fn ty_key(program: &Program, ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Prim(p) => p.name().to_string(),
        Ty::List(_) => "List".into(),
        Ty::Map(..) => "Map".into(),
        Ty::Set(_) => "Set".into(),
        Ty::Tuple(_) => "Tuple".into(),
        Ty::Named(d, _) => program.defs[*d].name.clone(),
        _ => return None,
    })
}

fn value_key(v: &Value) -> String {
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
        Value::Struct(s) => s.type_name.to_string(),
        Value::Enum(e) => e.type_name.to_string(),
        Value::Resource(cell) => value_key(&cell.value.borrow()),
        Value::Function(_) => "Function".into(),
        Value::Unit => "Unit".into(),
    }
}

fn is_place(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Name(_) | ExprKind::SelfValue => true,
        ExprKind::Field { target, .. } | ExprKind::Index { target, .. } => is_place(target),
        ExprKind::Paren(e) => is_place(e),
        _ => false,
    }
}

/// Whether `try` applies to a stored `Result` value rather than to a call that fails.
fn operand_is_value(e: &Expr) -> bool {
    !matches!(
        &e.kind,
        ExprKind::Call { .. } | ExprKind::GenericCall { .. }
    )
}

/// Whether the right operand of `or` is itself an `Option` (`Option or Option`).
fn yields_option(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Name(n) => n == "none",
        ExprKind::Call { callee, .. } => matches!(&callee.kind, ExprKind::Name(n) if n == "some"),
        ExprKind::Paren(e) => yields_option(e),
        _ => false,
    }
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
}

fn parse_int(text: &str) -> Option<i64> {
    let t = text.replace('_', "");
    if let Some(h) = t.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()
    } else if let Some(b) = t.strip_prefix("0b") {
        i64::from_str_radix(b, 2).ok()
    } else {
        t.parse().ok()
    }
}

fn pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn find_handler(simple: &Simple) -> Option<(&Type, &Option<Name>)> {
    fn search(e: &Expr) -> Option<(&Type, &Option<Name>)> {
        match &e.kind {
            ExprKind::Try {
                handler: Some((t, n)),
                ..
            } => Some((t, n)),
            ExprKind::Try { operand, .. } | ExprKind::Paren(operand) => search(operand),
            _ => None,
        }
    }
    match simple {
        Simple::Let { value, .. } | Simple::Assign { value, .. } => search(value),
        Simple::Return(Some(e)) | Simple::Expr(e) => search(e),
        _ => None,
    }
}

fn bind_names(rt: &Runtime, env: &mut Env, names: &[Name], item: Value) -> Result<(), Unwind> {
    if names.len() == 1 {
        env.bind(&names[0].text, item);
        return Ok(());
    }
    match item {
        Value::Tuple(t) => {
            for (n, v) in names.iter().zip(t.iter()) {
                env.bind(&n.text, v.clone());
            }
            Ok(())
        }
        other => Err(internal(
            rt,
            format!("cannot destructure a {}", other.kind()),
        )),
    }
}

fn navigate<'a>(rt: &Runtime, v: &'a mut Value, steps: &[Step]) -> Result<&'a mut Value, Unwind> {
    let Some((first, rest)) = steps.split_first() else {
        return Ok(v);
    };
    let next: &'a mut Value = match (v, first) {
        (Value::Struct(s), Step::Field(n)) => {
            let s = Rc::make_mut(s);
            match s.fields.iter_mut().find(|(k, _)| **k == **n) {
                Some((_, x)) => x,
                None => return Err(internal(rt, format!("no field {n}"))),
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
                        format!("index {i} is outside 0..{len}"),
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

fn field_of(rt: &Runtime, v: &Value, name: &str) -> Outcome {
    match v {
        Value::Struct(s) => s
            .fields
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, x)| x.clone())
            .ok_or_else(|| internal(rt, format!("no field {name} in {}", s.type_name))),
        Value::Enum(e) => e
            .fields
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, x)| x.clone())
            .ok_or_else(|| internal(rt, format!("no field {name} in {}", e.variant))),
        Value::Resource(cell) => field_of(rt, &cell.value.borrow(), name),
        other => Err(internal(
            rt,
            format!("no field {name} in a {}", other.kind()),
        )),
    }
}

fn index_value(rt: &Runtime, t: &Value, i: &Value) -> Outcome {
    match (t, i) {
        (Value::List(l), Value::Int(i)) => usize::try_from(*i)
            .ok()
            .and_then(|i| l.get(i))
            .cloned()
            .ok_or_else(|| {
                rt.fault(
                    FaultKind::IndexOutOfRange,
                    format!("index {i} is outside 0..{}", l.len()),
                )
            }),
        (Value::Map(m), key) => m
            .get(key)
            .cloned()
            .ok_or_else(|| rt.fault(FaultKind::MissingKey, "the map has no such key")),
        (other, _) => Err(internal(rt, format!("cannot index a {}", other.kind()))),
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
    match interp.call_fn(rt, main, &mut args, Rc::new(HashMap::new())) {
        Ok(v) => match rf::main_exit_code(&v) {
            Ok(code) => code,
            Err(fault) => {
                rt.write_error(&fault.render());
                rf::EXIT_FAULT
            }
        },
        Err(e) => end(rt, e),
    }
}

fn end(rt: &mut Runtime, e: Unwind) -> i32 {
    match e {
        Unwind::Fault(f) => {
            rt.write_error(&f.render());
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
    let mut env = interp.env(module);
    let result = rt
        .enter(
            label.as_str(),
            Rc::clone(&interp.files[module]),
            test.span.start.line as u32,
        )
        .and_then(|_| interp.exec_block(&mut rt, &mut env, &test.body));
    let millis = started.elapsed().as_millis();
    let (passed, message) = match result {
        Ok(_) => (true, String::new()),
        Err(Unwind::Fault(f)) => (false, f.render()),
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
