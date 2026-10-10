//! The null device: runs the callback on a timer thread without hardware, so
//! tests and CI exercise the whole stream path on any machine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bl_rt::{RtThreadConfig, WorkInterval};

use crate::buffer::PlanarScratch;
use crate::runner::{CallbackRunner, frames_to_duration};
use crate::stats::Counters;
use crate::{
    AudioCallback, Backend, BackendKind, BufferSizes, DeviceId, DeviceInfo, DeviceSelector, Error,
    ErrorKind, OpenDevice, PortRequest, STANDARD_SAMPLE_RATES, SampleFormat, Stream, StreamInfo,
    StreamRequest, StreamStats, Workgroup, WorkgroupInfo,
};

const NAME: &str = "Null device";
const CHANNELS: u16 = 2;
const DEFAULT_RATE: u32 = 48_000;
const DEFAULT_BUFFER: u32 = 256;
const MIN_BUFFER: u32 = 16;
const MAX_BUFFER: u32 = 8192;

/// How the null device paces its blocks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pacing {
    /// Blocks follow the system clock, as a sound card's would. When the
    /// thread wakes late it catches up by running blocks back to back, so the
    /// device itself never drops audio: its only dropouts are blocks whose
    /// callback took longer than the block lasts.
    #[default]
    RealTime,
    /// Blocks run back to back as fast as the callback allows, and lateness
    /// isn't measured. Tests use this to run minutes of audio in moments.
    Free,
}

/// Settings for the null backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NullOptions {
    /// How blocks are paced.
    pub pacing: Pacing,
    /// If set, a running stream stops by itself after this many frames.
    pub frame_limit: Option<u64>,
}

/// A backend with one device, `null:null`, that runs the callback on a timer:
/// two input and two output channels, the standard sample rates, and buffers
/// of 16 to 8,192 frames. Input is silence.
#[derive(Debug, Clone, Default)]
pub struct NullBackend {
    options: NullOptions,
}

impl NullBackend {
    /// A null backend paced in real time.
    pub fn new() -> Self {
        Self::default()
    }

    /// A null backend with these settings.
    pub fn with_options(options: NullOptions) -> Self {
        Self { options }
    }

    /// The ID of the null device.
    pub fn device_id() -> DeviceId {
        DeviceId::new(BackendKind::Null, "null")
    }

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: Self::device_id(),
            name: NAME.to_owned(),
            is_default_output: true,
            is_default_input: true,
            output_channels: CHANNELS,
            input_channels: CHANNELS,
            sample_rates: STANDARD_SAMPLE_RATES.to_vec(),
            current_sample_rate: Some(DEFAULT_RATE),
            buffer_sizes: BufferSizes::Range {
                min: MIN_BUFFER,
                max: MAX_BUFFER,
            },
            default_buffer_size: Some(DEFAULT_BUFFER),
        }
    }
}

/// Checks one direction of a request and returns its channel count, or 0 if
/// the request doesn't ask for that direction.
fn channels_for(port: Option<&PortRequest>, direction: &str) -> Result<u16, Error> {
    let Some(port) = port else {
        return Ok(0);
    };
    if let DeviceSelector::Id(id) = &port.device
        && *id != NullBackend::device_id()
    {
        return Err(Error::new(
            ErrorKind::DeviceNotFound,
            format!("the null backend has no device `{id}`"),
        ));
    }
    let channels = port.channels.unwrap_or(CHANNELS);
    if !(1..=CHANNELS).contains(&channels) {
        return Err(Error::new(
            ErrorKind::UnsupportedConfig,
            format!("the null device has 1 or 2 {direction} channels, not {channels}"),
        ));
    }
    Ok(channels)
}

