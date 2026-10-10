//! What a stream counts while it runs, and how it spots dropouts.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

/// A snapshot of a stream's counters since it opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Blocks passed to the callback.
    pub blocks: u64,
    /// Frames passed to the callback.
    pub frames: u64,
    /// Callbacks from the device, before large ones are split into blocks.
    pub device_callbacks: u64,
    /// The smallest device callback, in frames.
    pub min_device_frames: Option<u32>,
    /// The largest device callback, in frames.
    pub max_device_frames: Option<u32>,
    /// The latest device callback, in frames: the buffer size the device
    /// actually runs at, whatever was requested.
    pub last_device_frames: Option<u32>,
    /// Distinct dropouts. Three detectors look for them: backend xruns, clock
    /// gaps, and late blocks. One glitch usually trips more than one, so
    /// detections less than [`DROPOUT_MERGE`] of stream time apart count once.
    pub dropouts: u64,
    /// Underruns and overruns the backend reported.
    pub xruns: u64,
    /// Breaks in the device clock: a block that didn't start where the one
    /// before it ended, because the device skipped or repeated audio.
    pub gaps: u64,
    /// Blocks whose callback took longer than the block lasts.
    pub late_blocks: u64,
    /// The longest any callback took.
    pub longest_block: Duration,
    /// Times the system default device changed under a stream that follows it.
    pub device_changes: u64,
    /// Whether the device went away.
    pub device_lost: bool,
    /// Whether the backend says the stream must be rebuilt, for example after
    /// the device's sample rate changed.
    pub invalidated: bool,
    /// Whether the backend asked for real-time scheduling and was refused.
    pub realtime_denied: bool,
    /// Other errors the backend reported while the stream ran.
    pub other_errors: u64,
    /// Whether the audio thread has run a block.
    pub audio_thread_started: bool,
}

/// How close in stream time two dropout detections must be to count as one
/// dropout.
pub const DROPOUT_MERGE: Duration = Duration::from_millis(100);

/// The live counters behind [`StreamStats`], shared between the audio thread,
/// the backend's notification threads, and the thread that owns the stream.
/// Every method is a handful of relaxed atomic operations, so any of them is
/// safe on the audio thread.
#[derive(Debug)]
pub(crate) struct Counters {
    blocks: AtomicU64,
    frames: AtomicU64,
    device_callbacks: AtomicU64,
    min_device_frames: AtomicU32,
    max_device_frames: AtomicU32,
    last_device_frames: AtomicU32,
    /// Dropouts the audio thread has counted.
    dropouts: AtomicU64,
    xruns: AtomicU64,
    /// Xruns the audio thread has already folded into `dropouts`.
    xruns_seen: AtomicU64,
    gaps: AtomicU64,
    late_blocks: AtomicU64,
    longest_block_ns: AtomicU64,
    device_changes: AtomicU64,
    device_lost: AtomicBool,
    invalidated: AtomicBool,
    realtime_denied: AtomicBool,
    other_errors: AtomicU64,
    audio_thread_started: AtomicBool,
    resync: AtomicBool,
}

impl Counters {
    pub(crate) fn new() -> Self {
        Self {
            blocks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            device_callbacks: AtomicU64::new(0),
            min_device_frames: AtomicU32::new(u32::MAX),
            max_device_frames: AtomicU32::new(0),
            last_device_frames: AtomicU32::new(0),
            dropouts: AtomicU64::new(0),
            xruns: AtomicU64::new(0),
            xruns_seen: AtomicU64::new(0),
            gaps: AtomicU64::new(0),
            late_blocks: AtomicU64::new(0),
            longest_block_ns: AtomicU64::new(0),
            device_changes: AtomicU64::new(0),
            device_lost: AtomicBool::new(false),
            invalidated: AtomicBool::new(false),
            realtime_denied: AtomicBool::new(false),
            other_errors: AtomicU64::new(0),
            audio_thread_started: AtomicBool::new(false),
            resync: AtomicBool::new(false),
        }
    }

    pub(crate) fn record_device_callback(&self, frames: usize) {
        let frames = u32::try_from(frames).unwrap_or(u32::MAX);
        self.device_callbacks.fetch_add(1, Relaxed);
        self.min_device_frames.fetch_min(frames, Relaxed);
        self.max_device_frames.fetch_max(frames, Relaxed);
        self.last_device_frames.store(frames, Relaxed);
    }

