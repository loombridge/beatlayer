//! The one path from every backend's device callback into Beatlayer's code.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_rt::log::{Level, Writer};
use bl_rt::{AudioThreadSetup, RtThreadConfig, WorkgroupJoin, rt_log};

use crate::stats::{Counters, DROPOUT_MERGE, GapDetector};
use crate::{AudioBuffer, AudioBufferMut, AudioCallback, ProcessContext};

/// Runs an [`AudioCallback`] block by block on the audio thread and keeps the
/// stream's counters.
///
/// Every backend calls [`device_callback`](Self::device_callback) once per
/// device callback and [`run`](Self::run) once per block, from the backend's
/// audio thread, and nothing else touches the callback while the stream runs.
/// That makes this the place for what every audio thread needs: real-time
/// set-up on each new thread, and every block run as a real-time section
/// (denormals flushed, heap use trapped, RealtimeSanitizer watching when
/// it's built in).
///
/// The audio thread usually belongs to the audio API, so Beatlayer's first
/// code on it is a block with its deadline already running. Everything the
/// set-up needs is allocated here, when the stream opens, and the block that
/// finds itself on a new thread does the set-up instead of calling the
/// callback, as a real-time section like any other, and plays silence (see
/// [`AudioThreadSetup`]). That happens before a stream's first block, and
/// again whenever a block arrives on a different thread, because a backend
/// can move its callback mid-stream (Core Audio does when the default device
/// changes).
pub(crate) struct CallbackRunner {
    callback: Box<dyn AudioCallback>,
    counters: Arc<Counters>,
    sample_rate: u32,
    position: u64,
    gaps: GapDetector,
    check_deadlines: bool,
    /// Where the latest dropout detection happened, in stream frames.
    last_detection: Option<u64>,
    /// [`DROPOUT_MERGE`] in frames.
    merge_frames: u64,
    /// Each new audio thread's set-up, prepared when the stream opened.
    setup: AudioThreadSetup,
    /// Where the audio thread logs. Made here, so logging needs nothing from
    /// a thread the audio API owns.
    log: Writer,
}

impl CallbackRunner {
    /// A runner for a stream at `sample_rate`. With `check_deadlines`, a block
    /// whose callback takes longer than the block lasts counts as late; the
    /// free-running null device turns this off because it has no deadlines.
    /// `thread_config` is what each audio thread's set-up asks for.
    ///
    /// Runs when the stream opens, off the audio thread: it allocates what
    /// the audio thread's set-up and logging will need.
    pub(crate) fn new(
        callback: Box<dyn AudioCallback>,
        counters: Arc<Counters>,
        sample_rate: u32,
        check_deadlines: bool,
        thread_config: RtThreadConfig,
    ) -> Self {
        let (setup, reports) = AudioThreadSetup::new(thread_config);
        counters.set_thread_reports(reports);
        Self {
            callback,
            counters,
            sample_rate,
            position: 0,
            gaps: GapDetector::default(),
            check_deadlines,
            last_detection: None,
            merge_frames: duration_to_frames(DROPOUT_MERGE, sample_rate),
            setup,
            log: Writer::new(),
        }
    }

    /// Notes a callback from the device asking for `frames` frames, before the
    /// backend splits it into blocks.
    pub(crate) fn device_callback(&self, frames: usize) {
        self.counters.record_device_callback(frames);
    }

    /// Runs one block as a real-time section. On a thread that's already set
    /// up, it clears `output`, calls the callback, and counts gaps and late
    /// blocks. On a new thread, it sets the thread up instead and leaves
    /// `output` silent. `timestamp` is when the device handles the block's
    /// first frame on the backend's clock, if the backend says.
    pub(crate) fn run(
        &mut self,
        timestamp: Option<Duration>,
        input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let heap_before = bl_rt::trap::thread_violations();
        bl_rt::realtime(|| {
            if self.setup.is_current_thread() {
                self.run_block(timestamp, input, output);
            } else {
                self.set_up_in_block(timestamp, output);
            }
        });
        self.count_heap_use(heap_before);
    }

