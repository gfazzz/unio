//! The runtime side of native code (docs/runtime.md 8.4): the context a native function
//! receives, the functions native code calls for operations on values that own memory, the
//! hooks through which the interpreter's dispatch (methods, function values, constants,
//! defaults) enters native code, and the start of an executable made by `unio build`.
//!
//! Every function here has the C calling convention and a stable name, so that the object
//! file of the build refers to it by name and the system linker resolves it in `libunio.a`.
//! Values are passed by address. A function that may fail returns a status: 0 when it
//! succeeded, 1 when evaluation unwinds, with the error, fault, or exhaustion kept in the
//! context until the native caller that handles it (`try`, `on`, `using`) or the entry of the
//! native code returns it to the interpreter. An argument documented as "taken" is moved
//! out of its address whatever the status; "borrowed" is only read.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::too_many_arguments)]

use crate::interp::{
    bind_names, field_cached, field_of, handles, index_value, internal, navigate, navigate_fields,
    take, traced, Cursor, FnRef, Interp, Step, Types,
};
use crate::lower::{ExprCode, LambdaCode, Node, Op, PStep, Pat, Place, S};
use std::rc::Rc;
use unio_runtime::fault::{self as rf, FaultKind, Outcome, Unwind};
use unio_runtime::runtime::Runtime;
use unio_runtime::value::{Callable, OrderedMap, Value};

/// What native code receives in its first argument. The first three fields are read by
/// native code at fixed offsets to push its call frame without a call (C-14, C-18). Native code
/// keeps the depth in a register and writes it, with the line of its frame, before each call of
/// a function here that takes the context; the entries from the runtime restore it on return.
#[repr(C)]
pub struct Ctx {
    pub frames: *mut (u32, u32),
    pub depth: *mut usize,
    pub limit: usize,
    pub rt: *mut Runtime,
    pub interp: *const Interp,
    pub pending: Option<Unwind>,
}

/// The entry of a unit: `(context, frame, result, type arguments) -> status`. The frame holds
/// the arguments in its first slots, in parameter order (`self` first).
pub type Entry = unsafe extern "C" fn(*mut Ctx, *mut Value, *mut Value, *const ()) -> i32;

/// The native code of a program, installed into the interpreter before `main` runs.
pub(crate) struct Hooks {
    /// The entry of every function with a body.
    pub funcs: Vec<Option<Entry>>,
    /// The entry of every lowered expression unit (constants and defaults), by the address of
    /// its lowered code.
    pub exprs: std::collections::HashMap<usize, Entry>,
}

impl Hooks {
    /// Calls the native code of `f` with the arguments in the first slots of `frame`.
    pub fn invoke(
        &self,
        interp: &Interp,
        rt: &mut Runtime,
        f: usize,
        frame: &mut Vec<Value>,
        types: &Types,
    ) -> Outcome {
        let Some(entry) = self.funcs[f] else {
            return Err(internal(
                rt,
                format!("{} has no native code", interp.program.funcs[f].name),
            ));
        };
        let def = &interp.program.funcs[f];
        let count = def.params.len() + usize::from(def.self_param.is_some());
        while frame.len() < count {
            frame.push(Value::Unit);
        }
        enter(interp, rt, entry, frame.as_mut_ptr(), types)
    }

    /// Evaluates a constant or a default by its native code, in a frame of its own.
    pub fn eval_expr(&self, interp: &Interp, rt: &mut Runtime, code: &ExprCode) -> Outcome {
        let Some(entry) = self.exprs.get(&(code as *const ExprCode as usize)) else {
            return Err(internal(rt, "an expression without native code"));
        };
        let mut vars = vec![Value::Unit; code.slots];
        enter(interp, rt, *entry, vars.as_mut_ptr(), &interp.empty_types)
    }
}

/// Runs native code from the interpreter: a new context on this stack, the result as an
/// outcome.
fn enter(
    interp: &Interp,
    rt: &mut Runtime,
    entry: Entry,
    frame: *mut Value,
    types: &Types,
) -> Outcome {
    let (frames, depth, limit) = rt.native_stack();
    let mut ctx = Ctx {
        frames,
        depth,
        limit,
        rt,
        interp,
        pending: None,
    };
    let mut out = std::mem::MaybeUninit::<Value>::uninit();
    let types = if types.is_empty() {
        std::ptr::null()
    } else {
        Rc::as_ptr(types) as *const ()
    };
    let status = unsafe { entry(&mut ctx, frame, out.as_mut_ptr(), types) };
    if status == 0 {
        Ok(unsafe { out.assume_init() })
    } else {
        Err(ctx.pending.take().unwrap_or_else(|| {
            internal(unsafe { &*ctx.rt }, "native code unwound without a reason")
        }))
    }
}

/// A lambda whose body is native code, with the values it captured (L-11a).
struct NativeClosure {
    interp: Rc<Interp>,
    code: Rc<LambdaCode>,
    entry: Entry,
    captured: Vec<Value>,
    types: Types,
}

impl Callable for NativeClosure {
    fn call(&self, runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
        let mut vars = vec![Value::Unit; self.code.slots];
        for (slot, value) in vars.iter_mut().zip(arguments.iter().take(self.code.params)) {
            *slot = value.clone();
        }
        for ((_, inner), value) in self.code.captures.iter().zip(&self.captured) {
            vars[*inner as usize] = value.clone();
        }
        enter(
            &self.interp,
            runtime,
            self.entry,
            vars.as_mut_ptr(),
            &self.types,
        )
    }
    fn name(&self) -> &str {
        "function"
    }
}

// ---------------------------------------------------------------- access

struct Cx<'a> {
    interp: &'a Interp,
    rt: &'a mut Runtime,
    ctx: &'a mut Ctx,
}

#[inline]
unsafe fn cx<'a>(ctx: *mut Ctx) -> Cx<'a> {
    let c = &mut *ctx;
    Cx {
        interp: &*c.interp,
        rt: &mut *c.rt,
        ctx: c,
    }
}

