//! The heap of a program and its limit (docs/runtime.md 3.3 to 3.5; C-18, SB-3, RFC 0010).
//!
//! The heap of a program is every byte the program's threads request from the global allocator
//! while it runs and have not freed: values and their wrappers, reference-count headers,
//! vectors, names, temporary buffers of the runtime, and the reserve for `release` of C-17
//! ([`crate::reserve`]). The global allocator ([`crate::reserve::ReserveAllocator`]) counts each
//! allocation, reallocation, and free here, at the moment it happens, and refuses an allocation
//! that would take the count over the heap limit of the policy (`limits.heap_bytes`). A refused
//! allocation is exhaustion: a fallible reservation ([`crate::reserve::fallible`]) receives the
//! refusal and turns it into code 102 itself; any other ends the program with code 102 and the
//! prepared diagnostic, as an allocation the system refuses does.
//!
//! A size is counted as requested: `size` rounded up to a multiple of `align`, what
//! `Layout::pad_to_align` gives. What the allocator adds on its own (headers of blocks, size
//! classes, pages it keeps after a free) is not counted; docs/runtime.md 3.5 states its measured
//! bound, and the limit of the operating system (`sandbox::apply_rlimits`) leaves room for it.
//!
//! Every program has a [`Meter`], made by its runtime before anything of the program runs and
//! dropped after everything else of the runtime. The thread that runs the program counts into
//! it without atomic read-modify-write operations; a thread the runtime starts for the program
//! (the readers of `process.run`) joins the same meter with [`Share::join`], so that a block
//! allocated on one thread and freed on the other leaves the count exact. Values are not `Send`
//! (they hold `Rc`), so no value of a program is freed on a thread outside its meter.
//!
//! The harness of `unio test` defers a refusal instead ([`Meter::defer`]): the allocation is
//! granted, the meter is marked exhausted, and the next check of the heap ends the test with
//! exhaustion, so that the other tests still run. What a deferred refusal grants is bounded by
//! [`OVERDRAFT`]; beyond it the process ends with code 102 as a program does.

use std::alloc::Layout;
use std::cell::Cell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering};

/// How many meters can exist at once in a process: one per runtime alive.
const METERS: usize = 1024;

/// The most bytes a deferred refusal grants above the limit before the process ends with code
/// 102 (the harness of `unio test` only): the size of the reserve for `release`.
pub const OVERDRAFT: usize = crate::reserve::RESERVE_BYTES;

/// One meter of the static table. The thread that runs the program writes `owner` with a plain
/// load and store; joined threads add to `joined` atomically. Both are signed: a block counted
/// on one thread may be freed on the other.
#[repr(align(64))]
struct Slot {
    in_use: AtomicBool,
    /// Changes each time the slot is taken, so that a joined thread that outlives its program
    /// stops counting into the next one.
    generation: AtomicU32,
    owner: AtomicIsize,
    joined: AtomicIsize,
    /// The ballast of the test hook ([`fill_to`]): counted bytes that hold no memory.
    ballast: AtomicIsize,
    /// The heap limit; `usize::MAX` for none.
    limit: AtomicUsize,
    defer: AtomicBool,
    exhausted: AtomicBool,
    overdraft: AtomicUsize,
}

#[allow(clippy::declare_interior_mutable_const)]
const FREE: Slot = Slot {
    in_use: AtomicBool::new(false),
    generation: AtomicU32::new(0),
    owner: AtomicIsize::new(0),
    joined: AtomicIsize::new(0),
    ballast: AtomicIsize::new(0),
    limit: AtomicUsize::new(usize::MAX),
    defer: AtomicBool::new(false),
    exhausted: AtomicBool::new(false),
    overdraft: AtomicUsize::new(0),
};

static SLOTS: [Slot; METERS] = [FREE; METERS];

/// The tag of a thread: bits 0..16 the slot plus one (0 for no meter), bit 16 set for a joined
/// thread, bits 32..64 the generation the joined thread expects.
const JOINED: u64 = 1 << 16;
const SLOT_BITS: u64 = 0xffff;