    /// Sets up the calling thread now, if it isn't yet, so its first block
    /// can go to the callback. For a backend that drives a thread of its
    /// own: it calls this before its first block, before the block's
    /// deadline starts. The set-up still runs as a real-time section.
    pub(crate) fn prepare_thread(&mut self) {
        if !self.setup.is_current_thread() {
            let heap_before = bl_rt::trap::thread_violations();
            bl_rt::realtime(|| self.set_up_thread());
            self.count_heap_use(heap_before);
        }
    }

    /// Counts and logs heap use in the real-time section that just ended, if
    /// the allocation trap saw any since `before`.
    fn count_heap_use(&mut self, before: u64) {
        let heap_operations = bl_rt::trap::thread_violations() - before;
        if heap_operations > 0 {
            self.counters.record_heap_operations(heap_operations);
            rt_log!(
                to: self.log,
                Level::Error,
                "{} heap operations on the audio thread in the block at frame {}",
                heap_operations,
                self.position
            );
        }
    }

    /// Undoes the set-up on a thread the backend owns and is about to end:
    /// leaves the workgroup now, rather than in thread-exit clean-up.
    pub(crate) fn release_thread(&mut self) {
        self.setup.release();
    }

    /// The set-up itself: allocation-free, and nothing in it blocks (see
    /// [`AudioThreadSetup::enter`]).
    fn set_up_thread(&mut self) {
        let report = self.setup.enter();
        let setups = self.counters.record_thread_setup();
        self.counters.mark_audio_thread_started();
        rt_log!(
            to: self.log,
            Level::Info,
            "audio thread set up (set-up {} of this stream): {} priority, workgroup {}",
            setups,
            if report.priority.is_realtime() {
                "real-time"
            } else if report.promotion == bl_rt::Promotion::Pending {
                "raising to real-time"
            } else {
                "normal"
            },
            match report.workgroup {
                WorkgroupJoin::None => "none",
                WorkgroupJoin::Joined => "joined",
                WorkgroupJoin::AlreadyMember => "already a member",
                WorkgroupJoin::Failed(_) => "join failed",
            }
        );
    }

    /// The block that finds the callback on a new thread: sets the thread up
    /// and leaves `output` silent, so set-up never shares a deadline with
    /// audio work. The block still passes on the device's clock, so the
    /// callback's next block starts after it.
    fn set_up_in_block(&mut self, timestamp: Option<Duration>, output: &mut AudioBufferMut<'_>) {
        let began = Instant::now();
        self.set_up_thread();
        output.fill(0.0);
        let took = began.elapsed();
        self.finish_block(timestamp, output.frames(), took);
    }

    fn run_block(
        &mut self,
        timestamp: Option<Duration>,
        input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let frames = output.frames();
        debug_assert_eq!(input.frames(), frames, "input and output blocks differ");
        output.fill(0.0);
        let ctx = ProcessContext {
            sample_rate: self.sample_rate,
            frames,
            position: self.position,
            timestamp,
        };
        let began = Instant::now();
        self.callback.process(&ctx, input, output);
        let took = began.elapsed();
        self.finish_block(timestamp, frames, took);
    }

    /// Counts a block of `frames` that took `took`: checks the device clock
    /// for a gap, the deadline, and new xruns, and moves the stream on.
    fn finish_block(&mut self, timestamp: Option<Duration>, frames: usize, took: Duration) {
        if self.counters.take_resync() {
            self.gaps.reset();
        }
        let lasts = frames_to_duration(frames as u64, self.sample_rate);
        let gap = timestamp.is_some_and(|start| self.gaps.observe(start, lasts));
        let late = self.check_deadlines && took > lasts;
        self.counters.record_block(frames, took, late, gap);
        let xrun = self.counters.take_new_xruns() > 0;
        if late || gap || xrun {
            self.note_detection();
        }
        self.position += frames as u64;
    }

    /// Counts a dropout unless another detection happened within
    /// [`DROPOUT_MERGE`] before this block, in which case it's the same one.
    fn note_detection(&mut self) {
        let fresh = self
            .last_detection
            .is_none_or(|at| self.position.saturating_sub(at) > self.merge_frames);
        if fresh {
            self.counters.record_dropout();
        }
        self.last_detection = Some(self.position);
    }
}

