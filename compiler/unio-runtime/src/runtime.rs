//! The state a running program needs from the runtime: its arguments, standard output and
//! error, the call stack for traces and its limit, the heap budget, open file descriptors,
//! resource identities, and a fault-injection hook for tests (requirement S-9b).

use crate::fault::{Exhaustion, Fault, FaultKind, Frame, Unwind};
use crate::value::{ResourceCell, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::rc::Rc;

/// A system call of the atomic commit protocol that a test can make fail (S-9b).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailPoint {
    Write,
    SyncData,
    Close,
    Rename,
    SyncDirectory,
}

/// Bytes written to a captured standard output or error.
pub type Captured = Rc<RefCell<Vec<u8>>>;

/// Where standard output or error goes.
pub enum Sink {
    Stdout,
    Stderr,
    /// Captured in memory, for tests and for `unio test`.
    Buffer(Rc<RefCell<Vec<u8>>>),
}

impl Sink {
    fn write(&mut self, bytes: &[u8]) {
        // Writing to standard output cannot fail in Unio (write_output has no fails with);
        // a closed pipe loses the text, as in other command-line programs.
        let _ = match self {
            Sink::Stdout => std::io::stdout().write_all(bytes),
            Sink::Stderr => std::io::stderr().write_all(bytes),
            Sink::Buffer(buffer) => {
                buffer.borrow_mut().extend_from_slice(bytes);
                Ok(())
            }
        };
    }
}

/// The runtime state of one program or one test.
pub struct Runtime {
    /// The arguments of the program without its name (std.args.list).
    pub args: Vec<String>,
    output: Sink,
    error: Sink,
    /// The function and file of each place a call can be in, named once (see [`Runtime::site`]).
    sites: Vec<(Rc<str>, Rc<str>)>,
    /// The call stack: a site and the current line of each call, in `frames[..depth]`. Names
    /// are looked up only when a fault needs a trace, so that a call costs two integers. The
    /// storage only grows, so that native code can keep its address (see
    /// [`Runtime::native_stack`]).
    frames: Vec<(u32, u32)>,
    depth: usize,
    /// The first site of the table installed by [`Runtime::site_base`].
    base: Option<u32>,
    /// The deepest call stack allowed before code 102 (C-18).
    pub stack_limit: usize,
    /// The heap limit of the policy in bytes (SB-3); `None` leaves the system to decide.
    pub heap_limit: Option<usize>,
    descriptors: BTreeMap<i64, File>,
    next_descriptor: i64,
    next_resource: u64,
    temporary_counter: u64,
    /// The system call that the next protocol step fails at; tests only.
    pub fail_point: Option<FailPoint>,
    /// How many descriptors were released, for tests of "closed once" (S-9c).
    pub releases: Cell<usize>,
}

impl Runtime {
    pub fn new(args: Vec<String>) -> Runtime {
        Runtime {
            args,
            output: Sink::Stdout,
            error: Sink::Stderr,
            sites: Vec::new(),
            frames: Vec::new(),
            depth: 0,
            base: None,
            stack_limit: 10_000,
            heap_limit: crate::sandbox::heap_limit(),
            descriptors: BTreeMap::new(),
            next_descriptor: 3,
            next_resource: 1,
            temporary_counter: 0,
            fail_point: None,
            releases: Cell::new(0),
        }
    }

    /// A runtime whose standard output and error are captured; returns both buffers.
    pub fn captured(args: Vec<String>) -> (Runtime, Captured, Captured) {
        let out = Rc::new(RefCell::new(Vec::new()));
        let err = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = Runtime::new(args);
        runtime.output = Sink::Buffer(Rc::clone(&out));
        runtime.error = Sink::Buffer(Rc::clone(&err));
        (runtime, out, err)
    }

    pub fn write_output(&mut self, text: &str) {
        self.output.write(text.as_bytes());
    }

    pub fn write_error(&mut self, text: &str) {
        self.error.write(text.as_bytes());
    }

    // ---------------------------------------------------------------- calls and traces

    /// Enters a call; exceeding the stack limit is exhaustion with code 102.
    pub fn enter(
        &mut self,
        function: impl Into<std::rc::Rc<str>>,
        file: impl Into<std::rc::Rc<str>>,
        line: u32,
    ) -> Result<(), Unwind> {
        let site = self.site(function.into(), file.into());
        self.enter_site(site, line)
    }

    /// Names a function and its file for [`Runtime::enter_site`].
    pub fn site(&mut self, function: Rc<str>, file: Rc<str>) -> u32 {
        self.sites.push((function, file));
        (self.sites.len() - 1) as u32
    }

    /// The first site of a table of sites that the caller makes once per runtime, such as
    /// one site per function of a program; the table is made by `make` on the first call.
    #[inline]
    pub fn site_base(&mut self, make: impl FnOnce() -> Vec<(Rc<str>, Rc<str>)>) -> u32 {
        match self.base {
            Some(base) => base,
            None => {
                let base = self.sites.len() as u32;
                self.sites.extend(make());
                self.base = Some(base);
                base
            }
        }
    }

    /// Enters a call of a site made by [`Runtime::site`].
    #[inline]
    pub fn enter_site(&mut self, site: u32, line: u32) -> Result<(), Unwind> {
        if self.depth >= self.stack_limit {
            return Err(Unwind::Exhausted(Exhaustion::Stack));
        }
        if self.depth == self.frames.len() {
            self.frames.push((site, line));
        } else {
            self.frames[self.depth] = (site, line);
        }
        self.depth += 1;
        Ok(())
    }