impl Backend for NullBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Null
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, Error> {
        Ok(vec![Self::device()])
    }

    fn open_stream(
        &self,
        request: &StreamRequest,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn Stream>, Error> {
        if request.output.is_none() && request.input.is_none() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "a stream needs output, input, or both",
            ));
        }
        let output_channels = channels_for(request.output.as_ref(), "output")?;
        let input_channels = channels_for(request.input.as_ref(), "input")?;

        let sample_rate = request.sample_rate.unwrap_or(DEFAULT_RATE);
        if !STANDARD_SAMPLE_RATES.contains(&sample_rate) {
            return Err(Error::new(
                ErrorKind::UnsupportedConfig,
                format!("the null device doesn't run at {sample_rate} Hz"),
            ));
        }
        let frames = request.buffer_size.unwrap_or(DEFAULT_BUFFER);
        if !(MIN_BUFFER..=MAX_BUFFER).contains(&frames) {
            return Err(Error::new(
                ErrorKind::UnsupportedConfig,
                format!(
                    "the null device takes buffers of {MIN_BUFFER} to {MAX_BUFFER} frames, not {frames}"
                ),
            ));
        }

        // Paced in real time, the device's thread works like a sound card's:
        // real-time priority and, on macOS, a work interval of its own, which
        // it starts and finishes around each block. Free pacing runs blocks
        // back to back without sleeping, which at real-time priority would
        // starve other threads on its core (and on Linux, run past the CPU
        // time rtkit allows a real-time thread).
        let realtime = self.options.pacing == Pacing::RealTime;
        let period = frames_to_duration(u64::from(frames), sample_rate);
        let interval = if realtime {
            WorkInterval::new("Beatlayer null device").ok()
        } else {
            None
        };
        let workgroup = interval.as_ref().map(|i| i.workgroup().clone());
        let counters = Arc::new(Counters::new());
        let runner = CallbackRunner::new(
            callback,
            Arc::clone(&counters),
            sample_rate,
            realtime,
            RtThreadConfig {
                period,
                promote: realtime,
                workgroup: workgroup.clone(),
            },
        );
        let engine = NullEngine {
            runner,
            input: PlanarScratch::new(input_channels.into(), frames as usize),
            output: PlanarScratch::new(output_channels.into(), frames as usize),
            frames: frames as usize,
            sample_rate,
            options: self.options,
            blocks: 0,
            interval,
            period,
        };
        let device = || OpenDevice {
            id: Self::device_id(),
            name: NAME.to_owned(),
        };
        let info = StreamInfo {
            backend: BackendKind::Null,
            output_device: (output_channels > 0).then(device),
            input_device: (input_channels > 0).then(device),
            follows_default: false,
            sample_rate,
            output_channels,
            input_channels,
            sample_format: SampleFormat::F32,
            requested_buffer_size: request.buffer_size,
            buffer_size: Some(frames),
            max_block_frames: frames,
            workgroup: workgroup.as_ref().map(WorkgroupInfo::of),
        };
        Ok(Box::new(NullStream {
            engine: Some(Box::new(engine)),
            thread: None,
            stop: Arc::new(AtomicBool::new(false)),
            info,
            counters,
            workgroup,
        }))
    }
}

/// Everything the null device's thread owns while it runs.
struct NullEngine {
    runner: CallbackRunner,
    input: PlanarScratch,
    output: PlanarScratch,
    frames: usize,
    sample_rate: u32,
    options: NullOptions,
    /// Blocks run since the stream opened: the device clock.
    blocks: u64,
    /// The thread's own workgroup (macOS, real-time pacing only).
    interval: Option<WorkInterval>,
    /// How long one block lasts.
    period: Duration,
}

impl NullEngine {
    /// The device thread: runs blocks until `stop` is set or the frame limit
    /// is reached, then hands the engine back.
    fn run(mut self: Box<Self>, stop: &AtomicBool) -> Box<Self> {
        // Set the thread up before the clock starts, so the first block isn't
        // late by the time set-up takes.
        self.runner.prepare_thread();
        let frames = self.frames as u64;
        let began = Instant::now();
        let first_block = self.blocks;
        while !stop.load(Ordering::Acquire) {
            if self
                .options
                .frame_limit
                .is_some_and(|limit| self.blocks * frames >= limit)
            {
                break;
            }
            if self.options.pacing == Pacing::RealTime {
                let due = began
                    + frames_to_duration((self.blocks - first_block) * frames, self.sample_rate);
                let now = Instant::now();
                if due > now {
                    thread::sleep(due - now);
                }
            }

            let timestamp = frames_to_duration(self.blocks * frames, self.sample_rate);
            if let Some(interval) = &self.interval {
                // Only fails if the thread isn't in the interval, which set-up
                // reports; the block runs either way.
                let _ = interval.start(self.period);
            }
            self.runner.device_callback(self.frames);
            let input = self.input.view(self.frames);
            let mut output = self.output.view_mut(self.frames);
            self.runner.run(Some(timestamp), &input, &mut output);
            if let Some(interval) = &self.interval {
                let _ = interval.finish();
            }
            self.blocks += 1;
        }
        self.runner.release_thread();
        self
    }
}

