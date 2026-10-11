//! Audio device backends per platform behind one full-duplex trait.
//!
//! Beatlayer owns this trait (decision D9 in the architecture notes), so a
//! backend can change underneath without touching the engine. The pieces:
//!
//! - [`Backend`]: one audio API on this machine (Core Audio, WASAPI, ASIO,
//!   PipeWire, JACK, ALSA, or the null device). It lists [`DeviceInfo`]s and
//!   opens [`Stream`]s.
//! - [`AudioCallback`]: the code that makes sound. A stream calls it once per
//!   block on the audio thread with the block's input and output, both
//!   planar `f32`, so recording (WP-2.13) uses the same callback as playback.
//! - [`Stream`]: starts and stops the device and reports what it actually
//!   runs at ([`StreamInfo`]) and what went wrong ([`StreamStats`]).
//! - [`Supervisor`]: keeps a stream running through device changes. When the
//!   device goes away, its sample rate changes, the stream stalls, or the
//!   machine sleeps, it closes the stream, says why ([`DeviceStatus`],
//!   [`DeviceEvent`]), and opens it again when the device can run, asking a
//!   [`StreamClient`] for the callback each time (WP-1.13).
//!
//! # Backends
//!
//! | Backend | Platform | Built on | Duplex |
//! |---|---|---|---|
//! | Core Audio | macOS | `cpal` | not yet |
//! | WASAPI (shared) | Windows | `cpal` | not yet |
//! | ASIO | Windows, `asio` feature | `azo` | yes |
//! | JACK, ALSA | Linux | `cpal` | not yet |
//! | Null | all | a timer thread | yes |
//!
//! `cpal` 0.18 has no duplex streams on Core Audio or PipeWire, so its
//! backends refuse a request with input. Beatlayer's own Core Audio backend
//! replaces it on macOS in a later package.
//!
//! On Linux, PipeWire plays through its ALSA plug-in and its JACK server.
//! `cpal`'s native PipeWire host stays off until `deny.toml` allows the
//! licence of one of its build-time dependencies (see `Cargo.toml`).
//!
//! # The audio thread
//!
//! Every backend hands each block to one function, `CallbackRunner::run` in
//! `runner.rs`, on whatever thread the backend uses for audio: Core Audio's
//! I/O thread, WASAPI's or ALSA's render thread, the ASIO driver's thread, or
//! the null device's `bl-audio-null` thread. That makes it the one place for
//! the real-time harness in `bl-rt`:
//!
//! - The first block on each new thread sets the thread up: real-time
//!   priority (MMCSS "Pro Audio" on Windows, `SCHED_FIFO` or rtkit on Linux,
//!   the Mach time-constraint policy on macOS) unless the backend already
//!   gave it that, and on macOS the output device's audio workgroup. A
//!   backend can move its callback to a new thread mid-stream (Core Audio
//!   does when the default device changes), so set-up keys on the thread,
//!   not the stream. [`StreamStats::audio_thread`] reports the outcome.
//! - Every block runs as a real-time section: denormals flushed to zero,
//!   heap use counted by the allocation trap in builds that install it
//!   ([`StreamStats::heap_operations`]), and RealtimeSanitizer watching in
//!   builds with the `rtsan` feature (`cargo xtask rtsan`).

#[cfg(test)]
bl_rt::install_trap!();

mod buffer;
mod cpal_backend;
mod error;
mod null;
mod power;
mod runner;
mod stats;
mod supervisor;

#[cfg(all(windows, feature = "asio"))]
mod asio;
#[cfg(any(test, all(windows, feature = "asio")))]
mod wire;

use std::fmt;
use std::str::FromStr;

pub use bl_rt::{Priority, Promotion, ThreadReport, Workgroup, WorkgroupJoin};
pub use buffer::{AudioBuffer, AudioBufferMut};
pub use error::{Error, ErrorKind};
pub use null::{NullBackend, NullControl, NullOptions, NullPower, Pacing};
pub use power::{ClockJump, ClockReading, DEFAULT_SLEEP_THRESHOLD, PowerEvent, PowerMonitor};
pub use stats::{DROPOUT_MERGE, StreamStats};
pub use supervisor::{
    BackendFactory, Cause, DeviceEvent, DeviceStatus, Interruption, StreamClient, Supervisor,
    SupervisorOptions,
};

