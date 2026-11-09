//! The memory reserve for `release` (requirement C-17, docs/runtime.md 3.6).
//!
//! Before `main`, and before each test, the runtime sets aside [`RESERVE_BYTES`]: the memory is
//! allocated from the system, every page is written so that the system has delivered it, and
//! the size is counted in the heap ([`crate::heap`]). The reserve counts within the heap limit
//! of the policy, so user code may use the limit minus the reserve; a limit below
//! [`MINIMUM_HEAP_LIMIT`] is refused before `main` (code 103, [`crate::sandbox::parse_policy`]).
//!
//! The reserve opens only on a controlled fault (code 101): [`crate::fault::Fault::new`] opens
//! it before it allocates anything, so the message, the trace, and the unwinding with the
//! `release` calls of the open `using` blocks are served by it. While it is open, every
//! allocation of the thread that set it aside comes from the reserved pages: the global
//! allocator of this crate ([`ReserveAllocator`]) hands out blocks of the reserve instead of
//! asking the system. When the reserve has no room left, the allocation goes to the system and
//! the reserve is marked as overflowed; from then on the heap limit applies again, so that a
//! `release` that exhausts the reserve on a full heap ends the program with code 102 at the next
//! check of the heap, and the remaining releases are skipped (C-18).
//!
//! Exhaustion (code 102) never opens the reserve: no user code runs after it. `expect_fault`
//! closes the reserve again when it received the fault, so that a test goes on with the budget
//! it had (docs/runtime.md 4.3).
//!
//! The allocator inside the reserve is small and serves a short path: blocks up to 2 KiB come
//! in eight power-of-two classes with free lists, larger blocks are cut from the top and given
//! back to the top when they are the last ones, or kept in a first-fit list, and the whole
//! reserve is empty again when its last block is freed. [`peak`] reports the highest point the
//! path reached, which `the_reserve_peak_is_measured` records for docs/runtime.md 3.6.
//!
//! Outside an open reserve, the allocator costs one relaxed load of an atomic counter per
//! allocation and passes everything to the system allocator, which is what Rust uses by
//! default.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The size of the reserve: 1 MiB, the initial parameter of 0.1 (plan section 12, condition 1).
pub const RESERVE_BYTES: usize = 1 << 20;

/// The smallest heap limit a policy may set: the reserve and as much again for the program.
pub const MINIMUM_HEAP_LIMIT: usize = 2 * RESERVE_BYTES;

/// The bookkeeping at the start of the reserve: the lock, then the state.
const HEADER: usize = 256;
const STATE: usize = 64;
const PAGE: usize = 4096;
/// Blocks of 16, 32, ..., 2048 bytes have free lists of their size.
const CLASSES: usize = 8;
const LARGEST_SMALL: usize = 16 << (CLASSES - 1);
/// How many reserves can be open at once in a process (one per thread that runs a program).
const SLOTS: usize = 64;

/// How many reserves route allocations now; zero almost always.
static OPEN: AtomicUsize = AtomicUsize::new(0);
/// How many reserves have handed out blocks that may still be freed.
static REGISTERED: AtomicUsize = AtomicUsize::new(0);
/// The start of every registered reserve, so that a block freed on any thread finds its reserve.
static ARENAS: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
/// One past the highest slot ever used, so that a free looks at the slots in use only.
static HIGHEST: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// The reserve this thread set aside, by its address; 0 for none.
    static CURRENT: Cell<usize> = const { Cell::new(0) };
    /// The reserve that serves the allocations of this thread now; 0 while it is closed.
    static ROUTE: Cell<usize> = const { Cell::new(0) };
}

fn current() -> usize {
    CURRENT.try_with(Cell::get).unwrap_or(0)
}

fn route() -> usize {
    ROUTE.try_with(Cell::get).unwrap_or(0)
}

fn set_route(start: usize) {
    let _ = ROUTE.try_with(|r| r.set(start));
}

