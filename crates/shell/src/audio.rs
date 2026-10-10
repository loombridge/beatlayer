//! The audio host: the device the app plays through, the settings that
//! choose it, and the CPU meter (L-SET-012, L-CTRL-023, L-CTRL-024).
//!
//! [`AudioHost`] owns one stream from `bl-audio-io`. Changing a setting
//! ([`AudioHost::configure`]) closes the stream and opens a new one with the
//! new settings straight away, so a device, rate, buffer, or channel change
//! applies without restarting the app. If the device refuses, the previous
//! settings are reopened and the error is reported.
//!
//! Until the engine exists (WP-0.9), the stream's callback is a stand-in
//! that plays silence, or the Settings window's test tone at -60 dBFS, and
//! measures how long it takes, which the CPU meter shows.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, BackendKind, BufferSizes, DeviceId,
    DeviceInfo, DeviceSelector, Error, PortRequest, ProcessContext, Stream, StreamRequest,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The level of the Settings window's test tone, in dBFS. Never louder.
pub const TEST_TONE_DBFS: f64 = -60.0;
/// The test tone's frequency in hertz.
pub const TEST_TONE_HZ: f64 = 440.0;

/// The test tone's peak amplitude: [`TEST_TONE_DBFS`] as a linear gain.
#[must_use]
pub fn test_tone_amplitude() -> f32 {
    10f64.powf(TEST_TONE_DBFS / 20.0) as f32
}

/// The audio device settings, as the Settings window's Audio page sets them
/// and the preferences file keeps them. `None` means the platform's or the
/// device's own choice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AudioSettings {
    /// The driver type's ID (`coreaudio`, `wasapi`, `asio`, `jack`, `alsa`,
    /// or `null`), or nothing for the platform's default.
    pub driver: Option<String>,
    /// The output device's ID, or nothing for the system's default output
    /// device, which the stream then follows where the driver can.
    pub output_device: Option<String>,
    /// The input device's ID, or nothing for no input.
    pub input_device: Option<String>,
    /// The sample rate in hertz, or nothing for the device's current rate,
    /// which leaves the device's own setting alone.
    pub sample_rate: Option<u32>,
    /// The buffer size in frames, or nothing for the device's default.
    pub buffer_size: Option<u32>,
    /// Output channels to open, or nothing for all the device has.
    pub output_channels: Option<u16>,
}

/// Arguments of `audio.configure`: the settings to change. Fields left out
/// keep their values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureAudio {
    /// The driver type: an ID from `audio.devices`, such as `coreaudio`,
    /// `wasapi`, `asio`, `jack`, `alsa`, or `null`. Changing it resets the
    /// devices to the driver's defaults unless they're given too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// The output device: an ID from `audio.devices`, or `default` for the
    /// system's default output device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// The input device: an ID from `audio.devices`, or `none` for no input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_device: Option<String>,
    /// The sample rate in hertz, or 0 for the device's current rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// The buffer size in frames, or 0 for the device's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buffer_size: Option<u32>,
    /// Output channels to open, or 0 for all the device has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_channels: Option<u16>,
}

/// A device in use, by ID and name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceRef {
    /// The device's ID, as `audio.configure` takes it.
    pub id: String,
    /// The name the system gives it.
    pub name: String,
}

