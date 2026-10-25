//! Keeping a stream running through device changes (WP-1.13).
//!
//! A [`Supervisor`] owns one stream on a thread of its own and watches it.
//! When the device goes away, its sample rate changes, the stream stops
//! calling back, or the machine sleeps, it closes the stream, says so (its
//! [`DeviceStatus`] and a [`DeviceEvent`]), and opens the stream again when
//! the device can run: straight away after a rate change, when the device is
//! back after an unplug, and on waking after a sleep. The rules are in
//! the device changes spec.
//!
//! What it watches:
//!
//! | Event | How it's noticed |
//! |---|---|
//! | Device gone | The backend reports it ([`StreamStats::device_lost`]), or a stream it invalidated is on a device no longer listed |
//! | Sample rate changed | The backend invalidates the stream ([`StreamStats::invalidated`]) and the device now runs at another rate |
//! | Stream stalled | No frames for [`SupervisorOptions::stall_timeout`], whatever the backend says |
//! | Sleep and wake | A [`PowerMonitor`]: by default [`ClockJump`], which notices a wake afterwards |
//! | Default device changed | The backend reports it ([`StreamStats::device_changes`]); the stream carries on, unless it also records, when it reopens on the new default |
//!
//! What runs in the stream is a [`StreamClient`]'s: each time a stream
//! opens, the client makes its callback for what the stream actually runs
//! at, so the engine can restart at a new sample rate; each time one closes,
//! the client hears of it. The callback reaches the audio thread through a
//! hand-off that moves one pointer, so nothing here adds work to the audio
//! thread beyond an empty-queue check per block.
//!
//! Streams are opened, watched, and closed only on the supervisor's thread,
//! and the backend is made there, so neither needs to be `Send`.

use std::collections::BTreeMap;
use std::fmt;
use std::mem;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bl_rt::Workgroup;
use rtrb::{Consumer, RingBuffer};

use crate::power::{ClockJump, PowerEvent, PowerMonitor};
use crate::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, DeviceId, DeviceSelector, Error,
    ErrorKind, OpenDevice, ProcessContext, Stream, StreamInfo, StreamRequest, StreamStats,
};

/// Makes the backend a [`Supervisor`] uses, on the supervisor's thread.
pub type BackendFactory = Box<dyn FnOnce() -> Result<Box<dyn Backend>, Error> + Send>;

/// What runs in a supervised stream.
///
/// Every method is called on the supervisor's thread, never an audio
/// thread. [`open`](Self::open) and [`closed`](Self::closed) come in pairs:
/// after a successful `open`, `closed` follows when that stream has stopped
/// and been dropped, before the next `open`.
pub trait StreamClient: Send + 'static {
    /// Makes the callback for a stream that has just opened, with what it
    /// actually runs at, and the audio workgroup its audio thread works in
    /// (macOS), for the client's own real-time threads to join. The stream
    /// starts once it returns. An error closes the stream, and the
    /// supervisor tries again later.
    ///
    /// # Errors
    ///
    /// If the client can't run at `info`'s settings.
    fn open(
        &mut self,
        info: &StreamInfo,
        workgroup: Option<Workgroup>,
    ) -> Result<Box<dyn AudioCallback>, Error>;

    /// The stream the last `open` made a callback for has stopped, and its
    /// callback has been dropped.
    fn closed(&mut self) {}

    /// Something happened. The same events arrive through
    /// [`Supervisor::events`]; this hears them first, on the supervisor's
    /// thread, for a client that wants to wake a UI.
    fn event(&mut self, _event: &DeviceEvent) {}
}

/// How a [`Supervisor`] watches and recovers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorOptions {
    /// How often it checks the stream. Default 50 ms.
    pub poll_interval: Duration,
    /// How often it tries to open a device that couldn't open. Default 1 s.
    pub retry_interval: Duration,
    /// How long a running stream may go without delivering a frame before
    /// it's treated as dead. Default 3 s.
    pub stall_timeout: Duration,
    /// Whether to play through the system's default output device while
    /// the chosen device is missing, moving back when it returns. Off by
    /// default, so audio never jumps to the computer's speakers unasked.
    pub fall_back_to_default: bool,
}

impl Default for SupervisorOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(50),
            retry_interval: Duration::from_secs(1),
            stall_timeout: Duration::from_secs(3),
            fall_back_to_default: false,
        }
    }
}

/// Why a stream stopped, or couldn't start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// The device went away: unplugged, switched off, or removed.
    DeviceLost,
    /// The device's sample rate changed under the stream.
    SampleRateChanged {
        /// The rate the stream ran at, in hertz.
        from: u32,
        /// The device's rate now.
        to: u32,
    },
    /// The stream stopped calling back without reporting why.
    Stalled,
    /// The machine went to sleep.
    Sleep,
    /// The backend stopped the stream for another reason.
    StreamFailed,
    /// The device couldn't be opened.
    Unavailable,
}

/// A stream that stopped, or couldn't open, and what's being done about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Interruption {
    /// Why.
    pub cause: Cause,
    /// The device lost or waited for, if a particular one: `None` means the
    /// system's default device.
    pub device: Option<OpenDevice>,
    /// When it happened.
    pub since: Instant,
    /// Attempts to open the device again so far.
    pub attempts: u32,
    /// Why the latest attempt failed.
    pub last_error: Option<Error>,
    /// The counters of the stream that stopped, if one did.
    pub stream_stats: Option<StreamStats>,
}

impl Interruption {
    /// A sentence for a person: what happened and what happens next.
    #[must_use]
    pub fn message(&self) -> String {
        let device = self.subject();
        let mut text = match &self.cause {
            Cause::DeviceLost => {
                format!("{device} was disconnected. Audio is off until it's connected again.")
            }
            Cause::SampleRateChanged { from, to } => format!(
                "The sample rate of {} changed from {from} Hz to {to} Hz. Audio is restarting at the new rate.",
                self.object()
            ),
            Cause::Stalled => format!("{device} stopped responding. Audio is restarting."),
            Cause::Sleep => {
                "Audio stopped while the computer slept, and restarts when it wakes.".to_owned()
            }
            Cause::StreamFailed => {
                format!("{device} stopped the audio stream. Audio is restarting.")
            }
            Cause::Unavailable => match &self.last_error {
                Some(error) => format!("{device} can't be opened ({error}). Trying again."),
                None => format!("{device} can't be opened. Trying again."),
            },
        };
        let restarting = matches!(
            self.cause,
            Cause::SampleRateChanged { .. } | Cause::Stalled | Cause::StreamFailed
        );
        if restarting
            && self.attempts > 0
            && let Some(error) = &self.last_error
        {
            text.push_str(&format!(" The last attempt failed: {error}."));
        }
        text
    }

    /// "The audio device "X"" or "The default audio device".
    fn subject(&self) -> String {
        match &self.device {
            Some(device) => format!("The audio device \"{}\"", device.name),
            None => "The default audio device".to_owned(),
        }
    }