thread_local! {
    static TAG: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn tag() -> u64 {
    TAG.try_with(Cell::get).unwrap_or(0)
}

#[inline]
fn set_tag(tag: u64) {
    let _ = TAG.try_with(|t| t.set(tag));
}

/// The slot of a tag, if it still belongs to the program the tag was made for.
#[inline]
fn slot_of(tag: u64) -> Option<&'static Slot> {
    let slot = &SLOTS[(tag & SLOT_BITS) as usize - 1];
    if tag & JOINED != 0 && slot.generation.load(Ordering::Acquire) != (tag >> 32) as u32 {
        return None;
    }
    Some(slot)
}

impl Slot {
    #[inline]
    fn total(&self) -> isize {
        self.owner
            .load(Ordering::Relaxed)
            .wrapping_add(self.joined.load(Ordering::Relaxed))
            .wrapping_add(self.ballast.load(Ordering::Relaxed))
    }

    #[inline]
    fn add(&self, joined: bool, bytes: isize) {
        if joined {
            self.joined.fetch_add(bytes, Ordering::Relaxed);
        } else {
            let owner = self.owner.load(Ordering::Relaxed);
            self.owner
                .store(owner.wrapping_add(bytes), Ordering::Relaxed);
        }
    }
}

/// The bytes a layout requests: its size rounded up to its alignment.
#[inline]
pub fn requested(layout: Layout) -> usize {
    let align = layout.align();
    layout.size().wrapping_add(align - 1) & !(align - 1)
}

/// Counts `bytes` more on this thread's meter before they are allocated. `false` refuses the
/// allocation: the caller returns null, which is exhaustion (C-18).
#[inline]
pub(crate) fn charge(bytes: usize) -> bool {
    let tag = tag();
    if tag == 0 {
        return true;
    }
    charge_tagged(tag, bytes)
}

#[inline]
fn charge_tagged(tag: u64, bytes: usize) -> bool {
    let Some(slot) = slot_of(tag) else {
        return true;
    };
    let after = slot.total().wrapping_add(bytes as isize);
    if after > 0 && after as usize > slot.limit.load(Ordering::Relaxed) {
        return over(slot, tag & JOINED != 0, bytes);
    }
    slot.add(tag & JOINED != 0, bytes as isize);
    true
}

/// An allocation that would exceed the limit: refused, or granted under a deferred refusal.
#[cold]
#[inline(never)]
fn over(slot: &Slot, joined: bool, bytes: usize) -> bool {
    if crate::reserve::in_fallible() || !slot.defer.load(Ordering::Relaxed) {
        return false;
    }
    let granted = slot.overdraft.load(Ordering::Relaxed).saturating_add(bytes);
    if granted > OVERDRAFT {
        return false;
    }
    slot.overdraft.store(granted, Ordering::Relaxed);
    slot.exhausted.store(true, Ordering::Relaxed);
    slot.add(joined, bytes as isize);
    true
}

/// Counts `bytes` less on this thread's meter after they were freed.
#[inline]
pub(crate) fn discharge(bytes: usize) {
    adjust(-(bytes as isize));
}

/// Counts memory the allocator did not hand out through [`charge`], without a check: the
/// reserve for `release`, which is taken from the system directly.
pub(crate) fn add(bytes: usize) {
    adjust(bytes as isize);
}

/// The counterpart of [`add`].
pub(crate) fn sub(bytes: usize) {
    discharge(bytes);
}

#[inline]
fn adjust(bytes: isize) {
    let tag = tag();
    if tag == 0 {
        return;
    }
    if let Some(slot) = slot_of(tag) {
        slot.add(tag & JOINED != 0, bytes);
    }
}

fn current() -> Option<&'static Slot> {
    let tag = tag();
    if tag == 0 {
        None
    } else {
        slot_of(tag)
    }
}

/// The heap of the program of this thread in bytes: what its threads requested and still hold,
/// the reserve, and the ballast of the test hook. 0 on a thread without a program.
pub fn live_bytes() -> usize {
    current().map_or(0, |slot| slot.total().max(0) as usize)
}

