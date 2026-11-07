//! The external functions of the standard library (docs/runtime.md, section 7).
//!
//! Each `external function` declared in `stdlib/` is bound here by its full name:
//!
//! - a method in `extend T:` is `T.name`, for example `Text.slice` or `List.append`;
//! - a function of `prelude.unio` or of a `builtin/` file is its bare name, for example
//!   `assert_fail`, `Path`, or `total_order_key`;
//! - a function of any other module is `std.module.name`, for example `std.files.system_rename`.
//!
//! An external receives every argument, including defaults, which the interpreter evaluates
//! at the call (docs/runtime.md, 1.3). A method receives `self` as the first argument; a `var
//! self` method changes `arguments[0]` in place and the interpreter stores it back into the
//! place it came from. The documentation comment of each declaration is its specification.

pub mod alloc;
pub mod core;
pub mod durability;
pub mod files;
pub mod float_text;
pub mod process;
pub mod system;

use crate::fault::{FaultKind, Outcome, Unwind};
use crate::runtime::Runtime;
use crate::value::{Text, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

/// A binding of an external function.
pub type ExternFn = fn(&mut Runtime, &mut [Value]) -> Outcome;

/// Every binding, by full name.
pub fn bindings() -> BTreeMap<&'static str, ExternFn> {
    let mut table: BTreeMap<&'static str, ExternFn> = BTreeMap::new();
    for (name, function) in core::BINDINGS
        .iter()
        .chain(alloc::BINDINGS)
        .chain(system::BINDINGS)
        .chain(files::BINDINGS)
        .chain(process::BINDINGS)
    {
        let previous = table.insert(name, *function);
        assert!(previous.is_none(), "external {name} is bound twice");
    }
    table
}

/// The binding of an external by its full name, which a caller can keep instead of finding it
/// at every call.
pub fn lookup(name: &str) -> Option<ExternFn> {
    static TABLE: std::sync::OnceLock<BTreeMap<&'static str, ExternFn>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(bindings).get(name).copied()
}

/// Calls an external by its full name.
pub fn call(runtime: &mut Runtime, name: &str, arguments: &mut [Value]) -> Outcome {
    match lookup(name) {
        Some(function) => function(runtime, arguments),
        None => Err(runtime.fault(
            FaultKind::NotProvided,
            format!("the runtime has no external function {name}"),
        )),
    }
}

// ---------------------------------------------------------------- arguments

/// An argument of the wrong kind cannot come from a checked program; it is an internal fault.
fn wrong(runtime: &Runtime, index: usize, expected: &str, found: &Value) -> Unwind {
    runtime.fault(
        FaultKind::InvalidArgument,
        format!(
            "internal: argument {} is a {}, expected {expected}",
            index + 1,
            found.kind()
        ),
    )
}

fn argument<'a>(
    runtime: &Runtime,
    arguments: &'a [Value],
    index: usize,
) -> Result<&'a Value, Unwind> {
    arguments.get(index).ok_or_else(|| {
        runtime.fault(
            FaultKind::InvalidArgument,
            format!("internal: argument {} is missing", index + 1),
        )
    })
}

pub(crate) fn int(runtime: &Runtime, arguments: &[Value], index: usize) -> Result<i64, Unwind> {
    match argument(runtime, arguments, index)? {
        Value::Int(i) => Ok(*i),
        other => Err(wrong(runtime, index, "Int", other)),
    }
}

pub(crate) fn float(runtime: &Runtime, arguments: &[Value], index: usize) -> Result<f64, Unwind> {
    match argument(runtime, arguments, index)? {
        Value::Float(x) => Ok(*x),
        other => Err(wrong(runtime, index, "Float", other)),
    }
}

pub(crate) fn text(runtime: &Runtime, arguments: &[Value], index: usize) -> Result<Text, Unwind> {
    match argument(runtime, arguments, index)? {
        Value::Text(t) => Ok(t.clone()),
        other => Err(wrong(runtime, index, "Text", other)),
    }
}

pub(crate) fn path(runtime: &Runtime, arguments: &[Value], index: usize) -> Result<Text, Unwind> {
    match argument(runtime, arguments, index)? {
        Value::Path(t) => Ok(t.clone()),
        other => Err(wrong(runtime, index, "Path", other)),
    }
}

