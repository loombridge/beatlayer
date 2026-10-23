//! The shell's state and the one place every command runs: [`Shell`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_commands::{Context, NoArgs, Registry};
use bl_core::id::Id;
use bl_core::time::{Frames, SampleRate, Ticks};
use bl_engine::session as engine_session;
use bl_engine::transport as engine_transport;
use bl_media::MediaStore;
use bl_model::{
    AudioData, AutomationTarget, ChangeSet, Clip, ClipData, Command, ContentHash, Editor, Input,
    LaunchSettings, TrackId, TrackKind, registry as model,
};
use bl_project::ProjectFolder;
use bl_project::autosave::{Autosave, Recoverable, Recovered};
use bl_project::registry as recovery_ids;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use bl_assistant::keychain::{Keychain, MemoryKeychain, OsKeychain};

use crate::agent::{
    self as agent, AgentServerState, AgentServerStatus, AgentSettings, AutonomousArgs, ClientArgs,
    SetServerArgs,
};
use crate::arrangement::automation::{self as automation, EnvelopeClipboard};
use crate::arrangement::editing::{self, Clipboard, EditFailure};
use crate::arrangement::{ArrangementView, TransportView, ViewError, Viewport};
use crate::assistant::{
    self as assistant_ids, ActivityEntry, AssistantSettings, AssistantState, Outcome, SendArgs,
    SetBaseUrlArgs, SetKeyArgs, SetModelArgs, SetPanelWidthArgs, SetProviderArgs, keychain_account,
};
use crate::audio::{AudioHost, ConfigureAudio, EngineTransport, ToneSettings};
use crate::auto_warp;
use crate::browser::{self, BrowserError, BrowserView, Waker};
use crate::clip_view::{self, ClipView};
use crate::commands::{
    self as ids, AutomationRecordModeArgs, ClipEnvelopeArgs, ClipTabArgs, CollectFilesArgs,
    DefaultLaunchModeArgs, DefaultLaunchQuantizationArgs, DefaultSetArgs, DefaultWarpModeArgs,
    FileReport, FoldNotesArgs, LaneArgs, LanesArgs, LoopWarpShortSamplesArgs, PathArgs,
    RecordSessionAutomationInArgs, SampleEditorArgs, SampleEditorResult, SaveAsArgs, SetResult,
    SettingsArgs, SwitchArgs, SwitchResult, TemplateArgs, TemporaryFolderArgs,
    TemporaryFolderResult, TestToneArgs, TestToneState, ThemeArgs, UserLibraryArgs,
    UserLibraryResult, ViewResult, ViewState, ZoomArgs,
};
use crate::commands::{AppearanceArgs, BrightnessArgs, HueArgs, PercentArgs, ToneArgs};
use crate::devices::{self, DeviceError, DeviceView};
use crate::files::{ProjectFiles, SetStore, new_set_from};
use crate::midi::MidiHost;
use crate::mixer_controls::{self, Switch};
use crate::mixer_view::{MixerOf, MixerView, MixerViewError};
use crate::permission::CallPermission;
use crate::sample_editor;
use crate::session_view::{SessionView, SessionViewError};
use crate::settings::{
    Appearance, BUNDLED_THEME, OnByDefault, Preferences, SettingsPage, ThemeLook, WarpPrefs,
    ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT, zoom_factor, zoom_percent, zoom_step,
};
use bl_devices::KitHost;
use bl_plugin_host::au::AuHost;
use bl_plugin_host::clap::ClapHost;
use bl_plugin_host::scan::Scanner;
use bl_plugin_host::vst3::Vst3Host;
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::audio_editing as audio_ids;
use bl_shell_spec::automation as automation_ids;
use bl_shell_spec::autosampler as autosampler_ids;
use bl_shell_spec::bounce as bounce_ids;
use bl_shell_spec::browser as browser_ids;
use bl_shell_spec::capture_midi as capture_ids;
use bl_shell_spec::comping as comping_ids;
use bl_shell_spec::dawproject as dawproject_ids;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::editing as edit_ids;
use bl_shell_spec::export as export_ids;
use bl_shell_spec::file_manager as file_manager_ids;
use bl_shell_spec::freeze as freeze_ids;
use bl_shell_spec::locator as locator_ids;
use bl_shell_spec::midi_files as midi_ids;
use bl_shell_spec::midi_io as midi_io_ids;
use bl_shell_spec::mixer_view as mixer_ids;
use bl_shell_spec::multisampler as multisampler_ids;
use bl_shell_spec::notes::{self as note_ids, ClipTab, RandomizeAmounts};
use bl_shell_spec::pads as pad_ids;
use bl_shell_spec::perception as perception_ids;
use bl_shell_spec::plugins as plugin_ids;
use bl_shell_spec::sample_editor as sample_ids;
use bl_shell_spec::scales as scales_ids;
use bl_shell_spec::search as search_ids;
use bl_shell_spec::session_launch as session_launch_ids;
use bl_shell_spec::session_record as session_record_ids;
use bl_shell_spec::session_view as session_ids;
use bl_shell_spec::settings::{
    AgentsPageSettings, AudioPageSettings, DisplayInputSettings, FileFolderSettings, LaunchPrefs,
    LibrarySettings, STATE as SETTINGS_STATE, SettingsState, ThemeColorsSettings,
};
use bl_shell_spec::slice as slice_ids;
use bl_shell_spec::stems as stems_ids;
use bl_shell_spec::tracks as track_ids;
use bl_shell_spec::transport as transport_ids;
use bl_shell_spec::transport::{RecordedFileReport, RecordingReport, RunningRecording};

pub(crate) mod audio_editing;
mod audio_panel;
pub mod autosample;
mod bounce;
mod capture;
mod export_dialog;
pub use export_dialog::ExportPrefs;
mod file_manager;
mod freeze;
mod keymap;
mod multisampler;
mod plugin_list;
mod processed_files;
mod sample_edits;
mod sample_keys;
pub use sample_keys::TransientList;
mod search;
mod session_recording;
mod slice;
mod stems;
mod warping;

use file_manager::FileManager;

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
    /// The assistant's panel in a window of its own (WP-2.23, part 3; D24).
    Assistant,
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
    /// The assistant's pane at the right edge (D24).
    Assistant,
    /// The Undo History panel at the right edge (WP-6.28).
    UndoHistory,
}

/// `mixer.set_solo_in_place`, which Options ▸ Solo in Place runs as a
/// switch (M-OPT-025).
const SOLO_IN_PLACE: &str = "mixer.set_solo_in_place";
const CUE_MODE: &str = "mixer.set_cue_mode";
/// `mixer.set_delay_compensation`, which Options ▸ Delay Compensation runs
/// as a switch (M-OPT-008).
const DELAY_COMPENSATION: &str = "mixer.set_delay_compensation";
/// `mixer.set_reduced_latency_monitoring`, which Options ▸ Reduced Latency
/// When Monitoring runs as a switch (M-OPT-009).
const REDUCED_LATENCY_MONITORING: &str = "mixer.set_reduced_latency_monitoring";
/// `song.set_lock_envelopes`, which Options ▸ Lock Envelopes runs as a
/// switch (M-OPT-026).
const LOCK_ENVELOPES: &str = "song.set_lock_envelopes";

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
    /// Ask where to write the Export Audio/Video dialog's file, in a save
    /// dialog that suggests `file_name`, then run `export.start` with the
    /// path; cancelling goes back to the Export dialog (the export spec,
    /// R40).
    PickExportPath {
        /// The suggested name: the set's, with the file type's extension.
        file_name: String,
        /// The folder the dialog opens in, if there is one.
        folder: Option<PathBuf>,
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
    /// Ask for a folder, then run `settings.set_temporary_folder` with it
    /// (WP-4.20).
    PickTemporaryFolder,
    /// Ask for a folder, then run `settings.set_user_library` with it
    /// (browser R24).
    PickUserLibrary,
    /// Ask for a program, then run `settings.set_sample_editor` with it
    /// (WP-4.10b).
    PickSampleEditor,
    /// Ask for a folder, then run `file_manager.set_search_folder` with it
    /// (file-management R14).
    PickSearchFolder,
    /// Ask where to save the set's plug-in list, then run
    /// `plugin.export_list` there (IMP-29, WP-6.41).
    PickPluginListPath {
        /// The file name the dialog suggests.
        default_name: String,
    },
    /// Start renaming a device in its title bar, where the person types the
    /// new name; Return runs `device.rename` with it (WP-2.4).
    RenameDevice(bl_model::DeviceId),
    /// Start renaming a chain in its row of the chain list; Return runs
    /// `chain.rename` with the new name (WP-5.2).
    RenameChain(bl_model::ChainId),
    /// Start renaming a macro in its Macro Controls slot; Return runs
    /// `rack.rename_macro` with the new name (WP-5.4).
    RenameMacro(bl_model::DeviceId, u8),
    /// Start renaming a variation in its Macro Variations row; Return runs
    /// `rack.rename_variation` with the new name (WP-5.5).
    RenameVariation(bl_model::DeviceId, usize),
    /// Start renaming a track in its title bar, where the person types the
    /// new name; Return runs `track.rename` with it (WP-4.22).
    RenameTrack(TrackId),
    /// Start renaming a locator in the Arrangement's scrub area, where the
    /// person types the new name; Return runs `locator.rename` with it
    /// (WP-6.24; the time editing spec, R8).
    RenameLocator(bl_model::LocatorId),
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

/// What the browser's selection loads into the Device View.
enum DevicesLoad {
    /// A preset file.
    Preset(PathBuf),
    /// A device listed under a label.
    Device(browser::BrowserDevice),
}

/// A gesture under way: what the selection was when it began, and the last
/// command that ran, to fall back on when a frame's command is refused.
#[derive(Debug)]
struct Gesture {
    view: ArrangementView,
    clip_view: ClipView,
    last: Option<Invocation>,
    /// The step every later frame runs first, from where the gesture began:
    /// the warp marker a pseudo-warp marker's drag makes, which the frames
    /// that follow then move (sample editor R8).
    base: Option<Invocation>,
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
    /// The name a set that has never been saved shows, until it is saved
    /// (the DAWproject spec, R11).
    pending_name: Option<String>,
    /// The report of the last DAWproject import, while the set the import
    /// made is open (R12).
    import_report: Option<bl_shell_spec::dawproject::ImportReport>,
    dirty: bool,
    /// The crash-recovery service (WP-6.62, the crash-recovery spec R21):
    /// it journals the open set's edits, and holds the sets a crash left.
    /// `None` when the shell runs without a recovery folder.
    autosave: Option<Autosave>,
    /// The label the last journalled edit went under, so an edit the
    /// undo history doesn't name keeps one.
    journal_label: String,
    /// The last problem with the journal, and the Status Bar's message
    /// about it (the crash-recovery spec, R27).
    recovery_error: Option<String>,
    recovery_message: Option<String>,
    /// Whether the recovery prompt was deferred (Later), so it doesn't come
    /// back this session (the crash-recovery spec, R22).
    recovery_deferred: bool,
    /// A number that changes when the set is replaced, so the journal
    /// doesn't hold the new set as an edit of the old one (the
    /// crash-recovery spec, R2).
    editor_generation: u64,
    store: Box<dyn SetStore>,
    audio: AudioHost,
    /// The MIDI ports and the computer MIDI keyboard (WP-2.2).
    midi: MidiHost,
    /// When the MIDI ports were last listed, so [`Shell::poll`] lists them
    /// again every few seconds and a replugged device shows.
    midi_listed: std::time::Instant,
    /// The counts of messages that reached tracks' inputs and left for
    /// ports when [`Shell::poll`] last looked, and when each last changed
    /// while the indicator still lit (external-devices R7).
    midi_seen: (u64, u64),
    midi_lit: [Option<std::time::Instant>; 2],
    /// The chord a Chord device's Learn is hearing.
    chord_learn: crate::chord_learn::ChordLearn,
    /// The keymap in use: Live 12's shortcuts with the person's own layered
    /// over them (the custom-shortcuts spec, R3).
    keymap: crate::keymap::Keymap,
    /// The person's own shortcuts, as `keymap.toml` holds them.
    user_keymap: crate::keymap::UserKeymap,
    /// The number of times the keymap changed, so a host knows to rebuild
    /// the keymap it matches keys against.
    keymap_revision: u64,
    prefs: Preferences,
    config_dir: Option<PathBuf>,
    main_view: MainView,
    browser: bool,
    /// Whether the Info View shows (L-INFO-001): view state, never part of
    /// the set, like the browser's.
    info_view: bool,
    full_screen: bool,
    settings_open: bool,
    /// The themes the Theme chooser offers, which the host reads from the
    /// theme files (themes R2, R9), and whether the operating system is in
    /// its dark mode, which Follow System draws (themes R3).
    themes: Vec<String>,
    system_dark: bool,
    focused_window: WindowKind,
    focused_area: Area,
    /// The track whose control has the keyboard focus (WP-1.14).
    focused_track: Option<TrackId>,
    /// Whether a device's value control (a knob or a number box) has the
    /// keyboard, so its keys go to it (WP-2.5).
    value_focused: bool,
    effects: Vec<Effect>,
    /// The state of the real-time render an export with external hardware
    /// runs, which the app's dialog shows and its buttons set (WP-5.35).
    realtime: crate::realtime::Session,
    message: Option<Message>,
    /// What the pointer is over says about itself, while there's no message.
    hint: Option<String>,
    revision: u64,
    agent_status: AgentServerStatus,
    /// Where the agent server's tokens are kept: the OS keychain in the
    /// app, an in-memory one in tests, or nowhere until one is given, when
    /// they last only as long as the shell ([`Shell::use_agent_keychain`]).
    agent_keychain: Option<Arc<dyn Keychain>>,
    /// The tokens as the keychain last held them, so they're written only
    /// when they change.
    agent_tokens_stored: Option<agent::StoredTokens>,
    /// Whether the preferences came with tokens in them, from a file
    /// written before the tokens moved to the keychain.
    agent_tokens_in_file: bool,
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
    /// Whether the assistant's panel is open (WP-2.23, part 3): view state,
    /// never part of the set, like the browser's. Where it sits and how
    /// wide are in the preferences.
    assistant_open: bool,
    /// The panel's conversation, which a turn fills while it runs.
    assistant_conversation: crate::assistant::SharedConversation,
    /// The panel's Stop button, which the turn running looks at between
    /// calls.
    assistant_stop_flag: crate::assistant::StopFlag,
    /// Whether the Undo History panel shows (WP-6.28): view state, never
    /// part of the set, like the browser's.
    undo_history_open: bool,
    /// What the Arrangement View shows (WP-1.8a).
    arrangement: ArrangementView,
    /// What the Clip View shows (WP-2.3): whether it's open, Draw Mode, and
    /// the notes selected in its MIDI Note Editor.
    clip_view: ClipView,
    /// The File Manager (WP-6.63): whether it shows, which files it shows,
    /// and the Automatic Search's switches. View state, never part of the
    /// set, like the browser's.
    file_manager: FileManager,
    /// The search field (WP-6.42): view state, never saved.
    search: crate::search::Field,
    /// The list of the set's plug-ins (IMP-29, WP-6.41): whether it shows
    /// in the detail area's place, and whether Show Only Missing is on.
    plugin_list: plugin_list::PluginList,
    /// A number that changes whenever the document may have changed.
    document_revision: u64,
    /// The browser (WP-1.10).
    browser_view: BrowserView,
    /// The media the set's clips play, which the browser fills and the
    /// Arrangement's waveforms read.
    media: Arc<MediaStore>,
    /// Processed files being written, with the edits that land when each is
    /// whole (WP-4.20).
    processed: Vec<processed_files::Pending>,
    /// Stem separation's download, separation, and dialog (IMP-11).
    stems: stems::StemsState,
    /// The Export Audio/Video dialog and the export under way (WP-6.71).
    export: export_dialog::ExportState,
    /// The autosampler at work (WP-5.16, IMP-27).
    autosample: Option<autosample::Job>,
    /// The autosampler's dialog, open for an instrument.
    autosample_dialog: Option<autosample::Dialog>,
    /// What the last autosample made, for the command's result.
    autosample_result_value: Option<bl_shell_spec::autosampler::Autosampled>,
    /// The Slice to New MIDI Track dialog (WP-4.16): its clip and the
    /// slicing presets it offers. Never part of the set.
    slice_dialog: Option<bl_shell_spec::slice::SliceDialogStatus>,
    /// The sample being auditioned from a click in a display (WP-5.15,
    /// R54): its device and layer, while the button is down. Not saved.
    auditioning: Option<(bl_model::DeviceId, Option<bl_model::LayerId>)>,
    /// How files are analysed (WP-4.3): in memory only until the app says
    /// otherwise.
    analysis_options: bl_analysis::cache::AnalysisOptions,
    /// The analysis cache, made when first used.
    analysis: Option<bl_analysis::cache::AnalysisCache>,
    /// How the peaks of the files an export writes are kept (WP-6.71, R34):
    /// in memory only until the app says otherwise.
    peak_options: bl_media::PeakCacheOptions,
    /// The peak cache for those files, made when first used.
    peaks: Option<bl_media::PeakCache>,
    /// The files whose analysis has been started this session.
    analysed: std::collections::HashSet<bl_media::ContentHash>,
    /// The files opened in the external sample editor, and the content they
    /// held then (WP-4.10b): the shell looks at them again when Beatlayer
    /// comes to the front.
    editing: Vec<audio_panel::Editing>,
    /// The Arrangement's clipboard (WP-1.8b), kept across sets as Live
    /// keeps it, and never part of a set.
    clipboard: Option<Clipboard>,
    /// The clipboard of copied envelope shapes (WP-6.25), kept across sets
    /// as Live keeps it, and never part of a set.
    envelope_clipboard: Option<EnvelopeClipboard>,
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
    /// The frames the Session slots that play launched on, as the shell
    /// last saw them, so a clip that starts playing is told from one that
    /// has been playing (the scales spec, R11).
    launched: BTreeMap<(TrackId, bl_model::SceneId), Frames>,
    /// The Arrangement clip the playhead was in, as the shell last saw it,
    /// so the playhead entering a clip is told from its staying in one
    /// (the scales spec, R11).
    playing_clip: Option<bl_model::ClipId>,
    /// The tempo and time signature a launched scene put in place of the
    /// set's, as the engine last published them (WP-3.5).
    scene_override: bl_engine::SceneOverride,
    /// The scene launched last since the set opened, which Capture and
    /// Insert Scene goes after.
    last_launched: Option<bl_model::SceneId>,
    session_over: Vec<TrackId>,
    /// The parameters under modulation, as the engine last published them
    /// (WP-5.9).
    modulated: Vec<bl_engine::PlayedParam>,
    /// Whether `modulated` came from a running engine, so it clears when
    /// the engine stops while values a host set stay.
    modulating: bool,
    /// The rack chains the engine found processing in its last block
    /// (WP-5.3), which a rack's Auto Select selects.
    processing: Vec<bl_model::ChainId>,
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
    /// their blocklist, and their cache. Shared with the background scan
    /// (WP-2.9b).
    scanner: Arc<Scanner>,
    /// The background plug-in scan under way, if any (WP-2.9b).
    /// It says it's done just before it wakes the app, so the wake finds
    /// it done.
    plugin_scan: Option<(
        std::thread::JoinHandle<()>,
        Arc<std::sync::atomic::AtomicBool>,
    )>,
    /// Another background scan asked for while one ran: whether it ignores
    /// the cache.
    plugin_scan_again: Option<bool>,
    /// Whether the app scans plug-ins by itself: at start, and when a
    /// Plug-In Sources switch turns on ([`Shell::start_plugin_scan`]).
    auto_scan: bool,
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
    /// What the last recording that ended came to, for `transport.state`.
    last_recording: Option<RecordingReport>,
    /// How many recordings this session has started: the take number in
    /// their files' names, so two in one second don't share a file.
    recorded_takes: u32,
    /// The Session recording under way, beside `recording` (WP-3.11).
    session_run: Option<session_recording::SessionRun>,
    /// The notes monitoring MIDI tracks received, for Capture MIDI
    /// (WP-3.14).
    capture: bl_engine::capture::Buffer,
    /// The control moves of the automation recording under way (WP-2.12).
    automation_take: crate::automation_record::Take,
    /// The control moves of the Session recording under way (WP-3.12).
    session_automation: crate::automation_record::SessionTake,
    /// The song frame the Session recording's take began on, once it has.
    session_take_start: Option<Frames>,
    /// What the Session clips that played during the recording under way
    /// were (WP-3.13).
    session_watch: crate::session_capture::Watch,
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

/// The automation editing command an Arrangement editing command runs as,
/// when the time selection is in automation lanes (the automation editing
/// spec, R5).
fn automation_command(id: &str, view: &ArrangementView) -> Option<&'static str> {
    let in_lanes = view
        .time_selection_lanes()
        .iter()
        .any(|lane| lane.target().is_some());
    if !in_lanes {
        return None;
    }
    match id {
        edit_ids::COPY => Some(automation_ids::COPY),
        edit_ids::CUT => Some(automation_ids::CUT),
        edit_ids::PASTE => Some(automation_ids::PASTE),
        edit_ids::DUPLICATE => Some(automation_ids::DUPLICATE),
        edit_ids::DELETE => Some(automation_ids::DELETE_RANGE),
        _ => None,
    }
}

/// Whether `id` is an automation edit a drag on an automation lane, or on a
/// clip's envelope in the Clip View, runs as one undo step (WP-2.12,
/// WP-3.16).
fn is_automation_edit(id: &str) -> bool {
    matches!(
        id,
        "automation.set_breakpoint"
            | "automation.set_curve"
            | "automation.move_breakpoint"
            | "automation.delete_breakpoint"
            | "clip.set_envelope_breakpoint"
            | "clip.set_envelope_curve"
            | "clip.move_envelope_breakpoint"
            | "clip.delete_envelope_breakpoint"
    )
}

/// The undo step a drag on an automation lane records, named as Live's undo
/// history names that work.
fn automation_label(id: &str) -> &'static str {
    match id {
        "automation.set_curve" => "Curve Automation Segment",
        "automation.move_breakpoint" => "Move Automation Breakpoint",
        "automation.delete_breakpoint" => "Delete Automation Breakpoint",
        "clip.set_envelope_curve" => "Curve Envelope Segment",
        "clip.move_envelope_breakpoint" => "Move Envelope Breakpoint",
        "clip.delete_envelope_breakpoint" => "Delete Envelope Breakpoint",
        "clip.set_envelope_breakpoint" => "Add Envelope Breakpoint",
        _ => "Add Automation Breakpoint",
    }
}

/// Whether `id` is one of the document's clip commands the Clip View's
/// drags run: the marker strip's (`clip.set_midi_region`,
/// `clip.set_midi_loop` and the audio clips' `clip.set_region` and
/// `clip.set_loop`) and the Sample Editor's (warp markers and transients).
/// They are document commands, but the drags run them as gestures, so moving
/// a marker or the loop brace however far is one undo step (piano-roll R11;
/// sample editor R7).
fn clip_marker_command(id: &str) -> bool {
    matches!(
        id,
        model::SET_MIDI_REGION
            | model::SET_MIDI_LOOP
            | "clip.set_region"
            | "clip.set_loop"
            | "clip.insert_warp_marker"
            | "clip.move_warp_markers"
            | "clip.shift_warp_marker"
            | "clip.move_transient"
    )
}

/// Whether `id` is one of the Arrangement's own clip drags, which the
/// document runs per frame as gestures: stretching a warped clip's edge (the
/// warping spec, R47), sliding a clip's contents (R54) and dragging a fade's
/// handles (the audio clips spec, R27), each one undo step however far the drag
/// goes.
fn arrangement_clip_command(id: &str) -> bool {
    matches!(
        id,
        "clip.stretch" | "clip.slide_contents" | "clip.set_fades" | "arrangement.set_crossfade"
    )
}

/// Whether `id` is an edit the Sample Editor makes to an audio clip's
/// markers, transients or region.
fn sample_edit_command(id: &str) -> bool {
    clip_marker_command(id)
        || matches!(
            id,
            "clip.delete_warp_markers"
                | "clip.insert_warp_markers"
                | "clip.insert_transient"
                | "clip.delete_transients"
                | "clip.reset_transients"
        )
}