/// The sample rates Beatlayer offers when a device accepts a range.
pub const STANDARD_SAMPLE_RATES: [u32; 13] = [
    8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
    352_800, 384_000,
];

/// The largest block a stream passes to [`AudioCallback::process`] unless the
/// requested buffer is larger. Bigger device buffers are split into blocks of
/// at most this size.
pub const DEFAULT_MAX_BLOCK_FRAMES: u32 = 4096;

/// An audio API that Beatlayer can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BackendKind {
    /// Apple's Core Audio (macOS).
    CoreAudio,
    /// Windows Audio Session API, shared mode.
    Wasapi,
    /// Steinberg's ASIO driver model (Windows).
    Asio,
    /// PipeWire (Linux). Not built yet: PipeWire plays through
    /// [`Alsa`](Self::Alsa) and [`Jack`](Self::Jack) meanwhile.
    PipeWire,
    /// JACK, the low-latency audio server (Linux).
    Jack,
    /// Advanced Linux Sound Architecture. On many systems this is PipeWire's
    /// or PulseAudio's ALSA plug-in rather than the hardware.
    Alsa,
    /// A device that runs the callback on a timer without hardware, for tests
    /// and CI.
    Null,
}

impl BackendKind {
    /// Every backend, whether or not this build or machine has it.
    pub const ALL: [Self; 7] = [
        Self::CoreAudio,
        Self::Wasapi,
        Self::Asio,
        Self::PipeWire,
        Self::Jack,
        Self::Alsa,
        Self::Null,
    ];

    /// The lowercase name used in device IDs and on the command line.
    pub const fn id(self) -> &'static str {
        match self {
            Self::CoreAudio => "coreaudio",
            Self::Wasapi => "wasapi",
            Self::Asio => "asio",
            Self::PipeWire => "pipewire",
            Self::Jack => "jack",
            Self::Alsa => "alsa",
            Self::Null => "null",
        }
    }

    /// The name to show a person.
    pub const fn label(self) -> &'static str {
        match self {
            Self::CoreAudio => "Core Audio",
            Self::Wasapi => "WASAPI",
            Self::Asio => "ASIO",
            Self::PipeWire => "PipeWire",
            Self::Jack => "JACK",
            Self::Alsa => "ALSA",
            Self::Null => "Null",
        }
    }

    /// Looks a backend up by its [`id`](Self::id), ignoring case.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.id().eq_ignore_ascii_case(id))
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A stable identifier for a device: `<backend>:<backend's own ID>`, for
/// example `coreaudio:BuiltInSpeakerDevice` or `null:null`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    /// Makes an ID from a backend and the backend's own identifier.
    pub fn new(backend: BackendKind, local: &str) -> Self {
        Self(format!("{}:{local}", backend.id()))
    }

    /// The backend this device belongs to.
    pub fn backend(&self) -> BackendKind {
        let (prefix, _) = self.0.split_once(':').unwrap_or((&self.0, ""));
        // `new` and `from_str` only build IDs with a known prefix.
        BackendKind::from_id(prefix).unwrap_or(BackendKind::Null)
    }

    /// The backend's own identifier, after the colon.
    pub fn local(&self) -> &str {
        self.0.split_once(':').map_or("", |(_, local)| local)
    }

    /// The whole ID as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for DeviceId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let (prefix, local) = s.split_once(':').ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("`{s}` isn't a device ID; IDs look like `backend:device`"),
            )
        })?;
        let backend = BackendKind::from_id(prefix).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("`{prefix}` in `{s}` isn't a backend"),
            )
        })?;
        Ok(Self::new(backend, local))
    }
}

/// The buffer sizes a device accepts, in frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferSizes {
    /// Any size from `min` to `max`, inclusive.
    Range {
        /// The smallest size.
        min: u32,
        /// The largest size.
        max: u32,
    },
    /// Only these sizes, in ascending order.
    List(Vec<u32>),
    /// The backend can't say before a stream runs.
    Unknown,
}