pub(crate) fn bytes(
    runtime: &Runtime,
    arguments: &[Value],
    index: usize,
) -> Result<Vec<u8>, Unwind> {
    match argument(runtime, arguments, index)? {
        Value::Bytes(b) => Ok(b.to_vec()),
        other => Err(wrong(runtime, index, "Bytes", other)),
    }
}

pub(crate) fn texts(
    runtime: &Runtime,
    arguments: &[Value],
    index: usize,
) -> Result<Vec<String>, Unwind> {
    match argument(runtime, arguments, index)? {
        Value::List(items) => items
            .iter()
            .map(|item| match item {
                Value::Text(t) => Ok(t.as_str().to_string()),
                other => Err(wrong(runtime, index, "List[Text]", other)),
            })
            .collect(),
        other => Err(wrong(runtime, index, "List[Text]", other)),
    }
}

pub(crate) fn any(runtime: &Runtime, arguments: &[Value], index: usize) -> Result<Value, Unwind> {
    argument(runtime, arguments, index).cloned()
}

/// The first argument as a uniquely owned list, copied first if it is shared (copy-on-write).
pub(crate) fn self_list<'a>(
    runtime: &Runtime,
    arguments: &'a mut [Value],
) -> Result<&'a mut crate::heap::Counted<Vec<Value>>, Unwind> {
    match arguments.first_mut() {
        Some(Value::List(items)) => Ok(Rc::make_mut(items)),
        Some(other) => {
            let other = other.clone();
            Err(wrong(runtime, 0, "List", &other))
        }
        None => Err(wrong(runtime, 0, "List", &Value::Unit)),
    }
}

/// The first argument as a uniquely owned map.
pub(crate) fn self_map<'a>(
    runtime: &Runtime,
    arguments: &'a mut [Value],
) -> Result<&'a mut crate::heap::Counted<crate::value::OrderedMap>, Unwind> {
    match arguments.first_mut() {
        Some(Value::Map(map)) => Ok(Rc::make_mut(map)),
        Some(other) => {
            let other = other.clone();
            Err(wrong(runtime, 0, "Map", &other))
        }
        None => Err(wrong(runtime, 0, "Map", &Value::Unit)),
    }
}

/// The first argument as a uniquely owned set.
pub(crate) fn self_set<'a>(
    runtime: &Runtime,
    arguments: &'a mut [Value],
) -> Result<&'a mut crate::heap::Counted<crate::value::OrderedSet>, Unwind> {
    match arguments.first_mut() {
        Some(Value::Set(set)) => Ok(Rc::make_mut(set)),
        Some(other) => {
            let other = other.clone();
            Err(wrong(runtime, 0, "Set", &other))
        }
        None => Err(wrong(runtime, 0, "Set", &Value::Unit)),
    }
}

// ---------------------------------------------------------------- errors of std.files

/// The variant of `std.files.IoErrorKind` for an operating-system error.
pub(crate) fn io_kind(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound => "NotFound",
        ErrorKind::PermissionDenied => "PermissionDenied",
        ErrorKind::AlreadyExists => "AlreadyExists",
        ErrorKind::InvalidData => "InvalidData",
        ErrorKind::Unsupported => "Unsupported",
        _ => "Other",
    }
}

/// A value of `std.files.IoError`.
pub(crate) fn io_error_value(path: &str, kind: &str, reason: impl Into<String>) -> Value {
    Value::record(
        "IoError",
        vec![
            ("path", Value::path(path)),
            ("kind", Value::variant("IoErrorKind", kind, vec![])),
            ("reason", Value::text(reason.into())),
        ],
    )
}

/// An `IoError` that ends the external with an error.
pub(crate) fn io_error(path: &str, kind: &str, reason: impl Into<String>) -> Unwind {
    Unwind::Error(io_error_value(path, kind, reason))
}

/// An `IoError` for an operating-system error.
pub(crate) fn os_error(path: &str, error: &std::io::Error) -> Unwind {
    io_error(path, io_kind(error), error.to_string())
}
