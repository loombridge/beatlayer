//! Deferred drops: what the audio thread lets go of, the drop thread frees.
//!
//! Built on `rtgc` (decision D14). Everything the audio thread may drop (an
//! engine program with the nodes it no longer runs, a node's data) is made
//! off the audio thread in an `rtgc` wrapper registered with the engine's
//! [`Collector`]. Dropping the wrapper on the audio thread only decrements a
//! reference count, because the collector holds another reference. The drop
//! thread runs [`Collector::collect`] every few milliseconds, which frees
//! whatever only the collector still holds.
//!
//! `rtgc` notices a drop through a flag that collection clears. A drop that
//! races a collection can leave its object for the collection after the next
//! drop, or until the engine shuts down; it's never freed on the audio
//! thread.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rtgc::{ArcGc, LocalRtGc, LocalRtGcHandle, OwnedGc};

use crate::node::NodeData;
use crate::program::EngineProgram;

/// How often the drop thread collects.
pub const DROP_PERIOD: Duration = Duration::from_millis(10);

/// An engine's collector: it holds everything the audio thread may drop and
/// frees what it lets go of.
pub struct Collector {
    gc: LocalRtGc,
}

impl Collector {
    /// A collector holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gc: LocalRtGc::new(),
        }
    }

    /// A handle for wrapping values in this collector, from any thread.
    #[must_use]
    pub fn handle(&self) -> GcHandle {
        GcHandle(self.gc.handle())
    }

    /// Frees everything dropped since the last collection. Takes a lock, so
    /// never call it on the audio thread.
    pub fn collect(&mut self) {
        self.gc.collect();
    }

    /// How many values the collector holds: live ones and dropped ones not
    /// yet freed.
    #[must_use]
    pub fn held(&self) -> usize {
        self.gc.num_allocations()
    }

    /// Starts the drop thread, which collects every `period` until it's
    /// dropped.
    ///
    /// # Errors
    ///
    /// If the thread can't start.
    pub fn spawn(self, period: Duration) -> std::io::Result<DropThread> {
        let handle = self.handle();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            let mut collector = self;
            thread::Builder::new()
                .name("bl-engine-drop".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        collector.collect();
                        thread::park_timeout(period);
                    }
                    collector
                })?
        };
        Ok(DropThread {
            stop,
            thread: Some(thread),
            handle,
        })
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Collector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collector")
            .field("held", &self.held())
            .finish()
    }
}

/// The thread that frees what the audio thread drops. Dropping it stops the
/// thread after a last collection.
#[derive(Debug)]
pub struct DropThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Collector>>,
    handle: GcHandle,
}

impl DropThread {
    /// A handle for wrapping values in the thread's collector.
    #[must_use]
    pub fn handle(&self) -> GcHandle {
        self.handle.clone()
    }

    /// How many values the collector holds: live ones and dropped ones not
    /// yet freed.
    #[must_use]
    pub fn held(&self) -> usize {
        self.handle.held()
    }

    /// Asks for a collection now rather than at the next period.
    pub fn wake(&self) {
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

impl Drop for DropThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake();
        if let Some(thread) = self.thread.take()
            && let Ok(mut collector) = thread.join()
        {
            // Freeing a program can drop node data, which the next collection
            // frees; keep going while each pass frees something.
            let mut held = usize::MAX;
            while collector.held() < held {
                held = collector.held();
                collector.collect();
            }
        }
    }
}

/// A handle to a [`Collector`], for wrapping values the audio thread may
/// drop.
#[derive(Clone)]
pub struct GcHandle(LocalRtGcHandle);

impl GcHandle {
    /// Wraps data for a node. Allocates and takes the collector's lock, so
    /// call it off the audio thread.
    pub fn data<T: Any + Send + Sync>(&self, value: T) -> NodeData {
        NodeData(ArcGc::new_any_loc(value, &self.0))
    }

    /// Wraps a compiled program.
    pub(crate) fn program(&self, program: EngineProgram) -> Program {
        Program(OwnedGc::new_loc(program, &self.0))
    }

    /// How many values the collector holds.
    #[must_use]
    pub fn held(&self) -> usize {
        self.0.num_allocations()
    }
}

impl std::fmt::Debug for GcHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcHandle")
            .field("held", &self.held())
            .finish()
    }
}

/// A compiled program on its way to the audio thread, wrapped so that
/// dropping it there never frees anything.
pub struct Program(OwnedGc<EngineProgram, LocalRtGc>);

impl Program {
    /// The program.
    #[must_use]
    pub fn get(&self) -> &EngineProgram {
        self.0.get()
    }

    pub(crate) fn get_mut(&mut self) -> &mut EngineProgram {
        self.0.get_mut()
    }
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn data_dropped_in_a_real_time_section_is_freed_by_the_collector() {
        let mut collector = Collector::new();
        let freed = Arc::new(AtomicUsize::new(0));
        let data = collector.handle().data(Counted(Arc::clone(&freed)));
        assert!(data.downcast_ref::<Counted>().is_some());
        assert!(data.downcast_ref::<u32>().is_none());
        bl_rt::rt_test!({ drop(data) });
        assert_eq!(freed.load(Ordering::Relaxed), 0);
        assert_eq!(collector.held(), 1);
        collector.collect();
        assert_eq!(freed.load(Ordering::Relaxed), 1);
        assert_eq!(collector.held(), 0);
    }

    #[test]
    fn the_drop_thread_collects_until_it_stops() {
        let freed = Arc::new(AtomicUsize::new(0));
        let thread = Collector::new().spawn(Duration::from_millis(1)).unwrap();
        let data = thread.handle().data(Counted(Arc::clone(&freed)));
        assert_eq!(thread.held(), 1);
        drop(data);
        let began = std::time::Instant::now();
        while freed.load(Ordering::Relaxed) == 0 {
            assert!(began.elapsed() < Duration::from_secs(5), "never freed");
            thread.wake();
            std::thread::sleep(Duration::from_millis(1));
        }
        // Held but not dropped: freed when the thread stops.
        let kept = Arc::new(AtomicUsize::new(0));
        let data = thread.handle().data(Counted(Arc::clone(&kept)));
        drop(thread);
        assert_eq!(kept.load(Ordering::Relaxed), 0, "still referenced");
        drop(data);
        assert_eq!(kept.load(Ordering::Relaxed), 1);
    }
}
