//! The shell's state and the one place every command runs: [`Shell`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_commands::{Context, NoArgs, Registry};
use bl_core::id::Id;
use bl_core::time::{SampleRate, Ticks};
use bl_engine::session as engine_session;
use bl_engine::transport as engine_transport;
use bl_media::MediaStore;
use bl_model::{
    AudioData, ChangeSet, Clip, ClipData, ContentHash, Editor, Input, LaunchSettings, TrackId,
    TrackKind, registry as model,
};
use bl_project::ProjectFolder;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use bl_assistant::keychain::{Keychain, MemoryKeychain, OsKeychain};

use crate::agent::{
    self as agent, AgentServerState, AgentServerStatus, AgentSettings, AutonomousArgs, ClientArgs,
    SetServerArgs,
};
use crate::arrangement::editing::{self, Clipboard, EditFailure};
use crate::arrangement::{ArrangementView, TransportView, ViewError, Viewport};
use crate::assistant::{
    self as assistant_ids, ActivityEntry, AssistantSettings, AssistantState, Outcome,
    SetBaseUrlArgs, SetKeyArgs, SetModelArgs, SetProviderArgs, keychain_account,
};
use crate::audio::{AudioHost, ConfigureAudio, EngineTransport, ToneSettings};
use crate::browser::{self, BrowserError, BrowserView, Waker};
use crate::clip_view::{self, ClipView};
use crate::commands::{
    self as ids, DefaultSetArgs, FileReport, FoldNotesArgs, LaneArgs, PathArgs, RelinkArgs,
    SaveAsArgs, SetResult, SettingsArgs, SwitchArgs, SwitchResult, TemplateArgs, TestToneArgs,
    TestToneState, ThemeArgs, ViewResult, ZoomArgs,
};
use crate::devices::{self, DeviceError, DeviceView};
use crate::files::{ProjectFiles, SetStore, new_set_from};
use crate::mixer_controls::{self, Switch};
use crate::mixer_view::{MixerOf, MixerView, MixerViewError};
use crate::permission::CallPermission;
use crate::session_view::{SessionView, SessionViewError};
use crate::settings::{
    OnByDefault, Preferences, SettingsPage, ThemeChoice, ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT,
    zoom_factor, zoom_percent, zoom_step,
};
use bl_devices::KitHost;
use bl_plugin_host::au::AuHost;
use bl_plugin_host::clap::ClapHost;
use bl_plugin_host::scan::Scanner;
use bl_plugin_host::vst3::Vst3Host;
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::browser as browser_ids;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::editing as edit_ids;
use bl_shell_spec::export as export_ids;
use bl_shell_spec::midi_files as midi_ids;
use bl_shell_spec::mixer_view as mixer_ids;
use bl_shell_spec::notes as note_ids;
use bl_shell_spec::perception as perception_ids;
use bl_shell_spec::plugins as plugin_ids;
use bl_shell_spec::session_launch as session_launch_ids;
use bl_shell_spec::session_view as session_ids;
use bl_shell_spec::tracks as track_ids;
use bl_shell_spec::transport as transport_ids;

/// The argument a key in a range (F1 to F8) adds to a toggling mixer
/// command for the track it numbers, counting the mixer's columns from 1;
/// [`Shell::invoke`] turns it into the track.
pub const KEY_TRACK_NUMBER: &str = "track_number";

/// The views the main window's main area shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MainView {
    /// The Arrangement View, where a new set opens (IMP-04, D20).
    Arrangement,
    /// The Session View.
    Session,
}

impl MainView {
    /// The view's ID in command results.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Arrangement => "arrangement",
            Self::Session => "session",
        }
    }
}

/// A window the shell knows about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// The main window.
    Main,
    /// The Settings window.
    Settings,
    /// A floating device's window (WP-2.5b, IMP-14).
    Device(bl_model::DeviceId),
}

/// An area of the main window that can hold the keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    /// The Control Bar along the top (L-WIN-003).
    ControlBar,
    /// The browser (L-WIN-004).
    Browser,
    /// The main view: the Arrangement or Session View (L-WIN-005).
    MainView,
    /// The detail area: Info, Clip, and Device Views (L-WIN-006).
    Detail,
    /// The Status Bar (L-WIN-007).
    StatusBar,
}

/// `mixer.set_solo_in_place`, which Options ▸ Solo in Place runs as a
/// switch (M-OPT-025).
const SOLO_IN_PLACE: &str = "mixer.set_solo_in_place";

/// A command and its arguments, kept to run later (after a dialog).
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    /// The command's ID.
    pub id: String,
    /// Its arguments, a JSON object.
    pub args: Value,
}

impl Invocation {
    /// An invocation of `id` with `args`.
    #[must_use]
    pub fn new(id: &str, args: Value) -> Self {
        Self {
            id: id.to_owned(),
            args,
        }
    }
}

/// Something only the platform can do, which a command asks for: the app
/// drains these after each command with [`Shell::take_effects`].
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Ask whether to save the set's unsaved changes, then run `then`. The
    /// person can save first, discard the changes, or cancel.
    ConfirmDiscard {
        /// What to run once the changes are saved or discarded.
        then: Invocation,
    },
    /// Ask for a set file to open, then run `set.open` with it.
    PickOpenPath {
        /// The person chose Don't Save first, so the unsaved changes go
        /// (`set.discard_changes`) once a file is chosen, just before it
        /// opens. Cancelling the dialog keeps them.
        discard_changes: bool,
    },
    /// Ask where to save the set, run `set.save_as` there, then run `then`
    /// if the save worked.
    PickSavePath {
        /// What to run after the save.
        then: Option<Invocation>,
    },
    /// Quit.
    Quit,
    /// Hide the app (macOS).
    Hide,
    /// Show the About window.
    ShowAbout,
    /// Open the Settings window, or bring it to the front.
    OpenSettings,
    /// Close the Settings window.
    CloseSettings,
    /// Enter or leave full-screen mode.
    SetFullScreen(bool),
    /// Move the keyboard focus to an area of the main window.
    FocusArea(Area),
    /// Move the keyboard focus to the next or previous control, or to the
    /// same control on the next or previous track.
    MoveFocus {
        /// Whether it moves backwards.
        backwards: bool,
        /// Whether it moves to the same control on a neighbouring track
        /// (Navigate ▸ Next Neighbor and Previous Neighbor) rather than to
        /// the next control.
        neighbor: bool,
    },
    /// Ask for a folder, then add it to the browser's Places with
    /// `browser.add_folder` (WP-1.10).
    PickFolder,
    /// Start renaming a device in its title bar, where the person types the
    /// new name; Return runs `device.rename` with it (WP-2.4).
    RenameDevice(bl_model::DeviceId),
}

/// Where a gesture that edits the set is (WP-1.8b): see [`Shell::gesture`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GesturePhase {
    /// The gesture goes on: run its command from where the gesture began,
    /// opening the gesture's transaction on the first update.
    Update,
    /// The gesture ends: run its command a last time, if one is given, and
    /// commit, recording one undo step if the set changed.
    Commit,
    /// The gesture is abandoned (Esc): the set goes back to where it began.
    Cancel,
}

/// A gesture under way: what the selection was when it began, and the last
/// command that ran, to fall back on when a frame's command is refused.
#[derive(Debug)]
struct Gesture {
    view: ArrangementView,
    clip_view: ClipView,
    last: Option<Invocation>,
}

/// A line for the Status Bar's message area (L-STAT-002).
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// The text.
    pub text: String,
    /// Whether it reports a failure.
    pub error: bool,
    /// What clicking it runs, if anything.
    pub action: Option<Invocation>,
}

/// Why a command didn't run.
#[derive(Clone, Debug, PartialEq)]
pub enum ShellError {
    /// No command has this ID.
    UnknownCommand(String),
    /// The arguments don't fit the command's schema.
    InvalidArgs {
        /// The command.
        command: String,
        /// What's wrong.
        message: String,
    },
    /// The command can't run now, for the reason given.
    Unavailable {
        /// The command.
        command: String,
        /// Why not.
        reason: String,
    },
    /// The command ran and failed.
    Failed {
        /// The command.
        command: String,
        /// What went wrong.
        message: String,
    },
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "no command is called `{id}`"),
            Self::InvalidArgs { command, message } => {
                write!(f, "invalid arguments for `{command}`: {message}")
            }
            Self::Unavailable { command, reason } => {
                write!(f, "`{command}` can't run now: {reason}")
            }
            Self::Failed { command, message } => write!(f, "`{command}` failed: {message}"),
        }
    }
}

impl std::error::Error for ShellError {}

/// The app shell without a window: the document, the set's file, the
/// window and view state, the preferences, and the audio device, and the
/// one place every command runs.
///
/// A person's menu choice or key press goes through [`Shell::invoke`],
/// which fills in what the person didn't say (a file chosen in a dialog,
/// the next track's name) and asks before losing unsaved changes. An agent
/// or a test goes through [`Shell::run`], which needs every argument and
/// never asks.
pub struct Shell {
    registry: Registry,
    editor: Editor,
    /// The document as it was last opened or saved (or the factory default
    /// set), which `set.discard_changes` goes back to.
    saved: Editor,
    set_path: Option<PathBuf>,
    dirty: bool,
    store: Box<dyn SetStore>,
    audio: AudioHost,
    prefs: Preferences,
    config_dir: Option<PathBuf>,
    main_view: MainView,
    browser: bool,
    /// Whether the Info View shows (L-INFO-001): view state, never part of
    /// the set, like the browser's.
    info_view: bool,
    full_screen: bool,
    settings_open: bool,
    focused_window: WindowKind,
    focused_area: Area,
    /// The track whose control has the keyboard focus (WP-1.14).
    focused_track: Option<TrackId>,
    /// Whether a device's value control (a knob or a number box) has the
    /// keyboard, so its keys go to it (WP-2.5).
    value_focused: bool,
    effects: Vec<Effect>,
    message: Option<Message>,
    revision: u64,
    agent_status: AgentServerStatus,
    /// The built-in assistant's API keys (WP-2.23): the OS keychain, or an
    /// in-memory stand-in in tests and headless runs. The keys are never in
    /// the preferences.
    keychain: Arc<dyn Keychain>,
    /// Whether a key is stored for the assistant's current provider, so the
    /// Settings window doesn't read the keychain every frame.
    assistant_key_set: bool,
    /// Every command the assistant ran this session, in order (WP-2.23,
    /// part 4).
    assistant_log: Vec<ActivityEntry>,
    /// Whether the activity log is open in the Settings window.
    assistant_log_open: bool,
    /// What the Arrangement View shows (WP-1.8a).
    arrangement: ArrangementView,
    /// What the Clip View shows (WP-2.3): whether it's open, Draw Mode, and
    /// the notes selected in its MIDI Note Editor.
    clip_view: ClipView,
    /// A number that changes whenever the document may have changed.
    document_revision: u64,
    /// The browser (WP-1.10).
    browser_view: BrowserView,
    /// The media the set's clips play, which the browser fills and the
    /// Arrangement's waveforms read.
    media: Arc<MediaStore>,
    /// The Arrangement's clipboard (WP-1.8b), kept across sets as Live
    /// keeps it, and never part of a set.
    clipboard: Option<Clipboard>,
    /// A gesture that edits the set, under way (WP-1.8b).
    edit_gesture: Option<Gesture>,
    /// The engine's insert marker when the shell last looked, so a move of
    /// it reaches the Arrangement View once (WP-1.18).
    seen_insert_marker: Option<Ticks>,
    /// Whether a person's gesture (a drag on a value) holds a transaction
    /// open, so its document commands are one undo step.
    gesture: bool,
    /// What the mixers show (WP-1.9).
    mixer: MixerView,
    /// What the Session View grid has selected and how far it's scrolled
    /// (WP-3.7).
    session: SessionView,
    /// The Session slots' states the engine last published (WP-3.2), which
    /// the grid's launch buttons draw from.
    session_states: Vec<bl_engine::SlotStatus>,
    /// The meters the engine last published, one per track (WP-1.9).
    meters: Vec<bl_engine::TrackMeter>,
    /// Whether the meters were the engine's when last polled.
    metering: bool,
    /// The agent API's last audio previews (WP-1.16).
    previews: crate::perception::Previews,
    /// The Device View's selection, folded devices, clipboard, and
    /// hot-swapping (WP-2.4).
    devices: DeviceView,
    /// Whether View ▸ Plug-In Windows shows the plug-in windows (the
    /// floating devices' windows and the plug-ins' editor windows;
    /// WP-2.5b): view state, never part of the set.
    plug_in_windows: bool,
    /// The native device types the engine offers, which the plug-ins'
    /// join in the Device View (WP-2.6).
    native_types: Vec<bl_engine::DeviceType>,
    /// The CLAP host: CLAP devices' instances, on this thread (WP-2.6).
    plugins: ClapHost,
    /// The Audio Unit host: AU devices' units, on this thread, macOS only
    /// (WP-2.8).
    au: AuHost,
    /// The VST3 host: VST3 devices' instances, on this thread (WP-2.7).
    vst3: Vst3Host,
    /// The plug-in scanner (WP-2.9): the plug-ins found on this machine,
    /// their blocklist, and their cache.
    scanner: Scanner,
    /// Plug-ins built on the plug-in kit, compiled in as native devices:
    /// their instances, on this thread.
    kit: KitHost,
    /// The device commands in the registry, by device: plug-ins' own
    /// commands scoped to their devices.
    device_commands: std::collections::BTreeMap<bl_model::DeviceId, Vec<String>>,
    /// What the device commands were last made from: each plug-in device,
    /// its title and track, and whether its plug-in listed commands.
    device_commands_key: Vec<(bl_model::DeviceId, String, String, bool)>,
    /// The CLAP host's generation when the shell last looked for plug-ins'
    /// commands.
    plugins_generation: u64,
    /// The recording under way, or the one whose take just ended
    /// (WP-2.13): its writer's report and the frames the engine dropped.
    recording: Option<crate::recording::Recording>,
    /// How many recordings this session has started: the take number in
    /// their files' names, so two in one second don't share a file.
    recorded_takes: u32,
    /// The control moves of the automation recording under way (WP-2.12).
    automation_take: crate::automation_record::Take,
    /// Whether automation was recording when the engine was last polled:
    /// the transport records and plays.
    automation_recording: bool,
}

impl fmt::Debug for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shell")
            .field("set_path", &self.set_path)
            .field("dirty", &self.dirty)
            .field("main_view", &self.main_view)
            .field("audio", &self.audio)
            .finish_non_exhaustive()
    }
}

/// The shell's error for an edit that didn't happen.
fn edit_error(id: &str, failure: EditFailure) -> ShellError {
    match failure {
        EditFailure::InvalidArgs(message) => ShellError::InvalidArgs {
            command: id.to_owned(),
            message,
        },
        EditFailure::Unavailable(reason) => ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        },
        EditFailure::Refused(message) => ShellError::Failed {
            command: id.to_owned(),
            message,
        },
    }
}

/// Whether `id` is an automation edit a drag on an automation lane runs as
/// one undo step (WP-2.12).
fn is_automation_edit(id: &str) -> bool {
    matches!(
        id,
        "automation.set_breakpoint"
            | "automation.set_curve"
            | "automation.move_breakpoint"
            | "automation.delete_breakpoint"
    )
}

/// The undo step a drag on an automation lane records, named as Live's undo
/// history names that work.
fn automation_label(id: &str) -> &'static str {
    match id {
        "automation.set_curve" => "Curve Automation Segment",
        "automation.move_breakpoint" => "Move Automation Breakpoint",
        "automation.delete_breakpoint" => "Delete Automation Breakpoint",
        _ => "Add Automation Breakpoint",
    }
}

/// Whether `id` is one of the clip marker commands the Clip View's marker
/// strip drags (`clip.set_midi_region`, `clip.set_midi_loop`). They are
/// document commands, but the strip's drags run them as gestures, so moving
/// a marker or the loop brace however far is one undo step (piano-roll R11).
fn clip_marker_command(id: &str) -> bool {
    matches!(id, model::SET_MIDI_REGION | model::SET_MIDI_LOOP)
}

/// The name of the undo step a marker-strip gesture makes.
fn marker_label(id: &str) -> &'static str {
    match id {
        model::SET_MIDI_REGION => "Change clip region",
        _ => "Change clip loop",
    }
}

/// The shell's error for a MIDI file command that didn't run.
fn midi_file_error(id: &str, error: crate::midi_files::MidiFileError) -> ShellError {
    match error {
        crate::midi_files::MidiFileError::InvalidArgs(message) => ShellError::InvalidArgs {
            command: id.to_owned(),
            message,
        },
        crate::midi_files::MidiFileError::Failed(message) => ShellError::Failed {
            command: id.to_owned(),
            message,
        },
    }
}

/// The shell's error for a Device View command that didn't run.
fn device_error(id: &str, error: DeviceError) -> ShellError {
    match error {
        DeviceError::InvalidArgs(message) => ShellError::InvalidArgs {
            command: id.to_owned(),
            message,
        },
        DeviceError::Unavailable(reason) => ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        },
        DeviceError::Failed(message) => ShellError::Failed {
            command: id.to_owned(),
            message,
        },
    }
}

/// Converts a command's result to JSON.
fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A fresh editor on the same document, with an empty undo history.
fn snapshot(editor: &Editor) -> Editor {
    Editor::from_document(editor.document().clone(), editor.ids())
}