impl Cx<'_> {
    /// Keeps the reason of an unwind for the native caller.
    #[inline]
    fn fail(&mut self, e: Unwind) -> i32 {
        self.ctx.pending = Some(e);
        1
    }

    /// Writes the value of an outcome or keeps its reason.
    #[inline]
    unsafe fn put(&mut self, out: *mut Value, result: Outcome) -> i32 {
        match result {
            Ok(v) => {
                std::ptr::write(out, v);
                0
            }
            Err(e) => self.fail(e),
        }
    }

    fn types(&self, types: *const ()) -> Types {
        if types.is_null() {
            Rc::clone(&self.interp.empty_types)
        } else {
            let p = types as *const std::collections::HashMap<String, crate::interp::RtTy>;
            unsafe {
                Rc::increment_strong_count(p);
                Rc::from_raw(p)
            }
        }
    }
}

#[inline]
unsafe fn read(p: *mut Value) -> Value {
    std::ptr::read(p)
}

#[inline]
unsafe fn read_n(p: *mut Value, n: usize) -> Vec<Value> {
    (0..n).map(|i| std::ptr::read(p.add(i))).collect()
}

#[inline]
unsafe fn slots<'a>(frame: *mut Value, n: i64) -> &'a mut [Value] {
    if n <= 0 {
        &mut []
    } else {
        std::slice::from_raw_parts_mut(frame, n as usize)
    }
}

unsafe fn stmt<'a>(s: *const S) -> &'a S {
    &*s
}

unsafe fn node<'a>(n: *const Node) -> &'a Node {
    &*n
}

/// The operators by the number native code passes.
pub(crate) const OPS: [Op; 14] = [
    Op::Add,
    Op::Subtract,
    Op::Multiply,
    Op::Divide,
    Op::Remainder,
    Op::Power,
    Op::Equal,
    Op::NotEqual,
    Op::Less,
    Op::LessEqual,
    Op::Greater,
    Op::GreaterEqual,
    Op::In,
    Op::NotIn,
];

pub(crate) fn op_number(op: Op) -> i32 {
    OPS.iter().position(|o| *o == op).unwrap_or(0) as i32
}

// ---------------------------------------------------------------- values

/// Releases a value (borrowed address, left without a value).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_drop(v: *mut Value) {
    std::ptr::drop_in_place(v);
}

/// Releases `n` values and leaves `Unit` in their places.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_clear(v: *mut Value, n: i64) {
    for i in 0..n.max(0) as usize {
        *v.add(i) = Value::Unit;
    }
}

/// Copies a value that owns memory (`dst` holds no value).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_clone(src: *const Value, dst: *mut Value) {
    std::ptr::write(dst, (*src).clone());
}

#[no_mangle]
pub unsafe extern "C" fn unio_rt_stack_exhausted(ctx: *mut Ctx) -> i32 {
    cx(ctx).fail(Unwind::Exhausted(rf::Exhaustion::Stack))
}

/// An integer operator failed: 0 overflow of `+`, 1 of `-`, 2 of `*`, 3 of `/`, 4 division by
/// zero, 5 remainder by zero.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_int_fault(ctx: *mut Ctx, which: i32) -> i32 {
    let mut c = cx(ctx);
    let fault = match which {
        0 => rf::int_add(i64::MAX, 1),
        1 => rf::int_subtract(i64::MIN, 1),
        2 => rf::int_multiply(i64::MAX, 2),
        3 => rf::int_divide(i64::MIN, -1),
        4 => rf::int_divide(1, 0),
        _ => rf::int_remainder(1, 0),
    }
    .err();
    let e = match fault {
        Some(f) => traced(c.rt, f),
        None => internal(c.rt, "integer fault"),
    };
    c.fail(e)
}

#[no_mangle]
pub unsafe extern "C" fn unio_rt_int_power(ctx: *mut Ctx, a: i64, b: i64, out: *mut i64) -> i32 {
    let mut c = cx(ctx);
    match rf::int_power(a, b) {
        Ok(v) => {
            *out = v;
            0
        }
        Err(f) => {
            let e = traced(c.rt, f);
            c.fail(e)
        }
    }
}

#[no_mangle]
pub extern "C" fn unio_rt_float_rem(a: f64, b: f64) -> f64 {
    a % b
}

#[no_mangle]
pub extern "C" fn unio_rt_float_pow(a: f64, b: f64) -> f64 {
    a.powf(b)
}

/// A value of an unexpected kind where the code needs one: 0 a condition, 1 an `Int`, 2 a
/// `Float`, 3 a `Bool` (borrowed).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_wrong_kind(ctx: *mut Ctx, which: i32, v: *const Value) -> i32 {
    let mut c = cx(ctx);
    let kind = (*v).kind();
    let e = match which {
        0 => internal(c.rt, format!("condition is a {kind}")),
        1 => internal(c.rt, format!("expected an Int, found a {kind}")),
        2 => internal(c.rt, format!("expected a Float, found a {kind}")),
        _ => internal(c.rt, format!("expected a Bool, found a {kind}")),
    };
    c.fail(e)
}

/// `a op b` on values (both taken), as the interpreter applies operators.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_binary(
    ctx: *mut Ctx,
    op: i32,
    a: *mut Value,
    b: *mut Value,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let (l, r) = (read(a), read(b));
    let op = OPS[op as usize];
    let result = c.interp.apply(c.rt, op, l, r);
    c.put(out, result)
}

/// `a op b` for a comparison or `in` on values (borrowed) of the kinds whose comparison no
/// program can define (integers, texts, booleans, a list of texts or integers): 0 false,
/// 1 true, 2 when the operator needs [`unio_rt_binary`].
#[no_mangle]
pub unsafe extern "C" fn unio_rt_compare(op: i32, a: *const Value, b: *const Value) -> i32 {
    let (a, b) = (&*a, &*b);
    let op = OPS[op as usize];
    match op {
        Op::In | Op::NotIn => match (a, b) {
            (Value::Text(_) | Value::Int(_), Value::List(items))
                if items
                    .iter()
                    .all(|x| matches!(x, Value::Text(_) | Value::Int(_))) =>
            {
                let found = items
                    .iter()
                    .any(|x| unio_runtime::value::values_equal(x, a));
                i32::from(found == (op == Op::In))
            }
            _ => 2,
        },
        _ => match crate::interp::compare_simple(op, a, b) {
            Some(r) => i32::from(r),
            None => 2,
        },
    }
}

#[no_mangle]
pub unsafe extern "C" fn unio_rt_not(ctx: *mut Ctx, v: *mut Value, out: *mut Value) -> i32 {
    let mut c = cx(ctx);
    let result = match read(v) {
        Value::Bool(b) => Ok(Value::Bool(!b)),
        v => Err(internal(c.rt, format!("unary not on {}", v.kind()))),
    };
    c.put(out, result)
}

