//! A global allocator that counts, so a test can say how many times a stretch
//! of code allocated. Counting is off until [`start`] or [`watch`] turns it on. Counters are
//! global rather than per thread: macOS allocates a thread's thread-local
//! storage with `malloc` on first touch, which would be counted (and reported
//! by RealtimeSanitizer) as the audio thread's own allocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

static WATCHING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static FREES: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

/// Install with `#[global_allocator] static A: Counting = Counting;`.
pub struct Counting;

// SAFETY: every method forwards to `System`, which upholds `GlobalAlloc`'s
// contract; the counters only add atomic increments.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if WATCHING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        // SAFETY: the caller's layout is passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if WATCHING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        // SAFETY: the caller's layout is passed on unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if WATCHING.load(Relaxed) {
            FREES.fetch_add(1, Relaxed);
        }
        // SAFETY: `ptr` and `layout` come from the matching `alloc`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if WATCHING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(new_size as u64, Relaxed);
        }
        // SAFETY: `ptr` and `layout` come from the matching `alloc`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// What the allocator saw while [`watch`] ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub allocs: u64,
    pub frees: u64,
    pub bytes: u64,
}

/// Starts counting, in every thread of the process, from zero.
pub fn start() {
    ALLOCS.store(0, Relaxed);
    FREES.store(0, Relaxed);
    BYTES.store(0, Relaxed);
    WATCHING.store(true, Relaxed);
}

/// Stops counting and says what was counted since [`start`].
pub fn stop() -> Counts {
    WATCHING.store(false, Relaxed);
    Counts {
        allocs: ALLOCS.load(Relaxed),
        frees: FREES.load(Relaxed),
        bytes: BYTES.load(Relaxed),
    }
}

/// Runs `f` with counting on and returns its result with what was counted.
/// Not reentrant, and not for use from two threads at once.
pub fn watch<R>(f: impl FnOnce() -> R) -> (R, Counts) {
    start();
    let result = f();
    (result, stop())
}