impl Shell {
    /// A shell with these preferences, saving them to `config_dir` (or
    /// nowhere), playing through `audio`, and opening and saving sets
    /// through `store`. It starts on the factory default set in the
    /// Arrangement View. The audio device opens with [`Shell::start_audio`].
    #[must_use]
    pub fn new(
        prefs: Preferences,
        config_dir: Option<PathBuf>,
        mut audio: AudioHost,
        store: Box<dyn SetStore>,
    ) -> Self {
        let mut registry = Registry::new();
        model::register(&mut registry).expect("the document's command IDs are unique");
        crate::register(&mut registry).expect("the shell's command IDs are unique");
        // The transport's commands (WP-1.5), which the shell sends to the
        // engine it plays the set through (WP-1.18).
        engine_transport::register(&mut registry).expect("the transport's command IDs are unique");
        // The Session View's launching and stopping (WP-3.2), which the
        // shell sends to the engine too.
        engine_session::register(&mut registry).expect("the Session View's command IDs are unique");
        debug_assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        audio.set_tone(prefs.tone);
        let browser_view = BrowserView::new(&prefs.browser, Waker::default());
        audio.set_preview_gain(prefs.browser.preview_volume as f32);
        // Spec: file-management R1. A saved default set is what a new set
        // starts from; without one, the factory default set.
        let editor = new_set_from(prefs.default_set.as_deref());
        // The engine's clips stream from the set's media (WP-1.4b).
        let media = Arc::new(MediaStore::new());
        audio.set_media(Arc::clone(&media));
        // CLAP plug-ins (WP-2.6): this thread is their main thread.
        let plugins = ClapHost::new();
        audio.set_plugins(plugins.clone());
        // Audio Units (WP-2.8): their units live with this thread too.
        let au = AuHost::new();
        audio.set_au(au.clone());
        // VST3 plug-ins (WP-2.7): this thread is their main thread too.
        let vst3 = Vst3Host::new();
        audio.set_vst3(vst3.clone());
        // The plug-in scanner (WP-2.9): finds the plug-ins on this machine
        // in a child process, caching and blocklisting what it finds. Its
        // cache lives with the preferences.
        let scanner = match &config_dir {
            Some(dir) => Scanner::new().with_store(dir.join(crate::settings::PLUGIN_SCAN_FILE)),
            None => Scanner::new(),
        };
        // The kit's plug-ins compiled in, as native devices: its examples
        // with bl-devices' `kit-examples` feature, none otherwise.
        let kit = bl_devices::kit_host();
        audio.set_kit(kit.clone());
        let native_types = bl_devices::device_types();
        let mut described = native_types.clone();
        described.extend(kit.device_types());
        audio.sync(editor.document());
        let saved = snapshot(&editor);
        let plugin_panels = prefs.plugin_panels.clone();
        let mut shell = Self {
            browser_view,
            media,
            registry,
            editor,
            saved,
            set_path: None,
            dirty: false,
            store,
            audio,
            settings_open: false,
            prefs,
            config_dir,
            main_view: MainView::Arrangement,
            browser: true,
            info_view: true,
            full_screen: false,
            focused_window: WindowKind::Main,
            focused_area: Area::MainView,
            focused_track: None,
            value_focused: false,
            effects: Vec::new(),
            message: None,
            revision: 0,
            agent_status: AgentServerStatus::default(),
            keychain: Arc::new(OsKeychain::new()),
            assistant_key_set: false,
            assistant_log: Vec::new(),
            assistant_log_open: false,
            arrangement: ArrangementView::new(),
            clip_view: ClipView::default(),
            document_revision: 0,
            clipboard: None,
            edit_gesture: None,
            au,
            seen_insert_marker: None,
            gesture: false,
            mixer: MixerView::new(),
            session: SessionView::new(),
            session_states: Vec::new(),
            meters: Vec::new(),
            metering: false,
            previews: crate::perception::Previews::default(),
            recording: None,
            recorded_takes: 0,
            automation_take: crate::automation_record::Take::default(),
            automation_recording: false,
            plug_in_windows: true,
            devices: {
                // Beatlayer's native devices (WP-2.5; none ship yet).
                let mut devices = DeviceView::new();
                devices.set_device_types(described);
                devices.set_plugin_panels(plugin_panels);
                devices
            },
            native_types,
            plugins,
            vst3,
            scanner,
            kit,
            device_commands: std::collections::BTreeMap::new(),
            device_commands_key: Vec::new(),
            plugins_generation: 0,
        };
        shell.give_agents_tokens();
        shell.publish_plugins();
        shell
    }

    /// What the plug-in scanner has found, with the browser's hidden and
    /// format filters applied (WP-2.9, IMP-07), pushed into the browser's
    /// Plug-Ins label.
    fn publish_plugins(&mut self) {
        let plugins = self.scanner.visible(
            self.prefs.show_hidden_plugins,
            &self.prefs.hidden_plugins,
            self.prefs.plugin_format_preference,
        );
        self.browser_view.set_plugins(plugins);
    }

    /// Makes each allowed agent that has no token of its own one (an agent
    /// allowed before agents had them), and keeps them (WP-1.15b).
    fn give_agents_tokens(&mut self) {
        let mut made = false;
        for client in &mut self.prefs.agent.clients {
            if client.token.is_empty()
                && let Ok(token) = agent::new_token()
            {
                client.token = token.into();
                made = true;
            }
        }
        if made {
            self.save_preferences();
        }
    }

    /// A shell for tests and headless runs: default preferences saved
    /// nowhere, the null audio device, and Beatlayer's set files.
    #[must_use]
    pub fn headless() -> Self {
        let mut shell = Self::new(
            Preferences::default(),
            None,
            AudioHost::null(Default::default()),
            Box::new(ProjectFiles::new()),
        );
        // Headless runs and tests touch no real keychain.
        shell.keychain = Arc::new(MemoryKeychain::new());
        shell
    }

    /// Opens the audio device, if the engine is on. A failure shows in the
    /// Status Bar with a way to the Audio settings.
    pub fn start_audio(&mut self) {
        if let Err(error) = self.audio.start() {
            self.audio_failed(&error);
        }
        self.touch();
    }

    fn audio_failed(&mut self, error: &str) {
        self.message = Some(Message {
            text: format!(
                "The audio device couldn't start: {error}. Click to open the Audio settings."
            ),
            error: true,
            action: Some(Invocation::new(
                ids::APP_SETTINGS,
                json!({ "page": "audio" }),
            )),
        });
    }