#[no_mangle]
pub unsafe extern "C" fn unio_rt_negate(ctx: *mut Ctx, v: *mut Value, out: *mut Value) -> i32 {
    let mut c = cx(ctx);
    let result = match read(v) {
        Value::Int(i) => rf::int_subtract(0, i)
            .map(Value::Int)
            .map_err(|f| traced(c.rt, f)),
        Value::Float(f) => Ok(Value::Float(-f)),
        v => Err(internal(c.rt, format!("unary - on {}", v.kind()))),
    };
    c.put(out, result)
}

// ---------------------------------------------------------------- calls

/// Calls `f` with `count` arguments in parameter order (taken; `var` parameters are written
/// back into their places, which the caller then reads and releases).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_call(
    ctx: *mut Ctx,
    f: i64,
    args: *mut Value,
    count: i64,
    types: *const (),
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let args = slots(args, count);
    let types = c.types(types);
    let result = c.interp.call_fn(c.rt, f as usize, args, types);
    c.put(out, result)
}

/// A call with explicit type arguments (`std.json.from_value[T]`): the call node gives them,
/// resolved in their module by the lowering and here with the caller's type arguments. The
/// module of the caller is passed but no longer needed.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_call_typed(
    ctx: *mut Ctx,
    n: *const Node,
    _module: i64,
    types: *const (),
    args: *mut Value,
    count: i64,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let Node::Call(call) = node(n) else {
        let e = internal(c.rt, "a typed call");
        return c.fail(e);
    };
    let env_types = c.types(types);
    let types = match c.interp.generic_types_in(&env_types, call.f, &call.types) {
        Ok(t) => t,
        Err(e) => return c.fail(e),
    };
    let args = slots(args, count);
    let result = c.interp.call_fn(c.rt, call.f, args, types);
    c.put(out, result)
}

/// Calls a function value (taken) with `n` arguments (taken).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_call_value(
    ctx: *mut Ctx,
    function: *mut Value,
    args: *mut Value,
    n: i64,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let function = read(function);
    let args = read_n(args, n as usize);
    let result = c.interp.call_value(c.rt, &function, args);
    c.put(out, result)
}

/// Index steps of a place, evaluated by native code in order, with its field steps.
unsafe fn place_steps(place: &Place, values: *mut Value) -> Vec<Step> {
    let mut i = 0;
    place
        .steps
        .iter()
        .map(|s| match s {
            PStep::Field(n) => Step::Field(Rc::clone(n)),
            PStep::Index(_) => {
                let v = read(values.add(i));
                i += 1;
                Step::Index(v)
            }
        })
        .collect()
}

pub(crate) fn index_steps(place: &Place) -> usize {
    place
        .steps
        .iter()
        .filter(|s| matches!(s, PStep::Index(_)))
        .count()
}

/// Runs `f` on the place `root` + `steps` of the frame.
fn with_resolved<R>(
    interp: &Interp,
    rt: &mut Runtime,
    vars: &mut [Value],
    root: usize,
    steps: &[Step],
    f: impl FnOnce(&Interp, &mut Runtime, &mut Value) -> Result<R, Unwind>,
) -> Result<R, Unwind> {
    let mut value = take(&mut vars[root]);
    let result = if let Value::Resource(cell) = &value {
        let cell = Rc::clone(cell);
        let mut inner = take(&mut cell.value.borrow_mut());
        let r = navigate(rt, &mut inner, steps).and_then(|target| f(interp, rt, target));
        *cell.value.borrow_mut() = inner;
        r
    } else {
        navigate(rt, &mut value, steps).and_then(|target| f(interp, rt, target))
    };
    vars[root] = value;
    result
}

/// Writes a value into a place whose steps are fields (a `var` argument going back).
fn put_place_fields(
    interp: &Interp,
    rt: &mut Runtime,
    vars: &mut [Value],
    place: &Place,
    value: Value,
) -> Result<(), Unwind> {
    let root = place.root as usize;
    if place.steps.is_empty() && !matches!(vars[root], Value::Resource(_)) {
        vars[root] = value;
        return Ok(());
    }
    let steps: Vec<Step> = place
        .steps
        .iter()
        .map(|s| match s {
            PStep::Field(n) => Ok(Step::Field(Rc::clone(n))),
            PStep::Index(_) => Err(internal(rt, "an indexed var argument")),
        })
        .collect::<Result<_, _>>()?;
    with_resolved(interp, rt, vars, root, &steps, |_, _, slot| {
        *slot = value;
        Ok(())
    })
}

/// Takes the value of the place of argument `arg` of a call node out of the frame (index
/// steps taken), for a `var` parameter.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_take_arg(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    n: *const Node,
    arg: i64,
    steps: *mut Value,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let Node::Call(call) = node(n) else {
        let e = internal(c.rt, "a var argument");
        return c.fail(e);
    };
    let Some(place) = &call.args[arg as usize].place else {
        let e = internal(c.rt, "a var argument");
        return c.fail(e);
    };
    let vars = slots(frame, nslots);
    let steps = place_steps(place, steps);
    let result = with_resolved(
        c.interp,
        c.rt,
        vars,
        place.root as usize,
        &steps,
        |_, _, v| Ok(take(v)),
    );
    c.put(out, result)
}

/// Puts the value of a `var` argument (taken) back into its place (index steps taken).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_put_arg(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    n: *const Node,
    arg: i64,
    steps: *mut Value,
    value: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let value = read(value);
    let Node::Call(call) = node(n) else {
        let e = internal(c.rt, "a var argument");
        return c.fail(e);
    };
    let Some(place) = &call.args[arg as usize].place else {
        let e = internal(c.rt, "a var argument");
        return c.fail(e);
    };
    let vars = slots(frame, nslots);
    let steps = place_steps(place, steps);
    match with_resolved(
        c.interp,
        c.rt,
        vars,
        place.root as usize,
        &steps,
        |_, _, v| {
            *v = value;
            Ok(())
        },
    ) {
        Ok(()) => 0,
        Err(e) => c.fail(e),
    }
}