/// The heap limit of the program of this thread.
pub fn limit() -> Option<usize> {
    current()
        .map(|slot| slot.limit.load(Ordering::Relaxed))
        .filter(|&limit| limit != usize::MAX)
}

/// Whether a deferred refusal marked the program of this thread exhausted.
pub fn exhausted() -> bool {
    current().is_some_and(|slot| slot.exhausted.load(Ordering::Relaxed))
}

/// Test hook, like `fail_point` of S-9b (plan section 12, condition 1): sets the count of this
/// thread to `count` bytes by adding a ballast that holds no memory, so that a test fills the
/// heap exactly. A count below what the program holds removes the ballast. The ballast only
/// ever shrinks the budget of the program.
pub fn fill_to(count: usize) {
    if let Some(slot) = current() {
        let ballast = slot.ballast.load(Ordering::Relaxed);
        let without = slot.total().wrapping_sub(ballast);
        let wanted = (count as isize).saturating_sub(without).max(0);
        slot.ballast.store(wanted, Ordering::Relaxed);
    }
}

/// Removes the ballast of [`fill_to`].
pub fn clear_ballast() {
    if let Some(slot) = current() {
        slot.ballast.store(0, Ordering::Relaxed);
    }
}

/// Runs `f` without counting on this thread: for memory that is not the program's, such as the
/// captured output of a test and the report of its result.
pub fn outside<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_tag(self.0);
        }
    }
    let _restore = Restore(tag());
    set_tag(0);
    f()
}

/// Stops counting on this thread, for the exit after a refusal (C-18).
pub(crate) fn stop_counting() {
    set_tag(0);
}

/// The meter of one program: made by its runtime, it counts on the thread that made it until it
/// is dropped; then that thread counts into the meter it had before, if any.
pub struct Meter {
    slot: usize,
    generation: u32,
    previous: u64,
}

impl Meter {
    /// A meter with the limit, counting on this thread from now on.
    ///
    /// # Panics
    /// When more than 1,024 runtimes are alive at once in the process.
    pub fn start(limit: Option<usize>) -> Meter {
        // Taking a slot allocates nothing, so nothing is counted before the meter is set.
        let slot = (0..METERS)
            .find(|&i| {
                SLOTS[i]
                    .in_use
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            })
            .expect("at most 1,024 runtimes at once");
        let s = &SLOTS[slot];
        s.owner.store(0, Ordering::Relaxed);
        s.joined.store(0, Ordering::Relaxed);
        s.ballast.store(0, Ordering::Relaxed);
        s.limit
            .store(limit.unwrap_or(usize::MAX), Ordering::Relaxed);
        s.defer.store(false, Ordering::Relaxed);
        s.exhausted.store(false, Ordering::Relaxed);
        s.overdraft.store(0, Ordering::Relaxed);
        let generation = s.generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        let previous = tag();
        set_tag(slot as u64 + 1);
        Meter {
            slot,
            generation,
            previous,
        }
    }

    fn slot(&self) -> &'static Slot {
        &SLOTS[self.slot]
    }

    /// Changes the heap limit; `None` removes it. A limit below the count refuses the next
    /// allocation and makes the next check of the heap exhaustion.
    pub fn set_limit(&self, limit: Option<usize>) {
        self.slot()
            .limit
            .store(limit.unwrap_or(usize::MAX), Ordering::Relaxed);
    }

    /// The heap limit.
    pub fn limit(&self) -> Option<usize> {
        Some(self.slot().limit.load(Ordering::Relaxed)).filter(|&l| l != usize::MAX)
    }

    /// Defers a refusal instead of ending the process (the harness of `unio test`): see the
    /// module documentation.
    pub fn defer(&self) {
        self.slot().defer.store(true, Ordering::Relaxed);
    }

    /// The bytes counted on this meter.
    pub fn bytes(&self) -> usize {
        self.slot().total().max(0) as usize
    }

    /// What another thread needs to count into this meter.
    pub fn share(&self) -> Share {
        Share {
            tag: (self.slot as u64 + 1) | JOINED | (u64::from(self.generation) << 32),
        }
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        set_tag(self.previous);
        let s = self.slot();
        // A thread that joined the meter and outlives it stops counting: the generation moves.
        s.generation.fetch_add(1, Ordering::AcqRel);
        s.in_use.store(false, Ordering::Release);
    }
}

