//! A CLAP plug-in built from source for the plug-in host's tests (WP-2.6).
//! It's never shipped; the tests load its cdylib by path.
//!
//! "Test Gain Echo" (`io.beatlayer.test.gain-echo`) is an audio effect that
//! also takes notes, so one plug-in exercises everything the host does:
//!
//! - **Audio.** One stereo port in and out: each sample is its input times
//!   [`GAIN`], which changes on the exact frame of its event.
//! - **Notes in and out.** A note-on adds its velocity to the output on its
//!   frame (an impulse), and every note-on and note-off goes back out an
//!   octave up ([`ECHO_TRANSPOSE`]) on the same frame.
//! - **Parameters out.** On a note-on it sets its own [`LAST_NOTE`]
//!   parameter to the note's key, as an editor would, inside a gesture.
//! - **Latency.** It reports [`LATENCY`]'s value, as of its last
//!   activation; a change asks the host to restart it. (It doesn't delay
//!   its audio: delay compensation is WP-2.11's.)
//! - **State.** It saves its parameters ([`STATE_MAGIC`] then gain,
//!   latency, and last note, little-endian) and refuses anything else, but
//!   for three states that make it fail on purpose: [`FAIL_PROCESS`]
//!   makes every process call fail, [`FAIL_ACTIVATE`] its next activation,
//!   and [`FAIL_GUI`] its next editor show or hide.
//! - **Editor.** A window that draws nothing, so tests open and close it
//!   headless: floating in any windowing API, and embedded into the host's
//!   window on the platforms that build one (win32 and x11; WP-2.10). What
//!   the host tells it about the window — the display's scale (`set_scale`)
//!   and the size it sets (`set_size`) — it reports back as its
//!   [`EDITOR_SCALE`], [`EDITOR_WIDTH`] and [`EDITOR_HEIGHT`] parameters,
//!   so tests can check the host's window layer reaches the plug-in.

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, PluginGui, PluginGuiImpl, Window,
};
use clack_extensions::latency::{PluginLatency, PluginLatencyImpl};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::Match;
use clack_plugin::events::event_types::{
    NoteOffEvent, NoteOnEvent, ParamGestureBeginEvent, ParamGestureEndEvent, ParamValueEvent,
};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use clack_plugin::utils::Cookie;
use std::io::{Read, Write};

/// The plug-in's CLAP ID.
pub const PLUGIN_ID: &str = "io.beatlayer.test.gain-echo";
/// The plug-in's name.
pub const PLUGIN_NAME: &str = "Test Gain Echo";
/// Gain: a linear factor from 0 to 2, 1 by default.
pub const GAIN: u32 = 1;
/// Latency: the frames the plug-in reports, 0 to 4,096, stepped.
pub const LATENCY: u32 = 2;
/// Last Note: the key of the last note-on, 0 to 127, stepped. The plug-in
/// sets it itself.
pub const LAST_NOTE: u32 = 3;
/// Editor Scale: the display's scale the host gave the editor
/// (`set_scale`), 1.0 at 96 DPI; 0 until the host gives one.
pub const EDITOR_SCALE: u32 = 4;
/// Editor Width: the width the host last set the editor to (`set_size`),
/// in physical pixels; 0 until it sets one.
pub const EDITOR_WIDTH: u32 = 5;
/// Editor Height: the height the host last set the editor to (`set_size`).
pub const EDITOR_HEIGHT: u32 = 6;
/// How far the echoed notes are transposed, in semitones.
pub const ECHO_TRANSPOSE: i16 = 12;
/// What a saved state starts with.
pub const STATE_MAGIC: &[u8; 4] = b"BLTC";
/// A state that makes every process call fail.
pub const FAIL_PROCESS: &[u8] = b"FAIL:PROCESS";
/// A state that makes the next activation fail.
pub const FAIL_ACTIVATE: &[u8] = b"FAIL:ACTIVATE";
/// A state that makes the next editor show or hide fail.
pub const FAIL_GUI: &[u8] = b"FAIL:GUI";

/// The environment variable a test sets so a scan crashes (`crash`) or hangs
/// (`hang`) on purpose. Only the scanner's tests set it.
pub const TEST_SCAN_VAR: &str = "BL_TEST_CLAP_SCAN";

const NO_FAILURE: u8 = 0;
const PROCESS_FAILS: u8 = 1;
const ACTIVATE_FAILS: u8 = 2;
const GUI_FAILS: u8 = 3;

/// The plug-in.
pub struct GainEcho;

