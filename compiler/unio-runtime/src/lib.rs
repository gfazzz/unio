//! The Unio runtime (docs/runtime.md): values with copy-on-write and reference counting,
//! faults, exhaustion and termination, the release stack of `using`, and the 70 external
//! functions of the standard library behind a table of bindings.
//!
//! The interpreter of sprint S5 evaluates programs on top of this crate; the native code of S6
//! links the same functions.

pub mod externs;
pub mod fault;
pub mod heap;
pub mod runtime;
pub mod sandbox;
pub mod value;

pub use externs::{bindings, call, ExternFn};
pub use fault::{Exhaustion, Fault, FaultKind, Frame, Outcome, ReleaseStack, Unwind};
pub use runtime::{FailPoint, Runtime};
pub use value::{Callable, OrderedMap, OrderedSet, Text, Value};