/// The meter of a program for a thread the runtime starts for it.
#[derive(Clone, Copy, Debug)]
pub struct Share {
    tag: u64,
}

impl Share {
    /// Counts the allocations of this thread into the meter from now on, until the program
    /// ends.
    pub fn join(self) {
        set_tag(self.tag);
    }
}

// ---------------------------------------------------------------- heap objects of values

/// What a heap object of a value does when it is dropped.
pub trait Contents {
    /// Called once when the object is dropped, before its contents are. An object that holds
    /// values hands them to [`crate::value::release`] here, so that a deep structure is freed
    /// by a loop instead of by recursion on the native stack (RFC 0007).
    fn release(&mut self) {}
}

/// A heap object of a value (the storage of a text, a list, a map, a set, bytes). Its memory is
/// counted by the allocator like every other block; the wrapper only frees its contents
/// without recursion ([`Contents::release`]).
#[derive(Clone)]
pub struct Counted<T: Contents> {
    value: T,
}

impl<T: Contents> Counted<T> {
    pub fn new(value: T) -> Counted<T> {
        Counted { value }
    }
}

impl<T: Contents> Drop for Counted<T> {
    fn drop(&mut self) {
        self.value.release();
    }
}

impl<T: Contents> Deref for Counted<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: Contents> DerefMut for Counted<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_is_counted_as_requested() {
        let meter = Meter::start(None);
        assert_eq!(meter.bytes(), 0);
        let a: Vec<u8> = Vec::with_capacity(1000);
        assert_eq!(meter.bytes(), 1000);
        let b: Box<[u64; 3]> = Box::new([1, 2, 3]);
        assert_eq!(meter.bytes(), 1024);
        let mut c: Vec<u32> = Vec::with_capacity(10);
        c.reserve_exact(90);
        assert_eq!(meter.bytes(), 1024 + 360);
        c.shrink_to(5);
        assert_eq!(meter.bytes(), 1024 + 20);
        drop((a, b, c));
        assert_eq!(meter.bytes(), 0);
        // Alignment rounds the size up as the layout asks.
        let layout = Layout::from_size_align(5, 64).unwrap();
        assert_eq!(requested(layout), 64);
    }

    #[test]
    fn the_test_hook_fills_the_heap_exactly() {
        let meter = Meter::start(None);
        fill_to(12_345);
        assert_eq!(live_bytes(), 12_345);
        let data: Vec<u8> = Vec::with_capacity(1000);
        fill_to(12_345);
        assert_eq!(live_bytes(), 12_345);
        drop(data);
        clear_ballast();
        assert_eq!(live_bytes(), 0);
        drop(meter);
        assert_eq!(live_bytes(), 0);
    }

    #[test]
    fn an_allocation_over_the_limit_is_refused() {
        let meter = Meter::start(Some(10_000));
        let small: Vec<u8> = Vec::with_capacity(4000);
        let mut more: Vec<u8> = Vec::new();
        assert!(crate::reserve::fallible(|| more.try_reserve_exact(7000)).is_err());
        assert_eq!(meter.bytes(), 4000, "a refused block is not counted");
        assert!(crate::reserve::fallible(|| more.try_reserve_exact(6000)).is_ok());
        assert_eq!(meter.bytes(), 10_000);
        drop((small, more));
        assert_eq!(meter.bytes(), 0);
    }

    #[test]
    fn a_deferred_refusal_is_granted_once_and_marks_exhaustion() {
        let meter = Meter::start(Some(10_000));
        meter.defer();
        let over: Vec<u8> = Vec::with_capacity(20_000);
        assert!(exhausted());
        assert_eq!(meter.bytes(), 20_000);
        // A fallible reservation still receives the refusal.
        let mut more: Vec<u8> = Vec::new();
        assert!(crate::reserve::fallible(|| more.try_reserve_exact(OVERDRAFT)).is_err());
        drop(over);
        assert!(exhausted(), "exhaustion stays until the program ends");
    }

    #[test]
    fn a_joined_thread_counts_into_the_program() {
        let meter = Meter::start(None);
        let share = meter.share();
        let block = std::thread::spawn(move || {
            share.join();
            vec![7u8; 50_000]
        })
        .join()
        .unwrap();
        // The thread's own bookkeeping is freed when it ends; the block stays counted.
        let counted = meter.bytes();
        assert!(counted >= 50_000, "{counted}");
        drop(block);
        assert!(meter.bytes() < counted - 49_000);
        let fresh = Meter::start(None);
        assert_eq!(fresh.bytes(), 0);
        drop(fresh);
    }

    #[test]
    fn outside_counts_nothing() {
        let meter = Meter::start(None);
        let block = outside(|| Vec::<u8>::with_capacity(5000));
        assert_eq!(meter.bytes(), 0);
        outside(|| drop(block));
        assert_eq!(meter.bytes(), 0);
    }

    /// docs/runtime.md 3.5: what the system allocator adds to the requested bytes of values,
    /// measured with glibc's own figures (`mallinfo2`: bytes in use, block headers and size
    /// rounding included) in a child process, so that no other test allocates meanwhile.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn the_overhead_of_the_allocator_is_measured() {
        use crate::value::Value;
        #[repr(C)]
        struct MallInfo2 {
            arena: usize,
            ordblks: usize,
            smblks: usize,
            hblks: usize,
            hblkhd: usize,
            usmblks: usize,
            fsmblks: usize,
            uordblks: usize,
            fordblks: usize,
            keepcost: usize,
        }
        extern "C" {
            fn mallinfo2() -> MallInfo2;
        }
        fn in_use() -> usize {
            // SAFETY: mallinfo2 returns a struct by value.
            let info = unsafe { mallinfo2() };
            info.uordblks + info.hblkhd
        }
        if std::env::var_os("UNIO_TEST_OVERHEAD").is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "heap::tests::the_overhead_of_the_allocator_is_measured",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("UNIO_TEST_OVERHEAD", "1")
                .output()
                .unwrap();
            let out = String::from_utf8_lossy(&output.stdout);
            print!("{out}");
            assert!(
                output.status.success(),
                "{out}{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let n = 100_000;
        type Make = fn(i64) -> Value;
        let cases: [(&str, Make); 7] = [
            ("one-byte Bytes", |_| Value::bytes(vec![1])),
            ("short texts", |i| Value::text(format!("w{i}"))),
            ("Int fields of a struct", |i| {
                Value::record("P", vec![("x", Value::Int(i)), ("y", Value::Int(i))])
            }),
            ("an enum with one field", |i| {
                Value::variant("S", "C", vec![("r", Value::Int(i))])
            }),
            ("lists of one Int", |i| Value::list(vec![Value::Int(i)])),
            ("texts of 100 characters", |_| Value::text("x".repeat(100))),
            ("Bytes of 64 KiB", |_| Value::bytes(vec![0; 64 * 1024])),
        ];
        let mut worst = 0.0f64;
        for (name, make) in cases {
            let count = if name.contains("64 KiB") { 200 } else { n };
            let meter = Meter::start(None);
            let (counted_before, used_before) = (meter.bytes(), in_use());
            let values: Vec<Value> = (0..count).map(make).collect();
            let counted = meter.bytes() - counted_before;
            let used = in_use() - used_before;
            let ratio = used as f64 / counted as f64;
            worst = worst.max(ratio);
            println!(
                "allocator overhead, {name}: {counted} bytes requested, {used} in use, {:+.1}%",
                100.0 * (ratio - 1.0)
            );
            drop(values);
            drop(meter);
        }
        assert!(worst <= 1.6, "the allocator adds {worst:.2}x");
    }
}