impl BufferSizes {
    /// Whether `frames` is allowed, or `None` if the backend can't say.
    pub fn contains(&self, frames: u32) -> Option<bool> {
        match self {
            Self::Range { min, max } => Some((*min..=*max).contains(&frames)),
            Self::List(sizes) => Some(sizes.contains(&frames)),
            Self::Unknown => None,
        }
    }

    /// The allowed size closest to `frames`, preferring the larger on a tie,
    /// or `None` if the backend can't say.
    pub fn nearest(&self, frames: u32) -> Option<u32> {
        match self {
            Self::Range { min, max } => Some(frames.clamp(*min, *max)),
            Self::List(sizes) => sizes
                .iter()
                .copied()
                .min_by_key(|size| (size.abs_diff(frames), u32::MAX - size)),
            Self::Unknown => None,
        }
    }
}

impl fmt::Display for BufferSizes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Range { min, max } => write!(f, "{min} to {max} frames"),
            Self::List(sizes) => {
                let sizes: Vec<String> = sizes.iter().map(u32::to_string).collect();
                write!(f, "{} frames", sizes.join(", "))
            }
            Self::Unknown => f.write_str("unknown until a stream runs"),
        }
    }
}

/// What a device is and what it can run at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The device's stable ID.
    pub id: DeviceId,
    /// The name the system gives the device.
    pub name: String,
    /// Whether the system uses this device for output by default.
    pub is_default_output: bool,
    /// Whether the system uses this device for input by default.
    pub is_default_input: bool,
    /// The most output channels a stream can open.
    pub output_channels: u16,
    /// The most input channels a stream can open.
    pub input_channels: u16,
    /// The sample rates the device accepts, in hertz, ascending.
    pub sample_rates: Vec<u32>,
    /// The rate the device runs at now, if known. A stream that doesn't ask
    /// for a rate runs at this one, so opening it changes nothing.
    pub current_sample_rate: Option<u32>,
    /// The buffer sizes the device accepts.
    pub buffer_sizes: BufferSizes,
    /// The buffer size the device prefers, if it says.
    pub default_buffer_size: Option<u32>,
}

impl DeviceInfo {
    /// The backend this device belongs to.
    pub fn backend(&self) -> BackendKind {
        self.id.backend()
    }
}

/// Which device a stream should use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DeviceSelector {
    /// The system's default device. Where the backend supports it (Core Audio
    /// and WASAPI), the stream follows the default when it changes.
    #[default]
    Default,
    /// One device, which the stream stays on.
    Id(DeviceId),
}

/// One direction of a stream request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortRequest {
    /// The device to use.
    pub device: DeviceSelector,
    /// How many channels to open, or `None` for the device's default.
    pub channels: Option<u16>,
}

/// What a stream should open. `None` fields take the device's current or
/// default value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamRequest {
    /// Output, or `None` for an input-only stream.
    pub output: Option<PortRequest>,
    /// Input, or `None` for an output-only stream.
    pub input: Option<PortRequest>,
    /// The sample rate in hertz, or `None` for the device's current rate.
    pub sample_rate: Option<u32>,
    /// The buffer size in frames, or `None` for the device's default.
    pub buffer_size: Option<u32>,
}

impl StreamRequest {
    /// An output-only request for `device` with the device's defaults.
    pub fn output(device: DeviceSelector) -> Self {
        Self {
            output: Some(PortRequest {
                device,
                channels: None,
            }),
            ..Self::default()
        }
    }
}

/// How samples travel between Beatlayer and the device. Callbacks always see
/// `f32`; the backend converts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleFormat {
    /// Signed 8-bit integer.
    I8,
    /// Signed 16-bit integer.
    I16,
    /// Signed 24-bit integer.
    I24,
    /// Signed 32-bit integer.
    I32,
    /// Signed 64-bit integer.
    I64,
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit integer.
    U16,
    /// Unsigned 24-bit integer.
    U24,
    /// Unsigned 32-bit integer.
    U32,
    /// Unsigned 64-bit integer.
    U64,
    /// 32-bit float.
    F32,
    /// 64-bit float.
    F64,
}

