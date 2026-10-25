//! The null device: runs the callback on a timer thread without hardware, so
//! tests and CI exercise the whole stream path on any machine.
//!
//! A null backend can also simulate what real hardware does to a stream
//! (WP-1.13): [`NullBackend::with_devices`] gives it several devices and a
//! [`NullControl`] that unplugs and replugs them, changes their sample rates,
//! stalls their streams, and puts the machine to sleep and wakes it. Each
//! stream's thread notices on its next block, as a backend would, and
//! reports through its [`StreamStats`] what `cpal` reports for the same
//! event on macOS: a lost device, a stream invalidated by a rate change, or
//! a default device that changed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bl_core::clock::TestClock;
use bl_rt::{RtThreadConfig, WorkInterval};

use crate::buffer::PlanarScratch;
use crate::power::{PowerEvent, PowerMonitor};
use crate::runner::{CallbackRunner, frames_to_duration};
use crate::stats::Counters;
use crate::{
    AudioCallback, Backend, BackendKind, BufferSizes, DeviceId, DeviceInfo, DeviceSelector, Error,
    ErrorKind, OpenDevice, PortRequest, STANDARD_SAMPLE_RATES, SampleFormat, Stream, StreamInfo,
    StreamRequest, StreamStats, Workgroup, WorkgroupInfo,
};

const LOCAL: &str = "null";
const NAME: &str = "Null device";
const CHANNELS: u16 = 2;
const DEFAULT_RATE: u32 = 48_000;
const DEFAULT_BUFFER: u32 = 256;
const MIN_BUFFER: u32 = 16;
const MAX_BUFFER: u32 = 8192;
/// How often a stream that isn't calling back looks again.
const IDLE_PERIOD: Duration = Duration::from_millis(1);

/// A cable between the interface's outputs and inputs that works out what
/// comes back from what was played: given every output channel's block, it
/// fills every input channel's next block, one block later (WP-7.17). A test
/// patches a simulated oscillator or any other hardware with it.
#[derive(Clone)]
pub struct Cable(Arc<Mutex<Patch>>);

/// What a cable runs after each block.
type Patch = dyn FnMut(&[Vec<f32>], &mut [Vec<f32>]) + Send;

impl Cable {
    /// A cable that runs `patch` after each block with the block the outputs
    /// played and the block the inputs will hear next, both one `Vec` of
    /// frames per channel.
    pub fn new(patch: impl FnMut(&[Vec<f32>], &mut [Vec<f32>]) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(patch)))
    }
}

impl std::fmt::Debug for Cable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cable")
    }
}

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
    /// No block runs until a test asks, through
    /// [`NullControl::run_blocks`] or [`NullControl::run_frames`] (T199).
    /// A test clock advances by each block's length, so notes it plays land
    /// on their frames however busy the machine is.
    Manual,
}

/// Settings for the null backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NullOptions {
    /// How blocks are paced.
    pub pacing: Pacing,
    /// If set, a running stream stops by itself after this many frames.
    pub frame_limit: Option<u64>,
    /// How many input and output channels each device has. Zero — and the
    /// default — means [`CHANNELS`], two.
    pub channels: u16,
    /// The block after which the stream reports a dropout, as a device does
    /// when it underruns: none for none (WP-5.35).
    pub dropout_after: Option<u64>,
}

impl Default for NullOptions {
    fn default() -> Self {
        Self {
            pacing: Pacing::default(),
            frame_limit: None,
            channels: CHANNELS,
            dropout_after: None,
        }
    }
}

impl NullOptions {
    /// The channels a device has: [`channels`](Self::channels), or
    /// [`CHANNELS`] if it's zero.
    #[must_use]
    pub fn device_channels(&self) -> u16 {
        if self.channels == 0 {
            CHANNELS
        } else {
            self.channels
        }
    }
}

/// One simulated device.
#[derive(Debug)]
struct SimDevice {
    /// The backend's own ID for it, after `null:`.
    local: String,
    name: String,
    present: AtomicBool,
    /// The rate it runs at now.
    rate: AtomicU32,
    /// Bumped each time [`NullControl::set_sample_rate`] changes the rate
    /// under the device's streams.
    rate_changes: AtomicU64,
}

/// One input channel's test tone: a frequency in hertz and a peak
/// amplitude, as `f32` bits, written by [`NullControl::set_input_tone`] and
/// read by the device's thread with relaxed atomics.
#[derive(Debug)]
struct InputTone {
    hz: AtomicU32,
    amplitude: AtomicU32,
}

/// Hand pacing: a test asks for blocks, and the stream's own thread runs
/// them, one at a time, until the count is reached.
#[derive(Debug, Default)]
struct ManualState {
    /// Blocks asked for since the stream started.
    requested: u64,
    /// Blocks finished since the stream started.
    completed: u64,
    /// Whether a stream is running to run them.
    active: bool,
    /// Whether that stream is stopping, so a waiter gives up.
    stopping: bool,
}

#[derive(Debug, Default)]
struct ManualPacer {
    state: Mutex<ManualState>,
    /// The stream's thread waits here for a block to be asked for.
    ready: Condvar,
    /// A test waits here for the blocks it asked for to finish.
    done: Condvar,
}

impl ManualPacer {
    /// Says a stream is about to run, and forgets requests from before.
    fn start(&self) {
        let mut state = self.lock();
        assert!(!state.active, "two manual streams share one null backend");
        state.active = true;
        state.stopping = false;
        state.requested = state.completed;
    }

    /// Says the stream has stopped, waking anyone waiting for its blocks.
    fn stop(&self) {
        self.lock().stopping = true;
        self.ready.notify_all();
        self.done.notify_all();
    }

    fn stopped(&self) {
        let mut state = self.lock();
        state.active = false;
        state.stopping = false;
    }