/// A method call. The receiver is taken when `owned`, otherwise borrowed from the frame
/// (a local name); the arguments are taken in written order; `steps` holds the index steps
/// of the receiver's place when a method of that name may change `self`.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_method(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    n: *const Node,
    receiver: *mut Value,
    owned: i32,
    args: *mut Value,
    steps: *mut Value,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let Node::Method(m) = node(n) else {
        let e = internal(c.rt, "a method call");
        return c.fail(e);
    };
    let interp = c.interp;
    // The frequent case first: positional arguments, a method that does not change `self`
    // or other places, found from the receiver without copying it. Its arguments go into an
    // array on this stack, and it is called without a frame of the interpreter.
    if m.positional && !matches!(*receiver, Value::Resource(_)) {
        if let Some(f) = interp.cached_method(m, &*receiver) {
            let def = &interp.program.funcs[f];
            let count = 1 + def.params.len();
            if def.self_param != Some(true)
                && !interp.var_params[f]
                && count <= 8
                && m.args.len() <= def.params.len()
            {
                let external = match (def.body.is_none(), interp.externs[f]) {
                    (true, Some(function))
                        if interp.externs_json[f] == 0 && m.args.len() == def.params.len() =>
                    {
                        Some(function)
                    }
                    _ => None,
                };
                let entry = match (&def.body, interp.native.get()) {
                    (Some(_), Some(hooks)) if interp.intrinsics[f].is_none() => hooks.funcs[f],
                    _ => None,
                };
                if external.is_some() || entry.is_some() {
                    let mut frame: [Value; 8] = std::array::from_fn(|_| Value::Unit);
                    frame[0] = if owned != 0 {
                        read(receiver)
                    } else {
                        (*receiver).clone()
                    };
                    for i in 0..m.args.len() {
                        frame[1 + i] = read(args.add(i));
                    }
                    let result = match (external, entry) {
                        (Some(function), _) => function(c.rt, &mut frame[..count]),
                        (None, Some(entry)) => {
                            enter(interp, c.rt, entry, frame.as_mut_ptr(), &interp.empty_types)
                        }
                        _ => unreachable!("an external or native code"),
                    };
                    return c.put(out, result);
                }
            }
        }
    }
    let receiver = if owned != 0 {
        read(receiver)
    } else {
        (*receiver).clone()
    };
    let args = read_n(args, m.args.len());
    let steps = match (&m.place, steps.is_null()) {
        (Some(place), false) => Some(place_steps(place, steps)),
        (Some(place), true) if index_steps(place) == 0 => Some(place_steps(place, steps)),
        _ => None,
    };
    let vars = slots(frame, nslots);
    let result = method_on(interp, c.rt, vars, m, receiver, args, steps);
    c.put(out, result)
}

/// `items.get(i) or right` on a list: the item without building an `Option`, as in the
/// interpreter. Receiver and index as for [`unio_rt_method`] (the index taken); 0 the result
/// is in `out`, 1 the right side is the result, 2 unwind.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_get_or(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    n: *const Node,
    receiver: *mut Value,
    owned: i32,
    index: *mut Value,
    out: *mut Value,
) -> i32 {
    if let (Value::List(items), Value::Int(i)) = (&*receiver, &*index) {
        let item = usize::try_from(*i).ok().and_then(|i| items.get(i)).cloned();
        if owned != 0 {
            std::ptr::drop_in_place(receiver);
        }
        return match item {
            Some(v) => {
                std::ptr::write(out, v);
                0
            }
            None => 1,
        };
    }
    let mut option = std::mem::MaybeUninit::<Value>::uninit();
    if unio_rt_method(
        ctx,
        frame,
        nslots,
        n,
        receiver,
        owned,
        index,
        std::ptr::null_mut(),
        option.as_mut_ptr(),
    ) != 0
    {
        return 2;
    }
    unio_rt_or(ctx, option.as_mut_ptr(), 0, out)
}

/// The address of `target.name` inside a value (borrowed) at a field node, for reading it in
/// place while the expression that reads it runs; null with the fault pending when there is
/// no such field.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_field_ref(
    ctx: *mut Ctx,
    n: *const Node,
    v: *const Value,
) -> *const Value {
    let mut c = cx(ctx);
    let Node::Field(_, name, at) = node(n) else {
        let e = internal(c.rt, "a field");
        c.fail(e);
        return std::ptr::null();
    };
    let v: &Value = match &*v {
        Value::Resource(cell) => &*cell.value.as_ptr(),
        other => other,
    };
    let fields = match v {
        Value::Struct(s) => &s.fields[..],
        Value::Enum(e) => &e.fields[..],
        _ => {
            let e = match field_of(c.rt, v, name) {
                Err(e) => e,
                Ok(_) => internal(c.rt, "a field"),
            };
            c.fail(e);
            return std::ptr::null();
        }
    };
    if let Some((k, x)) = fields.get(at.get() as usize) {
        if Rc::ptr_eq(k, name) {
            return x;
        }
    }
    match fields
        .iter()
        .position(|(k, _)| Rc::ptr_eq(k, name) || **k == **name)
    {
        Some(i) => {
            at.set(i as u32);
            &fields[i].1
        }
        None => {
            let e = match field_of(c.rt, v, name) {
                Err(e) => e,
                Ok(_) => internal(c.rt, "a field"),
            };
            c.fail(e);
            std::ptr::null()
        }
    }
}

/// The address of the field that `place = value` changes, for a place of field steps on a
/// name that does not hold a resource (borrowed): shared levels are copied on the way, as the
/// interpreter does. Null with the fault pending when a field is missing.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_field_place(
    ctx: *mut Ctx,
    root: *mut Value,
    s: *const S,
) -> *mut Value {
    let mut c = cx(ctx);
    let S::Assign(place, _) = stmt(s) else {
        let e = internal(c.rt, "an assignment");
        c.fail(e);
        return std::ptr::null_mut();
    };
    match navigate_fields(c.rt, &mut *root, &place.steps) {
        Ok(target) => target,
        Err(e) => {
            c.fail(e);
            std::ptr::null_mut()
        }
    }
}