/// The name of the undo step a marker-strip or Sample Editor gesture makes.
fn marker_label(id: &str) -> &'static str {
    match id {
        "clip.stretch" => "Stretch clip",
        "clip.slide_contents" => "Slide clip contents",
        "clip.set_fades" | "arrangement.set_crossfade" => "Change Fade",
        model::SET_MIDI_REGION | "clip.set_region" => "Change clip region",
        model::SET_MIDI_LOOP | "clip.set_loop" => "Change clip loop",
        "clip.insert_warp_marker" | "clip.move_warp_markers" => "Move warp markers",
        "clip.shift_warp_marker" => "Shift warp marker",
        "clip.move_transient" => "Move transient",
        _ => "Change clip",
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
/// A recorded file's name without its extension: its clips' name.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A fresh editor on the same document, with an empty undo history.
fn snapshot(editor: &Editor) -> Editor {
    Editor::from_document(editor.document().clone(), editor.ids())
}

/// How often [`Shell::poll`] lists the MIDI ports again.
const MIDI_RELIST: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a MIDI indicator stays lit after a message.
const MIDI_LAMP: std::time::Duration = std::time::Duration::from_millis(120);

/// The macro edit a document command makes, so the shell writes the values
/// its law gives each mapped parameter in the same undo step (macros spec,
/// R13).
enum MacroEdit {
    /// A macro moved by `device.set_param`, named in the undo step.
    Macro {
        rack: bl_model::DeviceId,
        number: u8,
        label: String,
    },
    /// A new mapping: `first` holds the target's value before it, when the
    /// macro had no mapping yet (R6).
    Mapping {
        rack: bl_model::DeviceId,
        number: u8,
        target: bl_model::MacroTarget,
        first: Option<f64>,
    },
    /// A mapping's range changed (R9).
    Target {
        rack: bl_model::DeviceId,
        target: bl_model::MacroTarget,
    },
    /// Randomize set the mapped macros (R15).
    Randomize { rack: bl_model::DeviceId },
    /// A macro's style changed, so it may have moved to another step
    /// (R21, R22).
    Style {
        rack: bl_model::DeviceId,
        number: u8,
    },
    /// A variation launch set the macros (R27).
    Variation { rack: bl_model::DeviceId },
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
        // The recovery commands (WP-6.62, the crash-recovery spec, R26),
        // which run against the app's own recovery.
        recovery_ids::register(&mut registry).expect("the recovery command IDs are unique");
        // The transport's commands (WP-1.5), which the shell sends to the
        // engine it plays the set through (WP-1.18).
        engine_transport::register(&mut registry).expect("the transport's command IDs are unique");
        // The Session View's launching and stopping (WP-3.2), which the
        // shell sends to the engine too.
        engine_session::register(&mut registry).expect("the Session View's command IDs are unique");
        debug_assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        audio.set_tone(prefs.tone);
        // MIDI input (WP-2.2). No MIDI system until the app gives one
        // ([`Shell::use_midi_backend`]), so a headless shell touches no
        // hardware. The queue has no engine yet; each engine attaches its own.
        let (midi_sender, _) = bl_midi_io::input_queue(1);
        audio.set_midi(midi_sender.clone());
        let midi_output = bl_midi_io::OutputBus::new();
        audio.set_midi_output(midi_output.clone());
        let midi = MidiHost::new(
            Box::new(bl_midi_io::NullBackend),
            midi_sender,
            midi_output,
            &prefs.midi,
        );
        let mut browser_view = BrowserView::new(&prefs.browser, Waker::default());
        // Spec: browser-tags R1. The browser's database holds the index of
        // the places' files, the tags, and the items that carry them.
        if let Some(dir) = &config_dir {
            browser_view.open_database(dir.join(crate::settings::BROWSER_DATABASE_FILE));
        }
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
        let scanner = Arc::new(match &config_dir {
            Some(dir) => Scanner::new().with_store(dir.join(crate::settings::PLUGIN_SCAN_FILE)),
            None => Scanner::new(),
        });
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
            processed: Vec::new(),
            stems: stems::StemsState::default(),
            export: export_dialog::ExportState::default(),
            autosample: None,
            autosample_dialog: None,
            autosample_result_value: None,
            slice_dialog: None,
            auditioning: None,
            analysis_options: bl_analysis::cache::AnalysisOptions::memory_only(),
            analysis: None,
            peak_options: bl_media::PeakCacheOptions::memory_only(),
            peaks: None,
            analysed: std::collections::HashSet::new(),
            editing: Vec::new(),
            registry,
            editor,
            saved,
            set_path: None,
            pending_name: None,
            import_report: None,
            dirty: false,
            autosave: None,
            journal_label: String::new(),
            recovery_error: None,
            recovery_message: None,
            recovery_deferred: false,
            editor_generation: 0,
            store,
            audio,
            midi,
            midi_listed: std::time::Instant::now(),
            midi_seen: (0, 0),
            midi_lit: [None; 2],
            chord_learn: crate::chord_learn::ChordLearn::default(),
            keymap: crate::keymap::Keymap::live12(),
            user_keymap: crate::keymap::UserKeymap::default(),
            keymap_revision: 0,
            settings_open: false,
            themes: vec![BUNDLED_THEME.to_owned()],
            system_dark: true,
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
            realtime: crate::realtime::Session::default(),
            message: None,
            hint: None,
            revision: 0,
            agent_status: AgentServerStatus::default(),
            agent_keychain: None,
            agent_tokens_stored: None,
            agent_tokens_in_file: false,
            keychain: Arc::new(OsKeychain::new()),
            assistant_key_set: false,
            assistant_log: Vec::new(),
            assistant_log_open: false,
            assistant_open: false,
            assistant_conversation: Arc::default(),
            assistant_stop_flag: Arc::default(),
            undo_history_open: false,
            arrangement: ArrangementView::new(),
            clip_view: ClipView::default(),
            file_manager: FileManager::default(),
            search: crate::search::Field::default(),
            plugin_list: plugin_list::PluginList::default(),
            document_revision: 0,
            clipboard: None,
            envelope_clipboard: None,
            edit_gesture: None,
            au,
            seen_insert_marker: None,
            gesture: false,
            mixer: MixerView::new(),
            session: SessionView::new(),
            session_states: Vec::new(),
            launched: BTreeMap::new(),
            playing_clip: None,
            scene_override: bl_engine::SceneOverride::default(),
            last_launched: None,
            session_over: Vec::new(),
            modulated: Vec::new(),
            processing: Vec::new(),
            modulating: false,
            meters: Vec::new(),
            metering: false,
            previews: crate::perception::Previews::default(),
            recording: None,
            last_recording: None,
            recorded_takes: 0,
            session_run: None,
            capture: bl_engine::capture::Buffer::default(),
            automation_take: crate::automation_record::Take::default(),
            session_automation: crate::automation_record::SessionTake::default(),
            session_take_start: None,
            session_watch: crate::session_capture::Watch::default(),
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
            plugin_scan: None,
            plugin_scan_again: None,
            auto_scan: false,
            kit,
            device_commands: std::collections::BTreeMap::new(),
            device_commands_key: Vec::new(),
            plugins_generation: 0,
        };
        shell.agent_tokens_in_file = !agent::StoredTokens::of(&shell.prefs.agent).is_empty();
        shell.give_agents_tokens();
        shell.publish_plugins();
        shell.load_user_keymap();
        // Spec: mixer-view R21. The return tracks scroll with the tracks as
        // the application setting says, in the mixers and in the
        // Arrangement's lanes.
        shell
            .mixer
            .set_return_tracks_scroll(shell.prefs.return_tracks_scroll);
        shell
            .arrangement
            .set_returns_scroll(shell.prefs.return_tracks_scroll);
        shell
    }

    /// What the plug-in scanner has found, with the browser's hidden and
    /// format filters applied (WP-2.9, IMP-07), pushed into the browser's
    /// Plug-Ins label.
    fn publish_plugins(&mut self) {
        let formats = crate::plugins::listed_formats(&self.prefs);
        let mut plugins = self.scanner.visible(
            self.prefs.show_hidden_plugins,
            &self.prefs.hidden_plugins,
            self.prefs.plugin_format_preference,
        );
        // Spec: plugin-scanning R12. A format is listed only while its
        // Plug-In Sources switch is on, as in Live.
        plugins.retain(|p| formats.contains(&p.format));
        self.browser_view.set_plugins(plugins);
    }

    /// Starts scanning plug-ins by itself, as Live does at launch (WP-2.9b;
    /// the plug-in scanning spec, R13): a scan of the system folders of the
    /// formats switched on runs now in the background, answered from the
    /// cache for unchanged files, and again whenever a Plug-In Sources
    /// switch turns on. The browser lists what the cache knew until it
    /// finishes. The app calls it once its window is up; tests and tools
    /// that don't, scan only when asked.
    pub fn start_plugin_scan(&mut self) {
        self.auto_scan = true;
        self.spawn_plugin_scan(false);
    }

    /// Whether a background plug-in scan is under way.
    #[must_use]
    pub fn plugin_scan_running(&self) -> bool {
        self.plugin_scan.is_some()
    }

    fn spawn_plugin_scan(&mut self, fresh: bool) {
        if self.plugin_scan.is_some() {
            self.plugin_scan_again = Some(self.plugin_scan_again.unwrap_or(false) | fresh);
            return;
        }
        let scanner = Arc::clone(&self.scanner);
        let formats = crate::plugins::listed_formats(&self.prefs);
        let waker = self.browser_view.waker().clone();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished = Arc::clone(&done);
        let spawned = std::thread::Builder::new()
            .name("plugin-scan".to_owned())
            .spawn(move || {
                let _ = crate::plugins::scan_report(
                    &scanner,
                    None,
                    fresh,
                    &formats,
                    &std::collections::BTreeSet::new(),
                );
                finished.store(true, std::sync::atomic::Ordering::Release);
                waker.wake();
            });
        self.plugin_scan = spawned.ok().map(|handle| (handle, done));
    }

    /// Takes a finished background scan's results into the browser, and
    /// starts the next one if one was asked for. Returns whether the
    /// browser changed.
    fn poll_plugin_scan(&mut self) -> bool {
        if !self
            .plugin_scan
            .as_ref()
            .is_some_and(|(_, done)| done.load(std::sync::atomic::Ordering::Acquire))
        {
            return false;
        }
        if let Some((scan, _)) = self.plugin_scan.take() {
            let _ = scan.join();
        }
        self.publish_plugins();
        if let Some(fresh) = self.plugin_scan_again.take() {
            self.spawn_plugin_scan(fresh);
        }
        true
    }

    /// Waits for the background plug-in scan to finish, up to `timeout`,
    /// for tests and tools. Returns whether it finished.
    pub fn wait_for_plugin_scan(&mut self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            self.poll_plugin_scan();
            if self.plugin_scan.is_none() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Makes each allowed agent that has no token of its own one (an agent
    /// allowed before agents had them, or one whose token the keychain
    /// lost), and the server one while it's on (WP-1.15b). The caller keeps
    /// them.
    fn give_agents_tokens(&mut self) {
        for client in &mut self.prefs.agent.clients {
            if client.token.is_empty()
                && let Ok(token) = agent::new_token()
            {
                client.token = token.into();
            }
        }
        if self.prefs.agent.server
            && self.prefs.agent.token.is_empty()
            && let Ok(token) = agent::new_token()
        {
            self.prefs.agent.token = token.into();
        }
    }

    /// Keeps the agent server's tokens in `keychain` from now on: the OS
    /// keychain in the app, an in-memory one in tests. Reads the tokens it
    /// holds, moves any that the preferences' file still had into it and
    /// rewrites the file without them, and makes the ones still missing.
    ///
    /// If the keychain can't be read or written, the Status Bar says so and
    /// the tokens last only until Beatlayer quits; the agents keep being
    /// allowed, and are given new tokens at the next start.
    pub fn use_agent_keychain(&mut self, keychain: Arc<dyn Keychain>) {
        let stored = match keychain.read(agent::KEYCHAIN_ACCOUNT) {
            Ok(Some(text)) => agent::StoredTokens::parse(&text),
            Ok(None) => Ok(agent::StoredTokens::default()),
            Err(error) => Err(error.to_string()),
        };
        self.agent_keychain = Some(keychain);
        match stored {
            Ok(stored) => {
                stored.apply(&mut self.prefs.agent);
                self.agent_tokens_stored = Some(stored);
            }
            Err(error) => {
                self.agent_tokens_stored = None;
                self.set_message(
                    format!("Couldn't read the agent tokens from the keychain: {error}"),
                    true,
                );
            }
        }
        self.give_agents_tokens();
        if self.store_agent_tokens() && self.agent_tokens_in_file {
            // Only once the keychain has them: the file loses its copy.
            self.agent_tokens_in_file = false;
            self.save_preferences();
        }
    }

    /// Writes the agent server's tokens to the keychain if they changed
    /// since it was last read or written. Returns whether the keychain now
    /// holds them; a failure shows in the Status Bar.
    fn store_agent_tokens(&mut self) -> bool {
        let Some(keychain) = &self.agent_keychain else {
            return false;
        };
        let tokens = agent::StoredTokens::of(&self.prefs.agent);
        if self.agent_tokens_stored.as_ref() == Some(&tokens) {
            return true;
        }
        // No tokens, and a keychain that couldn't be read: nothing to keep,
        // and nothing worth a message about.
        if tokens.is_empty() && self.agent_tokens_stored.is_none() {
            return true;
        }
        let written = if tokens.is_empty() {
            keychain.delete(agent::KEYCHAIN_ACCOUNT)
        } else {
            keychain.write(agent::KEYCHAIN_ACCOUNT, &tokens.to_text())
        };
        match written {
            Ok(()) => {
                self.agent_tokens_stored = Some(tokens);
                true
            }
            Err(error) => {
                self.set_message(
                    format!(
                        "Couldn't keep the agent tokens in the keychain: {error}. They last \
                         until Beatlayer quits."
                    ),
                    true,
                );
                false
            }
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

    /// Runs one of the recovery commands against the app's own recovery
    /// (the crash-recovery spec, R26): `set.list_recoverable`,
    /// `set.recover`, `set.discard_recovery` and
    /// `settings.set_continuous_save`.
    fn run_recovery(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if id == recovery_ids::SET_CONTINUOUS_SAVE {
            let args: recovery_ids::ContinuousSaveArgs =
                serde_json::from_value(args).map_err(|error| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: error.to_string(),
                })?;
            self.prefs.save_continuously = args.on;
            self.save_preferences();
            if let Some(autosave) = self.autosave.as_mut() {
                autosave.set_continuous(args.on);
            }
            return Ok(to_value(&recovery_ids::ContinuousSave { on: args.on }));
        }
        let ran = {
            let Some(autosave) = self.autosave.as_mut() else {
                return Err(ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "crash recovery isn't running in this session".to_owned(),
                });
            };
            recovery_ids::run(autosave, id, args).map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: error.to_string(),
            })?
        };
        if let Some(recovered) = ran.recovered {
            self.open_recovered(recovered);
        }
        Ok(ran.result)
    }

    /// Opens a recovered set in place of the open one (the crash-recovery
    /// spec, R3): as it was after its last journalled edit, unsaved, with
    /// an empty undo history.
    fn open_recovered(&mut self, recovered: Recovered) {
        let Recovered { set, project, .. } = recovered;
        self.set_replaced();
        self.editor = project.editor();
        self.set_path = set.as_ref().map(|set| set.path().to_owned());
        // Discard Changes goes back to the set file, or, for a set never
        // saved, to the set a new one starts from.
        self.saved = new_set_from(
            self.set_path
                .as_deref()
                .or(self.prefs.default_set.as_deref()),
        );
        self.dirty = true;
        self.store.adopt(set, project);
        // The views saved beside the song, and its files, as `set.open`
        // takes them.
        self.devices.set_windows(self.store.windows());
        self.devices.set_rack_views(self.store.rack_views());
        self.devices
            .set_multisampler_views(self.store.multisampler_views());
        self.clip_view.set_folds(self.store.folds());
        for (hash, path, size) in self.store.media_files() {
            self.media
                .add_reference(bl_media::ContentHash::from_bytes(hash.0), path, size);
        }
        self.arrangement.reset();
        self.clip_view.forget_selection();
        self.mixer.reset();
        self.session.reset();
        self.new_set_transport();
        self.document_changed();
        self.set_file_changed();
        self.touch();
    }

    /// Tells the journal the set was saved, so it retires it: the set file
    /// holds every edit (the crash-recovery spec, R6).
    fn journal_saved(&mut self) {
        let set = self.store.set_file().cloned();
        if let (Some(autosave), Some(set)) = (self.autosave.as_mut(), set.as_ref()) {
            autosave.saved(set);
        }
    }

    /// Closes the journal: the set was saved, or its changes were thrown
    /// away, so nothing is offered at the next launch (the crash-recovery
    /// spec, R6).
    fn close_journal(&mut self) {
        if let Some(autosave) = self.autosave.as_mut() {
            let _ = autosave.close_set();
        }
    }

    /// Opens the audio device, if the engine is on. A failure shows in the
    /// Status Bar with a way to the Audio settings.
    pub fn start_audio(&mut self) {
        if let Err(error) = self.audio.start() {
            self.audio_failed(&error);
        }
        self.touch();
    }

    /// Runs crash recovery in `dir` (WP-6.62, the crash-recovery spec R21):
    /// the open set's edits are journalled there, and the sets a crash left
    /// are offered at the next launch. The app calls it at start-up; a
    /// headless run with a recovery folder of its own calls it too.
    ///
    /// # Errors
    ///
    /// A message saying why the recovery folder couldn't be used. The shell
    /// goes on working without it.
    pub fn use_recovery(&mut self, dir: PathBuf) -> Result<(), String> {
        let mut autosave = Autosave::start(&dir).map_err(|error| error.to_string())?;
        autosave.set_continuous(self.prefs.save_continuously);
        self.autosave = Some(autosave);
        self.recovery_error = None;
        self.begin_journal();
        self.check_recovery();
        Ok(())
    }

    /// The sets a crash left unsaved, newest first (the crash-recovery
    /// spec, R2). They are offered at the next launch.
    #[must_use]
    pub fn recovery_offers(&self) -> Vec<Recoverable> {
        self.autosave
            .as_ref()
            .map_or_else(Vec::new, Autosave::offers)
    }

    /// Whether the recovery prompt is open: there is something to offer,
    /// and it hasn't been deferred (the crash-recovery spec, R22).
    #[must_use]
    pub fn recovery_prompt_open(&self) -> bool {
        !self.recovery_deferred && !self.recovery_offers().is_empty()
    }

    /// Whether Save Continuously is on (the crash-recovery spec, R8).
    #[must_use]
    pub fn save_continuously(&self) -> bool {
        self.prefs.save_continuously
    }

    /// Starts the journal for the open set, so that its edits are kept for
    /// recovery (the crash-recovery spec, R21).
    fn begin_journal(&mut self) {
        let Some(autosave) = self.autosave.as_mut() else {
            return;
        };
        let set = self.store.set_file().cloned();
        if let Err(error) = autosave.begin(set.as_ref()) {
            self.recovery_error = Some(error.to_string());
        }
    }

    /// Journals the open set after a finished edit, so that the set can be
    /// recovered after a crash (the crash-recovery spec, R1 and R21).
    /// `label` names the edit as the undo history does; without one, the
    /// newest undo step's name is used.
    fn journal_edit(&mut self, label: Option<&str>) {
        if self.editor.in_transaction() {
            // The edit isn't finished: the transaction is still open.
            return;
        }
        if self.autosave.is_none() {
            return;
        }
        let label = label
            .map(str::to_owned)
            .or_else(|| self.editor.undo_history().next().map(str::to_owned))
            .unwrap_or_else(|| self.journal_label.clone());
        self.journal_label = label.clone();
        let project = self.store.project(&self.editor);
        if let Some(autosave) = self.autosave.as_mut() {
            autosave.edited(&project, label);
        }
        self.check_recovery();
    }

    /// Waits until every edit of the open set is on disk (the crash-recovery
    /// spec, R1). The app doesn't need it: the journal is written within a
    /// second on a thread of its own. A test that kills the app uses it, so
    /// that the edit it made is on disk before the kill.
    pub fn flush_recovery(&self) -> Result<(), String> {
        let Some(autosave) = self.autosave.as_ref() else {
            return Ok(());
        };
        autosave.flush().map_err(|error| error.to_string())
    }

    /// Tells the Status Bar about the journal when it can't be written, once
    /// (the crash-recovery spec, R27). The app goes on working, and the
    /// message goes when a write succeeds.
    fn check_recovery(&mut self) {
        let problem = self.recovery_problem();
        self.recovery_error = problem;
        let Some(problem) = self.recovery_error.clone() else {
            if let Some(message) = self.recovery_message.take()
                && self
                    .message
                    .as_ref()
                    .is_some_and(|shown| shown.text == message)
            {
                self.clear_message();
            }
            return;
        };
        let message = format!("Edits aren't being kept for recovery: {problem}");
        self.recovery_message = Some(message.clone());
        // The message says once, and stays until a write succeeds. Another
        // message in the Status Bar keeps its place until there is room.
        if self.message.is_none() {
            self.set_message(message, true);
        }
    }

    /// What's wrong with the journal now, if anything: the last write's
    /// error, or the reason the session couldn't start.
    fn recovery_problem(&mut self) -> Option<String> {
        let autosave = self.autosave.as_mut()?;
        if let Some(error) = autosave.status().error {
            return Some(error);
        }
        if autosave.status().session.is_none() {
            // The session never started: try again, so a folder that is
            // writable now is used (the crash-recovery spec, R27).
            if let Err(error) = autosave.retry() {
                return Some(error.to_string());
            }
        }
        autosave.status().error
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
            // Spec: crash-recovery R1. The finished edit is journalled.
            self.journal_edit(None);
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

    /// The panel's conversation, oldest first.
    #[must_use]
    pub fn assistant_rows(&self) -> Vec<assistant_ids::Row> {
        crate::assistant::lock(&self.assistant_conversation)
            .rows
            .clone()
    }

    /// Whether an assistant turn is running.
    #[must_use]
    pub fn assistant_running(&self) -> bool {
        crate::assistant::lock(&self.assistant_conversation).running
    }

    /// The message the person sent that no turn has taken yet, which the
    /// host answers by running a turn ([`run_turn`]).
    pub fn take_assistant_prompt(&mut self) -> Option<String> {
        crate::assistant::lock(&self.assistant_conversation)
            .pending
            .take()
    }

    /// Adds the person's message to the conversation, to be taken by
    /// [`Shell::take_assistant_prompt`].
    pub(crate) fn assistant_send(&mut self, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("the message is empty".to_owned());
        }
        let mut conversation = crate::assistant::lock(&self.assistant_conversation);
        if conversation.running || conversation.pending.is_some() {
            return Err("the assistant is still answering the last message".to_owned());
        }
        conversation
            .rows
            .push(assistant_ids::Row::You(text.to_owned()));
        conversation.pending = Some(text.to_owned());
        // The assistant is at work from the message on, so the panel shows
        // Stop until the turn ends.
        conversation.running = true;
        Ok(())
    }

    /// Moves the keyboard focus to the main view when the pane that had it
    /// closed or floated away, as closing the browser does.
    fn leave_hidden_assistant(&mut self) {
        if self.focused_area == Area::Assistant && self.assistant_docked_width().is_none() {
            self.focused_area = Area::MainView;
        }
    }

    /// Asks the running turn to stop between two calls; a message no turn
    /// has taken yet is withdrawn instead.
    pub(crate) fn assistant_stop(&self) {
        let mut conversation = crate::assistant::lock(&self.assistant_conversation);
        if conversation.pending.take().is_some() {
            conversation.running = false;
            conversation
                .rows
                .push(assistant_ids::Row::Notice("Stopped.".to_owned()));
        } else if conversation.running {
            self.assistant_stop_flag
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// The docked panel's width in points while it is open and docked: the
    /// main window lays itself out beside it (D24).
    #[must_use]
    pub fn assistant_docked_width(&self) -> Option<f32> {
        (self.assistant_open && !self.prefs.assistant_panel.floating)
            .then(|| assistant_ids::clamp_panel_width(self.prefs.assistant_panel.width))
    }

    /// Whether the panel is open in a window of its own.
    #[must_use]
    pub fn assistant_window_open(&self) -> bool {
        self.assistant_open && self.prefs.assistant_panel.floating
    }

    /// The panel's state, for the commands' results.
    #[must_use]
    pub fn assistant_panel_state(&self) -> assistant_ids::AssistantPanelState {
        assistant_ids::AssistantPanelState {
            open: self.assistant_open,
            floating: self.prefs.assistant_panel.floating,
            width: assistant_ids::clamp_panel_width(self.prefs.assistant_panel.width),
            running: self.assistant_running(),
        }
    }

    /// Whether the Undo History panel shows (WP-6.28).
    #[must_use]
    pub fn undo_history_open(&self) -> bool {
        self.undo_history_open
    }

    /// The Undo History panel's state, for the command's result.
    #[must_use]
    pub fn undo_history_state(&self) -> bl_shell_spec::commands::UndoHistoryState {
        let editor = self.editor();
        bl_shell_spec::commands::UndoHistoryState {
            open: self.undo_history_open,
            steps: editor.undo_history().len() + editor.redo_history().len(),
            current: editor.current_index(),
        }
    }

    /// The history's rows, from the top: the steps that can be redone
    /// first, then the ones that can be undone.
    #[must_use]
    pub fn undo_history_rows(&self) -> Vec<bl_model::HistoryStep> {
        self.editor.history()
    }

    /// The bottom row of the Undo History panel: the set's start, which no
    /// step undoes. A set made new reads "New Beatlayer Set", and one
    /// opened from a file "Open Beatlayer Set".
    #[must_use]
    pub fn set_start_label(&self) -> &'static str {
        if self.set_path().is_some() {
            "Open Beatlayer Set"
        } else {
            "New Beatlayer Set"
        }
    }

    /// The flag Stop sets, for a host that runs a turn while it also takes
    /// the person's clicks: setting it ends the turn between two calls, as
    /// `assistant.stop` does.
    #[must_use]
    pub fn assistant_stop_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.assistant_stop_flag)
    }

    /// The conversation and the Stop flag, for [`crate::assistant::run_turn`]
    /// to hold while the shell runs the turn's calls.
    pub(crate) fn assistant_handles(
        &self,
    ) -> (
        crate::assistant::SharedConversation,
        crate::assistant::StopFlag,
    ) {
        (
            Arc::clone(&self.assistant_conversation),
            Arc::clone(&self.assistant_stop_flag),
        )
    }

    /// Tells the window the panel changed.
    pub(crate) fn assistant_touch(&mut self) {
        self.touch();
    }

    /// Gives the call that waits for `command` its result in the panel.
    pub(crate) fn assistant_finish_call(&self, command: &str, outcome: Result<Value, String>) {
        crate::assistant::lock(&self.assistant_conversation).finish_call(command, outcome);
    }

    /// Opens a turn's changes as one undo step named `label`, which the
    /// assistant loop closes with [`Shell::end_assistant_turn`]
    /// (WP-2.23, part 2).
    pub(crate) fn begin_assistant_turn(&mut self, label: &str) {
        self.editor
            .begin_by(bl_model::Maker::Assistant, label.to_owned());
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
        // The extra audio outputs the set's devices offer past their main
        // one, which another track's Audio From lists (WP-5.45).
        let mut device_outputs = std::collections::BTreeMap::new();
        for track in self.editor.document().tracks() {
            for device in track.devices_deep() {
                let outputs = self.devices.outputs(&device.kind);
                if !outputs.is_empty() {
                    device_outputs.insert(device.id, outputs.to_vec());
                }
            }
        }
        crate::routing::Interface {
            inputs,
            outputs,
            midi_inputs: self.midi.input_names(),
            midi_outputs: self.midi.output_names(),
            device_outputs,
        }
    }

    /// Lights the Control Bar's MIDI indicators for a moment when messages
    /// have reached tracks' inputs or left for ports since the last call;
    /// returns whether either changed (midi-io R25; external-devices R7).
    fn track_midi_activity(&mut self) -> bool {
        let now = std::time::Instant::now();
        let seen = (
            self.midi.sender().stats().snapshot().to_tracks,
            self.midi.output().stats().snapshot().sent,
        );
        let counts = [(seen.0, self.midi_seen.0), (seen.1, self.midi_seen.1)];
        let mut changed = false;
        for (lit, (new, old)) in self.midi_lit.iter_mut().zip(counts) {
            if new != old {
                *lit = Some(now);
                changed = true;
            } else if lit.is_some_and(|since| now.duration_since(since) >= MIDI_LAMP) {
                *lit = None;
                changed = true;
            }
        }
        self.midi_seen = seen;
        changed
    }

    /// Whether the Control Bar's MIDI track input and output indicators are
    /// lit: a message reached a track's input, or left for a port, a moment
    /// ago (L-CTRL-026).
    #[must_use]
    pub fn midi_indicators(&self) -> (bool, bool) {
        (self.midi_lit[0].is_some(), self.midi_lit[1].is_some())
    }

    /// Plays MIDI through `backend` from now on: the platform's MIDI system
    /// in the app, the test backend in tests. The ports keep the switches
    /// the preferences hold.
    pub fn use_midi_backend(&mut self, backend: Box<dyn bl_midi_io::MidiBackend>) {
        let sender = self.midi.sender().clone();
        let output = self.midi.output().clone();
        self.midi = MidiHost::new(backend, sender, output, &self.prefs.midi);
    }

    /// Drives the shell's clocks from `clock`: the computer keyboard stamps
    /// its notes with it, and the next audio stream times its blocks from it
    /// (T199). Call it after [`Shell::use_midi_backend`] and before
    /// [`Shell::start_audio`].
    pub fn use_test_clock(&mut self, clock: bl_core::clock::TestClock) {
        self.midi.set_test_clock(clock.clone());
        self.audio.set_test_clock(clock);
    }

    /// The MIDI ports and the computer MIDI keyboard.
    #[must_use]
    pub fn midi(&self) -> &MidiHost {
        &self.midi
    }

    /// Whether the computer MIDI keyboard is on, so its letter keys play
    /// notes instead of running shortcuts.
    #[must_use]
    pub fn computer_keyboard_on(&self) -> bool {
        self.midi.keyboard_info().on
    }

    /// Lists the MIDI ports again, so a device plugged in or removed shows.
    /// Returns whether the list changed.
    pub fn refresh_midi(&mut self) -> bool {
        let before = self.midi.ports();
        self.midi.refresh();
        self.midi_listed = std::time::Instant::now();
        before != self.midi.ports()
    }

    /// Spec: midi-chord R15 and midi-cc-sender R7. While a Chord device's
    /// Learn is on, the keys played set its shifts; while a CC Control
    /// device's Learn is on, an incoming controller is handed to its next
    /// Custom dial without one. Each change is one undo step. Returns
    /// whether the set changed.
    fn learn_midi(&mut self) -> bool {
        use bl_devices::midi_cc::{
            CC_CONTROL, DIALS, LEARN as CC_LEARN, cc_is_listed, cc_type_for, dial_cc, dial_value,
        };
        use bl_devices::stack::{LEARN as CHORD_LEARN, STACK, shift};
        use bl_model::{Command, DeviceId, DeviceKind, ParamId};
        let devices: Vec<(DeviceId, String)> = self
            .editor
            .document()
            .tracks()
            .flat_map(|track| track.devices_deep())
            .filter_map(|device| match &device.kind {
                DeviceKind::Native { device_type } => Some((device.id, device_type.clone())),
                _ => None,
            })
            .collect();
        let learning = |device_type: &str, param: ParamId| -> Vec<DeviceId> {
            devices
                .iter()
                .filter(|(id, kind)| {
                    kind == device_type
                        && self
                            .editor
                            .document()
                            .device(*id)
                            .and_then(|device| device.params.get(&param).copied())
                            .is_some_and(|on| on >= 0.5)
                })
                .map(|(id, _)| *id)
                .collect()
        };
        let chords = learning(STACK, CHORD_LEARN);
        let senders = learning(CC_CONTROL, CC_LEARN);
        let sender = self.midi.sender().clone();
        if chords.is_empty() && senders.is_empty() {
            if self.chord_learn.tapping {
                sender.set_tap(false);
                self.chord_learn.tapping = false;
                self.chord_learn.clear();
            }
            return false;
        }
        if !self.chord_learn.tapping {
            sender.set_tap(true);
            self.chord_learn.tapping = true;
        }
        let mut heard = Vec::new();
        sender.take_notes(&mut heard);
        let mut changed = false;
        if !chords.is_empty()
            && let Some(shifts) = self.chord_learn.listen_from(&heard)
        {
            self.edit(|editor| {
                editor.begin("Learn Chord");
                for device in &chords {
                    for (n, value) in shifts.iter().enumerate() {
                        editor
                            .apply(Command::SetDeviceParam {
                                device: *device,
                                param: shift(n as u32),
                                value: f64::from(*value),
                            })
                            .expect("a shift takes any whole number in range");
                    }
                }
                editor.commit().expect("the learned shifts commit");
            });
            changed = true;
        }
        // Spec: midi-cc-sender R6. A controller the CC Type list leaves out
        // (bank select and the channel mode messages) isn't learned.
        let learned: Vec<(u8, f64)> = heard
            .iter()
            .filter(|message| message.status() & 0xF0 == 0xB0 && cc_is_listed(message.data1()))
            .map(|message| (message.data1(), f64::from(message.data2())))
            .collect();
        if !senders.is_empty() && !learned.is_empty() {
            let free: Vec<(DeviceId, Vec<u32>)> = senders
                .iter()
                .map(|device| {
                    let params = &self
                        .editor
                        .document()
                        .device(*device)
                        .expect("the device is in the set")
                        .params;
                    let free = (0..DIALS)
                        .filter(|k| params.get(&dial_cc(*k)).is_none_or(|cc| *cc == 0.0))
                        .collect();
                    (*device, free)
                })
                .collect();
            self.edit(|editor| {
                editor.begin("Learn CC Control");
                for (device, mut free) in free.clone() {
                    for &(controller, value) in &learned {
                        let Some(k) = free.first().copied() else {
                            break;
                        };
                        free.remove(0);
                        editor
                            .apply(Command::SetDeviceParam {
                                device,
                                param: dial_cc(k),
                                value: cc_type_for(controller),
                            })
                            .expect("a controller is in the CC Type list");
                        editor
                            .apply(Command::SetDeviceParam {
                                device,
                                param: dial_value(k),
                                value,
                            })
                            .expect("a controller value is in range");
                    }
                }
                editor.commit().expect("the learned controllers commit");
            });
            changed = true;
        }
        changed
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
                || {
                    self.pending_name
                        .clone()
                        .unwrap_or_else(|| "Untitled".to_owned())
                },
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

    /// The file name the plug-in list's Export… suggests: `<set name>
    /// Plug-Ins.txt` (the plug-in list spec, R7).
    #[must_use]
    pub fn plugin_list_file_name(&self) -> String {
        format!(
            "{} Plug-Ins.txt",
            bl_project::preset::file_safe(&self.set_name())
        )
    }

    /// Whether the set has changes that aren't saved.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether the set has changes that aren't saved (the crash-recovery
    /// spec, R25): with Save Continuously on, a set that has a file has
    /// none, because every finished edit of it is committed within a
    /// second. A set that has never been saved still has some.
    #[must_use]
    pub fn unsaved_changes(&self) -> bool {
        self.dirty && !(self.prefs.save_continuously && self.set_path.is_some())
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

    /// Record Session automation in (Record, Warp & Launch; WP-3.12): which
    /// tracks a Session recording writes control moves into the clips of.
    #[must_use]
    pub fn record_session_automation_in(&self) -> crate::settings::SessionAutomationTracks {
        self.prefs.record_session_automation_in
    }

    /// The mode an Arrangement recording writes control moves in (the
    /// automation spec, R18): Touch, Latch or Write.
    #[must_use]
    pub fn automation_record_mode(&self) -> crate::settings::AutomationRecordMode {
        self.prefs.automation_record_mode
    }

    /// Whether Warp recorded audio is on (IMP-02). Clips have no warp fields
    /// yet, so recorded clips play unwarped either way.
    #[must_use]
    pub fn warp_recorded_audio(&self) -> bool {
        self.prefs.warp_recorded_audio.0
    }

    /// Whether Create Fades on Clip Edges is on: new audio clips get 4 ms
    /// fades at their edges.
    #[must_use]
    pub fn create_fades_on_clip_edges(&self) -> bool {
        self.prefs.create_fades_on_clip_edges.0
    }

    /// Loop/Warp Short Samples, Auto-Warp Long Samples and Default Warp
    /// Mode (WP-4.9).
    #[must_use]
    pub fn warp_prefs(&self) -> WarpPrefs {
        self.prefs.warp
    }

    /// The Launch section of Record, Warp & Launch (WP-3.4).
    #[must_use]
    pub fn launch_prefs(&self) -> LaunchPrefs {
        self.prefs.launch
    }

    /// What `settings.state` reports: the Settings window's preferences by
    /// page.
    fn settings_state(&mut self) -> SettingsState {
        self.refresh_midi();
        let agent = &self.prefs.agent;
        let server = AgentServerState::from(agent);
        SettingsState {
            page: self.prefs.settings_page,
            display_input: DisplayInputSettings {
                zoom_percent: self.zoom_percent(),
                outline_view_in_focus: self.outline_view_in_focus(),
                use_tab_key_to_move_focus: self.tab_navigation(),
                wrap_tab_navigation: self.wrap_navigation(),
            },
            theme_colors: ThemeColorsSettings {
                look: self.prefs.look.clone(),
                appearance: self.prefs.appearance,
                drawn_appearance: if self.theme_dark() {
                    Appearance::Dark
                } else {
                    Appearance::Light
                },
                themes: self.themes.clone(),
            },
            audio: AudioPageSettings {
                device: self.audio.settings().clone(),
                tone: self.audio.tone(),
            },
            tempo_midi: self.midi.report(),
            create_fades_on_clip_edges: self.create_fades_on_clip_edges(),
            warp_recorded_audio: self.warp_recorded_audio(),
            warp: self.prefs.warp,
            file_folder: FileFolderSettings {
                temporary_folder: self.temporary_folder().to_string_lossy().into_owned(),
                sample_editor: self.sample_editor().unwrap_or_default(),
                save_continuously: self.save_continuously(),
            },
            launch: self.prefs.launch,
            record_session_automation_in: self.prefs.record_session_automation_in,
            library: LibrarySettings {
                collect_files_on_export: self.prefs.collect_files_on_export,
                user_library: self
                    .browser_view
                    .user_library()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
            automation_record_mode: self.prefs.automation_record_mode,
            auto_hide_plug_in_windows: self.auto_hide_plug_in_windows(),
            plug_ins: self.plugin_settings(),
            agents: AgentsPageSettings {
                server: server.server,
                port: agent.port,
                url: server.url,
                token_stored: !agent.token.is_empty(),
                clients: server.clients,
            },
            assistant: self.assistant_state(),
        }
    }

    /// The Appearance chosen (themes R3).
    #[must_use]
    pub fn appearance(&self) -> Appearance {
        self.prefs.appearance
    }

    /// The theme, tone, high contrast and Customization chosen (themes R1).
    #[must_use]
    pub fn look(&self) -> &ThemeLook {
        &self.prefs.look
    }

    /// Whether the dark colours are drawn now: Follow System resolved
    /// (themes R3).
    #[must_use]
    pub fn theme_dark(&self) -> bool {
        match self.prefs.appearance {
            Appearance::Light => false,
            Appearance::Dark => true,
            Appearance::FollowSystem => self.system_dark,
        }
    }

    /// Tells the shell the operating system's mode changed; the host calls
    /// it at start and on winit's theme event (themes R3).
    pub fn set_system_dark(&mut self, dark: bool) {
        self.system_dark = dark;
    }

    /// The User Library's `Themes` folder, where theme files are read
    /// from (themes R9), if the User Library is known.
    #[must_use]
    pub fn themes_folder(&self) -> Option<PathBuf> {
        self.browser_view
            .user_library()
            .map(|library| library.join("Themes"))
    }

    /// The themes the Theme chooser offers, Beatlayer first.
    #[must_use]
    pub fn themes(&self) -> &[String] {
        &self.themes
    }

    /// Replaces the list of themes the chooser offers (themes R2); the
    /// host calls it after reading the theme files.
    pub fn set_themes(&mut self, names: Vec<String>) {
        self.themes = names;
        self.touch();
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

    /// The line the Status Bar shows while it has no message: what the
    /// pointer is over says about itself, such as a drum rack's pad
    /// (WP-5.6).
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// Sets, or with `None` clears, the Status Bar's hint.
    pub fn set_hint(&mut self, hint: Option<String>) {
        if self.hint != hint {
            self.hint = hint;
            self.touch();
        }
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
        self.clip_view.sync_sample(self.editor.document());
        // The kit plug-ins' instances first, so a node the engine makes at once
        // finds its device's.
        self.kit.sync(self.editor.document());
        self.audio.sync(self.editor.document());
        self.start_analyses();
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
                    .devices_deep()
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

    /// The hosts an offline render borrows the engine's plug-ins from
    /// (WP-2.6b, WP-4.21).
    #[must_use]
    pub fn plugin_hosts(&self) -> bl_plugin_host::Hosts<'_> {
        bl_plugin_host::Hosts {
            clap: Some(&self.plugins),
            vst3: Some(&self.vst3),
            au: Some(&self.au),
        }
    }

    /// Lets go of the plug-ins of the set being replaced (WP-2.6): the new
    /// set's plug-in devices load their plug-ins from their saved state.
    fn set_replaced(&mut self) {
        self.editor_replaced();
        // What a DAWproject import said about the set that's gone (R11, R12).
        self.pending_name = None;
        self.import_report = None;
        // A file still being written was for the set that's gone: its worker
        // deletes it when it finds nobody waiting (WP-4.20).
        self.processed.clear();
        if self.session_run.take().is_some() {
            // The recording's document is gone; its take ends unplaced.
            let _ = self
                .audio
                .send_session(bl_engine::SessionCommand::StopRecording);
            self.recording = None;
        }
        self.plugins.clear();
        self.au.clear();
        self.vst3.clear();
        self.kit.clear();
    }

    /// Notes that the editor was replaced, so the journal doesn't hold the
    /// new set as an edit of the old one (the crash-recovery spec, R2).
    fn editor_replaced(&mut self) {
        self.editor_generation = self.editor_generation.wrapping_add(1);
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
                            midi_sidechain: false,
                        },
                        track,
                        chain: None,
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
                    bl_model::DeviceKind::Rack { kind } => kind.name().to_owned(),
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
                let preset = found.preset;
                let added: device_ids::DeviceAdded = serde_json::from_value(self.run_devices(
                    device_ids::ADD,
                    to_value(&device_ids::AddDeviceArgs {
                        device: device_ids::NewDevice {
                            kind: device_type.kind.clone(),
                            name: device_type.name.clone(),
                            role: device_type.role,
                            sidechain: false,
                            midi_sidechain: false,
                        },
                        track,
                        chain: None,
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
                // Spec: plugin-list R5. The name of the preset the unit
                // holds, as it reports it.
                let _ = self.editor.amend_device_preset_name(added.device, preset);
                let plugin_id = match &device_type.kind {
                    bl_model::DeviceKind::Plugin { plugin_id, .. } => plugin_id.clone(),
                    bl_model::DeviceKind::Native { device_type } => device_type.clone(),
                    bl_model::DeviceKind::Rack { kind } => kind.name().to_owned(),
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
            plugin_ids::CONFIGURE => {
                let plugin_ids::ConfigureArgs { device, on } = Self::args(id, args)?;
                let device = target(self, device)?;
                let kind = self
                    .editor
                    .document()
                    .device(device)
                    .map(|d| d.kind.clone())
                    .ok_or_else(|| unavailable(format!("no {device}")))?;
                if DeviceView::plugin_key(&kind).is_none() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("{device} isn't a plug-in device"),
                    });
                }
                let on = on.unwrap_or(!self.devices.configuring(device));
                self.devices.set_configuring(device, on);
                self.touch();
                Ok(to_value(&self.device_view_state()))
            }
            plugin_ids::CONFIGURE_PARAM => {
                let plugin_ids::ConfigureParamArgs {
                    device,
                    param,
                    index,
                } = Self::args(id, args)?;
                let device = target(self, device)?;
                let Some((all, mut configured)) = self.configured_lists(device) else {
                    return Err(unavailable(format!(
                        "{device} isn't a described plug-in device"
                    )));
                };
                if !all.contains(&param) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("{} has no parameter {}", device, param.0),
                    });
                }
                configured.retain(|p| *p != param);
                let at = index.unwrap_or(configured.len()).min(configured.len());
                configured.insert(at, param);
                self.editor
                    .transact("Configure Plug-In", |e| {
                        e.apply(bl_model::Command::SetConfiguredParams {
                            device,
                            params: configured.clone(),
                        })
                    })
                    .map_err(|error| ShellError::Failed {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.devices.clear_pending_param(device);
                self.dirty = true;
                self.document_changed();
                self.touch();
                self.configured_params_result(id, device)
            }
            plugin_ids::UNCONFIGURE_PARAM => {
                let plugin_ids::UnconfigureParamArgs {
                    device,
                    param,
                    force,
                } = Self::args(id, args)?;
                let device = target(self, device)?;
                let Some((all, mut configured)) = self.configured_lists(device) else {
                    return Err(unavailable(format!(
                        "{device} isn't a described plug-in device"
                    )));
                };
                if !all.contains(&param) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("{} has no parameter {}", device, param.0),
                    });
                }
                if !force {
                    let uses = self.param_uses(device, param);
                    if !uses.is_empty() {
                        // Manual 24.1.2: Live warns before deleting a
                        // parameter with automation, clip envelopes, or
                        // mappings.
                        return Err(ShellError::Failed {
                            command: id.to_owned(),
                            message: format!(
                                "this parameter has {}, so removing it from the panel changes \
                                 what plays; run again with force to remove it",
                                uses.join(", ")
                            ),
                        });
                    }
                }
                configured.retain(|p| *p != param);
                self.editor
                    .transact("Unconfigure Plug-In Parameter", |e| {
                        e.apply(bl_model::Command::SetConfiguredParams {
                            device,
                            params: configured.clone(),
                        })
                    })
                    .map_err(|error| ShellError::Failed {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.devices.clear_pending_param(device);
                self.dirty = true;
                self.document_changed();
                self.touch();
                self.configured_params_result(id, device)
            }
            plugin_ids::MOVE_CONFIGURED_PARAM => {
                let plugin_ids::MoveConfiguredParamArgs { device, from, to } =
                    Self::args(id, args)?;
                let device = target(self, device)?;
                let Some((_all, mut configured)) = self.configured_lists(device) else {
                    return Err(unavailable(format!(
                        "{device} isn't a described plug-in device"
                    )));
                };
                if from >= configured.len() || to >= configured.len() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!(
                            "the configured list has {} entries, so {from} to {to} is out of \
                             range",
                            configured.len()
                        ),
                    });
                }
                let param = configured.remove(from);
                configured.insert(to, param);
                self.editor
                    .transact("Move Plug-In Parameter", |e| {
                        e.apply(bl_model::Command::SetConfiguredParams {
                            device,
                            params: configured.clone(),
                        })
                    })
                    .map_err(|error| ShellError::Failed {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.dirty = true;
                self.document_changed();
                self.touch();
                self.configured_params_result(id, device)
            }
            plugin_ids::SCAN | plugin_ids::RESCAN => {
                let plugin_ids::ScanArgs { paths, background } = Self::args(id, args)?;
                if background {
                    // The Settings window's Rescan button: the scan runs
                    // while the app keeps drawing, and the browser lists
                    // what it found when it's done.
                    if paths.is_some() {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: "a background scan covers the system folders; leave \
                                      `paths` out"
                                .to_owned(),
                        });
                    }
                    self.spawn_plugin_scan(id == plugin_ids::RESCAN);
                    return Ok(to_value(&plugin_ids::PluginScan::default()));
                }
                let paths: Option<Vec<std::path::PathBuf>> =
                    paths.map(|paths| paths.into_iter().map(std::path::PathBuf::from).collect());
                // Named paths are searched for every hosted format; the
                // system folders only for the formats switched on.
                let formats = match &paths {
                    Some(_) => bl_plugin_host::scan::SCANNED_FORMATS.to_vec(),
                    None => crate::plugins::listed_formats(&self.prefs),
                };
                let report = crate::plugins::scan_report(
                    &self.scanner,
                    paths.as_deref(),
                    id == plugin_ids::RESCAN,
                    &formats,
                    &self.prefs.hidden_plugins,
                );
                self.publish_plugins();
                Ok(to_value(&report))
            }
            plugin_ids::LIST => {
                let _: plugin_ids::ListArgs = Self::args(id, args)?;
                let list = self.set_plugin_list();
                Ok(to_value(&list))
            }
            plugin_ids::EXPORT_LIST => {
                // Spec: plugin-list R7, R10.
                let plugin_ids::ExportListArgs { path } = Self::args(id, args)?;
                let Some(path) = path else {
                    // The list's Export… button: the app asks where to
                    // save it, and runs this again with the path.
                    self.effects.push(Effect::PickPluginListPath {
                        default_name: self.plugin_list_file_name(),
                    });
                    return Ok(Value::Null);
                };
                let path = std::path::PathBuf::from(path);
                let written = crate::plugins::export_list(
                    self.editor.document(),
                    &self.set_plugin_list(),
                    &path,
                    &self.set_name(),
                )
                .map_err(|message| ShellError::Failed {
                    command: id.to_owned(),
                    message,
                })?;
                Ok(to_value(&plugin_ids::ExportedList {
                    path: written.to_string_lossy().into_owned(),
                }))
            }
            plugin_ids::SETTINGS => {
                let plugin_ids::PluginSettingsArgs {
                    hidden,
                    show_hidden,
                    format_preference,
                    use_audio_units_v2,
                    use_audio_units_v3,
                    vst3_system_folders,
                } = Self::args(id, args)?;
                let mut changed = false;
                // Spec: plugin-scanning R12. Turning a format's switch on
                // scans it, as Live does.
                let mut scan = false;
                if let Some(on) = use_audio_units_v2 {
                    scan |= on && !self.prefs.use_audio_units_v2;
                    self.prefs.use_audio_units_v2 = on;
                    changed = true;
                }
                if let Some(on) = use_audio_units_v3 {
                    self.prefs.use_audio_units_v3 = on;
                    changed = true;
                }
                if let Some(on) = vst3_system_folders {
                    scan |= on && !self.prefs.vst3_system_folders;
                    self.prefs.vst3_system_folders = on;
                    changed = true;
                }
                if scan && self.auto_scan {
                    self.spawn_plugin_scan(false);
                }
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
                Ok(to_value(&self.plugin_settings()))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// What `plugin.info` reports for a plug-in device, for the Device
    /// View's window button (WP-2.10): whether it has an editor and whether
    /// its window is open. None for a device that isn't a loaded plug-in.
    #[must_use]
    pub fn plugin_info(&self, device: bl_model::DeviceId) -> Option<plugin_ids::PluginInfo> {
        crate::plugins::info(&self.plugins, &self.au, &self.vst3, device)
    }

    /// How the Browser shows plug-ins, and the Plug-In Sources switches:
    /// what `plugin.settings` returns.
    #[must_use]
    pub fn plugin_settings(&self) -> plugin_ids::PluginSettings {
        plugin_ids::PluginSettings {
            hidden: self.prefs.hidden_plugins.iter().cloned().collect(),
            show_hidden: self.prefs.show_hidden_plugins,
            format_preference: self.prefs.plugin_format_preference,
            use_audio_units_v2: self.prefs.use_audio_units_v2,
            use_audio_units_v3: self.prefs.use_audio_units_v3,
            vst3_system_folders: self.prefs.vst3_system_folders,
        }
    }

    /// What the Device View shows: the selected track's chain (WP-2.4).
    #[must_use]
    pub fn device_view_state(&self) -> device_ids::DeviceViewState {
        let doc = self.editor.document();
        self.devices
            .state(doc, devices::shown_track(&self.arrangement, doc))
    }

    /// A plug-in device's modifiable parameters and its configured list, in
    /// order (WP-5.40, clap-hosting R5); `None` for a device that isn't a
    /// plug-in, or whose type isn't described.
    fn configured_lists(
        &self,
        device: bl_model::DeviceId,
    ) -> Option<(Vec<bl_model::ParamId>, Vec<bl_model::ParamId>)> {
        let doc = self.editor.document();
        let kind = doc.device(device)?.kind.clone();
        if !matches!(kind, bl_model::DeviceKind::Plugin { .. }) {
            return None;
        }
        let description = self.devices.description(&kind)?;
        let all: Vec<bl_model::ParamId> = description.params.iter().map(|p| p.id).collect();
        let configured: Vec<bl_model::ParamId> = doc.device(device)?.configured(&all).into_owned();
        Some((all, configured))
    }

    /// What uses a plug-in parameter, which Live warns about before the panel
    /// drops it (manual 24.1.2): its automation, a clip envelope, or a
    /// mapping.
    fn param_uses(
        &self,
        device: bl_model::DeviceId,
        param: bl_model::ParamId,
    ) -> Vec<&'static str> {
        use bl_model::modulator::ModTarget;
        let doc = self.editor.document();
        let target = AutomationTarget::Device { device, param };
        let mut uses = Vec::new();
        if let Some(track) = doc.device_track(device).and_then(|t| doc.track(t)) {
            if track.automation_for(&target).is_some() {
                uses.push("automation");
            }
            if track.clips().any(|clip| clip.envelope(&target).is_some()) {
                uses.push("a clip envelope");
            }
        }
        let mapped = doc.tracks().any(|track| {
            track.devices_deep().any(|d| {
                d.mappings
                    .iter()
                    .any(|m| m.target == ModTarget::Device { device, param })
            })
        });
        if mapped {
            uses.push("a mapping");
        }
        uses
    }

    /// Adds a plug-in parameter to its device's configured list, as one undo
    /// step, when the device hasn't configured it (WP-5.40, manual 24.1.2):
    /// using a parameter in the automation or clip-envelope choosers makes its
    /// temporary entry permanent.
    fn configure_on_use(&mut self, device: bl_model::DeviceId, param: bl_model::ParamId) {
        let Some((all, mut configured)) = self.configured_lists(device) else {
            return;
        };
        if !all.contains(&param) || configured.contains(&param) {
            return;
        }
        configured.push(param);
        let command = bl_model::Command::SetConfiguredParams {
            device,
            params: configured,
        };
        if self
            .editor
            .transact("Configure Plug-In", |e| e.apply(command))
            .is_ok()
        {
            self.devices.clear_pending_param(device);
            self.dirty = true;
            self.document_changed();
        }
    }

    /// `device.parameters` after a configure edit, as its command returns it.
    fn configured_params_result(
        &self,
        id: &str,
        device: bl_model::DeviceId,
    ) -> Result<Value, ShellError> {
        self.devices
            .parameters_of(self.editor.document(), device)
            .map(|params| to_value(&params))
            .ok_or_else(|| ShellError::Failed {
                command: id.to_owned(),
                message: format!("no {device}"),
            })
    }

    /// The modulator slot whose Map button is lit, if one is (WP-5.10): a
    /// click on any parameter's control maps it.
    #[must_use]
    pub fn armed_map(&self) -> Option<device_ids::ArmedMap> {
        self.devices
            .armed()
            .map(|(device, slot)| device_ids::ArmedMap { device, slot })
    }

    /// Sets the display data a device's display draws, as
    /// [`Shell::poll_engine`] does with the engine's (for a shell without an
    /// engine, as in tests; while one runs, the next poll takes its data
    /// instead).
    pub fn set_device_display_data(&mut self, device: bl_model::DeviceId, values: &[f32]) {
        let data = self.devices.displays_mut().entry(device).or_default();
        data.published += 1;
        data.values.clear();
        data.values.extend_from_slice(values);
    }

    /// What `device`'s display shows, as the engine last published it
    /// (the device framework spec, R36): the cell draws it, a screen reader
    /// says it. `None` if the set has no such device.
    #[must_use]
    pub fn device_display(&self, device: bl_model::DeviceId) -> Option<bl_model::DeviceDisplay> {
        let device = self.editor.document().device(device)?;
        Some(self.devices.display(device))
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

    /// Whether any plug-in editor window is open (IMP-25).
    fn any_plug_in_editor(&self) -> bool {
        !self.plugins.open_editors().is_empty() || !self.au.open_editors().is_empty()
    }

    /// Closes every open plug-in editor window and says how many it closed
    /// (View ▸ Close All Plug-in Windows, IMP-25). It closes editors only,
    /// so a floating native device window stays as it is.
    fn close_plug_in_windows(&mut self) -> usize {
        let mut closed = 0;
        for device in self.plugins.open_editors() {
            if self.plugins.close_editor(device).is_ok() {
                closed += 1;
            }
        }
        for device in self.au.open_editors() {
            if self.au.close_editor(device).is_ok() {
                closed += 1;
            }
        }
        closed
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
        if id == pad_ids::PAD_DROP {
            return self.pad_drop(args);
        }
        if id == pad_ids::PAD_HOT_SWAP_SAMPLE {
            return self.pad_hot_swap_sample(args);
        }
        if matches!(
            id,
            device_ids::CUT
                | device_ids::COPY
                | device_ids::DUPLICATE
                | device_ids::SAVE_PRESET
                | device_ids::SAVE_DEFAULT_PRESET
                | device_ids::CHAIN_SAVE_PRESET
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
        if id == device_ids::DISPLAY && self.audio.engine_running() {
            // Spec: device-framework R37. The newest the nodes published,
            // for an agent that never polls the engine.
            self.audio.displays_into(self.devices.displays_mut());
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
                media: &self.media,
                zoom,
                auto_hide,
                windows_shown: self.plug_in_windows,
            },
        );
        let outcome = result.map_err(|error| device_error(id, error))?;
        // Spec: drum-racks R5. A held Preview button plays its chain.
        for &(chain, on) in &outcome.previews {
            self.audio.send_param(
                bl_engine::NodeKey::ChainNotes(chain),
                bl_engine::drum::param::PREVIEW,
                f64::from(u8::from(on)),
            );
        }
        // Spec: racks R42. Samples a rack or chain preset brought in.
        for file in &outcome.media {
            self.store.add_media(file.hash, &file.path, file.size);
        }
        if outcome.document_changed {
            self.dirty = true;
            self.document_changed();
        } else if outcome.view_changed {
            self.dirty = true;
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

    /// Drops samples, a device or a preset on a pad of a drum rack
    /// (`pad.drop`, the drum racks spec, R14 to R16): the media store takes
    /// the files, the set records where they are, and the pad's chains play
    /// them. One undo step.
    fn pad_drop(&mut self, args: Value) -> Result<Value, ShellError> {
        let id = pad_ids::PAD_DROP;
        let args: pad_ids::PadDropArgs = Self::args(id, args)?;
        let mut samples = Vec::with_capacity(args.samples.len());
        for path in &args.samples {
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
            let media = bl_model::ContentHash(*hash.as_bytes());
            self.store.add_media(media, &entry.path, entry.size);
            samples.push(devices::PadSample {
                media,
                name: file_stem(&entry.path),
            });
        }
        let user_library = self.browser_view.user_library().map(Path::to_path_buf);
        let dropped = self
            .devices
            .pad_drop(&mut self.editor, user_library.as_deref(), args, &samples)
            .map_err(|error| device_error(id, error))?;
        self.dirty = true;
        self.document_changed();
        self.touch();
        Ok(to_value(&dropped))
    }

    /// A pad's Hot-Swap Sample button (`pad.hot_swap_sample`, the drum racks
    /// spec, R19): the pad becomes the hot-swap target and the browser opens
    /// the folder of the sample it plays.
    fn pad_hot_swap_sample(&mut self, args: Value) -> Result<Value, ShellError> {
        let id = pad_ids::PAD_HOT_SWAP_SAMPLE;
        let args: pad_ids::PadHotSwapArgs = Self::args(id, args)?;
        let media = self
            .devices
            .pad_sample_of(self.editor.document(), args.rack, args.note)
            .map_err(|error| device_error(id, error))?
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "the pad's chain plays no sample".to_owned(),
            })?;
        let state = self.run_devices(pad_ids::PAD_HOT_SWAP, to_value(&args))?;
        let folder = self
            .media
            .entry(bl_media::ContentHash::from_bytes(media.0))
            .and_then(|entry| entry.path.parent().map(Path::to_path_buf));
        if let Some(folder) = folder {
            self.browser = true;
            self.run_browser(
                browser_ids::OPEN_FOLDER,
                json!({ "path": folder, "open": true, "keep_others": true }),
            )
            .ok();
        }
        Ok(state)
    }

    /// Loads a file into a device that plays one sample
    /// (`device.load_sample`, WP-2.16): the media store takes the file, the
    /// set records where it is and the device names it by content hash, and
    /// the engine plays it. One undo step.
    fn load_sample(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let device_ids::LoadSampleArgs { device, path, slot } = Self::args(id, args)?;
        let device =
            device
                .or_else(|| self.devices.selected())
                .ok_or_else(|| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "select a device first".to_owned(),
                })?;
        let target = self
            .editor
            .document()
            .device(device)
            .cloned()
            .ok_or_else(|| ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("no {device}"),
            })?;
        // Only the sampler and the drum sampler play samples (the sampler
        // spec, R11).
        let plays = matches!(
            &target.kind,
            bl_model::DeviceKind::Native { device_type } if bl_devices::plays_samples(device_type)
        );
        if !plays {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!("{device} doesn't play a sample"),
            });
        }
        // A device with several slots loads into the one selected unless
        // told which.
        let slot = slot.unwrap_or_else(|| self.devices.slot_of(&target) as u8);
        let slots = self.devices.slot_count(&target.kind).max(1);
        if u32::from(slot) >= slots {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!(
                    "{device} has {slots} sample slots, counted from 0; there is no slot {slot}"
                ),
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
                slot,
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
            slot,
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
            track_ids::SET_DEVICES_ENABLED => {
                // Spec: device-chains R39. Deactivate All Devices switches
                // off every top-level device on the selected tracks, and
                // switches them back on when they are all off already; it
                // reaches no rack's contents. One undo step either way.
                let track_ids::SetDevicesEnabledArgs { tracks, enabled } = Self::args(id, args)?;
                let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                let on = enabled.unwrap_or_else(|| self.all_devices_off(&tracks));
                let step = if on {
                    track_ids::ACTIVATE_ALL_DEVICES_STEP
                } else {
                    track_ids::DEACTIVATE_ALL_DEVICES_STEP
                };
                let opened = !self.gesture;
                if opened {
                    self.begin_gesture(step);
                }
                let mut result = Ok(());
                for &track in &tracks {
                    let devices: Vec<bl_model::DeviceId> = self
                        .editor
                        .document()
                        .track(track)
                        .into_iter()
                        .flat_map(|track| track.devices().map(|device| device.id))
                        .collect();
                    for device in devices {
                        result = self
                            .run(
                                "device.set_enabled",
                                json!({ "device": device, "enabled": on }),
                            )
                            .map(|_| ());
                        if result.is_err() {
                            break;
                        }
                    }
                    if result.is_err() {
                        break;
                    }
                }
                if opened {
                    self.end_gesture();
                }
                result?;
                Ok(to_value(&track_ids::DevicesEnabled {
                    tracks,
                    enabled: on,
                }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Whether every top-level device on `tracks` is switched off, with at
    /// least one device among them (IMP-25, the device chains spec R39).
    fn all_devices_off(&self, tracks: &[TrackId]) -> bool {
        let doc = self.editor.document();
        let mut any = false;
        for &track in tracks {
            let Some(track) = doc.track(track) else {
                continue;
            };
            for device in track.devices() {
                any = true;
                if device.enabled {
                    return false;
                }
            }
        }
        any
    }

    /// Loads a plug-in the browser lists (WP-2.9b): onto the selected track
    /// as `plugin.load` (or `plugin.load_au`) places it, or, while a device
    /// is being hot-swapped, in its place, as one undo step.
    fn load_browser_plugin(
        &mut self,
        id: &str,
        plugin: &bl_plugin_host::scan::ScannedPlugin,
    ) -> Result<Value, ShellError> {
        let swapped = self.devices.hot_swap().and_then(|device| {
            let doc = self.editor.document();
            let track = doc.device_track(device)?;
            let index = doc.track(track)?.device_position(device)?;
            Some((device, track, index))
        });
        // An instrument replaces the selected track's instrument, as Live's
        // browser does (the plug-in scanning spec, R11).
        let swapped = swapped.or_else(|| {
            if plugin.role != bl_model::DeviceRole::Instrument {
                return None;
            }
            let doc = self.editor.document();
            let track = devices::shown_track(&self.arrangement, doc)?;
            let chain = doc.track(track)?;
            let device = chain
                .devices()
                .find(|d| d.role == bl_model::DeviceRole::Instrument)?
                .id;
            Some((device, track, chain.device_position(device)?))
        });
        let (mut track, index) = swapped.map_or((None, None), |(_, t, i)| (Some(t), Some(i)));
        self.editor.begin("Load Plug-In");
        let loaded = (|| {
            // Live always has a track selected; a set with none gets a new
            // track for the plug-in: MIDI for an instrument or a MIDI
            // effect, audio for an audio effect.
            if track.is_none()
                && devices::shown_track(&self.arrangement, self.editor.document()).is_none()
            {
                let kind = match plugin.role {
                    bl_model::DeviceRole::AudioEffect => "audio",
                    _ => "midi",
                };
                let before: std::collections::BTreeSet<TrackId> = self
                    .editor
                    .document()
                    .tracks()
                    .map(bl_model::Track::id)
                    .collect();
                self.invoke_inner("track.create", json!({ "kind": kind }))?;
                track = self
                    .editor
                    .document()
                    .tracks()
                    .map(bl_model::Track::id)
                    .find(|t| !before.contains(t));
            }
            // The device it replaces goes first, so the chain never holds
            // two instruments.
            if let Some((old, _, _)) = swapped {
                self.editor
                    .apply(bl_model::Command::RemoveDevice { device: old })
                    .map_err(|e| ShellError::Failed {
                        command: id.to_owned(),
                        message: e.to_string(),
                    })?;
            }
            let loaded = match plugin.format {
                bl_model::PluginFormat::AudioUnit => {
                    let (kind, subtype, manufacturer) = bl_plugin_host::au::parse_description_id(
                        &plugin.plugin_id,
                    )
                    .ok_or_else(|| ShellError::Failed {
                        command: id.to_owned(),
                        message: format!("{} isn't an Audio Unit's identifier", plugin.plugin_id),
                    })?;
                    self.run_plugins(
                        plugin_ids::LOAD_AU,
                        to_value(&plugin_ids::LoadAudioUnitArgs {
                            component_type: kind.to_owned(),
                            subtype: subtype.to_owned(),
                            manufacturer: manufacturer.to_owned(),
                            track,
                            index,
                        }),
                    )?
                }
                _ => self.run_plugins(
                    plugin_ids::LOAD,
                    to_value(&plugin_ids::LoadPluginArgs {
                        path: plugin.path.to_string_lossy().into_owned(),
                        plugin: Some(plugin.plugin_id.clone()),
                        track,
                        index,
                    }),
                )?,
            };
            Ok(loaded)
        })();
        match loaded {
            Ok(loaded) => {
                if self.editor.commit().unwrap_or(false) {
                    self.dirty = true;
                }
                self.document_changed();
                Ok(loaded)
            }
            Err(error) => {
                self.editor.cancel().ok();
                self.document_changed();
                Err(error)
            }
        }
    }

    /// What the browser's selection loads into the Device View: one preset
    /// file, or a device listed under a label (device chains R19, racks R43
    /// and R45).
    fn devices_load(&self) -> Option<DevicesLoad> {
        match self.browser_view.selected() {
            [one] if bl_project::preset::is_preset(one) => Some(DevicesLoad::Preset(one.clone())),
            _ if self.browser_view.selected_audio_is_empty() => {
                self.browser_view.selected_device().map(DevicesLoad::Device)
            }
            _ => None,
        }
    }

    /// Loads what the browser has selected: added where `device.add` puts
    /// a device, or, while a device, rack or chain is hot-swapped, in place
    /// of it.
    fn load_into_devices(&mut self, load: DevicesLoad) -> Result<Value, ShellError> {
        let armed_chain = self.devices.hot_swap_chain();
        let swapping = self.devices.hot_swap().is_some() || armed_chain.is_some();
        match load {
            DevicesLoad::Preset(path) => {
                let path = path.to_string_lossy().into_owned();
                if devices::is_chain_preset(Path::new(&path)) {
                    self.run_devices(device_ids::CHAIN_LOAD_PRESET, json!({ "path": path }))
                } else if swapping {
                    self.run_devices(
                        device_ids::REPLACE,
                        json!({ "preset": path, "chain": armed_chain }),
                    )
                } else {
                    self.run_devices(device_ids::ADD_PRESET, json!({ "path": path }))
                }
            }
            DevicesLoad::Device(device) => {
                let new = device_ids::NewDevice {
                    kind: device.kind,
                    name: device.name,
                    role: device.role,
                    sidechain: device.sidechain,
                    midi_sidechain: device.midi_sidechain,
                };
                if swapping {
                    self.run_devices(
                        device_ids::REPLACE,
                        json!({ "new_device": new, "chain": armed_chain }),
                    )
                } else {
                    self.run_devices(
                        device_ids::ADD,
                        to_value(&device_ids::AddDeviceArgs {
                            device: new,
                            track: None,
                            chain: None,
                            index: None,
                        }),
                    )
                }
            }
        }
    }

    /// The browser's selection, if it's one audio file and a device that
    /// plays samples is being hot-swapped: what Enter or a double-click
    /// loads into the device's selected slot (WP-5.17).
    fn pad_hot_swap_drop(
        &self,
        rack: bl_model::DeviceId,
        note: u8,
    ) -> Option<pad_ids::PadDropArgs> {
        let selected = self.browser_view.selected();
        let samples: Vec<String> = selected
            .iter()
            .filter(|p| browser::arrange::is_audio(&p.to_string_lossy()))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let preset = match selected {
            [one]
                if samples.is_empty()
                    && one
                        .extension()
                        .is_some_and(|e| e == bl_project::preset::PRESET_EXTENSION) =>
            {
                Some(one.to_string_lossy().into_owned())
            }
            _ => None,
        };
        (!samples.is_empty() || preset.is_some()).then(|| pad_ids::PadDropArgs {
            rack: Some(rack),
            note,
            samples,
            preset,
            ..Default::default()
        })
    }

    fn hot_swap_sample(&self) -> Option<(bl_model::DeviceId, PathBuf)> {
        let device = self.devices.hot_swap()?;
        let takes = self.editor.document().device(device).is_some_and(|d| {
            matches!(
                &d.kind,
                bl_model::DeviceKind::Native { device_type }
                    if bl_devices::plays_samples(device_type)
            )
        });
        match self.browser_view.selected() {
            [one] if takes && browser::arrange::is_audio(&one.to_string_lossy()) => {
                Some((device, one.clone()))
            }
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

    /// The list of the set's plug-ins (IMP-29, WP-6.41).
    #[must_use]
    pub fn plugin_list(&self) -> &plugin_list::PluginList {
        &self.plugin_list
    }

    /// The set's plug-in list, as `plugin.list` reports it.
    #[must_use]
    pub fn set_plugin_list(&self) -> plugin_ids::SetPluginList {
        crate::plugins::set_plugin_list(self.editor.document(), &self.scanner)
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

    /// Each track's Performance Impact, as the engine last published it:
    /// the track's share of the set's processing time and how many of the
    /// six segments it lights (the mixer spec, R31).
    #[must_use]
    pub fn impact_values(&self) -> Vec<bl_shell_spec::mixer_view::TrackImpact> {
        self.meters
            .iter()
            .map(|meter| bl_shell_spec::mixer_view::TrackImpact {
                track: meter.track,
                segments: bl_shell_spec::mixer_view::impact_segments(meter.impact),
                share: meter.impact,
            })
            .collect()
    }

    /// One track's gain reduction in dB, as its compressors and limiters
    /// last reported it: zero when the engine published no meter for the
    /// track, or none of them reduced (the mixer view spec, R40).
    #[must_use]
    pub fn gain_reduction(&self, track: TrackId) -> f32 {
        self.meter(track).map_or(0.0, |meter| meter.gain_reduction)
    }

    /// Every track's gain reduction, as `mixer_view.state` reports it: one
    /// entry per track the engine published a meter for, in the order the
    /// tracks run (IMP-25, the mixer view spec R40).
    #[must_use]
    pub fn gain_reductions(&self) -> Vec<crate::mixer_view::TrackGainReduction> {
        self.meters
            .iter()
            .map(|meter| crate::mixer_view::TrackGainReduction {
                track: meter.track,
                reduction: meter.gain_reduction,
            })
            .collect()
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

    /// Waits for the browser's index to finish, up to `timeout`, for tests
    /// and tools. Returns whether it finished.
    #[doc(hidden)]
    pub fn wait_for_browser_index(&mut self, timeout: std::time::Duration) -> bool {
        self.browser_view.wait_for_index(timeout)
    }

    /// What Collect Files on Export is set to (Settings ▸ Library).
    #[must_use]
    pub fn collect_files_on_export(&self) -> bl_shell_spec::settings::CollectFiles {
        self.prefs.collect_files_on_export
    }

    /// Whether saving `clip` into `folder` (the User Library's `Clips` folder
    /// when `None`) as a clip file must ask the person first: Collect Files
    /// on Export is Ask and the clip or its track's devices play an audio
    /// file outside the project the file lands in (file management R8). The
    /// answer goes to `clip.save_file` as `copy`.
    #[must_use]
    pub fn clip_save_asks(&self, clip: bl_model::ClipId, folder: Option<&Path>) -> bool {
        let library = self.browser_view.user_library();
        let Some(folder) = folder
            .map(Path::to_path_buf)
            .or_else(|| library.map(|library| library.join("Clips")))
        else {
            return false;
        };
        self.prefs.collect_files_on_export == bl_shell_spec::settings::CollectFiles::Ask
            && crate::clip_files::uses_outside(
                self.editor.document(),
                &self.media,
                clip,
                &folder,
                library,
            )
    }

    /// Takes background work's news: the browser's index, a decoded
    /// preview, a preview that finished. The host calls it every frame and
    /// when woken; a failure shows in the Status Bar. Returns whether
    /// anything a window shows changed.
    pub fn poll(&mut self) -> bool {
        self.analysis_cache();
        let warping = auto_warp::Warping {
            prefs: self.prefs.warp,
            analysis: self.analysis.as_ref(),
        };
        let set_bpm = self
            .editor
            .document()
            .settings()
            .tempo
            .bpm_at(self.arrangement.transport().position)
            .get();
        let polled = self
            .browser_view
            .poll(&mut self.audio, &self.media, warping, set_bpm);
        if let Some(error) = polled.error {
            self.set_message(error, true);
        }
        // Spec: browser-tags R3. A database that couldn't be read is set
        // aside and a new one is built; the Status Bar says so.
        if let Some(message) = self.browser_view.take_message() {
            self.set_message(message, true);
        }
        let mut changed = polled.changed | self.poll_processed() | self.poll_stems();
        // Spec: export R41. The export on its thread: its share moves the
        // progress window, and its end lands in the Status Bar.
        changed |= self.poll_export();
        // Spec: multisampler R73. The autosampler makes one sample at a
        // time, so the dialog sees its progress and Cancel stops it.
        self.poll_autosample();
        // A MIDI device plugged in or removed (WP-2.2).
        if self.midi_listed.elapsed() >= MIDI_RELIST {
            changed |= self.refresh_midi();
        }
        changed |= self.learn_midi();
        changed |= self.track_midi_activity();
        // Plug-ins' main-thread work (WP-2.6 to WP-2.8), and the changes
        // they made themselves.
        changed |= self.poll_plugin_scan();
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
        if crate::plugins::apply_param_changes(changes, &mut self.editor, &mut self.devices) {
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

    /// The Automatic Search's switches as they are unless asked otherwise
    /// (file-management R14): the set's project folder and the User
    /// Library are searched.
    fn default_search() -> bl_shell_spec::file_manager::Search {
        bl_shell_spec::file_manager::Search {
            folder: None,
            project: true,
            library: true,
            candidates: Vec::new(),
        }
    }

    /// Searches for the open set's missing or changed files and relinks them
    /// (WP-2.25, file-management R6 and R14): the places the search is
    /// switched on for, each with its subfolders, and the folders the files
    /// were last in. A file with one candidate is relinked to it; a file
    /// with several keeps its row, and the person chooses. Returns how many
    /// were relinked, and the candidates the others have.
    fn relink_missing(
        &mut self,
        extra: &[PathBuf],
        search: &bl_shell_spec::file_manager::Search,
    ) -> (usize, Vec<bl_shell_spec::file_manager::Candidate>) {
        let mut folders: Vec<PathBuf> = Vec::new();
        if search.project
            && let Some(folder) = self.set_path.as_deref().and_then(Path::parent)
        {
            folders.push(folder.to_path_buf());
        }
        if search.library
            && let Some(library) = self.browser_view.user_library()
        {
            folders.push(library.to_path_buf());
        }
        if let Some(folder) = &search.folder {
            folders.push(PathBuf::from(folder));
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
        let Ok(candidates) = self.media.relink_candidates(&folders) else {
            return (0, Vec::new());
        };
        let mut found = Vec::new();
        let mut several = Vec::new();
        for (hash, paths) in candidates {
            if paths.len() == 1 {
                self.media.relocate(hash, &paths[0]);
                found.push((bl_model::ContentHash(*hash.as_bytes()), paths[0].clone()));
            } else {
                several.push(bl_shell_spec::file_manager::Candidate {
                    hash: hash
                        .as_bytes()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                    name: paths
                        .first()
                        .and_then(|path| path.file_name())
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                    paths: paths
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                });
            }
        }
        self.store.relink(&found);
        // A relinked file's failed analysis is made again (R20).
        for (hash, _) in &found {
            let hash = bl_media::ContentHash::from_bytes(hash.0);
            if let Some(cache) = &self.analysis {
                cache.forget(hash);
            }
            self.analysed.remove(&hash);
        }
        self.start_analyses();
        (found.len(), several)
    }

    /// Runs a browser command and carries out what it asks of the rest of
    /// the shell.
    fn run_browser(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        self.analysis_cache();
        let search = self.browser_view.search_folders();
        let context = browser::Context {
            insert_marker: self.arrangement.insert_marker(),
            selected_track: self.arrangement.selected_tracks().first().copied(),
            editor: &mut self.editor,
            audio: &mut self.audio,
            media: &self.media,
            edge_fades: self.prefs.create_fades_on_clip_edges.0,
            warping: auto_warp::Warping {
                prefs: self.prefs.warp,
                analysis: self.analysis.as_ref(),
            },
            collect_files: self.prefs.collect_files_on_export,
            search: &search,
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
        if let Some(message) = outcome.message {
            self.set_message(message, false);
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

    /// The clipboard of copied envelope shapes, if anything has been
    /// copied (WP-6.25).
    #[must_use]
    pub fn envelope_clipboard(&self) -> Option<&EnvelopeClipboard> {
        self.envelope_clipboard.as_ref()
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
    /// labelled with the agent's name, and made by `maker`, so the history
    /// says who made it. A command that changes nothing in the set records
    /// nothing. Undo and redo can't be labelled, nor can a command that
    /// replaces the set (opening or making one).
    ///
    /// # Errors
    ///
    /// As [`Shell::run`]; the set is unchanged.
    pub fn run_labelled(
        &mut self,
        maker: bl_model::Maker,
        label: impl Into<std::borrow::Cow<'static, str>>,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let labelled = !matches!(id, model::UNDO | model::REDO | model::GO_TO_STEP)
            && (model::handles(id)
                || audio_ids::handles(id)
                || freeze_ids::handles(id)
                || bounce_ids::handles(id)
                || edit_ids::handles(id)
                || device_ids::handles(id)
                || multisampler_ids::handles(id)
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
        let name: std::borrow::Cow<'static, str> = label.into();
        self.editor.begin_by(maker, name.clone());
        let result = self.run(id, args);
        if result.is_ok() {
            if self.editor.commit().unwrap_or(false) {
                self.dirty = true;
                // Spec: crash-recovery R1. The step is journalled under the
                // name the undo history gives it.
                self.journal_edit(Some(&name));
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
        let shown = self
            .clip_view
            .shown_any_clip(self.editor.document(), &self.arrangement);
        crate::permission::assess(
            id,
            args,
            registered,
            crate::permission::Scene {
                insert_marker: self.arrangement.insert_marker(),
                selected_track: self.arrangement.selected_tracks().first().copied(),
                shown_clip: shown,
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
                && !locator_ids::handles(&invocation.id)
                && !note_ids::handles(&invocation.id)
                && !is_automation_edit(&invocation.id)
                && !automation_ids::handles(&invocation.id)
                && !clip_marker_command(&invocation.id)
                && !arrangement_clip_command(&invocation.id)
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
            // Spec: crash-recovery R1. The gesture's transaction is
            // journalled as the one edit it was.
            self.journal_edit(None);
        }
        self.document_changed();
        self.touch();
        outcome
    }

    fn gesture_step(&mut self, invocation: &Invocation) -> Result<Value, ShellError> {
        let label = if note_ids::handles(&invocation.id) {
            clip_view::label(&invocation.id)
        } else if automation_ids::handles(&invocation.id) {
            automation::label(&invocation.id)
        } else if locator_ids::handles(&invocation.id) {
            crate::arrangement::locator::label(&invocation.id)
        } else if is_automation_edit(&invocation.id) {
            automation_label(&invocation.id)
        } else if clip_marker_command(&invocation.id) || arrangement_clip_command(&invocation.id) {
            marker_label(&invocation.id)
        } else {
            editing::label(&invocation.id, &invocation.args)
        };
        // Sample editor R8: a frame that inserts a warp marker begins the
        // gesture's base, on which the frames that follow move it.
        let makes_base = invocation.id == "clip.insert_warp_marker";
        let base = if makes_base {
            None
        } else {
            self.edit_gesture.as_ref().and_then(|g| g.base.clone())
        };
        match &self.edit_gesture {
            None => {
                self.edit_gesture = Some(Gesture {
                    view: self.arrangement.clone(),
                    clip_view: self.clip_view.clone(),
                    last: None,
                    base: None,
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
        let launch = self.launch_settings();
        let edge_fades = self.create_fades_on_clip_edges();
        let run = |editor: &mut Editor,
                   arrangement: &mut ArrangementView,
                   clip_view: &mut ClipView,
                   clipboard: &mut Option<Clipboard>,
                   envelope_clipboard: &mut Option<EnvelopeClipboard>,
                   devices: &DeviceView,
                   invocation: &Invocation| {
            if clip_marker_command(&invocation.id) || arrangement_clip_command(&invocation.id) {
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
            } else if automation_ids::handles(&invocation.id) {
                // Spec: automation-editing R2. A drag that moves several
                // breakpoints of an envelope at once.
                automation::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    arrangement,
                    envelope_clipboard,
                    devices,
                )
            } else if is_automation_edit(&invocation.id) {
                // WP-2.12: a drag on an automation lane moves, curves, or
                // deletes breakpoints, as one undo step.
                model::run(editor, &invocation.id, invocation.args.clone())
                    .map(|changes| to_value(&changes))
                    .map_err(|error| {
                        crate::arrangement::editing::EditFailure::Refused(error.to_string())
                    })
            } else if locator_ids::handles(&invocation.id) {
                // The locators' own commands (WP-6.24).
                crate::arrangement::locator::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    arrangement,
                    &launch,
                    None,
                )
            } else {
                editing::run(
                    &invocation.id,
                    invocation.args.clone(),
                    editor,
                    arrangement,
                    clipboard,
                    edge_fades,
                )
            }
        };
        // The warp commands check the files' lengths and fill in the
        // transients, which only the shell can (warping R11, R25, R26).
        let mut steps = Vec::with_capacity(2);
        for step in base.iter().chain(std::iter::once(invocation)) {
            match self.prepare_gesture_step(step) {
                Ok(step) => steps.push(step),
                Err(error) => {
                    let failure = EditFailure::Refused(error.to_string());
                    return self.gesture_refused(&base, &invocation.id, label, failure);
                }
            }
        }
        let mut result = Ok(Value::Null);
        for step in &steps {
            result = run(
                &mut self.editor,
                &mut self.arrangement,
                &mut self.clip_view,
                &mut self.clipboard,
                &mut self.envelope_clipboard,
                &self.devices,
                step,
            );
            if result.is_err() {
                break;
            }
        }
        let Some(gesture) = self.edit_gesture.as_mut() else {
            return Ok(Value::Null);
        };
        match result {
            Ok(value) => {
                gesture.last = Some(invocation.clone());
                if makes_base {
                    gesture.base = Some(invocation.clone());
                }
                if sample_edit_command(&invocation.id) {
                    self.clip_view.sample_follow(true);
                }
                Ok(value)
            }
            Err(failure) => self.gesture_refused(&base, &invocation.id, label, failure),
        }
    }

    /// A gesture frame that was refused: the set goes back to the last frame
    /// that worked.
    fn gesture_refused(
        &mut self,
        base: &Option<Invocation>,
        id: &str,
        label: &'static str,
        failure: EditFailure,
    ) -> Result<Value, ShellError> {
        let Some(last) = self.edit_gesture.as_ref().and_then(|g| g.last.clone()) else {
            return Err(edit_error(id, failure));
        };
        // Back to where the gesture began, then the last frame that worked.
        self.editor.cancel().ok();
        self.editor.begin(label);
        for step in base.iter().chain(std::iter::once(&last)) {
            let Ok(step) = self.prepare_gesture_step(step) else {
                break;
            };
            let outcome = if clip_marker_command(&step.id) || arrangement_clip_command(&step.id) {
                model::run(&mut self.editor, &step.id, step.args.clone())
                    .map(|_| ())
                    .map_err(|error| EditFailure::Refused(error.to_string()))
            } else if note_ids::handles(&step.id) {
                clip_view::run(
                    &step.id,
                    step.args.clone(),
                    &mut self.editor,
                    &mut self.clip_view,
                    &self.arrangement,
                )
                .map(|_| ())
            } else if is_automation_edit(&step.id) {
                model::run(&mut self.editor, &step.id, step.args.clone())
                    .map(|_| ())
                    .map_err(|error| EditFailure::Refused(error.to_string()))
            } else {
                editing::run(
                    &step.id,
                    step.args.clone(),
                    &mut self.editor,
                    &mut self.arrangement,
                    &mut self.clipboard,
                    self.prefs.create_fades_on_clip_edges.0,
                )
                .map(|_| ())
            };
            if outcome.is_err() {
                break;
            }
        }
        Err(edit_error(id, failure))
    }

    /// Takes what the engine last published (the playhead, whether the song
    /// plays, the insert marker) for the Arrangement View and the Control
    /// Bar, with the loop brace from the document. The window calls it
    /// before drawing each frame, so what it draws lags the engine by at
    /// most a frame and an audio block (the control bar transport spec,
    /// R14). Returns the transport it took.
    pub fn poll_engine(&mut self) -> EngineTransport {
        let engine = self.audio.transport_state();
        self.poll_capture();
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
            // Spec: device-framework R35. The display data the devices'
            // nodes published, which the device displays draw. Not a
            // revision either.
            self.audio.displays_into(self.devices.displays_mut());
            // Spec: articulation-map R15. A row whose Learn button is lit
            // takes the next note-on or control change the device hears as
            // its trigger, which is one undo step.
            for (device, row, trigger) in self.devices.learned_triggers() {
                self.devices.stop_learning(device);
                self.edit(|editor| {
                    // The list may have changed while the button was lit,
                    // and then the row is gone: the trigger is dropped
                    // rather than ending the shell.
                    let _ = editor.apply(Command::SetArticulation {
                        device,
                        index: row,
                        name: None,
                        trigger: Some(trigger),
                        outputs: None,
                        channel: None,
                    });
                });
            }
        }
        // Spec: drum-racks R20. With Auto Select on, a drum rack's chain
        // list selects each chain a note reaches.
        if running {
            let mut hits = std::collections::BTreeMap::new();
            self.audio.chain_hits_into(&mut hits);
            if self.devices.auto_select(self.editor.document(), &hits) {
                self.touch();
            }
        }
        // Spec: launch-quantisation R10. The slots the engine is launching
        // or stopping, which the grid's launch buttons draw.
        self.audio.session_into(&mut self.session_states);
        // Spec: scales R11. A clip that starts playing, and the clip the
        // playhead enters, make their scale the current scale.
        self.follow_playing_scale(&engine);
        // A drag in progress waits: its end is one change, not a split for
        // every frame of it.
        if self.recording.is_some()
            && !self.gesture
            && let Some(rate) = self.audio.sample_rate().and_then(SampleRate::new)
        {
            // Spec: session-to-arrangement R3. The clips that play, as they
            // were, for the copies a recording makes.
            self.session_watch.poll(
                self.editor.document(),
                &self.session_states,
                engine.position,
                &self.editor.document().settings().tempo,
                rate,
            );
        }
        self.scene_override = self.audio.scene_override();
        // Spec: session-view R24. The tracks a Session clip has taken over,
        // which the Back to Arrangement buttons light from.
        self.audio.session_over_into(&mut self.session_over);
        // Spec: modulation R7. What the modulated controls played in the
        // last block: the rings and the fader marks draw from it. Spec:
        // racks R37. The chains processing, which Auto Select follows,
        // clear with them when the engine stops.
        if running || self.modulating {
            self.modulating = running;
            self.audio.modulated_into(&mut self.modulated);
            self.audio.processing_into(&mut self.processing);
        }
        if self
            .devices
            .follow_processing(self.editor.document(), &self.processing)
        {
            self.touch();
        }
        // Spec: sample-editor R11. Starting or stopping playback lets the
        // Sample Editor follow it again.
        if engine.playing != self.arrangement.transport().playing {
            self.clip_view.sample_follow(false);
        }
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

    /// Points the set's current scale at the clip that starts playing, and
    /// at the Arrangement clip the playhead enters (the scales spec, R11).
    /// It is no undo step, though the set is marked as changed.
    fn follow_playing_scale(&mut self, engine: &EngineTransport) {
        if !engine.playing {
            self.launched.clear();
            self.playing_clip = None;
            return;
        }
        let doc = self.editor.document().clone();
        let order = doc.track_order();
        let (launches, launched) =
            crate::scales::new_launches(&self.session_states, &self.launched);
        self.launched = launched;
        if let Some(scale) = crate::scales::launched_scale(&doc, &launches, &order) {
            self.follow_scale(scale);
            return;
        }
        let clip = crate::scales::arrangement_clip(&doc, engine.position, &self.session_over);
        if clip != self.playing_clip {
            self.playing_clip = clip;
            if let Some(scale) = clip.and_then(|clip| doc.clip(clip)).map(|clip| clip.scale) {
                self.follow_scale(scale);
            }
        }
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

    /// The scene the Scene View shows in the detail area (L-SES-011): the
    /// first scene selected in the Session View, while it's in front.
    #[must_use]
    pub fn scene_view_scene(&self) -> Option<bl_model::SceneId> {
        (self.main_view == MainView::Session)
            .then(|| self.session.state().scenes.first().copied())
            .flatten()
            .filter(|scene| self.editor.document().scene(*scene).is_some())
    }

    /// The tempo and time signature a launched scene put in place of the
    /// set's, as the engine last published them (WP-3.5): they hold until
    /// Re-Enable Automation or Back to Arrangement. Nothing while no engine
    /// runs.
    #[must_use]
    pub fn scene_override(&self) -> bl_engine::SceneOverride {
        self.scene_override
    }

    /// The tempo in force at the playhead: a launched scene's, or the
    /// set's tempo map there (WP-3.5).
    #[must_use]
    pub fn current_tempo(&self) -> bl_core::time::Bpm {
        self.scene_override.tempo.map_or_else(
            || {
                let at = self.arrangement.transport().position;
                self.editor.document().settings().tempo.bpm_at(at)
            },
            |(_, bpm)| bpm,
        )
    }

    /// The time signature in force at the playhead: a launched scene's, or
    /// the set's map there (WP-3.5).
    #[must_use]
    pub fn current_signature(&self) -> bl_core::time::TimeSignature {
        self.scene_override.signature.map_or_else(
            || {
                let at = self.arrangement.transport().position;
                self.editor
                    .document()
                    .settings()
                    .time_signatures
                    .signature_at(at)
            },
            |(_, signature)| signature,
        )
    }

    /// Sets the chains processing, for a host with no engine and for tests;
    /// a running engine's replace them.
    pub fn set_processing(&mut self, processing: Vec<bl_model::ChainId>) {
        self.processing = processing;
    }

    /// Sets the parameters under modulation, for a host with no engine and
    /// for tests; a running engine's replace them.
    pub fn set_modulated(&mut self, modulated: Vec<bl_engine::PlayedParam>) {
        self.modulated = modulated;
    }

    /// What `target` of `track` played in the last block while a clip's
    /// modulation acted on it (the modulation spec, R7): its base and the
    /// value it played, in the parameter's own units. `None` while it plays
    /// its base.
    #[must_use]
    pub fn played(&self, track: TrackId, target: &AutomationTarget) -> Option<(f64, f64)> {
        let (node, param) = bl_engine::target_node(track, target)?;
        self.modulated
            .iter()
            .find(|entry| entry.node == node && entry.param == param)
            .map(|entry| (entry.base, entry.played))
    }

    /// The tracks a Session clip has taken over from the Arrangement
    /// (WP-3.6): the Main column's and the Arrangement's Back to Arrangement
    /// buttons light while it lists any, and a track's own button while it
    /// lists the track.
    #[must_use]
    pub fn session_over(&self) -> &[TrackId] {
        &self.session_over
    }

    /// Sends a transport command to the engine, as menus, keys, the Control
    /// Bar and agents all do. Recording gives the engine what to write
    /// before it knows the transport records (WP-2.13). A reason no files
    /// can come of it (a set with no project folder, say) is reported in the
    /// Status Bar; the transport still records, as the metronome's "only
    /// while recording" follows it.
    ///
    /// # Errors
    ///
    /// A message if no engine runs.
    pub fn send_transport(&mut self, command: bl_engine::TransportCommand) -> Result<(), String> {
        if command == bl_engine::TransportCommand::SetRecording(true) {
            // Spec: session-recording R11. One recording at a time: a new
            // plan would replace the Session recording's.
            if self.session_run.is_some() {
                return Err("a Session recording is under way; end it first".to_owned());
            }
            if let Err(reason) = self.begin_recording() {
                self.set_message(reason, true);
            }
        }
        self.audio.send_transport(command)
    }

    /// Starts a recording (WP-2.13, WP-2.14): a ring for every armed audio
    /// track whose Audio From is Ext. In, and a writer thread draining them
    /// into the project's `Samples/Recorded/`; and a ring for every armed
    /// MIDI track, which needs no folder. The engine is given the rings
    /// before the transport is told it records.
    fn begin_recording(&mut self) -> Result<(), String> {
        let rate = self
            .audio
            .sample_rate()
            .and_then(SampleRate::new)
            .ok_or("no audio device is open")?;
        let folder = self
            .set_path
            .as_ref()
            .map(|set| ProjectFolder::of_set(set).root().to_path_buf());
        let stamp = crate::recording::stamp();
        self.recorded_takes += 1;
        let take = self.recorded_takes;
        let document = self.editor.document();
        let mut tracks = Vec::new();
        let mut midi = Vec::new();
        for id in document.track_order() {
            let Some(track) = document.track(id) else {
                continue;
            };
            if !track.mixer().arm {
                continue;
            }
            match (track.kind(), &track.routing().input) {
                (TrackKind::Midi, _) => midi.push(id),
                (TrackKind::Audio, Input::External { channel }) => {
                    tracks.push(crate::recording::Track {
                        track: id,
                        channel: *channel,
                        name: crate::recording::file_name(
                            &crate::routing::track_label(document, id),
                            take,
                            &stamp,
                        ),
                    });
                }
                _ => {}
            }
        }
        if folder.is_none() && !tracks.is_empty() {
            // The audio has nowhere to go; the MIDI still records.
            self.set_message(
                "the set has never been saved; use `set.save_as` to give it a project folder, \
                 where its recordings go",
                true,
            );
            tracks.clear();
        }
        let (mut plan, mut recording) =
            crate::recording::start(folder.as_deref(), rate, &tracks, &midi, Vec::new())?;
        self.session_watch = crate::session_capture::Watch::default();
        let delay = self
            .audio
            .recording_delay()
            .ok_or("no audio device is open")?;
        plan.delay = delay;
        recording.delay = delay;
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
        } else {
            self.grow_session_clips(engine);
        }
    }

    /// Writes the control moves of an automation recording into the set
    /// when the transport stops recording (automation R17).
    fn poll_automation_record(&mut self, engine: &EngineTransport) {
        let recording = engine.recording && engine.playing;
        let was = std::mem::replace(&mut self.automation_recording, recording);
        if recording {
            if !was {
                // The recording starts: the take learns the mode and where
                // it began (automation R18 to R21).
                let document = self.editor.document();
                let settings = document.settings();
                self.automation_take.start(
                    self.prefs.automation_record_mode,
                    crate::automation_record::take_start(document, engine.position),
                    settings.arrangement_loop.start,
                );
            }
            self.automation_take.advance(engine.position);
            return;
        }
        if !was {
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

    /// Makes the clips of a finished recording, one undo step (WP-2.13,
    /// WP-2.14): each file becomes an audio clip on its track, and each
    /// armed MIDI track's notes a MIDI clip, from the song frame the take
    /// started on to the one it ended on. Undo removes the clips; the files
    /// stay, as Live keeps recorded files. A report the writer
    /// sent (a failed file) makes no clips and shows in the Status Bar, and
    /// frames the engine dropped because the disk fell behind are reported
    /// there too.
    fn finish_recording(&mut self, outcome: crate::recording::Outcome) {
        let crate::recording::Outcome {
            take,
            delay,
            files,
            midi,
            dropped,
            launches,
        } = outcome;
        self.last_recording = Some(RecordingReport {
            error: None,
            clips_made: false,
            files: files.as_ref().map_or_else(
                |_| Vec::new(),
                |files| {
                    files
                        .iter()
                        .map(|file| RecordedFileReport {
                            track: file.track,
                            path: file.path.display().to_string(),
                            frames: file.frames,
                            channels: file.channels,
                        })
                        .collect()
                },
            ),
            midi_tracks: midi.iter().map(|take| take.track).collect(),
            dropped_frames: dropped,
            delay_frames: delay,
        });
        let files = match files {
            Ok(files) => files,
            Err(error) => {
                self.abandon_session_recording();
                self.fail_recording(format!("Recording failed: {error}"));
                return;
            }
        };
        if let Some(run) = self.session_run.take() {
            // Spec: session-recording R7. The clips go into slots.
            self.finish_session_recording(
                run,
                crate::recording::Outcome {
                    take,
                    delay,
                    files: Ok(files),
                    midi,
                    dropped,
                    launches,
                },
            );
            return;
        }
        if files.is_empty() && midi.is_empty() && launches.is_empty() {
            return;
        }
        let Some(take) = take else {
            self.fail_recording("Recording stopped without a position, so its clips weren't made");
            return;
        };
        let Some(rate) = self.audio.sample_rate().and_then(SampleRate::new) else {
            self.fail_recording("Recording stopped: the audio device closed");
            return;
        };
        let tempo = self.editor.document().settings().tempo.clone();
        // Spec: recording R7. The audio is moved `delay` frames earlier; a
        // negative delay starts it later, where the file's first sample is.
        let start = tempo.frames_to_ticks(take.start + Frames((-delay).max(0)), rate);
        let end = tempo.frames_to_ticks(take.end - Frames(delay), rate);
        let edge_fades = self.create_fades_on_clip_edges();
        let warp_recorded = self.warp_recorded_audio();
        let warp_mode = auto_warp::warp_mode(self.prefs.warp.default_warp_mode);
        let overdub = self.editor.document().settings().midi_overdub;
        // shortcut: a take that went round the loop copies no Session
        // clips; upgrade when Live's behaviour there is observed.
        let watch = std::mem::take(&mut self.session_watch);
        let runs = if take.wrap.is_none() {
            crate::session_capture::runs(&launches, take.end, &tempo, rate)
        } else {
            Vec::new()
        };
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
                    self.fail_recording(format!(
                        "Recording failed: {}: {error}",
                        file.path.display()
                    ));
                    return;
                }
            }
        }
        let applied = match take.wrap {
            None => {
                let mut placed = std::collections::BTreeSet::new();
                let label = if runs.is_empty() {
                    crate::recording::UNDO_LABEL
                } else {
                    crate::recording::LOOP_UNDO_LABEL
                };
                let armed: Vec<TrackId> = imported
                    .iter()
                    .map(|(track, ..)| *track)
                    .chain(midi.iter().map(|take| take.track))
                    .collect();
                let before = self.editor.document().clone();
                self.editor.transact(label, |editor| {
                    for (track, hash, path, _) in &imported {
                        let id = editor.new_id();
                        // Spec: audio-clips R7. With Create Fades on Clip Edges
                        // on, a new recorded clip's edges fade over 4 ms, like a
                        // dropped file's.
                        let audio = AudioData::new(*hash);
                        // Spec: warping R37.
                        let audio = if warp_recorded {
                            auto_warp::warp_recorded(audio, &tempo, start, end, warp_mode)
                        } else {
                            audio
                        };
                        let audio = if edge_fades {
                            audio.with_edge_fades()
                        } else {
                            audio
                        };
                        let mut clip =
                            Clip::new(id, start, end.saturating_sub(start), ClipData::Audio(audio));
                        clip.name = file_stem(path);
                        // Spec: comping R10. The take goes on a take lane too,
                        // and the main lane's clip is a copy of it.
                        crate::recording::put_on_lane(editor, *track, clip.clone())?;
                        clip.id = editor.new_id();
                        crate::arrangement::placing::clear(editor, *track, start, end, &placed)?;
                        placed.insert(clip.id);
                        editor.apply(bl_model::Command::CreateClip {
                            track: *track,
                            clip,
                        })?;
                    }
                    crate::recording::place_midi(
                        editor,
                        &midi,
                        take,
                        rate,
                        overdub,
                        true,
                        &mut placed,
                    )?;
                    crate::session_capture::place(editor, &watch, &runs, &armed, end)?;
                    // Spec: audio-clips R26. A take that meets a clip
                    // already there is crossfaded with it.
                    crate::arrangement::fades::settle(editor, &before, edge_fades)
                })
            }
            Some(_) => {
                let frames = files.first().map_or(0, |file| file.frames);
                self.place_loop_passes(frames, &imported, &midi, take, rate, delay)
            }
        };
        if let Err(error) = applied {
            self.fail_recording(format!("Recording failed: {error}"));
            return;
        }
        if let Some(report) = &mut self.last_recording {
            report.clips_made = true;
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

    /// Shows why a finished recording made no clips, in the Status Bar and
    /// in the last recording's report.
    fn fail_recording(&mut self, message: impl Into<String>) {
        let message = message.into();
        if let Some(report) = &mut self.last_recording {
            report.error = Some(message.clone());
        }
        self.set_message(message, true);
    }

    /// The recording under way, for `transport.state`: its armed tracks and
    /// the frames dropped so far. `None` when no recording is under way.
    #[must_use]
    pub fn running_recording(&self) -> Option<RunningRecording> {
        self.recording.as_ref().map(|recording| RunningRecording {
            armed_tracks: recording.armed().to_vec(),
            dropped_frames: recording.dropped(),
            session_slots: self.session_slots(),
        })
    }

    /// What the last recording that ended came to, for `transport.state`.
    #[must_use]
    pub fn last_recording(&self) -> Option<&RecordingReport> {
        self.last_recording.as_ref()
    }

    /// Puts a take that went round the loop on its tracks (the recording
    /// spec, R12 and R16): the lead-in before the loop's start is a clip on
    /// the main lane, and each pass is an undo step that puts the pass on a
    /// take lane and copies it to the main lane over what was there, so Undo
    /// run again and again unrolls the passes from the last. Every audio clip
    /// plays its pass of the one recorded file, which holds all of them.
    /// `frames` is how many frames each track's file holds: the rings fill
    /// together. A MIDI track's passes are cut from its messages by the
    /// frames the take had run for, and go the same way, the main lane
    /// merging each one with MIDI Arrangement Overdub on. A negative `delay`
    /// places every pass that much later, as it does a plain take's clip.
    fn place_loop_passes(
        &mut self,
        frames: u64,
        imported: &[(TrackId, ContentHash, PathBuf, u64)],
        midi: &[crate::recording::MidiTake],
        take: bl_engine::TakeBounds,
        rate: SampleRate,
        delay: i64,
    ) -> Result<(), bl_model::EditError> {
        let tempo = self.editor.document().settings().tempo.clone();
        let edge_fades = self.create_fades_on_clip_edges();
        let warp_recorded = self.warp_recorded_audio();
        let warp_mode = auto_warp::warp_mode(self.prefs.warp.default_warp_mode);
        let overdub = self.editor.document().settings().midi_overdub;
        let (lead_in, passes) = crate::recording::passes(take, frames);
        let (midi_lead_in, midi_passes) = crate::recording::passes(take, take.frames);
        let later = Frames((-delay).max(0));
        let seconds = |frames: u64| frames as f64 / rate.as_f64();
        let ticks = |frame: Frames| tempo.frames_to_ticks(frame, rate);
        let audio = |hash: ContentHash, pass: &crate::recording::Pass| {
            let (from, to) = (
                seconds(pass.file_from),
                seconds(pass.file_from + pass.frames),
            );
            let audio = AudioData {
                start: from,
                end: Some(to),
                loop_start: from,
                loop_end: Some(to),
                ..AudioData::new(hash)
            };
            // Spec: warping R37.
            let audio = if warp_recorded {
                let start = ticks(pass.song_start + later);
                let end = ticks(pass.song_start + later + Frames(pass.frames as i64));
                auto_warp::warp_recorded(audio, &tempo, start, end, warp_mode)
            } else {
                audio
            };
            if edge_fades {
                audio.with_edge_fades()
            } else {
                audio
            }
        };
        // The pass's clip, named after its file.
        let clip_of =
            |editor: &mut Editor, name: &str, pass: &crate::recording::Pass, data: AudioData| {
                let start = ticks(pass.song_start + later);
                let end = ticks(pass.song_start + later + Frames(pass.frames as i64));
                let mut clip = Clip::new(
                    editor.new_id(),
                    start,
                    end.saturating_sub(start),
                    ClipData::Audio(data),
                );
                clip.name = name.to_owned();
                clip
            };
        let put_on_main =
            |editor: &mut Editor, track: TrackId, clip: Clip| -> Result<(), bl_model::EditError> {
                crate::arrangement::placing::clear(
                    editor,
                    track,
                    clip.start,
                    clip.end(),
                    &std::collections::BTreeSet::new(),
                )?;
                editor.apply(bl_model::Command::CreateClip { track, clip })?;
                Ok(())
            };
        let ends = |pass: &crate::recording::Pass| bl_engine::TakeBounds {
            start: pass.song_start,
            end: pass.song_start + Frames(pass.frames as i64),
            wrap: None,
            frames: pass.frames,
        };
        for step in 0..passes.len().max(midi_passes.len()).max(1) {
            self.editor
                .transact(crate::recording::LOOP_UNDO_LABEL, |editor| {
                    for (track, hash, path, _) in imported {
                        let name = file_stem(path);
                        if let (0, Some(pass)) = (step, &lead_in) {
                            let clip = clip_of(editor, &name, pass, audio(*hash, pass));
                            put_on_main(editor, *track, clip)?;
                        }
                        let Some(pass) = passes.get(step) else {
                            continue;
                        };
                        // Spec: comping R10. The pass goes on a take lane and
                        // is copied to the main lane.
                        let clip = clip_of(editor, &name, pass, audio(*hash, pass));
                        crate::recording::put_on_lane(editor, *track, clip.clone())?;
                        let copy = Clip {
                            id: editor.new_id(),
                            ..clip
                        };
                        put_on_main(editor, *track, copy)?;
                    }
                    let mut placed = std::collections::BTreeSet::new();
                    if let (0, Some(pass)) = (step, &midi_lead_in) {
                        let cut = crate::recording::midi_in_pass(midi, take, pass);
                        crate::recording::place_midi(
                            editor,
                            &cut,
                            ends(pass),
                            rate,
                            overdub,
                            false,
                            &mut placed,
                        )?;
                    }
                    if let Some(pass) = midi_passes.get(step) {
                        let cut = crate::recording::midi_in_pass(midi, take, pass);
                        crate::recording::place_midi(
                            editor,
                            &cut,
                            ends(pass),
                            rate,
                            overdub,
                            true,
                            &mut placed,
                        )?;
                    }
                    Ok::<(), bl_model::EditError>(())
                })?;
        }
        Ok(())
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
            if self.editor.commit().unwrap_or(false) {
                // Spec: crash-recovery R1. The gesture's transaction is
                // journalled as the one edit it was.
                self.journal_edit(None);
            }
            self.touch();
        }
        // The control a drag moved is let go (automation R17).
        self.automation_take
            .release(self.arrangement.transport().position);
        self.release_session_moves();
    }

    /// The effects commands have asked for since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Writes the preferences to the config directory, if there is one, and
    /// the agent server's tokens to their keychain, if one is in use
    /// ([`Shell::use_agent_keychain`]); the file never holds the tokens. A
    /// failure shows in the Status Bar.
    pub fn save_preferences(&mut self) {
        self.store_agent_tokens();
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
            // The floating assistant: only the global keys work in it.
            WindowKind::Assistant => vec![Context::Global],
            // A floating device's window: its device is selected, so the
            // Device View's keys act on it (Float's among them), and the
            // global ones work.
            WindowKind::Device(_) => vec![Context::DeviceView, Context::Global],
            WindowKind::Main => {
                let mut contexts = Vec::with_capacity(3);
                // Spec: crash-recovery R23. While the recovery prompt is
                // open, it has the keyboard: the main window's own keys
                // don't run, so Tab moves between the prompt's buttons and
                // Esc closes it.
                if self.recovery_prompt_open() {
                    return contexts;
                }
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
                        if self.clip_view.tab() == ClipTab::Envelopes {
                            contexts.push(Context::Envelope);
                        } else if self
                            .clip_view
                            .shown_audio_clip(self.editor.document(), &self.arrangement)
                            .is_some()
                        {
                            // Spec: sample-editor R1. An audio clip's editor
                            // is the Sample Editor.
                            contexts.push(Context::SampleEditor);
                        } else {
                            contexts.push(Context::NoteEditor);
                            if !self
                                .clip_view
                                .selected_notes(self.editor.document(), &self.arrangement)
                                .is_empty()
                            {
                                contexts.push(Context::NoteSelection);
                            }
                        }
                    }
                    Area::Detail => {
                        // Spec: racks R22. A chain list with the focus
                        // takes S and the edit keys.
                        if self.devices.chain_list_focus() {
                            contexts.push(Context::RackChains);
                        }
                        contexts.push(Context::DeviceView);
                    }
                    Area::ControlBar | Area::StatusBar | Area::Assistant | Area::UndoHistory => {}
                }
                if self.prefs.tab_navigation {
                    contexts.push(Context::TabFocus);
                }
                contexts.push(Context::Global);
                contexts
            }
        }
    }

    /// Whether a menu item that runs `id` with `args` is the chosen one of
    /// its group now, for the radio pairs a menu shows with a check mark
    /// (Options: Solo Switches and Cue Switches). `None` for commands that
    /// aren't choices.
    #[must_use]
    pub fn choice_state(&self, id: &str, args: &Value) -> Option<bool> {
        match id {
            CUE_MODE => Some(self.editor.document().cue_active() == args["enabled"].as_bool()?),
            // Spec: time-editing R10. The loop brace's menu item is ticked
            // while the song starts at the brace's start.
            locator_ids::SET_START_TIME if args["loop_brace"].as_bool() == Some(true) => Some(
                self.editor.document().settings().song_start
                    == Some(bl_model::SongStart::LoopBrace),
            ),
            _ => None,
        }
    }

    /// Whether a switch-like command is on now: the state a menu shows as a
    /// check mark. `None` for commands that aren't switches.
    #[must_use]
    pub fn switch_state(&self, id: &str) -> Option<bool> {
        match id {
            // Spec: time-editing R10. A locator's menu item is ticked while
            // the song starts at that locator.
            locator_ids::SET_START_TIME => {
                let selected = self.arrangement.selected_locators().first().copied();
                Some(
                    self.editor.document().settings().song_start
                        == selected.map(bl_model::SongStart::Locator),
                )
            }
            ids::VIEW_FULL_SCREEN => Some(self.full_screen),
            ids::VIEW_TOGGLE_BROWSER => Some(self.browser),
            ids::VIEW_TOGGLE_CLIP_VIEW => Some(self.clip_view.visible()),
            ids::VIEW_TOGGLE_PLUGIN_LIST => Some(self.plugin_list.visible()),
            ids::VIEW_DRAW_MODE => Some(self.clip_view.draw_mode()),
            ids::VIEW_FOLD_NOTES => Some(
                self.clip_view
                    .shown_clip(self.editor.document(), &self.arrangement)
                    .is_some_and(|clip| self.clip_view.is_folded(clip)),
            ),
            ids::VIEW_TOGGLE_INFO_VIEW => Some(self.info_view),
            assistant_ids::VIEW_TOGGLE_ASSISTANT => Some(self.assistant_open),
            ids::VIEW_TOGGLE_UNDO_HISTORY => Some(self.undo_history_open),
            ids::VIEW_TOGGLE_PLUG_IN_WINDOWS => Some(self.plug_in_windows),
            ids::FOCUS_TOGGLE_TAB_NAVIGATION => Some(self.prefs.tab_navigation),
            ids::FOCUS_TOGGLE_WRAP_NAVIGATION => Some(self.prefs.wrap_navigation.0),
            session_launch_ids::SET_QUANTIZATION_ENABLED => Some(self.store.launch().enabled),
            SOLO_IN_PLACE => Some(self.editor.document().settings().solo_in_place),
            DELAY_COMPENSATION => Some(self.editor.document().settings().delay_compensation),
            REDUCED_LATENCY_MONITORING => {
                Some(self.editor.document().settings().reduced_latency_monitoring)
            }
            LOCK_ENVELOPES => Some(self.editor.document().settings().lock_envelopes),
            "transport.set_metronome_enabled" => {
                Some(self.editor.document().settings().metronome.enabled)
            }
            "transport.set_midi_overdub" => Some(self.editor.document().settings().midi_overdub),
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
    /// mixer view spec, R12), and the set's backups are disabled while the
    /// set has none (file-management R18).
    #[must_use]
    pub fn can_open_submenu(&self, row: &str) -> bool {
        if row == "M-VIEW-033" && self.main_view == MainView::Session {
            return false;
        }
        if row == "M-FILE-008" {
            return !self.backups().is_empty();
        }
        true
    }

    /// The label a menu shows for a command now: Undo and Redo name the step
    /// they act on, or say there's nothing (M-EDIT-001, M-EDIT-002).
    #[must_use]
    /// The command a menu item runs: the one its row names, or the one
    /// that fits the state, such as Create ▸ Add Locator, which reads
    /// Delete Locator and deletes while a locator is selected (the time
    /// editing spec, R4).
    pub fn menu_command<'a>(&self, id: &'a str) -> &'a str {
        if self.arrangement.selected_locators().is_empty() {
            return id;
        }
        match id {
            // Spec: time-editing R4. Create ▸ Add Locator reads Delete
            // Locator and deletes while a locator is selected.
            locator_ids::ADD => locator_ids::DELETE,
            // Spec: time-editing R8. Edit ▸ Rename renames the selected
            // locator.
            "clip.rename" => locator_ids::RENAME,
            _ => id,
        }
    }

    pub fn dynamic_label(&self, id: &str) -> Option<String> {
        match id {
            // Spec: time-editing R4. The Create menu's Add Locator reads
            // Delete Locator while a locator is selected.
            locator_ids::DELETE => Some("Delete Locator".to_owned()),
            // Spec: audio-clips R28. Create ▸ Create Fade reads Create Fade
            // In/Out or Create Crossfade as the selection gives one.
            edit_ids::CREATE_FADE => Some(
                editing::create_fade_label(self.editor.document(), &self.arrangement).to_owned(),
            ),
            model::UNDO => Some(self.editor.undo_history().next().map_or_else(
                || "Nothing to Undo".to_owned(),
                |step| format!("Undo {step}"),
            )),
            model::REDO => Some(self.editor.redo_history().next().map_or_else(
                || "Nothing to Redo".to_owned(),
                |step| format!("Redo {step}"),
            )),
            // Spec: device-compare R3, R4: the items follow the state of the
            // selected device.
            device_ids::COMPARE_SWITCH | device_ids::COMPARE_COPY => {
                let b = self
                    .devices
                    .selected()
                    .and_then(|d| self.editor.document().device(d))
                    .is_some_and(|d| d.b_active);
                Some(
                    match (id == device_ids::COMPARE_SWITCH, b) {
                        (true, false) => "Compare: Switch to B",
                        (true, true) => "Compare: Switch to A",
                        (false, false) => "Compare: Copy A to B",
                        (false, true) => "Compare: Copy B to A",
                    }
                    .to_owned(),
                )
            }
            // IMP-24: the item follows the selected device's position.
            device_ids::SET_ROLE => {
                let selected = self
                    .devices
                    .selected()
                    .and_then(|device| self.editor.document().device(device));
                Some(match selected {
                    Some(device) if device.role == bl_model::DeviceRole::MidiEffect => {
                        let role = self
                            .devices
                            .device_type(&device.kind)
                            .map_or(bl_model::DeviceRole::AudioEffect, |kind| kind.role);
                        format!("Use as {}", role.label())
                    }
                    _ => "Use as MIDI Effect".to_owned(),
                })
            }
            // Spec: note-editing R18. The item reads Activate Note(s) when
            // every selected note is deactivated already.
            note_ids::DEACTIVATE => Some(
                if self
                    .clip_view
                    .selection_deactivated(self.editor.document(), &self.arrangement)
                {
                    "Activate Note(s)"
                } else {
                    "Deactivate Note(s)"
                }
                .to_owned(),
            ),
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
        self.can_invoke_in(id, args, &self.active_contexts())
    }

    /// The contexts the MIDI Note Editor's context menus work in (C-NOTE,
    /// C-KEYS): the note editor's, and the note selection's while notes are
    /// selected, whatever has the keyboard, because the menu acts on what it
    /// was opened on (Spec: piano-roll R30; note-editing R18).
    #[must_use]
    pub fn note_menu_contexts(&self) -> Vec<Context> {
        let mut contexts = vec![Context::NoteEditor];
        if !self
            .clip_view
            .selected_notes(self.editor.document(), &self.arrangement)
            .is_empty()
        {
            contexts.push(Context::NoteSelection);
        }
        contexts.extend([Context::ClipView, Context::Global]);
        contexts
    }

    /// The contexts a track's title-bar menu works in (WP-4.22): the active
    /// ones, with `track-headers` first, because the menu acts on the
    /// selected title bars whichever view it opened in.
    #[must_use]
    pub fn track_menu_contexts(&self) -> Vec<Context> {
        let mut contexts = self.active_contexts();
        contexts.retain(|c| *c != Context::TrackHeaders);
        contexts.insert(0, Context::TrackHeaders);
        contexts
    }

    /// Whether a comping command can run with `args` now: the menus' enabled
    /// state (the comping spec, R29). Its arguments must read, name things the
    /// set has, and, for Insert Take Lane, there must be a track to add to.
    fn can_comp(&self, id: &str, args: &Value) -> bool {
        let args = if args.is_null() {
            json!({})
        } else {
            args.clone()
        };
        let doc = self.editor.document();
        let lane = |lane: &bl_model::TakeLaneId| doc.take_lane_track(*lane).is_some();
        let track = |track: &TrackId| {
            doc.track(*track)
                .is_some_and(|t| matches!(t.kind(), TrackKind::Audio | TrackKind::Midi))
        };
        match id {
            comping_ids::INSERT => serde_json::from_value::<comping_ids::InsertArgs>(args)
                .ok()
                .is_some_and(|args| {
                    let tracks = args
                        .tracks
                        .unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    !tracks.is_empty() && tracks.iter().all(track)
                }),
            comping_ids::DUPLICATE => serde_json::from_value::<comping_ids::DuplicateArgs>(args)
                .ok()
                .is_some_and(|args| !args.lanes.is_empty() && args.lanes.iter().all(lane)),
            comping_ids::COPY_TO_MAIN_LANE => {
                serde_json::from_value::<comping_ids::CopyToMainLaneArgs>(args)
                    .ok()
                    .is_some_and(|args| !args.lanes.is_empty() && args.lanes.iter().all(lane))
            }
            comping_ids::REPLACE_WITH_TAKE => {
                serde_json::from_value::<comping_ids::ReplaceWithTakeArgs>(args)
                    .ok()
                    .is_some_and(|args| track(&args.track))
            }
            comping_ids::MOVE_COMP_EDGE => {
                serde_json::from_value::<comping_ids::MoveCompEdgeArgs>(args)
                    .ok()
                    .is_some_and(|args| track(&args.track))
            }
            comping_ids::COMP_SOURCES => {
                serde_json::from_value::<comping_ids::CompSourcesArgs>(args)
                    .ok()
                    .is_some_and(|args| track(&args.track))
            }
            _ => false,
        }
    }

    /// The frozen track that refuses `id`, if any (the freeze and bounce
    /// spec, R8).
    fn frozen_blocks(&self, id: &str, args: &Value) -> Option<TrackId> {
        // Spec: freeze-and-bounce R30. A frozen track's take lanes play no part
        // in the freeze, but a lane can't be auditioned on it and nothing is
        // copied from one to the main lane.
        let doc = self.editor.document();
        let lane_track = |lane: &Value| {
            let lane = serde_json::from_value::<bl_model::TakeLaneId>(lane.clone()).ok()?;
            doc.take_lane_track(lane)
        };
        let frozen = |track: Option<TrackId>| {
            track.filter(|track| doc.track(*track).is_some_and(|t| t.is_frozen()))
        };
        match id {
            "take_lane.set_audition" if args.get("on").and_then(Value::as_bool) == Some(true) => {
                return frozen(args.get("lane").and_then(lane_track));
            }
            // It copies from one lane on each track it names.
            comping_ids::COPY_TO_MAIN_LANE => {
                return args
                    .get("lanes")
                    .and_then(Value::as_array)
                    .and_then(|lanes| lanes.iter().find_map(|lane| frozen(lane_track(lane))));
            }
            comping_ids::REPLACE_WITH_TAKE => {
                return frozen(
                    args.get("track")
                        .and_then(|track| serde_json::from_value::<TrackId>(track.clone()).ok()),
                );
            }
            _ => {}
        }
        // R7 keeps a frozen track's mixer live: volume, pan, sends, the
        // activator, solo and the crossfader all work; only its devices, its
        // clips' own settings, arming and new clips are locked (R8).
        if !(id.starts_with("device.")
            || id.starts_with("clip.")
            || matches!(
                id,
                "mixer.set_arm"
                    | "mixer.arm_exclusive"
                    | "browser.add_to_arrangement"
                    | "browser.insert"
                    | "track.create"
            ))
        {
            return None;
        }
        // R7: moving, copying, deleting, trimming and launching a frozen
        // clip still work.
        if matches!(
            id,
            "clip.move"
                | "clip.move_by"
                | "clip.move_to_slot"
                | "clip.delete"
                | "clip.set_length"
                | "clip.trim_start"
                | "clip.set_launch_mode"
                | "clip.set_launch_quantization"
                | "clip.set_legato"
                | "clip.set_follow_actions"
        ) {
            return None;
        }
        let doc = self.editor.document();
        if let Some(clip) = args
            .get("clip")
            .and_then(|value| serde_json::from_value::<bl_model::ClipId>(value.clone()).ok())
        {
            // A take-lane clip plays no part in the freeze (R30).
            return doc
                .clip_track(clip)
                .filter(|_| doc.clip_lane(clip).is_none())
                .filter(|track| doc.track(*track).is_some_and(|t| t.is_frozen()));
        }
        if let Some(device) = args
            .get("device")
            .and_then(|value| serde_json::from_value::<bl_model::DeviceId>(value.clone()).ok())
        {
            return doc
                .device_track(device)
                .filter(|track| doc.track(*track).is_some_and(|t| t.is_frozen()));
        }
        args.get("track")
            .and_then(|value| serde_json::from_value::<TrackId>(value.clone()).ok())
            .filter(|track| doc.track(*track).is_some_and(|t| t.is_frozen()))
    }

    /// [`Shell::can_invoke`] with `active` as the contexts that apply.
    #[must_use]
    pub fn can_invoke_in(&self, id: &str, args: &Value, active: &[Context]) -> bool {
        let Some(spec) = self.registry.get(id) else {
            return false;
        };
        if self.edit_gesture.is_some() {
            // A drag is under way; keys wait for it (Esc cancels it).
            return false;
        }
        if !spec.contexts.iter().any(|context| active.contains(context)) {
            return false;
        }
        if freeze_ids::handles(id) {
            return self.can_freeze(id, args);
        }
        if bounce_ids::handles(id) {
            return self.can_bounce(id, args);
        }
        if comping_ids::handles(id) {
            return self.can_comp(id, args);
        }
        if audio_ids::handles(id) {
            // Spec: audio-clip-editing R28. The menus' enabled states.
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
                && self.can_audio_edit(id, &args);
        }
        match id {
            model::UNDO => return self.editor.can_undo(),
            model::REDO => return self.editor.can_redo(),
            ids::APP_HIDE => return cfg!(target_os = "macos"),
            ids::SET_OPEN | ids::SET_SAVE | ids::SET_SAVE_AS | ids::APP_SETTINGS => return true,
            ids::SET_DISCARD_CHANGES => return self.unsaved_changes(),
            // Options ▸ Reduced Latency When Monitoring is available only
            // while Delay Compensation is on (the delay-compensation spec,
            // R8).
            REDUCED_LATENCY_MONITORING => {
                return self.editor.document().settings().delay_compensation;
            }
            ids::FOCUS_NEXT_NEIGHBOR => return self.has_neighbor(false),
            ids::FOCUS_PREVIOUS_NEIGHBOR => return self.has_neighbor(true),
            // Spec: time-editing R8. Cmd-R renames the selected locators
            // while there are any.
            "clip.rename" => return self.arrangement.selected_locators().is_empty(),
            // Zoom In and Zoom Out stop at the zoom's ends (interface-zoom
            // R3).
            ids::VIEW_ZOOM_IN => return self.zoom_percent() < ZOOM_MAX_PERCENT,
            ids::VIEW_ZOOM_OUT => return self.zoom_percent() > ZOOM_MIN_PERCENT,
            // A person's Zoom setting always names a percentage in range, as
            // the Customization settings do (themes R7).
            ids::VIEW_SET_ZOOM
            | ids::SETTINGS_SET_GRID_LINE_INTENSITY
            | ids::SETTINGS_SET_BRIGHTNESS
            | ids::SETTINGS_SET_COLOR_INTENSITY
            | ids::SETTINGS_SET_COLOR_HUE => {
                return spec
                    .args
                    .as_ref()
                    .is_some_and(|schema| bl_commands::validate(schema, args).is_ok());
            }
            // The frontmost window must be a secondary one: with only the
            // main window open, Close Window is disabled (M-FILE-004).
            // Live's Plug-In Windows is disabled with none open (M-VIEW-028).
            ids::VIEW_TOGGLE_PLUG_IN_WINDOWS => return self.any_plug_in_window(),
            // IMP-25. Close All Plug-in Windows is disabled with no editor
            // open: it never closes a floating native device window.
            ids::CLOSE_PLUG_IN_WINDOWS => return self.any_plug_in_editor(),
            ids::WINDOW_CLOSE => {
                return match self.focused_window {
                    WindowKind::Settings => self.settings_open,
                    // Closing the floating assistant closes the panel.
                    WindowKind::Assistant => self.assistant_window_open(),
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
        if stems_ids::handles(id) {
            return self.can_stems(id, args);
        }
        if export_ids::handles(id) {
            return self.can_export(id);
        }
        if autosampler_ids::handles(id) {
            return self.can_autosampler(id, args);
        }
        if slice_ids::handles(id) {
            // Spec: slice-to-midi R1. The menus' enabled states: one audio
            // clip selected, in either view.
            return self.can_slice(id, args);
        }
        if id == CUE_MODE {
            // Spec: mixer S10, R24. Solo Switches and Cue Switches, and the
            // Main strip's button, are dimmed while Cue Out and Main Out
            // share a channel.
            return self.editor.document().cue_available();
        }
        if capture_ids::handles(id) {
            // Spec: capture-midi R1. Nothing played, nothing to capture.
            return self.capture_midi_ready() && self.audio.engine_running();
        }
        if ids::handles(id) || transport_ids::handles(id) {
            return true;
        }
        if id == browser_ids::LOAD_SELECTED
            && (self.devices_load().is_some()
                || self.hot_swapped_layer_file().is_some()
                || self.hot_swap_sample().is_some()
                || self.browser_view.selected_plugin().is_some())
        {
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
                track_ids::SET_DEVICES_ENABLED => {
                    // IMP-25. It works while the selected tracks hold a
                    // top-level device to switch.
                    let named: Option<Vec<TrackId>> = args
                        .get("tracks")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let tracks =
                        named.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
                    return tracks.iter().any(|&track| {
                        self.editor
                            .document()
                            .track(track)
                            .is_some_and(|track| track.devices().next().is_some())
                    });
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
            // A person names the device in its title bar; with a macro or a
            // variation selected, Rename names that instead (macros spec,
            // R14, R29).
            return self.devices.selected().is_some()
                || self.devices.macro_selected().is_some()
                || self.devices.variation_selected().is_some();
        }
        if id == "chain.rename" && args.get("name").is_none() {
            // A person names the chain in its row.
            return self.devices.selected_chains(self.editor.document()).len() == 1;
        }
        if id == "track.delete" && args.get("track").is_none() {
            return !self.deletable_tracks().is_empty();
        }
        if id == "track.rename" && args.get("name").is_none() {
            return self.renamable_track(args).is_some();
        }
        if id == device_ids::SET_FLOATING
            || matches!(id, device_ids::COMPARE_SWITCH | device_ids::COMPARE_COPY)
        {
            // IMP-14: only native devices float; plug-ins open their own
            // editor windows. Spec: device-compare R6: only native devices
            // have A and B states.
            let named = args
                .get("device")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let device = named
                .or(self.devices.selected())
                .and_then(|d| self.editor.document().device(d));
            let fits = if id == device_ids::SET_FLOATING {
                device.is_some_and(bl_model::Device::can_float)
            } else {
                device.is_some_and(bl_model::Device::can_compare)
            };
            if !fits {
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
                // Spec: time-editing R4. The Delete key deletes the
                // selected locators while there are any.
                && !(id == edit_ids::DELETE
                    && (!self.arrangement.selected_locators().is_empty()
                        // Spec: audio-clips R29. Delete deletes the selected
                        // fade handle's fade instead.
                        || self.arrangement.selected_fade().is_some()))
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
            // Back to Arrangement is off in a set no clip has taken over
            // (M-PLAY-012).
            return self.audio.engine_running()
                && (id != "session.back_to_arrangement" || !self.session_over.is_empty())
                && engine_session::command_from_args(id, Self::object(args.clone())).is_ok();
        }
        if session_record_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            return self.audio.engine_running()
                && spec
                    .args
                    .as_ref()
                    .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok());
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
                    || !self.session.selected_slots().is_empty())
                && (id != session_launch_ids::LAUNCH_SELECTED
                    || !self.launch_targets().is_empty());
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
        if locator_ids::handles(id) {
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            if !spec
                .args
                .as_ref()
                .is_some_and(|schema| bl_commands::validate(schema, &args).is_ok())
            {
                return false;
            }
            return match id {
                locator_ids::DELETE => {
                    args.get("locators").is_some()
                        || !self.arrangement.selected_locators().is_empty()
                }
                // Spec: time-editing R13. The jumps need the engine.
                locator_ids::JUMP | locator_ids::JUMP_PREVIOUS | locator_ids::JUMP_NEXT => {
                    self.audio.engine_running()
                }
                _ => true,
            };
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
        if id == bl_shell_spec::warping::QUANTIZE_AUDIO {
            if self.is_sample_key(id, args) {
                return self.complete_sample_command(id, args.clone()).is_ok();
            }
            let named: Option<bl_model::ClipId> = args
                .get("clip")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            if let Some(clip) = named {
                let doc = self.editor.document();
                return match doc.clip(clip).map(|c| &c.data) {
                    Some(ClipData::Audio(audio)) => audio.warp,
                    _ => false,
                };
            }
        }
        if self.is_sample_key(id, args) {
            return self.complete_sample_command(id, args.clone()).is_ok();
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
        if id == "transport.set_midi_overdub" && !fields.contains_key("enabled") {
            // Options ▸ MIDI Arrangement Overdub switches the setting to the
            // other state.
            let on = self.editor.document().settings().midi_overdub;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == LOCK_ENVELOPES && !fields.contains_key("enabled") {
            // Spec: automation-editing R8. Options ▸ Lock Envelopes switches
            // the setting to the other state.
            let on = self.editor.document().settings().lock_envelopes;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == SOLO_IN_PLACE && !fields.contains_key("enabled") {
            // Spec: accessibility R9. Options ▸ Solo in Place switches the
            // setting to the other state.
            let on = self.editor.document().settings().solo_in_place;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == DELAY_COMPENSATION && !fields.contains_key("enabled") {
            // Options ▸ Delay Compensation switches the setting to the
            // other state (the delay-compensation spec, R7).
            let on = self.editor.document().settings().delay_compensation;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == REDUCED_LATENCY_MONITORING && !fields.contains_key("enabled") {
            // Options ▸ Reduced Latency When Monitoring switches the
            // setting to the other state (the delay-compensation spec, R8).
            let on = self.editor.document().settings().reduced_latency_monitoring;
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

    /// The tracks Delete reaches: the selected ones, less Main, which can't
    /// be deleted.
    fn deletable_tracks(&self) -> Vec<TrackId> {
        let doc = self.editor.document();
        self.arrangement
            .selected_tracks()
            .iter()
            .copied()
            .filter(|&t| doc.track(t).is_some_and(|t| t.kind() != TrackKind::Main))
            .collect()
    }

    /// The track Rename reaches: the one `args` names, else the first
    /// selected, unless it's Main, whose name is fixed.
    fn renamable_track(&self, args: &Value) -> Option<TrackId> {
        let named = args
            .get("track")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        named
            .or_else(|| self.arrangement.selected_tracks().first().copied())
            .filter(|&t| {
                self.editor
                    .document()
                    .track(t)
                    .is_some_and(|t| t.kind() != TrackKind::Main)
            })
    }

    /// The locator Rename reaches: the one `args` names, else the first
    /// selected.
    fn renamable_locator(&self, args: &Value) -> Option<bl_model::LocatorId> {
        let named: Option<Vec<bl_model::LocatorId>> = args
            .get("locators")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        named
            .and_then(|mut locators| {
                if locators.is_empty() {
                    None
                } else {
                    Some(locators.remove(0))
                }
            })
            .or_else(|| self.arrangement.selected_locators().first().copied())
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
            ids::SET_NEW | ids::APP_QUIT if self.unsaved_changes() => {
                self.effects.push(Effect::ConfirmDiscard {
                    then: Invocation::new(id, args),
                });
                Ok(Value::Null)
            }
            ids::SET_OPEN if !has("path") => {
                if self.unsaved_changes() {
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
            ids::SET_OPEN if self.unsaved_changes() => {
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
            ids::SETTINGS_SET_TEMPORARY_FOLDER if !has("path") => {
                self.effects.push(Effect::PickTemporaryFolder);
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_USER_LIBRARY if !has("path") => {
                self.effects.push(Effect::PickUserLibrary);
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_SAMPLE_EDITOR if !has("path") => {
                self.effects.push(Effect::PickSampleEditor);
                Ok(Value::Null)
            }
            file_manager_ids::SET_SEARCH_FOLDER if !has("folder") => {
                // Spec: file-management R14. The Set Folder… button asks the
                // person for the folder the search looks in.
                self.effects.push(Effect::PickSearchFolder);
                Ok(Value::Null)
            }
            _ if self.is_sample_key(id, &args) => {
                // Spec: sample-editor R10. A key or a menu item acts on what
                // the Sample Editor has selected.
                let (id, args) = self.complete_sample_command(id, args)?;
                self.run(&id, args)
            }
            _ if !has("track") && Switch::of_command(id).is_some() => {
                let switch = Switch::of_command(id).expect("just checked");
                self.toggle_tracks(id, switch, args)
            }
            "track.delete" if !has("track") => {
                // Spec: track-context-menu R4. Delete acts on every selected
                // track, as one undo step.
                let tracks = self.deletable_tracks();
                if tracks.is_empty() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a track first".to_owned(),
                    });
                }
                let opened = !self.gesture;
                if opened {
                    self.begin_gesture(track_ids::DELETE_STEP);
                }
                let mut result = Ok(Value::Null);
                for track in tracks {
                    // A group's delete took its selected members with it.
                    if self.editor.document().track(track).is_none() {
                        continue;
                    }
                    result = self.run(id, json!({ "track": track }));
                    if result.is_err() {
                        break;
                    }
                }
                if opened {
                    self.end_gesture();
                }
                result
            }
            "track.rename" if !has("name") => {
                let track = self
                    .renamable_track(&args)
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a track first".to_owned(),
                    })?;
                self.effects.push(Effect::RenameTrack(track));
                self.touch();
                Ok(Value::Null)
            }
            locator_ids::RENAME if !has("name") => {
                // Spec: time-editing R8. Cmd-R and Edit ▸ Rename type the
                // new name in the scrub area.
                let locator =
                    self.renamable_locator(&args)
                        .ok_or_else(|| ShellError::Unavailable {
                            command: id.to_owned(),
                            reason: "select a locator first".to_owned(),
                        })?;
                self.effects.push(Effect::RenameLocator(locator));
                self.touch();
                Ok(Value::Null)
            }
            "device.rename" if !has("name") => {
                let args = self.filled_args(id, args)?;
                // Spec: macros R29. With a variation selected, Rename names
                // it, not the device.
                if let Some((rack, index)) = self.devices.variation_selected() {
                    self.effects.push(Effect::RenameVariation(rack, index));
                    self.touch();
                    return Ok(Value::Null);
                }
                // Spec: macros R14. With a macro selected, Rename names it,
                // not the device.
                if let Some((rack, number)) = self.devices.macro_selected() {
                    self.effects.push(Effect::RenameMacro(rack, number));
                    self.touch();
                    return Ok(Value::Null);
                }
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
            "chain.rename" if !has("name") => {
                let chain = args
                    .get("chain")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .or_else(|| {
                        self.devices
                            .selected_chains(self.editor.document())
                            .first()
                            .copied()
                    })
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a chain first".to_owned(),
                    })?;
                self.effects.push(Effect::RenameChain(chain));
                self.touch();
                Ok(Value::Null)
            }
            "rack.rename_macro" if !has("name") => {
                let (rack, number) =
                    self.devices
                        .macro_selected()
                        .ok_or_else(|| ShellError::Unavailable {
                            command: id.to_owned(),
                            reason: "select a macro first".to_owned(),
                        })?;
                self.effects.push(Effect::RenameMacro(rack, number));
                self.touch();
                Ok(Value::Null)
            }
            "device.duplicate" if !has("device") && self.devices.variation_selected().is_some() => {
                // Spec: macros R29. With a variation selected, Duplicate
                // copies it, not the device.
                let (rack, index) = self
                    .devices
                    .variation_selected()
                    .expect("checked to be selected");
                self.run(
                    "rack.duplicate_variation",
                    json!({ "rack": rack, "index": index }),
                )
            }
            "device.remove" if !has("device") && self.devices.variation_selected().is_some() => {
                // Spec: macros R29. With a variation selected, Delete removes
                // it, not the device.
                let (rack, index) = self
                    .devices
                    .variation_selected()
                    .expect("checked to be selected");
                self.run(
                    "rack.delete_variation",
                    json!({ "rack": rack, "index": index }),
                )
            }
            _ if device_ids::ON_SELECTED.contains(&id) && !has("device") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            SOLO_IN_PLACE | LOCK_ENVELOPES if !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            DELAY_COMPENSATION | REDUCED_LATENCY_MONITORING if !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            "transport.set_midi_overdub" if !has("enabled") => {
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
        self.poll_capture();
        let arranged = self.arrangement.selected_clips().to_vec();
        let slot = self.selected_slot_clip();
        let shown = self.shown_clip();
        let revision = self.document_revision;
        let generation = self.editor_generation;
        let result = self.run_command(id, args);
        if self.search.open && !search_ids::handles(id) {
            // Spec: command-palette R7, R10. The rows' availability and the
            // outlines follow the set as it changes.
            self.refresh_search();
        }
        if result.is_ok() {
            self.follow_clip_selection(id, &arranged, slot);
            // Spec: scales R10. Selecting a clip makes its scale the
            // current scale, and a change to that clip's scale changes it
            // with it.
            self.follow_shown_scale(id, shown);
            if self.document_revision != revision && self.editor_generation == generation {
                // Spec: crash-recovery R1. The finished edit is journalled,
                // so the set can be recovered after a crash.
                self.journal_edit(None);
            }
        }
        // Spec: crash-recovery R27. The Status Bar says when the journal
        // can't be written.
        self.check_recovery();
        result
    }

    /// The clip the Clip View shows: the Session clip most recently
    /// selected, or else the Arrangement's first selected clip.
    fn shown_clip(&self) -> Option<bl_model::ClipId> {
        self.clip_view
            .shown_any_clip(self.editor.document(), &self.arrangement)
    }

    /// Points the set's current scale at the clip the Clip View shows, when
    /// the selection changed or the clip's own scale did (the scales spec,
    /// R10). It is no undo step, though the set is marked as changed.
    fn follow_shown_scale(&mut self, id: &str, shown: Option<bl_model::ClipId>) {
        let now = self.shown_clip();
        let changed = now != shown;
        if !changed && id != scales_ids::SET_CLIP_SCALE {
            return;
        }
        let Some(scale) = crate::scales::shown_scale(self.editor.document(), now) else {
            return;
        };
        self.follow_scale(scale);
    }

    /// Points the set's current scale at `scale` (the scales spec, R10, R11
    /// and R12): no undo step, though the set is marked as changed.
    fn follow_scale(&mut self, scale: bl_model::Scale) {
        if self.editor.document().settings().scale == scale {
            return;
        }
        self.editor.follow_scale(scale);
        self.dirty = true;
        self.document_changed();
        self.touch();
    }

    /// The clip in the Session slot selected first, if the slot holds one.
    fn selected_slot_clip(&self) -> Option<bl_model::ClipId> {
        let slot = self.session.first_slot()?;
        self.editor
            .document()
            .slot(slot.track, slot.scene)
            .map(|clip| clip.id)
    }

    /// The clips the Control Bar's scale controls change (the scales spec,
    /// R12): the Arrangement's selected clips, or the Session slot's clip.
    #[must_use]
    pub fn scale_targets(&self) -> Vec<bl_model::ClipId> {
        let arranged = self.arrangement.selected_clips();
        if !arranged.is_empty() {
            return arranged.to_vec();
        }
        self.selected_slot_clip().into_iter().collect()
    }

    /// What the Control Bar's scale controls show (the scales spec, R6): the
    /// selected clips' scale, where every one of them has it, and the set's
    /// current scale when no clip is selected.
    #[must_use]
    pub fn scale_display(&self) -> crate::scales::Display {
        let targets = self.scale_targets();
        crate::scales::display(self.editor.document(), &targets)
    }

    /// Points the Clip View at the clip most recently selected, in the
    /// Session View or the Arrangement, after the command `id` ran with the
    /// selections `arranged` and `slot` before it. Selecting again counts:
    /// clicking the slot that is already selected shows its clip again.
    fn follow_clip_selection(
        &mut self,
        id: &str,
        arranged: &[bl_model::ClipId],
        slot: Option<bl_model::ClipId>,
    ) {
        let now = self.selected_slot_clip();
        let picked = matches!(
            id,
            session_ids::SELECT | session_ids::SELECT_ALL | session_ids::SELECT_EDGE
        ) || now != slot;
        if let Some(clip) = now.filter(|_| picked) {
            self.clip_view.show_session_clip(clip);
        } else if (matches!(id, edit_ids::SELECT_CLIPS | edit_ids::SELECT_ALL)
            || self.arrangement.selected_clips() != arranged)
            && !self.arrangement.selected_clips().is_empty()
        {
            self.clip_view.show_arranged_clips();
        }
    }

    /// Checks a document edit against the media and fills in what only the
    /// app knows, before it runs (the warping spec, R7, R11, R23 to R27 and
    /// R39 to R41). [`Shell::run`] does it for every edit; an agent's batch,
    /// which runs edits with the registry, calls it for each of its steps.
    ///
    /// # Errors
    ///
    /// If the edit names media it can't use, or needs an analysis that isn't
    /// ready.
    /// Runs an edit through `bl_model::registry::run`, inside a transaction
    /// named for the macro when the command moves one, and writes the values
    /// the macro law gives each mapped parameter before the step closes
    /// (macros spec, R13).
    fn run_document_edit(&mut self, id: &str, args: Value) -> Result<ChangeSet, ShellError> {
        let command = model::command_from_args(id, args.clone()).ok();
        // Spec: macros R11. A mapped parameter belongs to its macro: its own
        // control and `device.set_param` are refused, naming the macro.
        if let Some(bl_model::Command::SetDeviceParam { device, param, .. }) = &command
            && let Some(name) = self
                .devices
                .macro_holding(self.editor.document(), *device, *param)
        {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!(
                    "{device}'s parameter {} is held by macro \"{name}\"",
                    param.0
                ),
            });
        }
        let edit = command
            .as_ref()
            .and_then(|command| self.macro_edit(command));
        let Some(edit) = edit else {
            return model::run(&mut self.editor, id, args).map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: error.to_string(),
            });
        };
        let label = match &edit {
            MacroEdit::Macro { label, .. } => std::borrow::Cow::Owned(label.clone()),
            _ => std::borrow::Cow::Borrowed(
                command
                    .as_ref()
                    .map_or("Change macros", bl_model::Command::label),
            ),
        };
        self.editor.begin(label);
        let changes = match model::run(&mut self.editor, id, args) {
            Ok(changes) => changes,
            Err(error) => {
                self.editor.cancel().ok();
                return Err(ShellError::Failed {
                    command: id.to_owned(),
                    message: error.to_string(),
                });
            }
        };
        let updates = self.macro_edit_commands(&edit);
        for update in &updates {
            if let Err(error) = self.editor.apply(update.clone()) {
                self.editor.cancel().ok();
                return Err(ShellError::Failed {
                    command: id.to_owned(),
                    message: error.to_string(),
                });
            }
        }
        self.editor.commit().ok();
        Ok(changes)
    }

    /// Applies the document edits `steps` as one undo step named `label`,
    /// each with the macro law's follow-ups (macros spec, R6, R7, R11 and
    /// R13), as [`Shell::run`] does for one. The agent's `run` tool calls it
    /// for a batch of edits, so a macro moves the same way through the agent
    /// API as through a gesture. The step is made by `maker`, so the history
    /// says who made it.
    ///
    /// # Errors
    ///
    /// [`ShellError`] saying which step failed and why; the set is unchanged.
    pub fn run_document_edits(
        &mut self,
        maker: bl_model::Maker,
        label: &str,
        steps: &[(String, Value)],
    ) -> Result<(Vec<Value>, bool), ShellError> {
        self.editor.begin_by(maker, label.to_owned());
        let start = self.editor.document().clone();
        let mut results = Vec::with_capacity(steps.len());
        for (index, (id, args)) in steps.iter().enumerate() {
            let shown = self.shown_clip();
            match self.run_document_edit(id, args.clone()) {
                Ok(changes) => {
                    // Spec: scales R10. A change to the shown clip's scale
                    // changes the current scale with it, however the change
                    // was made.
                    self.follow_shown_scale(id, shown);
                    results.push(json!({ "command": id, "result": changes }))
                }
                Err(error) => {
                    self.editor.cancel().ok();
                    return Err(ShellError::Failed {
                        command: id.clone(),
                        message: format!("command {} (`{id}`): {error}", index + 1),
                    });
                }
            }
        }
        let changed = !ChangeSet::between(&start, self.editor.document()).is_empty();
        self.editor.commit().ok();
        if changed {
            self.dirty = true;
            self.document_changed();
            // Spec: crash-recovery R1. The batch is journalled as the one
            // edit it was.
            self.journal_edit(Some(label));
        }
        self.touch();
        Ok((results, changed))
    }

    /// The macro edit `command` makes, if it makes one.
    fn macro_edit(&self, command: &bl_model::Command) -> Option<MacroEdit> {
        let doc = self.editor.document();
        match command {
            bl_model::Command::SetDeviceParam { device, param, .. } => {
                let number = bl_model::macro_number(*param)?;
                let rack = doc.device(*device)?;
                rack.rack_kind()?;
                let name = self.devices.macro_name(doc, rack, number);
                Some(MacroEdit::Macro {
                    rack: *device,
                    number,
                    label: format!("Change \"{name}\""),
                })
            }
            bl_model::Command::MapMacro {
                rack,
                macro_number,
                target,
            } => {
                let device = doc.device(*rack)?;
                let first = device.macros.mappings_of(*macro_number).next().is_none();
                let before = first
                    .then(|| self.devices.target_value(doc, *target))
                    .flatten();
                Some(MacroEdit::Mapping {
                    rack: *rack,
                    number: *macro_number,
                    target: *target,
                    first: before,
                })
            }
            bl_model::Command::SetMappingRange { rack, target, .. }
            | bl_model::Command::InvertMapping { rack, target } => Some(MacroEdit::Target {
                rack: *rack,
                target: *target,
            }),
            bl_model::Command::RandomizeMacros { rack, .. } => {
                Some(MacroEdit::Randomize { rack: *rack })
            }
            bl_model::Command::SetMacroStyle {
                rack, macro_number, ..
            } => Some(MacroEdit::Style {
                rack: *rack,
                number: *macro_number,
            }),
            bl_model::Command::LaunchVariation { rack, .. } => {
                Some(MacroEdit::Variation { rack: *rack })
            }
            _ => None,
        }
    }

    /// The commands that write the values `edit` gives the parameters it
    /// maps, after the edit itself has run.
    fn macro_edit_commands(&self, edit: &MacroEdit) -> Vec<bl_model::Command> {
        let doc = self.editor.document();
        match edit {
            MacroEdit::Macro { rack, number, .. } => {
                doc.device(*rack).map_or_else(Vec::new, |rack| {
                    self.devices.macro_commands(doc, rack, *number)
                })
            }
            MacroEdit::Mapping {
                rack,
                number,
                target,
                first,
            } => {
                let Some(device) = doc.device(*rack) else {
                    return Vec::new();
                };
                if let Some(value) = first {
                    // R6. The macro moves to the position that gives the
                    // parameter the value it has, so nothing jumps.
                    let Some(info) = self.devices.target_info(doc, *target) else {
                        return Vec::new();
                    };
                    let position = info.travel.position(*value);
                    // Spec: macros R22. A stepped macro takes only its
                    // steps, so the macro moves to the nearest step, and
                    // the parameter takes the value that step gives.
                    let step = device.macros.setting(*number).position_of(position);
                    let mut commands = vec![bl_model::Command::SetMappedValue {
                        target: bl_model::MacroTarget::Device {
                            device: *rack,
                            param: bl_model::macro_param(*number),
                        },
                        value: step,
                    }];
                    if step != position
                        && let Some(mapping) = device.macros.mapping(*target)
                    {
                        commands.push(bl_model::Command::SetMappedValue {
                            target: *target,
                            value: info.travel.value(mapping.position(step)),
                        });
                    }
                    commands
                } else {
                    self.devices.target_commands(doc, device, *target)
                }
            }
            MacroEdit::Target { rack, target } => doc.device(*rack).map_or_else(Vec::new, |rack| {
                self.devices.target_commands(doc, rack, *target)
            }),
            MacroEdit::Randomize { rack } => doc
                .device(*rack)
                .map_or_else(Vec::new, |rack| self.devices.randomized_commands(doc, rack)),
            MacroEdit::Style { rack, number } => doc.device(*rack).map_or_else(Vec::new, |rack| {
                self.devices.macro_commands(doc, rack, *number)
            }),
            MacroEdit::Variation { rack } => doc.device(*rack).map_or_else(Vec::new, |rack| {
                (1..=bl_model::MACROS)
                    .filter(|number| !rack.macros.setting(*number).variation_excluded)
                    .flat_map(|number| self.devices.macro_commands(doc, rack, number))
                    .collect()
            }),
        }
    }

    pub fn prepare_edit(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        self.check_warp_files(id, &args)?;
        let args = self.prepare_device_role(id, args)?;
        let args = self.prepare_scale_clips(id, args)?;
        self.prepare_warping(id, args)
    }

    /// Spec: scales R5. `clip.set_scale` names the clips it sets; left out,
    /// it sets the selected ones.
    fn prepare_scale_clips(&self, id: &str, mut args: Value) -> Result<Value, ShellError> {
        if id != scales_ids::SET_CLIP_SCALE {
            return Ok(args);
        }
        let Some(object) = args.as_object_mut() else {
            return Ok(args);
        };
        if object.contains_key("clips") {
            return Ok(args);
        }
        let clips = self.scale_targets();
        if clips.is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no clip is selected".to_owned(),
            });
        }
        object.insert("clips".to_owned(), json!(clips));
        Ok(args)
    }

    /// Spec: device-chains R2, R3, and IMP-24. A device may take the
    /// MIDI-effect position only if its type sends notes on from a note
    /// output; otherwise its role is its type's role. The document can't
    /// check this: only the host knows whether a plug-in has a note output.
    fn prepare_device_role(&self, id: &str, mut args: Value) -> Result<Value, ShellError> {
        let Ok(bl_model::Command::SetDeviceRole { device, role }) =
            model::command_from_args(id, args.clone())
        else {
            return Ok(args);
        };
        // An unknown device is the document's to refuse.
        let Some(current) = self.editor.document().device(device) else {
            return Ok(args);
        };
        // A type the host hasn't met can't be checked; the chain's order
        // still is.
        let Some(kind) = self.devices.device_type(&current.kind) else {
            return Ok(args);
        };
        // Left out, the role is the other position: into the MIDI-effect
        // position when the type sends notes on and isn't there already,
        // and back where the type goes otherwise. A type that sends no
        // notes on can't take the MIDI-effect position at all, so the
        // menu's item says why rather than doing nothing.
        let role = match role {
            Some(role) => role,
            None if current.role == bl_model::DeviceRole::MidiEffect => kind.role,
            None if kind.note_output => bl_model::DeviceRole::MidiEffect,
            None => {
                return Err(ShellError::Failed {
                    command: id.to_owned(),
                    message: format!(
                        "{} has no note output, so it can't go in the MIDI-effect position",
                        current.title()
                    ),
                });
            }
        };
        if kind.role != role && !(role == bl_model::DeviceRole::MidiEffect && kind.note_output) {
            let message = if role == bl_model::DeviceRole::MidiEffect {
                format!(
                    "{} has no note output, so it can't go in the MIDI-effect position",
                    current.title()
                )
            } else {
                format!(
                    "{} goes in the chain as {}",
                    current.title(),
                    kind.role.label()
                )
            };
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message,
            });
        }
        if let Some(object) = args.as_object_mut() {
            object.insert(
                "role".to_owned(),
                serde_json::to_value(role).unwrap_or_default(),
            );
        }
        Ok(args)
    }

    /// Spec: warping R7, R11. A warp marker edit on several clips needs
    /// files as long as each other's, and a marker can't leave its file; the
    /// document doesn't hold the media, so the check is made here, when the
    /// media store knows the files' lengths.
    fn check_warp_files(&self, id: &str, args: &Value) -> Result<(), ShellError> {
        use bl_model::Command;
        let refuse = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let Ok(command) = model::command_from_args(id, args.clone()) else {
            return Ok(());
        };
        let document = self.editor.document();
        // Each clip's file length in seconds, where the store knows it.
        let length = |clip: &bl_model::ClipId| {
            let Some(bl_model::ClipData::Audio(audio)) = document.clip(*clip).map(|c| &c.data)
            else {
                return None;
            };
            self.file_seconds(audio)
        };
        let (clips, places) =
            match &command {
                Command::InsertWarpMarker { clips, .. }
                | Command::ShiftWarpMarker { clips, .. } => (clips, true),
                Command::MoveWarpMarkers { clips, .. }
                | Command::DeleteWarpMarkers { clips, .. } => (clips, false),
                _ => return Ok(()),
            };
        let lengths: Vec<Option<f64>> = clips.iter().map(length).collect();
        let known: Vec<f64> = lengths.iter().flatten().copied().collect();
        if known.len() == clips.len()
            && known
                .windows(2)
                .any(|pair| (pair[0] - pair[1]).abs() > 1e-9)
        {
            return Err(refuse(
                "the clips' files aren't all as long as each other's".to_owned(),
            ));
        }
        if !places {
            return Ok(());
        }
        for (&clip, length) in clips.iter().zip(lengths) {
            let Some(bl_model::ClipData::Audio(audio)) = document.clip(clip).map(|c| &c.data)
            else {
                continue;
            };
            let at = match &command {
                // A shift moves a marker from where it is.
                Command::ShiftWarpMarker { marker, by, .. } => {
                    audio.warp_markers.get(*marker).map(|m| m.time + by)
                }
                Command::InsertWarpMarker {
                    time: Some(time), ..
                } => Some(*time),
                // A marker put on a beat goes at the time the map places it.
                Command::InsertWarpMarker {
                    beat: Some(beat), ..
                } => audio
                    .warp_map()
                    .ok()
                    .map(|map| map.beat_to_time(beat.get() as f64)),
                _ => None,
            };
            if let (Some(at), Some(length)) = (at, length)
                && at > length + 1e-9
            {
                return Err(refuse(format!(
                    "the marker would leave the file, which is {length} seconds long"
                )));
            }
        }
        Ok(())
    }

    /// Runs an automation editing command, on the lanes the selection
    /// covers (the automation editing spec, R5, R7).
    fn run_automation(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let before = self.editor.document().clone();
        let result = automation::run(
            id,
            args,
            &mut self.editor,
            &mut self.arrangement,
            &mut self.envelope_clipboard,
            &self.devices,
        )
        .map_err(|failure| edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        result
    }

    fn run_command(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if recovery_ids::handles(id) {
            // Spec: crash-recovery R26. The recovery commands run against
            // the app's own recovery.
            return self.run_recovery(id, args);
        }
        if let Some(track) = self.frozen_blocks(id, &args) {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: format!(
                    "the track {} is frozen; unfreeze it to change its devices or clip settings",
                    track.0
                ),
            });
        }
        if self.edit_gesture.is_some()
            && (model::handles(id)
                || audio_ids::handles(id)
                || freeze_ids::handles(id)
                || bounce_ids::handles(id)
                || comping_ids::handles(id)
                || edit_ids::handles(id)
                || note_ids::handles(id)
                || device_ids::handles(id)
                || multisampler_ids::handles(id)
                || self.is_device_command(id)
                || track_ids::handles(id)
                || midi_ids::handles(id)
                || slice_ids::handles(id)
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
        if audio_ids::handles(id) {
            return self.run_audio_editing(id, args);
        }
        if freeze_ids::handles(id) {
            return self.run_freeze(id, args);
        }
        if bounce_ids::handles(id) {
            return self.run_bounce(id, args);
        }
        if comping_ids::handles(id) {
            // Spec: comping R4, R5, R17 to R21.
            let before = self.editor.document().clone();
            let result =
                crate::arrangement::comping::run(id, args, &mut self.editor, &self.arrangement)
                    .map_err(|failure| edit_error(id, failure));
            if *self.editor.document() != before {
                self.dirty = true;
                self.document_changed();
            }
            self.touch();
            return result;
        }
        if edit_ids::handles(id) {
            // Spec: automation-editing R5. With the time selection in
            // automation lanes, the edit commands act on the envelopes
            // alone, and the clips stay as they are.
            if let Some(automation_id) = automation_command(id, &self.arrangement) {
                return self.run_automation(automation_id, args);
            }
            let before = self.editor.document().clone();
            let result = editing::run(
                id,
                args,
                &mut self.editor,
                &mut self.arrangement,
                &mut self.clipboard,
                self.prefs.create_fades_on_clip_edges.0,
            )
            .map_err(|failure| edit_error(id, failure));
            if *self.editor.document() != before {
                self.dirty = true;
                self.document_changed();
            }
            self.touch();
            return result;
        }
        if automation_ids::handles(id) {
            return self.run_automation(id, args);
        }
        if locator_ids::handles(id) {
            // Spec: time-editing R1 to R13. The locators are part of the
            // set, and the jumps between them need the engine.
            let before = self.editor.document().clone();
            let launch = self.launch_settings();
            let result = crate::arrangement::locator::run(
                id,
                args,
                &mut self.editor,
                &mut self.arrangement,
                &launch,
                Some(&mut self.audio),
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
        if sample_ids::handles(id) {
            // Spec: sample-editor R21. The Sample Editor's selections and
            // view are the shell's, and change nothing in the set.
            let result = sample_editor::run(
                id,
                args,
                self.editor.document(),
                &mut self.clip_view,
                &self.arrangement,
            )
            .map_err(|failure| edit_error(id, failure));
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
            let args = self.prepare_edit(id, args)?;
            let args_floating = args.get("floating") == Some(&Value::Bool(true));
            // Spec: automation R17. With Automation Arm on and the transport
            // recording, a control move is noted for the envelope too.
            // Spec: session-to-arrangement R5. A Session clip that has taken a
            // track over writes the moves too, whether or not Automation Arm
            // is on.
            let moved = ((self.arrangement.automation_arm() || !self.session_over.is_empty())
                && self.automation_recording)
                .then(|| model::command_from_args(id, args.clone()).ok())
                .flatten()
                .map(|command| {
                    crate::automation_record::moved_all(self.editor.document(), &command)
                        .into_iter()
                        .map(|(track, target, value)| {
                            // The value the control had before the move, which
                            // Write starts the envelope from (automation R20).
                            let before = crate::modulation::control_value(
                                self.editor.document(),
                                &self.devices,
                                track,
                                &target,
                            )
                            .unwrap_or(value);
                            (track, target, before, value)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            // Spec: session-automation R1. With Automation Arm on and a
            // Session recording running, a control move is noted for the
            // clip it goes into.
            let session_moved: Vec<(TrackId, AutomationTarget, f64, f64)> =
                (self.arrangement.automation_arm() && self.session_run.is_some())
                    .then(|| model::command_from_args(id, args.clone()).ok())
                    .flatten()
                    .map(|command| {
                        crate::automation_record::moved_all(self.editor.document(), &command)
                            .into_iter()
                            .filter_map(|(track, target, value)| {
                                let before = crate::modulation::control_value(
                                    self.editor.document(),
                                    &self.devices,
                                    track,
                                    &target,
                                )?;
                                Some((track, target, before, value))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            let changes = self.run_document_edit(id, args)?;
            // Spec: sample-editor R11. An edit in the Sample Editor pauses
            // Follow until playback starts or stops.
            if sample_edit_command(id) {
                self.clip_view.sample_follow(true);
            }
            for (track, target, before, value) in moved {
                self.automation_take.write(
                    track,
                    target,
                    self.arrangement.transport().position,
                    value,
                    before,
                );
            }
            for (track, target, before, value) in session_moved {
                self.note_session_move(track, target, before, value);
            }
            if id == bl_model::registry::RE_ENABLE_AUTOMATION {
                // Spec: session-view R23. A launched scene's tempo and
                // signature hold until Re-Enable Automation.
                let _ = self
                    .audio
                    .send_session(engine_session::SessionCommand::ClearSceneOverride);
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
            self.send_transport(command)
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
            if let engine_session::SessionCommand::Launch { scene, .. }
            | engine_session::SessionCommand::LaunchScene(scene) = command
            {
                self.last_launched = Some(scene);
            }
            self.select_on_launch(command);
            self.re_enable_launched_automation(command);
            if let engine_session::SessionCommand::LaunchScene(scene) = command {
                // Spec: session-recording R10. Start Recording on Scene
                // Launch.
                self.record_on_scene_launch(scene);
            }
            return Ok(Value::Null);
        }
        if session_record_ids::handles(id) {
            return self.run_session_record(id, args);
        }
        if capture_ids::handles(id) {
            return self.run_capture_midi(id, args);
        }
        if session_launch_ids::handles(id) {
            // Spec: launch-quantisation R1, R3, R6, R10. The Control Bar's
            // Quantization Menu and the Options items run here, with the
            // value saved beside the song.
            return self.run_session_launch(id, args);
        }
        if id == SETTINGS_STATE {
            Self::args::<NoArgs>(id, args)?;
            return Ok(to_value(&self.settings_state()));
        }
        if transport_ids::handles(id) {
            Self::args::<NoArgs>(id, args)?;
            let engine = self.poll_engine();
            return Ok(to_value(&crate::transport::state(self, engine)));
        }
        if scales_ids::handles(id) {
            // Spec: scales R1, R13. The scale list, which the Scale Name
            // chooser shows and an agent reads.
            Self::args::<NoArgs>(id, args)?;
            return Ok(to_value(&bl_shell_spec::scales::ScaleList::new()));
        }
        if view_ids::handles(id) {
            // Spec: clap-hosting R5 (WP-5.40). Choosing a plug-in parameter
            // in the automation choosers makes its temporary entry permanent.
            let chosen = (id == view_ids::SHOW_AUTOMATION)
                .then(|| {
                    Self::args::<view_ids::ShowAutomationArgs>(id, args.clone())
                        .ok()
                        .and_then(|a| a.target)
                })
                .flatten()
                .and_then(|target| match target {
                    AutomationTarget::Device { device, param } => Some((device, param)),
                    _ => None,
                });
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
            if result.is_ok()
                && let Some((device, param)) = chosen
            {
                self.configure_on_use(device, param);
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
                )
                .map(|mut state| {
                    // Spec: mixer-view R40. The strips' gain-reduction bars
                    // read with the meters.
                    state.gain_reduction = self.gain_reductions();
                    // Spec: mixer R31. The Performance Impact values the
                    // engine last published, one per track.
                    state.performance_impact_values = self.impact_values();
                    state
                });
            if result.is_ok() && id == mixer_ids::FOCUS {
                // Spec: mixer-view R13.
                self.focused_window = WindowKind::Main;
                self.focused_area = Area::MainView;
                self.effects.push(Effect::FocusArea(Area::MainView));
            }
            if result.is_ok() && id == mixer_ids::RETURN_TRACKS_SCROLL {
                // Spec: mixer-view R21. The switch is an application
                // setting, kept between launches, and the Arrangement's
                // lanes follow it.
                let on = self.mixer.return_tracks_scroll();
                self.prefs.return_tracks_scroll = on;
                self.arrangement.set_returns_scroll(on);
                self.save_preferences();
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
            let search = self.browser_view.search_folders();
            self.analysis_cache();
            // Spec: session-view R24. Capture and Insert Scene copies the
            // clips the engine plays, whichever scene they sit in.
            let capture = crate::session_view::Capture {
                playing: self
                    .session_states
                    .iter()
                    .filter(|slot| slot.state != bl_engine::SlotState::Queued)
                    .map(|slot| (slot.track, slot.scene))
                    .collect(),
                last_launched: self.last_launched,
                tempo: self.current_tempo(),
                signature: self.current_signature(),
            };
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
                        warping: auto_warp::Warping {
                            prefs: self.prefs.warp,
                            analysis: self.analysis.as_ref(),
                        },
                        launch_mode: self.prefs.launch.default_mode,
                        capture,
                        launch_quantization: self.prefs.launch.default_quantization,
                        search: &search,
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
            if let Some(message) = outcome.message {
                self.set_message(message, false);
            }
            if let Some(scene) = outcome.captured {
                // Spec: session-view R24. The playing clips carry on in the
                // copies. Without an engine nothing plays, and the scene
                // is just inserted.
                let _ = self
                    .audio
                    .send_session(engine_session::SessionCommand::MoveToScene(scene));
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
            && let Some((rack, note)) = self.devices.hot_swap_pad_target()
            && let Some(drop) = self.pad_hot_swap_drop(rack, note)
        {
            // Spec: drum-racks R18. With a pad the target, what the browser
            // loads lands on the pad as a drop would.
            Self::args::<NoArgs>(id, args)?;
            return self.pad_drop(to_value(&drop));
        }
        if id == browser_ids::LOAD_SELECTED
            && self.devices.hot_swap_pad_target().is_some()
            && self.browser_view.selected_plugin().is_some()
        {
            // Spec: drum-racks R18. A plug-in can't go onto a pad from here
            // yet; loading it in the rack's place would lose the kit.
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: "a pad being hot-swapped takes samples and presets; drag the plug-in onto the pad".to_owned(),
            });
        }
        if id == browser_ids::LOAD_SELECTED
            && let Some(plugin) = self.browser_view.selected_plugin().cloned()
        {
            // Spec: plugin-scanning R11. A plug-in chosen in the browser
            // loads as `plugin.load` loads it.
            Self::args::<NoArgs>(id, args)?;
            let loaded = self.load_browser_plugin(id, &plugin)?;
            self.touch();
            return Ok(loaded);
        }
        if id == browser_ids::LOAD_SELECTED
            && self.main_view == MainView::Session
            && !self.browser_view.selected_clip_files().is_empty()
        {
            // Spec: clip files R10. With the Session View shown, the selected
            // clip files go in the selected slot when it is empty and on a
            // track of their kind, otherwise on a new track in the selected
            // scene.
            Self::args::<NoArgs>(id, args)?;
            let files = self.browser_view.selected_clip_files();
            let selected = self.session.first_slot();
            let doc = self.editor.document();
            let wanted = match browser::listing::clip_kind(&files[0]) {
                Some(bl_shell_spec::browser::ClipFileKind::Midi) => TrackKind::Midi,
                _ => TrackKind::Audio,
            };
            let scene = selected
                .map(|slot| slot.scene)
                .or_else(|| self.session.state().scenes.first().copied())
                .or_else(|| doc.scenes().next().map(|scene| scene.id))
                .ok_or_else(|| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "the set has no scene".to_owned(),
                })?;
            let track = selected
                .filter(|slot| {
                    doc.slot(slot.track, slot.scene).is_none()
                        && doc.track(slot.track).is_some_and(|t| t.kind() == wanted)
                })
                .map(|slot| slot.track);
            let paths: Vec<String> = files
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            return self.run(
                session_ids::DROP_FILES,
                json!({ "paths": paths, "track": track, "scene": scene }),
            );
        }
        if id == browser_ids::LOAD_SELECTED
            && let Some(path) = self.browser_view.selected_sfz()
        {
            // Spec: multisampler R47. A selected SFZ instrument loads as a
            // multisampler onto the selected track if it is a MIDI track,
            // otherwise onto a new one.
            Self::args::<NoArgs>(id, args)?;
            let doc = self.editor.document();
            let track = self
                .arrangement
                .selected_tracks()
                .first()
                .copied()
                .filter(|track| {
                    doc.track(*track)
                        .is_some_and(|t| t.kind() == bl_model::TrackKind::Midi)
                });
            return self.run_multisampler(
                multisampler_ids::IMPORT_SFZ,
                json!({ "path": path.to_string_lossy(), "track": track }),
            );
        }
        if id == browser_ids::LOAD_SELECTED
            && let Some((device, layer, path)) = self.hot_swapped_layer_file()
        {
            // Spec: multisampler R10. Loading a sample while hot-swapping a
            // layer replaces the layer's.
            Self::args::<NoArgs>(id, args)?;
            self.devices.set_layer_hot_swap(None);
            return self.run_multisampler(
                multisampler_ids::SET_LAYER_SAMPLE,
                json!({ "device": device, "layer": layer, "path": path.to_string_lossy() }),
            );
        }
        if id == browser_ids::LOAD_SELECTED
            && let Some((device, path)) = self.hot_swap_sample()
        {
            // Spec: eight-slot-drum-sampler R17. Loading a sample while
            // hot-swapping a device that plays samples loads it into the
            // device's selected slot, as dropping it there does.
            Self::args::<NoArgs>(id, args)?;
            return self.load_sample(
                device_ids::LOAD_SAMPLE,
                json!({ "device": device, "path": path.to_string_lossy() }),
            );
        }
        if id == browser_ids::LOAD_SELECTED
            && let Some(load) = self.devices_load()
        {
            // Spec: modulation R12, racks R43 and R45. A modulator, a rack
            // or a preset selected in the browser loads as `device.add`
            // adds a device, or replaces the one being hot-swapped.
            Self::args::<NoArgs>(id, args)?;
            return self.load_into_devices(load);
        }
        if multisampler_ids::handles(id) {
            return self.run_multisampler(id, args);
        }
        if autosampler_ids::handles(id) {
            return self.run_autosampler(id, args);
        }
        if device_ids::handles(id) {
            return self.run_devices(id, args);
        }
        if track_ids::handles(id) {
            return self.run_tracks(id, args);
        }
        if browser_ids::handles(id) {
            // Spec: file-management R11, R14. A file armed for hot-swapping
            // is replaced by the file the browser has chosen.
            if id == browser_ids::LOAD_SELECTED && self.file_manager.hot_swap.is_some() {
                return self.hot_swap_selected(id);
            }
            return self.run_browser(id, args);
        }
        if perception_ids::handles(id) {
            return self.run_perception(id, args);
        }
        if bl_shell_spec::warping::handles(id) {
            return self.run_warping(id, args);
        }
        if bl_shell_spec::audio_panel::handles(id) {
            return self.run_audio_panel(id, args);
        }
        if bl_shell_spec::modulation::handles(id) {
            let bl_shell_spec::modulation::StateArgs { track } = Self::args(id, args)?;
            return Ok(to_value(&crate::modulation::state(
                self.editor.document(),
                &self.devices,
                track,
                self.arrangement.transport().position,
                |track, target| self.played(track, target),
            )));
        }
        if stems_ids::handles(id) {
            return self.run_stems(id, args);
        }
        if slice_ids::handles(id) {
            return self.run_slice(id, args);
        }
        if export_ids::handles(id) {
            return match id {
                export_ids::AUDIO => self.run_export_audio(args),
                // Spec: external-devices R23. The real-time render's
                // dialog: its buttons set the session's flags.
                export_ids::REALTIME_STATUS
                | export_ids::REALTIME_RESTART
                | export_ids::REALTIME_SKIP
                | export_ids::REALTIME_CANCEL
                | export_ids::REALTIME_AUTO_RESTART => self.run_realtime(id, args),
                _ => self.run_export_dialog(id, args),
            };
        }
        if midi_ids::handles(id) {
            return self.run_midi_files(id, args);
        }
        if dawproject_ids::handles(id) {
            return self.run_dawproject(id, args);
        }
        if midi_io_ids::handles(id) {
            return self.run_midi_io(id, args);
        }
        if file_manager_ids::handles(id) {
            // Spec: file-management R10 to R17. The File Manager's own
            // commands.
            let result = self.run_file_manager(id, args);
            self.touch();
            return result;
        }
        if search_ids::handles(id) {
            // Spec: command-palette R15, R16. The search field's commands.
            return self.run_search(id, args);
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

    /// The state of the real-time render an export with external hardware
    /// runs: the app's dialog shows it, and its buttons set it (the
    /// external devices spec, R23).
    #[must_use]
    pub fn realtime_session(&self) -> crate::realtime::Session {
        self.realtime.clone()
    }

    /// What the real-time render is doing now (the external devices spec,
    /// R23).
    #[must_use]
    pub fn realtime_status(&self) -> export_ids::RealtimeStatus {
        let status = self.realtime.status();
        let phase = match status.phase {
            crate::realtime::Phase::Idle => export_ids::RealtimePhase::Idle,
            crate::realtime::Phase::Waiting => export_ids::RealtimePhase::Waiting {
                seconds: status.seconds,
            },
            crate::realtime::Phase::Rendering => export_ids::RealtimePhase::Rendering,
        };
        export_ids::RealtimeStatus {
            phase,
            progress: status.progress,
            attempts: status.attempts,
            auto_restart: status.auto_restart,
        }
    }

    /// Runs the real-time render's dialog's commands (the external devices
    /// spec, R23).
    fn run_realtime(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            export_ids::REALTIME_STATUS => Ok(to_value(&self.realtime_status())),
            export_ids::REALTIME_RESTART => {
                self.realtime.restart();
                Ok(to_value(&self.realtime_status()))
            }
            export_ids::REALTIME_SKIP => {
                self.realtime.skip();
                Ok(to_value(&self.realtime_status()))
            }
            export_ids::REALTIME_CANCEL => {
                self.realtime.cancel();
                Ok(to_value(&self.realtime_status()))
            }
            export_ids::REALTIME_AUTO_RESTART => {
                let args = Self::args::<export_ids::AutoRestartArgs>(id, args)?;
                self.realtime.set_auto_restart(args.auto_restart);
                Ok(to_value(&self.realtime_status()))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Runs a MIDI port or computer MIDI keyboard command (WP-2.2).
    fn run_midi_io(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let result = match id {
            midi_io_ids::PORTS => {
                Self::args::<NoArgs>(id, args)?;
                self.refresh_midi();
                to_value(&self.midi.report())
            }
            midi_io_ids::SET_PORT => {
                let args = Self::args::<midi_io_ids::SetPortArgs>(id, args)?;
                self.refresh_midi();
                let port = self
                    .midi
                    .set_port(
                        args.direction,
                        &args.name,
                        args.track,
                        args.sync,
                        args.remote,
                    )
                    .map_err(failed)?;
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                to_value(&port)
            }
            midi_io_ids::TOGGLE_COMPUTER_KEYBOARD => {
                let args = Self::args::<midi_io_ids::ToggleKeyboardArgs>(id, args)?;
                self.midi.set_keyboard_on(args.on);
                self.prefs.midi = self.midi.prefs();
                self.save_preferences();
                to_value(&self.midi.keyboard_info())
            }
            midi_io_ids::COMPUTER_KEYBOARD_OCTAVE => {
                let args = Self::args::<midi_io_ids::StepArgs>(id, args)?;
                self.midi.shift_octave(args.steps);
                to_value(&self.midi.keyboard_info())
            }
            midi_io_ids::COMPUTER_KEYBOARD_VELOCITY => {
                let args = Self::args::<midi_io_ids::StepArgs>(id, args)?;
                self.midi.step_velocity(args.steps);
                to_value(&self.midi.keyboard_info())
            }
            midi_io_ids::COMPUTER_KEYBOARD_KEY => {
                let args = Self::args::<midi_io_ids::KeyArgs>(id, args)?;
                let result = self.midi.key(&args.key, args.pressed).map_err(failed)?;
                // Spec: multisampler R7. Auto Select follows the note.
                if let Some(note) = result.note {
                    let velocity = self.midi.keyboard_info().velocity;
                    self.follow_note(note, velocity, args.pressed);
                }
                to_value(&result)
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        };
        self.touch();
        Ok(result)
    }

    /// Runs a DAWproject command (WP-6.36): opening a `.dawproject` file as
    /// a new set, or reading back the report of the last import.
    ///
    /// Spec: dawproject R10 to R12. The import is refused while the open
    /// set has unsaved changes; the reading, the extracting and the building
    /// run on a worker thread; the set changes only when the new set is
    /// whole, and a failure leaves the open set as it was.
    fn run_dawproject(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        match id {
            dawproject_ids::IMPORT_DAWPROJECT => {
                let args = Self::args::<dawproject_ids::ImportDawProjectArgs>(id, args)?;
                if self.dirty {
                    return Err(unavailable(
                        "the set has unsaved changes; save them with `set.save` or \
                         `set.save_as`, or throw them away with `set.discard_changes`, first",
                    ));
                }
                let path = PathBuf::from(&args.path);
                let folder = crate::dawproject::import_folder(&self.temporary_folder(), &path);
                let warp_mode = crate::auto_warp::warp_mode(self.prefs.warp.default_warp_mode);
                let mut job = crate::dawproject::Job::start(&path, &folder, warp_mode)
                    .map_err(|error| failed(error.to_string()))?;
                // An agent's call returns when the new set is open (R10).
                let imported = loop {
                    if let Some(result) = job.poll() {
                        break result;
                    }
                    self.set_message(
                        format!("Importing {} ({}%)", path.display(), job.percent()),
                        false,
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1));
                };
                let imported = imported.map_err(|error| {
                    self.set_message(error.to_string(), true);
                    failed(error.to_string())
                })?;
                // The set changes only when the new set is whole.
                self.store.close();
                for (hash, at, size) in &imported.media {
                    self.store.add_media(*hash, at, *size);
                    let hash = bl_media::ContentHash::from_bytes(hash.0);
                    if self.media.entry(hash).is_none() {
                        let _ = self.media.import(at);
                    }
                }
                self.saved = snapshot(&imported.editor);
                self.set_replaced();
                self.editor = imported.editor;
                self.set_path = None;
                // Spec: dawproject R11. Until the set is first saved its
                // name is the file's name without `.dawproject`.
                self.pending_name = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned());
                self.dirty = false;
                self.message = None;
                self.arrangement.reset();
                self.clip_view.forget_selection();
                self.mixer.reset();
                self.session.reset();
                self.new_set_transport();
                self.document_changed();
                self.set_file_changed();
                self.import_report = Some(imported.report.clone());
                Ok(to_value(&dawproject_ids::ImportDawProjectResult {
                    name: self.set_name(),
                    report: imported.report,
                }))
            }
            dawproject_ids::IMPORT_REPORT => {
                Self::args::<NoArgs>(id, args)?;
                let report = self.import_report.clone().unwrap_or_default();
                Ok(to_value(&report))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
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
            plugins: self.plugin_hosts(),
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

    fn view_window(&self) -> ViewResult {
        ViewResult {
            main_view: self.main_view.id().to_owned(),
            browser: self.browser,
            info_view: self.info_view,
            full_screen: self.full_screen,
            zoom: self.zoom(),
        }
    }

    fn view_result(&self) -> Value {
        to_value(&self.view_window())
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

    /// Select on Launch and Select Next Scene on Launch (Spec:
    /// launch-quantisation R16): a launched slot that holds a clip selects
    /// itself, and a launched scene selects the scene after it, while the
    /// preferences are on.
    fn select_on_launch(&mut self, command: engine_session::SessionCommand) {
        let document = self.editor.document();
        let target = match command {
            engine_session::SessionCommand::Launch { track, scene }
                if self.prefs.launch.select_on_launch && document.slot(track, scene).is_some() =>
            {
                json!({ "track": track, "scene": scene })
            }
            engine_session::SessionCommand::LaunchScene(scene)
                if self.prefs.launch.select_next_scene_on_launch =>
            {
                let next = document
                    .scene_position(scene)
                    .and_then(|at| document.scenes().nth(at + 1));
                let Some(next) = next else { return };
                json!({ "scene": next.id })
            }
            _ => return,
        };
        // A selection that can't be made leaves the one there is.
        let _ = self.run(session_ids::SELECT, target);
    }

    /// The launch commands Enter runs: one for each selected slot that
    /// holds a clip and one for each selected scene.
    fn launch_targets(&self) -> Vec<(&'static str, Value)> {
        let state = self.session.state();
        let document = self.editor.document();
        let slots = state
            .slots
            .iter()
            .filter(|slot| document.slot(slot.track, slot.scene).is_some())
            .map(|slot| {
                (
                    "session.launch",
                    json!({ "track": slot.track, "scene": slot.scene }),
                )
            });
        let scenes = state
            .scenes
            .iter()
            .map(|scene| ("session.launch_scene", json!({ "scene": scene })));
        slots.chain(scenes).collect()
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
            session_launch_ids::SET_FOLLOW_ACTIONS_ENABLED => {
                let session_launch_ids::SetFollowActionsEnabledArgs { enabled } =
                    Self::args(id, args)?;
                LaunchSettings {
                    follow_actions: enabled.unwrap_or(!current.follow_actions),
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
            session_launch_ids::LAUNCH_SELECTED => {
                Self::args::<NoArgs>(id, args)?;
                // Spec: session-view R32. Enter launches what the grid has
                // selected, one launch command each.
                let targets = self.launch_targets();
                if targets.is_empty() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a clip or a scene first".to_owned(),
                    });
                }
                for (command, args) in targets {
                    self.run(command, args)?;
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
                self.devices.set_rack_views(bl_model::RackViewMap::new());
                self.devices
                    .set_multisampler_views(bl_model::MultisamplerViews::new());
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
                // Spec: crash-recovery R21. The new set is journalled from
                // the start, and the set that was open keeps its unsaved
                // edits for recovery (R7).
                self.begin_journal();
                Ok(self.set_result())
            }
            ids::SET_OPEN => {
                let PathArgs { path } = Self::args(id, args)?;
                if self.unsaved_changes() {
                    return Err(unavailable(
                        "the set has unsaved changes; save them with `set.save` or \
                         `set.save_as`, or throw them away with `set.discard_changes`, first",
                    ));
                }
                let (editor, opened) = self.store.open(Path::new(&path)).map_err(failed)?;
                // Spec: device-chains R29. Floating windows reopen where
                // they were saved.
                self.devices.set_windows(self.store.windows());
                // Spec: racks R19. So do the views each rack shows.
                self.devices.set_rack_views(self.store.rack_views());
                self.devices
                    .set_multisampler_views(self.store.multisampler_views());
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
                // Spec: file-management R6, R14. Files that moved are found
                // again by content, so the set opens able to play them: a
                // file with one candidate is relinked to it, and a file with
                // several waits for the person to choose.
                let (relinked, candidates) = self.relink_missing(&[], &Self::default_search());
                self.file_manager.search.candidates = candidates;
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
                // Spec: crash-recovery R21. The opened set is journalled
                // from the start.
                self.begin_journal();
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
                self.store.set_rack_views(self.devices.rack_views().clone());
                self.store
                    .set_multisampler_views(self.devices.multisampler_views().clone());
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
                self.store.set_rack_views(self.devices.rack_views().clone());
                self.store
                    .set_multisampler_views(self.devices.multisampler_views().clone());
                self.store.set_folds(self.clip_view.folds().clone());
                self.store.set_temporary_folder(self.temporary_folder());
                let (path, warnings) = self
                    .store
                    .save_as(Path::new(&path), &self.editor)
                    .map_err(failed)?;
                // Processed files the first save moved out of the Temporary
                // Folder: the media store follows them.
                for (hash, now, _) in self.store.media_files() {
                    let hash = bl_media::ContentHash::from_bytes(hash.0);
                    if self.media.entry(hash).is_some_and(|e| !e.path.exists()) && now.is_file() {
                        let _ = self.media.import(&now);
                    }
                }
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
                    // Spec: crash-recovery R6. The changes are gone, so
                    // nothing is offered at the next launch.
                    self.close_journal();
                    // The set is still open, so its later edits are
                    // journalled from now on.
                    self.begin_journal();
                }
                Ok(self.set_result())
            }
            ids::SET_LATER_RECOVERY => {
                // Spec: crash-recovery R22. The prompt closes; every set it
                // listed is still offered at the next launch.
                Self::args::<NoArgs>(id, args)?;
                self.recovery_deferred = true;
                Ok(to_value(&ids::RecoveryPrompt {
                    open: self.recovery_prompt_open(),
                }))
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
                // Spec: file-management R4, R15, R16. The groups to collect
                // from, and the project to collect for.
                let crate::commands::CollectArgs { groups, project } = Self::args(id, args)?;
                let library = self.browser_view.user_library();
                let groups = if groups.is_empty() {
                    vec![
                        file_manager_ids::ExternalGroup::OtherProject,
                        file_manager_ids::ExternalGroup::UserLibrary,
                        file_manager_ids::ExternalGroup::Elsewhere,
                    ]
                } else {
                    groups
                };
                if let Some(project) = project {
                    let folder = PathBuf::from(project);
                    let files = self
                        .collect_project(&folder, &groups, library)
                        .map_err(failed)?;
                    return Ok(to_value(&FileReport {
                        path: Some(folder.to_string_lossy().into_owned()),
                        files,
                        candidates: Vec::new(),
                    }));
                }
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
                self.store.set_rack_views(self.devices.rack_views().clone());
                self.store
                    .set_multisampler_views(self.devices.multisampler_views().clone());
                self.store.set_folds(self.clip_view.folds().clone());
                let files = self
                    .store
                    .collect_groups(&groups, library)
                    .map_err(failed)?;
                let warnings = self.store.save(&path, &self.editor).map_err(failed)?;
                self.saved_as(path.clone(), &warnings);
                Ok(to_value(&FileReport {
                    path: Some(path.to_string_lossy().into_owned()),
                    files,
                    candidates: Vec::new(),
                }))
            }
            ids::SET_RELINK_MISSING => {
                // Spec: file-management R14. The places the search is
                // switched on for, and the folders it is given.
                let crate::commands::RelinkArgs {
                    folders,
                    project,
                    library,
                    folder,
                } = Self::args(id, args)?;
                let extra: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
                let search = bl_shell_spec::file_manager::Search {
                    folder: folder.clone(),
                    project: project.unwrap_or(true),
                    library: library.unwrap_or(true),
                    candidates: Vec::new(),
                };
                let (files, candidates) = self.relink_missing(&extra, &search);
                self.file_manager.search = search.clone();
                self.file_manager.search.candidates = candidates.clone();
                Ok(to_value(&FileReport {
                    path: self
                        .set_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                    files,
                    candidates,
                }))
            }
            ids::APP_QUIT => {
                Self::args::<NoArgs>(id, args)?;
                self.audio.set_test_tone(false);
                self.save_preferences();
                // Spec: crash-recovery R21. The set is closed, so nothing
                // is offered at the next launch.
                self.close_journal();
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
                if self.focused_window == WindowKind::Assistant {
                    self.focused_window = WindowKind::Main;
                    self.assistant_open = false;
                    self.touch();
                    return Ok(Value::Null);
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
            ids::CLOSE_PLUG_IN_WINDOWS => {
                // Spec: device-chains R40. It closes editors only, so it
                // changes nothing in the set and is never an undo step.
                Self::args::<NoArgs>(id, args)?;
                let closed = self.close_plug_in_windows();
                self.touch();
                Ok(to_value(&ids::ClosedPlugInWindows { closed }))
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
            ids::VIEW_TOGGLE_PLUGIN_LIST => {
                // Spec: plugin-list R1, R3. View state only.
                Self::args::<NoArgs>(id, args)?;
                self.plugin_list.toggle();
                // Spec: file-management R10. The File Manager and the list of
                // the set's plug-ins take the same place: showing either
                // hides the other.
                if self.plugin_list.visible() {
                    self.file_manager.open = false;
                }
                self.touch();
                Ok(to_value(&self.plugin_list.state()))
            }
            ids::VIEW_SET_PLUGIN_LIST_ONLY_MISSING => {
                // Spec: plugin-list R3, R10. View state only.
                let ids::PluginListOnlyMissingArgs { only_missing } = Self::args(id, args)?;
                self.plugin_list
                    .set_only_missing(only_missing.unwrap_or(!self.plugin_list.only_missing()));
                self.touch();
                Ok(to_value(&self.plugin_list.state()))
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
            ids::VIEW_CLIP_TAB => {
                // Spec: piano-roll R1; S-CLIPMODE-001 to 004. View state only.
                let ClipTabArgs { tab } = Self::args(id, args)?;
                self.clip_view
                    .set_tab(tab.unwrap_or_else(|| self.clip_view.tab().next()));
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_CLIP_ENVELOPE => {
                // Spec: clip-envelopes R8. View state only.
                let ClipEnvelopeArgs { target } = Self::args(id, args)?;
                if let Some(target) = target {
                    let doc = self.editor.document();
                    let clip = self
                        .clip_view
                        .shown_any_clip(doc, &self.arrangement)
                        .ok_or_else(|| unavailable("no clip is shown in the Clip View"))?;
                    let track = doc
                        .clip_track(clip)
                        .ok_or_else(|| failed("no track holds the clip".to_owned()))?;
                    let shown = doc.clip(clip).ok_or_else(|| failed(format!("no {clip}")))?;
                    doc.check_clip_target(track, shown, target)
                        .map_err(|error| failed(error.to_string()))?;
                    // Spec: clap-hosting R5 (WP-5.40). Choosing a plug-in
                    // parameter in the clip-envelope choosers makes its
                    // temporary entry permanent.
                    if let AutomationTarget::Device { device, param } = target {
                        self.configure_on_use(device, param);
                    }
                }
                self.clip_view.set_envelope(target);
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
            ids::VIEW_LANES => {
                // Spec: note-editing R1, R2, R13; the lanes under the note
                // editor and their Randomize amounts.
                let LanesArgs {
                    lane,
                    show,
                    swap,
                    amount,
                    all,
                } = Self::args(id, args)?;
                if lane.is_none() && (show.is_some() || swap.is_some() || amount.is_some()) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "`show`, `swap` and `amount` need a `lane`".to_owned(),
                    });
                }
                if let Some(lane) = lane {
                    if let Some(amount) = amount {
                        let limit = RandomizeAmounts::limit(lane);
                        if amount > limit {
                            return Err(ShellError::InvalidArgs {
                                command: id.to_owned(),
                                message: format!("`amount` must be from 0 to {limit}"),
                            });
                        }
                        self.clip_view.set_randomize_amount(lane, amount);
                    }
                    if swap == Some(true) {
                        self.clip_view.swap_lane(lane);
                    } else if let Some(on) = show {
                        self.clip_view.show_lane(lane, on);
                    } else if amount.is_none() && swap.is_none() {
                        let on = !self.clip_view.lanes().is_shown(lane);
                        self.clip_view.show_lane(lane, on);
                    }
                }
                if let Some(on) = all {
                    self.clip_view.show_all_lanes(on);
                }
                Ok(to_value(
                    &self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                ))
            }
            ids::VIEW_STATE => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&ViewState {
                    window: self.view_window(),
                    clip_view: self
                        .clip_view
                        .state(self.editor.document(), &self.arrangement),
                    plugin_list: self.plugin_list.state(),
                    assistant: self.assistant_panel_state(),
                    undo_history: self.undo_history_open,
                    plug_in_windows: self.plug_in_windows,
                }))
            }
            ids::VIEW_TOGGLE_UNDO_HISTORY => {
                // Spec: undo-history R1, R3. The panel is view state: it
                // isn't saved with the set and isn't an undo step.
                Self::args::<NoArgs>(id, args)?;
                self.undo_history_open = !self.undo_history_open;
                self.touch();
                Ok(to_value(&self.undo_history_state()))
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
            ids::KEYMAP_STATE => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.keymap_state()))
            }
            ids::KEYMAP_BIND => {
                let bind: crate::keymap::BindArgs = Self::args(id, args)?;
                self.bind_keys(id, &bind)?;
                Ok(to_value(&self.keymap_result()))
            }
            ids::KEYMAP_UNBIND => {
                let crate::keymap::UnbindArgs {
                    shortcut,
                    command,
                    keys,
                    platform,
                } = Self::args(id, args)?;
                self.unbind_keys(id, shortcut.as_deref(), command.as_deref(), &keys, platform)?;
                Ok(to_value(&self.keymap_result()))
            }
            ids::KEYMAP_RESET => {
                let crate::keymap::ResetArgs { command } = Self::args(id, args)?;
                self.reset_keys(command.as_deref());
                Ok(to_value(&self.keymap_result()))
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
                match theme.as_str() {
                    // What earlier versions took (themes R12).
                    "dark" => self.prefs.appearance = Appearance::Dark,
                    "light" => self.prefs.appearance = Appearance::Light,
                    name if self.themes.iter().any(|t| t == name) => {
                        self.prefs.look.theme = theme;
                    }
                    _ => {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: format!(
                                "there is no theme `{theme}`; the themes are {}",
                                self.themes.join(", ")
                            ),
                        });
                    }
                }
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_APPEARANCE => {
                let AppearanceArgs { appearance } = Self::args(id, args)?;
                self.prefs.appearance = appearance;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_TONE => {
                let ToneArgs { tone } = Self::args(id, args)?;
                self.prefs.look.tone = tone;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_HIGH_CONTRAST => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.look.high_contrast = on;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_GRID_LINE_INTENSITY => {
                let PercentArgs { percent } = Self::args(id, args)?;
                self.prefs.look.grid_line_intensity = percent.clamp(0, 200);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_BRIGHTNESS => {
                let BrightnessArgs { value } = Self::args(id, args)?;
                self.prefs.look.brightness = value.clamp(-50, 50);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_COLOR_INTENSITY => {
                let PercentArgs { percent } = Self::args(id, args)?;
                self.prefs.look.color_intensity = percent.clamp(0, 200);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_COLOR_HUE => {
                let HueArgs { degrees } = Self::args(id, args)?;
                self.prefs.look.color_hue = degrees.clamp(-180, 180);
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.create_fades_on_clip_edges = OnByDefault(on);
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_TEMPORARY_FOLDER => {
                let TemporaryFolderArgs { path } = Self::args(id, args)?;
                let path = path.map(PathBuf::from);
                if path.as_deref().is_some_and(|path| !path.is_absolute()) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the Temporary Folder's path must be absolute".to_owned(),
                    });
                }
                self.prefs.temporary_folder = path;
                self.save_preferences();
                Ok(to_value(&TemporaryFolderResult {
                    path: self.temporary_folder().to_string_lossy().into_owned(),
                }))
            }
            ids::SETTINGS_SET_COLLECT_FILES_ON_EXPORT => {
                let CollectFilesArgs { mode } = Self::args(id, args)?;
                self.prefs.collect_files_on_export = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_USER_LIBRARY => {
                let UserLibraryArgs { path } = Self::args(id, args)?;
                let chosen = path.map(PathBuf::from);
                if chosen.as_deref().is_some_and(|path| !path.is_absolute()) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the User Library's path must be absolute".to_owned(),
                    });
                }
                let library = chosen
                    .clone()
                    .or_else(browser::places::default_user_library);
                if let Some(library) = &library {
                    browser::places::make_user_library(library).map_err(|e| {
                        ShellError::Failed {
                            command: id.to_owned(),
                            message: format!("couldn't make {}: {e}", library.display()),
                        }
                    })?;
                }
                self.prefs.browser.user_library = chosen;
                self.browser_view.set_user_library(library.clone());
                self.save_preferences();
                Ok(to_value(&UserLibraryResult {
                    path: library
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                }))
            }
            ids::SETTINGS_SET_SAMPLE_EDITOR => {
                let SampleEditorArgs { path } = Self::args(id, args)?;
                let path = path
                    .map(|path| path.trim().to_owned())
                    .filter(|path| !path.is_empty());
                self.prefs.sample_editor = path;
                self.save_preferences();
                Ok(to_value(&SampleEditorResult {
                    path: self.sample_editor().unwrap_or_default(),
                }))
            }
            ids::SETTINGS_SET_LOOP_WARP_SHORT_SAMPLES => {
                let LoopWarpShortSamplesArgs { mode } = Self::args(id, args)?;
                self.prefs.warp.loop_warp_short_samples = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_AUTO_WARP_LONG_SAMPLES => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.warp.auto_warp_long_samples = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_DEFAULT_WARP_MODE => {
                let DefaultWarpModeArgs { mode } = Self::args(id, args)?;
                self.prefs.warp.default_warp_mode = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_WARP_RECORDED_AUDIO => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.warp_recorded_audio = OnByDefault(on);
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_DEFAULT_LAUNCH_MODE => {
                let DefaultLaunchModeArgs { mode } = Self::args(id, args)?;
                self.prefs.launch.default_mode = mode;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_DEFAULT_LAUNCH_QUANTIZATION => {
                let DefaultLaunchQuantizationArgs { quantization } = Self::args(id, args)?;
                self.prefs.launch.default_quantization = quantization;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_SELECT_ON_LAUNCH => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.launch.select_on_launch = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_SELECT_NEXT_SCENE_ON_LAUNCH => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.launch.select_next_scene_on_launch = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH => {
                let SwitchArgs { on } = Self::args(id, args)?;
                self.prefs.launch.start_recording_on_scene_launch = on;
                self.save_preferences();
                Ok(to_value(&SwitchResult { on }))
            }
            ids::SETTINGS_SET_RECORD_SESSION_AUTOMATION_IN => {
                let RecordSessionAutomationInArgs { tracks } = Self::args(id, args)?;
                self.prefs.record_session_automation_in = tracks;
                self.save_preferences();
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_AUTOMATION_RECORD_MODE => {
                let AutomationRecordModeArgs { mode } = Self::args(id, args)?;
                self.prefs.automation_record_mode = mode;
                self.save_preferences();
                Ok(Value::Null)
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
            assistant_ids::VIEW_TOGGLE_ASSISTANT => {
                Self::args::<NoArgs>(id, args)?;
                self.assistant_open = !self.assistant_open;
                if self.assistant_open && !self.prefs.assistant_panel.floating {
                    // The keyboard goes to the pane's prompt field, as
                    // the browser's focus command takes it into the browser.
                    self.focused_area = Area::Assistant;
                    self.effects.push(Effect::FocusArea(Area::Assistant));
                }
                self.leave_hidden_assistant();
                self.touch();
                Ok(to_value(&self.assistant_panel_state()))
            }
            assistant_ids::ASSISTANT_SET_PANEL_WIDTH => {
                let SetPanelWidthArgs { width } = Self::args(id, args)?;
                self.prefs.assistant_panel.width = assistant_ids::clamp_panel_width(width);
                self.save_preferences();
                self.touch();
                Ok(to_value(&self.assistant_panel_state()))
            }
            assistant_ids::ASSISTANT_TOGGLE_FLOATING => {
                Self::args::<NoArgs>(id, args)?;
                self.prefs.assistant_panel.floating = !self.prefs.assistant_panel.floating;
                self.save_preferences();
                self.leave_hidden_assistant();
                self.touch();
                Ok(to_value(&self.assistant_panel_state()))
            }
            assistant_ids::ASSISTANT_SEND => {
                let SendArgs { text } = Self::args(id, args)?;
                self.assistant_send(&text).map_err(failed)?;
                self.touch();
                Ok(to_value(&self.assistant_panel_state()))
            }
            assistant_ids::ASSISTANT_STOP => {
                Self::args::<NoArgs>(id, args)?;
                self.assistant_stop();
                Ok(to_value(&self.assistant_panel_state()))
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
        // Spec: crash-recovery R6. The save holds every edit, so the
        // journal retires.
        self.journal_saved();
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