impl fmt::Display for SampleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I24 => "i24",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U24 => "u24",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::F32 => "f32",
            Self::F64 => "f64",
        };
        f.write_str(name)
    }
}

/// A device a stream has open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDevice {
    /// The device's ID.
    pub id: DeviceId,
    /// The device's name.
    pub name: String,
}

/// What a stream actually runs at, which can differ from the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    /// The backend running the stream.
    pub backend: BackendKind,
    /// The output device, if the stream has output.
    pub output_device: Option<OpenDevice>,
    /// The input device, if the stream has input.
    pub input_device: Option<OpenDevice>,
    /// Whether the stream follows the system default output device when it
    /// changes (Core Audio and WASAPI, when no device was named).
    pub follows_default: bool,
    /// The sample rate in hertz.
    pub sample_rate: u32,
    /// Output channels.
    pub output_channels: u16,
    /// Input channels.
    pub input_channels: u16,
    /// The format on the device side.
    pub sample_format: SampleFormat,
    /// The buffer size the request asked for.
    pub requested_buffer_size: Option<u32>,
    /// The buffer size the backend says it set, if it says. Backends may not
    /// honour the request (WASAPI often doesn't); the block sizes the device
    /// actually delivered are in [`StreamStats`].
    pub buffer_size: Option<u32>,
    /// The largest block [`AudioCallback::process`] receives.
    pub max_block_frames: u32,
    /// The macOS audio workgroup the stream's audio thread works in, as it
    /// was when the stream opened. `None` on other platforms.
    pub workgroup: Option<WorkgroupInfo>,
}

/// What a stream knows about its audio workgroup (macOS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkgroupInfo {
    /// Where it comes from, for example "the I/O workgroup of MacBook Pro
    /// Speakers".
    pub label: String,
    /// The operating system's own description of it, which includes its name.
    pub description: String,
    /// How many threads the system suggests running in it at once: the size
    /// for a real-time worker pool.
    pub max_parallel_threads: Option<u32>,
}

impl WorkgroupInfo {
    /// Describes `workgroup`.
    pub fn of(workgroup: &Workgroup) -> Self {
        Self {
            label: workgroup.label().to_owned(),
            description: workgroup.description(),
            max_parallel_threads: workgroup.max_parallel_threads(),
        }
    }
}

/// Facts about one block, passed to [`AudioCallback::process`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessContext {
    /// The stream's sample rate in hertz.
    pub sample_rate: u32,
    /// Frames in this block.
    pub frames: usize,
    /// Frames the stream played before this block, including any silent
    /// block in which a new audio thread was set up.
    pub position: u64,
    /// When the device handles this block, on the backend's clock, if the
    /// backend says. Only differences between timestamps mean anything.
    pub timestamp: Option<std::time::Duration>,
}

/// Code that runs on the audio thread once per block.
///
/// `process` must follow the real-time rules in the architecture: no
/// allocation, no locks, no blocking calls, bounded work.
pub trait AudioCallback: Send + 'static {
    /// Reads `input` and fills `output` for one block. `output` arrives
    /// filled with silence. Both have `ctx.frames` frames.
    fn process(
        &mut self,
        ctx: &ProcessContext,
        input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    );
}

/// One audio API on this machine.
pub trait Backend {
    /// Which API this is.
    fn kind(&self) -> BackendKind;

    /// The devices this backend can open, with what each accepts.
    fn devices(&self) -> Result<Vec<DeviceInfo>, Error>;

    /// Opens a stream that will call `callback` once it's started.
    fn open_stream(
        &self,
        request: &StreamRequest,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn Stream>, Error>;
}

/// A running or paused connection to a device.
///
/// Dropping a stream stops it.
pub trait Stream {
    /// What the stream runs at.
    fn info(&self) -> &StreamInfo;

    /// Starts calling the callback.
    fn start(&mut self) -> Result<(), Error>;

    /// Stops calling the callback.
    fn stop(&mut self) -> Result<(), Error>;

    /// Counters since the stream opened. Safe to call while it runs.
    fn stats(&self) -> StreamStats;