    /// Leaves the innermost call.
    #[inline]
    pub fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Moves the line of the innermost call, so that a trace names the current line.
    #[inline]
    pub fn at_line(&mut self, line: u32) {
        if let Some(frame) = self.depth.checked_sub(1).map(|d| &mut self.frames[d]) {
            frame.1 = line;
        }
    }

    /// The call stack for native code: the storage of `stack_limit` frames, the depth, and the
    /// limit. Native code pushes a frame by writing `(site, line)` at `frames[depth]` and
    /// incrementing the depth when it is below the limit, and pops it by restoring the depth;
    /// the addresses stay valid while `stack_limit` does not grow.
    pub fn native_stack(&mut self) -> (*mut (u32, u32), *mut usize, usize) {
        if self.frames.len() < self.stack_limit {
            self.frames.resize(self.stack_limit, (0, 0));
        }
        (self.frames.as_mut_ptr(), &mut self.depth, self.stack_limit)
    }

    /// The current call stack with names, outermost first.
    pub fn trace(&self) -> Vec<Frame> {
        self.frames[..self.depth]
            .iter()
            .map(|&(site, line)| {
                let (function, file) = &self.sites[site as usize];
                Frame {
                    function: Rc::clone(function),
                    file: Rc::clone(file),
                    line,
                }
            })
            .collect()
    }

    /// A fault with the current trace.
    pub fn fault(&self, kind: FaultKind, message: impl Into<String>) -> Unwind {
        let mut fault = Fault::new(kind, message);
        fault.trace = self.trace();
        Unwind::Fault(Box::new(fault))
    }

    // ---------------------------------------------------------------- memory

    /// Ends with code 102 when the values of the program hold more than the heap limit.
    pub fn check_heap(&self) -> Result<(), Unwind> {
        match self.heap_limit {
            Some(limit) if crate::heap::live_bytes() > limit => {
                Err(Unwind::Exhausted(Exhaustion::Heap))
            }
            _ => Ok(()),
        }
    }

    /// Ends with code 102 when `bytes` more would exceed the heap limit. Called before an
    /// allocation whose size the program decides (a read buffer, a file, captured output), so
    /// that memory over the budget is refused instead of allocated.
    pub fn reserve_heap(&self, bytes: usize) -> Result<(), Unwind> {
        match self.heap_limit {
            Some(limit) if crate::heap::live_bytes().saturating_add(bytes) > limit => {
                Err(Unwind::Exhausted(Exhaustion::Heap))
            }
            _ => Ok(()),
        }
    }

    // ---------------------------------------------------------------- resources

    /// A new resource with its own identity.
    pub fn new_resource(&mut self, value: Value) -> Value {
        let id = self.next_resource;
        self.next_resource += 1;
        Value::Resource(Rc::new(ResourceCell {
            id,
            value: RefCell::new(value),
            released: Cell::new(false),
        }))
    }

    // ---------------------------------------------------------------- descriptors

    pub(crate) fn add_descriptor(&mut self, file: File) -> i64 {
        let descriptor = self.next_descriptor;
        self.next_descriptor += 1;
        self.descriptors.insert(descriptor, file);
        descriptor
    }

    pub(crate) fn descriptor(&mut self, descriptor: i64) -> Option<&mut File> {
        self.descriptors.get_mut(&descriptor)
    }

    /// Removes the descriptor; dropping the file releases it in the operating system.
    pub(crate) fn release_descriptor(&mut self, descriptor: i64) -> Option<File> {
        let file = self.descriptors.remove(&descriptor)?;
        self.releases.set(self.releases.get() + 1);
        Some(file)
    }

    /// Whether a descriptor is open; tests use it to check that a descriptor is released once.
    pub fn is_open(&self, descriptor: i64) -> bool {
        self.descriptors.contains_key(&descriptor)
    }

    pub(crate) fn next_temporary_id(&mut self) -> String {
        self.temporary_counter += 1;
        format!("{}-{}", std::process::id(), self.temporary_counter)
    }

    /// Takes the fail point if it matches, so that it fails exactly one call.
    pub(crate) fn fails_at(&mut self, point: FailPoint) -> bool {
        if self.fail_point == Some(point) {
            self.fail_point = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traces_follow_calls_and_the_stack_has_a_limit() {
        let mut runtime = Runtime::new(Vec::new());
        runtime.stack_limit = 2;
        runtime.enter("main", "a.unio", 1).unwrap();
        runtime.enter("load", "a.unio", 5).unwrap();
        runtime.at_line(7);
        match runtime.fault(FaultKind::AssertFail, "x") {
            Unwind::Fault(f) => assert_eq!(f.trace[1].line, 7),
            _ => unreachable!(),
        }
        assert!(matches!(
            runtime.enter("deep", "a.unio", 9),
            Err(Unwind::Exhausted(Exhaustion::Stack))
        ));
        runtime.leave();
        runtime.enter("again", "a.unio", 9).unwrap();
    }

    #[test]
    fn the_heap_limit_turns_into_exhaustion() {
        let mut runtime = Runtime::new(Vec::new());
        let _big = Value::bytes(vec![0; 1 << 20]);
        runtime.heap_limit = Some(1024);
        assert!(matches!(
            runtime.check_heap(),
            Err(Unwind::Exhausted(Exhaustion::Heap))
        ));
        runtime.heap_limit = None;
        assert!(runtime.check_heap().is_ok());
    }

    #[test]
    fn resources_have_identity() {
        let mut runtime = Runtime::new(Vec::new());
        let a = runtime.new_resource(Value::Int(1));
        let b = runtime.new_resource(Value::Int(1));
        assert_eq!(a, a.clone());
        assert_ne!(a, b);
    }
}
