//! The one path from every backend's device callback into Beatlayer's code.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::stats::{Counters, DROPOUT_MERGE, GapDetector};
use crate::{AudioBuffer, AudioBufferMut, AudioCallback, ProcessContext};

/// Runs an [`AudioCallback`] block by block on the audio thread and keeps the
/// stream's counters.
///
/// Every backend calls [`device_callback`](Self::device_callback) once per
/// device callback and [`run`](Self::run) once per block, from the backend's
/// audio thread, and nothing else touches the callback while the stream runs.
/// That makes this the place for anything that must happen on the audio thread
/// (see [`on_audio_thread_start`](Self::on_audio_thread_start)).
pub(crate) struct CallbackRunner {
    callback: Box<dyn AudioCallback>,
    counters: Arc<Counters>,
    sample_rate: u32,
    position: u64,
    gaps: GapDetector,
    check_deadlines: bool,
    started: bool,
    /// Where the latest dropout detection happened, in stream frames.
    last_detection: Option<u64>,
    /// [`DROPOUT_MERGE`] in frames.
    merge_frames: u64,
}

impl CallbackRunner {
    /// A runner for a stream at `sample_rate`. With `check_deadlines`, a block
    /// whose callback takes longer than the block lasts counts as late; the
    /// free-running null device turns this off because it has no deadlines.
    pub(crate) fn new(
        callback: Box<dyn AudioCallback>,
        counters: Arc<Counters>,
        sample_rate: u32,
        check_deadlines: bool,
    ) -> Self {
        Self {
            callback,
            counters,
            sample_rate,
            position: 0,
            gaps: GapDetector::default(),
            check_deadlines,
            started: false,
            last_detection: None,
            merge_frames: duration_to_frames(DROPOUT_MERGE, sample_rate),
        }
    }

    /// Notes a callback from the device asking for `frames` frames, before the
    /// backend splits it into blocks.
    pub(crate) fn device_callback(&self, frames: usize) {
        self.counters.record_device_callback(frames);
    }

    /// Runs one block: clears `output`, calls the callback, and counts gaps
    /// and late blocks. `timestamp` is when the device handles the block's
    /// first frame on the backend's clock, if the backend says.
    pub(crate) fn run(
        &mut self,
        timestamp: Option<Duration>,
        input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        if !self.started {
            self.started = true;
            self.on_audio_thread_start();
        }
        if self.counters.take_resync() {
            self.gaps.reset();
        }

        let frames = output.frames();
        debug_assert_eq!(input.frames(), frames, "input and output blocks differ");
        let lasts = frames_to_duration(frames as u64, self.sample_rate);
        let gap = timestamp.is_some_and(|start| self.gaps.observe(start, lasts));

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

    /// Runs on the audio thread just before the stream's first block.
    ///
    /// WP-0.4 adds real-time priority, the macOS audio workgroup join,
    /// denormal flushing, and the allocation trap here. A backend can move its
    /// callback to a new thread mid-stream (Core Audio does when the default
    /// device changes), so per-thread set-up should check the OS thread ID on
    /// each block rather than rely on this running once per thread.
    fn on_audio_thread_start(&mut self) {
        self.counters.mark_audio_thread_started();
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
    use std::sync::Mutex;

    /// Records each block's context and whether its output arrived silent.
    struct Recorder(Arc<Mutex<Vec<(ProcessContext, bool)>>>);

    impl AudioCallback for Recorder {
        fn process(
            &mut self,
            ctx: &ProcessContext,
            _input: &AudioBuffer<'_>,
            output: &mut AudioBufferMut<'_>,
        ) {
            let silent = output.as_buffer().iter().flatten().all(|&s| s == 0.0);
            // Test code: a lock is fine off a real audio thread.
            self.0.lock().unwrap().push((*ctx, silent));
            output.fill(1.0);
        }
    }

    #[test]
    fn blocks_carry_position_and_output_starts_silent() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let counters = Arc::new(Counters::new());
        let mut runner = CallbackRunner::new(
            Box::new(Recorder(Arc::clone(&seen))),
            Arc::clone(&counters),
            48_000,
            true,
        );
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
    }

    #[test]
    fn a_jump_in_timestamps_is_a_gap_until_a_resync() {
        let counters = Arc::new(Counters::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut runner = CallbackRunner::new(
            Box::new(Recorder(seen)),
            Arc::clone(&counters),
            48_000,
            true,
        );
        let mut out = PlanarScratch::new(1, 128);
        let mut block_at = |frames: u64, runner: &mut CallbackRunner| {
            let start = frames_to_duration(frames, 48_000);
            runner.run(
                Some(start),
                &AudioBuffer::empty(128),
                &mut out.view_mut(128),
            );
        };
        block_at(0, &mut runner);
        block_at(128, &mut runner);
        block_at(384, &mut runner); // One block missing.
        assert_eq!(counters.snapshot().gaps, 1);

        counters.request_resync();
        block_at(48_000, &mut runner);
        assert_eq!(counters.snapshot().gaps, 1, "a resync forgets the clock");
    }

    #[test]
    fn detections_close_together_are_one_dropout() {
        let counters = Arc::new(Counters::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut runner = CallbackRunner::new(
            Box::new(Recorder(seen)),
            Arc::clone(&counters),
            48_000,
            true,
        );
        let mut out = PlanarScratch::new(1, 128);
        let mut block_at = |frames: u64, runner: &mut CallbackRunner| {
            let start = frames_to_duration(frames, 48_000);
            runner.run(
                Some(start),
                &AudioBuffer::empty(128),
                &mut out.view_mut(128),
            );
        };
        block_at(0, &mut runner);
        block_at(128, &mut runner);
        assert_eq!(counters.snapshot().dropouts, 0);

        block_at(384, &mut runner); // A gap.
        assert_eq!(counters.snapshot().dropouts, 1);

        // The backend reports the same glitch a moment later.
        counters.record_xrun();
        assert_eq!(
            counters.snapshot().dropouts,
            2,
            "not merged until a block runs"
        );
        block_at(512, &mut runner);
        let stats = counters.snapshot();
        assert_eq!((stats.dropouts, stats.gaps, stats.xruns), (1, 1, 1));

        // After more than 100 ms of clean audio, another gap is another dropout.
        let mut at = 640;
        for _ in 0..40 {
            block_at(at, &mut runner);
            at += 128;
        }
        assert_eq!(counters.snapshot().dropouts, 1);
        block_at(at + 1_000, &mut runner);
        assert_eq!(counters.snapshot().dropouts, 2);
    }

    #[test]
    fn frames_convert_to_exact_durations() {
        assert_eq!(duration_to_frames(DROPOUT_MERGE, 48_000), 4_800);
        assert_eq!(frames_to_duration(48_000, 48_000), Duration::from_secs(1));
        assert_eq!(frames_to_duration(441, 44_100), Duration::from_millis(10));
        assert_eq!(frames_to_duration(1, 0), Duration::from_secs(1));
    }
}