/// What `audio.status` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioStatus {
    /// Whether the audio engine is on.
    pub engine_on: bool,
    /// The settings in force.
    pub settings: AudioSettings,
    /// The driver type in use (its ID).
    pub driver: String,
    /// The output device the stream opened, while one is open.
    pub output_device: Option<DeviceRef>,
    /// The input device the stream opened, if any.
    pub input_device: Option<DeviceRef>,
    /// Whether the stream follows the system's default output device.
    pub follows_default: bool,
    /// The sample rate the device runs at, in hertz, while a stream is open.
    pub sample_rate: Option<u32>,
    /// The buffer size the settings asked for, in frames.
    pub requested_buffer_size: Option<u32>,
    /// The buffer size the device actually delivers, in frames, once it has
    /// called back, or what the driver says it set until then.
    pub buffer_size: Option<u32>,
    /// Output channels open.
    pub output_channels: u16,
    /// Input channels open.
    pub input_channels: u16,
    /// The output buffer's length in milliseconds.
    pub buffer_ms: Option<f64>,
    /// The share of each buffer's time spent processing audio, in percent,
    /// averaged over the last measurement; nothing while the engine is off.
    pub cpu_load: Option<f32>,
    /// Dropouts since the stream opened.
    pub dropouts: u64,
    /// Whether the test tone plays.
    pub test_tone: bool,
    /// The last error from the device, if the stream couldn't open or failed.
    pub error: Option<String>,
}

/// A driver type this build and machine offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DriverInfo {
    /// Its ID, as `audio.configure` takes it.
    pub id: String,
    /// Its name.
    pub name: String,
}

/// One device, as `audio.devices` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceListing {
    /// The device's ID, as `audio.configure` takes it.
    pub id: String,
    /// The name the system gives it.
    pub name: String,
    /// Output channels it has.
    pub output_channels: u16,
    /// Input channels it has.
    pub input_channels: u16,
    /// Whether the system uses it for output by default.
    pub default_output: bool,
    /// Whether the system uses it for input by default.
    pub default_input: bool,
    /// The sample rates it accepts, in hertz.
    pub sample_rates: Vec<u32>,
    /// The rate it runs at now, if known.
    pub current_sample_rate: Option<u32>,
    /// The buffer sizes it accepts, in frames: every size from the first to
    /// the second number when `buffer_range` is true, otherwise exactly these.
    /// Empty if the driver can't say before a stream runs.
    pub buffer_sizes: Vec<u32>,
    /// Whether `buffer_sizes` is a range (its first and last values).
    pub buffer_range: bool,
    /// The buffer size it prefers, if it says.
    pub default_buffer_size: Option<u32>,
}

impl DeviceListing {
    fn from_info(info: &DeviceInfo) -> Self {
        let (buffer_sizes, buffer_range) = match &info.buffer_sizes {
            BufferSizes::Range { min, max } => (vec![*min, *max], true),
            BufferSizes::List(sizes) => (sizes.clone(), false),
            BufferSizes::Unknown => (Vec::new(), false),
        };
        Self {
            id: info.id.to_string(),
            name: info.name.clone(),
            output_channels: info.output_channels,
            input_channels: info.input_channels,
            default_output: info.is_default_output,
            default_input: info.is_default_input,
            sample_rates: info.sample_rates.clone(),
            current_sample_rate: info.current_sample_rate,
            buffer_sizes,
            buffer_range,
            default_buffer_size: info.default_buffer_size,
        }
    }

    /// The buffer sizes to offer in a chooser: powers of two within the
    /// device's range, or its own list.
    #[must_use]
    pub fn buffer_choices(&self) -> Vec<u32> {
        if self.buffer_range {
            let (min, max) = (self.buffer_sizes[0], self.buffer_sizes[1]);
            let mut sizes: Vec<u32> = (4..=13)
                .map(|shift| 1u32 << shift)
                .filter(|size| (min..=max).contains(size))
                .collect();
            if sizes.is_empty() {
                sizes.push(min);
            }
            sizes
        } else if self.buffer_sizes.is_empty() {
            vec![64, 128, 256, 512, 1024, 2048]
        } else {
            self.buffer_sizes.clone()
        }
    }
}

/// What `audio.devices` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioDevices {
    /// The driver types on offer, best first.
    pub drivers: Vec<DriverInfo>,
    /// The driver whose devices are listed: the one in use.
    pub driver: String,
    /// Its devices.
    pub devices: Vec<DeviceListing>,
    /// Why the devices couldn't be listed, if they couldn't.
    pub error: Option<String>,
}

