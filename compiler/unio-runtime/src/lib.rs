//! The Unio runtime (docs/runtime.md): values with copy-on-write and reference counting,
//! faults, exhaustion and termination, the release stack of `using`, and the 70 external
//! functions of the standard library behind a table of bindings.
//!
//! The interpreter of sprint S5 evaluates programs on top of this crate; the native code of S6
//! links the same functions.

pub mod deadline;
pub mod externs;
pub mod fault;
pub mod file_trace;
pub mod heap;
pub mod reserve;
pub mod runtime;
pub mod sandbox;
pub mod value;

pub use externs::{bindings, call, ExternFn};
pub use fault::{Exhaustion, Fault, FaultKind, Frame, Outcome, ReleaseStack, Unwind};
pub use runtime::{FailPoint, Runtime};
pub use value::{Callable, Captures, OrderedMap, OrderedSet, Text, Value};

/// Every allocation of a program goes through the allocator of the reserve for `release`
/// (C-17): the system allocator, except on a thread whose reserve a fault opened.
#[global_allocator]
static ALLOCATOR: reserve::ReserveAllocator = reserve::ReserveAllocator;