    pub(crate) fn record_block(&self, frames: usize, took: Duration, late: bool, gap: bool) {
        self.blocks.fetch_add(1, Relaxed);
        self.frames.fetch_add(frames as u64, Relaxed);
        let took = u64::try_from(took.as_nanos()).unwrap_or(u64::MAX);
        self.longest_block_ns.fetch_max(took, Relaxed);
        if late {
            self.late_blocks.fetch_add(1, Relaxed);
        }
        if gap {
            self.gaps.fetch_add(1, Relaxed);
        }
    }

    pub(crate) fn record_xrun(&self) {
        self.xruns.fetch_add(1, Relaxed);
    }

    /// Returns how many xruns arrived since the audio thread last asked, and
    /// marks them as seen.
    pub(crate) fn take_new_xruns(&self) -> u64 {
        let xruns = self.xruns.load(Relaxed);
        xruns.saturating_sub(self.xruns_seen.swap(xruns, Relaxed))
    }

    pub(crate) fn record_dropout(&self) {
        self.dropouts.fetch_add(1, Relaxed);
    }

    pub(crate) fn record_device_change(&self) {
        self.device_changes.fetch_add(1, Relaxed);
    }

    pub(crate) fn record_device_lost(&self) {
        self.device_lost.store(true, Relaxed);
    }

    pub(crate) fn record_invalidated(&self) {
        self.invalidated.store(true, Relaxed);
    }

    pub(crate) fn record_realtime_denied(&self) {
        self.realtime_denied.store(true, Relaxed);
    }

    pub(crate) fn record_other_error(&self) {
        self.other_errors.fetch_add(1, Relaxed);
    }

    pub(crate) fn mark_audio_thread_started(&self) {
        self.audio_thread_started.store(true, Relaxed);
    }

    /// Asks the audio thread to forget the device clock before its next
    /// block, because the stream is (re)starting and the clock may jump.
    pub(crate) fn request_resync(&self) {
        self.resync.store(true, Relaxed);
    }

    pub(crate) fn take_resync(&self) -> bool {
        self.resync.load(Relaxed) && self.resync.swap(false, Relaxed)
    }

    pub(crate) fn snapshot(&self) -> StreamStats {
        let device_callbacks = self.device_callbacks.load(Relaxed);
        let seen = |value: u32| (device_callbacks > 0).then_some(value);
        // Xruns the audio thread hasn't run since (for example after the
        // stream stopped) can't be merged with anything, so each counts.
        let unseen_xruns = self
            .xruns
            .load(Relaxed)
            .saturating_sub(self.xruns_seen.load(Relaxed));
        StreamStats {
            dropouts: self.dropouts.load(Relaxed) + unseen_xruns,
            blocks: self.blocks.load(Relaxed),
            frames: self.frames.load(Relaxed),
            device_callbacks,
            min_device_frames: seen(self.min_device_frames.load(Relaxed)),
            max_device_frames: seen(self.max_device_frames.load(Relaxed)),
            last_device_frames: seen(self.last_device_frames.load(Relaxed)),
            xruns: self.xruns.load(Relaxed),
            gaps: self.gaps.load(Relaxed),
            late_blocks: self.late_blocks.load(Relaxed),
            longest_block: Duration::from_nanos(self.longest_block_ns.load(Relaxed)),
            device_changes: self.device_changes.load(Relaxed),
            device_lost: self.device_lost.load(Relaxed),
            invalidated: self.invalidated.load(Relaxed),
            realtime_denied: self.realtime_denied.load(Relaxed),
            other_errors: self.other_errors.load(Relaxed),
            audio_thread_started: self.audio_thread_started.load(Relaxed),
        }
    }
}

/// Spots breaks in a device's clock: a block whose timestamp isn't where the
/// previous block's end put it.
///
/// A device that misses a cycle starts the next block a whole block late, so
/// anything more than three quarters of a block off counts. Core Audio, ASIO,
/// JACK, and the null device stamp blocks from the device clock, which makes
/// this exact. WASAPI and ALSA stamp them when the callback runs, so heavy
/// scheduling jitter at small buffers could read as a gap; their xrun reports
/// are the better signal there.
#[derive(Debug, Default)]
pub(crate) struct GapDetector {
    expected: Option<Duration>,
}