/// The state of one reserve, kept in its first bytes.
#[repr(C)]
struct State {
    open: bool,
    /// Whether this reserve routes allocations while open (it found a slot).
    routed: bool,
    overflowed: bool,
    /// The runtime ended while blocks were still alive; the last free returns the memory.
    orphaned: bool,
    slot: usize,
    /// Offset of the first byte never handed out since the reserve was last empty.
    top: usize,
    /// The highest `top` since [`reset_peak`].
    high: usize,
    blocks: usize,
    small: [usize; CLASSES],
    /// First free block of the large blocks: `[next, size]` at its start.
    large: usize,
}

const _: () = assert!(STATE + std::mem::size_of::<State>() <= HEADER);

fn layout() -> Layout {
    Layout::from_size_align(RESERVE_BYTES, PAGE).expect("the layout of the reserve")
}

/// The lock and the state of the reserve at `start`, held until dropped.
struct Locked {
    start: usize,
}

impl Locked {
    /// # Safety
    /// `start` is a reserve that is alive.
    unsafe fn new(start: usize) -> Locked {
        let lock = &*(start as *const AtomicBool);
        while lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        Locked { start }
    }

    fn state(&mut self) -> &mut State {
        // SAFETY: the state is only touched while the lock is held.
        unsafe { &mut *((self.start + STATE) as *mut State) }
    }

    fn end(&self) -> usize {
        self.start + RESERVE_BYTES
    }

    fn bump(&mut self, size: usize, align: usize) -> *mut u8 {
        let start = self.start;
        let end = self.end();
        let state = self.state();
        let at = (start + state.top + align - 1) & !(align - 1);
        if at + size > end {
            return std::ptr::null_mut();
        }
        state.top = (at + size - start + 15) & !15;
        state.high = state.high.max(state.top);
        state.blocks += 1;
        at as *mut u8
    }

    /// A block of the reserve, or null when it has no room.
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if layout.align() > 16 {
            return self.bump(layout.size().max(1), layout.align());
        }
        if let Some(class) = class_of(layout.size()) {
            let state = self.state();
            let head = state.small[class];
            if head != 0 {
                state.small[class] = *(head as *const usize);
                state.blocks += 1;
                return head as *mut u8;
            }
            return self.bump(16 << class, 16);
        }
        let size = large_size(layout.size());
        let state = self.state();
        let mut previous: *mut usize = &mut state.large;
        let mut node = state.large;
        while node != 0 {
            let next = *(node as *const usize);
            let room = *((node + 8) as *const usize);
            if room >= size {
                state.blocks += 1;
                if room - size >= 64 {
                    *((node + 8) as *mut usize) = room - size;
                    return (node + room - size) as *mut u8;
                }
                *previous = next;
                return node as *mut u8;
            }
            previous = node as *mut usize;
            node = next;
        }
        self.bump(size, 16)
    }

    /// Returns a block to the reserve; true when the reserve is now empty.
    unsafe fn dealloc(&mut self, p: *mut u8, layout: Layout) -> bool {
        let start = self.start;
        let state = self.state();
        state.blocks = state.blocks.saturating_sub(1);
        if state.blocks == 0 {
            state.top = HEADER;
            state.small = [0; CLASSES];
            state.large = 0;
            return true;
        }
        let p = p as usize;
        if layout.align() > 16 {
            return false;
        }
        if let Some(class) = class_of(layout.size()) {
            *(p as *mut usize) = state.small[class];
            state.small[class] = p;
            return false;
        }
        let size = large_size(layout.size());
        if p + size == start + state.top {
            state.top -= size;
            // Free large blocks that now end at the top go back to it as well.
            'absorb: loop {
                let mut previous: *mut usize = &mut state.large;
                let mut node = state.large;
                while node != 0 {
                    let next = *(node as *const usize);
                    let room = *((node + 8) as *const usize);
                    if node + room == start + state.top {
                        *previous = next;
                        state.top -= room;
                        continue 'absorb;
                    }
                    previous = node as *mut usize;
                    node = next;
                }
                break;
            }
        } else {
            *(p as *mut usize) = state.large;
            *((p + 8) as *mut usize) = size;
            state.large = p;
        }
        false
    }

    /// Grows or shrinks a block where it is, or returns null.
    fn resize_in_place(&mut self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() > 16 {
            return std::ptr::null_mut();
        }
        match (class_of(layout.size()), class_of(new_size)) {
            (Some(a), Some(b)) if a == b => return p,
            (None, None) => {}
            _ => return std::ptr::null_mut(),
        }
        let start = self.start;
        let end = self.end();
        let (old, new) = (large_size(layout.size()), large_size(new_size));
        let state = self.state();
        let at = p as usize;
        if at + old == start + state.top && at + new <= end {
            state.top = at + new - start;
            state.high = state.high.max(state.top);
            return p;
        }
        std::ptr::null_mut()
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: the reserve is alive while it is locked.
        unsafe { (*(self.start as *const AtomicBool)).store(false, Ordering::Release) }
    }
}

