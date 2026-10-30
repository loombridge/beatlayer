//! The multicore scheduler (WP-5.37): a pool of real-time workers that run a
//! program's independent tasks, and the chains of a rack, at the same time.
//!
//! The audio thread and the workers take ready work from one lock-free queue
//! ([`Queue`]). A task is ready when every task feeding it has finished;
//! the thread that finishes the last of them makes it ready. A rack's owner
//! offers the rack's chains to the same threads as tickets in the same queue,
//! and joins them before it sums them in the chains' order, so what a run
//! writes doesn't depend on which thread ran what.
//!
//! The audio thread never sleeps: it takes work and spins until the last
//! task finishes, so a pool whose workers are all asleep, or gone, only
//! costs the speed-up. Workers spin briefly when they find nothing to take,
//! then sleep until a thread that makes work, or the start of the next
//! block, wakes them (`park` and `unpark`, a futex or semaphore, never a
//! mutex). Between blocks the audio thread waits for the workers to leave
//! the program: a program is freed or swapped only when none of them holds
//! it.

use core::hint::spin_loop;
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release, SeqCst};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bl_rt::{RtThreadConfig, ThreadReport, Workgroup};

use crate::EngineConfig;
use crate::node::NodeEvent;
use crate::program::{Blk, EngineProgram};

/// The most workers a pool starts.
const MAX_WORKERS: usize = 15;

/// How many times a worker looks for work before it sleeps, in a block and
/// between blocks. About the time a task of a few tracks takes.
const IDLE_SPINS: u32 = 2_000;
const BLOCK_SPINS: u32 = 4_000;

/// How long [`Pool::start`] waits for a worker to set itself up.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// What a thread keeps to itself while it runs tasks, because tasks used to
/// share it when the audio thread ran them one at a time: the changes handed
/// to one node, and the scratch a delayed route or an interface input is
/// mapped through. Made with the thread, where allocating is allowed.
pub(crate) struct Lane {
    /// One node's changes, handed to it.
    pub(crate) events: Vec<NodeEvent>,
    /// A delayed route's input: `channels` channels of `max_frames` frames.
    pub(crate) delay: Vec<f32>,
    /// One task's device input mapped to the tracks' stereo layout, and
    /// after it the input channels a CV device reads, one buffer each of the
    /// same size (`docs/specs/cv-tools.md`, R6).
    pub(crate) device: Vec<f32>,
}

impl Lane {
    pub(crate) fn new(config: &EngineConfig) -> Self {
        let size = config.channels * config.max_frames;
        Self {
            events: Vec::with_capacity(config.max_pending_events),
            delay: vec![0.0; size],
            device: vec![0.0; size * (1 + bl_model::MAX_CV_PORTS)],
        }
    }
}