fn method_on(
    interp: &Interp,
    rt: &mut Runtime,
    vars: &mut [Value],
    m: &crate::lower::MethodCode,
    receiver: Value,
    args: Vec<Value>,
    steps: Option<Vec<Step>>,
) -> Outcome {
    let Some(f) = interp.cached_method(m, &receiver) else {
        // A field that holds a function value: `self.steps.write_data(...)` is not a method.
        if let Ok(Value::Function(_)) = field_of(rt, &receiver, &m.name) {
            let function = field_of(rt, &receiver, &m.name)?;
            return interp.call_value(rt, &function, args);
        }
        return interp.call_method_value(rt, receiver, &m.name, args);
    };
    let def = &interp.program.funcs[f];
    let var_self = def.self_param == Some(true);
    let count = 1 + def.params.len();
    let mut frame = interp.new_frame(f, count);
    let mut places: Vec<(usize, &Place)> = Vec::new();
    let mut position = 1;
    for (arg, value) in m.args.iter().zip(args) {
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
        if let Some(place) = &arg.place {
            if def.params[target - 1].var {
                places.push((target, place));
            }
        }
        frame[target] = value;
    }
    let empty = || Rc::clone(&interp.empty_types);
    let result = match (&m.place, steps, &receiver) {
        (Some(place), Some(steps), r) if var_self && !matches!(r, Value::Resource(_)) => {
            drop(receiver);
            with_resolved(
                interp,
                rt,
                vars,
                place.root as usize,
                &steps,
                |interp, rt, slot| {
                    frame[0] = take(slot);
                    let r = interp.invoke(rt, f, &mut frame, Rc::clone(&interp.empty_types));
                    *slot = take(&mut frame[0]);
                    Ok(r)
                },
            )?
        }
        (_, _, Value::Resource(cell)) => {
            if var_self {
                frame[0] = take(&mut cell.value.borrow_mut());
                let r = interp.invoke(rt, f, &mut frame, empty());
                *cell.value.borrow_mut() = take(&mut frame[0]);
                r
            } else {
                frame[0] = cell.value.borrow().clone();
                interp.invoke(rt, f, &mut frame, empty())
            }
        }
        _ => {
            frame[0] = receiver;
            interp.invoke(rt, f, &mut frame, empty())
        }
    };
    for (i, place) in places {
        let v = take(&mut frame[i]);
        put_place_fields(interp, rt, vars, place, v)?;
    }
    interp.recycle(frame);
    result
}

// ---------------------------------------------------------------- values of nodes

/// `target.name` of a value (borrowed) at a field node, which caches the position.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_field(
    ctx: *mut Ctx,
    n: *const Node,
    v: *const Value,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let result = match node(n) {
        Node::Field(_, name, at) => field_cached(c.rt, &*v, name, at),
        _ => Err(internal(c.rt, "a field")),
    };
    c.put(out, result)
}

/// `target?.name` (taken).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_safe_field(
    ctx: *mut Ctx,
    n: *const Node,
    v: *mut Value,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let v = read(v);
    let Node::SafeField(_, name) = node(n) else {
        let e = internal(c.rt, "a field");
        return c.fail(e);
    };
    let result = match v {
        Value::Enum(e) if &*e.type_name == "Option" => {
            if &*e.variant == "none" {
                Ok(Value::none())
            } else {
                let inner = e
                    .fields
                    .first()
                    .map(|(_, v)| v.clone())
                    .unwrap_or(Value::Unit);
                field_of(c.rt, &inner, name).map(Value::some)
            }
        }
        other => field_of(c.rt, &other, name),
    };
    c.put(out, result)
}

/// `target[index]` (both borrowed).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_index(
    ctx: *mut Ctx,
    t: *const Value,
    i: *const Value,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let result = index_value(c.rt, &*t, &*i);
    c.put(out, result)
}

#[no_mangle]
pub unsafe extern "C" fn unio_rt_constant(ctx: *mut Ctx, k: i64, out: *mut Value) -> i32 {
    let mut c = cx(ctx);
    let result = c.interp.constant(c.rt, k as usize);
    c.put(out, result)
}

#[no_mangle]
pub unsafe extern "C" fn unio_rt_fnref(ctx: *mut Ctx, f: i64, out: *mut Value) {
    let c = cx(ctx);
    std::ptr::write(
        out,
        Value::Function(Rc::new(FnRef {
            interp: c.interp.rc(),
            f: f as usize,
        })),
    );
}

/// A variant without fields written as a name.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_variant(ctx: *mut Ctx, n: *const Node, out: *mut Value) {
    let c = cx(ctx);
    let v = match node(n) {
        Node::Variant(d, name) => c.interp.make_variant(*d, name, Vec::new()),
        _ => Value::Unit,
    };
    std::ptr::write(out, v);
}

/// A struct, error, or variant from the arguments of a construction node (taken, in written
/// order); missing fields get their defaults.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_construct(
    ctx: *mut Ctx,
    n: *const Node,
    args: *mut Value,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let Node::Construct(code) = node(n) else {
        let e = internal(c.rt, "a construction");
        return c.fail(e);
    };
    let args = read_n(args, code.args.len());
    let result = match &code.variant {
        Some(variant) => {
            let mut values = vec![Value::Unit; code.fields];
            for ((i, _), v) in code.args.iter().zip(args) {
                if *i < values.len() {
                    values[*i] = v;
                }
            }
            Ok(c.interp.make_variant(code.def, variant, values))
        }
        None => {
            let mut values: Vec<Option<Value>> = vec![None; code.fields];
            for ((i, _), v) in code.args.iter().zip(args) {
                if *i < values.len() {
                    values[*i] = Some(v);
                }
            }
            c.interp.finish_struct(c.rt, code.def, values)
        }
    };
    c.put(out, result)
}

/// 0 a tuple, 1 a list of `n` values (taken).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_sequence(which: i32, items: *mut Value, n: i64, out: *mut Value) {
    let items = read_n(items, n as usize);
    std::ptr::write(
        out,
        if which == 0 {
            Value::tuple(items)
        } else {
            Value::list(items)
        },
    );
}

/// A map of `n` pairs (taken) in written order.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_map(items: *mut Value, n: i64, out: *mut Value) {
    let items = read_n(items, 2 * n as usize);
    let mut map = OrderedMap::new();
    let mut it = items.into_iter();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        map.put(k, v);
    }
    std::ptr::write(out, Value::map(map));
}

/// An empty text to append the parts of an interpolation to.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_text_new(out: *mut Value) {
    std::ptr::write(out, Value::text(String::new()));
}

