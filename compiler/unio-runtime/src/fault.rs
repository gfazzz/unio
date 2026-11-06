//! Errors, faults, exhaustion, and termination (docs/runtime.md, sections 4–6; requirements
//! C-14 to C-19).

use crate::value::Value;
use std::fmt;
use std::io::Write;

/// Exit code of a fault (C-15, C-17).
pub const EXIT_FAULT: i32 = 101;
/// Exit code of exhausted memory or stack (C-18).
pub const EXIT_EXHAUSTED: i32 = 102;
/// Exit code when the sandbox or the policy cannot be applied (C-15, SB-5).
pub const EXIT_SANDBOX: i32 = 103;

/// The result of evaluating something: a value, or the way evaluation stopped.
pub type Outcome = Result<Value, Unwind>;

/// How evaluation can stop before producing a value.
#[derive(Debug)]
pub enum Unwind {
    /// An error of a `fails with` function; `try` passes it on and `on` receives it.
    Error(Value),
    /// A fault: the program ends with code 101 after releasing its resources.
    Fault(Fault),
    /// Memory or stack exhausted: the program ends with code 102 without running user code.
    Exhausted(Exhaustion),
}

impl Unwind {
    /// The exit code with which this ends a program; an error never reaches the top of a
    /// checked program, so it has none.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Unwind::Error(_) => None,
            Unwind::Fault(_) => Some(EXIT_FAULT),
            Unwind::Exhausted(_) => Some(EXIT_EXHAUSTED),
        }
    }
}

impl From<Fault> for Unwind {
    fn from(fault: Fault) -> Unwind {
        Unwind::Fault(fault)
    }
}

/// What ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exhaustion {
    Heap,
    Stack,
}

impl Exhaustion {
    /// The diagnostic, prepared without allocation (C-18).
    pub fn message(self) -> &'static str {
        match self {
            Exhaustion::Heap => "fault: memory exhausted; the program ends with code 102\n",
            Exhaustion::Stack => {
                "fault: stack exhausted by recursion; the program ends with code 102\n"
            }
        }
    }
}

/// Ends the process after exhaustion: writes the prepared message and exits with code 102.
pub fn emergency_exit(what: Exhaustion) -> ! {
    let _ = std::io::stderr().write_all(what.message().as_bytes());
    std::process::exit(EXIT_EXHAUSTED)
}

/// The kinds of faults (L-17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    IntegerOverflow,
    DivisionByZero,
    IndexOutOfRange,
    MissingKey,
    AssertionFailed,
    AssertFail,
    ExpectedFault,
    InvalidExitCode,
    InvalidArgument,
    NotProvided,
}

/// One frame of a fault trace (C-14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub function: String,
    pub file: String,
    pub line: u32,
}

/// A fault: its kind, its message, and the trace of the calls that led to it.
#[derive(Clone, Debug)]
pub struct Fault {
    pub kind: FaultKind,
    pub message: String,
    pub trace: Vec<Frame>,
}

impl Fault {
    pub fn new(kind: FaultKind, message: impl Into<String>) -> Fault {
        Fault {
            kind,
            message: message.into(),
            trace: Vec::new(),
        }
    }