/// What the main thread and the audio thread share: the parameters, and
/// what the host told the editor (WP-2.10).
pub struct Shared<'a> {
    host: HostSharedHandle<'a>,
    gain: AtomicU64,
    latency: AtomicU32,
    active_latency: AtomicU32,
    last_note: AtomicU64,
    /// The display's scale the host gave the editor, as bits.
    editor_scale: AtomicU64,
    editor_width: AtomicU32,
    editor_height: AtomicU32,
    failure: AtomicU8,
}

impl Shared<'_> {
    fn gain(&self) -> f64 {
        f64::from_bits(self.gain.load(Relaxed))
    }

    fn set(&self, id: u32, value: f64) {
        match id {
            GAIN => self.gain.store(value.clamp(0.0, 2.0).to_bits(), Relaxed),
            LATENCY => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to 0..=4096 and rounded first"
                )]
                let frames = value.clamp(0.0, 4096.0).round() as u32;
                if self.latency.swap(frames, Relaxed) != frames {
                    self.host.request_restart();
                }
            }
            LAST_NOTE => self
                .last_note
                .store(value.clamp(0.0, 127.0).round().to_bits(), Relaxed),
            EDITOR_SCALE => self.editor_scale.store(value.to_bits(), Relaxed),
            EDITOR_WIDTH => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to 0..=8192 and rounded first"
                )]
                let width = value.clamp(0.0, 8192.0).round() as u32;
                self.editor_width.store(width, Relaxed);
            }
            EDITOR_HEIGHT => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to 0..=8192 and rounded first"
                )]
                let height = value.clamp(0.0, 8192.0).round() as u32;
                self.editor_height.store(height, Relaxed);
            }
            _ => {}
        }
    }

    fn get(&self, id: u32) -> Option<f64> {
        match id {
            GAIN => Some(self.gain()),
            LATENCY => Some(f64::from(self.latency.load(Relaxed))),
            LAST_NOTE => Some(f64::from_bits(self.last_note.load(Relaxed))),
            EDITOR_SCALE => Some(f64::from_bits(self.editor_scale.load(Relaxed))),
            EDITOR_WIDTH => Some(f64::from(self.editor_width.load(Relaxed))),
            EDITOR_HEIGHT => Some(f64::from(self.editor_height.load(Relaxed))),
            _ => None,
        }
    }

    fn apply(&self, events: &InputEvents<'_>) {
        for event in events {
            if let Some(param) = event.as_event::<ParamValueEvent>()
                && let Some(id) = param.param_id()
            {
                self.set(id.get(), param.value());
            }
        }
    }
}

impl<'a> PluginShared<'a> for Shared<'a> {}

/// The main thread's side.
pub struct MainThread<'a> {
    shared: &'a Shared<'a>,
}

impl<'a> PluginMainThread<'a, Shared<'a>> for MainThread<'a> {}

impl Plugin for GainEcho {
    type AudioProcessor<'a> = Processor<'a>;
    type Shared<'a> = Shared<'a>;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<'_, Self>, _shared: Option<&Shared<'_>>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>()
            .register::<PluginLatency>()
            .register::<PluginGui>();
    }
}

impl DefaultPluginFactory for GainEcho {
    fn get_descriptor() -> PluginDescriptor {
        // Spec: plugin-scanning R5. A scan can be made to crash or hang on
        // purpose, so the scanner's child process isolation and timeout are
        // tested with a real plug-in: the test sets `BL_TEST_CLAP_SCAN`.
        // Only the tests ever set it, and the test plug-in is never shipped.
        match std::env::var(TEST_SCAN_VAR).as_deref() {
            Ok("crash") => std::process::abort(),
            Ok("hang") => loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            },
            _ => {}
        }
        use clack_plugin::plugin::features::{AUDIO_EFFECT, STEREO};
        PluginDescriptor::new(PLUGIN_ID, PLUGIN_NAME)
            .with_vendor("Beatlayer tests")
            .with_version("1.0.0")
            .with_features([AUDIO_EFFECT, STEREO])
    }

    fn new_shared(host: HostSharedHandle<'_>) -> Result<Shared<'_>, PluginError> {
        Ok(Shared {
            host,
            gain: AtomicU64::new(1.0f64.to_bits()),
            latency: AtomicU32::new(0),
            active_latency: AtomicU32::new(0),
            last_note: AtomicU64::new(0.0f64.to_bits()),
            editor_scale: AtomicU64::new(0.0f64.to_bits()),
            editor_width: AtomicU32::new(0),
            editor_height: AtomicU32::new(0),
            failure: AtomicU8::new(NO_FAILURE),
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a Shared<'a>,
    ) -> Result<MainThread<'a>, PluginError> {
        Ok(MainThread { shared })
    }
}

/// The audio thread's side.
pub struct Processor<'a> {
    shared: &'a Shared<'a>,
}