/// A bounded queue of work that any number of threads push to and pop from
/// without locks (Vyukov's array queue). Work is a task's number, or a ticket
/// for one of a rack's chains.
pub(crate) struct Queue {
    slots: Box<[QueueSlot]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

struct QueueSlot {
    /// Where in the queue's laps the slot is: `position` when it is free for
    /// the push at that position, `position + 1` once that push has filled it.
    sequence: AtomicUsize,
    value: AtomicU32,
}

impl Queue {
    /// A queue that holds at least `capacity` items.
    pub(crate) fn new(capacity: usize) -> Self {
        let len = capacity.max(2).next_power_of_two();
        Self {
            slots: (0..len)
                .map(|i| QueueSlot {
                    sequence: AtomicUsize::new(i),
                    value: AtomicU32::new(0),
                })
                .collect(),
            mask: len - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Adds `value`. Returns `false`, and adds nothing, if the queue is full.
    pub(crate) fn push(&self, value: u32) -> bool {
        let mut position = self.tail.load(Relaxed);
        loop {
            let slot = &self.slots[position & self.mask];
            let ahead = slot.sequence.load(Acquire).wrapping_sub(position) as isize;
            if ahead == 0 {
                match self.tail.compare_exchange_weak(
                    position,
                    position.wrapping_add(1),
                    SeqCst,
                    Relaxed,
                ) {
                    Ok(_) => {
                        slot.value.store(value, Relaxed);
                        slot.sequence.store(position.wrapping_add(1), Release);
                        return true;
                    }
                    Err(now) => position = now,
                }
            } else if ahead < 0 {
                return false;
            } else {
                position = self.tail.load(Relaxed);
            }
        }
    }

    /// Takes the oldest item, if there is one.
    pub(crate) fn pop(&self) -> Option<u32> {
        let mut position = self.head.load(Relaxed);
        loop {
            let slot = &self.slots[position & self.mask];
            let ahead = slot
                .sequence
                .load(Acquire)
                .wrapping_sub(position.wrapping_add(1)) as isize;
            if ahead == 0 {
                match self.head.compare_exchange_weak(
                    position,
                    position.wrapping_add(1),
                    Relaxed,
                    Relaxed,
                ) {
                    Ok(_) => {
                        let value = slot.value.load(Relaxed);
                        slot.sequence
                            .store(position.wrapping_add(self.mask + 1), Release);
                        return Some(value);
                    }
                    Err(now) => position = now,
                }
            } else if ahead < 0 {
                return None;
            } else {
                position = self.head.load(Relaxed);
            }
        }
    }

    /// Whether nothing waits in the queue.
    fn is_empty(&self) -> bool {
        self.head.load(SeqCst) == self.tail.load(SeqCst)
    }
}

/// The tickets of one rack in a run: how many of its chains threads have
/// taken, and how many have finished.
#[derive(Debug, Default)]
pub(crate) struct Fork {
    next: AtomicU32,
    done: AtomicU32,
}

impl Fork {
    /// Starts the rack's run, before its tickets are offered.
    pub(crate) fn open(&self) {
        self.next.store(0, Relaxed);
        self.done.store(0, Relaxed);
    }

    /// Takes the next chain of `count`, if any is left.
    pub(crate) fn claim(&self, count: usize) -> Option<usize> {
        let chain = self.next.fetch_add(1, AcqRel) as usize;
        (chain < count).then_some(chain)
    }

    /// Marks a taken chain finished.
    pub(crate) fn finish(&self) {
        self.done.fetch_add(1, Release);
    }

    /// Waits until all `count` chains have finished. The chains it waits for
    /// are running on threads that took them, so the wait is short.
    pub(crate) fn join(&self, count: usize) {
        while (self.done.load(Acquire) as usize) < count {
            spin_loop();
        }
    }
}

/// A program's counters and queue for a run through a pool.
pub(crate) struct Sched {
    /// How many tasks each task still waits for.
    pub(crate) waiting: Vec<AtomicU32>,
    /// The tasks not yet finished.
    pub(crate) remaining: AtomicU32,
    /// The changes that reached a node or a route.
    pub(crate) delivered: AtomicU64,
    pub(crate) queue: Queue,
    /// One per rack.
    pub(crate) forks: Vec<Fork>,
}

/// A queue item that names a rack's chain rather than a task.
const TICKET: u32 = 1 << 31;

impl Sched {
    /// For `tasks` tasks and `racks` racks with `chains` chains in all.
    pub(crate) fn new(tasks: usize, racks: usize, chains: usize) -> Self {
        Self {
            waiting: (0..tasks).map(|_| AtomicU32::new(0)).collect(),
            remaining: AtomicU32::new(0),
            delivered: AtomicU64::new(0),
            // Every task is queued at most once in a run, and every chain
            // after the first of its rack once.
            queue: Queue::new(tasks + chains + 1),
            forks: (0..racks).map(|_| Fork::default()).collect(),
        }
    }

    /// The queue item that offers a chain of `rack`.
    pub(crate) fn ticket(rack: usize) -> u32 {
        TICKET | rack as u32
    }
}

/// What a worker is doing about sleeping.
const AWAKE: u8 = 0;
/// Asleep until the next block starts.
const BETWEEN: u8 = 1;
/// Asleep until work appears in this block.
const WORK: u8 = 2;

/// The workers' beds: who is asleep, why, and how to wake them.
pub(crate) struct Wake {
    sleepers: Box<[Sleeper]>,
}

struct Sleeper {
    state: AtomicU8,
    bed: bed::Bed,
}

impl Sleeper {
    /// Wakes the sleeper if it sleeps for `reason`.
    fn wake_for(&self, reason: u8) -> bool {
        if self
            .state
            .compare_exchange(reason, AWAKE, SeqCst, SeqCst)
            .is_ok()
        {
            self.unpark();
            true
        } else {
            false
        }
    }

    fn unpark(&self) {
        self.bed.post();
    }
}

impl Wake {
    fn new(workers: usize) -> Self {
        Self {
            sleepers: (0..workers)
                .map(|_| Sleeper {
                    state: AtomicU8::new(AWAKE),
                    bed: bed::Bed::new(),
                })
                .collect(),
        }
    }

    /// Wakes one worker that waits for work in the block, if there is one:
    /// work has just been made.
    pub(crate) fn one(&self) {
        for sleeper in &self.sleepers {
            if sleeper.wake_for(WORK) {
                return;
            }
        }
    }

    fn all_for(&self, reason: u8) {
        for sleeper in &self.sleepers {
            sleeper.wake_for(reason);
        }
    }

    /// Puts worker `me` to sleep for `reason` unless `still_waiting` says
    /// the reason is gone. A wake that comes first leaves its post on the
    /// bed, so the wait returns at once.
    fn sleep(&self, me: usize, reason: u8, still_waiting: impl Fn() -> bool) {
        let sleeper = &self.sleepers[me];
        sleeper.state.store(reason, SeqCst);
        if still_waiting() {
            sleeper.bed.wait();
        }
        sleeper.state.store(AWAKE, SeqCst);
    }
}

/// What a sleeping worker waits on and the threads that make work post to:
/// a semaphore. A POSIX `sem_t` where there is one that a real-time thread
/// may post, so that RealtimeSanitizer, which stops a raw futex call but not
/// the library's semaphore, lets the audio thread wake a worker. Apple's and
/// Windows' thread parking is a semaphore (`dispatch_semaphore`) and a
/// wait-on-address already, and is used there.
#[cfg(all(unix, not(target_vendor = "apple")))]
mod bed {
    use core::cell::UnsafeCell;
    use core::mem::MaybeUninit;
    use std::thread::Thread;

    pub(super) struct Bed {
        semaphore: Box<UnsafeCell<MaybeUninit<libc::sem_t>>>,
    }

    // SAFETY: a `sem_t` is made to be waited on and posted from any thread,
    // and the box keeps it where it was initialised.
    unsafe impl Send for Bed {}
    // SAFETY: as above.
    unsafe impl Sync for Bed {}

    impl Bed {
        pub(super) fn new() -> Self {
            let semaphore = Box::new(UnsafeCell::new(MaybeUninit::uninit()));
            // SAFETY: the box holds room for a `sem_t`, which this
            // initialises, not shared between processes, at zero.
            let made = unsafe { libc::sem_init(semaphore.get().cast(), 0, 0) };
            assert_eq!(made, 0, "a semaphore");
            Self { semaphore }
        }

        /// The thread that sleeps on the bed: a semaphore needs no handle.
        pub(super) fn bind(&self, _thread: &Thread) {}

        /// Wakes a sleeper, or lets the next sleep end at once.
        pub(super) fn post(&self) {
            // SAFETY: the semaphore was initialised and lives as long as the
            // bed. A post that would overflow its count fails; there is
            // nothing to do about it.
            unsafe { libc::sem_post(self.semaphore.get().cast()) };
        }

        /// Sleeps until a post. May return without one (a signal): callers
        /// check what they waited for.
        pub(super) fn wait(&self) {
            // SAFETY: as for `post`.
            unsafe { libc::sem_wait(self.semaphore.get().cast()) };
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            // SAFETY: nothing waits on the semaphore once its owner is gone.
            unsafe { libc::sem_destroy(self.semaphore.get().cast()) };
        }
    }
}

#[cfg(not(all(unix, not(target_vendor = "apple"))))]
mod bed {
    use std::sync::OnceLock;
    use std::thread::{self, Thread};

    pub(super) struct Bed {
        thread: OnceLock<Thread>,
    }

    impl Bed {
        pub(super) fn new() -> Self {
            Self {
                thread: OnceLock::new(),
            }
        }

        /// The thread that sleeps on the bed.
        pub(super) fn bind(&self, thread: &Thread) {
            let _ = self.thread.set(thread.clone());
        }

        /// Wakes a sleeper, or lets the next sleep end at once.
        pub(super) fn post(&self) {
            if let Some(thread) = self.thread.get() {
                thread.unpark();
            }
        }

        /// Sleeps until a post. May return without one: callers check what
        /// they waited for.
        pub(super) fn wait(&self) {
            thread::park();
        }
    }
}

/// Where a pool's workers find the block being run. It lives on the audio
/// thread's stack for the block, and the pool keeps the workers from holding
/// it longer.
struct Job {
    program: *const EngineProgram,
    block: *const Blk<'static>,
}

/// What the workers and the audio thread share.
struct Shared {
    wake: Wake,
    /// Counts the blocks run through the pool.
    epoch: AtomicU64,
    /// The block being run, or null.
    job: AtomicPtr<Job>,
    /// The workers inside a block's work.
    active: AtomicUsize,
    shutdown: AtomicBool,
    /// Set when a block's run ends early, for the workers to leave it.
    abort: AtomicBool,
    heap_operations: AtomicU64,
    panics: AtomicU64,
}

/// What a pool of workers is made from.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// How many workers to start, besides the audio thread.
    pub workers: usize,
    /// How long a block lasts, for the workers' scheduling policy (macOS).
    pub period: Duration,
    /// Whether to raise the workers to real-time priority.
    pub promote: bool,
    /// The audio workgroup the workers join (macOS): the output stream's.
    pub workgroup: Option<Workgroup>,
}

impl PoolConfig {
    /// The pool for a stream whose blocks last `period`: as many workers as
    /// its workgroup allows besides the audio thread, or, without one, the
    /// cores the machine has less one, raised to real-time priority if
    /// `promote` (a stream on a real device, not a test's pretend one). (The platforms that have no
    /// workgroups don't say which cores are the fast ones; a machine with
    /// slow cores gets a worker on each.)
    #[must_use]
    pub fn for_stream(period: Duration, workgroup: Option<Workgroup>, promote: bool) -> Self {
        let threads = workgroup
            .as_ref()
            .and_then(Workgroup::max_parallel_threads)
            .map(|n| n as usize)
            .or_else(|| thread::available_parallelism().ok().map(Into::into))
            .unwrap_or(1);
        Self {
            workers: threads.saturating_sub(1).min(MAX_WORKERS),
            period,
            promote,
            workgroup,
        }
    }