    /// The buffer size the backend reports now, which can change while the
    /// stream runs (for example after the default device changes).
    fn current_buffer_size(&self) -> Option<u32>;

    /// The audio workgroup the stream's audio thread works in (macOS), for
    /// other real-time threads that work to the same deadline to join. It's
    /// the one the stream opened with: a stream that follows the default
    /// device keeps it after the default changes.
    fn workgroup(&self) -> Option<Workgroup> {
        None
    }
}

/// The backends this build has and this machine can run, best first.
pub fn available_backends() -> Vec<BackendKind> {
    let mut kinds = cpal_backend::available();
    if cfg!(all(windows, feature = "asio")) {
        kinds.push(BackendKind::Asio);
    }
    kinds.push(BackendKind::Null);
    kinds
}

/// The backend Beatlayer uses when none is named: Core Audio on macOS,
/// WASAPI on Windows, and ALSA on Linux.
pub fn default_backend() -> BackendKind {
    cpal_backend::default_kind()
}

/// Opens a backend.
pub fn open_backend(kind: BackendKind) -> Result<Box<dyn Backend>, Error> {
    match kind {
        BackendKind::Null => Ok(Box::new(NullBackend::new())),
        BackendKind::Asio => open_asio(),
        _ => Ok(Box::new(cpal_backend::CpalBackend::new(kind)?)),
    }
}

#[cfg(all(windows, feature = "asio"))]
fn open_asio() -> Result<Box<dyn Backend>, Error> {
    Ok(Box::new(asio::AsioBackend::new()))
}

#[cfg(not(all(windows, feature = "asio")))]
fn open_asio() -> Result<Box<dyn Backend>, Error> {
    Err(Error::new(
        ErrorKind::BackendUnavailable,
        "ASIO needs Windows and a build with the `asio` feature",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_ids_round_trip() {
        for kind in BackendKind::ALL {
            assert_eq!(BackendKind::from_id(kind.id()), Some(kind));
            assert_eq!(BackendKind::from_id(&kind.id().to_uppercase()), Some(kind));
        }
        assert_eq!(BackendKind::from_id("dsound"), None);
    }

    #[test]
    fn device_ids_keep_colons_in_the_local_part() {
        let id: DeviceId = "alsa:hw:0,0".parse().unwrap();
        assert_eq!(id.backend(), BackendKind::Alsa);
        assert_eq!(id.local(), "hw:0,0");
        assert_eq!(id.to_string(), "alsa:hw:0,0");
        assert_eq!(DeviceId::new(BackendKind::Alsa, "hw:0,0"), id);
    }

    #[test]
    fn device_ids_need_a_known_backend() {
        assert_eq!(
            "nothing".parse::<DeviceId>().unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(
            "dsound:x".parse::<DeviceId>().unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn buffer_sizes_answer_contains_and_nearest() {
        let range = BufferSizes::Range { min: 32, max: 2048 };
        assert_eq!(range.contains(128), Some(true));
        assert_eq!(range.contains(16), Some(false));
        assert_eq!(range.nearest(16), Some(32));
        assert_eq!(range.nearest(4096), Some(2048));

        let list = BufferSizes::List(vec![64, 128, 256]);
        assert_eq!(list.contains(128), Some(true));
        assert_eq!(list.contains(100), Some(false));
        assert_eq!(list.nearest(100), Some(128));
        assert_eq!(list.nearest(96), Some(128), "a tie goes to the larger size");
        assert_eq!(list.nearest(1000), Some(256));

        assert_eq!(BufferSizes::Unknown.contains(128), None);
        assert_eq!(BufferSizes::Unknown.nearest(128), None);
    }

    #[test]
    fn the_null_backend_is_always_available() {
        assert!(available_backends().contains(&BackendKind::Null));
        assert_eq!(
            open_backend(BackendKind::Null).unwrap().kind(),
            BackendKind::Null
        );
    }

    #[cfg(not(all(windows, feature = "asio")))]
    #[test]
    fn asio_is_unavailable_without_windows_and_the_feature() {
        let error = open_backend(BackendKind::Asio).err().unwrap();
        assert_eq!(error.kind(), ErrorKind::BackendUnavailable);
    }
}