/// Appends literal UTF-8 bytes to the text at `acc` (borrowed).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_text_literal(acc: *mut Value, data: *const u8, len: i64) {
    let bytes = std::slice::from_raw_parts(data, len as usize);
    let text = std::str::from_utf8_unchecked(bytes);
    if let Value::Text(t) = take(&mut *acc) {
        *acc = Value::Text(t.append(text));
    }
}

/// Appends the text of a value (borrowed) to the text at `acc` (borrowed), as interpolation
/// shows it.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_text_value(
    ctx: *mut Ctx,
    acc: *mut Value,
    v: *const Value,
) -> i32 {
    let mut c = cx(ctx);
    let piece = match &*v {
        Value::Text(t) => {
            if let Value::Text(a) = take(&mut *acc) {
                *acc = Value::Text(a.append(t.as_str()));
            }
            return 0;
        }
        Value::Int(i) => i.to_string(),
        other => match c.interp.display(c.rt, other) {
            Ok(s) => s,
            Err(e) => return c.fail(e),
        },
    };
    if let Value::Text(a) = take(&mut *acc) {
        *acc = Value::Text(a.append(&piece));
    }
    0
}

/// Appends an `Int` to the text at `acc` (borrowed).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_text_int(acc: *mut Value, i: i64) {
    if let Value::Text(a) = take(&mut *acc) {
        *acc = Value::Text(a.append(&i.to_string()));
    }
}

/// A lambda value with native code `entry` and the captured values (taken).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_closure(
    ctx: *mut Ctx,
    n: *const Node,
    entry: Entry,
    captured: *mut Value,
    types: *const (),
    out: *mut Value,
) {
    let c = cx(ctx);
    let Node::Lambda(code) = node(n) else {
        std::ptr::write(out, Value::Unit);
        return;
    };
    let captured = read_n(captured, code.captures.len());
    let types = c.types(types);
    std::ptr::write(
        out,
        Value::Function(Rc::new(NativeClosure {
            interp: c.interp.rc(),
            code: Rc::clone(code),
            entry,
            captured,
            types,
        })),
    );
}

/// `left or right` once `left` (taken) is known: 0 the result is in `out`, 1 the right side
/// is the result, 2 unwind.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_or(
    ctx: *mut Ctx,
    l: *mut Value,
    yields_option: i32,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    match read(l) {
        Value::Bool(true) => {
            std::ptr::write(out, Value::Bool(true));
            0
        }
        Value::Bool(false) => 1,
        Value::Enum(e) if &*e.type_name == "Option" => {
            if &*e.variant == "none" {
                1
            } else if yields_option != 0 {
                std::ptr::write(out, Value::Enum(e));
                0
            } else {
                let v = e
                    .fields
                    .first()
                    .map(|(_, v)| v.clone())
                    .unwrap_or(Value::Unit);
                std::ptr::write(out, v);
                0
            }
        }
        other => {
            let e = internal(c.rt, format!("or on {}", other.kind()));
            c.fail(e);
            2
        }
    }
}

/// `try` on a value (taken): of `Result` when `is_value`, otherwise of a call that fails.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_try(
    ctx: *mut Ctx,
    v: *mut Value,
    is_value: i32,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    match read(v) {
        Value::Enum(e) if &*e.type_name == "Result" && is_value != 0 => {
            let inner = e
                .fields
                .first()
                .map(|(_, v)| v.clone())
                .unwrap_or(Value::Unit);
            if &*e.variant == "ok" {
                std::ptr::write(out, inner);
                0
            } else {
                c.fail(Unwind::Error(Box::new(inner)))
            }
        }
        v => {
            std::ptr::write(out, v);
            0
        }
    }
}

/// Whether a value (borrowed) matches a pattern, binding its names in the frame: 0 no, 1 yes,
/// 2 unwind.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_match(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    p: *const Pat,
    v: *const Value,
) -> i32 {
    let mut c = cx(ctx);
    let vars = slots(frame, nslots);
    match c.interp.matches(c.rt, vars, &*p, &*v) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(e) => {
            c.fail(e);
            2
        }
    }
}

/// The fault of a node that a checked program cannot reach, or no case of a `match` matched.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_fault_node(ctx: *mut Ctx, n: *const Node) -> i32 {
    let mut c = cx(ctx);
    let message = match n.is_null() {
        true => "internal: no case matched".to_string(),
        false => match node(n) {
            Node::Fault(m) => m.clone(),
            _ => "internal: a node without native code".to_string(),
        },
    };
    let e = c.rt.fault(FaultKind::InvalidArgument, message);
    c.fail(e)
}

// ---------------------------------------------------------------- statements

#[no_mangle]
pub unsafe extern "C" fn unio_rt_assert_failed(ctx: *mut Ctx, s: *const S) -> i32 {
    let mut c = cx(ctx);
    let text = match stmt(s) {
        S::Assert(_, text) => text.as_str(),
        _ => "",
    };
    let e = c.rt.fault(
        FaultKind::AssertionFailed,
        format_args!("assertion failed: {text}"),
    );
    c.fail(e)
}

/// `fail` with an error value (taken).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_fail(ctx: *mut Ctx, v: *mut Value) -> i32 {
    let v = read(v);
    cx(ctx).fail(Unwind::Error(Box::new(v)))
}

/// Whether handler `handler` of a `try` statement, or the inline handler of a statement,
/// receives the pending error; if so the error goes into its name and is no longer pending.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_catch(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    s: *const S,
    handler: i64,
) -> i32 {
    let c = cx(ctx);
    let (ty, bind) = match stmt(s) {
        S::Handled { ty, bind, .. } => (ty, *bind),
        S::Try(_, handlers) => {
            let h = &handlers[handler as usize];
            (&h.ty, h.bind)
        }
        _ => return 0,
    };
    let Some(Unwind::Error(error)) = &c.ctx.pending else {
        return 0;
    };
    if !handles(ty, error) {
        return 0;
    }
    let Some(Unwind::Error(error)) = c.ctx.pending.take() else {
        return 0;
    };
    if let Some(slot) = bind {
        slots(frame, nslots)[slot as usize] = *error;
    }
    1
}

/// Opens a `using` block with a value (taken): the resource goes into the name's place
/// (borrowed, holding a value) and into `resource`.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_using_open(
    ctx: *mut Ctx,
    v: *mut Value,
    slot: *mut Value,
    resource: *mut Value,
) {
    let c = cx(ctx);
    let v = read(v);
    let r = match v {
        Value::Resource(_) => v,
        other => c.rt.new_resource(other),
    };
    *slot = r.clone();
    std::ptr::write(resource, r);
}

