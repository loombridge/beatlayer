//! Storage that several threads write at once, each to its own part.
//!
//! The multicore scheduler runs a program's tasks on a pool of threads
//! (`docs/02-architecture.md`, "Scheduling and multicore"). Every task owns
//! its parts of the program: its nodes, its buffers, its delay lines. A
//! [`Shared`] list holds such parts, and hands out a part to whichever
//! thread runs the task that owns it. This module is the only place the
//! engine hands out mutable references to shared storage; every other
//! `unsafe` block in the scheduler rests on the contract of the two
//! methods here, [`Shared::element`] and [`Shared::elements`].
//!
//! Outside a run, a `Shared` list is an ordinary slice: it dereferences to
//! one, and mutably so through `&mut`.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut, Range};

/// A list whose elements different threads may write at the same time, as
/// long as no two write the same element.
pub(crate) struct Shared<T> {
    cells: Box<[UnsafeCell<T>]>,
}

// SAFETY: the list owns its elements, so sending it sends them. Sharing it
// lets other threads reach elements only through `element` and `elements`, whose
// callers promise that no element is reached by two threads at once, and
// through `Deref`, which no thread uses during a run; an element that is
// `Send` may be handed from the thread that made it to the one that writes
// it.
unsafe impl<T: Send> Sync for Shared<T> {}

impl<T> Shared<T> {
    /// A list of `items`.
    pub(crate) fn new(items: Vec<T>) -> Self {
        let cells: Vec<UnsafeCell<T>> = items.into_iter().map(UnsafeCell::new).collect();
        Self {
            cells: cells.into_boxed_slice(),
        }
    }

    /// Element `index`, mutably, through a shared reference to the list.
    ///
    /// # Safety
    ///
    /// For as long as the returned reference lives, no other reference to
    /// the element (from this method, [`elements`](Self::elements), or `Deref`)
    /// may be used, on this thread or another. The caller is the thread
    /// running the task that owns the element. The list outlives `'a`.
    ///
    /// # Panics
    ///
    /// If `index` is out of bounds.
    pub(crate) unsafe fn element<'a>(&self, index: usize) -> &'a mut T {
        // SAFETY: the cell is in bounds (indexing checked it), and the
        // caller promises nobody else reaches the element meanwhile.
        unsafe { &mut *self.cells[index].get() }
    }

    /// The elements in `range`, shared, through a shared reference to the
    /// list.
    ///
    /// # Safety
    ///
    /// For as long as the returned reference lives, no thread may write
    /// any element of `range` (through [`element`](Self::element) or
    /// [`elements`](Self::elements)). Other threads may read them at once.
    ///
    /// # Panics
    ///
    /// If `range` is out of bounds.
    pub(crate) unsafe fn view(&self, range: Range<usize>) -> &[T] {
        let cells = &self.cells[range];
        let start = cells.as_ptr().cast::<T>();
        // SAFETY: the range is in bounds, and the caller promises that no
        // thread writes these elements meanwhile.
        unsafe { core::slice::from_raw_parts(start, cells.len()) }
    }

    /// The elements in `range`, mutably, through a shared reference to the
    /// list.
    ///
    /// # Safety
    ///
    /// As for [`element`](Self::element), for every element of `range`, and
    /// the list outlives `'a`.
    ///
    /// # Panics
    ///
    /// If `range` is out of bounds.
    pub(crate) unsafe fn elements<'a>(&self, range: Range<usize>) -> &'a mut [T] {
        let cells = &self.cells[range];
        // `UnsafeCell<T>` has the layout of `T`, and the cells are
        // contiguous.
        let start = cells.as_ptr().cast::<T>().cast_mut();
        // SAFETY: the range is in bounds (slicing checked it), and the
        // caller promises nobody else reaches these elements meanwhile.
        unsafe { core::slice::from_raw_parts_mut(start, cells.len()) }
    }
}

impl<T> Default for Shared<T> {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl<T> From<Vec<T>> for Shared<T> {
    fn from(items: Vec<T>) -> Self {
        Self::new(items)
    }
}

impl<T> Deref for Shared<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        // SAFETY: the layout is `T`'s (above), and the contract of `get`
        // and `range` forbids mutable references while this one is used.
        unsafe { core::slice::from_raw_parts(self.cells.as_ptr().cast::<T>(), self.cells.len()) }
    }
}

impl<T> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: as for `Deref`, and `&mut self` makes this the only
        // reference.
        unsafe {
            core::slice::from_raw_parts_mut(self.cells.as_mut_ptr().cast::<T>(), self.cells.len())
        }
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Barrier;

    #[test]
    fn it_reads_and_writes_like_a_slice_outside_a_run() {
        let mut list = Shared::new(vec![1, 2, 3]);
        assert_eq!(list.len(), 3);
        list[1] = 20;
        list.iter_mut().for_each(|x| *x += 1);
        assert_eq!(&*list, &[2, 21, 4]);
    }

    #[test]
    fn threads_write_their_own_elements_at_once() {
        const THREADS: usize = 4;
        const EACH: usize = 1_000;
        let list = Shared::new(vec![0u64; THREADS * EACH]);
        let barrier = Barrier::new(THREADS);
        std::thread::scope(|scope| {
            for t in 0..THREADS {
                let (list, barrier) = (&list, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    // SAFETY: thread `t` is the only one that touches
                    // elements `t * EACH..(t + 1) * EACH`.
                    let mine = unsafe { list.elements(t * EACH..(t + 1) * EACH) };
                    for (i, x) in mine.iter_mut().enumerate() {
                        *x = (t * EACH + i) as u64;
                    }
                    // SAFETY: as above, for one element of the same part.
                    unsafe { *list.element(t * EACH) += 1 };
                });
            }
        });
        for (i, &x) in list.iter().enumerate() {
            let first = i % EACH == 0;
            assert_eq!(x, i as u64 + u64::from(first));
        }
    }

    #[test]
    fn a_range_is_as_long_as_asked_and_may_be_empty() {
        let list = Shared::new(vec![1.0f32; 8]);
        // SAFETY: nothing else touches the list.
        unsafe {
            assert_eq!(list.elements(2..5).len(), 3);
            assert!(list.elements(8..8).is_empty());
        }
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn a_range_out_of_bounds_panics_rather_than_reaching_past_the_list() {
        let list = Shared::new(vec![0u8; 4]);
        // SAFETY: nothing else touches the list.
        let _ = unsafe { list.elements(2..9) };
    }
}