    /// The diagnostic written to standard error: the message and the trace, innermost first.
    pub fn render(&self) -> String {
        let mut text = format!("fault: {}\n", self.message);
        for frame in self.trace.iter().rev() {
            text.push_str(&format!(
                "    at {} ({}:{})\n",
                frame.function, frame.file, frame.line
            ));
        }
        text
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The exit code of a program whose `main` returned `value` (C-15): `Unit` gives 0, an `Int`
/// in 0–100 gives itself, anything else is a fault.
pub fn main_exit_code(value: &Value) -> Result<i32, Fault> {
    match value {
        Value::Unit => Ok(0),
        Value::Int(code) if (0..=100).contains(code) => Ok(*code as i32),
        Value::Int(code) => Err(Fault::new(
            FaultKind::InvalidExitCode,
            format!("main returned {code}; an exit code of main lies in 0–100"),
        )),
        other => Err(Fault::new(
            FaultKind::InvalidExitCode,
            format!(
                "main returned a {}; it returns Int or nothing",
                other.kind()
            ),
        )),
    }
}

// ---------------------------------------------------------------- Int arithmetic (section 2.1)

fn overflow(operation: &str) -> Fault {
    Fault::new(
        FaultKind::IntegerOverflow,
        format!("Int overflow in {operation}; use the checked form to handle it"),
    )
}

pub fn int_add(a: i64, b: i64) -> Result<i64, Fault> {
    a.checked_add(b).ok_or_else(|| overflow("+"))
}

pub fn int_subtract(a: i64, b: i64) -> Result<i64, Fault> {
    a.checked_sub(b).ok_or_else(|| overflow("-"))
}

pub fn int_multiply(a: i64, b: i64) -> Result<i64, Fault> {
    a.checked_mul(b).ok_or_else(|| overflow("*"))
}

/// `/` truncates toward zero; division by zero and `MINIMUM_INT / -1` are faults.
pub fn int_divide(a: i64, b: i64) -> Result<i64, Fault> {
    if b == 0 {
        return Err(Fault::new(FaultKind::DivisionByZero, "division by zero"));
    }
    a.checked_div(b).ok_or_else(|| overflow("/"))
}

/// `%` has the sign of the left operand; remainder by zero is a fault.
pub fn int_remainder(a: i64, b: i64) -> Result<i64, Fault> {
    if b == 0 {
        return Err(Fault::new(FaultKind::DivisionByZero, "remainder by zero"));
    }
    Ok(a.checked_rem(b).unwrap_or(0))
}

/// `**` for a non-negative exponent; a negative exponent has no `Int` result and is a fault.
pub fn int_power(a: i64, b: i64) -> Result<i64, Fault> {
    if b < 0 {
        return Err(Fault::new(
            FaultKind::InvalidArgument,
            "a negative exponent of Int has no Int result; convert to Float",
        ));
    }
    let exponent = u32::try_from(b).map_err(|_| overflow("**"))?;
    a.checked_pow(exponent).ok_or_else(|| overflow("**"))
}

// ---------------------------------------------------------------- releasing resources (section 5)

/// A pending `release` of a resource opened by `using`.
pub type Release = Box<dyn FnOnce(&mut crate::runtime::Runtime) -> Result<(), Fault>>;

/// The stack of open `using` blocks. The interpreter pushes a release when a block opens and
/// closes the blocks above a depth when they end; a fault unwinds all of them.
#[derive(Default)]
pub struct ReleaseStack {
    entries: Vec<Release>,
}

impl ReleaseStack {
    pub fn new() -> ReleaseStack {
        ReleaseStack::default()
    }

    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    /// Opens a block; returns the depth to close it at.
    pub fn open(&mut self, release: Release) -> usize {
        self.entries.push(release);
        self.entries.len() - 1
    }

    /// Closes every block opened at `depth` or later, innermost first, on normal exit or on an
    /// error. Every release runs; the first fault among them is returned, because the program
    /// then ends with it.
    pub fn close_to(
        &mut self,
        depth: usize,
        runtime: &mut crate::runtime::Runtime,
    ) -> Result<(), Fault> {
        let mut first = None;
        while self.entries.len() > depth {
            let release = self.entries.pop().expect("an open block");
            if let Err(fault) = release(runtime) {
                first.get_or_insert(fault);
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// Runs every pending release, innermost first, while a fault unwinds the program (C-17).
    /// A fault inside a release is collected and the remaining releases still run.
    pub fn unwind_all(&mut self, runtime: &mut crate::runtime::Runtime) -> Vec<Fault> {
        let mut faults = Vec::new();
        while let Some(release) = self.entries.pop() {
            if let Err(fault) = release(runtime) {
                faults.push(fault);
            }
        }
        faults
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Runtime;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn integer_faults() {
        assert!(int_add(i64::MAX, 1).is_err());
        assert!(int_subtract(i64::MIN, 1).is_err());
        assert!(int_multiply(i64::MAX, 2).is_err());
        assert_eq!(int_divide(-7, 2).unwrap(), -3);
        assert_eq!(int_remainder(-7, 2).unwrap(), -1);
        assert_eq!(int_remainder(i64::MIN, -1).unwrap(), 0);
        assert_eq!(
            int_divide(1, 0).unwrap_err().kind,
            FaultKind::DivisionByZero
        );
        assert_eq!(
            int_remainder(1, 0).unwrap_err().kind,
            FaultKind::DivisionByZero
        );
        assert!(int_divide(i64::MIN, -1).is_err());
        assert_eq!(int_power(2, 10).unwrap(), 1024);
        assert!(int_power(2, 63).is_err());
        assert!(int_power(2, -1).is_err());
    }

    #[test]
    fn exit_codes_of_main() {
        assert_eq!(main_exit_code(&Value::Unit).unwrap(), 0);
        assert_eq!(main_exit_code(&Value::Int(100)).unwrap(), 100);
        assert_eq!(
            main_exit_code(&Value::Int(101)).unwrap_err().kind,
            FaultKind::InvalidExitCode
        );
        assert!(main_exit_code(&Value::Int(-1)).is_err());
        assert_eq!(
            Unwind::Fault(Fault::new(FaultKind::AssertFail, "x")).exit_code(),
            Some(101)
        );
        assert_eq!(Unwind::Exhausted(Exhaustion::Heap).exit_code(), Some(102));
        assert_eq!(EXIT_SANDBOX, 103);
    }

    fn recorder(log: &Rc<RefCell<Vec<String>>>, name: &str, fails: bool) -> Release {
        let log = Rc::clone(log);
        let name = name.to_string();
        Box::new(move |_| {
            log.borrow_mut().push(name.clone());
            if fails {
                Err(Fault::new(
                    FaultKind::AssertFail,
                    format!("release {name} failed"),
                ))
            } else {
                Ok(())
            }
        })
    }

    #[test]
    fn releases_run_in_reverse_order_and_survive_a_failing_release() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = Runtime::new(Vec::new());
        let mut stack = ReleaseStack::new();
        stack.open(recorder(&log, "a", false));
        let inner = stack.open(recorder(&log, "b", true));
        stack.open(recorder(&log, "c", false));
        let fault = stack.close_to(inner, &mut runtime).unwrap_err();
        assert_eq!(fault.message, "release b failed");
        assert_eq!(*log.borrow(), ["c", "b"]);
        stack.open(recorder(&log, "d", true));
        stack.open(recorder(&log, "e", false));
        let faults = stack.unwind_all(&mut runtime);
        assert_eq!(faults.len(), 1);
        assert_eq!(*log.borrow(), ["c", "b", "e", "d", "a"]);
        assert_eq!(stack.depth(), 0);
    }

    #[test]
    fn a_fault_renders_its_trace_innermost_first() {
        let mut fault = Fault::new(FaultKind::IndexOutOfRange, "index 5 is outside 0..3");
        fault.trace = vec![
            Frame {
                function: "main".into(),
                file: "a.unio".into(),
                line: 3,
            },
            Frame {
                function: "load".into(),
                file: "a.unio".into(),
                line: 9,
            },
        ];
        let text = fault.render();
        assert!(text.starts_with("fault: index 5 is outside 0..3\n    at load (a.unio:9)\n"));
        assert!(text.ends_with("at main (a.unio:3)\n"));
    }
}