    /// Every command: the document's and the shell's.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The editor holding the document.
    #[must_use]
    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// Runs `edit` on the editor holding the document, and marks the set
    /// changed if the document changed. The agent API (WP-1.15) edits this
    /// way where one command at a time isn't enough: a batch of document
    /// commands as one undo step labelled with the agent's name, a dry run
    /// that applies a batch and cancels it, a new ID for an object a
    /// command will create, and restoring a checkpoint. It runs document
    /// commands with `bl_model::registry::run`, as [`Shell::run`] does.
    ///
    /// `edit` must leave no transaction open.
    pub fn edit<T>(&mut self, edit: impl FnOnce(&mut Editor) -> T) -> T {
        let before = self.editor.document().clone();
        let result = edit(&mut self.editor);
        debug_assert!(
            !self.editor.in_transaction(),
            "an edit left a transaction open"
        );
        if !ChangeSet::between(&before, self.editor.document()).is_empty() {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        result
    }

    /// A new ID for an object a command will create, such as a track's.
    pub fn new_id<T: From<Id>>(&mut self) -> T {
        self.editor.new_id()
    }

    /// The agent server's settings.
    #[must_use]
    pub fn agent_settings(&self) -> &AgentSettings {
        &self.prefs.agent
    }

    /// What the app last said about the running agent server.
    #[must_use]
    pub fn agent_status(&self) -> &AgentServerStatus {
        &self.agent_status
    }

    /// Records what the app says about the running agent server, for the
    /// Settings window.
    pub fn set_agent_status(&mut self, status: AgentServerStatus) {
        if self.agent_status != status {
            self.agent_status = status;
            self.touch();
        }
    }

    /// The built-in assistant's provider, base URL and model (WP-2.23,
    /// part 4). Its API key is in the keychain, not here.
    #[must_use]
    pub fn assistant_settings(&self) -> &AssistantSettings {
        &self.prefs.assistant
    }

    /// The assistant's settings and whether a key is stored, for the
    /// Settings window and the settings' commands. The key itself is never
    /// here.
    #[must_use]
    pub fn assistant_state(&self) -> AssistantState {
        AssistantState {
            provider: self.prefs.assistant.provider,
            base_url: self.prefs.assistant.base_url.clone(),
            model: self.prefs.assistant.model.clone(),
            key_set: self.assistant_key_set,
            activity_log_open: self.assistant_log_open,
        }
    }

    /// Every command the assistant ran this session, in order (WP-2.23,
    /// part 4).
    #[must_use]
    pub fn assistant_log(&self) -> &[ActivityEntry] {
        &self.assistant_log
    }

    /// Whether the activity log is open.
    #[must_use]
    pub fn assistant_log_open(&self) -> bool {
        self.assistant_log_open
    }

    /// Whether an API key is stored for the assistant's current provider.
    /// Refreshed by [`Shell::refresh_assistant_key`], after a key changes,
    /// and when the Settings window opens.
    #[must_use]
    pub fn assistant_key_set(&self) -> bool {
        self.assistant_key_set
    }

    /// Runs the assistant's keychain operations through `keychain`, such as
    /// an in-memory one in tests, and refreshes whether a key is stored.
    pub fn use_keychain(&mut self, keychain: Arc<dyn Keychain>) {
        self.keychain = keychain;
        self.refresh_assistant_key();
    }

    /// Reads whether an API key is stored for the assistant's current
    /// provider, so the Settings window can show it without reading the
    /// keychain every frame.
    pub fn refresh_assistant_key(&mut self) {
        let account = keychain_account(self.prefs.assistant.provider);
        self.assistant_key_set = matches!(self.keychain.read(account), Ok(Some(_)));
    }

    /// Opens a turn's changes as one undo step named `label`, which the
    /// assistant loop closes with [`Shell::end_assistant_turn`]
    /// (WP-2.23, part 2).
    pub(crate) fn begin_assistant_turn(&mut self, label: &str) {
        self.editor.begin(label.to_owned());
    }

    /// Closes the turn's undo step, keeping whatever ran.
    pub(crate) fn end_assistant_turn(&mut self) {
        if self.editor.commit().unwrap_or(false) {
            self.dirty = true;
        }
        self.document_changed();
        self.touch();
    }

    /// Whether the person has chosen "always allow" for `command`.
    pub(crate) fn assistant_always_allows(&self, command: &str) -> bool {
        self.prefs.assistant_always.contains(command)
    }

    /// Remembers "always allow" for `command`, so later calls don't ask.
    pub(crate) fn assistant_remember_command(&mut self, command: &str) {
        if self.prefs.assistant_always.insert(command.to_owned()) {
            self.save_preferences();
        }
    }

    /// Records one command the assistant ran in the activity log.
    pub(crate) fn assistant_record(&mut self, entry: ActivityEntry) {
        self.assistant_log.push(entry);
        self.touch();
    }

    /// Records one command the assistant was refused, in the activity log.
    pub(crate) fn assistant_record_denied(&mut self, command: &str, message: String) {
        self.assistant_log.push(ActivityEntry {
            command: command.to_owned(),
            at: std::time::SystemTime::now(),
            outcome: Outcome::Denied(message),
        });
        self.touch();
    }

    /// The audio device.
    #[must_use]
    pub fn audio(&self) -> &AudioHost {
        &self.audio
    }

    /// The audio device, to sample its CPU load.
    pub fn audio_mut(&mut self) -> &mut AudioHost {
        &mut self.audio
    }

    /// The audio interface the In/Out choosers list: the open device's
    /// channels, or none while no stream is open.
    #[must_use]
    pub fn interface(&self) -> crate::routing::Interface {
        let (inputs, outputs) = self.audio.channels();
        crate::routing::Interface { inputs, outputs }
    }

    /// The preferences.
    #[must_use]
    pub fn preferences(&self) -> &Preferences {
        &self.prefs
    }

    /// The set's file, if it has one.
    #[must_use]
    pub fn set_path(&self) -> Option<&Path> {
        self.set_path.as_deref()
    }

    /// The set's name: its file's name without the extension, or `Untitled`.
    #[must_use]
    pub fn set_name(&self) -> String {
        self.set_path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(
                || "Untitled".to_owned(),
                |s| s.to_string_lossy().into_owned(),
            )
    }

    /// The folder the Save Set As dialog opens in: the open set's project
    /// folder, so keeping the suggestion saves into the same project
    /// (app-shell R22), or `None` for a set never saved, whose dialog opens
    /// where the system left it.
    #[must_use]
    pub fn save_dialog_folder(&self) -> Option<PathBuf> {
        self.set_path
            .as_deref()
            .map(|path| bl_project::ProjectFolder::of_set(path).root().to_owned())
    }

    /// Whether the set has changes that aren't saved.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The main view in front.
    #[must_use]
    pub fn main_view(&self) -> MainView {
        self.main_view
    }

    /// Whether the browser shows.
    #[must_use]
    pub fn browser_visible(&self) -> bool {
        self.browser
    }

    /// Whether the Info View shows (L-INFO-001): view state only.
    #[must_use]
    pub fn info_view_visible(&self) -> bool {
        self.info_view
    }

    /// Whether the main window is in full-screen mode.
    #[must_use]
    pub fn full_screen(&self) -> bool {
        self.full_screen
    }

    /// The interface zoom, where 1 is 100%: a whole percentage from 50% to
    /// 200% (WP-1.17). A preferences file with anything else starts at the
    /// nearest limit, or at 100% if it isn't a number (interface-zoom R6).
    #[must_use]
    pub fn zoom(&self) -> f32 {
        zoom_factor(self.zoom_percent())
    }

    /// The interface zoom as a whole percentage, as the Display & Input
    /// page's Zoom shows it.
    #[must_use]
    pub fn zoom_percent(&self) -> u16 {
        zoom_percent(self.prefs.zoom.unwrap_or(1.0))
    }

    /// Sets the interface zoom and keeps it in the preferences: none for
    /// 100%, so a new install's file has no zoom in it.
    fn set_zoom_percent(&mut self, percent: u16) {
        let percent = percent.clamp(ZOOM_MIN_PERCENT, ZOOM_MAX_PERCENT);
        let zoom = (percent != 100).then(|| zoom_factor(percent));
        if self.prefs.zoom != zoom {
            self.prefs.zoom = zoom;
            self.save_preferences();
        }
    }

    /// Whether the Settings window is open.
    #[must_use]
    pub fn settings_open(&self) -> bool {
        self.settings_open
    }

    /// The Settings page showing, or shown last.
    #[must_use]
    pub fn settings_page(&self) -> SettingsPage {
        self.prefs.settings_page
    }

    /// Shows a Settings page, as clicking it in the page chooser does.
    pub fn show_settings_page(&mut self, page: SettingsPage) {
        if self.prefs.settings_page != page {
            self.prefs.settings_page = page;
            self.save_preferences();
            self.touch();
        }
    }

    /// Whether Create Fades on Clip Edges is on: new audio clips get 4 ms
    /// fades at their edges.
    #[must_use]
    pub fn create_fades_on_clip_edges(&self) -> bool {
        self.prefs.create_fades_on_clip_edges.0
    }

    /// The theme in use.
    #[must_use]
    pub fn theme(&self) -> ThemeChoice {
        self.prefs.theme
    }

    /// Whether Tab moves the keyboard focus in the main window.
    #[must_use]
    pub fn tab_navigation(&self) -> bool {
        self.prefs.tab_navigation
    }

    /// Whether Tab and Shift-Tab wrap round at the ends of the focused area
    /// (Wrap Tab Navigation; the accessibility spec, R5).
    #[must_use]
    pub fn wrap_navigation(&self) -> bool {
        self.prefs.wrap_navigation.0
    }

    /// Whether a border is drawn round the focused area (Outline View in
    /// Focus; the accessibility spec, R7).
    #[must_use]
    pub fn outline_view_in_focus(&self) -> bool {
        self.prefs.outline_view_in_focus.0
    }

    /// The track whose control has the keyboard focus, as the window last
    /// reported it, for the neighbour commands (the accessibility spec,
    /// R4).
    #[must_use]
    pub fn focused_track(&self) -> Option<TrackId> {
        self.focused_track
    }

    /// Records the track whose control has the keyboard focus, or none.
    pub fn set_focused_track(&mut self, track: Option<TrackId>) {
        if self.focused_track != track {
            self.focused_track = track;
            self.touch();
        }
    }

    /// The tracks in the order the neighbour commands move through them:
    /// the mixer's columns, then the returns and Main.
    #[must_use]
    pub fn neighbor_order(&self) -> Vec<TrackId> {
        let doc = self.editor.document();
        let mut tracks = self.mixer_columns();
        tracks.extend(doc.returns());
        tracks.push(doc.main_id());
        tracks
    }

    /// Whether the track with the focus (or the selected track) has a
    /// neighbour that way: Navigate's Next Neighbor and Previous Neighbor
    /// are enabled by it, whatever the Tab setting (observed in Live
    /// 12.4.6; the accessibility spec, R4).
    fn has_neighbor(&self, backwards: bool) -> bool {
        let order = self.neighbor_order();
        let Some(track) = self
            .focused_track
            .or_else(|| self.arrangement.selected_tracks().first().copied())
        else {
            return false;
        };
        order.iter().position(|t| *t == track).is_some_and(|at| {
            if backwards {
                at > 0
            } else {
                at + 1 < order.len()
            }
        })
    }

    /// The window with the keyboard focus.
    #[must_use]
    pub fn focused_window(&self) -> WindowKind {
        self.focused_window
    }

    /// The main window's area with the keyboard focus.
    #[must_use]
    pub fn focused_area(&self) -> Area {
        self.focused_area
    }

    /// Records which window has the keyboard focus.
    pub fn set_focused_window(&mut self, window: WindowKind) {
        if self.focused_window != window {
            self.focused_window = window;
            if let WindowKind::Device(device) = window
                && self.editor.document().device(device).is_some()
            {
                // A floating device's window in front selects its device,
                // so keys act on it.
                self.devices.select_only(device);
            }
            self.touch();
        }
    }

    /// Records which area of the main window has the keyboard focus, as
    /// clicking in it does (I-FOCUS-001).
    pub fn set_focused_area(&mut self, area: Area) {
        if self.focused_area != area {
            self.focused_area = area;
            // A device's value control lives in the detail area.
            self.value_focused &= area == Area::Detail;
            self.touch();
        }
    }

    /// Records whether a device's value control has the keyboard: while it
    /// does, the Device View's keys give way to the control's own (Delete
    /// sets its default, as S-DEV-006 does in Live), and global ones still
    /// work.
    pub fn set_value_focused(&mut self, focused: bool) {
        if self.value_focused != focused {
            self.value_focused = focused;
            self.touch();
        }
    }

    /// Records the main window's size, kept for the next start.
    pub fn set_window_size(&mut self, width: f32, height: f32) {
        self.prefs.window_size = Some((width, height));
    }

    /// The Status Bar's message.
    #[must_use]
    pub fn message(&self) -> Option<&Message> {
        self.message.as_ref()
    }

    /// Shows a message in the Status Bar.
    pub fn set_message(&mut self, text: impl Into<String>, error: bool) {
        self.message = Some(Message {
            text: text.into(),
            error,
            action: None,
        });
        self.touch();
    }

    /// Clears the Status Bar's message.
    pub fn clear_message(&mut self) {
        if self.message.take().is_some() {
            self.touch();
        }
    }

    /// A number that changes whenever anything a window or menu shows may
    /// have changed, so they know when to refresh.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// A number that changes whenever the document may have changed: after
    /// every document command, undo and redo, and a new or opened set. Views
    /// that index the document rebuild their index when it changes.
    #[must_use]
    pub fn document_revision(&self) -> u64 {
        self.document_revision
    }

    /// Records that the document may have changed: drops view state for
    /// tracks that are gone, and gives the engine the new document, so it
    /// plays the set as it is now (its tempo, loop, and tracks).
    fn document_changed(&mut self) {
        self.document_revision = self.document_revision.wrapping_add(1);
        self.arrangement
            .forget_missing_tracks(self.editor.document());
        self.mixer
            .clamp_scroll(self.editor.document(), &self.arrangement);
        self.devices.forget_missing(self.editor.document());
        // The kit plug-ins' instances first, so a node the engine makes at once
        // finds its device's.
        self.kit.sync(self.editor.document());
        self.audio.sync(self.editor.document());
        self.refresh_device_commands(false);
    }

    /// Brings the registry's device commands in step with the set: each
    /// plug-in device whose plug-in lists commands has them, scoped to it,
    /// and a device that's gone has none. `force` remakes them even if no
    /// device changed, as when plug-ins may have started.
    fn refresh_device_commands(&mut self, force: bool) {
        let doc = self.editor.document();
        let key: Vec<(bl_model::DeviceId, String, String, bool)> = doc
            .tracks()
            .flat_map(|track| {
                let label = crate::routing::track_label(doc, track.id());
                track
                    .devices()
                    .filter(|d| self.kit.handles(&d.kind) || crate::plugins::is_clap(doc, d.id))
                    .map(|d| {
                        let listed = if self.kit.handles(&d.kind) {
                            self.kit.commands(d.id).is_some()
                        } else {
                            self.plugins.commands(d.id).is_some()
                        };
                        (d.id, d.title().to_owned(), label.clone(), listed)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        if !force && key == self.device_commands_key {
            return;
        }
        let found = crate::plugins::device_commands(doc, &self.plugins, &self.kit);
        for ids in std::mem::take(&mut self.device_commands).into_values() {
            for id in ids {
                self.registry.remove(&id);
            }
        }
        for (device, specs) in found {
            let mut ids = Vec::with_capacity(specs.len());
            for spec in specs {
                let id = spec.id.to_string();
                if self.registry.register(spec).is_ok() {
                    ids.push(id);
                }
            }
            self.device_commands.insert(device, ids);
        }
        self.device_commands_key = key;
    }

    /// The device commands of `device`: its plug-in's own commands, scoped
    /// to it (`device.d<device>.<command>`), as the registry holds them.
    #[must_use]
    pub fn device_command_ids(&self, device: bl_model::DeviceId) -> &[String] {
        self.device_commands.get(&device).map_or(&[], Vec::as_slice)
    }

    /// The kit plug-ins compiled in as native devices: their instances, on
    /// this thread.
    #[must_use]
    pub fn kit(&self) -> &KitHost {
        &self.kit
    }

    /// Runs `device`'s plug-in's own command `command`, which the registry
    /// holds as the device command `id`, and records the parameters it
    /// changed as one undo step named for it (or inside the step already
    /// open, as an agent's labelled one).
    fn run_device_command(
        &mut self,
        id: &str,
        device: bl_model::DeviceId,
        command: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let kind = self
            .editor
            .document()
            .device(device)
            .map(|d| d.kind.clone())
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: format!("{device} isn't in the set"),
            })?;
        let (result, changes) = if self.kit.handles(&kind) {
            self.kit.sync(self.editor.document());
            let run = self
                .kit
                .run_command(device, command, &args)
                .map_err(failed)?;
            (run.result, run.changes)
        } else {
            let run =
                self.plugins
                    .run_command(device, command, &args)
                    .map_err(|error| match error {
                        bl_plugin_host::clap::ClapError::Refused(why) => failed(why),
                        other => failed(other.to_string()),
                    })?;
            (run.result, run.changes)
        };
        let label = self
            .registry
            .get(id)
            .map_or_else(|| command.to_owned(), |spec| spec.label.to_string());
        let before = self.editor.document().clone();
        let applied = self.editor.transact(label, |editor| {
            for (param, value) in &changes {
                let same = editor
                    .document()
                    .device(device)
                    .and_then(|d| d.params.get(param))
                    == Some(value);
                if !same {
                    editor.apply(bl_model::Command::SetDeviceParam {
                        device,
                        param: *param,
                        value: *value,
                    })?;
                }
            }
            Ok::<(), bl_model::EditError>(())
        });
        applied.map_err(|error| failed(error.to_string()))?;
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        Ok(result)
    }

    /// The Device View's selection, folded devices, clipboard, and
    /// hot-swapping (WP-2.4).
    #[must_use]
    pub fn devices(&self) -> &DeviceView {
        &self.devices
    }

    /// Sets the device types the host's engine offers, with their
    /// parameters' descriptions, which the Device View's generic UI draws,
    /// `device.parameters` reports, and loading a preset checks values
    /// against (WP-2.5).
    pub fn set_device_types(&mut self, types: Vec<bl_engine::DeviceType>) {
        self.native_types = types;
        self.refresh_device_types();
    }

    /// Gives the Device View the native types, the kit plug-ins compiled
    /// in, and the plug-ins loaded.
    fn refresh_device_types(&mut self) {
        let mut types = self.native_types.clone();
        types.extend(self.kit.device_types());
        types.extend(self.plugins.types().into_iter().map(|t| t.device_type));
        types.extend(self.au.types().into_iter().map(|t| t.device_type));
        types.extend(self.vst3.types().into_iter().map(|t| t.device_type));
        self.devices.set_device_types(types);
    }

    /// The CLAP host, whose plug-ins are the set's CLAP devices
    /// (WP-2.6).
    #[must_use]
    pub fn plugins(&self) -> &ClapHost {
        &self.plugins
    }

    /// The VST3 host, whose plug-ins are the set's VST3 devices (WP-2.7).
    #[must_use]
    pub fn vst3(&self) -> &Vst3Host {
        &self.vst3
    }

    /// Lets go of the plug-ins of the set being replaced (WP-2.6): the new
    /// set's plug-in devices load their plug-ins from their saved state.
    fn set_replaced(&mut self) {
        self.plugins.clear();
        self.au.clear();
        self.vst3.clear();
        self.kit.clear();
    }

    /// Whether `id` is a device command the registry holds: a plug-in's
    /// own command scoped to its device.
    fn is_device_command(&self, id: &str) -> bool {
        id.starts_with(crate::plugins::DEVICE_COMMAND_PREFIX) && self.registry.contains(id)
    }

    /// Runs a plug-in command (WP-2.6).
    fn run_plugins(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        let target = |shell: &Self, device: Option<bl_model::DeviceId>| {
            let device = device
                .or(shell.devices.selected())
                .ok_or_else(|| unavailable("select a plug-in device first".to_owned()))?;
            if crate::plugins::is_plugin(shell.editor.document(), device)
                || crate::plugins::is_au(shell.editor.document(), device)
            {
                Ok(device)
            } else {
                Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!("{device} isn't a plug-in device"),
                })
            }
        };
        let info = |shell: &Self, device| {
            crate::plugins::info(&shell.plugins, &shell.au, &shell.vst3, device)
                .map(|mut info| {
                    // WP-2.10: where the window sits, from the frame store
                    // the set keeps.
                    info.window = shell.devices.window_frame(device);
                    to_value(&info)
                })
                .ok_or_else(|| unavailable(format!("{device}'s plug-in hasn't started yet")))
        };
        match id {
            plugin_ids::LOAD => {
                let plugin_ids::LoadPluginArgs {
                    path,
                    plugin,
                    track,
                    index,
                } = Self::args(id, args)?;
                let path = std::path::PathBuf::from(path);
                let failed = |e: String| ShellError::Failed {
                    command: id.to_owned(),
                    message: e,
                };
                let found: Vec<(bl_engine::DeviceType, std::path::PathBuf)> =
                    if crate::plugins::is_vst3_path(&path) {
                        self.vst3
                            .load(&path)
                            .map_err(|e| failed(e.to_string()))?
                            .into_iter()
                            .map(|t| (t.device_type, t.path))
                            .collect()
                    } else {
                        self.plugins
                            .load(&path)
                            .map_err(|e| failed(e.to_string()))?
                            .into_iter()
                            .map(|t| (t.device_type, t.path))
                            .collect()
                    };
                let (device_type, loaded_path) = match &plugin {
                    Some(wanted) => found.iter().find(|(t, _)| {
                        matches!(&t.kind,
                            bl_model::DeviceKind::Plugin { plugin_id, .. } if plugin_id == wanted)
                    }),
                    None => found.first(),
                }
                .ok_or_else(|| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!(
                        "{} has no plug-in {}",
                        path.display(),
                        plugin.unwrap_or_default()
                    ),
                })?
                .clone();
                self.refresh_device_types();
                let added: device_ids::DeviceAdded = serde_json::from_value(self.run_devices(
                    device_ids::ADD,
                    to_value(&device_ids::AddDeviceArgs {
                        device: device_ids::NewDevice {
                            kind: device_type.kind.clone(),
                            name: device_type.name.clone(),
                            role: device_type.role,
                            sidechain: false,
                        },
                        track,
                        index,
                    }),
                )?)
                .map_err(|e| ShellError::Failed {
                    command: id.to_owned(),
                    message: e.to_string(),
                })?;
                // Spec: clap-hosting R8. The set knows where the plug-in
                // came from from the start.
                let _ = self.editor.amend_plugin_state(
                    added.device,
                    Some(bl_model::PluginState {
                        path: loaded_path.to_string_lossy().into_owned(),
                        data: Vec::new(),
                    }),
                );
                let plugin_id = match &device_type.kind {
                    bl_model::DeviceKind::Plugin { plugin_id, .. } => plugin_id.clone(),
                    bl_model::DeviceKind::Native { device_type } => device_type.clone(),
                };
                Ok(to_value(&plugin_ids::PluginLoaded {
                    device: added.device,
                    track: added.track,
                    new_track: added.new_track,
                    plugin: plugin_id,
                    name: device_type.name,
                }))
            }
            plugin_ids::LOAD_AU => {
                let plugin_ids::LoadAudioUnitArgs {
                    component_type,
                    subtype,
                    manufacturer,
                    track,
                    index,
                } = Self::args(id, args)?;
                let found = self
                    .au
                    .load(&component_type, &subtype, &manufacturer)
                    .map_err(|error| match error {
                        bl_plugin_host::au::AuError::Unavailable => unavailable(error.to_string()),
                        error => ShellError::Failed {
                            command: id.to_owned(),
                            message: error.to_string(),
                        },
                    })?;
                self.refresh_device_types();
                let device_type = found.device_type;
                let added: device_ids::DeviceAdded = serde_json::from_value(self.run_devices(
                    device_ids::ADD,
                    to_value(&device_ids::AddDeviceArgs {
                        device: device_ids::NewDevice {
                            kind: device_type.kind.clone(),
                            name: device_type.name.clone(),
                            role: device_type.role,
                            sidechain: false,
                        },
                        track,
                        index,
                    }),
                )?)
                .map_err(|e| ShellError::Failed {
                    command: id.to_owned(),
                    message: e.to_string(),
                })?;
                // Spec: au-hosting R8. An Audio Unit has no file: the set
                // keeps its own state from the start.
                let _ = self.editor.amend_plugin_state(
                    added.device,
                    Some(bl_model::PluginState {
                        path: String::new(),
                        data: Vec::new(),
                    }),
                );
                let plugin_id = match &device_type.kind {
                    bl_model::DeviceKind::Plugin { plugin_id, .. } => plugin_id.clone(),
                    bl_model::DeviceKind::Native { device_type } => device_type.clone(),
                };
                Ok(to_value(&plugin_ids::PluginLoaded {
                    device: added.device,
                    track: added.track,
                    new_track: added.new_track,
                    plugin: plugin_id,
                    name: device_type.name,
                }))
            }
            plugin_ids::SHOW_WINDOW => {
                let plugin_ids::ShowWindowArgs { device, shown } = Self::args(id, args)?;
                let device = target(self, device)?;
                if crate::plugins::is_vst3(self.editor.document(), device) {
                    return Err(unavailable(
                        "a VST3 plug-in's editor is a later part of VST3 hosting".to_owned(),
                    ));
                }
                let is_au = crate::plugins::is_au(self.editor.document(), device);
                self.plugins.idle();
                self.au.idle();
                let open = crate::plugins::info(&self.plugins, &self.au, &self.vst3, device)
                    .is_some_and(|info| info.window_open);
                if shown.unwrap_or(!open) {
                    // Spec: clap-hosting R13 and au-hosting's second part.
                    // Titled "<device>/<track>", as Live titles a plug-in's
                    // window.
                    let doc = self.editor.document();
                    let title = doc
                        .device_track(device)
                        .map(|t| crate::routing::track_label(doc, t))
                        .unwrap_or_default();
                    let name = doc
                        .device(device)
                        .map(|d| d.title().to_owned())
                        .unwrap_or_default();
                    let title = format!("{name}/{title}");
                    if is_au {
                        self.au
                            .open_editor(device, &title)
                            .map_err(|e| ShellError::Failed {
                                command: id.to_owned(),
                                message: e.to_string(),
                            })?;
                    } else {
                        self.plugins.open_editor(device, &title).map_err(|e| {
                            ShellError::Failed {
                                command: id.to_owned(),
                                message: e.to_string(),
                            }
                        })?;
                    }
                    // Its window follows View ▸ Plug-In Windows and
                    // Auto-Hide Plug-In Windows (R31, R33).
                    self.apply_plug_in_windows();
                } else if is_au {
                    self.au
                        .close_editor(device)
                        .map_err(|e| ShellError::Failed {
                            command: id.to_owned(),
                            message: e.to_string(),
                        })?;
                } else {
                    self.plugins
                        .close_editor(device)
                        .map_err(|e| ShellError::Failed {
                            command: id.to_owned(),
                            message: e.to_string(),
                        })?;
                }
                self.touch();
                info(self, device)
            }
            plugin_ids::INFO => {
                let plugin_ids::PluginInfoArgs { device } = Self::args(id, args)?;
                let device = target(self, device)?;
                self.plugins.idle();
                self.au.idle();
                self.vst3.idle();
                info(self, device)
            }
            plugin_ids::SET_WINDOW => {
                let plugin_ids::SetWindowArgs { device, frame } = Self::args(id, args)?;
                let device = target(self, device)?;
                if !frame.is_valid() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message:
                            "a window's position and size must be finite, and its size above 0"
                                .to_owned(),
                    });
                }
                // WP-2.10: the frame store the set saves; no undo step.
                self.devices.set_editor_frame(device, frame);
                self.plugins.idle();
                info(self, device)
            }
            plugin_ids::SET_PANEL_PARAM => {
                let plugin_ids::SetPanelParamArgs {
                    device,
                    index,
                    param,
                } = Self::args(id, args)?;
                let device = target(self, device)?;
                if index >= plugin_ids::PANEL_KNOBS {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!(
                            "a plug-in panel has {} knobs, so index {index} is out of range",
                            plugin_ids::PANEL_KNOBS
                        ),
                    });
                }
                let kind = self
                    .editor
                    .document()
                    .device(device)
                    .map(|d| d.kind.clone())
                    .ok_or_else(|| unavailable(format!("no {device}")))?;
                let Some(key) = DeviceView::plugin_key(&kind) else {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("{device} isn't a plug-in device"),
                    });
                };
                let mapping = self.prefs.plugin_panels.entry(key).or_default();
                match param {
                    Some(param) => {
                        mapping.insert(index, param);
                    }
                    None => {
                        mapping.remove(&index);
                    }
                }
                self.devices
                    .set_plugin_panels(self.prefs.plugin_panels.clone());
                self.save_preferences();
                self.touch();
                Ok(to_value(&self.device_view_state()))
            }
            plugin_ids::SCAN | plugin_ids::RESCAN => {
                let plugin_ids::ScanArgs { paths } = Self::args(id, args)?;
                let paths: Option<Vec<std::path::PathBuf>> =
                    paths.map(|paths| paths.into_iter().map(std::path::PathBuf::from).collect());
                let report = crate::plugins::scan_report(
                    &self.scanner,
                    paths.as_deref(),
                    id == plugin_ids::RESCAN,
                    &self.prefs.hidden_plugins,
                );
                self.publish_plugins();
                Ok(to_value(&report))
            }
            plugin_ids::LIST => {
                let _: plugin_ids::ListArgs = Self::args(id, args)?;
                let list = crate::plugins::set_plugin_list(self.editor.document(), &self.scanner);
                Ok(to_value(&list))
            }
            plugin_ids::SETTINGS => {
                let plugin_ids::PluginSettingsArgs {
                    hidden,
                    show_hidden,
                    format_preference,
                } = Self::args(id, args)?;
                let mut changed = false;
                if let Some(hidden) = hidden {
                    self.prefs.hidden_plugins = hidden.into_iter().collect();
                    changed = true;
                }
                if let Some(show_hidden) = show_hidden {
                    self.prefs.show_hidden_plugins = show_hidden;
                    changed = true;
                }
                if let Some(format) = format_preference {
                    self.prefs.plugin_format_preference = Some(format);
                    changed = true;
                }
                if changed {
                    self.save_preferences();
                    self.publish_plugins();
                }
                Ok(to_value(&plugin_ids::PluginSettings {
                    hidden: self.prefs.hidden_plugins.iter().cloned().collect(),
                    show_hidden: self.prefs.show_hidden_plugins,
                    format_preference: self.prefs.plugin_format_preference,
                }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// What the Device View shows: the selected track's chain (WP-2.4).
    #[must_use]
    pub fn device_view_state(&self) -> device_ids::DeviceViewState {
        let doc = self.editor.document();
        self.devices
            .state(doc, devices::shown_track(&self.arrangement, doc))
    }

    /// Every floating device's window, on any track (WP-2.5b), for the
    /// app to open, close, and place.
    #[must_use]
    pub fn floating_windows(&self) -> device_ids::FloatingWindows {
        let doc = self.editor.document();
        self.devices.floating_windows(
            doc,
            devices::shown_track(&self.arrangement, doc),
            self.prefs.auto_hide_plug_in_windows.0,
            self.plug_in_windows,
        )
    }

    /// Whether any device floats, on any track.
    fn any_floating(&self) -> bool {
        let doc = self.editor.document();
        doc.track_order()
            .into_iter()
            .filter_map(|t| doc.track(t))
            .any(|t| t.devices().any(|d| d.floating))
    }

    /// Whether any window View ▸ Plug-In Windows shows and hides: a
    /// floating device's, or a plug-in's editor (R33).
    fn any_plug_in_window(&self) -> bool {
        self.any_floating()
            || !self.plugins.open_editors().is_empty()
            || !self.au.open_editors().is_empty()
    }

    /// Hides and shows the open plug-in editor windows to follow View ▸
    /// Plug-In Windows and Auto-Hide Plug-In Windows, as their tracks'
    /// floating windows do (R31, R33): a hidden editor stays open.
    fn apply_plug_in_windows(&mut self) {
        let doc = self.editor.document();
        let shown_track = devices::shown_track(&self.arrangement, doc);
        let auto_hide = self.prefs.auto_hide_plug_in_windows.0;
        let plug_in_windows = self.plug_in_windows;
        let shown =
            |device| plug_in_windows && (!auto_hide || doc.device_track(device) == shown_track);
        for device in self.plugins.open_editors() {
            let _ = self.plugins.set_editor_visible(device, shown(device));
        }
        for device in self.au.open_editors() {
            let _ = self.au.set_editor_visible(device, shown(device));
        }
    }

    /// Whether Auto-Hide Plug-In Windows is on: a floating device's window
    /// or a plug-in's editor hides while another track is selected
    /// (WP-2.5b).
    #[must_use]
    pub fn auto_hide_plug_in_windows(&self) -> bool {
        self.prefs.auto_hide_plug_in_windows.0
    }

    /// Runs a Device View command and carries out what it asks of the rest
    /// of the shell.
    fn run_devices(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if id == device_ids::LOAD_SAMPLE {
            return self.load_sample(id, args);
        }
        if matches!(
            id,
            device_ids::CUT | device_ids::COPY | device_ids::DUPLICATE
        ) {
            // Spec: clap-hosting R8. A copy of a plug-in device starts
            // from its plug-in's state now.
            crate::plugins::capture_states(
                &self.plugins,
                &self.au,
                &self.vst3,
                &self.kit,
                &mut self.editor,
            );
        }
        let user_library = self.browser_view.user_library().map(Path::to_path_buf);
        let zoom = self.zoom();
        let auto_hide = self.prefs.auto_hide_plug_in_windows.0;
        let result = self.devices.run(
            id,
            args,
            devices::Context {
                editor: &mut self.editor,
                arrangement: &mut self.arrangement,
                user_library: user_library.as_deref(),
                zoom,
                auto_hide,
                windows_shown: self.plug_in_windows,
            },
        );
        let outcome = result.map_err(|error| device_error(id, error))?;
        if outcome.document_changed {
            self.dirty = true;
            self.document_changed();
        }
        if let Some(message) = outcome.message {
            self.set_message(message, false);
        }
        if let Some(route) = outcome.browse {
            // Spec: device-chains R19. Hot-swapping shows the device's
            // presets in the browser.
            self.browser = true;
            self.run_browser(browser_ids::OPEN_PLACE, json!({ "label": "user_library" }))
                .ok();
            for folder in route {
                self.run_browser(
                    browser_ids::OPEN_FOLDER,
                    json!({ "path": folder, "open": true, "keep_others": true }),
                )
                .ok();
            }
        }
        self.touch();
        Ok(outcome.value)
    }

    /// Loads a file into a device that plays one sample
    /// (`device.load_sample`, WP-2.16): the media store takes the file, the
    /// set records where it is and the device names it by content hash, and
    /// the engine plays it. One undo step.
    fn load_sample(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let device_ids::LoadSampleArgs { device, path } = Self::args(id, args)?;
        let device =
            device
                .or_else(|| self.devices.selected())
                .ok_or_else(|| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "select a device first".to_owned(),
                })?;
        let kind = self
            .editor
            .document()
            .device(device)
            .map(|device| device.kind.clone())
            .ok_or_else(|| ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("no {device}"),
            })?;
        // Only the sampler plays a sample (the sampler spec, R11).
        let sampler = bl_model::DeviceKind::Native {
            device_type: bl_devices::sampler::SAMPLER.to_owned(),
        };
        if kind != sampler {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!("{device} doesn't play a sample"),
            });
        }
        let path = PathBuf::from(path);
        let hash = self
            .media
            .import(&path)
            .map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: format!("{}: {error}", path.display()),
            })?;
        let entry = self.media.entry(hash).ok_or_else(|| ShellError::Failed {
            command: id.to_owned(),
            message: "the media store lost the file".to_owned(),
        })?;
        // The set records where the file is, so it reopens playing it
        // (WP-2.25's media table).
        let media = bl_model::ContentHash(*hash.as_bytes());
        self.store.add_media(media, &entry.path, entry.size);
        self.editor
            .apply(bl_model::Command::SetDeviceSample {
                device,
                media: Some(media),
            })
            .map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: error.to_string(),
            })?;
        self.dirty = true;
        self.document_changed();
        self.touch();
        let name = entry
            .path
            .file_name()
            .map_or(String::new(), |name| name.to_string_lossy().into_owned());
        Ok(to_value(&device_ids::LoadSampleResult {
            device,
            media,
            name,
        }))
    }

    /// Runs a track command that needs the shell: Duplicate Track Without
    /// Clips (WP-2.19b, IMP-12), Group Tracks (T24), or Ungroup Tracks (T24).
    fn run_tracks(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            track_ids::DUPLICATE_WITHOUT_CLIPS => {
                let track_ids::DuplicateTracksArgs { tracks } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let copies = crate::tracks::duplicate_without_clips(
                    &mut self.editor,
                    &tracks,
                    track_ids::DUPLICATE_WITHOUT_CLIPS_STEP,
                )
                .map_err(|reason| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                })?;
                self.arrangement
                    .run(
                        view_ids::SELECT_TRACKS,
                        json!({ "tracks": copies }),
                        self.editor.document(),
                    )
                    .ok();
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksDuplicated { tracks: copies }))
            }
            track_ids::GROUP => {
                let track_ids::GroupTracksArgs { tracks } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let group_id =
                    crate::tracks::group_tracks(&mut self.editor, &tracks, track_ids::GROUP_STEP)
                        .map_err(|reason| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason,
                    })?;
                self.arrangement.unfold(group_id);
                self.arrangement
                    .run(
                        view_ids::SELECT_TRACKS,
                        json!({ "tracks": [group_id] }),
                        self.editor.document(),
                    )
                    .ok();
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksGrouped { track: group_id }))
            }
            track_ids::UNGROUP => {
                let track_ids::UngroupTracksArgs { track } = Self::args(id, args)?;
                let target = track
                    .or_else(|| self.arrangement.selected_tracks().first().copied())
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "no track is selected".to_owned(),
                    })?;
                let children = crate::tracks::ungroup_tracks(
                    &mut self.editor,
                    target,
                    track_ids::UNGROUP_STEP,
                )
                .map_err(|reason| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                })?;
                if let Some(&first) = children.first() {
                    self.arrangement
                        .run(
                            view_ids::SELECT_TRACKS,
                            json!({ "tracks": [first] }),
                            self.editor.document(),
                        )
                        .ok();
                }
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&track_ids::TracksUngrouped { tracks: children }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// The browser's selection, if it's one preset file and a device is
    /// being hot-swapped: what Enter or a double-click loads into it.
    fn hot_swap_preset(&self) -> Option<PathBuf> {
        self.devices.hot_swap()?;
        match self.browser_view.selected() {
            [one] if bl_project::preset::is_preset(one) => Some(one.clone()),
            _ => None,
        }
    }

    /// What the Arrangement View shows (WP-1.8a).
    #[must_use]
    pub fn arrangement(&self) -> &ArrangementView {
        &self.arrangement
    }

    /// What the Clip View shows (WP-2.3).
    #[must_use]
    pub fn clip_view(&self) -> &ClipView {
        &self.clip_view
    }

    /// What the mixers show (WP-1.9).
    #[must_use]
    pub fn mixer_view(&self) -> &MixerView {
        &self.mixer
    }

    /// What the Session View grid has selected and how far it's scrolled
    /// (WP-3.7).
    #[must_use]
    pub fn session(&self) -> &SessionView {
        &self.session
    }

    /// The mixer of the view in front.
    #[must_use]
    pub fn front_mixer(&self) -> MixerOf {
        match self.main_view {
            MainView::Arrangement => MixerOf::Arrangement,
            MainView::Session => MixerOf::Session,
        }
    }

    /// The tracks with a column in the mixers, left to right (the mixer
    /// view spec, R1).
    #[must_use]
    pub fn mixer_columns(&self) -> Vec<TrackId> {
        crate::mixer_view::columns(self.editor.document(), &self.arrangement)
    }

    /// The meters the engine last published, as [`Shell::poll_engine`] took
    /// them: one per track, in the order the tracks run.
    #[must_use]
    pub fn meters(&self) -> &[bl_engine::TrackMeter] {
        &self.meters
    }

    /// One track's meter, if the engine published one.
    #[must_use]
    pub fn meter(&self, track: TrackId) -> Option<&bl_engine::TrackMeter> {
        self.meters.iter().find(|meter| meter.track == track)
    }

    /// Sets the meters the windows draw, as [`Shell::poll_engine`] does with
    /// the engine's: for benchmarks and tests with no engine running (while
    /// one runs, the next poll takes its meters instead).
    pub fn set_meters(&mut self, meters: &[bl_engine::TrackMeter]) {
        self.meters.clear();
        self.meters.extend_from_slice(meters);
    }

    /// Records the size of the Arrangement's scrolling lanes, as the main
    /// window lays them out, so its view commands (zooming around the
    /// middle, Optimize Height and Width, Follow) work with what's on
    /// screen.
    pub fn set_arrangement_viewport(&mut self, width: f32, height: f32) {
        if self
            .arrangement
            .set_viewport(Viewport { width, height }, self.editor.document())
        {
            self.touch();
        }
    }

    /// Takes the transport's latest state (the playhead, whether it plays,
    /// the loop brace), for the Arrangement View to draw and, with Follow
    /// on, to keep in view. [`Shell::poll_engine`] calls it with the
    /// engine's.
    pub fn set_transport(&mut self, transport: TransportView) {
        if self.arrangement.set_transport(transport) {
            self.touch();
        }
    }

    /// The browser (WP-1.10).
    #[must_use]
    pub fn browser_view(&self) -> &BrowserView {
        &self.browser_view
    }

    /// The browser's preview as the audio thread last reported it, for the
    /// preview strip.
    #[must_use]
    pub fn browser_preview(&self) -> browser_ids::PreviewState {
        self.browser_view.preview_state(&self.audio)
    }

    /// The media the set's clips play: the browser registers the files it
    /// adds, and the Arrangement's peak cache reads them.
    #[must_use]
    pub fn media_store(&self) -> Arc<MediaStore> {
        Arc::clone(&self.media)
    }

    /// Makes background work (the browser's index and preview decoder) call
    /// `wake` when it has news, so the host runs [`Shell::poll`].
    pub fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.browser_view.waker().set(wake);
    }

    /// Uses `path` as the browser's User Library, as the app does at
    /// start-up with the preference or the default place (browser R3).
    pub fn set_user_library(&mut self, path: Option<PathBuf>) {
        self.browser_view.set_user_library(path);
        self.touch();
    }

    /// Takes background work's news: the browser's index, a decoded
    /// preview, a preview that finished. The host calls it every frame and
    /// when woken; a failure shows in the Status Bar. Returns whether
    /// anything a window shows changed.
    pub fn poll(&mut self) -> bool {
        let polled = self.browser_view.poll(&mut self.audio);
        if let Some(error) = polled.error {
            self.set_message(error, true);
        }
        let mut changed = polled.changed;
        // Plug-ins' main-thread work (WP-2.6 to WP-2.8), and the changes
        // they made themselves.
        let plugins_changed = self.plugins.idle() | self.au.idle() | self.vst3.idle();
        if plugins_changed {
            self.refresh_device_types();
            changed = true;
        }
        // A plug-in that changed its latency: the engine recompiles with
        // it, so delay compensation follows it (WP-2.7, WP-2.11).
        for (device, frames) in self.vst3.take_latency_changes() {
            self.audio.set_device_latency(device, frames);
            changed = true;
        }
        // The open plug-in editors follow View ▸ Plug-In Windows and
        // Auto-Hide Plug-In Windows, as the tracks' floating windows do.
        self.apply_plug_in_windows();
        if self.kit.idle() {
            changed = true;
        }
        let generation = self.plugins.generation();
        if plugins_changed || generation != self.plugins_generation {
            // A plug-in that started may list commands.
            self.plugins_generation = generation;
            self.refresh_device_commands(false);
            changed = true;
        }
        let changes: Vec<(bl_model::DeviceId, bl_model::ParamId, f64)> = self
            .plugins
            .take_param_changes()
            .into_iter()
            .map(|c| (c.device, c.param, c.value))
            .chain(
                self.au
                    .take_param_changes()
                    .into_iter()
                    .map(|c| (c.device, c.param, c.value)),
            )
            .chain(
                self.vst3
                    .take_param_changes()
                    .into_iter()
                    .map(|c| (c.device, c.param, c.value)),
            )
            .chain(
                self.kit
                    .take_param_changes()
                    .into_iter()
                    .map(|c| (c.device, c.param, c.value)),
            )
            .collect();
        if crate::plugins::apply_param_changes(changes, &mut self.editor, &self.devices) {
            self.dirty = true;
            self.document_changed();
            changed = true;
        }
        if changed {
            self.touch();
        }
        changed
    }

    /// Tells the browser the set's file changed, so Current Project follows
    /// it (browser R4). A set that isn't a file on disk (one a test or the
    /// benchmark holds in memory) has no project folder.
    fn set_file_changed(&mut self) {
        let folder = self
            .set_path
            .as_deref()
            .filter(|path| path.is_file())
            .and_then(|path| std::path::absolute(path).ok())
            .and_then(|path| path.parent().map(Path::to_path_buf));
        self.browser_view.set_project(folder);
    }

    /// Where a default set goes when no file is named: the saved default
    /// set's file, the User Library's `Templates` folder, or beside the
    /// app's preferences (WP-2.25, file-management R2).
    fn default_set_file(&self) -> Option<PathBuf> {
        if let Some(path) = &self.prefs.default_set {
            return Some(path.clone());
        }
        self.templates_folder()
            .or_else(|| self.config_dir.clone())
            .map(|folder| folder.join(format!("Default Set.{}", bl_project::SET_EXTENSION)))
    }

    /// The file `path` names for a default set, with the `.blset` extension
    /// added when it's missing (WP-2.25).
    fn default_set_file_at(&self, path: &Path) -> PathBuf {
        if bl_project::is_set(path) {
            return path.to_owned();
        }
        let mut name = path.as_os_str().to_owned();
        name.push(".");
        name.push(bl_project::SET_EXTENSION);
        PathBuf::from(name)
    }

    /// The User Library's `Templates` folder, where templates are saved
    /// (WP-2.25, file-management R3).
    fn templates_folder(&self) -> Option<PathBuf> {
        self.browser_view
            .user_library()
            .map(|library| library.join("Templates"))
    }

    /// Searches for the open set's missing or changed files and relinks them
    /// (WP-2.25, file-management R6): the set's project folder, the User
    /// Library, the folders the files were last in, and `extra`, each with
    /// its subfolders. Returns how many were relinked, and re-points the
    /// set's media table at them.
    fn relink_missing(&mut self, extra: &[PathBuf]) -> usize {
        let mut folders: Vec<PathBuf> = Vec::new();
        if let Some(folder) = self.set_path.as_deref().and_then(Path::parent) {
            folders.push(folder.to_path_buf());
        }
        if let Some(library) = self.browser_view.user_library() {
            folders.push(library.to_path_buf());
        }
        for entry in self.media.entries() {
            if let Some(folder) = entry.path.parent() {
                folders.push(folder.to_path_buf());
            }
        }
        folders.extend(extra.iter().cloned());
        // Keep each folder once, in the order the search should try them.
        let mut seen = std::collections::HashSet::new();
        folders.retain(|folder| seen.insert(folder.clone()));
        let Ok(found) = self.media.relink_missing(&folders) else {
            return 0;
        };
        let found: Vec<(bl_model::ContentHash, PathBuf)> = found
            .into_iter()
            .map(|(hash, path)| (bl_model::ContentHash(*hash.as_bytes()), path))
            .collect();
        self.store.relink(&found);
        found.len()
    }

    /// Runs a browser command and carries out what it asks of the rest of
    /// the shell.
    fn run_browser(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let context = browser::Context {
            insert_marker: self.arrangement.insert_marker(),
            selected_track: self.arrangement.selected_tracks().first().copied(),
            editor: &mut self.editor,
            audio: &mut self.audio,
            media: &self.media,
            edge_fades: self.prefs.create_fades_on_clip_edges.0,
        };
        let outcome = self
            .browser_view
            .run(id, args, context)
            .map_err(|error| match error {
                BrowserError::InvalidArgs(message) => ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message,
                },
                BrowserError::Unavailable(reason) => ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                },
                BrowserError::Failed(message) => ShellError::Failed {
                    command: id.to_owned(),
                    message,
                },
            })?;
        for file in &outcome.media {
            self.store.add_media(file.hash, &file.path, file.size);
        }
        if outcome.document_changed {
            self.dirty = true;
            self.document_changed();
        }
        if outcome.prefs_changed {
            self.prefs.browser = self
                .browser_view
                .prefs(self.prefs.browser.user_library.clone());
            self.save_preferences();
        }
        if outcome.show_browser {
            self.browser = true;
        }
        if outcome.focus {
            self.focused_window = WindowKind::Main;
            self.focused_area = Area::Browser;
            self.effects.push(Effect::FocusArea(Area::Browser));
        }
        if id != browser_ids::STATE {
            self.touch();
        }
        Ok(outcome.value)
    }

    /// The Arrangement's clipboard, if anything has been copied.
    #[must_use]
    pub fn clipboard(&self) -> Option<&Clipboard> {
        self.clipboard.as_ref()
    }

    /// Whether a gesture that edits the set is under way.
    #[must_use]
    pub fn gesture_open(&self) -> bool {
        self.edit_gesture.is_some()
    }

    /// Whether the person's hand is on the set: a gesture that edits it
    /// (a drag of clips, [`Shell::gesture`]) or one that drags a value
    /// ([`Shell::begin_gesture`]) is under way. Its edits become one undo
    /// step when it ends, so an agent's edits wait until then (WP-1.15b).
    #[must_use]
    pub fn gesture_under_way(&self) -> bool {
        self.edit_gesture.is_some() || self.gesture
    }

    /// Runs a command as [`Shell::run`] does, recording what it changes in
    /// the set as one undo step named `label`, such as an agent's edit
    /// labelled with the agent's name. A command that changes nothing in
    /// the set records nothing. Undo and redo can't be labelled, nor can a
    /// command that replaces the set (opening or making one).
    ///
    /// # Errors
    ///
    /// As [`Shell::run`]; the set is unchanged.
    pub fn run_labelled(
        &mut self,
        label: impl Into<std::borrow::Cow<'static, str>>,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let labelled = !matches!(id, model::UNDO | model::REDO)
            && (model::handles(id)
                || edit_ids::handles(id)
                || device_ids::handles(id)
                || self.is_device_command(id)
                || track_ids::handles(id)
                || note_ids::handles(id)
                || midi_ids::handles(id)
                || matches!(
                    id,
                    browser_ids::ADD_TO_ARRANGEMENT | browser_ids::LOAD_SELECTED
                ));
        if !labelled || self.edit_gesture.is_some() {
            return self.run(id, args);
        }
        self.editor.begin(label);
        let result = self.run(id, args);
        if result.is_ok() {
            if self.editor.commit().unwrap_or(false) {
                self.dirty = true;
            }
        } else {
            self.editor.cancel().ok();
        }
        self.touch();
        result
    }

    /// What calling `id` with `args` needs, decided from its arguments
    /// against the set as it is now: its registered class, and destructive
    /// as well when it would delete or shorten clips it doesn't act on, as
    /// moving, resizing, nudging, duplicating, or pasting clips over others,
    /// or dropping files on them, does (WP-1.15b). Nothing changes. An
    /// unknown command is external, the most a command can need.
    pub fn call_permission(&mut self, id: &str, args: &Value) -> CallPermission {
        let registered = self
            .registry
            .get(id)
            .and_then(|spec| spec.permission)
            .unwrap_or(bl_commands::Permission::External);
        crate::permission::assess(
            id,
            args,
            registered,
            crate::permission::Scene {
                insert_marker: self.arrangement.insert_marker(),
                selected_track: self.arrangement.selected_tracks().first().copied(),
                editor: &mut self.editor,
                view: &self.arrangement,
                clipboard: &self.clipboard,
            },
        )
    }

    /// Runs one step of a gesture that edits the set, such as dragging
    /// clips or notes (WP-1.8b; arrangement-editing R13, piano-roll R11). A
    /// gesture is one undo step however many frames it lasts: its first
    /// update opens a transaction, each update runs `invocation`, an editing
    /// command with arguments measured from where the gesture began, on the
    /// set as it was then, and the commit closes the transaction. The set
    /// follows the gesture as it goes, so the view and the engine show it. A
    /// frame whose command is refused (a clip pushed before 1.1.1, a note
    /// past key 127) keeps the last one that worked. Cancelling puts the set
    /// and the selection back.
    ///
    /// # Errors
    ///
    /// [`ShellError`] if the command isn't an editing command, or the
    /// frame's command was refused; the gesture stays open either way
    /// until it's committed or cancelled.
    pub fn gesture(
        &mut self,
        phase: GesturePhase,
        invocation: Option<&Invocation>,
    ) -> Result<Value, ShellError> {
        if phase == GesturePhase::Cancel {
            if let Some(gesture) = self.edit_gesture.take() {
                self.editor.cancel().ok();
                self.arrangement.restore_selection(&gesture.view);
                self.clip_view = gesture.clip_view;
                self.document_changed();
                self.touch();
            }
            return Ok(Value::Null);
        }
        let mut outcome = Ok(Value::Null);
        if let Some(invocation) = invocation {
            if !edit_ids::handles(&invocation.id)
                && !note_ids::handles(&invocation.id)
                && !is_automation_edit(&invocation.id)
                && !clip_marker_command(&invocation.id)
            {
                return Err(ShellError::Unavailable {
                    command: invocation.id.clone(),
                    reason: "only the Arrangement's and the Clip View's editing commands run as \
                             gestures"
                        .to_owned(),
                });
            }
            outcome = self.gesture_step(invocation);
        }
        if phase == GesturePhase::Commit
            && self.edit_gesture.take().is_some()
            && self.editor.commit().unwrap_or(false)
        {
            self.dirty = true;
        }
        self.document_changed();
        self.touch();
        outcome
    }

    fn gesture_step(&mut self, invocation: &Invocation) -> Result<Value, ShellError> {
        let label = if note_ids::handles(&invocation.id) {
            clip_view::label(&invocation.id)
        } else if is_automation_edit(&invocation.id) {
            automation_label(&invocation.id)
        } else if clip_marker_command(&invocation.id) {
            marker_label(&invocation.id)
        } else {
            editing::label(&invocation.id, &invocation.args)
        };
        match &self.edit_gesture {
            None => {
                self.edit_gesture = Some(Gesture {
                    view: self.arrangement.clone(),
                    clip_view: self.clip_view.clone(),
                    last: None,
                });
            }
            Some(gesture) => {
                // Back to where the gesture began, then the whole gesture
                // again.
                self.editor.cancel().ok();
                let view = gesture.view.clone();
                self.arrangement.restore_selection(&view);
                self.clip_view = gesture.clip_view.clone();
            }
        }
        self.editor.begin(label);
        let run = |editor: &mut Editor,
                   arrangement: &mut ArrangementView,
                   clip_view: &mut ClipView,
                   clipboard: &mut Option<Clipboard>,
                   invocation: &Invocation| {
            if clip_marker_command(&invocation.id) {
                // The document's own clip commands, run per frame like the
                // Clip View's note commands.
                model::run(editor, &invocation.id, invocation.args.clone())
                    .map(|changes| to_value(&changes))
                    .map_err(|error| EditFailure::Refused(error.to_string()))
            } else if note_ids::handles(&invocation.id) {
                clip_view::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    clip_view,
                    arrangement,
                )
            } else if is_automation_edit(&invocation.id) {
                // WP-2.12: a drag on an automation lane moves, curves, or
                // deletes breakpoints, as one undo step.
                model::run(editor, &invocation.id, invocation.args.clone())
                    .map(|changes| to_value(&changes))
                    .map_err(|error| {
                        crate::arrangement::editing::EditFailure::Refused(error.to_string())
                    })
            } else {
                editing::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    arrangement,
                    clipboard,
                )
            }
        };
        let result = run(
            &mut self.editor,
            &mut self.arrangement,
            &mut self.clip_view,
            &mut self.clipboard,
            invocation,
        );
        let Some(gesture) = self.edit_gesture.as_mut() else {
            return Ok(Value::Null);
        };
        match result {
            Ok(value) => {
                gesture.last = Some(invocation.clone());
                Ok(value)
            }
            Err(failure) => {
                if let Some(last) = gesture.last.clone() {
                    let _ = run(
                        &mut self.editor,
                        &mut self.arrangement,
                        &mut self.clip_view,
                        &mut self.clipboard,
                        &last,
                    );
                }
                Err(edit_error(&invocation.id, failure))
            }
        }
    }

    /// Takes what the engine last published (the playhead, whether the song
    /// plays, the insert marker) for the Arrangement View and the Control
    /// Bar, with the loop brace from the document. The window calls it
    /// before drawing each frame, so what it draws lags the engine by at
    /// most a frame and an audio block (the control bar transport spec,
    /// R14). Returns the transport it took.
    pub fn poll_engine(&mut self) -> EngineTransport {
        let engine = self.audio.transport_state();
        // WP-2.13. A recording's take reaches the shell with the engine's
        // state; the clips are made from it once the writer has finished.
        self.poll_recording(&engine);
        self.poll_automation_record(&engine);
        // Spec: mixer-view R10. The meters the strips and headers draw: the
        // engine's while one runs, and none once it stops. Without an
        // engine, meters set with `set_meters` stay.
        let running = self.audio.engine_running();
        if running || self.metering {
            self.metering = running;
            // Not a revision: the window redraws while a meter shows a
            // level, and nothing else depends on them.
            self.audio.meters_into(&mut self.meters);
        }
        // Spec: launch-quantisation R10. The slots the engine is launching
        // or stopping, which the grid's launch buttons draw.
        self.audio.session_into(&mut self.session_states);
        let brace = self.editor.document().settings().arrangement_loop;
        self.set_transport(TransportView {
            playing: engine.playing,
            position: engine.position,
            loop_start: brace.start,
            loop_length: brace.length,
            loop_on: brace.enabled,
        });
        // Spec: control-bar-transport R15. A move of the engine's insert
        // marker (the position fields, Home, a second Stop, an agent)
        // reaches the view once.
        if self.seen_insert_marker != Some(engine.insert_marker) {
            self.seen_insert_marker = Some(engine.insert_marker);
            if self.arrangement.move_insert_marker(engine.insert_marker) {
                self.touch();
            }
        }
        engine
    }

    /// Whether the song is playing, as the shell last heard from the
    /// engine.
    #[must_use]
    pub fn playing(&self) -> bool {
        self.arrangement.transport().playing
    }

    /// The global launch quantisation and its enabled switch (WP-3.2), which
    /// the Control Bar's Quantization Menu shows and the Options items
    /// change. It's saved with the set.
    #[must_use]
    pub fn launch_settings(&self) -> bl_model::LaunchSettings {
        self.store.launch()
    }

    /// The Session slots' states the engine last published (WP-3.2), which
    /// the grid's launch buttons draw: one entry per queued, playing, or
    /// stopping slot, and nothing once every slot has stopped.
    #[must_use]
    pub fn session_states(&self) -> &[bl_engine::SlotStatus] {
        &self.session_states
    }

    /// Starts a recording (WP-2.13): a ring for every armed audio track
    /// whose Audio From is Ext. In, and a writer thread draining them into
    /// the project's `Samples/Recorded/`. The engine is given the rings
    /// before the transport is told it records.
    fn begin_recording(&mut self) -> Result<(), String> {
        let Some(set) = self.set_path.clone() else {
            return Err(
                "the set has never been saved; use `set.save_as` to give it a project folder, \
                 where its recordings go"
                    .to_owned(),
            );
        };
        let rate = self
            .audio
            .sample_rate()
            .and_then(SampleRate::new)
            .ok_or("no audio device is open")?;
        let folder = ProjectFolder::of_set(&set).root().to_path_buf();
        let stamp = crate::recording::stamp();
        self.recorded_takes += 1;
        let take = self.recorded_takes;
        let document = self.editor.document();
        let mut tracks = Vec::new();
        for id in document.track_order() {
            let Some(track) = document.track(id) else {
                continue;
            };
            if track.kind() != TrackKind::Audio || !track.mixer().arm {
                continue;
            }
            let Input::External { channel } = track.routing().input else {
                continue;
            };
            tracks.push(crate::recording::Track {
                track: id,
                channel,
                name: crate::recording::file_name(
                    &crate::routing::track_label(document, id),
                    take,
                    &stamp,
                ),
            });
        }
        let (plan, recording) = crate::recording::start(&folder, rate, &tracks)?;
        self.audio.record(plan)?;
        self.recording = Some(recording);
        Ok(())
    }

    /// Follows the recording under way (WP-2.13): notes the take the engine
    /// published, and, once the writer has finished draining the rings,
    /// makes its clips. Called with every engine poll, so the clips appear
    /// a frame or two after the take ends.
    fn poll_recording(&mut self, engine: &EngineTransport) {
        let Some(recording) = self.recording.as_mut() else {
            return;
        };
        if let Some(take) = engine.take {
            recording.note_take(take);
        }
        if let Some(outcome) = recording.poll(engine.running) {
            self.recording = None;
            self.finish_recording(outcome);
        }
    }

    /// Writes the control moves of an automation recording into the set
    /// when the transport stops recording (automation R17).
    fn poll_automation_record(&mut self, engine: &EngineTransport) {
        let recording = engine.recording && engine.playing;
        let was = std::mem::replace(&mut self.automation_recording, recording);
        if recording || !was {
            return;
        }
        let take = std::mem::take(&mut self.automation_take);
        match take.commit(&mut self.editor, engine.position) {
            Ok(true) => {
                self.dirty = true;
                self.document_changed();
                self.touch();
            }
            Ok(false) => {}
            Err(error) => self.set_message(format!("Automation recording failed: {error}"), true),
        }
    }

    /// Makes the clips of a finished recording, one undo step (WP-2.13):
    /// each file becomes an audio clip on its track, from the song frame
    /// the take started on to the one it ended on. Undo removes the clips;
    /// the files stay, as Live keeps recorded files. A report the writer
    /// sent (a failed file) makes no clips and shows in the Status Bar, and
    /// frames the engine dropped because the disk fell behind are reported
    /// there too.
    fn finish_recording(&mut self, outcome: crate::recording::Outcome) {
        let crate::recording::Outcome {
            take,
            files,
            dropped,
        } = outcome;
        let files = match files {
            Ok(files) => files,
            Err(error) => {
                self.set_message(format!("Recording failed: {error}"), true);
                return;
            }
        };
        if files.is_empty() {
            return;
        }
        let Some(take) = take else {
            self.set_message(
                "Recording stopped without a position, so its clips weren't made",
                true,
            );
            return;
        };
        let Some(rate) = self.audio.sample_rate().and_then(SampleRate::new) else {
            self.set_message("Recording stopped: the audio device closed", true);
            return;
        };
        let tempo = self.editor.document().settings().tempo.clone();
        let start = tempo.frames_to_ticks(take.start, rate);
        let end = tempo.frames_to_ticks(take.end, rate);
        let edge_fades = self.create_fades_on_clip_edges();
        let mut imported = Vec::with_capacity(files.len());
        for file in &files {
            match self.media.import(&file.path) {
                Ok(hash) => {
                    let size = std::fs::metadata(&file.path).map_or(0, |data| data.len());
                    imported.push((
                        file.track,
                        ContentHash(*hash.as_bytes()),
                        file.path.clone(),
                        size,
                    ));
                }
                Err(error) => {
                    self.set_message(
                        format!("Recording failed: {}: {error}", file.path.display()),
                        true,
                    );
                    return;
                }
            }
        }
        let mut placed = std::collections::BTreeSet::new();
        let applied = self
            .editor
            .transact(crate::recording::UNDO_LABEL, |editor| {
                for (track, hash, path, _) in &imported {
                    let id = editor.new_id();
                    // Spec: audio-clips R7. With Create Fades on Clip Edges on,
                    // a new recorded clip's edges fade over 4 ms, like a
                    // dropped file's.
                    let audio = AudioData::new(*hash);
                    let audio = if edge_fades {
                        audio.with_edge_fades()
                    } else {
                        audio
                    };
                    let mut clip =
                        Clip::new(id, start, end.saturating_sub(start), ClipData::Audio(audio));
                    clip.name = path
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    crate::arrangement::placing::clear(editor, *track, start, end, &placed)?;
                    placed.insert(id);
                    editor.apply(bl_model::Command::CreateClip {
                        track: *track,
                        clip,
                    })?;
                }
                Ok::<(), bl_model::EditError>(())
            });
        if let Err(error) = applied {
            self.set_message(format!("Recording failed: {error}"), true);
            return;
        }
        for (_, hash, path, size) in imported {
            self.store.add_media(hash, &path, size);
        }
        self.dirty = true;
        self.document_changed();
        self.touch();
        if dropped > 0 {
            self.set_message(
                format!(
                    "Recording ended early: {dropped} frames were dropped because the disk \
                     couldn't keep up"
                ),
                true,
            );
        }
    }

    /// Opens a gesture, such as a drag on a value field: the document
    /// commands it runs until [`Shell::end_gesture`] are one undo step,
    /// named `label` (such as "Change tempo"). A gesture already open
    /// carries on.
    pub fn begin_gesture(&mut self, label: &str) {
        if !self.gesture {
            self.editor.begin(label.to_owned());
            self.gesture = true;
        }
    }

    /// Closes the open gesture, recording its changes as one undo step.
    pub fn end_gesture(&mut self) {
        if self.gesture {
            self.gesture = false;
            let _ = self.editor.commit();
            self.touch();
        }
        // The control a drag moved is let go (automation R17).
        self.automation_take
            .release(self.arrangement.transport().position);
    }

    /// The effects commands have asked for since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Writes the preferences to the config directory, if there is one. A
    /// failure shows in the Status Bar.
    pub fn save_preferences(&mut self) {
        let Some(dir) = &self.config_dir else {
            return;
        };
        if let Err(error) = self.prefs.save(dir) {
            self.set_message(error, true);
        }
    }

    /// The contexts that apply to a key press now, narrowest first: the
    /// focused area's, then `tab-focus` while Tab moves the focus, then
    /// `global`. In the Settings window: `settings`, `dialog`, `global`.
    #[must_use]
    pub fn active_contexts(&self) -> Vec<Context> {
        match self.focused_window {
            WindowKind::Settings => vec![Context::Settings, Context::Dialog, Context::Global],
            // A floating device's window: its device is selected, so the
            // Device View's keys act on it (Float's among them), and the
            // global ones work.
            WindowKind::Device(_) => vec![Context::DeviceView, Context::Global],
            WindowKind::Main => {
                let mut contexts = Vec::with_capacity(3);
                match self.focused_area {
                    Area::MainView => match self.main_view {
                        MainView::Arrangement => {
                            // A selected title bar's keys come first
                            // (shortcuts.md, `track-headers`).
                            if self.arrangement.title_bars_selected() {
                                contexts.push(Context::TrackHeaders);
                            }
                            contexts.push(Context::Arrangement);
                        }
                        MainView::Session => contexts.push(Context::Session),
                    },
                    Area::Browser => contexts.push(self.browser_view.focus().context()),
                    Area::Detail if self.value_focused => contexts.push(Context::ValueField),
                    // Spec: piano-roll. With the Clip View open, its MIDI
                    // Note Editor holds the detail area's keys.
                    Area::Detail if self.clip_view.visible() => {
                        contexts.push(Context::ClipView);
                        contexts.push(Context::NoteEditor);
                        if !self
                            .clip_view
                            .selected_notes(self.editor.document(), &self.arrangement)
                            .is_empty()
                        {
                            contexts.push(Context::NoteSelection);
                        }
                    }
                    Area::Detail => contexts.push(Context::DeviceView),
                    Area::ControlBar | Area::StatusBar => {}
                }
                if self.prefs.tab_navigation {
                    contexts.push(Context::TabFocus);
                }
                contexts.push(Context::Global);
                contexts
            }
        }
    }

    /// Whether a switch-like command is on now: the state a menu shows as a
    /// check mark. `None` for commands that aren't switches.
    #[must_use]
    pub fn switch_state(&self, id: &str) -> Option<bool> {
        match id {
            ids::VIEW_FULL_SCREEN => Some(self.full_screen),
            ids::VIEW_TOGGLE_BROWSER => Some(self.browser),
            ids::VIEW_TOGGLE_CLIP_VIEW => Some(self.clip_view.visible()),
            ids::VIEW_DRAW_MODE => Some(self.clip_view.draw_mode()),
            ids::VIEW_FOLD_NOTES => Some(
                self.clip_view
                    .shown_clip(self.editor.document(), &self.arrangement)
                    .is_some_and(|clip| self.clip_view.is_folded(clip)),
            ),
            ids::VIEW_TOGGLE_INFO_VIEW => Some(self.info_view),
            ids::VIEW_TOGGLE_PLUG_IN_WINDOWS => Some(self.plug_in_windows),
            ids::FOCUS_TOGGLE_TAB_NAVIGATION => Some(self.prefs.tab_navigation),
            ids::FOCUS_TOGGLE_WRAP_NAVIGATION => Some(self.prefs.wrap_navigation.0),
            session_launch_ids::SET_QUANTIZATION_ENABLED => Some(self.store.launch().enabled),
            SOLO_IN_PLACE => Some(self.editor.document().settings().solo_in_place),
            "transport.set_metronome_enabled" => {
                Some(self.editor.document().settings().metronome.enabled)
            }
            "transport.set_metronome_only_while_recording" => Some(
                self.editor
                    .document()
                    .settings()
                    .metronome
                    .only_while_recording,
            ),
            ids::AUDIO_TOGGLE_ENGINE => Some(self.audio.engine_on()),
            browser_ids::SET_PREVIEW => Some(self.browser_view.preview_on()),
            "device.set_show_preset_name" => self
                .devices
                .selected()
                .and_then(|d| self.editor.document().device(d))
                .map(|d| d.show_preset_name),
            // IMP-14: Float is ticked while the selected device floats.
            device_ids::SET_FLOATING => self
                .devices
                .selected()
                .and_then(|d| self.editor.document().device(d))
                .map(|d| d.floating),
            _ if mixer_ids::handles(id) => self.mixer.switch_state(id, self.front_mixer()),
            _ => self.arrangement.switch_state(id),
        }
    }

    /// Whether a submenu opens now: Arrangement Track Controls is disabled
    /// while the Session View is in front (observed in Live 12.4.6; the
    /// mixer view spec, R12).
    #[must_use]
    pub fn can_open_submenu(&self, row: &str) -> bool {
        !(row == "M-VIEW-033" && self.main_view == MainView::Session)
    }

    /// The label a menu shows for a command now: Undo and Redo name the step
    /// they act on, or say there's nothing (M-EDIT-001, M-EDIT-002).
    #[must_use]
    pub fn dynamic_label(&self, id: &str) -> Option<String> {
        match id {
            model::UNDO => Some(self.editor.undo_history().next().map_or_else(
                || "Nothing to Undo".to_owned(),
                |step| format!("Undo {step}"),
            )),
            model::REDO => Some(self.editor.redo_history().next().map_or_else(
                || "Nothing to Redo".to_owned(),
                |step| format!("Redo {step}"),
            )),
            // The Playback menu's first item toggles with Stop (M-PLAY-001).
            transport_ids::PLAY_STOP => {
                Some(if self.playing() { "Stop" } else { "Play" }.to_owned())
            }
            _ => None,
        }
    }

    /// Whether a person can run `id` with `args` now, from a menu or a key:
    /// it applies in an active context, the arguments the shell can fill in
    /// fit its schema, and nothing about the state rules it out.
    #[must_use]
    pub fn can_invoke(&self, id: &str, args: &Value) -> bool {
        let Some(spec) = self.registry.get(id) else {
            return false;
        };
        if self.edit_gesture.is_some() {
            // A drag is under way; keys wait for it (Esc cancels it).
            return false;
        }
        let active = self.active_contexts();
        if !spec.contexts.iter().any(|context| active.contains(context)) {
            return false;
        }
        match id {
            model::UNDO => return self.editor.can_undo(),
            model::REDO => return self.editor.can_redo(),
            ids::APP_HIDE => return cfg!(target_os = "macos"),
            ids::SET_OPEN | ids::SET_SAVE | ids::SET_SAVE_AS | ids::APP_SETTINGS => return true,
            ids::SET_DISCARD_CHANGES => return self.dirty,
            ids::FOCUS_NEXT_NEIGHBOR => return self.has_neighbor(false),
            ids::FOCUS_PREVIOUS_NEIGHBOR => return self.has_neighbor(true),
            // Zoom In and Zoom Out stop at the zoom's ends (interface-zoom
            // R3).
            ids::VIEW_ZOOM_IN => return self.zoom_percent() < ZOOM_MAX_PERCENT,
            ids::VIEW_ZOOM_OUT => return self.zoom_percent() > ZOOM_MIN_PERCENT,
            // A person's Zoom setting always names a percentage in range.
            ids::VIEW_SET_ZOOM => {
                return spec
                    .args
                    .as_ref()
                    .is_some_and(|schema| bl_commands::validate(schema, args).is_ok());
            }
            // The frontmost window must be a secondary one: with only the
            // main window open, Close Window is disabled (M-FILE-004).
            // Live's Plug-In Windows is disabled with none open (M-VIEW-028).
            ids::VIEW_TOGGLE_PLUG_IN_WINDOWS => return self.any_plug_in_window(),
            ids::WINDOW_CLOSE => {
                return match self.focused_window {
                    WindowKind::Settings => self.settings_open,
                    // Closing a floating device's window docks it.
                    WindowKind::Device(device) => self
                        .editor
                        .document()
                        .device(device)
                        .is_some_and(|d| d.floating),
                    WindowKind::Main => false,
                };
            }
            _ => {}
        }
        if ids::handles(id) || transport_ids::handles(id) {
            return true;
        }
        if id == browser_ids::LOAD_SELECTED && self.hot_swap_preset().is_some() {
            return true;
        }
        if device_ids::handles(id) {
            let args = Self::object(args.clone());
            let doc = self.editor.document();
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && self.devices.can_run(
                    id,
                    &args,
                    doc,
                    devices::shown_track(&self.arrangement, doc),
                );
        }
        if track_ids::handles(id) {
            match id {
                track_ids::DUPLICATE_WITHOUT_CLIPS => {
                    // Spec: routing R28. The selected tracks, or those named.
                    let named: Option<Vec<TrackId>> = args
                        .get("tracks")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let tracks =
                        named.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    return !crate::tracks::duplicated(self.editor.document(), &tracks).is_empty();
                }
                track_ids::GROUP => {
                    let named: Option<Vec<TrackId>> = args
                        .get("tracks")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let tracks =
                        named.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    return !crate::tracks::groupable(self.editor.document(), &tracks).is_empty();
                }
                track_ids::UNGROUP => {
                    let named: Option<TrackId> = args
                        .get("track")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let track =
                        named.or_else(|| self.arrangement.selected_tracks().first().copied());
                    return track
                        .and_then(|t| self.editor.document().track(t))
                        .is_some_and(|t| t.kind() == TrackKind::Group);
                }
                _ => return false,
            }
        }
        if id == "device.rename" && args.get("name").is_none() {
            // A person names the device in its title bar.
            return self.devices.selected().is_some();
        }
        if id == device_ids::SET_FLOATING {
            // IMP-14: only native devices float; plug-ins open their own
            // editor windows.
            let named = args
                .get("device")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let floats = named
                .or(self.devices.selected())
                .and_then(|d| self.editor.document().device(d))
                .is_some_and(bl_model::Device::can_float);
            if !floats {
                return false;
            }
        }
        if browser_ids::handles(id) {
            // A person adding a folder picks it in a dialog.
            if id == browser_ids::ADD_FOLDER && args.get("path").is_none() {
                return true;
            }
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && self.browser_view.can_run(id, &args);
        }
        if edit_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && editing::can_run(
                    id,
                    &args,
                    self.editor.document(),
                    &self.arrangement,
                    self.clipboard.as_ref(),
                );
        }
        if engine_transport::handles(id) {
            // The transport plays only while an engine runs on a device.
            return self.audio.engine_running()
                && engine_transport::command_from_args(id, Self::object(args.clone())).is_ok();
        }
        if engine_session::handles(id) {
            // The Session View launches only while an engine runs on a
            // device, as the transport plays.
            return self.audio.engine_running()
                && engine_session::command_from_args(id, Self::object(args.clone())).is_ok();
        }
        if session_launch_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && (id != session_launch_ids::STOP_SELECTED
                    || !self.session.selected_slots().is_empty());
        }
        if view_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && self.arrangement.can_run(id, &args, self.editor.document());
        }
        if mixer_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok());
        }
        let Ok(args) = self.filled_args(id, args.clone()) else {
            return false;
        };
        spec.args
            .as_ref()
            .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
    }

    /// The arguments a person's invocation implies: a new track's ID, name,
    /// and place, for the insert-track keys.
    fn filled_args(&self, id: &str, args: Value) -> Result<Value, ShellError> {
        let Value::Object(mut fields) = args else {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "the arguments must be a JSON object".to_owned(),
            });
        };
        if id == "track.create" && !fields.contains_key("id") {
            let kind: TrackKind = fields
                .get("kind")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or(TrackKind::Audio);
            let doc = self.editor.document();
            // Spec: app-shell R10. The new track goes directly after the
            // selected track, or after the last of several selected tracks,
            // in the list it belongs to; with no such track selected, after
            // the last track, as before.
            let anchor = self
                .arrangement
                .selected_tracks()
                .iter()
                .rev()
                .copied()
                .find(|&track| match doc.track(track) {
                    Some(track) if kind == TrackKind::Return => track.kind() == TrackKind::Return,
                    Some(track) => track.kind().can_be_grouped(),
                    None => false,
                });
            let (parent, index) = match anchor {
                Some(anchor) if kind == TrackKind::Return => (
                    None,
                    doc.returns()
                        .position(|r| r == anchor)
                        .map_or(doc.returns().len(), |i| i + 1),
                ),
                Some(anchor) => {
                    let parent = doc.parent(anchor);
                    let index = match parent {
                        Some(group) => {
                            let children: Vec<TrackId> =
                                doc.track(group).map_or_default(|g| g.children().collect());
                            children
                                .iter()
                                .position(|t| *t == anchor)
                                .map_or(children.len(), |i| i + 1)
                        }
                        None => doc
                            .top_level()
                            .position(|t| t == anchor)
                            .map_or(doc.top_level().len(), |i| i + 1),
                    };
                    (parent, index)
                }
                None => (
                    None,
                    if kind == TrackKind::Return {
                        doc.returns().len()
                    } else {
                        doc.top_level().len()
                    },
                ),
            };
            let name = if kind == TrackKind::Return {
                format!("{} Return", crate::routing::return_letter(index))
            } else {
                let word = match kind {
                    TrackKind::Midi => "MIDI",
                    TrackKind::Group => "Group",
                    _ => "Audio",
                };
                format!("{} {word}", index + 1)
            };
            // A placeholder that fits the schema; `invoke` issues the real ID.
            fields.insert("id".to_owned(), json!(1));
            if let Some(parent) = parent {
                fields.insert("parent".to_owned(), json!(parent));
            }
            fields.entry("name").or_insert_with(|| json!(name));
            fields.entry("index").or_insert_with(|| json!(index));
        }
        if device_ids::ON_SELECTED.contains(&id) && !fields.contains_key("device") {
            // Spec: device-chains R20. A key or menu item acts on the
            // selected device, switching it where it switches.
            let device = self
                .devices
                .selected()
                .ok_or_else(|| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "select a device first".to_owned(),
                })?;
            let current = self.editor.document().device(device);
            fields.insert("device".to_owned(), json!(device));
            if id == "device.set_show_preset_name" && !fields.contains_key("shown") {
                let on = current.is_some_and(|d| d.show_preset_name);
                fields.insert("shown".to_owned(), json!(!on));
            }
            if id == device_ids::SET_FLOATING && !fields.contains_key("floating") {
                let on = current.is_some_and(|d| d.floating);
                fields.insert("floating".to_owned(), json!(!on));
            }
            if id == "device.set_enabled" && !fields.contains_key("enabled") {
                let on = current.is_some_and(|d| d.enabled);
                fields.insert("enabled".to_owned(), json!(!on));
            }
        }
        if id == SOLO_IN_PLACE && !fields.contains_key("enabled") {
            // Spec: accessibility R9. Options ▸ Solo in Place switches the
            // setting to the other state.
            let on = self.editor.document().settings().solo_in_place;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == session_launch_ids::SET_QUANTIZATION_ENABLED && !fields.contains_key("enabled") {
            // Spec: launch-quantisation R3. Options ▸ Launch Quantization
            // Enabled switches the setting to the other state.
            let on = self.store.launch().enabled;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == "transport.set_metronome_enabled" && !fields.contains_key("enabled") {
            let on = self.editor.document().settings().metronome.enabled;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == "transport.set_metronome_only_while_recording"
            && !fields.contains_key("only_while_recording")
        {
            let on = self
                .editor
                .document()
                .settings()
                .metronome
                .only_while_recording;
            fields.insert("only_while_recording".to_owned(), json!(!on));
        }
        if let Some(switch) = Switch::of_command(id)
            && !fields.contains_key("track")
        {
            // Spec: mixer-view R9. A title-bar key, or F1 to F8, names no
            // track: it acts on the selected tracks, or the numbered one,
            // toggling the first one's switch.
            let tracks = self.key_tracks(&mut fields);
            let invocations = mixer_controls::toggled(self.editor.document(), switch, &tracks);
            let first = invocations.first().ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no selected track has this switch".to_owned(),
            })?;
            return Ok(first.args.clone());
        }
        Ok(Value::Object(fields))
    }

    /// The tracks a toggling mixer key acts on: the track numbered by
    /// `track_number` (F1 to F8, counting the mixer's columns from 1), which
    /// it takes out of `fields`, or the selected tracks.
    fn key_tracks(&self, fields: &mut serde_json::Map<String, Value>) -> Vec<TrackId> {
        match fields.remove(KEY_TRACK_NUMBER).and_then(|n| n.as_u64()) {
            Some(number) => self
                .mixer_columns()
                .get((number as usize).wrapping_sub(1))
                .copied()
                .into_iter()
                .collect(),
            None => self.arrangement.selected_tracks().to_vec(),
        }
    }

    /// Runs a toggling mixer key (S, 0, C, F1 to F8) on every track it acts
    /// on, as one undo step (the mixer view spec, R9).
    fn toggle_tracks(
        &mut self,
        id: &str,
        switch: Switch,
        args: Value,
    ) -> Result<Value, ShellError> {
        let Value::Object(mut fields) = args else {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "the arguments must be a JSON object".to_owned(),
            });
        };
        let tracks = self.key_tracks(&mut fields);
        let invocations = mixer_controls::toggled(self.editor.document(), switch, &tracks);
        if invocations.is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no selected track has this switch".to_owned(),
            });
        }
        let opened = !self.gesture;
        if opened {
            self.begin_gesture(switch.gesture());
        }
        let mut result = Ok(Value::Null);
        for invocation in invocations {
            result = self.run(&invocation.id, invocation.args);
            if result.is_err() {
                break;
            }
        }
        if opened {
            self.end_gesture();
        }
        result
    }

    /// Runs a command as a person asked for it, from a menu, a key, or a
    /// control: fills in arguments the person didn't give, asks before
    /// losing unsaved changes, and asks for a file where one is needed (as
    /// [`Effect`]s). Errors also show in the Status Bar.
    ///
    /// # Errors
    ///
    /// As [`Shell::run`], when the command runs straight away.
    pub fn invoke(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let result = self.invoke_inner(id, args);
        if let Err(error) = &result {
            self.set_message(error.to_string(), true);
        }
        result
    }

    fn invoke_inner(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let has = |key: &str| args.get(key).is_some();
        match id {
            ids::SET_NEW | ids::APP_QUIT if self.dirty => {
                self.effects.push(Effect::ConfirmDiscard {
                    then: Invocation::new(id, args),
                });
                Ok(Value::Null)
            }
            ids::SET_OPEN if !has("path") => {
                if self.dirty {
                    self.effects.push(Effect::ConfirmDiscard {
                        then: Invocation::new(id, args),
                    });
                } else {
                    self.effects.push(Effect::PickOpenPath {
                        discard_changes: false,
                    });
                }
                Ok(Value::Null)
            }
            ids::SET_OPEN if self.dirty => {
                self.effects.push(Effect::ConfirmDiscard {
                    then: Invocation::new(id, args),
                });
                Ok(Value::Null)
            }
            ids::SET_SAVE if self.set_path.is_none() => {
                self.effects.push(Effect::PickSavePath { then: None });
                Ok(Value::Null)
            }
            ids::SET_SAVE_AS if !has("path") => {
                self.effects.push(Effect::PickSavePath { then: None });
                Ok(Value::Null)
            }
            browser_ids::ADD_FOLDER if !has("path") => {
                self.effects.push(Effect::PickFolder);
                Ok(Value::Null)
            }
            _ if !has("track") && Switch::of_command(id).is_some() => {
                let switch = Switch::of_command(id).expect("just checked");
                self.toggle_tracks(id, switch, args)
            }
            "device.rename" if !has("name") => {
                let args = self.filled_args(id, args)?;
                let device = serde_json::from_value(args["device"].clone()).map_err(|error| {
                    ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: error.to_string(),
                    }
                })?;
                self.effects.push(Effect::RenameDevice(device));
                self.touch();
                Ok(Value::Null)
            }
            _ if device_ids::ON_SELECTED.contains(&id) && !has("device") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            SOLO_IN_PLACE if !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            "transport.set_metronome_enabled" if !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            "transport.set_metronome_only_while_recording" if !has("only_while_recording") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            _ if id == session_launch_ids::SET_QUANTIZATION_ENABLED && !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            "track.create" if !has("id") => {
                let mut args = self.filled_args(id, args)?;
                let new: TrackId = self.editor.new_id();
                args["id"] = json!(new);
                let result = self.run(id, args)?;
                // Spec: app-shell R10. The new track is selected.
                self.arrangement
                    .run(
                        view_ids::SELECT_TRACKS,
                        json!({ "tracks": [new] }),
                        self.editor.document(),
                    )
                    .ok();
                Ok(result)
            }
            _ => self.run(id, args),
        }
    }

    /// Runs what the save-changes dialog decided, or what follows a save it
    /// asked for: the unsaved changes go (`set.discard_changes`, a no-op
    /// after a save), then `then` runs without asking again. Opening a set
    /// asks for the file first and discards only once one is chosen, so
    /// cancelling the file dialog keeps the changes.
    ///
    /// # Errors
    ///
    /// As [`Shell::run`].
    pub fn discard_and_invoke(&mut self, then: &Invocation) -> Result<Value, ShellError> {
        if then.id == ids::SET_OPEN && then.args.get("path").is_none() {
            self.effects.push(Effect::PickOpenPath {
                discard_changes: true,
            });
            self.touch();
            return Ok(Value::Null);
        }
        self.invoke(ids::SET_DISCARD_CHANGES, json!({}))?;
        self.invoke(&then.id, then.args.clone())
    }

    /// Runs a command with every argument given, as an agent or a test does.
    /// It never asks: `set.new` and `app.quit` discard unsaved changes, and
    /// `set.open` refuses while there are any, so an agent saves or runs
    /// `set.discard_changes` first (the agent API asks the person before
    /// destructive commands).
    ///
    /// # Errors
    ///
    /// [`ShellError`] saying why the command didn't run or failed. The state
    /// is unchanged when a command fails, except that a device that refuses
    /// new audio settings goes back to the previous ones.
    pub fn run(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if self.edit_gesture.is_some()
            && (model::handles(id)
                || edit_ids::handles(id)
                || note_ids::handles(id)
                || device_ids::handles(id)
                || self.is_device_command(id)
                || track_ids::handles(id)
                || midi_ids::handles(id)
                || id == plugin_ids::LOAD
                || id.starts_with("set.")
                || id == ids::APP_QUIT)
        {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "a drag that edits the set is under way; it ends when it's released"
                    .to_owned(),
            });
        }
        if edit_ids::handles(id) {
            let before = self.editor.document().clone();
            let result = editing::run(
                id,
                args,
                &mut self.editor,
                &mut self.arrangement,
                &mut self.clipboard,
            )
            .map_err(|failure| edit_error(id, failure));
            if *self.editor.document() != before {
                self.dirty = true;
                self.document_changed();
            }
            self.touch();
            return result;
        }
        if note_ids::handles(id) {
            // Spec: piano-roll. The Clip View's note edits.
            let before = self.editor.document().clone();
            let result = clip_view::run(
                id,
                args,
                &mut self.editor,
                &mut self.clip_view,
                &self.arrangement,
            )
            .map_err(|failure| edit_error(id, failure));
            if *self.editor.document() != before {
                self.dirty = true;
                self.document_changed();
            }
            self.touch();
            return result;
        }
        if plugin_ids::handles(id) {
            return self.run_plugins(id, args);
        }
        if self.is_device_command(id)
            && let Some((device, command)) = crate::plugins::parse_device_command(id)
        {
            return self.run_device_command(id, device, command, args);
        }
        if model::handles(id) {
            let args_floating = args.get("floating") == Some(&Value::Bool(true));
            // Spec: automation R17. With Automation Arm on and the transport
            // recording, a control move is noted for the envelope too.
            let moved = (self.arrangement.automation_arm() && self.automation_recording)
                .then(|| model::command_from_args(id, args.clone()).ok())
                .flatten()
                .and_then(|command| {
                    crate::automation_record::moved(self.editor.document(), &command)
                });
            let changes =
                model::run(&mut self.editor, id, args).map_err(|error| ShellError::Failed {
                    command: id.to_owned(),
                    message: error.to_string(),
                })?;
            if let Some((track, target, value)) = moved {
                self.automation_take.write(
                    track,
                    target,
                    self.arrangement.transport().position,
                    value,
                );
            }
            if id == device_ids::SET_FLOATING && args_floating {
                // A device floated shows the windows again (R33).
                self.plug_in_windows = true;
                self.apply_plug_in_windows();
            }
            self.dirty = true;
            self.document_changed();
            self.touch();
            return Ok(to_value(&changes));
        }
        if engine_transport::handles(id) {
            // Spec: control-bar-transport R12, R13. Menus, keys, the Control
            // Bar, and agents all reach the engine's transport here.
            let command =
                engine_transport::command_from_args(id, Self::object(args)).map_err(|error| {
                    ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: error.to_string(),
                    }
                })?;
            // WP-2.13. Recording gives the engine what to write before it
            // knows the transport records. A reason no files can come of it
            // (a set with no project folder, say) is reported in the Status
            // Bar; the transport still records, as the metronome's "only
            // while recording" follows it.
            if command == bl_engine::TransportCommand::SetRecording(true)
                && let Err(reason) = self.begin_recording()
            {
                self.set_message(reason, true);
            }
            self.audio
                .send_transport(command)
                .map_err(|reason| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                })?;
            return Ok(Value::Null);
        }
        if engine_session::handles(id) {
            // Spec: launch-quantisation R9. The grid's launch buttons, the
            // status row, and agents all reach the engine's launching here.
            let command =
                engine_session::command_from_args(id, Self::object(args)).map_err(|error| {
                    ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: error.to_string(),
                    }
                })?;
            self.audio
                .send_session(command)
                .map_err(|reason| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                })?;
            return Ok(Value::Null);
        }
        if session_launch_ids::handles(id) {
            // Spec: launch-quantisation R1, R3, R6, R10. The Control Bar's
            // Quantization Menu and the Options items run here, with the
            // value saved beside the song.
            return self.run_session_launch(id, args);
        }
        if transport_ids::handles(id) {
            Self::args::<NoArgs>(id, args)?;
            let engine = self.poll_engine();
            return Ok(to_value(&crate::transport::state(self, engine)));
        }
        if view_ids::handles(id) {
            let result = self
                .arrangement
                .run(id, args, self.editor.document())
                .map_err(|error| match error {
                    ViewError::InvalidArgs(message) => ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message,
                    },
                    ViewError::Unavailable(reason) => ShellError::Unavailable {
                        command: id.to_owned(),
                        reason,
                    },
                });
            if id != view_ids::STATE {
                // A query changes nothing a window shows.
                self.touch();
            }
            if result.is_ok() && id == view_ids::FOCUS_TITLE_BAR {
                // Spec: accessibility R4. Esc from a track's control gives
                // the keyboard back to the Arrangement, at the title bar.
                self.focused_window = WindowKind::Main;
                self.focused_area = Area::MainView;
                self.effects.push(Effect::FocusArea(Area::MainView));
            }
            if result.is_ok() && matches!(id, view_ids::SET_INSERT_MARKER | view_ids::SELECT_TIME) {
                // Spec: control-bar-transport R15. A click or a time
                // selection in the Arrangement moves the engine's insert
                // marker, where Play starts, and playback carries on.
                let at = self.arrangement.insert_marker();
                self.audio.set_insert_marker(at);
            }
            return result.map(|state| to_value(&state));
        }
        if mixer_ids::handles(id) {
            let front = self.front_mixer();
            let result = self
                .mixer
                .run(id, args, front, self.editor.document(), &self.arrangement)
                .map_err(
                    |MixerViewError::InvalidArgs(message)| ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message,
                    },
                );
            if result.is_ok() && id == mixer_ids::FOCUS {
                // Spec: mixer-view R13.
                self.focused_window = WindowKind::Main;
                self.focused_area = Area::MainView;
                self.effects.push(Effect::FocusArea(Area::MainView));
            }
            if id != mixer_ids::STATE {
                self.touch();
            }
            return result.map(|state| to_value(&state));
        }
        if session_ids::handles(id) {
            // Spec: session-view. The grid's selection and scroll are view
            // state; its edits run the set's scene and slot commands as one
            // undo step.
            let columns = crate::mixer_view::columns(self.editor.document(), &self.arrangement);
            let scenes: Vec<bl_model::SceneId> = self
                .editor
                .document()
                .scenes()
                .map(|scene| scene.id)
                .collect();
            let edge_fades = self.prefs.create_fades_on_clip_edges.0;
            let outcome = self
                .session
                .run(
                    id,
                    args,
                    crate::session_view::Context {
                        editor: &mut self.editor,
                        media: &self.media,
                        columns: &columns,
                        scenes: &scenes,
                        edge_fades,
                    },
                )
                .map_err(|error| match error {
                    SessionViewError::InvalidArgs(message) => ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message,
                    },
                    SessionViewError::Unavailable(reason) => ShellError::Unavailable {
                        command: id.to_owned(),
                        reason,
                    },
                    SessionViewError::Failed(message) => ShellError::Failed {
                        command: id.to_owned(),
                        message,
                    },
                })?;
            for file in &outcome.media {
                self.store.add_media(file.hash, &file.path, file.size);
            }
            if outcome.document_changed {
                self.dirty = true;
                self.document_changed();
            }
            if id == session_ids::FOCUS {
                // Spec: session-view. Navigate ▸ Session View shows it and
                // moves the focus into it.
                self.main_view = MainView::Session;
                self.focused_window = WindowKind::Main;
                self.focused_area = Area::MainView;
                self.effects.push(Effect::FocusArea(Area::MainView));
            }
            if id != session_ids::STATE {
                self.touch();
            }
            return Ok(outcome.value);
        }
        if id == browser_ids::LOAD_SELECTED
            && let Some(path) = self.hot_swap_preset()
        {
            // Spec: device-chains R19. Loading a preset while hot-swapping
            // loads it into the device.
            Self::args::<NoArgs>(id, args)?;
            let loaded = devices::load_hot_swapped(&self.devices, &mut self.editor, &path)
                .map_err(|error| device_error(id, error))?;
            self.dirty = true;
            self.document_changed();
            self.touch();
            return Ok(to_value(&loaded));
        }
        if device_ids::handles(id) {
            return self.run_devices(id, args);
        }
        if track_ids::handles(id) {
            return self.run_tracks(id, args);
        }
        if browser_ids::handles(id) {
            return self.run_browser(id, args);
        }
        if perception_ids::handles(id) {
            return self.run_perception(id, args);
        }
        if export_ids::handles(id) {
            // Spec: export R15. The set, its media, the Arrangement's time
            // selection, and the engine's rate; the export changes none.
            let engine_rate = self.engine_rate();
            let args = Self::args(id, args)?;
            // Spec: offline-plugin-renders R7. Export stops the song first,
            // as Live does, and borrows the engine's plug-ins (R1).
            if self.audio.transport_state().playing {
                let _ = self.audio.send_transport(bl_engine::TransportCommand::Stop);
            }
            let media: Arc<dyn bl_engine::MediaLibrary> = self.media.clone();
            return crate::export::run_command(
                self.editor.document(),
                media,
                Some(&self.plugins),
                self.arrangement.time_selection(),
                self.arrangement.selected_tracks(),
                engine_rate,
                &args,
            )
            .map(|result| to_value(&result));
        }
        if midi_ids::handles(id) {
            return self.run_midi_files(id, args);
        }
        if !ids::handles(id) {
            return Err(ShellError::UnknownCommand(id.to_owned()));
        }
        let result = self.run_shell(id, args);
        self.touch();
        result
    }

    /// The engine's rate: the audio device's, or the rate set in the audio
    /// settings, or 48 kHz when neither is known (the export spec, R6).
    #[must_use]
    pub fn engine_rate(&self) -> u32 {
        self.audio
            .sample_rate()
            .or(self.audio.settings().sample_rate)
            .unwrap_or(crate::export::DEFAULT_ENGINE_RATE)
    }

    /// Runs a Standard MIDI File command (WP-2.21): importing a file into the
    /// Arrangement, or writing a MIDI clip out as one.
    fn run_midi_files(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            midi_ids::IMPORT => {
                let args = Self::args::<midi_ids::MidiImportArgs>(id, args)?;
                let at = args.at.unwrap_or_else(|| self.arrangement.insert_marker());
                let path = PathBuf::from(&args.path);
                let result = crate::midi_files::import(
                    &mut self.editor,
                    &path,
                    args.track,
                    at,
                    args.import_tempo,
                )
                .map_err(|error| midi_file_error(id, error))?;
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&result))
            }
            midi_ids::EXPORT => {
                let args = Self::args::<midi_ids::MidiExportArgs>(id, args)?;
                let result = crate::midi_files::export(self.editor.document(), &args)
                    .map_err(|error| midi_file_error(id, error))?;
                self.touch();
                Ok(to_value(&result))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Whether the transport is currently playing.
    #[must_use]
    pub fn is_playing(&mut self) -> bool {
        self.audio.transport_state().playing
    }
    /// The Arrangement's time selection, if there is one.
    #[must_use]
    pub fn time_selection(&self) -> Option<(bl_core::time::Ticks, bl_core::time::Ticks)> {
        self.arrangement.time_selection()
    }

    /// Runs a perception command (WP-1.16, WP-2.28): an audio preview or an
    /// analysis, rendered offline from the set as it is now. Neither
    /// changes the set, the view, or the engine playing on the device.
    fn run_perception(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let engine_rate = self.engine_rate();
        let playing = self.audio.transport_state().playing;
        let source = crate::perception::Source {
            document: self.editor.document(),
            media: Arc::clone(&self.media) as Arc<dyn bl_engine::MediaLibrary>,
            selection: self.arrangement.time_selection(),
            engine_rate,
            plugins: Some(&self.plugins),
            playing,
            cancel: None,
        };
        match id {
            perception_ids::PREVIEW => {
                let args = Self::args(id, args)?;
                let outcome = crate::perception::render_preview(&source, &args)?;
                let preview = self.previews.add(
                    outcome.samples,
                    args.bit_depth,
                    outcome.rendered,
                    self.editor.document().clone(),
                );
                let result = preview
                    .result(outcome.peak_dbfs, outcome.took_seconds)
                    .map_err(|message| ShellError::Failed {
                        command: perception_ids::PREVIEW.to_owned(),
                        message,
                    })?;
                Ok(to_value(&result))
            }
            perception_ids::ANALYZE => {
                let args = Self::args(id, args)?;
                crate::perception::analyze(&self.previews, &source, &args)
                    .map(|result| to_value(&result))
            }
            _ => {
                let _: perception_ids::CaptureArgs = Self::args(id, args)?;
                Err(ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "a view capture is an image of the app's window, which only the \
                             agent API's `capture_view` tool returns, while the app's window \
                             is open"
                        .to_owned(),
                })
            }
        }
    }

    /// The agent API's last audio previews (WP-1.16).
    #[must_use]
    pub fn previews(&self) -> &crate::perception::Previews {
        &self.previews
    }

    /// The agent API's last audio previews (WP-1.16, WP-2.28).
    pub fn previews_mut(&mut self) -> &mut crate::perception::Previews {
        &mut self.previews
    }

    /// Arguments as a JSON object: `null` is no arguments.
    fn object(args: Value) -> Value {
        if args.is_null() { json!({}) } else { args }
    }

    fn args<T: DeserializeOwned>(id: &str, args: Value) -> Result<T, ShellError> {
        let args = Self::object(args);
        serde_json::from_value(args).map_err(|error| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: error.to_string(),
        })
    }

    fn view_result(&self) -> Value {
        to_value(&ViewResult {
            main_view: self.main_view.id().to_owned(),
            browser: self.browser,
            info_view: self.info_view,
            full_screen: self.full_screen,
            zoom: self.zoom(),
        })
    }

    fn set_result(&self) -> Value {
        to_value(&SetResult {
            path: self
                .set_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            name: self.set_name(),
        })
    }

    /// Runs one of the launch quantisation's commands (WP-3.2) and the
    /// Session View's Cmd+Enter stop: the quantisation and its switch are
    /// saved with the set, beside the song, and the engine that plays the
    /// set hears every change.
    fn run_session_launch(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let current = self.store.launch();
        let settings = match id {
            session_launch_ids::SET_QUANTIZATION => {
                let session_launch_ids::SetQuantizationArgs { quantization } =
                    Self::args(id, args)?;
                LaunchSettings {
                    quantization,
                    ..current
                }
            }
            session_launch_ids::SMALLER_QUANTIZATION => {
                Self::args::<NoArgs>(id, args)?;
                LaunchSettings {
                    quantization: current.quantization.smaller(),
                    ..current
                }
            }
            session_launch_ids::LARGER_QUANTIZATION => {
                Self::args::<NoArgs>(id, args)?;
                LaunchSettings {
                    quantization: current.quantization.larger(),
                    ..current
                }
            }
            session_launch_ids::TRIPLET_QUANTIZATION => {
                Self::args::<NoArgs>(id, args)?;
                LaunchSettings {
                    quantization: current.quantization.triplet(),
                    ..current
                }
            }
            session_launch_ids::SET_QUANTIZATION_ENABLED => {
                let session_launch_ids::SetQuantizationEnabledArgs { enabled } =
                    Self::args(id, args)?;
                LaunchSettings {
                    enabled: enabled.unwrap_or(!current.enabled),
                    ..current
                }
            }
            session_launch_ids::STOP_SELECTED => {
                Self::args::<NoArgs>(id, args)?;
                // Spec: launch-quantisation R6. Every track with a selected
                // slot stops on the grid, as Cmd+Enter does.
                let mut tracks: Vec<TrackId> = Vec::new();
                for slot in self.session.selected_slots() {
                    if !tracks.contains(&slot.track) {
                        tracks.push(slot.track);
                    }
                }
                if tracks.is_empty() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a clip slot first".to_owned(),
                    });
                }
                for track in tracks {
                    self.audio
                        .send_session(engine_session::SessionCommand::StopTrack(track))
                        .map_err(|reason| ShellError::Unavailable {
                            command: id.to_owned(),
                            reason,
                        })?;
                }
                return Ok(Value::Null);
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        };
        self.store.set_launch(settings);
        self.audio.set_launch_settings(settings);
        self.touch();
        Ok(to_value(&settings))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per command keeps each one's behaviour in sight"
    )]
    fn run_shell(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        match id {
            ids::SET_NEW => {
                Self::args::<NoArgs>(id, args)?;
                self.set_replaced();
                // Spec: file-management R1. A new set starts from the saved
                // default set when there is one.
                let editor = new_set_from(self.prefs.default_set.as_deref());
                self.saved = snapshot(&editor);
                self.editor = editor;
                self.store.close();
                self.devices.set_windows(bl_model::DeviceWindows::new());
                self.clip_view.set_folds(bl_model::ClipFolds::new());
                self.set_path = None;
                self.dirty = false;
                self.main_view = MainView::Arrangement;
                self.message = None;
                self.arrangement.reset();
                self.clip_view.forget_selection();
                self.mixer.reset();
                self.session.reset();
                self.new_set_transport();
                self.document_changed();
                self.set_file_changed();
                Ok(self.set_result())
            }
            ids::SET_OPEN => {
                let PathArgs { path } = Self::args(id, args)?;
                if self.dirty {
                    return Err(unavailable(
                        "the set has unsaved changes; save them with `set.save` or \
                         `set.save_as`, or throw them away with `set.discard_changes`, first",
                    ));
                }
                let (editor, opened) = self.store.open(Path::new(&path)).map_err(failed)?;
                // Spec: device-chains R29. Floating windows reopen where
                // they were saved.
                self.devices.set_windows(self.store.windows());
                // Spec: piano-roll. Folded note editors too.
                self.clip_view.set_folds(self.store.folds());
                self.saved = snapshot(&editor);
                self.set_replaced();
                self.editor = editor;
                self.set_path = Some(opened);
                self.dirty = false;
                self.message = None;
                self.arrangement.reset();
                self.clip_view.forget_selection();
                self.mixer.reset();
                self.session.reset();
                // The set's files go into the media store, so its clips'
                // waveforms build (browser R15) and the engine finds their
                // audio when the set reaches it.
                for (hash, path, size) in self.store.media_files() {
                    self.media
                        .add_reference(bl_media::ContentHash::from_bytes(hash.0), path, size);
                }
                // Spec: file-management R6. Files that moved are found
                // again by content, so the set opens able to play them.
                let relinked = self.relink_missing(&[]);
                if relinked > 0 {
                    self.message = Some(Message {
                        text: format!(
                            "{relinked} missing file{} found again and relinked",
                            if relinked == 1 { " was" } else { "s were" }
                        ),
                        error: false,
                        action: None,
                    });
                }
                self.new_set_transport();
                self.document_changed();
                self.set_file_changed();
                Ok(self.set_result())
            }
            ids::SET_SAVE => {
                Self::args::<NoArgs>(id, args)?;
                let path = self.set_path.clone().ok_or_else(|| {
                    unavailable("the set has never been saved; use `set.save_as` to give it a file")
                })?;
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.kit,
                    &mut self.editor,
                );
                self.store.set_windows(self.devices.windows().clone());
                self.store.set_folds(self.clip_view.folds().clone());
                let warnings = self.store.save(&path, &self.editor).map_err(failed)?;
                self.saved_as(path, &warnings);
                Ok(self.set_result())
            }
            ids::SET_SAVE_AS => {
                // Spec: app-shell R17 to R21. The store decides where in a
                // project folder the set goes; the result says where.
                let SaveAsArgs { path } = Self::args(id, args)?;
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.kit,
                    &mut self.editor,
                );
                self.store.set_windows(self.devices.windows().clone());
                self.store.set_folds(self.clip_view.folds().clone());
                let (path, warnings) = self
                    .store
                    .save_as(Path::new(&path), &self.editor)
                    .map_err(failed)?;
                self.saved_as(path, &warnings);
                Ok(self.set_result())
            }
            ids::SET_DISCARD_CHANGES => {
                Self::args::<NoArgs>(id, args)?;
                if self.dirty {
                    self.set_replaced();
                    self.editor = snapshot(&self.saved);
                    self.gesture = false;
                    self.dirty = false;
                    self.document_changed();
                }
                Ok(self.set_result())
            }
            ids::SET_SAVE_AS_DEFAULT => {
                // Spec: file-management R2. The open set becomes the one a
                // new set starts from, saved as a copy so it keeps its own
                // file and stays open.
                let DefaultSetArgs { path } = Self::args(id, args)?;
                let path = match path {
                    Some(path) => self.default_set_file_at(Path::new(&path)),
                    None => self.default_set_file().ok_or_else(|| {
                        unavailable(
                            "there's nowhere to save the default set: give `path`, or set up a \
                             User Library",
                        )
                    })?,
                };
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.kit,
                    &mut self.editor,
                );
                let warnings = self.store.save_copy(&path, &self.editor).map_err(failed)?;
                self.prefs.default_set = Some(path.clone());
                self.save_preferences();
                self.message = (!warnings.is_empty()).then(|| Message {
                    text: format!("The default set was saved, but: {}", warnings.join("; ")),
                    error: false,
                    action: None,
                });
                Ok(to_value(&SetResult {
                    path: Some(path.to_string_lossy().into_owned()),
                    name: self.set_name(),
                }))
            }
            ids::SET_SAVE_AS_TEMPLATE => {
                // Spec: file-management R3. Templates live in the User
                // Library's `Templates` folder, as in Live.
                let TemplateArgs { name } = Self::args(id, args)?;
                let folder = self
                    .templates_folder()
                    .ok_or_else(|| unavailable("there's no User Library to save a template in"))?;
                let name = name.unwrap_or_else(|| self.set_name());
                if name.is_empty() || name.contains(['/', '\\']) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "a template's name can't be empty or hold a path separator"
                            .to_owned(),
                    });
                }
                let path = folder.join(format!("{name}.{}", bl_project::SET_EXTENSION));
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.kit,
                    &mut self.editor,
                );
                let warnings = self.store.save_copy(&path, &self.editor).map_err(failed)?;
                self.message = (!warnings.is_empty()).then(|| Message {
                    text: format!("The template was saved, but: {}", warnings.join("; ")),
                    error: false,
                    action: None,
                });
                Ok(to_value(&SetResult {
                    path: Some(path.to_string_lossy().into_owned()),
                    name,
                }))
            }
            ids::SET_COLLECT_ALL => {
                Self::args::<NoArgs>(id, args)?;
                let path = self.set_path.clone().ok_or_else(|| {
                    unavailable(
                        "the set has never been saved; use `set.save_as` to give it a project \
                         folder to collect into",
                    )
                })?;
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.kit,
                    &mut self.editor,
                );
                self.store.set_windows(self.devices.windows().clone());
                self.store.set_folds(self.clip_view.folds().clone());
                let files = self.store.collect().map_err(failed)?;
                let warnings = self.store.save(&path, &self.editor).map_err(failed)?;
                self.saved_as(path.clone(), &warnings);
                Ok(to_value(&FileReport {
                    path: Some(path.to_string_lossy().into_owned()),
                    files,
                }))
            }
            ids::SET_RELINK_MISSING => {
                let RelinkArgs { folders } = Self::args(id, args)?;
                let extra: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
                let files = self.relink_missing(&extra);
                Ok(to_value(&FileReport {
                    path: self
                        .set_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                    files,
                }))
            }
            ids::APP_QUIT => {
                Self::args::<NoArgs>(id, args)?;
                self.audio.set_test_tone(false);
                self.save_preferences();
                self.effects.push(Effect::Quit);
                Ok(Value::Null)
            }
            ids::APP_HIDE => {
                Self::args::<NoArgs>(id, args)?;
                self.effects.push(Effect::Hide);
                Ok(Value::Null)
            }
            ids::APP_ABOUT => {
                Self::args::<NoArgs>(id, args)?;
                self.effects.push(Effect::ShowAbout);
                Ok(Value::Null)
            }
            ids::APP_SETTINGS => {
                let SettingsArgs { page } = Self::args(id, args)?;
                if let Some(page) = page {
                    self.prefs.settings_page = page;
                }
                self.settings_open = true;
                self.focused_window = WindowKind::Settings;
                self.effects.push(Effect::OpenSettings);
                Ok(Value::Null)
            }
            ids::WINDOW_CLOSE => {
                Self::args::<NoArgs>(id, args)?;
                if let WindowKind::Device(device) = self.focused_window {
                    // Spec: device-chains R27. Closing a floating device's
                    // window docks the device: one undo step.
                    self.focused_window = WindowKind::Main;
                    return self.run(
                        device_ids::SET_FLOATING,
                        json!({ "device": device, "floating": false }),
                    );
                }
                if !self.settings_open {
                    return Err(unavailable("no secondary window is open"));
                }
                self.close_settings();
                Ok(Value::Null)
            }
            ids::VIEW_FULL_SCREEN => {
                Self::args::<NoArgs>(id, args)?;
                self.full_screen = !self.full_screen;
                self.effects.push(Effect::SetFullScreen(self.full_screen));
                Ok(self.view_result())
            }
            ids::VIEW_TOGGLE_PLUG_IN_WINDOWS => {
                // Spec: device-chains R33. It hides and shows windows only.
                let ids::TogglePlugInWindowsArgs { shown } = Self::args(id, args)?;
                if !self.any_plug_in_window() {
                    return Err(unavailable("no window to show or hide"));
                }
                self.plug_in_windows = shown.unwrap_or(!self.plug_in_windows);
                self.apply_plug_in_windows();
                Ok(to_value(&SwitchResult {
                    on: self.plug_in_windows,
                }))
            }
            ids::VIEW_TOGGLE_BROWSER => {
                Self::args::<NoArgs>(id, args)?;
                self.browser = !self.browser;
                if !self.browser && self.focused_area == Area::Browser {
                    self.focused_area = Area::MainView;
                }
                Ok(self.view_result())
            }
            ids::VIEW_TOGGLE_CLIP_VIEW => {
                // Spec: piano-roll; L-STAT-004. View state only.
                Self::args::<NoArgs>(id, args)?;
                self.clip_view.toggle();
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_DRAW_MODE => {
                // Spec: piano-roll R2; S-GRID-001, M-OPT-017. View state only.
                Self::args::<NoArgs>(id, args)?;
                self.clip_view.toggle_draw_mode();
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_FOLD_NOTES => {
                // Spec: piano-roll; M-VIEW-039, S-NOTE-038. View state only,
                // saved with the set.
                let FoldNotesArgs { clip, folded } = Self::args(id, args)?;
                let clip = match clip {
                    Some(clip) => clip,
                    None => self
                        .clip_view
                        .shown_clip(self.editor.document(), &self.arrangement)
                        .ok_or_else(|| unavailable("no MIDI clip is shown in the Clip View"))?,
                };
                match self.editor.document().clip(clip) {
                    None => {
                        return Err(failed(format!("no clip has the ID {}", clip.0)));
                    }
                    Some(value) if !matches!(value.data, ClipData::Midi(_)) => {
                        return Err(failed("the clip isn't a MIDI clip".to_owned()));
                    }
                    Some(_) => {}
                }
                let folded = folded.unwrap_or(!self.clip_view.is_folded(clip));
                self.clip_view.set_folded(clip, folded);
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_LANE => {
                // Spec: piano-roll; the lane shows one of a note's values.
                let LaneArgs { lane } = Self::args(id, args)?;
                self.clip_view.set_lane(lane);
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_TOGGLE_INFO_VIEW => {
                Self::args::<NoArgs>(id, args)?;
                self.info_view = !self.info_view;
                Ok(self.view_result())
            }
            ids::VIEW_TOGGLE_SESSION_ARRANGEMENT => {
                Self::args::<NoArgs>(id, args)?;
                self.main_view = match self.main_view {
                    MainView::Arrangement => MainView::Session,
                    MainView::Session => MainView::Arrangement,
                };
                Ok(self.view_result())
            }
            // Spec: interface-zoom R3, R5, R6.
            ids::VIEW_ZOOM_IN | ids::VIEW_ZOOM_OUT | ids::VIEW_ZOOM_RESET => {
                Self::args::<NoArgs>(id, args)?;
                let next = match id {
                    ids::VIEW_ZOOM_IN => zoom_step(self.zoom_percent(), 1),
                    ids::VIEW_ZOOM_OUT => zoom_step(self.zoom_percent(), -1),
                    _ => 100,
                };
                self.set_zoom_percent(next);
                Ok(self.view_result())
            }
            ids::VIEW_SET_ZOOM => {
                let ZoomArgs { percent } = Self::args(id, args)?;
                self.set_zoom_percent(percent);
                Ok(self.view_result())
            }
            ids::FOCUS_CONTROL_BAR => {
                Self::args::<NoArgs>(id, args)?;
                self.focused_window = WindowKind::Main;
                self.focused_area = Area::ControlBar;
                self.effects.push(Effect::FocusArea(Area::ControlBar));
                Ok(Value::Null)
            }
            ids::FOCUS_ARRANGEMENT => {
                Self::args::<NoArgs>(id, args)?;
                self.main_view = MainView::Arrangement;
                self.focused_window = WindowKind::Main;
                self.focused_area = Area::MainView;
                self.effects.push(Effect::FocusArea(Area::MainView));
                Ok(Value::Null)
            }
            ids::FOCUS_NEXT | ids::FOCUS_PREVIOUS => {
                Self::args::<NoArgs>(id, args)?;
                self.effects.push(Effect::MoveFocus {
                    backwards: id == ids::FOCUS_PREVIOUS,
                    neighbor: false,
                });
                Ok(Value::Null)
            }
            // Spec: accessibility R4.
            ids::FOCUS_NEXT_NEIGHBOR | ids::FOCUS_PREVIOUS_NEIGHBOR => {
                Self::args::<NoArgs>(id, args)?;
                self.effects.push(Effect::MoveFocus {
                    backwards: id == ids::FOCUS_PREVIOUS_NEIGHBOR,
                    neighbor: true,
                });
                Ok(Value::Null)
            }
            // Spec: accessibility R5.
            ids::FOCUS_TOGGLE_WRAP_NAVIGATION => {
                Self::args::<NoArgs>(id, args)?;
                self.prefs.wrap_navigation = OnByDefault(!self.prefs.wrap_navigation.0);
                self.save_preferences();
                Ok(to_value(&SwitchResult {
                    on: self.prefs.wrap_navigation.0,
                }))
            }
            // Spec: accessibility R7.
            ids::SETTINGS_SET_OUTLINE_VIEW_IN_FOCUS => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.outline_view_in_focus = OnByDefault(on);
                self.save_preferences();
                self.touch();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::FOCUS_TOGGLE_TAB_NAVIGATION => {
                Self::args::<NoArgs>(id, args)?;
                self.prefs.tab_navigation = !self.prefs.tab_navigation;
                self.save_preferences();
                Ok(to_value(&SwitchResult {
                    on: self.prefs.tab_navigation,
                }))
            }
            ids::SETTINGS_NEXT_PAGE | ids::SETTINGS_PREVIOUS_PAGE => {
                Self::args::<NoArgs>(id, args)?;
                if !self.settings_open {
                    return Err(unavailable("the Settings window isn't open"));
                }
                let step = if id == ids::SETTINGS_NEXT_PAGE { 1 } else { -1 };
                self.prefs.settings_page = self.prefs.settings_page.step(step);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_THEME => {
                let ThemeArgs { theme } = Self::args(id, args)?;
                self.prefs.theme = theme;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.create_fades_on_clip_edges = OnByDefault(on);
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.auto_hide_plug_in_windows = OnByDefault(on);
                self.apply_plug_in_windows();
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::AUDIO_DEVICES => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.audio.devices()))
            }
            ids::AUDIO_STATUS => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.audio.status()))
            }
            ids::AUDIO_CONFIGURE => {
                let change: ConfigureAudio = Self::args(id, args)?;
                let result = self.audio.configure(&change);
                self.prefs.audio = self.audio.settings().clone();
                self.save_preferences();
                result.map_err(failed)?;
                if self.message.as_ref().is_some_and(|m| m.error) {
                    self.message = None;
                }
                Ok(to_value(&self.audio.status()))
            }
            ids::AUDIO_TOGGLE_ENGINE => {
                Self::args::<NoArgs>(id, args)?;
                let on = !self.audio.engine_on();
                match self.audio.set_engine(on) {
                    Ok(()) if on => self.message = None,
                    Ok(()) => {
                        self.message = Some(Message {
                            text: "The audio engine is off. Click to open the Audio settings."
                                .to_owned(),
                            error: false,
                            action: Some(Invocation::new(
                                ids::APP_SETTINGS,
                                json!({ "page": "audio" }),
                            )),
                        });
                    }
                    Err(error) => self.audio_failed(&error),
                }
                Ok(to_value(&SwitchResult { on }))
            }
            ids::AUDIO_SET_TEST_TONE => {
                let TestToneArgs {
                    on,
                    volume_db,
                    frequency_hz,
                } = Self::args(id, args)?;
                if volume_db.is_some() || frequency_hz.is_some() {
                    let current = self.audio.tone();
                    self.audio.set_tone(ToneSettings {
                        volume_db: volume_db.unwrap_or(current.volume_db),
                        frequency_hz: frequency_hz.unwrap_or(current.frequency_hz),
                    });
                    self.prefs.tone = self.audio.tone();
                    self.save_preferences();
                }
                if let Some(on) = on {
                    self.audio.set_test_tone(on);
                }
                let tone = self.audio.tone();
                Ok(to_value(&TestToneState {
                    on: self.audio.test_tone(),
                    volume_db: tone.volume_db,
                    frequency_hz: tone.frequency_hz,
                }))
            }
            agent::AGENT_SET_SERVER => {
                let SetServerArgs { on, port } = Self::args(id, args)?;
                if let Some(port) = port {
                    if port < 1024 {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: format!("port {port} is below 1024"),
                        });
                    }
                    self.prefs.agent.port = port;
                }
                if let Some(on) = on {
                    self.prefs.agent.server = on;
                }
                if self.prefs.agent.server && self.prefs.agent.token.is_empty() {
                    self.prefs.agent.token = agent::new_token().map_err(failed)?.into();
                }
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_APPROVE_CLIENT => {
                let ClientArgs { client } = Self::args(id, args)?;
                if client.trim().is_empty() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the agent's name is empty".to_owned(),
                    });
                }
                if self.prefs.agent.client(&client).is_none() {
                    // Its rights go with a token of its own (WP-1.15b).
                    let token = agent::new_token().map_err(failed)?;
                    self.prefs.agent.clients.push(agent::AgentClient {
                        name: client,
                        autonomous: false,
                        token: token.into(),
                    });
                    self.save_preferences();
                }
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_FORGET_CLIENT => {
                let ClientArgs { client } = Self::args(id, args)?;
                let before = self.prefs.agent.clients.len();
                self.prefs.agent.clients.retain(|c| c.name != client);
                if self.prefs.agent.clients.len() == before {
                    return Err(unavailable(&format!(
                        "no agent called “{client}” is allowed"
                    )));
                }
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_SET_AUTONOMOUS => {
                let AutonomousArgs { client, on } = Self::args(id, args)?;
                let Some(entry) = self
                    .prefs
                    .agent
                    .clients
                    .iter_mut()
                    .find(|c| c.name == client)
                else {
                    return Err(unavailable(&format!(
                        "no agent called “{client}” is allowed; it's allowed when it first \
                         connects"
                    )));
                };
                entry.autonomous = on;
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_NEW_TOKEN => {
                Self::args::<NoArgs>(id, args)?;
                self.prefs.agent.token = agent::new_token().map_err(failed)?.into();
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_NEW_CLIENT_TOKEN => {
                let ClientArgs { client } = Self::args(id, args)?;
                let token = agent::new_token().map_err(failed)?;
                let Some(entry) = self
                    .prefs
                    .agent
                    .clients
                    .iter_mut()
                    .find(|c| c.name == client)
                else {
                    return Err(unavailable(&format!(
                        "no agent called “{client}” is allowed"
                    )));
                };
                entry.token = token.into();
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            assistant_ids::ASSISTANT_SET_PROVIDER => {
                let SetProviderArgs { provider } = Self::args(id, args)?;
                self.prefs.assistant.provider = provider;
                self.save_preferences();
                self.refresh_assistant_key();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_SET_BASE_URL => {
                let SetBaseUrlArgs { base_url } = Self::args(id, args)?;
                self.prefs.assistant.base_url = base_url.trim().to_owned();
                self.save_preferences();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_SET_MODEL => {
                let SetModelArgs { model } = Self::args(id, args)?;
                self.prefs.assistant.model = model.trim().to_owned();
                self.save_preferences();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_SET_KEY => {
                // Spec: agent-permissions R7, by analogy. The key is written
                // to the keychain and never returned or logged.
                let SetKeyArgs { key } = Self::args(id, args)?;
                if key.trim().is_empty() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the API key is empty".to_owned(),
                    });
                }
                let account = keychain_account(self.prefs.assistant.provider);
                self.keychain
                    .write(account, key.trim())
                    .map_err(|error| failed(error.to_string()))?;
                self.refresh_assistant_key();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_FORGET_KEY => {
                Self::args::<NoArgs>(id, args)?;
                let account = keychain_account(self.prefs.assistant.provider);
                self.keychain
                    .delete(account)
                    .map_err(|error| failed(error.to_string()))?;
                self.refresh_assistant_key();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_TOGGLE_ACTIVITY_LOG => {
                Self::args::<NoArgs>(id, args)?;
                self.assistant_log_open = !self.assistant_log_open;
                self.touch();
                Ok(to_value(&self.assistant_state()))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// A new or opened set starts stopped at 1.1.1
    /// (the control bar transport spec, R16), with no gesture open
    /// on the editor it replaced.
    fn new_set_transport(&mut self) {
        self.gesture = false;
        self.audio.reset_transport();
        // Spec: launch-quantisation R1. The set's global quantisation, so
        // launching waits for the right grid (the engine defaults to it too,
        // so this only matters for a set that was saved with another).
        self.audio.set_launch_settings(self.store.launch());
    }

    /// Records a save to `path`: it's the set's file, the set is clean, and
    /// warnings from the save show in the Status Bar.
    fn saved_as(&mut self, path: PathBuf, warnings: &[String]) {
        self.saved = snapshot(&self.editor);
        self.set_path = Some(path);
        self.set_file_changed();
        self.dirty = false;
        self.message = (!warnings.is_empty()).then(|| Message {
            text: format!("The set was saved, but: {}", warnings.join("; ")),
            error: false,
            action: None,
        });
    }

    /// Closes the Settings window, as its close button or Esc does: the test
    /// tone stops, and the focus returns to the main window.
    pub fn close_settings(&mut self) {
        if self.settings_open {
            self.settings_open = false;
            self.audio.set_test_tone(false);
            self.focused_window = WindowKind::Main;
            self.effects.push(Effect::CloseSettings);
            self.touch();
        }
    }

    /// Records that the window left full-screen mode by other means, such
    /// as the window's own button.
    pub fn set_full_screen_state(&mut self, on: bool) {
        if self.full_screen != on {
            self.full_screen = on;
            self.touch();
        }
    }
}