impl<'a> PluginAudioProcessor<'a, Shared<'a>, MainThread<'a>> for Processor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &MainThread<'a>,
        shared: &'a Shared<'a>,
        _config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        if shared
            .failure
            .compare_exchange(ACTIVATE_FAILS, NO_FAILURE, Relaxed, Relaxed)
            .is_ok()
        {
            return Err(PluginError::Message("activation fails on purpose"));
        }
        shared
            .active_latency
            .store(shared.latency.load(Relaxed), Relaxed);
        Ok(Self { shared })
    }

    fn process(
        &mut self,
        _process: Process<'_>,
        mut audio: Audio<'_>,
        events: Events<'_>,
    ) -> Result<ProcessStatus, PluginError> {
        if self.shared.failure.load(Relaxed) == PROCESS_FAILS {
            return Err(PluginError::Message("processing fails on purpose"));
        }
        let frames = audio.frames_count() as usize;
        let mut port = audio
            .port_pair(0)
            .ok_or(PluginError::Message("no audio port"))?;
        let mut channels = port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("not 32-bit audio"))?;
        // Render each stretch between events at its gain, then apply the
        // changes on their frame.
        let mut start = 0usize;
        let mut batches = events.input.batch();
        loop {
            let (end, batch) = match batches.next() {
                Some(batch) => (batch.first_sample().min(frames), Some(batch.events())),
                None => (frames, None),
            };
            let gain = self.shared.gain() as f32;
            for mut pair in channels.iter_mut() {
                match &mut pair {
                    ChannelPair::InputOutput(input, output) => {
                        for (o, i) in output[start..end].iter_mut().zip(&input[start..end]) {
                            *o = i * gain;
                        }
                    }
                    ChannelPair::InPlace(buffer) => {
                        for s in &mut buffer[start..end] {
                            *s *= gain;
                        }
                    }
                    ChannelPair::OutputOnly(output) => output[start..end].fill(0.0),
                    ChannelPair::InputOnly(_) => {}
                }
            }
            start = end;
            let Some(batch) = batch else { break };
            for event in batch {
                if let Some(CoreEventSpace::ParamValue(param)) = event.as_core_event()
                    && let Some(id) = param.param_id()
                {
                    self.shared.set(id.get(), param.value());
                }
            }
        }
        // Then the notes, over the audio: an impulse on each note-on's
        // frame, and each note echoed an octave up on its frame.
        for event in events.input {
            match event.as_core_event() {
                Some(CoreEventSpace::NoteOn(note)) => {
                    let at = event.header().time();
                    // The impulse lands on the note's own frame.
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a velocity is from 0 to 1"
                    )]
                    let velocity = note.velocity() as f32;
                    if (at as usize) < frames {
                        for mut pair in channels.iter_mut() {
                            if let Some(out) = pair.output_mut() {
                                out[at as usize] += velocity;
                            }
                        }
                    }
                    let key = note.pckn().key;
                    let echo = NoteOnEvent::new(at, transposed(note.pckn()), note.velocity());
                    let _ = events.output.try_push(echo);
                    if let Some(key) = key.into_specific() {
                        let id = ClapId::new(LAST_NOTE);
                        self.shared.set(LAST_NOTE, f64::from(key));
                        let _ = events.output.try_push(ParamGestureBeginEvent::new(at, id));
                        let _ = events.output.try_push(ParamValueEvent::new(
                            at,
                            id,
                            Pckn::match_all(),
                            f64::from(key),
                        ));
                        let _ = events.output.try_push(ParamGestureEndEvent::new(at, id));
                    }
                }
                Some(CoreEventSpace::NoteOff(note)) => {
                    let at = event.header().time();
                    let echo = NoteOffEvent::new(at, transposed(note.pckn()), note.velocity());
                    let _ = events.output.try_push(echo);
                }
                _ => {}
            }
        }
        Ok(ProcessStatus::ContinueIfNotQuiet)
    }
}

/// A note an octave up.
fn transposed(pckn: Pckn) -> Pckn {
    let key = pckn.key.into_specific().map_or(Match::All, |key| {
        Match::Specific((key + ECHO_TRANSPOSE as u16).min(127))
    });
    Pckn { key, ..pckn }
}

impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }

    fn get(&self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter<'_>) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"Main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for MainThread<'_> {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }

    fn get(&self, index: u32, _is_input: bool, writer: &mut NotePortInfoWriter<'_>) {
        if index == 0 {
            writer.set(&NotePortInfo {
                id: ClapId::new(0),
                name: b"Notes",
                supported_dialects: NoteDialects::CLAP,
                preferred_dialect: Some(NoteDialect::Clap),
            });
        }
    }
}