    /// `workers` workers at the priority their threads start with, in no
    /// workgroup: for renders and tests.
    #[must_use]
    pub fn plain(workers: usize) -> Self {
        Self {
            workers: workers.min(MAX_WORKERS),
            period: Duration::from_millis(3),
            promote: false,
            workgroup: None,
        }
    }
}

/// A pool of real-time workers that run a program's tasks beside the audio
/// thread (WP-5.37; the architecture, "Scheduling and multicore").
///
/// It is made where allocating and waiting are allowed, with the stream or
/// the offline engine, and given to the [`Processor`](crate::Processor).
/// Each worker is a thread prepared with `bl-rt` before its first block
/// (real-time priority, the output stream's workgroup on macOS), which
/// leaves the workgroup again when the pool is dropped. A run through a pool
/// gives the same output, to the bit, as a run without one.
pub struct Pool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    reports: Vec<ThreadReport>,
}

impl Pool {
    /// Starts the workers, each set up by `bl-rt` and ready for its first
    /// block when this returns, with work areas made for `config`.
    ///
    /// # Errors
    ///
    /// If a thread can't start or doesn't report in time.
    pub fn start(config: &EngineConfig, pool: &PoolConfig) -> std::io::Result<Self> {
        let workers = pool.workers.min(MAX_WORKERS);
        let shared = Arc::new(Shared {
            wake: Wake::new(workers),
            epoch: AtomicU64::new(0),
            job: AtomicPtr::new(core::ptr::null_mut()),
            active: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            abort: AtomicBool::new(false),
            heap_operations: AtomicU64::new(0),
            panics: AtomicU64::new(0),
        });
        let mut started = Self {
            shared: Arc::clone(&shared),
            threads: Vec::with_capacity(workers),
            reports: Vec::with_capacity(workers),
        };
        for index in 0..workers {
            let (ready, report) = mpsc::channel();
            let rt = RtThreadConfig {
                period: pool.period,
                promote: pool.promote,
                workgroup: pool.workgroup.clone(),
            };
            let (own, config) = (Arc::clone(&shared), *config);
            let handle = thread::Builder::new()
                .name(format!("bl-engine worker {}", index + 1))
                .spawn(move || worker(index, &own, &config, &rt, &ready))?;
            shared.wake.sleepers[index].bed.bind(handle.thread());
            started.threads.push(handle);
            let report = report.recv_timeout(SETUP_TIMEOUT).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "a worker didn't finish its real-time set-up",
                )
            })?;
            started.reports.push(report);
        }
        Ok(started)
    }

    /// How many workers the pool has, besides the audio thread.
    #[must_use]
    pub fn workers(&self) -> usize {
        self.threads.len()
    }

    /// What each worker's set-up did, in the order they started.
    #[must_use]
    pub fn reports(&self) -> &[ThreadReport] {
        &self.reports
    }

    /// Heap operations the workers made inside their blocks, so far. The
    /// real-time rules allow none. Always 0 unless the program installed the
    /// allocation trap ([`bl_rt::install_trap!`]), as test and debug builds
    /// do.
    #[must_use]
    pub fn heap_operations(&self) -> u64 {
        self.shared.heap_operations.load(Relaxed)
    }

    /// Tasks and chains that panicked on a worker. Their audio is left as
    /// it was; the block carries on.
    #[must_use]
    pub fn panics(&self) -> u64 {
        self.shared.panics.load(Relaxed)
    }

    /// A handle that reads the counters above after the pool has moved to
    /// the audio thread, or been dropped.
    #[must_use]
    pub fn monitor(&self) -> PoolMonitor {
        PoolMonitor(Arc::clone(&self.shared))
    }

    /// Runs the program's tasks over the block on the workers and this
    /// thread, and returns how many changes reached a node or a route.
    pub(crate) fn run(&mut self, program: &EngineProgram, lane: &mut Lane, block: &Blk<'_>) -> u64 {
        let tasks = program.tasks.len();
        if self.threads.is_empty() || tasks < 2 {
            return program.run_serial(lane, block);
        }
        let sched = &program.sched;
        for (waiting, task) in sched.waiting.iter().zip(&program.tasks) {
            waiting.store(task.dependencies, Relaxed);
        }
        sched.remaining.store(tasks as u32, Relaxed);
        sched.delivered.store(0, Relaxed);
        for &t in &program.order {
            if program.tasks[t as usize].dependencies == 0 {
                let queued = sched.queue.push(t);
                debug_assert!(queued, "the queue holds every task");
            }
        }
        let job = Job {
            program,
            block: core::ptr::from_ref(block).cast::<Blk<'static>>(),
        };
        let shared = &*self.shared;
        // However this run ends, the workers leave the program before the
        // caller can free or swap it.
        let _quiet = Quiet {
            shared,
            queue: &sched.queue,
        };
        shared.job.store(&raw const job as *mut Job, SeqCst);
        shared.epoch.fetch_add(1, SeqCst);
        shared.wake.all_for(BETWEEN);
        work(program, block, lane, shared, None);
        sched.delivered.load(Acquire)
    }
}

