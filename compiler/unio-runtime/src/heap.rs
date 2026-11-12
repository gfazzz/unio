//! Accounting of the memory that values hold (docs/runtime.md, section 3.3).
//!
//! Every heap object of a value (the storage of a text, a list, a map, a set, bytes) is wrapped
//! in [`Counted`], which adds its size to a per-thread total when it is created or cloned and
//! subtracts it when it is dropped. After a change that can grow an object, the owner calls
//! [`Counted::recount`]. [`crate::runtime::Runtime::check_heap`] compares the total with the
//! heap limit of the policy and turns exhaustion into code 102. The sizes are estimates of
//! capacity, not exact allocator figures; they are close enough to enforce a budget.

use std::cell::Cell;
use std::ops::{Deref, DerefMut};

thread_local! {
    static LIVE_BYTES: Cell<usize> = const { Cell::new(0) };
    static BALLAST: Cell<usize> = const { Cell::new(0) };
}

/// The bytes that live values of this thread hold, as estimated by [`Measure`].
pub fn live_bytes() -> usize {
    LIVE_BYTES.with(Cell::get)
}

pub(crate) fn add(bytes: usize) {
    LIVE_BYTES.with(|live| live.set(live.get().saturating_add(bytes)));
}

pub(crate) fn sub(bytes: usize) {
    LIVE_BYTES.with(|live| live.set(live.get().saturating_sub(bytes)));
}

/// Test hook, like `fail_point` of S-9b (plan section 12, condition 1): sets the count of this
/// thread to `count` bytes by adding a ballast that holds no memory, so that a test fills the
/// heap exactly instead of by estimate. A count below what live values hold removes the
/// ballast. The ballast only ever shrinks the budget of the program.
pub fn fill_to(count: usize) {
    let ballast = BALLAST.with(Cell::get);
    let without = live_bytes().saturating_sub(ballast);
    let wanted = count.saturating_sub(without);
    sub(ballast);
    add(wanted);
    BALLAST.with(|b| b.set(wanted));
}

/// Removes the ballast of [`fill_to`].
pub fn clear_ballast() {
    sub(BALLAST.with(|b| b.replace(0)));
}

/// The size of the heap memory an object holds.
pub trait Measure {
    fn measure(&self) -> usize;

    /// Called once when the object is dropped, before its contents are. An object that holds
    /// values hands them to [`crate::value::release`] here, so that a deep structure is freed
    /// by a loop instead of by recursion on the native stack (RFC 0007).
    fn release(&mut self) {}
}

/// A heap object whose size is counted.
pub struct Counted<T: Measure> {
    bytes: usize,
    value: T,
}

impl<T: Measure> Counted<T> {
    pub fn new(value: T) -> Counted<T> {
        let bytes = value.measure();
        add(bytes);
        Counted { bytes, value }
    }

    /// Updates the count after the object changed size.
    pub fn recount(&mut self) {
        let bytes = self.value.measure();
        if bytes > self.bytes {
            add(bytes - self.bytes);
        } else {
            sub(self.bytes - bytes);
        }
        self.bytes = bytes;
    }
}

impl<T: Measure + Clone> Clone for Counted<T> {
    fn clone(&self) -> Counted<T> {
        Counted::new(self.value.clone())
    }
}

impl<T: Measure> Drop for Counted<T> {
    fn drop(&mut self) {
        sub(self.bytes);
        self.value.release();
    }
}

impl<T: Measure> Deref for Counted<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: Measure> DerefMut for Counted<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_follow_creation_growth_and_drop() {
        let before = live_bytes();
        let mut data = Counted::new(Vec::<u8>::with_capacity(1000));
        assert!(live_bytes() >= before + 1000);
        data.extend(std::iter::repeat_n(0, 5000));
        data.recount();
        assert!(live_bytes() >= before + 5000);
        drop(data);
        assert_eq!(live_bytes(), before);
    }

    #[test]
    fn the_test_hook_fills_the_heap_exactly() {
        let before = live_bytes();
        fill_to(before + 12_345);
        assert_eq!(live_bytes(), before + 12_345);
        let data = Counted::new(Vec::<u8>::with_capacity(1000));
        fill_to(before + 12_345);
        assert_eq!(live_bytes(), before + 12_345);
        drop(data);
        clear_ballast();
        assert_eq!(live_bytes(), before);
    }
}