/// Opens a backend of a kind. The app uses `bl_audio_io::open_backend`;
/// tests pass their own.
pub type BackendFactory = Box<dyn Fn(BackendKind) -> Result<Box<dyn Backend>, Error>>;

/// Counters the audio thread updates and the CPU meter reads: relaxed
/// atomics only, so the audio thread never waits.
#[derive(Debug, Default)]
struct EngineShared {
    /// Nanoseconds the callback spent processing.
    busy_ns: AtomicU64,
    /// Nanoseconds of audio the callback produced.
    audio_ns: AtomicU64,
    /// Whether the test tone plays.
    tone: AtomicBool,
}

/// The stand-in engine on the audio thread: silence, or the test tone at
/// [`TEST_TONE_DBFS`], timed for the CPU meter. It allocates nothing, takes
/// no lock, and makes no system call besides reading the clock.
pub struct EngineCallback {
    shared: Arc<EngineShared>,
    phase: f64,
    amplitude: f32,
}

impl EngineCallback {
    fn new(shared: Arc<EngineShared>) -> Self {
        Self {
            shared,
            phase: 0.0,
            amplitude: test_tone_amplitude(),
        }
    }
}

impl AudioCallback for EngineCallback {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let started = Instant::now();
        if self.shared.tone.load(Relaxed) {
            let step = TEST_TONE_HZ / f64::from(ctx.sample_rate.max(1));
            let mut channels = output.iter_mut();
            if let Some(first) = channels.next() {
                for sample in first.iter_mut() {
                    *sample = (self.phase * TAU).sin() as f32 * self.amplitude;
                    self.phase += step;
                    if self.phase >= 1.0 {
                        self.phase -= 1.0;
                    }
                }
                for channel in channels {
                    channel.copy_from_slice(first);
                }
            }
        }
        let busy = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let audio = ctx.frames as u64 * 1_000_000_000 / u64::from(ctx.sample_rate.max(1));
        self.shared.busy_ns.fetch_add(busy, Relaxed);
        self.shared.audio_ns.fetch_add(audio, Relaxed);
    }
}

/// The audio device the app plays through, and the settings that choose it.
pub struct AudioHost {
    available: Vec<BackendKind>,
    factory: BackendFactory,
    settings: AudioSettings,
    engine_on: bool,
    backend: Option<(BackendKind, Box<dyn Backend>)>,
    stream: Option<Box<dyn Stream>>,
    shared: Arc<EngineShared>,
    error: Option<String>,
    /// The counters at the last CPU sample, and the load measured then.
    last_sample: (u64, u64),
    cpu_load: Option<f32>,
}