/// Reads a [`Pool`]'s counters from any thread.
#[derive(Clone)]
pub struct PoolMonitor(Arc<Shared>);

impl PoolMonitor {
    /// See [`Pool::heap_operations`].
    #[must_use]
    pub fn heap_operations(&self) -> u64 {
        self.0.heap_operations.load(Relaxed)
    }

    /// See [`Pool::panics`].
    #[must_use]
    pub fn panics(&self) -> u64 {
        self.0.panics.load(Relaxed)
    }
}

/// Ends a block's run when it drops: the workers are told to leave, and
/// the audio thread waits until they have.
struct Quiet<'a> {
    shared: &'a Shared,
    queue: &'a Queue,
}

impl Drop for Quiet<'_> {
    fn drop(&mut self) {
        let shared = self.shared;
        shared.abort.store(true, SeqCst);
        shared.job.store(core::ptr::null_mut(), SeqCst);
        shared.wake.all_for(WORK);
        while shared.active.load(SeqCst) != 0 {
            spin_loop();
        }
        shared.abort.store(false, SeqCst);
        // Tickets nobody took, because the owner of their rack took the
        // chains itself, would crowd out the next block's work.
        while self.queue.pop().is_some() {}
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, SeqCst);
        for sleeper in &self.shared.wake.sleepers {
            sleeper.unpark();
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl core::fmt::Debug for Pool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Pool")
            .field("workers", &self.threads.len())
            .finish_non_exhaustive()
    }
}