    /// The stream's thread asks for a block. Returns whether to run one;
    /// false when the stream is stopping.
    fn wait_for_block(&self, stop: &AtomicBool) -> bool {
        let mut state = self.lock();
        while state.requested <= state.completed && !state.stopping && !stop.load(Ordering::Acquire)
        {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        !state.stopping && !stop.load(Ordering::Acquire)
    }

    /// Counts a block the stream ran and wakes the test waiting for it.
    fn finished_block(&self) {
        self.lock().completed += 1;
        self.done.notify_all();
    }

    /// Runs exactly `blocks` blocks on the stream's thread, and returns once
    /// they have run.
    fn run(&self, blocks: u64) {
        let mut state = self.lock();
        assert!(state.active, "start the null stream before running blocks");
        if blocks == 0 {
            return;
        }
        state.requested = state.requested.saturating_add(blocks);
        let target = state.requested;
        self.ready.notify_all();
        while state.completed < target {
            assert!(
                !state.stopping,
                "the null stream stopped before the blocks finished"
            );
            state = self
                .done
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ManualState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What one null backend's devices and streams share.
#[derive(Debug)]
struct World {
    /// Fixed when the backend is made, so a stream's thread reads it without
    /// a lock.
    devices: Vec<SimDevice>,
    /// How many input and output channels each device has.
    channels: u16,
    /// The test tone each input channel carries, silent by default
    /// (WP-2.13).
    input: Vec<InputTone>,
    /// While set, no stream calls back: the machine sleeps.
    asleep: AtomicBool,
    /// Bumped by [`NullControl::stall`]: streams opened before stop calling
    /// back for good.
    stalls: AtomicU64,
    /// Power events for [`NullPower`], oldest first, and when the machine
    /// went to sleep. Never touched by a stream's thread.
    power: Mutex<(VecDeque<PowerEvent>, Option<Instant>)>,
    /// With manual pacing, the clock a test advances with the blocks (T199).
    clock: Option<TestClock>,
    /// With manual pacing, the blocks a test asks for.
    pacer: Option<Arc<ManualPacer>>,
    /// The frames in one block of the running manual stream, for
    /// [`NullControl::run_frames`].
    manual_frames: AtomicU32,
}

impl World {
    fn new(devices: &[(&str, &str)], channels: u16, pacing: Pacing) -> Self {
        let manual = pacing == Pacing::Manual;
        assert!(!devices.is_empty(), "a null backend needs a device");
        for (i, (local, _)) in devices.iter().enumerate() {
            assert!(
                devices[..i].iter().all(|(other, _)| other != local),
                "two null devices are called `{local}`"
            );
        }
        Self {
            devices: devices
                .iter()
                .map(|&(local, name)| SimDevice {
                    local: local.to_owned(),
                    name: name.to_owned(),
                    present: AtomicBool::new(true),
                    rate: AtomicU32::new(DEFAULT_RATE),
                    rate_changes: AtomicU64::new(0),
                })
                .collect(),
            channels,
            input: (0..channels)
                .map(|_| InputTone {
                    hz: AtomicU32::new(0.0f32.to_bits()),
                    amplitude: AtomicU32::new(0.0f32.to_bits()),
                })
                .collect(),
            asleep: AtomicBool::new(false),
            stalls: AtomicU64::new(0),
            power: Mutex::new((VecDeque::new(), None)),
            clock: manual.then(TestClock::new),
            pacer: manual.then(|| Arc::new(ManualPacer::default())),
            manual_frames: AtomicU32::new(0),
        }
    }

    /// The system default: the first device present. Allocation-free, so a
    /// stream's thread can ask.
    fn default_device(&self) -> Option<usize> {
        self.devices
            .iter()
            .position(|d| d.present.load(Ordering::Acquire))
    }

    fn index(&self, local: &str) -> usize {
        self.devices
            .iter()
            .position(|d| d.local == local)
            .unwrap_or_else(|| panic!("the null backend has no device `{local}`"))
    }

    fn id(&self, index: usize) -> DeviceId {
        DeviceId::new(BackendKind::Null, &self.devices[index].local)
    }

    fn push_power(&self, event: PowerEvent) {
        let mut power = self.power.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            PowerEvent::Sleeping => power.1 = Some(Instant::now()),
            PowerEvent::Woke { .. } => power.1 = None,
        }
        power.0.push_back(event);
    }
}

/// A backend whose devices run the callback on a timer: two input and two
/// output channels by default (more with [`NullOptions::channels`]), the
/// standard sample rates, and buffers of 16 to 8,192 frames. Input is
/// silence until a test tone is set.
///
/// Made with [`new`](Self::new) or [`with_options`](Self::with_options), it
/// has one device, `null:null`. Made with
/// [`with_devices`](Self::with_devices), it has the devices asked for, and a
/// [`NullControl`] that changes them under their streams. Clones share their
/// devices.
#[derive(Debug, Clone)]
pub struct NullBackend {
    options: NullOptions,
    world: Arc<World>,
    /// The input and output latency, in frames, that a stream reports in its
    /// [`StreamInfo`] (WP-2.13).
    latency: (u32, u32),
    /// If set, each input channel carries the output channel of the same
    /// index this many frames late instead of its test tone, as a cable from
    /// the interface's output to its input does (WP-2.13). It can't be less
    /// than a block, because a block's input is read before its output is
    /// made; a stream with a bigger buffer is refused.
    loopback_delay: Option<u32>,
    /// If set, each input channel carries what the cable works out from the
    /// outputs a block before (WP-7.17).
    cable: Option<Cable>,
}

impl Default for NullBackend {
    fn default() -> Self {
        Self::with_options(NullOptions::default())
    }
}

impl NullBackend {
    /// A null backend paced in real time.
    pub fn new() -> Self {
        Self::default()
    }

    /// This backend with streams that report `input` and `output` frames of
    /// latency (WP-2.13).
    #[must_use]
    pub fn with_latency(mut self, input: u32, output: u32) -> Self {
        self.latency = (input, output);
        self
    }

    /// This backend with each input channel carrying the output channel of
    /// the same index `delay` frames late (WP-2.13).
    #[must_use]
    pub fn with_loopback(mut self, delay: u32) -> Self {
        self.loopback_delay = Some(delay);
        self
    }

    /// This backend with the inputs carrying what `cable` works out from the
    /// outputs a block before, in place of the test tones (WP-7.17).
    #[must_use]
    pub fn with_cable(mut self, cable: Cable) -> Self {
        self.cable = Some(cable);
        self
    }

    /// A null backend with these settings.
    pub fn with_options(options: NullOptions) -> Self {
        Self::with_devices(options, &[(LOCAL, NAME)]).0
    }

    /// A null backend with these devices, as `(ID, name)` pairs: each ID
    /// becomes `null:<ID>`. The first device present is the system default.
    /// The [`NullControl`] simulates what hardware does to them.
    ///
    /// # Panics
    ///
    /// If `devices` is empty or names an ID twice.
    pub fn with_devices(options: NullOptions, devices: &[(&str, &str)]) -> (Self, NullControl) {
        let world = Arc::new(World::new(
            devices,
            options.device_channels(),
            options.pacing,
        ));
        let control = NullControl {
            world: Arc::clone(&world),
        };
        (
            Self {
                options,
                world,
                latency: (0, 0),
                loopback_delay: None,
                cable: None,
            },
            control,
        )
    }

    /// The ID of the device a plain null backend has.
    pub fn device_id() -> DeviceId {
        DeviceId::new(BackendKind::Null, LOCAL)
    }

    fn describe(&self, index: usize, default: Option<usize>) -> DeviceInfo {
        let device = &self.world.devices[index];
        DeviceInfo {
            id: self.world.id(index),
            name: device.name.clone(),
            is_default_output: default == Some(index),
            is_default_input: default == Some(index),
            output_channels: self.world.channels,
            input_channels: self.world.channels,
            sample_rates: STANDARD_SAMPLE_RATES.to_vec(),
            current_sample_rate: Some(device.rate.load(Ordering::Acquire)),
            buffer_sizes: BufferSizes::Range {
                min: MIN_BUFFER,
                max: MAX_BUFFER,
            },
            default_buffer_size: Some(DEFAULT_BUFFER),
        }
    }

    /// The device a selector names, if it's present.
    fn resolve(&self, selector: &DeviceSelector) -> Result<usize, Error> {
        let index = match selector {
            DeviceSelector::Default => self.world.default_device(),
            DeviceSelector::Id(id) if id.backend() == BackendKind::Null => self
                .world
                .devices
                .iter()
                .position(|d| d.local == id.local())
                .filter(|&i| self.world.devices[i].present.load(Ordering::Acquire)),
            DeviceSelector::Id(_) => None,
        };
        index.ok_or_else(|| {
            let what = match selector {
                DeviceSelector::Default => "default device".to_owned(),
                DeviceSelector::Id(id) => format!("device `{id}`"),
            };
            Error::new(
                ErrorKind::DeviceNotFound,
                format!("the null backend has no {what} now"),
            )
        })
    }
}

/// Checks one direction of a request and returns its channel count, or 0 if
/// the request doesn't ask for that direction.
fn channels_for(port: Option<&PortRequest>, available: u16, direction: &str) -> Result<u16, Error> {
    let Some(port) = port else {
        return Ok(0);
    };
    let channels = port.channels.unwrap_or(available);
    if !(1..=available).contains(&channels) {
        return Err(Error::new(
            ErrorKind::UnsupportedConfig,
            format!("the null device has 1 to {available} {direction} channels, not {channels}"),
        ));
    }
    Ok(channels)
}

impl Backend for NullBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Null
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, Error> {
        let default = self.world.default_device();
        Ok((0..self.world.devices.len())
            .filter(|&i| self.world.devices[i].present.load(Ordering::Acquire))
            .map(|i| self.describe(i, default))
            .collect())
    }

    fn open_stream(
        &self,
        request: &StreamRequest,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn Stream>, Error> {
        let Some(selector) = request
            .output
            .as_ref()
            .or(request.input.as_ref())
            .map(|port| &port.device)
        else {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "a stream needs output, input, or both",
            ));
        };
        let index = self.resolve(selector)?;
        if let (Some(output), Some(input)) = (&request.output, &request.input)
            && self.resolve(&input.device)? != self.resolve(&output.device)?
        {
            return Err(Error::new(
                ErrorKind::UnsupportedConfig,
                "a null stream's input and output are on one device",
            ));
        }
        let output_channels = channels_for(request.output.as_ref(), self.world.channels, "output")?;
        let input_channels = channels_for(request.input.as_ref(), self.world.channels, "input")?;

        let device = &self.world.devices[index];
        let sample_rate = request
            .sample_rate
            .unwrap_or_else(|| device.rate.load(Ordering::Acquire));
        if !STANDARD_SAMPLE_RATES.contains(&sample_rate) {
            return Err(Error::new(
                ErrorKind::UnsupportedConfig,
                format!("the null device doesn't run at {sample_rate} Hz"),
            ));
        }
        let frames = request.buffer_size.unwrap_or(DEFAULT_BUFFER);
        if let Some(delay) = self.loopback_delay
            && delay < frames
        {
            return Err(Error::new(
                ErrorKind::UnsupportedConfig,
                format!("a loopback of {delay} frames is shorter than the {frames}-frame buffer"),
            ));
        }
        if !(MIN_BUFFER..=MAX_BUFFER).contains(&frames) {
            return Err(Error::new(
                ErrorKind::UnsupportedConfig,
                format!(
                    "the null device takes buffers of {MIN_BUFFER} to {MAX_BUFFER} frames, not {frames}"
                ),
            ));
        }
        // As on Core Audio, opening a device at another rate sets the
        // device's own rate. Its other streams aren't told.
        device.rate.store(sample_rate, Ordering::Release);
        let follows_default = *selector == DeviceSelector::Default;

        // Paced in real time, the device's thread works like a sound card's:
        // real-time priority and, on macOS, a work interval of its own, which
        // it starts and finishes around each block. Free pacing runs blocks
        // back to back without sleeping, which at real-time priority would
        // starve other threads on its core (and on Linux, run past the CPU
        // time rtkit allows a real-time thread).
        let realtime = self.options.pacing == Pacing::RealTime;
        let period = frames_to_duration(u64::from(frames), sample_rate);
        if self.options.pacing == Pacing::Manual {
            self.world.manual_frames.store(frames, Ordering::Release);
        }
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
            counters: Arc::clone(&counters),
            input: PlanarScratch::new(input_channels.into(), frames as usize),
            output: PlanarScratch::new(output_channels.into(), frames as usize),
            input_phases: vec![0.0; input_channels.into()],
            loopback: self.loopback_delay.map(|delay| {
                (0..input_channels.min(output_channels))
                    .map(|_| VecDeque::from(vec![0.0; delay as usize]))
                    .collect()
            }),
            cable: self.cable.clone(),
            cable_input: vec![vec![0.0; frames as usize]; usize::from(input_channels)],
            cable_output: vec![vec![0.0; frames as usize]; usize::from(output_channels)],
            frames: frames as usize,
            sample_rate,
            options: self.options,
            blocks: 0,
            interval,
            period,
            clock: self.world.clock.clone(),
            pacer: self.world.pacer.clone(),
            world: Arc::clone(&self.world),
            device: index,
            follows_default,
            rate_changes_seen: device.rate_changes.load(Ordering::Acquire),
            stalls_seen: self.world.stalls.load(Ordering::Acquire),
        };
        let open = || OpenDevice {
            id: self.world.id(index),
            name: device.name.clone(),
        };
        let info = StreamInfo {
            backend: BackendKind::Null,
            output_device: (output_channels > 0).then(open),
            input_device: (input_channels > 0).then(open),
            follows_default,
            sample_rate,
            output_channels,
            input_channels,
            sample_format: SampleFormat::F32,
            requested_buffer_size: request.buffer_size,
            buffer_size: Some(frames),
            max_block_frames: frames,
            input_latency: self.latency.0,
            output_latency: self.latency.1,
            workgroup: workgroup.as_ref().map(WorkgroupInfo::of),
        };
        Ok(Box::new(NullStream {
            engine: Some(Box::new(engine)),
            thread: None,
            stop: Arc::new(AtomicBool::new(false)),
            info,
            counters,
            workgroup,
            pacer: self.world.pacer.clone(),
        }))
    }
}