impl std::fmt::Debug for AudioHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioHost")
            .field("settings", &self.settings)
            .field("engine_on", &self.engine_on)
            .field("open", &self.stream.is_some())
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl AudioHost {
    /// A host on this machine's backends, with these settings. Nothing
    /// opens until [`AudioHost::start`].
    #[must_use]
    pub fn new(settings: AudioSettings) -> Self {
        Self::with_backends(
            bl_audio_io::available_backends(),
            Box::new(bl_audio_io::open_backend),
            settings,
        )
    }

    /// A host on the null device only, for tests and headless runs: it runs
    /// the whole stream path on a timer, without hardware.
    #[must_use]
    pub fn null(settings: AudioSettings) -> Self {
        let settings = AudioSettings {
            driver: Some(BackendKind::Null.id().to_owned()),
            ..settings
        };
        Self::with_backends(
            vec![BackendKind::Null],
            Box::new(bl_audio_io::open_backend),
            settings,
        )
    }

    /// A host on these backends, opened by `factory`.
    #[must_use]
    pub fn with_backends(
        available: Vec<BackendKind>,
        factory: BackendFactory,
        settings: AudioSettings,
    ) -> Self {
        Self {
            available,
            factory,
            settings,
            engine_on: true,
            backend: None,
            stream: None,
            shared: Arc::new(EngineShared::default()),
            error: None,
            last_sample: (0, 0),
            cpu_load: None,
        }
    }

    /// The settings in force.
    #[must_use]
    pub fn settings(&self) -> &AudioSettings {
        &self.settings
    }

    /// Whether the audio engine is on.
    #[must_use]
    pub fn engine_on(&self) -> bool {
        self.engine_on
    }

    /// Whether a stream is open and running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.stream.is_some()
    }

    /// The driver types on offer, best first.
    #[must_use]
    pub fn drivers(&self) -> &[BackendKind] {
        &self.available
    }

    /// The driver type the settings choose: the named one if this machine
    /// has it, otherwise the platform's default, otherwise the first offered.
    #[must_use]
    pub fn driver(&self) -> BackendKind {
        self.settings
            .driver
            .as_deref()
            .and_then(BackendKind::from_id)
            .filter(|kind| self.available.contains(kind))
            .or_else(|| {
                let default = bl_audio_io::default_backend();
                self.available.contains(&default).then_some(default)
            })
            .or_else(|| self.available.first().copied())
            .unwrap_or(BackendKind::Null)
    }

    /// Opens the device if the engine is on. A failure is kept for
    /// [`AudioHost::status`] and returned.
    ///
    /// # Errors
    ///
    /// The device's error, as text.
    pub fn start(&mut self) -> Result<(), String> {
        if !self.engine_on || self.stream.is_some() {
            return Ok(());
        }
        let result = self.open();
        self.error = result.as_ref().err().cloned();
        result
    }

    /// Closes the device.
    pub fn stop(&mut self) {
        self.stream = None;
        self.cpu_load = None;
    }

    /// Switches the engine on or off: off closes the device, on reopens it
    /// with the current settings.
    ///
    /// # Errors
    ///
    /// The device's error, if it couldn't reopen. The engine is on anyway,
    /// and [`AudioHost::status`] reports the error.
    pub fn set_engine(&mut self, on: bool) -> Result<(), String> {
        self.engine_on = on;
        if on {
            self.start()
        } else {
            self.stop();
            self.error = None;
            Ok(())
        }
    }

    /// Plays or stops the test tone.
    pub fn set_test_tone(&self, on: bool) {
        self.shared.tone.store(on, Relaxed);
    }

    /// Whether the test tone plays.
    #[must_use]
    pub fn test_tone(&self) -> bool {
        self.shared.tone.load(Relaxed)
    }

    /// Applies new settings straight away: closes the stream and opens one
    /// with the new settings. While the engine is off, the settings are
    /// only stored.
    ///
    /// # Errors
    ///
    /// A message if a value is malformed or the device refuses the new
    /// settings. The previous settings are then reopened, so sound carries
    /// on as before.
    pub fn configure(&mut self, change: &ConfigureAudio) -> Result<(), String> {
        let next = self.merged(change)?;
        if !self.engine_on {
            self.settings = next;
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.settings, next);
        self.stream = None;
        match self.open() {
            Ok(()) => {
                self.error = None;
                Ok(())
            }
            Err(message) => {
                self.settings = previous;
                self.error = self.open().err();
                Err(message)
            }
        }
    }

    /// The settings `change` leads to, checked.
    fn merged(&self, change: &ConfigureAudio) -> Result<AudioSettings, String> {
        let mut next = self.settings.clone();
        if let Some(driver) = &change.driver {
            let kind = BackendKind::from_id(driver).ok_or_else(|| {
                format!("`{driver}` isn't a driver type; `audio.devices` lists them")
            })?;
            if !self.available.contains(&kind) {
                return Err(format!(
                    "the {kind} driver isn't available on this computer"
                ));
            }
            if Some(kind) != next.driver.as_deref().and_then(BackendKind::from_id) {
                next.output_device = None;
                next.input_device = None;
                next.sample_rate = None;
                next.buffer_size = None;
                next.output_channels = None;
            }
            next.driver = Some(kind.id().to_owned());
        }
        if let Some(device) = &change.output_device {
            next.output_device = if device == "default" {
                None
            } else {
                Some(parse_device(device)?)
            };
        }
        if let Some(device) = &change.input_device {
            next.input_device = if device == "none" {
                None
            } else {
                Some(parse_device(device)?)
            };
        }
        if let Some(rate) = change.sample_rate {
            next.sample_rate = (rate != 0).then_some(rate);
        }
        if let Some(frames) = change.buffer_size {
            next.buffer_size = (frames != 0).then_some(frames);
        }
        if let Some(channels) = change.output_channels {
            next.output_channels = (channels != 0).then_some(channels);
        }
        Ok(next)
    }

    fn backend(&mut self) -> Result<&dyn Backend, String> {
        let kind = self.driver();
        if self.backend.as_ref().is_none_or(|(open, _)| *open != kind) {
            self.backend = None;
            let backend = (self.factory)(kind).map_err(|e| e.to_string())?;
            self.backend = Some((kind, backend));
        }
        Ok(self
            .backend
            .as_ref()
            .map(|(_, backend)| backend.as_ref())
            .expect("just opened"))
    }

    fn open(&mut self) -> Result<(), String> {
        self.stream = None;
        let request = StreamRequest {
            output: Some(PortRequest {
                device: selector(self.settings.output_device.as_deref())?,
                channels: self.settings.output_channels,
            }),
            input: match self.settings.input_device.as_deref() {
                Some(id) => Some(PortRequest {
                    device: selector(Some(id))?,
                    channels: None,
                }),
                None => None,
            },
            sample_rate: self.settings.sample_rate,
            buffer_size: self.settings.buffer_size,
        };
        let callback = Box::new(EngineCallback::new(Arc::clone(&self.shared)));
        let mut stream = self
            .backend()?
            .open_stream(&request, callback)
            .map_err(|e| e.to_string())?;
        stream.start().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        self.last_sample = (
            self.shared.busy_ns.load(Relaxed),
            self.shared.audio_ns.load(Relaxed),
        );
        self.cpu_load = Some(0.0);
        Ok(())
    }

    /// Measures the CPU load since the last call: the share of the audio
    /// produced in that time that the callback spent processing, in percent.
    /// The meter calls it a few times a second. `None` while the engine is
    /// off or the device is closed.
    pub fn sample_cpu_load(&mut self) -> Option<f32> {
        if self.stream.is_none() {
            self.cpu_load = None;
            return None;
        }
        let busy = self.shared.busy_ns.load(Relaxed);
        let audio = self.shared.audio_ns.load(Relaxed);
        let (last_busy, last_audio) = self.last_sample;
        if audio > last_audio {
            let load = (busy - last_busy) as f64 / (audio - last_audio) as f64 * 100.0;
            self.cpu_load = Some(load as f32);
            self.last_sample = (busy, audio);
        }
        self.cpu_load
    }

    /// What the device runs at now.
    #[must_use]
    pub fn status(&self) -> AudioStatus {
        let info = self.stream.as_ref().map(|stream| stream.info());
        let stats = self.stream.as_ref().map(|stream| stream.stats());
        let sample_rate = info.map(|info| info.sample_rate);
        let buffer_size = stats
            .and_then(|stats| stats.last_device_frames)
            .or_else(|| info.and_then(|info| info.buffer_size));
        let device = |device: &bl_audio_io::OpenDevice| DeviceRef {
            id: device.id.to_string(),
            name: device.name.clone(),
        };
        AudioStatus {
            engine_on: self.engine_on,
            settings: self.settings.clone(),
            driver: self.driver().id().to_owned(),
            output_device: info.and_then(|info| info.output_device.as_ref().map(device)),
            input_device: info.and_then(|info| info.input_device.as_ref().map(device)),
            follows_default: info.is_some_and(|info| info.follows_default),
            sample_rate,
            requested_buffer_size: self.settings.buffer_size,
            buffer_size,
            output_channels: info.map_or(0, |info| info.output_channels),
            input_channels: info.map_or(0, |info| info.input_channels),
            buffer_ms: buffer_size
                .zip(sample_rate)
                .map(|(frames, rate)| f64::from(frames) * 1000.0 / f64::from(rate.max(1))),
            cpu_load: self.cpu_load,
            dropouts: stats.map_or(0, |stats| stats.dropouts),
            test_tone: self.test_tone(),
            error: self.error.clone(),
        }
    }

    /// The driver types on offer and the devices of the one in use.
    pub fn devices(&mut self) -> AudioDevices {
        let drivers = self
            .available
            .iter()
            .map(|kind| DriverInfo {
                id: kind.id().to_owned(),
                name: kind.label().to_owned(),
            })
            .collect();
        let driver = self.driver().id().to_owned();
        let listed = self
            .backend()
            .and_then(|backend| backend.devices().map_err(|e| e.to_string()));
        let (devices, error) = match listed {
            Ok(devices) => (devices.iter().map(DeviceListing::from_info).collect(), None),
            Err(error) => (Vec::new(), Some(error)),
        };
        AudioDevices {
            drivers,
            driver,
            devices,
            error,
        }
    }
}