    /// "\"X\"" or "the default audio device".
    fn object(&self) -> String {
        match &self.device {
            Some(device) => format!("\"{}\"", device.name),
            None => "the default audio device".to_owned(),
        }
    }
}

/// Where a supervised stream stands.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceStatus {
    /// Opening the first stream.
    Starting,
    /// A stream is running as asked.
    Running(StreamInfo),
    /// The chosen device is missing, and a stream runs on the system's
    /// default output device meanwhile
    /// ([`SupervisorOptions::fall_back_to_default`]).
    Fallback {
        /// The stream on the default device.
        info: StreamInfo,
        /// What happened to the chosen device.
        interruption: Interruption,
    },
    /// No stream runs: one stopped, or couldn't open, and the supervisor is
    /// bringing it back.
    Interrupted(Interruption),
    /// The supervisor has stopped.
    Stopped,
}

impl DeviceStatus {
    /// What a person should be told, or `None` while all is well.
    #[must_use]
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Starting | Self::Running(_) | Self::Stopped => None,
            Self::Interrupted(interruption) => Some(interruption.message()),
            Self::Fallback { info, interruption } => Some(format!(
                "{} is missing. Audio is playing through \"{}\" until it's back.",
                interruption.subject(),
                device_name(info)
            )),
        }
    }

    /// The running stream, if one runs.
    #[must_use]
    pub fn info(&self) -> Option<&StreamInfo> {
        match self {
            Self::Running(info) | Self::Fallback { info, .. } => Some(info),
            _ => None,
        }
    }

    /// The interruption, if the chosen device isn't running.
    #[must_use]
    pub fn interruption(&self) -> Option<&Interruption> {
        match self {
            Self::Interrupted(interruption) | Self::Fallback { interruption, .. } => {
                Some(interruption)
            }
            _ => None,
        }
    }
}

/// Something that happened to a supervised stream.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceEvent {
    /// A stream opened and started, when the supervisor started or after
    /// [`Supervisor::reconfigure`].
    Started(StreamInfo),
    /// The stream stopped, or couldn't open.
    Interrupted(Interruption),
    /// A stream runs again on the chosen device.
    Recovered {
        /// What it runs at.
        info: StreamInfo,
        /// Why it had stopped.
        cause: Cause,
        /// How long it was stopped, or on the fallback device.
        after: Duration,
        /// Attempts it took.
        attempts: u32,
    },
    /// The chosen device is missing and a stream runs on the system's
    /// default output device meanwhile.
    FellBack {
        /// The stream on the default device.
        info: StreamInfo,
        /// What happened to the chosen device.
        interruption: Interruption,
    },
    /// The system's default device changed under a stream that follows it,
    /// and the stream carried on with the new one.
    DefaultDeviceChanged,
    /// The machine woke from sleep.
    Woke {
        /// How long it slept, if known.
        slept: Option<Duration>,
    },
    /// The supervisor stopped, and closed its stream.
    Stopped,
}

impl DeviceEvent {
    /// A line for a person or a log.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Started(info) => format!(
                "Audio is running on \"{}\" at {} Hz.",
                device_name(info),
                info.sample_rate
            ),
            Self::Interrupted(interruption) => interruption.message(),
            Self::Recovered { info, after, .. } => format!(
                "Audio is back on \"{}\" at {} Hz, after {:.1} s.",
                device_name(info),
                info.sample_rate,
                after.as_secs_f64()
            ),
            Self::FellBack { info, interruption } => DeviceStatus::Fallback {
                info: info.clone(),
                interruption: interruption.clone(),
            }
            .message()
            .unwrap_or_default(),
            Self::DefaultDeviceChanged => {
                "The system's default audio device changed, and audio moved to it.".to_owned()
            }
            Self::Woke { slept: Some(slept) } => format!(
                "The computer woke after sleeping for {:.1} s.",
                slept.as_secs_f64()
            ),
            Self::Woke { slept: None } => "The computer woke from sleep.".to_owned(),
            Self::Stopped => "Audio stopped.".to_owned(),
        }
    }
}

fn device_name(info: &StreamInfo) -> &str {
    info.output_device
        .as_ref()
        .or(info.input_device.as_ref())
        .map_or("the audio device", |device| device.name.as_str())
}

/// What the supervisor's thread publishes for [`Supervisor`]'s readers.
#[derive(Debug, Clone)]
struct Published {
    status: DeviceStatus,
    stats: Option<StreamStats>,
    request: StreamRequest,
}

enum Command {
    Reconfigure(StreamRequest, mpsc::Sender<Result<StreamInfo, Error>>),
    Retry,
    Stop,
}

/// Keeps one audio stream running through device changes, on a thread of
/// its own (WP-1.13; the rules are in the device changes spec).
///
/// When the device goes away, its sample rate changes, the stream stops
/// calling back, or the machine sleeps, it closes the stream, says so (its
/// [`DeviceStatus`] and a [`DeviceEvent`]), and opens the stream again when
/// the device can run: straight away after a rate change, when the device is
/// back after an unplug, and on waking after a sleep.
///
/// | Event | How it's noticed |
/// |---|---|
/// | Device gone | The backend reports it ([`StreamStats::device_lost`]), or a stream it invalidated is on a device no longer listed |
/// | Sample rate changed | The backend invalidates the stream ([`StreamStats::invalidated`]) and the device now runs at another rate |
/// | Stream stalled | No frames for [`SupervisorOptions::stall_timeout`], whatever the backend says |
/// | Sleep and wake | A [`PowerMonitor`]: by default [`ClockJump`], which notices a wake afterwards |
/// | Default device changed | The backend reports it ([`StreamStats::device_changes`]); the stream carries on, unless it also records, when it reopens on the new default |
///
/// What runs in the stream is a [`StreamClient`]'s: each time a stream
/// opens, the client makes its callback for what the stream actually runs
/// at; each time one closes, the client hears of it. Streams are opened,
/// watched, and closed only on the supervisor's thread, and the backend is
/// made there, so neither needs to be `Send`.
///
/// Dropping it closes the stream and stops the thread.
pub struct Supervisor {
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<DeviceEvent>,
    shared: Arc<Mutex<Published>>,
    thread: Option<JoinHandle<()>>,
}

impl Supervisor {
    /// Starts a supervisor that makes its backend with `backend`, opens
    /// `request` for `client`, and notices sleep from the clocks
    /// ([`ClockJump`]). It returns once the first attempt to open has
    /// finished: if that failed, the status says why and the supervisor
    /// keeps trying.
    ///
    /// # Errors
    ///
    /// If the backend can't be made or the thread can't start.
    pub fn start(
        backend: BackendFactory,
        request: StreamRequest,
        client: Box<dyn StreamClient>,
        options: SupervisorOptions,
    ) -> Result<Self, Error> {
        Self::start_with_power(
            backend,
            request,
            client,
            options,
            Box::new(ClockJump::default()),
        )
    }