fn class_of(size: usize) -> Option<usize> {
    let n = size.max(16).next_power_of_two();
    (n <= LARGEST_SMALL).then(|| n.trailing_zeros() as usize - 4)
}

fn large_size(size: usize) -> usize {
    (size + 15) & !15
}

/// The registered reserve that holds `p`.
fn owner(p: *mut u8) -> Option<usize> {
    let p = p as usize;
    let highest = HIGHEST.load(Ordering::Acquire).min(SLOTS);
    ARENAS[..highest].iter().find_map(|slot| {
        let start = slot.load(Ordering::Acquire);
        (start != 0 && p >= start && p < start + RESERVE_BYTES).then_some(start)
    })
}

fn unregister(state: &mut State) {
    if state.slot < SLOTS {
        ARENAS[state.slot].store(0, Ordering::Release);
        REGISTERED.fetch_sub(1, Ordering::Release);
        state.slot = usize::MAX;
    }
}

/// A block of the reserve that serves this thread now, or null.
unsafe fn routed_alloc(layout: Layout) -> *mut u8 {
    let start = route();
    if start == 0 {
        return std::ptr::null_mut();
    }
    let mut locked = Locked::new(start);
    let p = locked.alloc(layout);
    if p.is_null() {
        locked.state().overflowed = true;
    }
    p
}

unsafe fn reserve_dealloc(start: usize, p: *mut u8, layout: Layout) {
    let mut locked = Locked::new(start);
    let empty = locked.dealloc(p, layout);
    let free = empty && locked.state().orphaned;
    if free {
        unregister(locked.state());
    }
    drop(locked);
    if free {
        System.dealloc(start as *mut u8, self::layout());
    }
}

/// The global allocator: the system allocator, except while this thread's reserve is open.
pub struct ReserveAllocator;

unsafe impl GlobalAlloc for ReserveAllocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if OPEN.load(Ordering::Relaxed) != 0 {
            let p = routed_alloc(layout);
            if !p.is_null() {
                return p;
            }
        }
        System.alloc(layout)
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if OPEN.load(Ordering::Relaxed) != 0 {
            let p = routed_alloc(layout);
            if !p.is_null() {
                std::ptr::write_bytes(p, 0, layout.size());
                return p;
            }
        }
        System.alloc_zeroed(layout)
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if REGISTERED.load(Ordering::Relaxed) != 0 {
            if let Some(start) = owner(p) {
                reserve_dealloc(start, p, layout);
                return;
            }
        }
        System.dealloc(p, layout)
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if OPEN.load(Ordering::Relaxed) == 0 && REGISTERED.load(Ordering::Relaxed) == 0 {
            return System.realloc(p, layout, new_size);
        }
        let owned = owner(p);
        match owned {
            Some(start) => {
                let q = Locked::new(start).resize_in_place(p, layout, new_size);
                if !q.is_null() {
                    return q;
                }
            }
            None if route() == 0 => return System.realloc(p, layout, new_size),
            None => {}
        }
        // Into the open reserve, or out of a reserve: a new block, then the copy.
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let q = self.alloc(new_layout);
        if !q.is_null() {
            std::ptr::copy_nonoverlapping(p, q, layout.size().min(new_size));
            self.dealloc(p, layout);
        }
        q
    }
}

// ---------------------------------------------------------------- the reserve of a runtime