/// Simulates what hardware does to a null backend's devices, for tests of
/// what happens to a stream when its device goes away, changes rate, stops
/// responding, or the machine sleeps (WP-1.13). Each stream notices on its
/// next block. Clones control the same devices.
#[derive(Debug, Clone)]
pub struct NullControl {
    world: Arc<World>,
}

impl NullControl {
    /// Unplugs a device: it leaves the device list, its streams stop calling
    /// back and report it lost, and opening it fails until it's plugged back
    /// in. A stream following the default device moves to the next device
    /// present instead, and reports a device change.
    ///
    /// # Panics
    ///
    /// If the backend has no device with this ID, here and below.
    pub fn unplug(&self, local: &str) {
        let index = self.world.index(local);
        self.world.devices[index]
            .present
            .store(false, Ordering::Release);
    }

    /// Plugs a device back in. Streams it had before stay dead, as they do on
    /// real hardware; new ones open.
    pub fn replug(&self, local: &str) {
        let index = self.world.index(local);
        self.world.devices[index]
            .present
            .store(true, Ordering::Release);
    }

    /// Whether a device is plugged in.
    #[must_use]
    pub fn is_present(&self, local: &str) -> bool {
        self.world.devices[self.world.index(local)]
            .present
            .load(Ordering::Acquire)
    }