/// Releases the resource of a `using` block (taken) when the block ends normally.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_using_release(ctx: *mut Ctx, resource: *mut Value) -> i32 {
    let mut c = cx(ctx);
    let r = read(resource);
    match c.interp.release(c.rt, &r) {
        Ok(()) => 0,
        Err(e) => c.fail(e),
    }
}

/// Releases the resource of a `using` block (taken) while the block unwinds; the reason of the
/// unwind stays pending, and a fault of `release` is reported on standard error (C-17).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_using_unwind(ctx: *mut Ctx, resource: *mut Value) {
    let c = cx(ctx);
    let r = read(resource);
    if matches!(c.ctx.pending, Some(Unwind::Exhausted(_))) {
        return;
    }
    match c.interp.release(c.rt, &r) {
        Err(Unwind::Fault(f)) => c.rt.write_fault(&f),
        // A release that exhausted the reserve or the heap: code 102, and the remaining
        // releases are skipped (C-18).
        Err(exhausted @ Unwind::Exhausted(_)) => c.ctx.pending = Some(exhausted),
        _ => {}
    }
}

/// The size of the cursor of a `for` loop that native code keeps on its stack.
pub(crate) const CURSOR_SIZE: usize = std::mem::size_of::<Cursor>();

/// Starts iterating over a value (taken), writing the cursor to `cursor`.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_cursor_init(
    ctx: *mut Ctx,
    source: *mut Value,
    cursor: *mut Cursor,
) -> i32 {
    let mut c = cx(ctx);
    let source = read(source);
    match c.interp.cursor(c.rt, source) {
        Ok(cur) => {
            std::ptr::write(cursor, cur);
            0
        }
        Err(e) => c.fail(e),
    }
}

/// The next item of a cursor: 0 none, 1 an item in `out`, 2 unwind.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_cursor_next(
    ctx: *mut Ctx,
    cursor: *mut Cursor,
    out: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    match c.interp.next(c.rt, &mut *cursor) {
        Ok(Some(v)) => {
            std::ptr::write(out, v);
            1
        }
        Ok(None) => 0,
        Err(e) => {
            c.fail(e);
            2
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn unio_rt_cursor_drop(cursor: *mut Cursor) {
    std::ptr::drop_in_place(cursor);
}

/// Binds the item (taken) of a `for` statement or a comprehension node to its names.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_bind(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    s: *const S,
    n: *const Node,
    item: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let item = read(item);
    let names = if !s.is_null() {
        match stmt(s) {
            S::For(names, _, _) => &names[..],
            _ => &[],
        }
    } else {
        match node(n) {
            Node::Comprehension(code) => &code.names[..],
            _ => &[],
        }
    };
    match bind_names(c.rt, slots(frame, nslots), names, item) {
        Ok(()) => 0,
        Err(e) => c.fail(e),
    }
}

/// Appends a value (taken) to the list at `acc` (borrowed), for a comprehension.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_list_push(acc: *mut Value, v: *mut Value) {
    let v = read(v);
    if let Value::List(l) = &mut *acc {
        Rc::make_mut(l).push(v);
    }
}

/// Ends a comprehension: the list's size is counted and the heap limit checked.
#[no_mangle]
pub unsafe extern "C" fn unio_rt_list_done(ctx: *mut Ctx, acc: *mut Value) -> i32 {
    let mut c = cx(ctx);
    if let Value::List(l) = &mut *acc {
        Rc::make_mut(l).recount();
    }
    match c.rt.check_heap() {
        Ok(()) => 0,
        Err(e) => c.fail(e),
    }
}

/// `place = value` for a place with steps or a resource (index steps and value taken).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_assign(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    s: *const S,
    steps: *mut Value,
    value: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let S::Assign(place, _) = stmt(s) else {
        let _ = read(value);
        let e = internal(c.rt, "an assignment");
        return c.fail(e);
    };
    let steps = place_steps(place, steps);
    let value = read(value);
    let vars = slots(frame, nslots);
    let root = place.root as usize;
    let r = if place.steps.is_empty() && !matches!(vars[root], Value::Resource(_)) {
        vars[root] = value;
        Ok(())
    } else if place.steps.iter().all(|s| matches!(s, PStep::Field(_)))
        && !matches!(vars[root], Value::Resource(_))
    {
        navigate_fields(c.rt, &mut vars[root], &place.steps).map(|target| *target = value)
    } else {
        with_resolved(c.interp, c.rt, vars, root, &steps, |_, _, slot| {
            *slot = value;
            Ok(())
        })
    };
    match r {
        Ok(()) => 0,
        Err(e) => c.fail(e),
    }
}

/// `place[key] = value` (index steps, key and value taken).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_set_index(
    ctx: *mut Ctx,
    frame: *mut Value,
    nslots: i64,
    s: *const S,
    steps: *mut Value,
    key: *mut Value,
    value: *mut Value,
) -> i32 {
    let mut c = cx(ctx);
    let S::SetIndex(place, _, _) = stmt(s) else {
        let e = internal(c.rt, "an indexed assignment");
        return c.fail(e);
    };
    let steps = place_steps(place, steps);
    let key = read(key);
    let mut value = Some(read(value));
    let vars = slots(frame, nslots);
    let r = with_resolved(
        c.interp,
        c.rt,
        vars,
        place.root as usize,
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
    );
    match r {
        Ok(()) => 0,
        Err(e) => c.fail(e),
    }
}

/// `x = x + a + b` on a text in place: `n` parts (taken) appended to the text at `slot`
/// (borrowed, holding a text).
#[no_mangle]
pub unsafe extern "C" fn unio_rt_concat(
    ctx: *mut Ctx,
    slot: *mut Value,
    parts: *mut Value,
    n: i64,
) -> i32 {
    let mut c = cx(ctx);
    let values = read_n(parts, n as usize);
    let Value::Text(mut text) = take(&mut *slot) else {
        let e = internal(c.rt, "concatenation");
        return c.fail(e);
    };
    for v in values {
        match v {
            Value::Text(t) => text = text.append(t.as_str()),
            other => {
                *slot = Value::Text(text);
                let e = internal(c.rt, format!("text + {}", other.kind()));
                return c.fail(e);
            }
        }
    }
    *slot = Value::Text(text);
    0
}