/// The reserve of one runtime: set aside when the runtime is made, returned when it is dropped.
pub struct Reserve {
    start: usize,
}

impl Reserve {
    /// Sets aside the reserve for this thread: allocated, written page by page, counted in the
    /// heap. Without the memory (the system refused 1 MiB) there is no reserve, and a fault
    /// then runs its path on the heap like any other code.
    pub fn set_aside() -> Reserve {
        // SAFETY: the layout has a non-zero size.
        let p = unsafe { System.alloc(layout()) };
        if p.is_null() {
            return Reserve { start: 0 };
        }
        for offset in (0..RESERVE_BYTES).step_by(PAGE) {
            // SAFETY: inside the allocation; volatile, so that every page is really written.
            unsafe { std::ptr::write_volatile(p.add(offset), 0xA5) };
        }
        let start = p as usize;
        // SAFETY: the header lies inside the allocation and is aligned to its page.
        unsafe {
            std::ptr::write(start as *mut AtomicBool, AtomicBool::new(false));
            std::ptr::write(
                (start + STATE) as *mut State,
                State {
                    open: false,
                    routed: false,
                    overflowed: false,
                    orphaned: false,
                    slot: usize::MAX,
                    top: HEADER,
                    high: HEADER,
                    blocks: 0,
                    small: [0; CLASSES],
                    large: 0,
                },
            );
        }
        crate::heap::add(RESERVE_BYTES);
        let _ = CURRENT.try_with(|c| c.set(start));
        Reserve { start }
    }

    /// Whether the memory is set aside.
    pub fn is_set_aside(&self) -> bool {
        self.start != 0
    }

    /// The addresses of the reserved pages.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start..self.start + if self.start == 0 { 0 } else { RESERVE_BYTES }
    }

    /// Whether every page of the reserve was written (the byte written at its start is there).
    pub fn pages_are_written(&self) -> bool {
        self.start != 0
            && (HEADER.div_ceil(PAGE) * PAGE..RESERVE_BYTES)
                .step_by(PAGE)
                // SAFETY: inside the allocation; pages not yet handed out still hold the mark.
                .all(|offset| unsafe {
                    std::ptr::read_volatile((self.start + offset) as *const u8) == 0xA5
                })
    }
}

impl Drop for Reserve {
    fn drop(&mut self) {
        if self.start == 0 {
            return;
        }
        crate::heap::sub(RESERVE_BYTES);
        let start = self.start;
        let _ = CURRENT.try_with(|c| {
            if c.get() == start {
                c.set(0)
            }
        });
        // SAFETY: the reserve is alive until the end of this function.
        let mut locked = unsafe { Locked::new(start) };
        close_locked(&mut locked);
        let state = locked.state();
        if state.slot < SLOTS && state.blocks > 0 {
            // A block outlives the runtime (a buffer of the standard library made while the
            // reserve was open): the reserve stays registered and its last free returns it.
            state.orphaned = true;
            return;
        }
        unregister(state);
        drop(locked);
        // SAFETY: nothing points into the reserve any more.
        unsafe { System.dealloc(start as *mut u8, layout()) };
    }
}

fn close_locked(locked: &mut Locked) {
    let start = locked.start;
    let state = locked.state();
    if !state.open {
        return;
    }
    state.open = false;
    state.overflowed = false;
    if state.routed {
        state.routed = false;
        if route() == start {
            set_route(0);
        }
        OPEN.fetch_sub(1, Ordering::Release);
    }
}

/// Runs `f` on the reserve of this thread while it is locked.
fn with_current<R>(f: impl FnOnce(&mut Locked) -> R) -> Option<R> {
    let start = current();
    if start == 0 {
        return None;
    }
    // SAFETY: the current reserve is alive: its runtime clears it when dropped.
    let mut locked = unsafe { Locked::new(start) };
    Some(f(&mut locked))
}

