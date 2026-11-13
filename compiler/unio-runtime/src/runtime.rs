//! The state a running program needs from the runtime: its arguments, standard output and
//! error, the call stack for traces and its limit, the heap budget, open file descriptors,
//! resource identities, and a fault-injection hook for tests (requirement S-9b).

use crate::fault::{Exhaustion, Fault, FaultKind, Frame, Site, Unwind};
use crate::layers::HookWriter;
use crate::reserve::Reserve;
use crate::value::{ResourceCell, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs::File;
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
        // Standard output and error belong to the std layer: the core runtime writes them
        // through the hooks of the profile (S-12).
        match self {
            Sink::Stdout => (crate::layers::hooks().write_output)(bytes),
            Sink::Stderr => (crate::layers::hooks().write_error)(bytes),
            // The captured output belongs to the harness, not to the program: it is not
            // counted in the heap and never taken from the reserve for `release`.
            Sink::Buffer(buffer) => {
                crate::reserve::outside(|| buffer.borrow_mut().extend_from_slice(bytes));
            }
        }
    }

    /// Writes a fault line by line, without building its text (C-17).
    fn write_fault(&mut self, fault: &Fault) {
        let _ = match self {
            Sink::Stdout => fault.write_to(&mut HookWriter(crate::layers::hooks().write_output)),
            Sink::Stderr => fault.write_to(&mut HookWriter(crate::layers::hooks().write_error)),
            Sink::Buffer(buffer) => {
                crate::reserve::outside(|| fault.write_to(&mut *buffer.borrow_mut()))
            }
        };
    }
}

/// The environment variable of the test hook that fills the heap before `main` (plan section
/// 12, condition 1): with a heap limit, the heap is counted as full but for this many bytes, so
/// that a test reaches a fault on a full heap exactly in `unio run`, `unio test`, and an
/// executable alike. It can only shrink the budget of the program.
pub const TEST_HEAP_FREE: &str = "UNIO_TEST_HEAP_FREE";

/// The runtime state of one program or one test.
pub struct Runtime {
    /// The arguments of the program without its name (std.args.list).
    pub args: Vec<String>,
    output: Sink,
    error: Sink,
    /// The function and file of each place a call can be in, named once (see [`Runtime::site`]).
    sites: Vec<Rc<Site>>,
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
    descriptors: BTreeMap<i64, File>,
    next_descriptor: i64,
    next_resource: u64,
    temporary_counter: u64,
    /// The system call that the next protocol step fails at; tests only.
    pub fail_point: Option<FailPoint>,
    /// How many descriptors were released, for tests of "closed once" (S-9c).
    pub releases: Cell<usize>,
    /// The reserve for `release`, set aside before `main` (C-17).
    reserve: Reserve,
    /// Whether a refusal of the allocator is deferred (the harness of `unio test`).
    deferred: bool,
    /// The heap of the program and its limit (SB-3, C-18): the last field, so that it counts
    /// the frees of everything else of the runtime.
    meter: crate::heap::Meter,
}

