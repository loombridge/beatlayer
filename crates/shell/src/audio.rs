//! The audio host: the device the app plays through, the settings that
//! choose it, the test tone, and the CPU meter (L-SET-012, L-CTRL-023,
//! L-CTRL-024).
//!
//! [`AudioHost`] owns one stream from `bl-audio-io`. Changing a setting
//! ([`AudioHost::configure`]) closes the stream and opens a new one with the
//! new settings straight away, so a device, rate, buffer, or channel change
//! applies without restarting the app. If the device refuses, the previous
//! settings are reopened and the error is reported.
//!
//! The stream's callback ([`EngineCallback`]) is a stand-in until the app
//! runs the engine: it plays silence, or the Settings window's test tone at
//! the Tone Volume and Tone Frequency the person set (D23), and measures how
//! long it takes, which the CPU meter shows. It runs on the audio thread, so
//! it follows the real-time rules: no allocation, no lock, no blocking call
//! (`tests/realtime.rs` checks it under the allocation trap).
//!
//! The settings and reports themselves are data in `bl-shell-spec`,
//! re-exported here.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, BackendKind, BufferSizes, DeviceId,
    DeviceInfo, DeviceSelector, Error, PortRequest, ProcessContext, Stream, StreamRequest,
    StreamStats,
};
pub use bl_shell_spec::audio::{
    AudioDevices, AudioSettings, AudioStatus, ConfigureAudio, DeviceListing, DeviceRef, DriverInfo,
    TONE_DEFAULT_DBFS, TONE_DEFAULT_HZ, TONE_MAX_DBFS, TONE_MAX_HZ, TONE_MIN_DBFS, TONE_MIN_HZ,
    ToneSettings,
};