/// Opens the reserve of this thread for the path of a fault: from now on its allocations come
/// from the reserved pages. Nothing happens when it is open already or there is none.
pub fn open() {
    let start = current();
    if start == 0 || route() == start {
        return;
    }
    with_current(|locked| {
        let state = locked.state();
        if state.open {
            return;
        }
        state.open = true;
        if state.slot >= SLOTS {
            // Registered before the first block is handed out, so that every free finds it.
            match ARENAS.iter().position(|slot| {
                slot.compare_exchange(0, start, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            }) {
                Some(slot) => {
                    HIGHEST.fetch_max(slot + 1, Ordering::AcqRel);
                    REGISTERED.fetch_add(1, Ordering::AcqRel);
                    state.slot = slot;
                }
                // More open reserves than slots: this one only changes the accounting.
                None => return,
            }
        }
        state.routed = true;
        OPEN.fetch_add(1, Ordering::AcqRel);
        set_route(start);
    });
}

/// Closes the reserve of this thread after `expect_fault` received its fault. The blocks
/// still alive stay in the reserve until they are freed.
pub fn close() {
    with_current(close_locked);
}

/// Whether the reserve of this thread is open.
pub fn is_open() -> bool {
    with_current(|l| l.state().open).unwrap_or(false)
}

/// Whether the reserve of this thread is open, has not overflowed, and has `bytes` free at its
/// top: then the path of a fault may go on although the heap is full.
pub fn covers(bytes: usize) -> bool {
    with_current(|l| {
        let end = l.end();
        let start = l.start;
        let state = l.state();
        state.open && !state.overflowed && start + state.top + bytes <= end
    })
    .unwrap_or(false)
}

/// Whether an allocation found no room in the open reserve of this thread.
pub fn overflowed() -> bool {
    with_current(|l| l.state().overflowed).unwrap_or(false)
}

/// The most bytes of the reserve of this thread in use at once since [`reset_peak`], counted
/// from the first byte after the bookkeeping to the highest point blocks reached.
pub fn peak() -> usize {
    with_current(|l| l.state().high - HEADER).unwrap_or(0)
}

/// Starts a new measurement of [`peak`].
pub fn reset_peak() {
    with_current(|l| {
        let state = l.state();
        state.high = state.top;
    });
}

/// The bytes of the reserve the path of a fault can use.
pub const USABLE_BYTES: usize = RESERVE_BYTES - HEADER;

/// Whether `p` lies in the reserved pages of this thread.
pub fn contains<T: ?Sized>(p: *const T) -> bool {
    let start = current();
    let p = p as *const u8 as usize;
    start != 0 && p >= start && p < start + RESERVE_BYTES
}

/// Runs `f` with the allocations of this thread going to the system even while the reserve is
/// open: for memory that is not the program's, such as the captured output of a test, which
/// the heap does not count either.
pub fn outside<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_route(self.0);
        }
    }
    let _restore = Restore(route());
    set_route(0);
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_and_sizes() {
        assert_eq!(class_of(1), Some(0));
        assert_eq!(class_of(16), Some(0));
        assert_eq!(class_of(17), Some(1));
        assert_eq!(class_of(2048), Some(7));
        assert_eq!(class_of(2049), None);
        assert_eq!(large_size(2049), 2064);
    }

    #[test]
    fn blocks_are_reused_and_the_reserve_empties() {
        let reserve = Reserve::set_aside();
        open();
        let a: Vec<u8> = Vec::with_capacity(100);
        let b: Vec<u8> = Vec::with_capacity(10_000);
        assert!(contains(a.as_ptr()) && contains(b.as_ptr()));
        let first = b.as_ptr() as usize;
        drop(b);
        let c: Vec<u8> = Vec::with_capacity(10_000);
        assert_eq!(c.as_ptr() as usize, first, "the top is given back");
        drop((a, c));
        assert!(peak() >= 10_000);
        let mut grown: Vec<u64> = Vec::new();
        for i in 0..50_000 {
            grown.push(i);
        }
        assert!(contains(grown.as_ptr()));
        let too_big: Vec<u8> = Vec::with_capacity(2 * RESERVE_BYTES);
        assert!(!contains(too_big.as_ptr()));
        assert!(overflowed());
        drop((grown, too_big));
        close();
        let after: Vec<u8> = Vec::with_capacity(100);
        assert!(!contains(after.as_ptr()));
        drop(reserve);
    }
}