/// A worker's thread: it sets itself up for real-time work, then runs the
/// blocks the audio thread starts until the pool is dropped.
fn worker(
    index: usize,
    shared: &Shared,
    config: &EngineConfig,
    rt: &RtThreadConfig,
    ready: &mpsc::Sender<ThreadReport>,
) {
    // Everything that allocates, waits, or touches a thread-local for the
    // first time (real-time rule 1's macOS note) happens here, before the
    // first block.
    let report = bl_rt::prepare_current_thread(rt);
    let mut lane = Lane::new(config);
    let _ = ready.send(report);
    let mut seen = shared.epoch.load(SeqCst);
    'blocks: loop {
        let mut spins = 0;
        while shared.epoch.load(SeqCst) == seen {
            if shared.shutdown.load(SeqCst) {
                break 'blocks;
            }
            if spins < BLOCK_SPINS {
                spins += 1;
                spin_loop();
            } else {
                shared.wake.sleep(index, BETWEEN, || {
                    shared.epoch.load(SeqCst) == seen && !shared.shutdown.load(SeqCst)
                });
                spins = 0;
            }
        }
        seen = shared.epoch.load(SeqCst);
        shared.active.fetch_add(1, SeqCst);
        // The audio thread clears the job, then waits for `active` to fall to
        // zero, so a job that is there now stays until this worker leaves.
        let job = shared.job.load(SeqCst);
        if !job.is_null() {
            // SAFETY: as above, the job and the program and block it points
            // to outlive this worker's count in `active`.
            let (program, block) = unsafe { (&*(*job).program, &*(*job).block) };
            let before = bl_rt::trap::thread_violations();
            work(program, block, &mut lane, shared, Some(index));
            shared
                .heap_operations
                .fetch_add(bl_rt::trap::thread_violations() - before, Relaxed);
        }
        shared.active.fetch_sub(1, SeqCst);
    }
    bl_rt::thread::release_current_thread();
}