    /// Changes a device's sample rate, as another program or the system
    /// might. A stream on that device at another rate stops and reports
    /// that it must be rebuilt. (A stream following the default device keeps
    /// running, as Core Audio converts for it.)
    pub fn set_sample_rate(&self, local: &str, rate: u32) {
        let device = &self.world.devices[self.world.index(local)];
        if device.rate.swap(rate, Ordering::AcqRel) != rate {
            device.rate_changes.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// A device's sample rate now.
    #[must_use]
    pub fn sample_rate(&self, local: &str) -> u32 {
        self.world.devices[self.world.index(local)]
            .rate
            .load(Ordering::Acquire)
    }

    /// Sets the sine the null device's input `channel` carries, counted from
    /// 0, for tests of what a device's input does (WP-2.13): `hz` hertz at
    /// `amplitude` peak (brought within 0 to 1). A stream's channels that
    /// aren't set carry silence, and a new stream starts every tone at phase
    /// zero, so two channels set alike stay in phase. A frequency or
    /// amplitude of zero makes the channel silent.
    ///
    /// # Panics
    ///
    /// If the backend has no input channel with this index.
    pub fn set_input_tone(&self, channel: u16, hz: f64, amplitude: f32) {
        let tone = &self.world.input[usize::from(channel)];
        tone.hz.store((hz as f32).to_bits(), Ordering::Relaxed);
        tone.amplitude
            .store(amplitude.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Makes every stream open now stop calling back for good, without
    /// reporting anything, as a driver that has stopped responding does.
    /// Streams opened afterwards run.
    pub fn stall(&self) {
        self.world.stalls.fetch_add(1, Ordering::AcqRel);
    }

    /// Puts the machine to sleep: no stream calls back until
    /// [`wake`](Self::wake), and [`NullPower`] reports
    /// [`PowerEvent::Sleeping`], as an operating system's notification would.
    pub fn sleep(&self) {
        self.world.asleep.store(true, Ordering::Release);
        self.world.push_power(PowerEvent::Sleeping);
    }

    /// Wakes the machine: streams that were open call back again, and
    /// [`NullPower`] reports [`PowerEvent::Woke`] with how long it slept.
    pub fn wake(&self) {
        let slept = {
            let power = self
                .world
                .power
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            power.1.map(|since| since.elapsed())
        };
        self.world.asleep.store(false, Ordering::Release);
        self.world.push_power(PowerEvent::Woke { slept });
    }

    /// Reports a power event through [`NullPower`] without changing any
    /// device: for example a wake noticed afterwards, with streams that kept
    /// running.
    pub fn notify(&self, event: PowerEvent) {
        self.world.push_power(event);
    }

    /// A [`PowerMonitor`] that reports what [`sleep`](Self::sleep),
    /// [`wake`](Self::wake), and [`notify`](Self::notify) do. Give a
    /// supervisor one; two would share the events between them.
    #[must_use]
    pub fn power(&self) -> NullPower {
        NullPower {
            world: Arc::clone(&self.world),
        }
    }

    /// The clock manual pacing advances by each block's length (T199).
    /// `None` unless the backend is paced [`Manual`](Pacing::Manual).
    #[must_use]
    pub fn clock(&self) -> Option<TestClock> {
        self.world.clock.clone()
    }

    /// Runs exactly `blocks` blocks on the stream's own thread, and returns
    /// once they have run (T199). The stream must be started first, and the
    /// backend paced [`Manually`](Pacing::Manual); the test clock moves on by
    /// each block's length as they run.
    ///
    /// # Panics
    ///
    /// If the backend isn't paced manually, or its stream isn't running.
    pub fn run_blocks(&self, blocks: u64) {
        self.pacer().run(blocks);
    }

    /// Runs as many whole blocks as `frames` need on the stream's own thread,
    /// and returns once they have run (T199). The last block runs in full, so
    /// a little more than `frames` may play. Panics as
    /// [`run_blocks`](Self::run_blocks) does.
    pub fn run_frames(&self, frames: u64) {
        let block = u64::from(self.world.manual_frames.load(Ordering::Acquire));
        assert!(block > 0, "start the null stream before running frames");
        self.run_blocks(frames.div_ceil(block));
    }

    fn pacer(&self) -> &ManualPacer {
        self.world
            .pacer
            .as_deref()
            .expect("the null backend isn't paced manually")
    }
}

/// The power events a [`NullControl`] simulates, as a [`PowerMonitor`].
#[derive(Debug)]
pub struct NullPower {
    world: Arc<World>,
}

impl PowerMonitor for NullPower {
    fn poll(&mut self) -> Option<PowerEvent> {
        self.world
            .power
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .pop_front()
    }
}

/// What a null stream's thread does before a block.
enum Health {
    /// Run the block.
    Run,
    /// Don't call back now; look again shortly.
    Idle,
    /// The stream is dead.
    End,
}

/// Everything the null device's thread owns while it runs.
struct NullEngine {
    runner: CallbackRunner,
    counters: Arc<Counters>,
    input: PlanarScratch,
    output: PlanarScratch,
    /// Each input channel's test tone phase, carried across blocks.
    input_phases: Vec<f64>,
    /// With a loopback, the last `delay` frames each output channel played,
    /// oldest first, which the same input channel reads.
    loopback: Option<Vec<VecDeque<f32>>>,
    /// With a cable, what it worked out for the inputs' next block, and the
    /// block the outputs played.
    cable: Option<Cable>,
    cable_input: Vec<Vec<f32>>,
    cable_output: Vec<Vec<f32>>,
    frames: usize,
    sample_rate: u32,
    options: NullOptions,
    /// Blocks run since the stream opened: the device clock.
    blocks: u64,
    /// The thread's own workgroup (macOS, real-time pacing only).
    interval: Option<WorkInterval>,
    /// How long one block lasts.
    period: Duration,
    /// Manual pacing's clock (T199), which moves on by `period` a block.
    clock: Option<TestClock>,
    /// Manual pacing's block requests (T199).
    pacer: Option<Arc<ManualPacer>>,
    world: Arc<World>,
    /// The device the stream runs on now.
    device: usize,
    /// Whether the stream follows the default device.
    follows_default: bool,
    rate_changes_seen: u64,
    stalls_seen: u64,
}

impl NullEngine {
    /// The device thread: runs blocks until `stop` is set, the frame limit
    /// is reached, or the device goes, then hands the engine back.
    fn run(mut self: Box<Self>, stop: &AtomicBool) -> Box<Self> {
        // Set the thread up before the clock starts, so the first block isn't
        // late by the time set-up takes.
        self.runner.prepare_thread();
        let frames = self.frames as u64;
        let mut began = Instant::now();
        let mut first_block = self.blocks;
        let mut idle = false;
        while !stop.load(Ordering::Acquire) {
            if self
                .options
                .frame_limit
                .is_some_and(|limit| self.blocks * frames >= limit)
            {
                break;
            }
            match self.health() {
                Health::Run => {}
                Health::Idle => {
                    idle = true;
                    thread::sleep(IDLE_PERIOD);
                    continue;
                }
                Health::End => break,
            }
            if idle {
                // Calling back again after a pause: start the clock afresh
                // rather than catch up on the blocks the pause skipped.
                idle = false;
                began = Instant::now();
                first_block = self.blocks;
                self.counters.request_resync();
            }
            if let Some(pacer) = &self.pacer {
                // Paced by hand (T199): wait for a test to ask for this block,
                // then move the test clock to the block's start.
                if !pacer.wait_for_block(stop) {
                    break;
                }
                if let Some(clock) = &self.clock {
                    clock.advance(self.period);
                }
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
            if let Some(after) = self.options.dropout_after
                && self.blocks == after
            {
                // The device underruns, as it does when the machine is too
                // busy (WP-5.35).
                self.counters.record_xrun();
            }
            if let Some(interval) = &self.interval {
                // Only fails if the thread isn't in the interval, which set-up
                // reports; the block runs either way.
                let _ = interval.start(self.period);
            }
            self.runner.device_callback(self.frames);
            self.fill_input();
            let input = self.input.view(self.frames);
            let mut output = self.output.view_mut(self.frames);
            self.runner.run(Some(timestamp), &input, &mut output);
            if let Some(lines) = &mut self.loopback {
                for (line, played) in lines.iter_mut().zip(output.as_buffer().iter()) {
                    line.extend(played);
                }
            }
            if let Some(cable) = &self.cable {
                for (to, played) in self.cable_output.iter_mut().zip(output.as_buffer().iter()) {
                    to.clear();
                    to.extend(played);
                }
                for input in &mut self.cable_input {
                    input.resize(self.frames, 0.0);
                }
                let mut patch = cable.0.lock().unwrap_or_else(PoisonError::into_inner);
                patch(&self.cable_output, &mut self.cable_input);
            }
            if let Some(interval) = &self.interval {
                let _ = interval.finish();
            }
            self.blocks += 1;
            if let Some(pacer) = &self.pacer {
                pacer.finished_block();
            }
        }
        self.runner.release_thread();
        self
    }

    /// Fills the input scratch with the test tones (WP-2.13): each input
    /// channel carries the sine [`NullControl::set_input_tone`] set, silent
    /// unless one was, with its phase carried across blocks so a tone is
    /// continuous. Real-time safe: reads atomics, writes the scratch the
    /// stream made.
    fn fill_input(&mut self) {
        let rate = f64::from(self.sample_rate.max(1));
        let frames = self.frames;
        let mut view = self.input.view_mut(frames);
        if self.cable.is_some() {
            for (channel, next) in view.iter_mut().zip(&self.cable_input) {
                channel.copy_from_slice(&next[..frames]);
            }
            return;
        }
        if let Some(lines) = &mut self.loopback {
            for (index, channel) in view.iter_mut().enumerate() {
                match lines.get_mut(index) {
                    Some(line) => channel
                        .iter_mut()
                        .for_each(|s| *s = line.pop_front().unwrap_or(0.0)),
                    None => channel.fill(0.0),
                }
            }
            return;
        }
        for (index, channel) in view.iter_mut().enumerate() {
            let tone = &self.world.input[index];
            let hz = f64::from(f32::from_bits(tone.hz.load(Ordering::Relaxed)));
            let amplitude = f32::from_bits(tone.amplitude.load(Ordering::Relaxed));
            let phase = &mut self.input_phases[index];
            if hz <= 0.0 || amplitude == 0.0 {
                channel.fill(0.0);
                continue;
            }
            let step = hz / rate;
            for sample in channel.iter_mut() {
                *sample = (std::f64::consts::TAU * *phase).sin() as f32 * amplitude;
                *phase += step;
                if *phase >= 1.0 {
                    *phase -= 1.0;
                }
            }
        }
    }

    /// What the simulated hardware lets the stream do now, reporting what a
    /// backend would. Atomic loads only.
    fn health(&mut self) -> Health {
        let world = &self.world;
        if world.stalls.load(Ordering::Acquire) != self.stalls_seen
            || world.asleep.load(Ordering::Acquire)
        {
            return Health::Idle;
        }
        if self.follows_default {
            // The default device's rate doesn't matter: Core Audio converts
            // for a stream that follows the default.
            match world.default_device() {
                Some(device) if device == self.device => {}
                Some(device) => {
                    self.device = device;
                    self.counters.record_device_change();
                }
                None => {
                    self.counters.record_device_lost();
                    return Health::End;
                }
            }
            return Health::Run;
        }
        let device = &world.devices[self.device];
        if !device.present.load(Ordering::Acquire) {
            self.counters.record_device_lost();
            Health::End
        } else if device.rate_changes.load(Ordering::Acquire) != self.rate_changes_seen {
            self.counters.record_invalidated();
            Health::End
        } else {
            Health::Run
        }
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
    /// Manual pacing's block requests (T199).
    pacer: Option<Arc<ManualPacer>>,
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
        if let Some(pacer) = &self.pacer {
            pacer.start();
        }
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
        if let Some(pacer) = &self.pacer {
            pacer.stop();
        }
        let engine = thread.join().map_err(|_| {
            Error::new(
                ErrorKind::Backend,
                "the null device's thread panicked; a callback probably did",
            )
        })?;
        if let Some(pacer) = &self.pacer {
            pacer.stopped();
        }
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
            channels: 2,
            dropout_after: None,
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
            channels: 2,
            dropout_after: None,
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
            channels: 2,
            dropout_after: None,
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
            channels: 2,
            dropout_after: None,
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

    // T199: a test drives the blocks, and the clock moves on by their length.
    #[test]
    fn manual_pacing_runs_the_blocks_a_test_asks_for() {
        let options = NullOptions {
            pacing: Pacing::Manual,
            frame_limit: None,
            channels: 2,
            dropout_after: None,
        };
        let (backend, control) = NullBackend::with_devices(options, &[(LOCAL, NAME)]);
        let clock = control.clock().expect("a manual clock");
        let blocks = Arc::new(AtomicU64::new(0));
        let mut stream = backend
            .open_stream(&duplex(256), Box::new(Count(Arc::clone(&blocks))))
            .unwrap();
        stream.start().unwrap();
        assert_eq!(blocks.load(Ordering::Relaxed), 0, "no block runs unasked");

        control.run_blocks(7);
        assert_eq!(blocks.load(Ordering::Relaxed), 7);
        assert_eq!(stream.stats().blocks, 7);
        let period = frames_to_duration(256, 48_000);
        assert_eq!(clock.elapsed(), period * 7);

        // Frames round up to whole blocks.
        control.run_frames(256 * 3 + 1);
        assert_eq!(blocks.load(Ordering::Relaxed), 11);
        assert_eq!(clock.elapsed(), period * 11);
        assert_eq!(stream.stats().heap_operations, 0);
        stream.stop().unwrap();
        assert!(control.clock().is_some());
    }

    #[test]
    fn a_free_running_null_device_leaves_its_thread_alone() {
        let options = NullOptions {
            pacing: Pacing::Free,
            frame_limit: Some(10 * 128),
            channels: 2,
            dropout_after: None,
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
            channels: 2,
            dropout_after: None,
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
            channels: 2,
            dropout_after: None,
        };
        let run = run(options, 64, Some((3, Duration::from_millis(5))));
        assert_eq!(run.stats.dropouts, 0);
    }

    #[test]
    fn a_stream_resumes_after_stop() {
        let backend = NullBackend::with_options(NullOptions {
            pacing: Pacing::Free,
            frame_limit: None,
            channels: 2,
            dropout_after: None,
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

    /// Records the highest level each input channel carries.
    struct HearInput(Arc<Mutex<[f32; 4]>>);

    impl AudioCallback for HearInput {
        fn process(
            &mut self,
            _: &ProcessContext,
            input: &AudioBuffer<'_>,
            _: &mut AudioBufferMut<'_>,
        ) {
            let mut peaks = self.0.lock().unwrap();
            for (channel, samples) in input.iter().enumerate() {
                let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
                peaks[channel] = peaks[channel].max(peak);
            }
        }
    }

    // Spec: routing R2, WP-2.13
    #[test]
    fn an_input_tone_reaches_only_the_channels_it_is_set_on() {
        let options = NullOptions {
            channels: 4,
            dropout_after: None,
            ..NullOptions::default()
        };
        let (backend, control) = NullBackend::with_devices(options, &[(LOCAL, NAME)]);
        control.set_input_tone(2, 300.0, 0.5);
        let peaks = Arc::new(Mutex::new([0.0f32; 4]));
        let mut stream = backend
            .open_stream(&duplex(256), Box::new(HearInput(Arc::clone(&peaks))))
            .unwrap();
        assert_eq!(
            (stream.info().input_channels, stream.info().output_channels),
            (4, 4),
        );
        stream.start().unwrap();
        wait_for("the tone", || peaks.lock().unwrap()[2] > 0.4);
        stream.stop().unwrap();
        let peaks = peaks.lock().unwrap();
        assert!(peaks[2] > 0.4, "channel 3 carries the tone: {peaks:?}");
        assert!(
            peaks
                .iter()
                .enumerate()
                .all(|(channel, peak)| channel == 2 || *peak == 0.0),
            "the other channels stay silent: {peaks:?}"
        );
    }

    /// Counts blocks and nothing else.
    struct Count(Arc<AtomicU64>);

    impl AudioCallback for Count {
        fn process(&mut self, _: &ProcessContext, _: &AudioBuffer<'_>, _: &mut AudioBufferMut<'_>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A simulated backend with two devices, paced in real time at 64-frame
    /// blocks (1.3 ms), so streams keep calling back without hurrying.
    fn simulated() -> (NullBackend, NullControl) {
        NullBackend::with_devices(
            NullOptions::default(),
            &[("built-in", "Built-in Output"), ("usb", "USB Interface")],
        )
    }

    fn on(local: &str) -> StreamRequest {
        StreamRequest {
            buffer_size: Some(64),
            ..StreamRequest::output(DeviceSelector::Id(DeviceId::new(BackendKind::Null, local)))
        }
    }

    /// Opens and starts a stream, and returns it with its block counter.
    fn started(
        backend: &NullBackend,
        request: &StreamRequest,
    ) -> (Box<dyn Stream>, Arc<AtomicU64>) {
        let blocks = Arc::new(AtomicU64::new(0));
        let mut stream = backend
            .open_stream(request, Box::new(Count(Arc::clone(&blocks))))
            .unwrap();
        stream.start().unwrap();
        wait_for("the first blocks", || blocks.load(Ordering::Relaxed) > 4);
        (stream, blocks)
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let began = Instant::now();
        while !done() {
            assert!(
                began.elapsed() < Duration::from_secs(10),
                "waited for {what}"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Whether `blocks` stays put for 50 ms, as a stream that's stopped
    /// calling back does.
    fn stays_still(blocks: &AtomicU64) -> bool {
        let before = blocks.load(Ordering::Relaxed);
        thread::sleep(Duration::from_millis(50));
        blocks.load(Ordering::Relaxed) == before
    }

    fn present(backend: &NullBackend) -> Vec<String> {
        backend
            .devices()
            .unwrap()
            .into_iter()
            .map(|d| d.id.local().to_owned())
            .collect()
    }

    // Spec: device-changes R1
    #[test]
    fn an_unplugged_device_stops_its_streams_and_leaves_the_list() {
        let (backend, control) = simulated();
        let (stream, blocks) = started(&backend, &on("usb"));
        control.unplug("usb");
        wait_for("the loss", || stream.stats().device_lost);
        assert!(stays_still(&blocks));
        assert!(!control.is_present("usb"));
        assert_eq!(present(&backend), ["built-in"]);
        let refused = backend
            .open_stream(&on("usb"), Box::new(Count(Arc::default())))
            .err()
            .unwrap();
        assert_eq!(refused.kind(), ErrorKind::DeviceNotFound);

        // Plugged back in, the old stream stays dead and a new one runs.
        control.replug("usb");
        assert!(stays_still(&blocks));
        assert_eq!(present(&backend), ["built-in", "usb"]);
        let (again, _) = started(&backend, &on("usb"));
        assert!(!again.stats().device_lost);
    }

    // Spec: device-changes R3
    #[test]
    fn a_rate_change_invalidates_the_devices_streams() {
        let (backend, control) = simulated();
        let (stream, blocks) = started(&backend, &on("usb"));
        let (other, other_blocks) = started(&backend, &on("built-in"));
        control.set_sample_rate("usb", 44_100);
        wait_for("the invalidation", || stream.stats().invalidated);
        assert!(stays_still(&blocks));
        assert!(
            !stays_still(&other_blocks),
            "another device's stream runs on"
        );
        assert!(!other.stats().invalidated);
        let usb = backend
            .devices()
            .unwrap()
            .into_iter()
            .find(|d| d.id.local() == "usb")
            .unwrap();
        assert_eq!(usb.current_sample_rate, Some(44_100));
        // A stream that doesn't ask for a rate opens at the device's.
        let reopened = backend
            .open_stream(&on("usb"), Box::new(Count(Arc::default())))
            .unwrap();
        assert_eq!(reopened.info().sample_rate, 44_100);
        // Setting the same rate again changes nothing.
        control.set_sample_rate("usb", 44_100);
        assert_eq!(control.sample_rate("usb"), 44_100);
    }

    // Spec: device-changes R5
    #[test]
    fn a_stall_silences_the_streams_open_now_and_reports_nothing() {
        let (backend, control) = simulated();
        let (stream, blocks) = started(&backend, &on("usb"));
        control.stall();
        thread::sleep(Duration::from_millis(5));
        assert!(stays_still(&blocks));
        let stats = stream.stats();
        assert!(!stats.device_lost && !stats.invalidated);
        let (_fresh_stream, fresh) = started(&backend, &on("usb"));
        assert!(!stays_still(&fresh), "a stream opened afterwards runs");
    }

    // Spec: device-changes R4
    #[test]
    fn sleep_pauses_every_stream_until_the_machine_wakes() {
        let (backend, control) = simulated();
        let mut power = control.power();
        let (stream, blocks) = started(&backend, &on("usb"));
        control.sleep();
        thread::sleep(Duration::from_millis(5));
        assert!(stays_still(&blocks));
        assert_eq!(power.poll(), Some(PowerEvent::Sleeping));
        assert_eq!(power.poll(), None);
        control.wake();
        assert!(!stays_still(&blocks), "streams call back again");
        assert!(matches!(
            power.poll(),
            Some(PowerEvent::Woke { slept: Some(slept) }) if slept >= Duration::from_millis(50)
        ));
        let stats = stream.stats();
        assert_eq!(stats.gaps, 0, "the clock starts afresh after the pause");
        control.notify(PowerEvent::Woke { slept: None });
        assert_eq!(power.poll(), Some(PowerEvent::Woke { slept: None }));
    }

    // Spec: device-changes R6
    #[test]
    fn a_default_stream_moves_when_its_device_goes() {
        let (backend, control) = simulated();
        let request = StreamRequest {
            buffer_size: Some(64),
            ..StreamRequest::output(DeviceSelector::Default)
        };
        let (stream, blocks) = started(&backend, &request);
        assert!(stream.info().follows_default);
        assert_eq!(
            stream.info().output_device.as_ref().unwrap().id.local(),
            "built-in"
        );
        control.unplug("built-in");
        wait_for("the move", || stream.stats().device_changes == 1);
        assert!(!stays_still(&blocks), "it carries on");
        // A rate change on the new default doesn't stop it either.
        control.set_sample_rate("usb", 96_000);
        assert!(!stays_still(&blocks));
        control.unplug("usb");
        wait_for("the loss", || stream.stats().device_lost);
        assert!(stays_still(&blocks));
        assert!(backend.devices().unwrap().is_empty());
        let refused = backend
            .open_stream(&request, Box::new(Count(Arc::default())))
            .err()
            .unwrap();
        assert_eq!(refused.kind(), ErrorKind::DeviceNotFound);
    }

    #[test]
    fn a_streams_input_and_output_share_one_device() {
        let (backend, _control) = simulated();
        let usb = DeviceSelector::Id(DeviceId::new(BackendKind::Null, "usb"));
        let request = StreamRequest {
            output: Some(PortRequest {
                device: usb.clone(),
                channels: None,
            }),
            input: Some(PortRequest {
                device: DeviceSelector::Default,
                channels: None,
            }),
            ..StreamRequest::default()
        };
        let refused = backend
            .open_stream(&request, Box::new(Count(Arc::default())))
            .err()
            .unwrap();
        assert_eq!(refused.kind(), ErrorKind::UnsupportedConfig);
        let request = StreamRequest {
            input: Some(PortRequest {
                device: usb,
                channels: None,
            }),
            ..request
        };
        assert!(
            backend
                .open_stream(&request, Box::new(Count(Arc::default())))
                .is_ok()
        );
    }

    #[test]
    fn opening_at_another_rate_sets_the_devices_rate() {
        let (backend, control) = simulated();
        let stream = backend
            .open_stream(
                &StreamRequest {
                    sample_rate: Some(96_000),
                    ..on("usb")
                },
                Box::new(Count(Arc::default())),
            )
            .unwrap();
        assert_eq!(stream.info().sample_rate, 96_000);
        assert_eq!(control.sample_rate("usb"), 96_000);
        assert_eq!(control.sample_rate("built-in"), 48_000);
    }

    #[test]
    #[should_panic(expected = "two null devices are called `a`")]
    fn device_ids_are_unique() {
        let _ = NullBackend::with_devices(NullOptions::default(), &[("a", "A"), ("a", "B")]);
    }
}