    /// [`start`](Self::start), learning of sleep and wake from `power`.
    ///
    /// # Errors
    ///
    /// If the backend can't be made or the thread can't start.
    pub fn start_with_power(
        backend: BackendFactory,
        request: StreamRequest,
        client: Box<dyn StreamClient>,
        options: SupervisorOptions,
        power: Box<dyn PowerMonitor>,
    ) -> Result<Self, Error> {
        let (commands, command_receiver) = mpsc::channel();
        let (event_sender, events) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Published {
            status: DeviceStatus::Starting,
            stats: None,
            request: request.clone(),
        }));
        let (ready, started) = mpsc::channel();
        let thread = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("bl-audio-supervisor".into())
                .spawn(move || {
                    let backend = match backend() {
                        Ok(backend) => backend,
                        Err(error) => {
                            let _ = ready.send(Err(error));
                            return;
                        }
                    };
                    let mut worker = Worker {
                        backend,
                        client,
                        options,
                        power,
                        request,
                        state: State::Stopped,
                        shared,
                        events: event_sender,
                        names: BTreeMap::new(),
                    };
                    worker.start();
                    let _ = ready.send(Ok(()));
                    worker.run(&command_receiver);
                })
                .map_err(|e| {
                    Error::new(
                        ErrorKind::Backend,
                        format!("couldn't start the audio supervisor's thread: {e}"),
                    )
                })?
        };
        let outcome = started.recv().unwrap_or_else(|_| {
            Err(Error::new(
                ErrorKind::Backend,
                "the audio supervisor's thread ended while starting",
            ))
        });
        let mut supervisor = Self {
            commands,
            events,
            shared,
            thread: Some(thread),
        };
        match outcome {
            Ok(()) => Ok(supervisor),
            Err(error) => {
                supervisor.shut_down();
                Err(error)
            }
        }
    }

    fn published(&self) -> std::sync::MutexGuard<'_, Published> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Where the stream stands now.
    #[must_use]
    pub fn status(&self) -> DeviceStatus {
        self.published().status.clone()
    }

    /// The running stream's counters, as of the last check.
    #[must_use]
    pub fn stats(&self) -> Option<StreamStats> {
        self.published().stats
    }

    /// The settings the supervisor opens: the request it started with or was
    /// last reconfigured to, without a sample rate once a device's rate has
    /// changed under it (it follows the device from then on).
    #[must_use]
    pub fn request(&self) -> StreamRequest {
        self.published().request.clone()
    }

    /// Events since the last call, oldest first.
    #[must_use]
    pub fn events(&self) -> Vec<DeviceEvent> {
        self.events.try_iter().collect()
    }

    /// Closes the stream and opens `request` instead: another device, rate,
    /// or buffer size. If it can't open, the previous settings are opened
    /// again and the error is returned.
    ///
    /// # Errors
    ///
    /// If `request` can't open.
    pub fn reconfigure(&self, request: StreamRequest) -> Result<StreamInfo, Error> {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(Command::Reconfigure(request, reply))
            .map_err(|_| stopped())?;
        result.recv().map_err(|_| stopped())?
    }

    /// Tries to open the device again now, rather than at the next retry.
    pub fn retry(&self) {
        let _ = self.commands.send(Command::Retry);
    }

    /// Closes the stream and stops the thread.
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shut_down();
    }
}

impl fmt::Debug for Supervisor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Supervisor")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

fn stopped() -> Error {
    Error::new(ErrorKind::Backend, "the audio supervisor has stopped")
}

/// A stream that's running.
struct Live {
    stream: Box<dyn Stream>,
    info: StreamInfo,
    /// Frames it had delivered at the last check.
    frames: u64,
    /// When `frames` last moved.
    progress: Instant,
    /// Default-device changes already reported.
    device_changes: u64,
}

impl Live {
    fn device(&self) -> Option<OpenDevice> {
        self.info
            .output_device
            .clone()
            .or_else(|| self.info.input_device.clone())
    }
}

enum State {
    Running(Box<Live>),
    /// On the default device while the chosen one is missing; the instant is
    /// the next look for the chosen device.
    Fallback(Box<Live>, Interruption, Instant),
    /// Waiting to try again at the instant.
    Waiting(Interruption, Instant),
    /// Asleep: nothing is tried until the machine wakes.
    Asleep(Interruption),
    Stopped,
}

/// The supervisor's thread.
struct Worker {
    backend: Box<dyn Backend>,
    client: Box<dyn StreamClient>,
    options: SupervisorOptions,
    power: Box<dyn PowerMonitor>,
    request: StreamRequest,
    state: State,
    shared: Arc<Mutex<Published>>,
    events: mpsc::Sender<DeviceEvent>,
    /// The names of devices opened before, for messages about them once
    /// they're gone.
    names: BTreeMap<DeviceId, String>,
}

impl Worker {
    fn start(&mut self) {
        let interruption = self.unavailable(Instant::now());
        self.state = self.attempt(interruption, true);
        self.power.rebaseline();
        self.publish();
    }