fn parse_device(id: &str) -> Result<String, String> {
    id.parse::<DeviceId>()
        .map(|id| id.to_string())
        .map_err(|e| e.message().to_owned())
}

fn selector(id: Option<&str>) -> Result<DeviceSelector, String> {
    match id {
        None => Ok(DeviceSelector::Default),
        Some(id) => id
            .parse::<DeviceId>()
            .map(DeviceSelector::Id)
            .map_err(|e| e.message().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_tone_never_exceeds_minus_60_dbfs() {
        // Spec: app-shell R13.
        let shared = Arc::new(EngineShared::default());
        shared.tone.store(true, Relaxed);
        let mut engine = EngineCallback::new(Arc::clone(&shared));
        let mut data = vec![0.0f32; 2 * 4800];
        let mut output = AudioBufferMut::new(&mut data, 2, 4800, 4800);
        let ctx = ProcessContext {
            sample_rate: 48_000,
            frames: 4800,
            position: 0,
            timestamp: None,
        };
        engine.process(&ctx, &AudioBuffer::empty(4800), &mut output);
        let peak = data.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(peak > 0.0009, "the tone plays: {peak}");
        assert!(peak <= 0.001 + 1e-7, "at most -60 dBFS: {peak}");
        assert_eq!(shared.audio_ns.load(Relaxed), 100_000_000);
    }

    #[test]
    fn silence_is_the_default() {
        let shared = Arc::new(EngineShared::default());
        let mut engine = EngineCallback::new(shared);
        let mut data = vec![0.0f32; 256];
        let mut output = AudioBufferMut::new(&mut data, 1, 256, 256);
        let ctx = ProcessContext {
            sample_rate: 44_100,
            frames: 256,
            position: 0,
            timestamp: None,
        };
        engine.process(&ctx, &AudioBuffer::empty(256), &mut output);
        assert!(data.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn buffer_choices_are_powers_of_two_in_range() {
        let listing = DeviceListing {
            id: "null:null".into(),
            name: "Null".into(),
            output_channels: 2,
            input_channels: 2,
            default_output: true,
            default_input: true,
            sample_rates: vec![48_000],
            current_sample_rate: Some(48_000),
            buffer_sizes: vec![30, 3000],
            buffer_range: true,
            default_buffer_size: Some(256),
        };
        assert_eq!(
            listing.buffer_choices(),
            [32, 64, 128, 256, 512, 1024, 2048]
        );
    }
}