struct NullStream {
    /// The engine while the stream is stopped.
    engine: Option<Box<NullEngine>>,
    /// The device thread while the stream runs; it returns the engine.
    thread: Option<JoinHandle<Box<NullEngine>>>,
    stop: Arc<AtomicBool>,
    info: StreamInfo,
    counters: Arc<Counters>,
    workgroup: Option<Workgroup>,
}

impl Stream for NullStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn start(&mut self) -> Result<(), Error> {
        if self.thread.is_some() {
            return Ok(());
        }
        let engine = self.engine.take().ok_or_else(|| {
            Error::new(
                ErrorKind::Backend,
                "the null device's thread panicked earlier, so the stream can't restart",
            )
        })?;
        self.stop.store(false, Ordering::Release);
        self.counters.request_resync();
        let stop = Arc::clone(&self.stop);
        let thread = thread::Builder::new()
            .name("bl-audio-null".into())
            .spawn(move || engine.run(&stop))
            .map_err(|e| {
                Error::new(
                    ErrorKind::Backend,
                    format!("couldn't start the null device's thread: {e}"),
                )
            })?;
        self.thread = Some(thread);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Error> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        self.stop.store(true, Ordering::Release);
        let engine = thread.join().map_err(|_| {
            Error::new(
                ErrorKind::Backend,
                "the null device's thread panicked; a callback probably did",
            )
        })?;
        self.engine = Some(engine);
        Ok(())
    }

    fn stats(&self) -> StreamStats {
        self.counters.snapshot()
    }

    fn current_buffer_size(&self) -> Option<u32> {
        self.info.buffer_size
    }

    fn workgroup(&self) -> Option<Workgroup> {
        self.workgroup.clone()
    }
}