impl PluginMainThreadParams for MainThread<'_> {
    fn count(&self) -> u32 {
        6
    }

    fn get_info(&self, index: u32, info: &mut ParamInfoWriter<'_>) {
        let automatable = ParamInfoFlags::IS_AUTOMATABLE;
        let (id, name, max, default, flags): (u32, &[u8], f64, f64, ParamInfoFlags) = match index {
            0 => (GAIN, b"Gain", 2.0, 1.0, automatable),
            1 => (
                LATENCY,
                b"Latency",
                4096.0,
                0.0,
                automatable | ParamInfoFlags::IS_STEPPED,
            ),
            2 => (
                LAST_NOTE,
                b"Last Note",
                127.0,
                0.0,
                automatable | ParamInfoFlags::IS_STEPPED,
            ),
            3 => (EDITOR_SCALE, b"Editor Scale", 8.0, 0.0, automatable),
            4 => (
                EDITOR_WIDTH,
                b"Editor Width",
                8192.0,
                0.0,
                automatable | ParamInfoFlags::IS_STEPPED,
            ),
            5 => (
                EDITOR_HEIGHT,
                b"Editor Height",
                8192.0,
                0.0,
                automatable | ParamInfoFlags::IS_STEPPED,
            ),
            _ => return,
        };
        info.set(&ParamInfo {
            id: ClapId::new(id),
            flags,
            cookie: Cookie::empty(),
            name,
            module: b"",
            min_value: 0.0,
            max_value: max,
            default_value: default,
        });
    }

    fn get_value(&self, id: ClapId) -> Option<f64> {
        self.shared.get(id.get())
    }

    fn value_to_text(
        &self,
        id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter<'_>,
    ) -> std::fmt::Result {
        use std::fmt::Write as _;
        match id.get() {
            GAIN => write!(writer, "{value:.2}"),
            _ => write!(writer, "{value:.0}"),
        }
    }

    fn text_to_value(&self, _id: ClapId, text: &CStr) -> Option<f64> {
        text.to_str().ok()?.trim().parse().ok()
    }

    fn flush(&self, input: &InputEvents<'_>, _output: &mut OutputEvents<'_>) {
        self.shared.apply(input);
    }
}

impl PluginAudioProcessorParams for Processor<'_> {
    fn flush(&mut self, input: &InputEvents<'_>, _output: &mut OutputEvents<'_>) {
        self.shared.apply(input);
    }
}

impl PluginStateImpl for MainThread<'_> {
    fn save(&self, output: &mut OutputStream<'_>) -> Result<(), PluginError> {
        let shared = self.shared;
        let mut bytes = Vec::with_capacity(24);
        bytes.extend_from_slice(STATE_MAGIC);
        bytes.extend_from_slice(&shared.gain().to_le_bytes());
        bytes.extend_from_slice(&shared.latency.load(Relaxed).to_le_bytes());
        bytes.extend_from_slice(&shared.get(LAST_NOTE).unwrap_or(0.0).to_le_bytes());
        output.write_all(&bytes)?;
        Ok(())
    }

    fn load(&self, input: &mut InputStream<'_>) -> Result<(), PluginError> {
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        let shared = self.shared;
        if bytes == FAIL_PROCESS {
            shared.failure.store(PROCESS_FAILS, Relaxed);
            return Ok(());
        }
        if bytes == FAIL_ACTIVATE {
            shared.failure.store(ACTIVATE_FAILS, Relaxed);
            return Ok(());
        }
        if bytes == FAIL_GUI {
            shared.failure.store(GUI_FAILS, Relaxed);
            return Ok(());
        }
        let field = |range: core::ops::Range<usize>| bytes.get(range);
        let (Some(magic), Some(gain), Some(latency), Some(last), 24) = (
            field(0..4),
            field(4..12),
            field(12..16),
            field(16..24),
            bytes.len(),
        ) else {
            return Err(PluginError::Message("not a Test Gain Echo state"));
        };
        if magic != STATE_MAGIC {
            return Err(PluginError::Message("not a Test Gain Echo state"));
        }
        let f64_at = |b: &[u8]| f64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
        shared.set(GAIN, f64_at(gain));
        shared.set(
            LATENCY,
            f64::from(u32::from_le_bytes(latency.try_into().unwrap_or([0; 4]))),
        );
        shared.set(LAST_NOTE, f64_at(last));
        shared.failure.store(NO_FAILURE, Relaxed);
        Ok(())
    }
}