impl GapDetector {
    /// Forgets the clock, so the next block starts a new run.
    pub(crate) fn reset(&mut self) {
        self.expected = None;
    }

    /// Notes a block that starts at `timestamp` and lasts `duration`, and
    /// returns whether it starts a gap.
    pub(crate) fn observe(&mut self, timestamp: Duration, duration: Duration) -> bool {
        let gap = self
            .expected
            .is_some_and(|expected| timestamp.abs_diff(expected) > duration * 3 / 4);
        self.expected = Some(timestamp.saturating_add(duration));
        gap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: Duration = Duration::from_micros(2_667);

    fn run(detector: &mut GapDetector, starts_us: &[u64]) -> Vec<bool> {
        starts_us
            .iter()
            .map(|&us| detector.observe(Duration::from_micros(us), BLOCK))
            .collect()
    }

    #[test]
    fn a_steady_clock_has_no_gaps() {
        let mut detector = GapDetector::default();
        let starts: Vec<u64> = (0..1000).map(|n| n * 2_667).collect();
        assert!(run(&mut detector, &starts).iter().all(|gap| !gap));
    }

    #[test]
    fn jitter_within_three_eighths_of_a_block_is_not_a_gap() {
        // Each start is compared with the previous block's end, so a block
        // 0.9 ms late followed by one on time reads as 0.9 ms early.
        let mut detector = GapDetector::default();
        assert_eq!(
            run(&mut detector, &[0, 2_667 + 900, 5_334, 8_001 - 900, 10_668]),
            [false; 5]
        );
    }

    #[test]
    fn a_skipped_block_is_a_gap() {
        let mut detector = GapDetector::default();
        assert_eq!(
            run(&mut detector, &[0, 2_667, 8_001, 10_668]),
            [false, false, true, false]
        );
    }

    #[test]
    fn a_clock_that_jumps_back_is_a_gap() {
        let mut detector = GapDetector::default();
        assert_eq!(
            run(&mut detector, &[10_000, 12_667, 1_000]),
            [false, false, true]
        );
    }

    #[test]
    fn reset_forgets_the_clock() {
        let mut detector = GapDetector::default();
        run(&mut detector, &[0, 2_667]);
        detector.reset();
        assert_eq!(run(&mut detector, &[1_000_000]), [false]);
    }

    #[test]
    fn counters_track_device_buffer_sizes() {
        let counters = Counters::new();
        assert_eq!(counters.snapshot().min_device_frames, None);
        for frames in [480, 441, 512, 480] {
            counters.record_device_callback(frames);
        }
        let stats = counters.snapshot();
        assert_eq!(stats.device_callbacks, 4);
        assert_eq!(stats.min_device_frames, Some(441));
        assert_eq!(stats.max_device_frames, Some(512));
        assert_eq!(stats.last_device_frames, Some(480));
    }

    #[test]
    fn blocks_record_each_detector_separately() {
        let counters = Counters::new();
        counters.record_block(128, Duration::from_millis(5), true, true);
        counters.record_block(128, Duration::from_millis(1), false, false);
        let stats = counters.snapshot();
        assert_eq!((stats.late_blocks, stats.gaps), (1, 1));
        assert_eq!(stats.dropouts, 0, "only the audio thread counts dropouts");
        assert_eq!(stats.blocks, 2);
        assert_eq!(stats.frames, 256);
        assert_eq!(stats.longest_block, Duration::from_millis(5));
    }

    #[test]
    fn xruns_the_audio_thread_has_not_seen_count_as_dropouts() {
        let counters = Counters::new();
        counters.record_xrun();
        counters.record_xrun();
        assert_eq!(counters.snapshot().dropouts, 2);
        assert_eq!(counters.take_new_xruns(), 2);
        assert_eq!(counters.take_new_xruns(), 0);
        assert_eq!(counters.snapshot().dropouts, 0, "seen, but not yet counted");
        counters.record_dropout();
        counters.record_xrun();
        let stats = counters.snapshot();
        assert_eq!((stats.xruns, stats.dropouts), (3, 2));
    }

    #[test]
    fn resync_is_taken_once() {
        let counters = Counters::new();
        assert!(!counters.take_resync());
        counters.request_resync();
        assert!(counters.take_resync());
        assert!(!counters.take_resync());
    }
}