impl Drop for NullStream {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioBuffer, AudioBufferMut, PortRequest, ProcessContext};
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    /// Counts blocks, checks every block is contiguous and well formed, and
    /// optionally sleeps on one block to make it late.
    struct Checker {
        expected_position: u64,
        frames: usize,
        out_channels: usize,
        in_channels: usize,
        sleep_on_block: Option<(u64, Duration)>,
        blocks: Arc<AtomicU64>,
        faults: Arc<AtomicU64>,
    }

    impl AudioCallback for Checker {
        fn process(
            &mut self,
            ctx: &ProcessContext,
            input: &AudioBuffer<'_>,
            output: &mut AudioBufferMut<'_>,
        ) {
            let block = self.blocks.fetch_add(1, Ordering::Relaxed);
            let well_formed = ctx.position == self.expected_position
                && ctx.frames == self.frames
                && output.channels() == self.out_channels
                && input.channels() == self.in_channels
                && input.iter().flatten().all(|&s| s == 0.0);
            if !well_formed {
                self.faults.fetch_add(1, Ordering::Relaxed);
            }
            self.expected_position += ctx.frames as u64;
            if let Some((on, nap)) = self.sleep_on_block
                && on == block
            {
                // Deliberately breaks the real-time rules to make a late block.
                thread::sleep(nap);
            }
        }
    }

    struct Run {
        stats: StreamStats,
        blocks: u64,
        faults: u64,
        took: Duration,
    }

    fn duplex(buffer: u32) -> StreamRequest {
        StreamRequest {
            output: Some(PortRequest::default()),
            input: Some(PortRequest::default()),
            sample_rate: Some(48_000),
            buffer_size: Some(buffer),
        }
    }

    /// Runs a stream on the null device until its frame limit, and returns
    /// what happened.
    fn run(options: NullOptions, buffer: u32, sleep_on_block: Option<(u64, Duration)>) -> Run {
        let blocks = Arc::new(AtomicU64::new(0));
        let faults = Arc::new(AtomicU64::new(0));
        let checker = Checker {
            expected_position: 0,
            frames: buffer as usize,
            out_channels: 2,
            in_channels: 2,
            sleep_on_block,
            blocks: Arc::clone(&blocks),
            faults: Arc::clone(&faults),
        };
        let limit = options.frame_limit.expect("tests set a frame limit");
        let backend = NullBackend::with_options(options);
        let mut stream = backend
            .open_stream(&duplex(buffer), Box::new(checker))
            .unwrap();
        let began = Instant::now();
        stream.start().unwrap();
        while stream.stats().frames < limit {
            assert!(
                began.elapsed() < Duration::from_secs(120),
                "the stream stalled"
            );
            thread::sleep(Duration::from_millis(1));
        }
        let took = began.elapsed();
        stream.stop().unwrap();
        Run {
            stats: stream.stats(),
            blocks: blocks.load(Ordering::Relaxed),
            faults: faults.load(Ordering::Relaxed),
            took,
        }
    }

    #[test]
    fn five_minutes_at_128_frames_run_clean() {
        let options = NullOptions {
            pacing: Pacing::Free,
            frame_limit: Some(300 * 48_000),
        };
        let run = run(options, 128, None);
        assert_eq!(run.blocks, 112_500);
        assert_eq!(run.faults, 0);
        assert_eq!(run.stats.frames, 14_400_000);
        assert_eq!(run.stats.blocks, 112_500);
        assert_eq!(run.stats.device_callbacks, 112_500);
        assert_eq!(run.stats.min_device_frames, Some(128));
        assert_eq!(run.stats.max_device_frames, Some(128));
        assert_eq!(run.stats.dropouts, 0);
        assert!(run.stats.audio_thread_started);
    }

    #[test]
    fn real_time_pacing_follows_the_clock() {
        // 20 blocks of 10 ms: the last is due 190 ms after the first.
        let options = NullOptions {
            pacing: Pacing::RealTime,
            frame_limit: Some(20 * 480),
        };
        let run = run(options, 480, None);
        assert_eq!(run.stats.blocks, 20);
        assert_eq!(run.faults, 0);
        assert!(
            run.took >= Duration::from_millis(190),
            "took {:?}",
            run.took
        );
        assert_eq!(run.stats.xruns + run.stats.gaps, 0);
    }

    #[test]
    fn a_callback_that_overruns_its_block_is_a_dropout() {
        // Blocks of 64 frames last 1.3 ms; block 3 takes 20 ms.
        let options = NullOptions {
            pacing: Pacing::RealTime,
            frame_limit: Some(10 * 64),
        };
        let run = run(options, 64, Some((3, Duration::from_millis(20))));
        assert!(run.stats.late_blocks >= 1);
        assert!(run.stats.dropouts >= 1);
        assert!(run.stats.longest_block >= Duration::from_millis(20));
        assert_eq!(run.stats.gaps, 0, "the device clock stays contiguous");
        assert_eq!(run.faults, 0);
    }

    #[test]
    fn a_real_time_null_device_runs_on_a_real_time_thread() {
        let options = NullOptions {
            pacing: Pacing::RealTime,
            frame_limit: Some(10 * 128),
        };
        let run = run(options, 128, None);
        assert_eq!(run.faults, 0);
        assert_eq!(run.stats.audio_thread_setups, 1);
        assert_eq!(run.stats.heap_operations, 0);
        let report = run.stats.audio_thread.expect("a set-up report");
        assert!(report.trap_installed);
        assert_eq!(report.denormals_flushed, bl_rt::denormals::SUPPORTED);
        // Linux needs rtprio limits or rtkit, which CI machines may lack.
        #[cfg(any(target_os = "macos", windows))]
        {
            assert!(report.priority.is_realtime(), "{}", report.priority);
            assert!(matches!(report.promotion, crate::Promotion::Raised(_)));
        }
        #[cfg(target_os = "macos")]
        assert_eq!(report.workgroup, crate::WorkgroupJoin::Joined);
    }

    #[test]
    fn a_free_running_null_device_leaves_its_thread_alone() {
        let options = NullOptions {
            pacing: Pacing::Free,
            frame_limit: Some(10 * 128),
        };
        let report = run(options, 128, None)
            .stats
            .audio_thread
            .expect("a set-up report");
        assert_eq!(report.promotion, crate::Promotion::NotRequested);
        assert!(!report.priority.is_realtime());
        assert_eq!(report.workgroup, crate::WorkgroupJoin::None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_real_time_null_stream_has_its_own_work_interval() {
        let stream = NullBackend::new()
            .open_stream(&duplex(256), Box::new(Silence))
            .unwrap();
        let info = stream.info().workgroup.clone().expect("a workgroup");
        assert_eq!(info.label, "the work interval \"Beatlayer null device\"");
        assert!(stream.workgroup().is_some());

        let free = NullBackend::with_options(NullOptions {
            pacing: Pacing::Free,
            frame_limit: None,
        });
        let stream = free.open_stream(&duplex(256), Box::new(Silence)).unwrap();
        assert!(stream.info().workgroup.is_none());
    }

    #[cfg(target_os = "macos")]
    struct Silence;

    #[cfg(target_os = "macos")]
    impl AudioCallback for Silence {
        fn process(&mut self, _: &ProcessContext, _: &AudioBuffer<'_>, _: &mut AudioBufferMut<'_>) {
        }
    }

    #[test]
    fn free_pacing_has_no_deadlines() {
        let options = NullOptions {
            pacing: Pacing::Free,
            frame_limit: Some(10 * 64),
        };
        let run = run(options, 64, Some((3, Duration::from_millis(5))));
        assert_eq!(run.stats.dropouts, 0);
    }

    #[test]
    fn a_stream_resumes_after_stop() {
        let backend = NullBackend::with_options(NullOptions {
            pacing: Pacing::Free,
            frame_limit: None,
        });
        let blocks = Arc::new(AtomicU64::new(0));
        let faults = Arc::new(AtomicU64::new(0));
        let checker = Checker {
            expected_position: 0,
            frames: 64,
            out_channels: 2,
            in_channels: 2,
            sleep_on_block: None,
            blocks: Arc::clone(&blocks),
            faults: Arc::clone(&faults),
        };
        let mut stream = backend.open_stream(&duplex(64), Box::new(checker)).unwrap();
        for round in 1..=2 {
            stream.start().unwrap();
            while blocks.load(Ordering::Relaxed) < round * 100 {
                thread::sleep(Duration::from_millis(1));
            }
            stream.stop().unwrap();
        }
        let stats = stream.stats();
        assert!(stats.blocks >= 200);
        assert_eq!(stats.blocks, blocks.load(Ordering::Relaxed));
        assert_eq!(
            faults.load(Ordering::Relaxed),
            0,
            "positions stay contiguous"
        );
        assert_eq!(stats.dropouts, 0);
    }

    #[test]
    fn requests_the_device_cannot_meet_are_refused() {
        let backend = NullBackend::new();
        let open = |request: StreamRequest| {
            let checker = Checker {
                expected_position: 0,
                frames: 0,
                out_channels: 0,
                in_channels: 0,
                sleep_on_block: None,
                blocks: Arc::default(),
                faults: Arc::default(),
            };
            backend
                .open_stream(&request, Box::new(checker))
                .err()
                .map(|e| e.kind())
        };
        let output = |channels| {
            Some(PortRequest {
                device: DeviceSelector::Default,
                channels,
            })
        };

        assert_eq!(
            open(StreamRequest::default()),
            Some(ErrorKind::InvalidInput)
        );
        assert_eq!(
            open(StreamRequest {
                sample_rate: Some(12_345),
                ..StreamRequest::output(DeviceSelector::Default)
            }),
            Some(ErrorKind::UnsupportedConfig)
        );
        assert_eq!(
            open(StreamRequest {
                buffer_size: Some(8),
                ..StreamRequest::output(DeviceSelector::Default)
            }),
            Some(ErrorKind::UnsupportedConfig)
        );
        assert_eq!(
            open(StreamRequest {
                output: output(Some(3)),
                ..StreamRequest::default()
            }),
            Some(ErrorKind::UnsupportedConfig)
        );
        assert_eq!(
            open(StreamRequest::output(DeviceSelector::Id(DeviceId::new(
                BackendKind::Null,
                "other"
            )))),
            Some(ErrorKind::DeviceNotFound)
        );
        assert_eq!(
            open(StreamRequest::output(DeviceSelector::Id(
                NullBackend::device_id()
            ))),
            None
        );
    }

    #[test]
    fn stream_info_reports_what_runs() {
        let backend = NullBackend::new();
        let checker = Checker {
            expected_position: 0,
            frames: 0,
            out_channels: 0,
            in_channels: 0,
            sleep_on_block: None,
            blocks: Arc::default(),
            faults: Arc::default(),
        };
        let stream = backend
            .open_stream(
                &StreamRequest {
                    output: Some(PortRequest {
                        device: DeviceSelector::Default,
                        channels: Some(1),
                    }),
                    ..StreamRequest::default()
                },
                Box::new(checker),
            )
            .unwrap();
        let info = stream.info();
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.buffer_size, Some(256));
        assert_eq!(info.requested_buffer_size, None);
        assert_eq!((info.output_channels, info.input_channels), (1, 0));
        assert!(info.input_device.is_none());
        assert_eq!(
            info.output_device.as_ref().map(|d| d.id.clone()),
            Some(NullBackend::device_id())
        );
    }
}