    fn run(&mut self, commands: &mpsc::Receiver<Command>) {
        loop {
            match commands.recv_timeout(self.options.poll_interval) {
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Command::Reconfigure(request, reply)) => {
                    let result = self.reconfigure(request);
                    self.power.rebaseline();
                    let _ = reply.send(result);
                }
                Ok(Command::Retry) => {
                    if let State::Waiting(_, next) | State::Fallback(_, _, next) = &mut self.state {
                        *next = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick();
            self.publish();
        }
        match mem::replace(&mut self.state, State::Stopped) {
            State::Running(live) | State::Fallback(live, ..) => self.close(live),
            _ => {}
        }
        self.emit(DeviceEvent::Stopped);
        self.publish();
    }

    /// One check: power events first, then the stream.
    fn tick(&mut self) {
        while let Some(event) = self.power.poll() {
            self.power_event(event);
        }
        let now = Instant::now();
        self.state = match mem::replace(&mut self.state, State::Stopped) {
            State::Running(mut live) => match self.check(&mut live, now) {
                None => State::Running(live),
                Some(cause) => {
                    let interruption = self.interrupt(live, cause, now);
                    self.attempt(interruption, false)
                }
            },
            State::Fallback(mut live, interruption, next) => {
                if self.check(&mut live, now).is_some() {
                    self.close(live);
                    self.attempt(interruption, false)
                } else if now >= next {
                    if self.chosen_device_is_back() {
                        self.close(live);
                        self.attempt(interruption, false)
                    } else {
                        State::Fallback(live, interruption, now + self.options.retry_interval)
                    }
                } else {
                    State::Fallback(live, interruption, next)
                }
            }
            State::Waiting(interruption, next) if now >= next => self.attempt(interruption, false),
            other => other,
        };
        // Opening and closing devices block, and a slow one must not read as
        // a sleep at the next poll.
        self.power.rebaseline();
    }

    /// Checks a running stream; returns why it must close, if it must.
    fn check(&mut self, live: &mut Live, now: Instant) -> Option<Cause> {
        let stats = live.stream.stats();
        if stats.device_changes > live.device_changes {
            live.device_changes = stats.device_changes;
            self.emit(DeviceEvent::DefaultDeviceChanged);
        }
        if stats.device_lost {
            return Some(Cause::DeviceLost);
        }
        if stats.invalidated {
            return Some(self.why_invalidated(&live.info));
        }
        if stats.frames != live.frames {
            live.frames = stats.frames;
            live.progress = now;
        } else if now.duration_since(live.progress) > self.options.stall_timeout {
            return Some(Cause::Stalled);
        }
        None
    }

    /// Tells a rate change from a lost device or another failure, by asking
    /// the backend about the stream's device.
    fn why_invalidated(&self, info: &StreamInfo) -> Cause {
        let Some(device) = info.output_device.as_ref().or(info.input_device.as_ref()) else {
            return Cause::StreamFailed;
        };
        let Ok(devices) = self.backend.devices() else {
            return Cause::StreamFailed;
        };
        match devices.iter().find(|d| d.id == device.id) {
            None => Cause::DeviceLost,
            Some(now) => match now.current_sample_rate {
                Some(rate) if rate != info.sample_rate => Cause::SampleRateChanged {
                    from: info.sample_rate,
                    to: rate,
                },
                _ => Cause::StreamFailed,
            },
        }
    }

    /// Closes a stream that stopped for `cause`, and reports it.
    fn interrupt(&mut self, live: Box<Live>, cause: Cause, now: Instant) -> Interruption {
        let stats = live.stream.stats();
        // A stream that follows the default may have moved since it opened,
        // so it names no particular device.
        let device = if live.info.follows_default {
            None
        } else {
            live.device()
        };
        self.close(live);
        if matches!(cause, Cause::SampleRateChanged { .. }) {
            // Follow the device's new rate rather than set it back.
            self.request.sample_rate = None;
        }
        let interruption = Interruption {
            cause,
            device,
            since: now,
            attempts: 0,
            last_error: None,
            stream_stats: Some(stats),
        };
        self.emit(DeviceEvent::Interrupted(interruption.clone()));
        interruption
    }

    /// Tries to open the chosen device, then the fallback if asked for.
    fn attempt(&mut self, mut interruption: Interruption, starting: bool) -> State {
        interruption.attempts += 1;
        let request = self.request.clone();
        match self.open(&request) {
            Ok(live) => {
                let info = live.info.clone();
                self.emit(if starting {
                    DeviceEvent::Started(info)
                } else {
                    DeviceEvent::Recovered {
                        info,
                        cause: interruption.cause.clone(),
                        after: interruption.since.elapsed(),
                        attempts: interruption.attempts,
                    }
                });
                State::Running(live)
            }
            Err(error) => {
                interruption.last_error = Some(error);
                if starting {
                    self.emit(DeviceEvent::Interrupted(interruption.clone()));
                }
                let now = Instant::now();
                if let Some(fallback) = self.fallback_request()
                    && let Ok(live) = self.open(&fallback)
                {
                    self.emit(DeviceEvent::FellBack {
                        info: live.info.clone(),
                        interruption: interruption.clone(),
                    });
                    return State::Fallback(live, interruption, now + self.options.retry_interval);
                }
                State::Waiting(interruption, now + self.options.retry_interval)
            }
        }
    }

    /// The request on the system's default device, if falling back is on and
    /// the request names a device.
    fn fallback_request(&self) -> Option<StreamRequest> {
        if !self.options.fall_back_to_default {
            return None;
        }
        let names_one = |port: &Option<crate::PortRequest>| {
            port.as_ref()
                .is_some_and(|p| matches!(p.device, DeviceSelector::Id(_)))
        };
        if !(names_one(&self.request.output) || names_one(&self.request.input)) {
            return None;
        }
        let mut request = self.request.clone();
        for port in [&mut request.output, &mut request.input]
            .into_iter()
            .flatten()
        {
            port.device = DeviceSelector::Default;
        }
        // The default device runs at its own rate.
        request.sample_rate = None;
        Some(request)
    }

    /// Whether the device the request names is listed again.
    fn chosen_device_is_back(&self) -> bool {
        let Ok(devices) = self.backend.devices() else {
            return false;
        };
        [&self.request.output, &self.request.input]
            .into_iter()
            .flatten()
            .all(|port| match &port.device {
                DeviceSelector::Default => true,
                DeviceSelector::Id(id) => devices.iter().any(|d| d.id == *id),
            })
    }

    /// An interruption for a device that couldn't open.
    fn unavailable(&self, now: Instant) -> Interruption {
        let device = [&self.request.output, &self.request.input]
            .into_iter()
            .flatten()
            .find_map(|port| match &port.device {
                DeviceSelector::Id(id) => Some(OpenDevice {
                    id: id.clone(),
                    name: self
                        .names
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| id.to_string()),
                }),
                DeviceSelector::Default => None,
            });
        Interruption {
            cause: Cause::Unavailable,
            device,
            since: now,
            attempts: 0,
            last_error: None,
            stream_stats: None,
        }
    }

    fn power_event(&mut self, event: PowerEvent) {
        let now = Instant::now();
        self.state = match (event, mem::replace(&mut self.state, State::Stopped)) {
            (PowerEvent::Sleeping, State::Running(live)) => {
                State::Asleep(self.interrupt(live, Cause::Sleep, now))
            }
            (PowerEvent::Sleeping, State::Fallback(live, interruption, _)) => {
                self.close(live);
                State::Asleep(interruption)
            }
            (PowerEvent::Sleeping, State::Waiting(interruption, _)) => State::Asleep(interruption),
            (PowerEvent::Woke { slept }, state) => {
                self.emit(DeviceEvent::Woke { slept });
                match state {
                    // Try at once: the device may be back. A stream that ran
                    // through the sleep gets the stall timeout from now.
                    State::Asleep(interruption) | State::Waiting(interruption, _) => {
                        State::Waiting(interruption, now)
                    }
                    State::Running(mut live) => {
                        live.progress = now;
                        State::Running(live)
                    }
                    State::Fallback(mut live, interruption, _) => {
                        live.progress = now;
                        State::Fallback(live, interruption, now)
                    }
                    State::Stopped => State::Stopped,
                }
            }
            (PowerEvent::Sleeping, state) => state,
        };
    }

    fn reconfigure(&mut self, request: StreamRequest) -> Result<StreamInfo, Error> {
        match mem::replace(&mut self.state, State::Stopped) {
            State::Running(live) | State::Fallback(live, ..) => self.close(live),
            _ => {}
        }
        let previous = mem::replace(&mut self.request, request.clone());
        match self.open(&request) {
            Ok(live) => {
                let info = live.info.clone();
                self.emit(DeviceEvent::Started(info.clone()));
                self.state = State::Running(live);
                Ok(info)
            }
            Err(error) => {
                self.request = previous;
                let interruption = self.unavailable(Instant::now());
                self.state = self.attempt(interruption, true);
                Err(error)
            }
        }
    }

    /// Opens a stream, has the client make its callback, and starts it.
    fn open(&mut self, request: &StreamRequest) -> Result<Box<Live>, Error> {
        let (mut handoff, incoming) = RingBuffer::new(1);
        let mut stream = self.backend.open_stream(
            request,
            Box::new(Handoff {
                incoming,
                current: None,
            }),
        )?;
        let info = stream.info().clone();
        for device in [&info.output_device, &info.input_device]
            .into_iter()
            .flatten()
        {
            self.names.insert(device.id.clone(), device.name.clone());
        }
        let callback = self.client.open(&info, stream.workgroup())?;
        if handoff.push(callback).is_err() {
            unreachable!("a new hand-off has room for one callback");
        }
        if let Err(error) = stream.start() {
            // The queue frees its callback only when both ends are gone, and
            // the callback's drop is what hands the client's state back, so
            // `closed` needs the producer gone first.
            drop(handoff);
            drop(stream);
            self.client.closed();
            return Err(error);
        }
        Ok(Box::new(Live {
            stream,
            info,
            frames: 0,
            progress: Instant::now(),
            device_changes: 0,
        }))
    }

    /// Stops and drops a stream, then tells the client.
    fn close(&mut self, mut live: Box<Live>) {
        // A stream whose device has gone may refuse to stop; it's dropped
        // either way.
        let _ = live.stream.stop();
        drop(live);
        self.client.closed();
    }

    fn emit(&mut self, event: DeviceEvent) {
        self.client.event(&event);
        let _ = self.events.send(event);
    }

    fn publish(&self) {
        let (status, stats) = match &self.state {
            State::Running(live) => (
                DeviceStatus::Running(live.info.clone()),
                Some(live.stream.stats()),
            ),
            State::Fallback(live, interruption, _) => (
                DeviceStatus::Fallback {
                    info: live.info.clone(),
                    interruption: interruption.clone(),
                },
                Some(live.stream.stats()),
            ),
            State::Waiting(interruption, _) | State::Asleep(interruption) => {
                (DeviceStatus::Interrupted(interruption.clone()), None)
            }
            State::Stopped => (DeviceStatus::Stopped, None),
        };
        let mut published = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        published.status = status;
        published.stats = stats;
        published.request.clone_from(&self.request);
    }
}

/// The callback a supervised stream opens with: it runs the client's
/// callback once that arrives, a moment after the stream opened. Until then
/// the stream plays silence. Taking the callback moves one pointer out of a
/// lock-free queue; dropping it happens with the stream, off the audio
/// thread.
struct Handoff {
    incoming: Consumer<Box<dyn AudioCallback>>,
    current: Option<Box<dyn AudioCallback>>,
}

impl AudioCallback for Handoff {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        if self.current.is_none() {
            self.current = self.incoming.pop().ok();
        }
        if let Some(callback) = &mut self.current {
            callback.process(ctx, input, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackendKind, NullBackend, NullControl, NullOptions};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

    /// Counts blocks.
    struct Count(Arc<AtomicU64>);

    impl AudioCallback for Count {
        fn process(&mut self, _: &ProcessContext, _: &AudioBuffer<'_>, _: &mut AudioBufferMut<'_>) {
            self.0.fetch_add(1, Relaxed);
        }
    }

    /// A client that logs what it's told and counts its stream's blocks.
    #[derive(Clone, Default)]
    struct Client {
        log: Arc<Mutex<Vec<String>>>,
        blocks: Arc<AtomicU64>,
        refuse: Arc<AtomicBool>,
    }

    impl StreamClient for Client {
        fn open(
            &mut self,
            info: &StreamInfo,
            _workgroup: Option<Workgroup>,
        ) -> Result<Box<dyn AudioCallback>, Error> {
            if self.refuse.swap(false, Relaxed) {
                self.log.lock().unwrap().push("refused".to_owned());
                return Err(Error::new(ErrorKind::Backend, "the client refused"));
            }
            let device = info.output_device.as_ref().unwrap().id.local().to_owned();
            self.log
                .lock()
                .unwrap()
                .push(format!("open {device} {}", info.sample_rate));
            Ok(Box::new(Count(Arc::clone(&self.blocks))))
        }

        fn closed(&mut self) {
            self.log.lock().unwrap().push("closed".to_owned());
        }
    }

    impl Client {
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        /// Whether the stream's blocks move within a second.
        fn playing(&self) -> bool {
            let before = self.blocks.load(Relaxed);
            let began = Instant::now();
            while began.elapsed() < Duration::from_secs(1) {
                if self.blocks.load(Relaxed) > before + 2 {
                    return true;
                }
                thread::sleep(Duration::from_millis(1));
            }
            false
        }
    }

    fn options() -> SupervisorOptions {
        SupervisorOptions {
            poll_interval: Duration::from_millis(5),
            retry_interval: Duration::from_millis(40),
            stall_timeout: Duration::from_millis(250),
            fall_back_to_default: false,
        }
    }

    fn on(local: &str) -> StreamRequest {
        StreamRequest {
            buffer_size: Some(128),
            ..StreamRequest::output(DeviceSelector::Id(DeviceId::new(BackendKind::Null, local)))
        }
    }

    /// Two simulated devices, and a supervisor of `request` on them, with
    /// the null control's power events.
    fn supervise(
        request: StreamRequest,
        options: SupervisorOptions,
    ) -> (Supervisor, NullControl, Client) {
        let (backend, control) = NullBackend::with_devices(
            NullOptions::default(),
            &[("built-in", "Built-in Output"), ("usb", "USB Interface")],
        );
        let client = Client::default();
        let supervisor = Supervisor::start_with_power(
            Box::new(move || Ok(Box::new(backend) as Box<dyn Backend>)),
            request,
            Box::new(client.clone()),
            options,
            Box::new(control.power()),
        )
        .unwrap();
        (supervisor, control, client)
    }

    fn wait_for<T>(
        supervisor: &Supervisor,
        what: &str,
        mut found: impl FnMut(&DeviceStatus) -> Option<T>,
    ) -> T {
        let began = Instant::now();
        loop {
            if let Some(value) = found(&supervisor.status()) {
                return value;
            }
            assert!(
                began.elapsed() < Duration::from_secs(10),
                "waited for {what}; the status is {:?}",
                supervisor.status()
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Every event up to and including the first that `last` picks.
    fn events_until(
        supervisor: &Supervisor,
        mut last: impl FnMut(&DeviceEvent) -> bool,
    ) -> Vec<DeviceEvent> {
        let mut events = Vec::new();
        let began = Instant::now();
        loop {
            for event in supervisor.events() {
                let done = last(&event);
                events.push(event);
                if done {
                    return events;
                }
            }
            assert!(
                began.elapsed() < Duration::from_secs(10),
                "waited for an event; had {events:?}"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn running_on(supervisor: &Supervisor, local: &str) -> StreamInfo {
        wait_for(
            supervisor,
            &format!("a stream on {local}"),
            |status| match status {
                DeviceStatus::Running(info)
                    if info.output_device.as_ref().unwrap().id.local() == local =>
                {
                    Some(info.clone())
                }
                _ => None,
            },
        )
    }

    fn interrupted(supervisor: &Supervisor) -> Interruption {
        wait_for(supervisor, "an interruption", |status| match status {
            DeviceStatus::Interrupted(interruption) => Some(interruption.clone()),
            _ => None,
        })
    }

    #[test]
    fn it_starts_on_the_requested_device() {
        let (supervisor, _control, client) = supervise(on("usb"), options());
        let info = running_on(&supervisor, "usb");
        assert_eq!(info.sample_rate, 48_000);
        assert!(client.playing());
        assert_eq!(supervisor.events(), [DeviceEvent::Started(info.clone())]);
        assert_eq!(client.log(), ["open usb 48000"]);
        assert_eq!(supervisor.status().message(), None);
        wait_for(&supervisor, "published counters", |_| {
            supervisor.stats().filter(|s| s.frames > 0)
        });
        assert_eq!(
            DeviceEvent::Started(info).message(),
            "Audio is running on \"USB Interface\" at 48000 Hz."
        );
    }

    // Spec: device-changes R1, R2
    #[test]
    fn unplugging_interrupts_and_replugging_recovers() {
        let (supervisor, control, client) = supervise(on("usb"), options());
        running_on(&supervisor, "usb");
        control.unplug("usb");
        let interruption = interrupted(&supervisor);
        assert_eq!(interruption.cause, Cause::DeviceLost);
        assert_eq!(interruption.device.as_ref().unwrap().name, "USB Interface");
        assert!(interruption.stream_stats.is_some_and(|s| s.device_lost));
        assert_eq!(
            supervisor.status().message().unwrap(),
            "The audio device \"USB Interface\" was disconnected. Audio is off until it's connected again."
        );
        assert!(!client.playing());

        // It keeps trying, and counts.
        let tried = wait_for(&supervisor, "three attempts", |status| match status {
            DeviceStatus::Interrupted(i) if i.attempts >= 3 => Some(i.clone()),
            _ => None,
        });
        assert_eq!(tried.last_error.unwrap().kind(), ErrorKind::DeviceNotFound);

        control.replug("usb");
        running_on(&supervisor, "usb");
        assert!(client.playing());
        let events = supervisor.events();
        assert!(matches!(&events[1], DeviceEvent::Interrupted(i) if i.cause == Cause::DeviceLost));
        let DeviceEvent::Recovered {
            info,
            cause,
            after,
            attempts,
        } = &events[2]
        else {
            panic!("expected a recovery, not {events:?}");
        };
        assert_eq!(*cause, Cause::DeviceLost);
        assert!(*attempts >= 3);
        assert!(*after >= Duration::from_millis(80));
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(events.len(), 3);
        assert_eq!(
            client.log(),
            ["open usb 48000", "closed", "open usb 48000"],
            "the client closes before it opens again"
        );
    }

    // Spec: device-changes R3
    #[test]
    fn a_sample_rate_change_reopens_at_the_new_rate() {
        let request = StreamRequest {
            sample_rate: Some(48_000),
            ..on("usb")
        };
        let (supervisor, control, client) = supervise(request, options());
        running_on(&supervisor, "usb");
        control.set_sample_rate("usb", 44_100);
        let info = wait_for(&supervisor, "the new rate", |status| match status {
            DeviceStatus::Running(info) if info.sample_rate == 44_100 => Some(info.clone()),
            _ => None,
        });
        assert!(client.playing());
        assert_eq!(control.sample_rate("usb"), 44_100, "not set back");
        assert_eq!(
            supervisor.request().sample_rate,
            None,
            "it follows the device now"
        );
        let events = supervisor.events();
        let DeviceEvent::Interrupted(interruption) = &events[1] else {
            panic!("expected an interruption, not {events:?}");
        };
        assert_eq!(
            interruption.cause,
            Cause::SampleRateChanged {
                from: 48_000,
                to: 44_100
            }
        );
        assert_eq!(
            interruption.message(),
            "The sample rate of \"USB Interface\" changed from 48000 Hz to 44100 Hz. Audio is restarting at the new rate."
        );
        assert!(matches!(
            &events[2],
            DeviceEvent::Recovered { info: i, attempts: 1, .. } if *i == info
        ));
        assert_eq!(client.log(), ["open usb 48000", "closed", "open usb 44100"]);
    }

    // Spec: device-changes R4
    #[test]
    fn sleep_stops_the_stream_and_wake_reopens_it() {
        let (supervisor, control, client) = supervise(on("usb"), options());
        running_on(&supervisor, "usb");
        control.sleep();
        let interruption = interrupted(&supervisor);
        assert_eq!(interruption.cause, Cause::Sleep);
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            client.log(),
            ["open usb 48000", "closed"],
            "nothing is tried while asleep"
        );
        control.wake();
        running_on(&supervisor, "usb");
        assert!(client.playing());
        let events = supervisor.events();
        assert!(
            matches!(&events[2], DeviceEvent::Woke { slept: Some(s) } if *s >= Duration::from_millis(150))
        );
        assert!(matches!(
            &events[3],
            DeviceEvent::Recovered {
                cause: Cause::Sleep,
                ..
            }
        ));
    }

    // Spec: device-changes R4
    #[test]
    fn a_wake_noticed_afterwards_leaves_a_healthy_stream_alone() {
        let (supervisor, control, client) = supervise(on("usb"), options());
        running_on(&supervisor, "usb");
        control.notify(PowerEvent::Woke {
            slept: Some(Duration::from_secs(600)),
        });
        let events = events_until(&supervisor, |e| matches!(e, DeviceEvent::Woke { .. }));
        assert_eq!(events.len(), 2, "{events:?}");
        thread::sleep(Duration::from_millis(100));
        assert!(matches!(supervisor.status(), DeviceStatus::Running(_)));
        assert!(supervisor.events().is_empty());
        assert_eq!(client.log(), ["open usb 48000"]);
        assert_eq!(
            DeviceEvent::Woke {
                slept: Some(Duration::from_secs(600))
            }
            .message(),
            "The computer woke after sleeping for 600.0 s."
        );
    }

    // Spec: device-changes R5
    #[test]
    fn a_stalled_stream_is_restarted() {
        let (supervisor, control, client) = supervise(on("usb"), options());
        running_on(&supervisor, "usb");
        control.stall();
        // It reopens at once, so the interruption shows in the events.
        let events = events_until(&supervisor, |e| matches!(e, DeviceEvent::Recovered { .. }));
        assert!(matches!(&events[1], DeviceEvent::Interrupted(i) if i.cause == Cause::Stalled));
        assert!(matches!(
            &events[2],
            DeviceEvent::Recovered {
                cause: Cause::Stalled,
                ..
            }
        ));
        running_on(&supervisor, "usb");
        assert!(client.playing());
        assert_eq!(client.log(), ["open usb 48000", "closed", "open usb 48000"]);
    }

    // Spec: device-changes R6
    #[test]
    fn a_stream_on_the_default_device_moves_with_it() {
        let request = StreamRequest {
            buffer_size: Some(128),
            ..StreamRequest::output(DeviceSelector::Default)
        };
        let (supervisor, control, client) = supervise(request, options());
        running_on(&supervisor, "built-in");
        control.unplug("built-in");
        events_until(&supervisor, |e| *e == DeviceEvent::DefaultDeviceChanged);
        assert!(client.playing());
        assert!(matches!(supervisor.status(), DeviceStatus::Running(_)));
        // With no device left, it's lost, and comes back with one.
        control.unplug("usb");
        let interruption = interrupted(&supervisor);
        assert_eq!(interruption.cause, Cause::DeviceLost);
        // It had moved since it opened, so it names no particular device.
        assert_eq!(interruption.device, None);
        assert_eq!(
            interruption.message(),
            "The default audio device was disconnected. Audio is off until it's connected again."
        );
        control.replug("usb");
        running_on(&supervisor, "usb");
        assert!(client.playing());
    }

    // Spec: device-changes R8
    #[test]
    fn falling_back_to_the_default_is_off_unless_asked() {
        let (supervisor, control, _client) = supervise(on("usb"), options());
        running_on(&supervisor, "usb");
        control.unplug("usb");
        interrupted(&supervisor);
        thread::sleep(Duration::from_millis(100));
        assert!(matches!(supervisor.status(), DeviceStatus::Interrupted(_)));
    }

    // Spec: device-changes R8
    #[test]
    fn the_fallback_moves_back_when_the_device_returns() {
        let options = SupervisorOptions {
            fall_back_to_default: true,
            ..options()
        };
        let (supervisor, control, client) = supervise(on("usb"), options);
        running_on(&supervisor, "usb");
        control.unplug("usb");
        let (info, interruption) = wait_for(&supervisor, "the fallback", |status| match status {
            DeviceStatus::Fallback { info, interruption } => {
                Some((info.clone(), interruption.clone()))
            }
            _ => None,
        });
        assert_eq!(info.output_device.as_ref().unwrap().id.local(), "built-in");
        assert_eq!(interruption.cause, Cause::DeviceLost);
        assert_eq!(
            supervisor.status().message().unwrap(),
            "The audio device \"USB Interface\" is missing. Audio is playing through \"Built-in Output\" until it's back."
        );
        assert!(client.playing());
        control.replug("usb");
        running_on(&supervisor, "usb");
        assert!(client.playing());
        assert_eq!(
            client.log(),
            [
                "open usb 48000",
                "closed",
                "open built-in 48000",
                "closed",
                "open usb 48000"
            ]
        );
        let events = supervisor.events();
        assert!(matches!(&events[2], DeviceEvent::FellBack { .. }));
        assert!(matches!(
            &events[3],
            DeviceEvent::Recovered {
                cause: Cause::DeviceLost,
                ..
            }
        ));
    }

    // Spec: device-changes R9
    #[test]
    fn reconfiguring_moves_to_another_device() {
        let (supervisor, _control, client) = supervise(on("usb"), options());
        running_on(&supervisor, "usb");
        let info = supervisor
            .reconfigure(StreamRequest {
                sample_rate: Some(96_000),
                ..on("built-in")
            })
            .unwrap();
        assert_eq!(info.sample_rate, 96_000);
        assert_eq!(running_on(&supervisor, "built-in"), info);
        assert_eq!(supervisor.request().sample_rate, Some(96_000));
        assert!(client.playing());
        assert_eq!(
            client.log(),
            ["open usb 48000", "closed", "open built-in 96000"]
        );
        assert_eq!(supervisor.events()[1], DeviceEvent::Started(info));
    }

    // Spec: device-changes R9
    #[test]
    fn a_failed_reconfigure_reopens_the_previous_settings() {
        let (supervisor, _control, client) = supervise(on("usb"), options());
        running_on(&supervisor, "usb");
        let error = supervisor.reconfigure(on("nowhere")).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DeviceNotFound);
        running_on(&supervisor, "usb");
        assert_eq!(supervisor.request(), on("usb"));
        assert!(client.playing());
    }

    // Spec: device-changes R2
    #[test]
    fn a_device_missing_at_start_is_waited_for() {
        let (backend, control) = NullBackend::with_devices(
            NullOptions::default(),
            &[("built-in", "Built-in Output"), ("usb", "USB Interface")],
        );
        control.unplug("usb");
        let client = Client::default();
        let supervisor = Supervisor::start_with_power(
            Box::new(move || Ok(Box::new(backend) as Box<dyn Backend>)),
            on("usb"),
            Box::new(client.clone()),
            options(),
            Box::new(control.power()),
        )
        .unwrap();
        let interruption = interrupted(&supervisor);
        assert_eq!(interruption.cause, Cause::Unavailable);
        assert_eq!(interruption.device.as_ref().unwrap().name, "null:usb");
        assert_eq!(
            interruption.message(),
            "The audio device \"null:usb\" can't be opened (the null backend has no device \
             `null:usb` now). Trying again."
        );
        control.replug("usb");
        running_on(&supervisor, "usb");
        assert!(client.playing());
    }

    #[test]
    fn a_client_that_cannot_open_is_tried_again() {
        let (backend, control) =
            NullBackend::with_devices(NullOptions::default(), &[("usb", "USB Interface")]);
        let client = Client::default();
        client.refuse.store(true, Relaxed);
        let supervisor = Supervisor::start_with_power(
            Box::new(move || Ok(Box::new(backend) as Box<dyn Backend>)),
            on("usb"),
            Box::new(client.clone()),
            options(),
            Box::new(control.power()),
        )
        .unwrap();
        running_on(&supervisor, "usb");
        assert_eq!(client.log(), ["refused", "open usb 48000"]);
        let events = supervisor.events();
        assert!(matches!(&events[0], DeviceEvent::Interrupted(i) if i.cause == Cause::Unavailable));
        assert!(matches!(&events[1], DeviceEvent::Recovered { .. }));
    }

    /// A callback that logs when it is dropped, as the engine's does when it
    /// hands its state back.
    struct Returns(Arc<Mutex<Vec<String>>>);

    impl AudioCallback for Returns {
        fn process(&mut self, _: &ProcessContext, _: &AudioBuffer<'_>, _: &mut AudioBufferMut<'_>) {
        }
    }

    impl Drop for Returns {
        fn drop(&mut self) {
            self.0.lock().unwrap().push("returned".to_owned());
        }
    }

    /// Logs the order of `returned` and `closed`.
    struct Returning(Arc<Mutex<Vec<String>>>);

    impl StreamClient for Returning {
        fn open(
            &mut self,
            _: &StreamInfo,
            _: Option<Workgroup>,
        ) -> Result<Box<dyn AudioCallback>, Error> {
            Ok(Box::new(Returns(Arc::clone(&self.0))))
        }

        fn closed(&mut self) {
            self.0.lock().unwrap().push("closed".to_owned());
        }
    }

    /// A backend whose first stream fails to start.
    struct FailsToStart(NullBackend, Arc<AtomicBool>);

    struct FailingStream(Box<dyn Stream>, bool);

    impl Backend for FailsToStart {
        fn kind(&self) -> BackendKind {
            self.0.kind()
        }

        fn devices(&self) -> Result<Vec<crate::DeviceInfo>, Error> {
            self.0.devices()
        }

        fn open_stream(
            &self,
            request: &StreamRequest,
            callback: Box<dyn AudioCallback>,
        ) -> Result<Box<dyn Stream>, Error> {
            let fail = self.1.swap(false, Relaxed);
            Ok(Box::new(FailingStream(
                self.0.open_stream(request, callback)?,
                fail,
            )))
        }
    }

    impl Stream for FailingStream {
        fn info(&self) -> &StreamInfo {
            self.0.info()
        }

        fn start(&mut self) -> Result<(), Error> {
            if self.1 {
                return Err(Error::new(ErrorKind::DeviceBusy, "the device is busy"));
            }
            self.0.start()
        }

        fn stop(&mut self) -> Result<(), Error> {
            self.0.stop()
        }

        fn stats(&self) -> StreamStats {
            self.0.stats()
        }

        fn current_buffer_size(&self) -> Option<u32> {
            self.0.current_buffer_size()
        }
    }

    // Spec: device-changes R2
    #[test]
    fn a_stream_that_fails_to_start_returns_the_callback_before_it_closes() {
        let (backend, control) =
            NullBackend::with_devices(NullOptions::default(), &[("usb", "USB Interface")]);
        let log = Arc::new(Mutex::new(Vec::new()));
        let supervisor = Supervisor::start_with_power(
            Box::new(move || {
                Ok(
                    Box::new(FailsToStart(backend, Arc::new(AtomicBool::new(true))))
                        as Box<dyn Backend>,
                )
            }),
            on("usb"),
            Box::new(Returning(Arc::clone(&log))),
            options(),
            Box::new(control.power()),
        )
        .unwrap();
        running_on(&supervisor, "usb");
        // The first attempt returns its callback, and only then closes.
        assert_eq!(log.lock().unwrap()[..2], ["returned", "closed"]);
    }

    /// A client whose opens take `4 s` of a manual clock, as a slow driver's does.
    struct Slow(Arc<Mutex<crate::ClockReading>>);

    impl StreamClient for Slow {
        fn open(
            &mut self,
            _: &StreamInfo,
            _: Option<Workgroup>,
        ) -> Result<Box<dyn AudioCallback>, Error> {
            let mut reading = self.0.lock().unwrap();
            reading.monotonic += Duration::from_secs(4);
            reading.wall += Duration::from_secs(4);
            Ok(Box::new(Count(Arc::default())))
        }
    }

    // Spec: device-changes R4
    #[test]
    fn a_slow_open_is_not_taken_for_a_wake() {
        let (backend, control) =
            NullBackend::with_devices(NullOptions::default(), &[("usb", "USB Interface")]);
        let reading = Arc::new(Mutex::new(crate::ClockReading {
            monotonic: Duration::ZERO,
            wall: Duration::from_secs(1_000_000),
        }));
        let clock = Arc::clone(&reading);
        let supervisor = Supervisor::start_with_power(
            Box::new(move || Ok(Box::new(backend) as Box<dyn Backend>)),
            on("usb"),
            Box::new(Slow(Arc::clone(&reading))),
            options(),
            Box::new(ClockJump::with_clock(Duration::from_secs(3), move || {
                *clock.lock().unwrap()
            })),
        )
        .unwrap();
        running_on(&supervisor, "usb");
        control.unplug("usb");
        interrupted(&supervisor);
        control.replug("usb");
        running_on(&supervisor, "usb");
        // The next poll after the slow open must find no wake.
        control.unplug("usb");
        interrupted(&supervisor);
        let events = supervisor.events();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, DeviceEvent::Woke { .. })),
            "{events:?}"
        );
    }

    #[test]
    fn stopping_closes_the_stream() {
        let (supervisor, _control, client) = supervise(on("usb"), options());
        running_on(&supervisor, "usb");
        supervisor.stop();
        assert_eq!(client.log(), ["open usb 48000", "closed"]);
        assert!(!client.playing());
    }

    #[test]
    fn a_backend_that_cannot_be_made_fails_the_start() {
        let error = Supervisor::start(
            Box::new(|| Err(Error::new(ErrorKind::BackendUnavailable, "no such backend"))),
            on("usb"),
            Box::new(Client::default()),
            options(),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::BackendUnavailable);
    }

    #[test]
    fn messages_name_the_device_and_what_happens_next() {
        let interruption = |cause| Interruption {
            cause,
            device: Some(OpenDevice {
                id: DeviceId::new(BackendKind::Null, "usb"),
                name: "USB Interface".to_owned(),
            }),
            since: Instant::now(),
            attempts: 2,
            last_error: Some(Error::new(ErrorKind::DeviceBusy, "the device is busy")),
            stream_stats: None,
        };
        assert_eq!(
            interruption(Cause::Stalled).message(),
            "The audio device \"USB Interface\" stopped responding. Audio is restarting. The last attempt failed: the device is busy."
        );
        assert_eq!(
            interruption(Cause::Sleep).message(),
            "Audio stopped while the computer slept, and restarts when it wakes."
        );
        assert_eq!(
            interruption(Cause::Unavailable).message(),
            "The audio device \"USB Interface\" can't be opened (the device is busy). Trying again."
        );
        let default = Interruption {
            device: None,
            ..interruption(Cause::DeviceLost)
        };
        assert_eq!(
            default.message(),
            "The default audio device was disconnected. Audio is off until it's connected again."
        );
        assert_eq!(DeviceStatus::Stopped.message(), None);
    }

    /// Hands a callback over and runs blocks through it, with every block a
    /// real-time section under the allocation trap.
    // Spec: device-changes R10
    #[test]
    fn the_hand_off_stays_off_the_heap() {
        let (mut sender, incoming) = RingBuffer::<Box<dyn AudioCallback>>::new(1);
        let mut handoff = Handoff {
            incoming,
            current: None,
        };
        let blocks = Arc::new(AtomicU64::new(0));
        let mut data = vec![0.0f32; 2 * 128];
        let ctx = ProcessContext {
            sample_rate: 48_000,
            frames: 128,
            position: 0,
            timestamp: None,
        };
        let mut run = |handoff: &mut Handoff| {
            let mut output = AudioBufferMut::new(&mut data, 2, 128, 128);
            bl_rt::rt_test!({ handoff.process(&ctx, &AudioBuffer::empty(128), &mut output) });
        };
        run(&mut handoff);
        assert_eq!(
            blocks.load(Relaxed),
            0,
            "silence until the callback arrives"
        );
        let callback: Box<dyn AudioCallback> = Box::new(Count(Arc::clone(&blocks)));
        assert!(sender.push(callback).is_ok());
        for _ in 0..3 {
            run(&mut handoff);
        }
        assert_eq!(blocks.load(Relaxed), 3);
    }
}