/// How long `frames` frames last at `sample_rate`.
pub(crate) fn frames_to_duration(frames: u64, sample_rate: u32) -> Duration {
    let nanos = u128::from(frames) * 1_000_000_000 / u128::from(sample_rate.max(1));
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// How many whole frames fit in `duration` at `sample_rate`.
fn duration_to_frames(duration: Duration, sample_rate: u32) -> u64 {
    let frames = duration.as_nanos() * u128::from(sample_rate) / 1_000_000_000;
    u64::try_from(frames).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::PlanarScratch;
    use std::hint::black_box;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Each block's context and whether its output arrived silent.
    type Seen = Arc<Mutex<Vec<(ProcessContext, bool)>>>;

    /// Records what [`Seen`] holds.
    struct Recorder(Seen);

    impl Recorder {
        /// A recorder with room for `blocks` blocks, ready to record without
        /// allocating.
        fn new(blocks: usize) -> (Self, Seen) {
            let seen = Arc::new(Mutex::new(Vec::with_capacity(blocks)));
            // A mutex may allocate on its first lock (macOS's does), so take
            // that first lock here rather than in a block.
            drop(seen.lock().unwrap());
            (Self(Arc::clone(&seen)), seen)
        }
    }

    impl AudioCallback for Recorder {
        fn process(
            &mut self,
            ctx: &ProcessContext,
            _input: &AudioBuffer<'_>,
            output: &mut AudioBufferMut<'_>,
        ) {
            let silent = output.as_buffer().iter().flatten().all(|&s| s == 0.0);
            // Test code: nothing else holds the lock while blocks run.
            self.0.lock().unwrap().push((*ctx, silent));
            output.fill(1.0);
        }
    }

    /// A runner that leaves its thread's priority alone, as tests should.
    fn runner(callback: impl AudioCallback, counters: &Arc<Counters>) -> CallbackRunner {
        CallbackRunner::new(
            Box::new(callback),
            Arc::clone(counters),
            48_000,
            true,
            RtThreadConfig {
                period: frames_to_duration(128, 48_000),
                promote: false,
                workgroup: None,
            },
        )
    }

    fn block_at(runner: &mut CallbackRunner, out: &mut PlanarScratch, frames: u64) {
        let start = frames_to_duration(frames, 48_000);
        let len = out.capacity();
        runner.run(
            Some(start),
            &AudioBuffer::empty(len),
            &mut out.view_mut(len),
        );
    }

    #[test]
    fn blocks_carry_position_and_output_starts_silent() {
        let (recorder, seen) = Recorder::new(3);
        let counters = Arc::new(Counters::new());
        let mut runner = runner(recorder, &counters);
        runner.prepare_thread();
        // The previous block's samples are still in the scratch buffer.
        let mut out = PlanarScratch::new(2, 64);
        for block in 0..3u64 {
            let mut view = out.view_mut(64);
            let start = frames_to_duration(block * 64, 48_000);
            runner.run(Some(start), &AudioBuffer::empty(64), &mut view);
            assert!(view.as_buffer().iter().flatten().all(|&s| s == 1.0));
        }
        let seen = seen.lock().unwrap();
        let positions: Vec<u64> = seen.iter().map(|(ctx, _)| ctx.position).collect();
        assert_eq!(positions, [0, 64, 128]);
        assert!(seen.iter().all(|&(_, silent)| silent));
        let stats = counters.snapshot();
        assert_eq!((stats.blocks, stats.frames, stats.gaps), (3, 192, 0));
        assert!(stats.audio_thread_started);
        assert_eq!(
            stats.heap_operations, 0,
            "the runner's own path stays off the heap"
        );
    }

    /// A thread the audio API owns runs Beatlayer's code first in a block:
    /// that block sets the thread up and plays silence, and the callback's
    /// first block is the next one, at the next position on the device's
    /// clock.
    #[test]
    fn a_new_threads_first_block_sets_it_up_and_plays_silence() {
        let (recorder, seen) = Recorder::new(3);
        let counters = Arc::new(Counters::new());
        let mut runner = runner(recorder, &counters);
        let mut out = PlanarScratch::new(2, 64);
        // Left over from before, so a silent block can be told apart.
        out.view_mut(64).fill(0.5);
        let mut silent = Vec::new();
        for block in 0..3u64 {
            let mut view = out.view_mut(64);
            let start = frames_to_duration(block * 64, 48_000);
            runner.run(Some(start), &AudioBuffer::empty(64), &mut view);
            silent.push(view.as_buffer().iter().flatten().all(|&s| s == 0.0));
        }
        assert_eq!(silent, [true, false, false]);
        let positions: Vec<u64> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(ctx, _)| ctx.position)
            .collect();
        assert_eq!(
            positions,
            [64, 128],
            "the set-up block isn't the callback's"
        );
        let stats = counters.snapshot();
        assert_eq!((stats.blocks, stats.frames), (3, 192));
        assert_eq!(
            (stats.gaps, stats.dropouts),
            (0, 0),
            "the device clock runs on"
        );
        assert_eq!(stats.audio_thread_setups, 1);
    }

    /// The set-up a block does on a new thread, with everything a real
    /// stream asks for (real-time priority and, on macOS, a workgroup it
    /// really joins), allocates nothing. The runner runs it inside the
    /// block's real-time section, so the trap counts what it does.
    #[test]
    fn set_up_in_a_block_stays_off_the_heap() {
        #[cfg(target_os = "macos")]
        let interval = bl_rt::WorkInterval::new("bl-audio-io runner test").unwrap();
        let counters = Arc::new(Counters::new());
        let mut runner = CallbackRunner::new(
            Box::new(Recorder::new(8).0),
            Arc::clone(&counters),
            48_000,
            true,
            RtThreadConfig {
                period: frames_to_duration(128, 48_000),
                promote: true,
                #[cfg(target_os = "macos")]
                workgroup: Some(interval.workgroup().clone()),
                #[cfg(not(target_os = "macos"))]
                workgroup: None,
            },
        );
        let (runner, first) = std::thread::spawn(move || {
            let mut out = PlanarScratch::new(1, 128);
            let ((), heap) = bl_rt::trap::watch(|| block_at(&mut runner, &mut out, 0));
            block_at(&mut runner, &mut out, 128);
            runner.release_thread();
            (runner, heap.map(|heap| heap.to_string()))
        })
        .join()
        .unwrap();
        drop(runner);
        assert_eq!(first, None, "the set-up block touched the heap");
        let stats = counters.snapshot();
        assert_eq!(stats.heap_operations, 0);
        assert_eq!(stats.audio_thread_setups, 1);
        let report = stats.audio_thread.expect("a set-up report");
        #[cfg(any(target_os = "macos", windows))]
        assert!(report.priority.is_realtime(), "{}", report.priority);
        #[cfg(target_os = "macos")]
        assert_eq!(report.workgroup, WorkgroupJoin::Joined);
        #[cfg(target_os = "linux")]
        let _ = report;
    }

    #[test]
    fn a_jump_in_timestamps_is_a_gap_until_a_resync() {
        let counters = Arc::new(Counters::new());
        let mut runner = runner(Recorder::new(8).0, &counters);
        let mut out = PlanarScratch::new(1, 128);
        block_at(&mut runner, &mut out, 0);
        block_at(&mut runner, &mut out, 128);
        block_at(&mut runner, &mut out, 384); // One block missing.
        assert_eq!(counters.snapshot().gaps, 1);

        counters.request_resync();
        block_at(&mut runner, &mut out, 48_000);
        assert_eq!(counters.snapshot().gaps, 1, "a resync forgets the clock");
    }

    #[test]
    fn detections_close_together_are_one_dropout() {
        let counters = Arc::new(Counters::new());
        let mut runner = runner(Recorder::new(64).0, &counters);
        let mut out = PlanarScratch::new(1, 128);
        block_at(&mut runner, &mut out, 0);
        block_at(&mut runner, &mut out, 128);
        assert_eq!(counters.snapshot().dropouts, 0);

        block_at(&mut runner, &mut out, 384); // A gap.
        assert_eq!(counters.snapshot().dropouts, 1);

        // The backend reports the same glitch a moment later.
        counters.record_xrun();
        assert_eq!(
            counters.snapshot().dropouts,
            2,
            "not merged until a block runs"
        );
        block_at(&mut runner, &mut out, 512);
        let stats = counters.snapshot();
        assert_eq!((stats.dropouts, stats.gaps, stats.xruns), (1, 1, 1));

        // After more than 100 ms of clean audio, another gap is another dropout.
        let mut at = 640;
        for _ in 0..40 {
            block_at(&mut runner, &mut out, at);
            at += 128;
        }
        assert_eq!(counters.snapshot().dropouts, 1);
        block_at(&mut runner, &mut out, at + 1_000);
        assert_eq!(counters.snapshot().dropouts, 2);
    }

    #[test]
    fn each_new_thread_is_set_up_once() {
        let counters = Arc::new(Counters::new());
        let mut runner = runner(Recorder::new(8).0, &counters);
        assert_eq!(counters.snapshot().audio_thread, None);
        // Blocks arrive on one thread, then the backend moves to another, as
        // Core Audio does when the default device changes.
        let mut at = 0;
        for _ in 0..2 {
            (runner, at) = std::thread::spawn(move || {
                let mut out = PlanarScratch::new(1, 128);
                for _ in 0..3 {
                    block_at(&mut runner, &mut out, at);
                    at += 128;
                }
                (runner, at)
            })
            .join()
            .unwrap();
        }
        let stats = counters.snapshot();
        assert_eq!(stats.blocks, 6, "two of them set threads up");
        assert_eq!(stats.audio_thread_setups, 2);
        let report = stats.audio_thread.expect("a set-up report");
        assert_eq!(report.promotion, bl_rt::Promotion::NotRequested);
        assert_eq!(report.workgroup, WorkgroupJoin::None);
        assert_eq!(report.denormals_flushed, bl_rt::denormals::SUPPORTED);
        assert!(report.trap_installed);
        assert_eq!(stats.gaps, 0, "a new thread doesn't break the clock");
    }

    /// Allocates and frees in every block.
    struct Allocator;

    impl AudioCallback for Allocator {
        fn process(&mut self, _: &ProcessContext, _: &AudioBuffer<'_>, _: &mut AudioBufferMut<'_>) {
            black_box(Box::new(0u64));
        }
    }

    #[test]
    fn heap_use_in_a_block_is_counted() {
        let counters = Arc::new(Counters::new());
        let mut runner = runner(Allocator, &counters);
        let mut out = PlanarScratch::new(1, 128);
        block_at(&mut runner, &mut out, 0); // Sets the thread up.
        block_at(&mut runner, &mut out, 128);
        block_at(&mut runner, &mut out, 256);
        assert_eq!(
            counters.snapshot().heap_operations,
            4,
            "an allocation and a free per block the callback ran"
        );
    }

    /// Halves the smallest normal float, which gives a denormal unless the
    /// block runs with denormals flushed, and keeps the result.
    struct Halver(Arc<AtomicU32>);

    impl AudioCallback for Halver {
        fn process(&mut self, _: &ProcessContext, _: &AudioBuffer<'_>, _: &mut AudioBufferMut<'_>) {
            let half = black_box(f32::MIN_POSITIVE) / black_box(2.0f32);
            self.0.store(half.to_bits(), Ordering::Relaxed);
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn blocks_run_with_denormals_flushed_to_zero() {
        let result = Arc::new(AtomicU32::new(u32::MAX));
        let counters = Arc::new(Counters::new());
        let mut runner = runner(Halver(Arc::clone(&result)), &counters);
        runner.prepare_thread();
        block_at(&mut runner, &mut PlanarScratch::new(1, 128), 0);
        assert_eq!(f32::from_bits(result.load(Ordering::Relaxed)), 0.0);
        let outside = black_box(f32::MIN_POSITIVE) / black_box(2.0f32);
        assert!(
            outside.is_subnormal(),
            "the thread's own setting is restored"
        );
    }

    #[test]
    fn frames_convert_to_exact_durations() {
        assert_eq!(duration_to_frames(DROPOUT_MERGE, 48_000), 4_800);
        assert_eq!(frames_to_duration(48_000, 48_000), Duration::from_secs(1));
        assert_eq!(frames_to_duration(441, 44_100), Duration::from_millis(10));
        assert_eq!(frames_to_duration(1, 0), Duration::from_secs(1));
    }
}