/// Takes work from the program's queue until every task has finished. A
/// worker (`me`) sleeps when there is nothing to take for a while, and runs
/// each item as a real-time section, leaving it to sleep; the audio thread
/// (`None`), which is in one already, never sleeps.
fn work(
    program: &EngineProgram,
    block: &Blk<'_>,
    lane: &mut Lane,
    shared: &Shared,
    me: Option<usize>,
) {
    let sched = &program.sched;
    let mut idle = 0;
    loop {
        if let Some(item) = sched.queue.pop() {
            idle = 0;
            if me.is_some() {
                bl_rt::realtime(|| run_item(item, program, block, lane, shared));
            } else {
                run_item(item, program, block, lane, shared);
            }
        } else if sched.remaining.load(SeqCst) == 0 || shared.abort.load(SeqCst) {
            // The audio thread ends the block: tell the workers that sleep
            // for work it is over.
            if me.is_none() {
                shared.wake.all_for(WORK);
            }
            return;
        } else if idle < IDLE_SPINS || me.is_none() {
            idle += 1;
            spin_loop();
        } else if let Some(me) = me {
            shared.wake.sleep(me, WORK, || {
                sched.queue.is_empty()
                    && sched.remaining.load(SeqCst) != 0
                    && !shared.abort.load(SeqCst)
            });
            idle = 0;
        }
    }
}

/// Runs one queue item: a task, and then the tasks it was the last to hold
/// up, one at a time, with the rest queued for the others; or one of a
/// rack's chains.
fn run_item(item: u32, program: &EngineProgram, block: &Blk<'_>, lane: &mut Lane, shared: &Shared) {
    let sched = &program.sched;
    let wake = &shared.wake;
    if item & TICKET != 0 {
        let rack = (item & !TICKET) as usize;
        // SAFETY: a ticket is in the queue only while its rack's owner
        // waits for its chains.
        let run = || unsafe { program.run_offered(rack, lane, block, wake) };
        sched.delivered.fetch_add(survive(shared, run), Relaxed);
        return;
    }
    let mut next = Some(item);
    while let Some(t) = next.take() {
        // SAFETY: a task is queued, or handed on below, only once every task
        // feeding it has finished, and only once.
        let run = || unsafe { program.run_task(t, lane, block, Some(wake)) };
        sched.delivered.fetch_add(survive(shared, run), Relaxed);
        let task = &program.tasks[t as usize];
        for &dependent in &program.task_dependents[task.dependents.range()] {
            if sched.waiting[dependent as usize].fetch_sub(1, AcqRel) == 1 {
                if next.is_none() {
                    next = Some(dependent);
                } else {
                    while !sched.queue.push(dependent) {
                        spin_loop();
                    }
                    wake.one();
                }
            }
        }
        sched.remaining.fetch_sub(1, SeqCst);
    }
}