/// A device as `audio.devices` lists it.
fn listing(info: &DeviceInfo) -> DeviceListing {
    let (buffer_sizes, buffer_range) = match &info.buffer_sizes {
        BufferSizes::Range { min, max } => (vec![*min, *max], true),
        BufferSizes::List(sizes) => (sizes.clone(), false),
        BufferSizes::Unknown => (Vec::new(), false),
    };
    DeviceListing {
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

/// Opens a backend of a kind. The app uses `bl_audio_io::open_backend`;
/// tests pass their own.
pub type BackendFactory = Box<dyn Fn(BackendKind) -> Result<Box<dyn Backend>, Error>>;

/// What the audio thread and the rest of the app share: relaxed atomics
/// only, so the audio thread never waits.
#[derive(Debug)]
pub struct EngineShared {
    /// Nanoseconds the callback spent processing.
    busy_ns: AtomicU64,
    /// Nanoseconds of audio the callback produced.
    audio_ns: AtomicU64,
    /// Whether the test tone plays.
    tone: AtomicBool,
    /// The test tone's peak amplitude, as `f32` bits.
    tone_amplitude: AtomicU32,
    /// The test tone's frequency in hertz, as `f32` bits.
    tone_hz: AtomicU32,
}

impl Default for EngineShared {
    fn default() -> Self {
        let shared = Self {
            busy_ns: AtomicU64::new(0),
            audio_ns: AtomicU64::new(0),
            tone: AtomicBool::new(false),
            tone_amplitude: AtomicU32::new(0),
            tone_hz: AtomicU32::new(0),
        };
        shared.set_tone(ToneSettings::default());
        shared
    }
}

impl EngineShared {
    /// Switches the test tone on or off.
    pub fn set_tone_on(&self, on: bool) {
        self.tone.store(on, Relaxed);
    }

    /// Sets the test tone's level and frequency, within the controls'
    /// ranges.
    pub fn set_tone(&self, tone: ToneSettings) {
        let tone = tone.clamped();
        self.tone_amplitude
            .store(tone.amplitude().to_bits(), Relaxed);
        self.tone_hz
            .store((tone.frequency_hz as f32).to_bits(), Relaxed);
    }

    /// Nanoseconds of audio the callback has produced.
    #[must_use]
    pub fn audio_ns(&self) -> u64 {
        self.audio_ns.load(Relaxed)
    }
}

/// The stand-in engine on the audio thread: silence, or the test tone,
/// timed for the CPU meter. It allocates nothing, takes no lock, and makes
/// no system call besides reading the clock.
pub struct EngineCallback {
    shared: Arc<EngineShared>,
    phase: f64,
}

impl EngineCallback {
    /// A callback reading the tone settings from `shared` and adding its
    /// timings there.
    #[must_use]
    pub fn new(shared: Arc<EngineShared>) -> Self {
        Self { shared, phase: 0.0 }
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
            let amplitude = f32::from_bits(self.shared.tone_amplitude.load(Relaxed));
            let hz = f64::from(f32::from_bits(self.shared.tone_hz.load(Relaxed)));
            let step = hz / f64::from(ctx.sample_rate.max(1));
            let mut channels = output.iter_mut();
            if let Some(first) = channels.next() {
                for sample in first.iter_mut() {
                    *sample = (self.phase * TAU).sin() as f32 * amplitude;
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
    tone: ToneSettings,
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
            tone: ToneSettings::default(),
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
        self.shared.set_tone_on(on);
    }

    /// Whether the test tone plays.
    #[must_use]
    pub fn test_tone(&self) -> bool {
        self.shared.tone.load(Relaxed)
    }

    /// The test tone's level and frequency.
    #[must_use]
    pub fn tone(&self) -> ToneSettings {
        self.tone
    }

    /// Sets the test tone's level and frequency, brought within the
    /// controls' ranges; a playing tone changes at the next block.
    pub fn set_tone(&mut self, tone: ToneSettings) {
        self.tone = tone.clamped();
        self.shared.set_tone(self.tone);
    }

    /// The running stream's statistics: dropouts, buffer sizes, and the
    /// heap operations the allocation trap counted inside audio blocks.
    /// `None` while no stream is open.
    #[must_use]
    pub fn stats(&self) -> Option<StreamStats> {
        self.stream.as_ref().map(|stream| stream.stats())
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
            tone: self.tone,
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
            Ok(devices) => (devices.iter().map(listing).collect(), None),
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

    fn run_block(engine: &mut EngineCallback, data: &mut [f32], channels: usize, rate: u32) {
        let frames = data.len() / channels;
        let mut output = AudioBufferMut::new(data, channels, frames, frames);
        let ctx = ProcessContext {
            sample_rate: rate,
            frames,
            position: 0,
            timestamp: None,
        };
        engine.process(&ctx, &AudioBuffer::empty(frames), &mut output);
    }

    fn peak(data: &[f32]) -> f32 {
        data.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
    }

    #[test]
    fn the_test_tone_plays_at_its_volume_and_frequency() {
        // Spec: app-shell R13. Only the samples are checked; nothing plays.
        let shared = Arc::new(EngineShared::default());
        shared.set_tone_on(true);
        let mut engine = EngineCallback::new(Arc::clone(&shared));
        let mut data = vec![0.0f32; 2 * 4800];
        run_block(&mut engine, &mut data, 2, 48_000);
        let default = peak(&data);
        let expected = 10f32.powf(-36.0 / 20.0);
        assert!((default - expected).abs() < 1e-4, "-36 dBFS: {default}");
        assert_eq!(shared.audio_ns(), 100_000_000);

        shared.set_tone(ToneSettings {
            volume_db: -60.0,
            frequency_hz: 1000.0,
        });
        let mut data = vec![0.0f32; 4800];
        run_block(&mut engine, &mut data, 1, 48_000);
        let quiet = peak(&data);
        assert!(quiet > 0.0009 && quiet <= 0.001 + 1e-7, "-60 dBFS: {quiet}");
        // 1 kHz at 48 kHz: 48 samples a cycle, so 100 cycles and about as
        // many upward zero crossings in 4800 samples.
        let crossings = data
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!((99..=100).contains(&crossings), "{crossings}");

        shared.set_tone(ToneSettings {
            volume_db: 20.0,
            frequency_hz: 440.0,
        });
        run_block(&mut engine, &mut data, 1, 48_000);
        assert!(peak(&data) <= 1.0, "never past full scale");
    }

    #[test]
    fn silence_is_the_default() {
        let shared = Arc::new(EngineShared::default());
        let mut engine = EngineCallback::new(shared);
        let mut data = vec![0.0f32; 256];
        run_block(&mut engine, &mut data, 1, 44_100);
        assert!(data.iter().all(|s| *s == 0.0));
    }
}
