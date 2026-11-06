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
}

/// The bytes that live values of this thread hold, as estimated by [`Measure`].
pub fn live_bytes() -> usize {
    LIVE_BYTES.with(Cell::get)
}

fn add(bytes: usize) {
    LIVE_BYTES.with(|live| live.set(live.get().saturating_add(bytes)));
}

fn sub(bytes: usize) {
    LIVE_BYTES.with(|live| live.set(live.get().saturating_sub(bytes)));
}

/// The size of the heap memory an object holds.
pub trait Measure {
    fn measure(&self) -> usize;
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
}