/// Runs `f`, counting a panic instead of letting it end the worker, which
/// would leave the block waiting for work that never finishes.
fn survive(shared: &Shared, f: impl FnOnce() -> u64) -> u64 {
    panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        shared.panics.fetch_add(1, Relaxed);
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;
    use std::sync::Barrier;

    #[test]
    fn a_queue_hands_items_out_in_the_order_they_came() {
        let queue = Queue::new(4);
        assert_eq!(queue.pop(), None);
        for i in 0..4 {
            assert!(queue.push(i));
        }
        assert!(!queue.push(99), "a full queue refuses");
        for i in 0..4 {
            assert_eq!(queue.pop(), Some(i));
        }
        assert_eq!(queue.pop(), None);
        // And again, past the first lap.
        for round in 0..10 {
            assert!(queue.push(round));
            assert_eq!(queue.pop(), Some(round));
        }
    }

    #[test]
    fn threads_push_and_pop_every_item_once() {
        const THREADS: u32 = 4;
        const EACH: u32 = 5_000;
        let queue = Queue::new(64);
        let barrier = Barrier::new(THREADS as usize * 2);
        let taken = std::thread::scope(|scope| {
            for t in 0..THREADS {
                let (queue, barrier) = (&queue, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    for i in 0..EACH {
                        while !queue.push(t * EACH + i) {
                            spin_loop();
                        }
                    }
                });
            }
            let takers: Vec<_> = (0..THREADS)
                .map(|_| {
                    let (queue, barrier) = (&queue, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        let mut mine = Vec::new();
                        while mine.len() < EACH as usize {
                            match queue.pop() {
                                Some(item) => mine.push(item),
                                None => spin_loop(),
                            }
                        }
                        mine
                    })
                })
                .collect();
            takers
                .into_iter()
                .flat_map(|taker| taker.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(taken.len(), (THREADS * EACH) as usize);
        let distinct: HashSet<_> = taken.iter().copied().collect();
        assert_eq!(distinct.len(), taken.len(), "no item is taken twice");
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn a_fork_gives_each_chain_to_one_thread_and_joins_them() {
        let fork = Fork::default();
        fork.open();
        let ran = AtomicU32::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    while let Some(chain) = fork.claim(3) {
                        assert!(chain < 3);
                        ran.fetch_add(1, Relaxed);
                        fork.finish();
                    }
                });
            }
            fork.join(3);
            assert_eq!(ran.load(Relaxed), 3);
        });
        assert_eq!(fork.claim(3), None, "a stale ticket finds nothing");
    }

    #[test]
    fn a_post_before_a_wait_ends_it_and_a_wait_lasts_until_a_post() {
        let early = bed::Bed::new();
        early.bind(&thread::current());
        early.post();
        early.wait();
        let bed = bed::Bed::new();
        let woke = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                bed.bind(&thread::current());
                bed.wait();
                woke.store(true, SeqCst);
            });
            std::thread::sleep(Duration::from_millis(50));
            assert!(!woke.load(SeqCst), "it waits for a post");
            while !woke.load(SeqCst) {
                bed.post();
                std::thread::sleep(Duration::from_millis(5));
            }
        });
    }

    #[test]
    fn ending_a_block_leaves_no_ticket_in_the_queue_for_the_next() {
        // The owner of a rack that takes all its chains itself leaves the
        // tickets it offered; they would crowd out the next block's tasks.
        let shared = Shared {
            wake: Wake::new(0),
            epoch: AtomicU64::new(0),
            job: AtomicPtr::new(core::ptr::null_mut()),
            active: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            abort: AtomicBool::new(false),
            heap_operations: AtomicU64::new(0),
            panics: AtomicU64::new(0),
        };
        let queue = Queue::new(8);
        assert!(queue.push(Sched::ticket(0)));
        assert!(queue.push(Sched::ticket(0)));
        drop(Quiet {
            shared: &shared,
            queue: &queue,
        });
        assert_eq!(queue.pop(), None);
        assert!(!shared.abort.load(SeqCst), "the next block runs");
    }

    #[test]
    fn a_ticket_names_its_rack_and_is_not_a_task() {
        let ticket = Sched::ticket(7);
        assert_ne!(ticket & TICKET, 0);
        assert_eq!(ticket & !TICKET, 7);
    }
}