// ---------------------------------------------------------------- the start of an executable

/// What the object file of a build describes (written by `native::codegen`).
#[repr(C)]
pub struct Image {
    pub magic: u64,
    pub value_size: u64,
    /// The embedded program: see `native::image`.
    pub data: *const u8,
    pub data_len: u64,
    pub units: u64,
    /// For each unit: its entry, its table of references, and the table's length.
    pub entries: *const Entry,
    pub tables: *const *mut usize,
    pub lengths: *const u64,
}

pub const MAGIC: u64 = 0x554e_494f_0001_0004;

/// The stack of the thread that runs the program, as for `unio run`.
const RUN_STACK: usize = 1024 * 1024 * 1024;

#[cfg(unix)]
extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

/// The `main` of an executable made by `unio build`.
#[no_mangle]
pub unsafe extern "C" fn unio_native_main(
    argc: i32,
    argv: *const *const std::ffi::c_char,
    image: *const Image,
) -> i32 {
    // Started as the writer process of a sandbox, this serves and ends here (docs/runtime.md 9.2).
    #[cfg(unix)]
    unio_runtime::sandbox::writer::serve_if_requested();
    // As in a Rust program: writing to a closed pipe is an error, not a signal.
    #[cfg(unix)]
    signal(13, 1);
    let mut args = Vec::new();
    if cfg!(windows) {
        // The `argv` of the C runtime on Windows is in the code page of the system; the
        // arguments are those of the command line in UTF-16, as `unio run` reads them.
        let _ = argv;
        args.extend(
            std::env::args_os()
                .skip(1)
                .map(|a| a.to_string_lossy().into_owned()),
        );
    } else {
        for i in 1..argc.max(0) as usize {
            let s = std::ffi::CStr::from_ptr(*argv.add(i));
            args.push(s.to_string_lossy().into_owned());
        }
    }
    let image = &*image;
    if image.magic != MAGIC || image.value_size as usize != unio_runtime::value::layout::SIZE {
        eprintln!(
            "unio: {}",
            super::image::Refusal::Format(image.magic as u32 & 0xffff)
        );
        return unio_runtime::fault::EXIT_IMAGE;
    }
    let data = std::slice::from_raw_parts(image.data, image.data_len as usize).to_vec();
    let units = image.units as usize;
    let entries: Vec<Entry> = std::slice::from_raw_parts(image.entries, units).to_vec();
    let tables: Vec<usize> = std::slice::from_raw_parts(image.tables, units)
        .iter()
        .map(|p| *p as usize)
        .collect();
    let lengths: Vec<u64> = std::slice::from_raw_parts(image.lengths, units).to_vec();
    let code = std::thread::Builder::new()
        .stack_size(RUN_STACK)
        .spawn(move || start(&data, &entries, &tables, &lengths, args))
        .map(|t| t.join().unwrap_or(101))
        .unwrap_or(101);
    use std::io::Write;
    let _ = std::io::stdout().flush();
    code
}

/// Refuses an image this runtime cannot run, with code 104.
fn refuse(why: super::image::Refusal) -> i32 {
    eprintln!("unio: {why}");
    unio_runtime::fault::EXIT_IMAGE
}

/// The start of an executable (docs/runtime.md 8.4): reads the program image, builds the
/// interpreter's tables from its declarations without the front end, installs the lowered
/// bodies and the native code of every unit, applies the sandbox, and runs `main`.
fn start(
    data: &[u8],
    entries: &[Entry],
    tables: &[usize],
    lengths: &[u64],
    args: Vec<String>,
) -> i32 {
    let opened = match super::image::open(data) {
        Ok(opened) => opened,
        Err(why) => return refuse(why),
    };
    let plan = match crate::sandbox_cli::prepare_embedded(opened.permissions.as_ref()) {
        Ok(plan) => plan,
        Err(code) => return code,
    };
    let module = opened.main;
    let interp = Interp::prepared(opened.program);
    let mut units = match super::image::read_units(opened.rest, &interp) {
        Ok(units) => units,
        Err(why) => return refuse(why),
    };
    super::units::add_lambdas(&mut units);
    if units.len() != entries.len() {
        return refuse(super::image::Refusal::Damaged(
            "it does not match the native code".into(),
        ));
    }
    let mut hooks = Hooks {
        funcs: vec![None; interp.program.funcs.len()],
        exprs: std::collections::HashMap::new(),
    };
    for (i, unit) in units.iter().enumerate() {
        let mut refs = Vec::new();
        let mut lambdas = Vec::new();
        super::units::refs(unit, &mut refs, &mut lambdas);
        if refs.len() as u64 != lengths[i] {
            return refuse(super::image::Refusal::Damaged(
                "it does not match the native code".into(),
            ));
        }
        let table = tables[i] as *mut usize;
        for (j, r) in refs.iter().enumerate() {
            unsafe { *table.add(j) = r.address() };
        }
        match (&unit.key, &unit.body) {
            (super::units::Key::Func(f), super::units::Body::Func(_, code)) => {
                hooks.funcs[*f] = Some(entries[i]);
                let _ = interp.codes[*f].set(Rc::clone(code));
            }
            (key, super::units::Body::Expr(code)) => {
                hooks.exprs.insert(Rc::as_ptr(code) as usize, entries[i]);
                let (cache, at) = match *key {
                    super::units::Key::Const(c) => (&interp.default_codes, (usize::MAX, c)),
                    super::units::Key::Field(d, f) => (&interp.field_codes, (d, f)),
                    super::units::Key::ExternDefault(f, p) => (&interp.default_codes, (f, p)),
                    _ => continue,
                };
                cache.borrow_mut().insert(at, Rc::clone(code));
            }
            _ => {}
        }
    }
    // The lowered code lives in the interpreter's tables, which the tables of references point
    // into; the lambdas live inside it.
    drop(units);
    let _ = interp.native.set(hooks);
    // SB-1: the sandbox applies after the program is loaded and before main runs.
    let _sandbox = crate::sandbox_cli::enter_or_exit(&plan, 0);
    let mut rt = Runtime::new(args);
    rt.site_base(|| interp.sites());
    crate::interp::run_main(&interp, &mut rt, module)
}