impl Runtime {
    pub fn new(args: Vec<String>) -> Runtime {
        // The heap of the program is counted from here on (SB-3, C-18), the reserve for
        // `release` included, which is set aside before anything of the program runs (C-17).
        let heap_limit = crate::sandbox::heap_limit();
        let meter = crate::heap::Meter::start(heap_limit);
        let reserve = Reserve::set_aside();
        let free = std::env::var(TEST_HEAP_FREE)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok());
        Runtime {
            args,
            output: Sink::Stdout,
            error: Sink::Stderr,
            sites: Vec::new(),
            // Room for the deepest stack, so that a `release` that runs deeper than the program
            // went before does not grow it on the path of a fault (C-17).
            frames: Vec::with_capacity(10_000),
            depth: 0,
            base: None,
            stack_limit: 10_000,
            descriptors: BTreeMap::new(),
            next_descriptor: 3,
            next_resource: 1,
            temporary_counter: 0,
            fail_point: None,
            releases: Cell::new(0),
            reserve,
            deferred: false,
            meter,
        }
        .filled(heap_limit.zip(free))
    }

    /// Fills the heap by the test hook, once the runtime holds what it holds before `main`.
    fn filled(self, limit_and_free: Option<(usize, usize)>) -> Runtime {
        if let Some((limit, free)) = limit_and_free {
            crate::heap::fill_to(limit.saturating_sub(free));
        }
        self
    }

    /// The reserve for `release` of this runtime (C-17).
    pub fn reserve(&self) -> &Reserve {
        &self.reserve
    }

    /// A runtime whose standard output and error are captured; returns both buffers.
    pub fn captured(args: Vec<String>) -> (Runtime, Captured, Captured) {
        let out = Rc::new(RefCell::new(Vec::new()));
        let err = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = Runtime::new(args);
        runtime.output = Sink::Buffer(Rc::clone(&out));
        runtime.error = Sink::Buffer(Rc::clone(&err));
        // The harness goes on after a test that exhausted the heap (crate::heap, OVERDRAFT).
        runtime.defer_refusals();
        (runtime, out, err)
    }

    pub fn write_output(&mut self, text: &str) {
        self.output.write(text.as_bytes());
    }

    pub fn write_error(&mut self, text: &str) {
        self.error.write(text.as_bytes());
    }

    /// Writes the diagnostic of a fault to standard error, line by line (C-14, C-17).
    pub fn write_fault(&mut self, fault: &Fault) {
        self.error.write_fault(fault);
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
        self.sites.push(Rc::new(Site { function, file }));
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
                self.sites.extend(
                    make()
                        .into_iter()
                        .map(|(function, file)| Rc::new(Site { function, file })),
                );
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
        // A refusal the harness deferred ends the test at its next call (crate::heap).
        if self.deferred && crate::heap::exhausted() {
            return Err(Unwind::Exhausted(Exhaustion::Heap));
        }
        // So does the time limit of a test (T-12, crate::deadline).
        if crate::deadline::time_up() {
            return Err(Unwind::Exhausted(Exhaustion::Time));
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
    /// limit. Native code pushes a frame by writing `(site, line)` at `frames[depth]` when the
    /// depth is below the limit; it keeps the depth in a register and stores it here before any
    /// call that can read the stack, and its entries restore it when they return. The addresses
    /// stay valid while `stack_limit` does not grow.
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
            .map(|&(site, line)| Frame {
                site: Rc::clone(&self.sites[site as usize]),
                line,
            })
            .collect()
    }

    /// A fault with the current trace. The reserve for `release` opens before the message and
    /// the trace are built (C-17); pass the message unformatted (`format_args!`).
    pub fn fault(&self, kind: FaultKind, message: impl std::fmt::Display) -> Unwind {
        self.traced(Fault::new(kind, message))
    }

    /// The fault with the current trace.
    pub fn traced(&self, mut fault: Fault) -> Unwind {
        crate::reserve::open();
        fault.trace = self.trace();
        Unwind::Fault(Box::new(fault))
    }

    // ---------------------------------------------------------------- memory

    /// Makes a refusal of the allocator end the test at its next check of the heap instead of
    /// the process, as the harness of `unio test` does ([`Runtime::captured`], [`crate::heap`]).
    pub fn defer_refusals(&mut self) {
        self.meter.defer();
        self.deferred = true;
    }

    /// How much a deferred refusal may grant above the limit ([`crate::heap::Meter::set_overdraft`]).
    pub fn set_heap_overdraft(&mut self, bytes: usize) {
        self.meter.set_overdraft(bytes);
    }

    /// The heap limit of the policy in bytes (SB-3); `None` leaves the system to decide.
    pub fn heap_limit(&self) -> Option<usize> {
        self.meter.limit()
    }

    /// Changes the heap limit, which the allocator enforces from the next allocation on.
    pub fn set_heap_limit(&mut self, limit: Option<usize>) {
        self.meter.set_limit(limit);
    }

    /// The heap of the program in bytes: every block its threads requested and still hold, the
    /// reserve for `release` included (docs/runtime.md 3.3).
    pub fn heap_bytes(&self) -> usize {
        self.meter.bytes()
    }

    /// What a thread the runtime starts for the program needs to count into its heap.
    pub fn heap_share(&self) -> crate::heap::Share {
        self.meter.share()
    }

    /// Ends with code 102 when the program holds more than the heap limit, or a refusal of the
    /// allocator was deferred (the harness of `unio test`, [`crate::heap`]). On the path of a
    /// fault, the reserve for `release` bounds it instead, with or without a heap limit and
    /// whatever room the heap has: once an allocation of the path did not fit in the reserve,
    /// also a block freed again before this check, it is exhaustion ([`crate::reserve::room`];
    /// C-17, C-18).
    pub fn check_heap(&self) -> Result<(), Unwind> {
        self.heap_room(0)
    }

    /// Ends with code 102 when `bytes` more would exceed the heap limit, or, on the path of a
    /// fault, would not fit in the reserve. Called before an allocation whose size the program
    /// decides (a read buffer, a file, captured output), so that it unwinds instead of reaching
    /// the refusal of the allocator.
    pub fn reserve_heap(&self, bytes: usize) -> Result<(), Unwind> {
        self.heap_room(bytes)
    }

    fn heap_room(&self, bytes: usize) -> Result<(), Unwind> {
        if crate::heap::exhausted() {
            return Err(Unwind::Exhausted(Exhaustion::Heap));
        }
        let fits = match crate::reserve::room(bytes) {
            Some(fits) => fits,
            None => self
                .meter
                .limit()
                .is_none_or(|limit| self.meter.bytes().saturating_add(bytes) <= limit),
        };
        if fits {
            Ok(())
        } else {
            Err(Unwind::Exhausted(Exhaustion::Heap))
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
        runtime.set_heap_limit(Some(1024));
        assert!(matches!(
            runtime.check_heap(),
            Err(Unwind::Exhausted(Exhaustion::Heap))
        ));
        runtime.set_heap_limit(None);
        assert!(runtime.check_heap().is_ok());
    }

    // ------------------------------------------------------------ the reserve for release (C-17)

    use crate::fault::{int_divide, ReleaseStack};
    use crate::heap::{fill_to, live_bytes};
    use crate::reserve::{self, MINIMUM_HEAP_LIMIT, RESERVE_BYTES};

    #[test]
    fn the_release_reserve_is_set_aside_before_main() {
        let mut runtime = Runtime::new(Vec::new());
        // Allocated, its pages written, counted in the heap, and closed.
        assert!(runtime.reserve().is_set_aside());
        assert_eq!(runtime.reserve().range().len(), RESERVE_BYTES);
        assert!(runtime.reserve().pages_are_written());
        // The reserve, and the frames of the call stack, set aside as well (C-17).
        let frames = 10_000 * std::mem::size_of::<(u32, u32)>();
        assert_eq!(live_bytes(), RESERVE_BYTES + frames);
        assert!(!reserve::is_open());
        // Within the limit, not on top of it: at the minimum limit the program has the limit
        // minus the reserve, and the allocator refuses a block beyond it.
        runtime.set_heap_limit(Some(MINIMUM_HEAP_LIMIT));
        let room = MINIMUM_HEAP_LIMIT - live_bytes();
        assert!(room <= MINIMUM_HEAP_LIMIT - RESERVE_BYTES);
        let fits = Value::bytes(vec![0; room - 64 * 1024]);
        assert!(runtime.check_heap().is_ok());
        let mut over: Vec<u8> = Vec::new();
        assert!(reserve::fallible(|| over.try_reserve_exact(128 * 1024)).is_err());
        assert!(matches!(
            runtime.reserve_heap(128 * 1024),
            Err(Unwind::Exhausted(Exhaustion::Heap))
        ));
        drop(fits);
        drop(runtime);
        assert_eq!(live_bytes(), 0, "the count leaves with its runtime");
    }

    #[test]
    fn the_reserve_opens_only_for_unwinding() {
        let mut runtime = Runtime::new(Vec::new());
        runtime.set_heap_limit(Some(MINIMUM_HEAP_LIMIT));
        runtime.stack_limit = 10;
        runtime.enter("main", "a.unio", 1).unwrap();
        let deep = runtime.site("deep".into(), "a.unio".into());
        let error = Unwind::Error(Box::new(Value::Int(1)));
        fill_to(MINIMUM_HEAP_LIMIT + 1);
        // User code on a full heap gets exhaustion, not the reserve: the allocator refuses.
        let mut ordinary: Vec<u8> = Vec::new();
        assert!(reserve::fallible(|| ordinary.try_reserve_exact(64)).is_err());
        assert!(matches!(
            runtime.check_heap(),
            Err(Unwind::Exhausted(Exhaustion::Heap))
        ));
        assert!(runtime.reserve_heap(16).is_err());
        assert!(!reserve::is_open());
        // Neither an error nor stack exhaustion opens it.
        assert_eq!(error.exit_code(), None);
        runtime.stack_limit = 1;
        assert!(runtime.enter_site(deep, 1).is_err());
        assert!(!reserve::is_open());
        // A fault does, and the path of the fault may go on on the full heap.
        runtime.stack_limit = 10;
        let fault = runtime.fault(FaultKind::AssertFail, "boom");
        assert!(reserve::is_open());
        assert!(runtime.check_heap().is_ok());
        assert!(runtime.reserve_heap(4096).is_ok());
        assert!(runtime.reserve_heap(2 * RESERVE_BYTES).is_err());
        let served: Vec<u8> = Vec::with_capacity(64);
        assert!(reserve::contains(served.as_ptr()));
        // Received by expect_fault, it closes again: the program has its old budget.
        drop((fault, served));
        reserve::close();
        assert!(!reserve::is_open());
        assert!(runtime.check_heap().is_err());
        crate::heap::clear_ballast();
    }

    #[test]
    fn the_reserve_opens_before_the_fault_is_built() {
        let mut runtime = Runtime::new(Vec::new());
        runtime.enter("main", "a.unio", 3).unwrap();
        runtime.enter("load", "a.unio", 9).unwrap();
        assert!(!reserve::is_open());
        let Unwind::Fault(fault) = runtime.fault(
            FaultKind::IndexOutOfRange,
            format_args!("index {} is outside 0..{}", 5, 3),
        ) else {
            unreachable!()
        };
        // The message, the trace, and the fault itself are on the reserved pages.
        assert_eq!(fault.message, "index 5 is outside 0..3");
        assert!(reserve::contains(fault.message.as_ptr()));
        assert!(reserve::contains(fault.trace.as_ptr()));
        assert!(reserve::contains(&*fault as *const Fault));
        assert_eq!(fault.trace.len(), 2);
        drop(fault);
        reserve::close();
        // A fault of arithmetic, made without the runtime, opens it as well.
        let fault = int_divide(1, 0).unwrap_err();
        assert!(reserve::is_open());
        assert!(reserve::contains(fault.message.as_ptr()));
        let Unwind::Fault(traced) = runtime.traced(fault) else {
            unreachable!()
        };
        assert!(reserve::contains(traced.trace.as_ptr()));
    }

    #[test]
    fn the_unwinding_path_allocates_from_the_reserved_pages() {
        let mut runtime = Runtime::new(Vec::new());
        runtime.set_heap_limit(Some(MINIMUM_HEAP_LIMIT));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut stack = ReleaseStack::new();
        for name in ["outer", "inner"] {
            let seen = Rc::clone(&seen);
            stack.open(Box::new(move |runtime: &mut Runtime| {
                // What a release does: values, texts, a list, checked against the heap.
                let words: Vec<Value> = (0..2000)
                    .map(|i| Value::text(format!("{name} {i}")))
                    .collect();
                let first = match &words[0] {
                    Value::Text(t) => reserve::contains(t.as_str().as_ptr()),
                    _ => false,
                };
                let list = Value::list(words);
                let checked = runtime.check_heap().is_ok();
                seen.borrow_mut()
                    .push((name, first, checked, matches!(list, Value::List(..))));
                Ok(())
            }));
        }
        // Full from here on: what the releases allocate can only come from the reserve.
        fill_to(MINIMUM_HEAP_LIMIT);
        reserve::reset_peak();
        let fault = runtime.fault(FaultKind::DivisionByZero, "division by zero");
        let faults = stack.unwind_all(&mut runtime);
        assert!(faults.is_empty());
        assert_eq!(
            *seen.borrow(),
            [("inner", true, true, true), ("outer", true, true, true)]
        );
        assert!(reserve::peak() > 2000 * 16, "the releases used the reserve");
        assert!(!reserve::overflowed());
        // A release that needs more than the reserve is exhaustion, with room on the heap too
        // (on a full heap the allocator refuses the block, which ends the program at once).
        fill_to(0);
        runtime.set_heap_limit(Some(8 * RESERVE_BYTES));
        let big: Vec<u8> = vec![1; RESERVE_BYTES + 1];
        let counted = Value::bytes(big);
        assert!(reserve::overflowed());
        assert!(matches!(
            runtime.check_heap(),
            Err(Unwind::Exhausted(Exhaustion::Heap))
        ));
        drop((counted, fault));
        crate::heap::clear_ballast();
    }

    #[test]
    fn a_block_over_the_reserve_freed_before_the_check_is_exhaustion() {
        let mut runtime = Runtime::new(Vec::new());
        // With little room on the heap, with much room, and without a heap limit: once the
        // reserve is open, a block that does not fit in it is exhaustion, even freed at once.
        // (On a full heap the allocator refuses the block itself: code 102 at once.)
        for (limit, filled) in [
            (Some(8 * RESERVE_BYTES), 5 * RESERVE_BYTES),
            (Some(8 * RESERVE_BYTES), 0),
            (None, 0),
        ] {
            runtime.set_heap_limit(limit);
            fill_to(filled);
            let fault = runtime.fault(FaultKind::DivisionByZero, "division by zero");
            assert!(runtime.check_heap().is_ok(), "{limit:?}");
            assert!(runtime.reserve_heap(4096).is_ok(), "{limit:?}");
            drop(std::hint::black_box(Vec::<u8>::with_capacity(
                2 * RESERVE_BYTES,
            )));
            assert!(reserve::overflowed());
            assert!(
                matches!(
                    runtime.check_heap(),
                    Err(Unwind::Exhausted(Exhaustion::Heap))
                ),
                "{limit:?}"
            );
            assert!(runtime.reserve_heap(0).is_err(), "{limit:?}");
            // expect_fault closes the reserve: the overflow of its path is gone with it.
            drop(fault);
            reserve::close();
            assert!(!reserve::overflowed());
            fill_to(0);
            assert!(runtime.check_heap().is_ok(), "{limit:?}");
        }
        // A buffer the program sizes is refused when it does not fit in the open reserve,
        // without a heap limit as with one.
        let fault = runtime.fault(FaultKind::DivisionByZero, "division by zero");
        assert!(runtime.reserve_heap(2 * RESERVE_BYTES).is_err());
        drop(fault);
        reserve::close();
        assert!(runtime.reserve_heap(2 * RESERVE_BYTES).is_ok());
        crate::heap::clear_ballast();
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