impl PluginLatencyImpl for MainThread<'_> {
    fn get(&self) -> u32 {
        self.shared.active_latency.load(Relaxed)
    }
}

/// The windowing API an embedded editor uses on this platform: `win32` on
/// Windows and `x11` on the other X11 platforms; macOS editors only float
/// for now, so it refuses to embed there (WP-2.10).
fn embed_api() -> Option<GuiApiType<'static>> {
    #[cfg(target_os = "windows")]
    {
        Some(GuiApiType::WIN32)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Some(GuiApiType::X11)
    }
    #[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos")))))]
    {
        None
    }
}

impl PluginGuiImpl for MainThread<'_> {
    fn is_api_supported(&self, configuration: GuiConfiguration<'_>) -> bool {
        configuration.is_floating || embed_api().is_some_and(|api| api == configuration.api_type)
    }

    fn get_preferred_api(&self) -> Option<GuiConfiguration<'_>> {
        None
    }

    fn create(&self, configuration: GuiConfiguration<'_>) -> Result<(), PluginError> {
        if configuration.is_floating || embed_api().is_some_and(|api| api == configuration.api_type)
        {
            Ok(())
        } else {
            Err(PluginError::Message("this editor doesn't embed that way"))
        }
    }

    fn destroy(&self) {}

    fn set_scale(&self, scale: f64) -> Result<(), PluginError> {
        self.shared.editor_scale.store(scale.to_bits(), Relaxed);
        Ok(())
    }

    fn get_size(&self) -> Option<GuiSize> {
        Some(GuiSize {
            width: 240,
            height: 120,
        })
    }

    fn can_resize(&self) -> bool {
        true
    }

    fn adjust_size(&self, size: GuiSize) -> Option<GuiSize> {
        Some(size)
    }

    fn set_size(&self, size: GuiSize) -> Result<(), PluginError> {
        self.shared
            .editor_width
            .store(size.width.min(8192), Relaxed);
        self.shared
            .editor_height
            .store(size.height.min(8192), Relaxed);
        Ok(())
    }

    fn set_parent(&self, window: Window<'_, '_>) -> Result<(), PluginError> {
        if window.api_type().to_standard_api() == embed_api() {
            Ok(())
        } else {
            Err(PluginError::Message("this editor doesn't embed that way"))
        }
    }

    fn set_transient(&self, _window: Window<'_, '_>) -> Result<(), PluginError> {
        Ok(())
    }

    fn show(&self) -> Result<(), PluginError> {
        if self
            .shared
            .failure
            .compare_exchange(GUI_FAILS, NO_FAILURE, Relaxed, Relaxed)
            .is_ok()
        {
            return Err(PluginError::Message("showing the editor fails on purpose"));
        }
        Ok(())
    }

    fn hide(&self) -> Result<(), PluginError> {
        if self
            .shared
            .failure
            .compare_exchange(GUI_FAILS, NO_FAILURE, Relaxed, Relaxed)
            .is_ok()
        {
            return Err(PluginError::Message("hiding the editor fails on purpose"));
        }
        Ok(())
    }
}

/// For the host's tests: the test plug-in as a CLAP bundle, made once per
/// test run from the cdylib Cargo built beside the running test binary: on macOS a `.clap` bundle with its
/// `Info.plist`, elsewhere the library renamed `.clap`, as the CLAP
/// specification lays them out.
#[must_use]
pub fn bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let library = library();
        let dir = std::env::temp_dir().join(format!("beatlayer-test-clap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        let bundle = dir.join("Test Gain Echo.clap");
        if cfg!(target_os = "macos") {
            let macos = bundle.join("Contents").join("MacOS");
            std::fs::create_dir_all(&macos).expect("the bundle's folders");
            std::fs::copy(&library, macos.join("Test Gain Echo")).expect("the library");
            std::fs::write(
                bundle.join("Contents").join("Info.plist"),
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Test Gain Echo</string>
<key>CFBundleIdentifier</key><string>io.beatlayer.test.gain-echo</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>
"#,
            )
            .expect("the bundle's Info.plist");
        } else {
            std::fs::copy(&library, &bundle).expect("the library");
        }
        bundle
    })
}

/// The newest test plug-in library Cargo built beside the running test
/// binary.
fn library() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary's path");
    let deps = exe.parent().expect("the test binary's folder");
    let prefix = format!("{}bl_test_clap", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = [deps, deps.parent().unwrap_or(deps)]
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(suffix))
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    found.sort();
    found
        .pop()
        .map(|(_, path)| path)
        .expect("Cargo builds the test plug-in's cdylib beside the tests")
}

clack_plugin::clack_export_entry!(SinglePluginEntry<GainEcho>);
