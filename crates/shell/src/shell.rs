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
    ChangeSet, ClipData, ContentHash, Editor, Input, TrackId, TrackKind, registry as model,
};
use bl_project::ProjectFolder;
use bl_project::autosave::Autosave;
use bl_project::history;
use bl_project::registry as recovery_ids;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use bl_assistant::keychain::{Keychain, MemoryKeychain, OsKeychain};

use crate::agent::{self as agent, AgentServerStatus};
use crate::arrangement::ArrangementView;
use crate::arrangement::automation::EnvelopeClipboard;
use crate::arrangement::editing::{Clipboard, EditFailure};
use crate::assistant::ActivityEntry;
use crate::audio::{AudioHost, EngineTransport};
use crate::browser::{self, BrowserView, Waker};
use crate::clip_view::{self, ClipView};
use crate::commands::{self as ids, SetResult, SwitchArgs};
use crate::detail::DetailArea;
use crate::devices::{self, DeviceError, DeviceView};
use crate::files::{ProjectFiles, SetStore, new_set_from};
use crate::midi::MidiHost;
use crate::midi_tools;
use crate::mixer_view::MixerView;
use crate::sample_editor;
use crate::session_view::SessionView;
use crate::settings::{BUNDLED_THEME, Preferences};
use bl_devices::KitHost;
use bl_plugin_host::au::AuHost;
use bl_plugin_host::clap::ClapHost;
use bl_plugin_host::lv2::Lv2Host;
use bl_plugin_host::scan::Scanner;
use bl_plugin_host::vst3::Vst3Host;
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::audio_editing as audio_ids;
use bl_shell_spec::automation as automation_ids;
use bl_shell_spec::autosampler as autosampler_ids;
use bl_shell_spec::bounce as bounce_ids;
use bl_shell_spec::browser as browser_ids;
use bl_shell_spec::capture_midi as capture_ids;
use bl_shell_spec::clip_envelope_editing as clip_envelope_ids;
use bl_shell_spec::comping as comping_ids;
use bl_shell_spec::dawproject as dawproject_ids;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::editing as edit_ids;
use bl_shell_spec::envelope_keys as envelope_key_ids;
use bl_shell_spec::export as export_ids;
use bl_shell_spec::file_manager as file_manager_ids;
use bl_shell_spec::freeze as freeze_ids;
use bl_shell_spec::locator as locator_ids;
use bl_shell_spec::midi_files as midi_ids;
use bl_shell_spec::midi_io as midi_io_ids;
use bl_shell_spec::midi_tools as tool_ids;
use bl_shell_spec::mixer_view as mixer_ids;
use bl_shell_spec::multisampler as multisampler_ids;
use bl_shell_spec::notes::{self as note_ids};
use bl_shell_spec::perception as perception_ids;
use bl_shell_spec::plugins as plugin_ids;
use bl_shell_spec::sample_editor as sample_ids;
use bl_shell_spec::scales as scales_ids;
use bl_shell_spec::search as search_ids;
use bl_shell_spec::session_launch as session_launch_ids;
use bl_shell_spec::session_record as session_record_ids;
use bl_shell_spec::session_view as session_ids;
use bl_shell_spec::set_exchange as set_exchange_ids;
use bl_shell_spec::settings::STATE as SETTINGS_STATE;
use bl_shell_spec::slice as slice_ids;
use bl_shell_spec::stems as stems_ids;
use bl_shell_spec::tracks as track_ids;
use bl_shell_spec::transport as transport_ids;
use bl_shell_spec::transport::RecordingReport;
use bl_shell_spec::tunings as tuning_ids;

mod ara;
pub(crate) mod audio_editing;
mod audio_panel;
pub mod autosample;
mod bounce;
mod capture;
mod envelope_keys;
mod export_dialog;
pub use export_dialog::ExportPrefs;
pub use info_text::InfoTarget;
mod file_manager;
mod freeze;
mod grooves;
mod keymap;
mod multisampler;
mod plugin_list;
mod processed_files;
mod sample_edits;
mod sample_keys;
mod save_history;
pub use sample_keys::TransientList;
mod agent_server;
mod app_settings;
mod assistant_ui;
mod audio_device;
mod browser_commands;
mod clip_scale;
mod device_commands;
mod document_edits;
mod focus;
mod gestures;
mod info_text;
mod invoke;
mod menus;
mod midi_input;
mod mixer;
mod packs;
mod perception_commands;
mod playback;
mod plugin_commands;
mod plugin_scan;
mod plugin_windows;
mod recording_takes;
mod recovery;
mod search;
mod session_launch;
mod session_recording;
mod set_exchange;
mod set_files;
mod slice;
mod stems;
mod step_input;
mod track_commands;
mod tunings;
mod view_commands;
mod warping;
mod windows;

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
    /// The second window (WP-6.29): another main view, with a Control Bar,
    /// a detail area and a Status Bar of its own.
    Second,
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
    /// Ask whether to retune the set's notes to the tuning about to be
    /// loaded or removed (the tunings spec, R15), then run `then` with
    /// `retune` true if the person says Retune. Cancel changes nothing.
    ConfirmRetune {
        /// The load or removal.
        then: Invocation,
    },
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
    /// Ask where to write a pack of the project folder `project` (the open
    /// set's when `None`), in a save dialog that suggests `file_name` in
    /// `folder`, then run `set.create_pack` with the path (set exchange R16).
    PickPackPath {
        /// The project folder to pack, when it isn't the open set's.
        project: Option<String>,
        /// The suggested name: the project folder's, with `.blpack`.
        file_name: String,
        /// The folder the dialog opens in: the project's parent.
        folder: PathBuf,
    },
    /// Ask for a pack to install, then ask where to unpack it (set exchange
    /// R20).
    PickPack,
    /// Ask for the folder to unpack `pack` into, the pack's own folder
    /// first, then run `set.install_pack` with it (set exchange R20).
    PickInstallFolder {
        /// The pack to unpack.
        pack: String,
    },
    /// Quit.
    Quit,
    /// Hide the app (macOS).
    Hide,
    /// Show the About window.
    ShowAbout,
    /// Show these files and folders in the system's file manager (Finder,
    /// File Explorer): the browser's Show in Finder (browser R25).
    ShowInFileManager(Vec<PathBuf>),
    /// Open the Settings window, or bring it to the front.
    OpenSettings,
    /// Close the Settings window.
    CloseSettings,
    /// Enter or leave full-screen mode.
    SetFullScreen(bool),
    /// Enter or leave full-screen mode in the second window.
    SetSecondFullScreen(bool),
    /// Move the keyboard focus to an area of the main window.
    FocusArea(Area),
    /// Move the keyboard focus to an area of the second window, and bring
    /// it to the front.
    FocusSecondArea(Area),
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
    /// Turn the Info View into a text field for an item's info text, with
    /// the keyboard focus; Return runs the item's command with the text
    /// ([`Shell::info_text_invocation`]), Option-Return starts a new line
    /// and Esc drops the change (Edit Info Text; the Info View spec, R10).
    EditInfoText(InfoTarget),
    /// Start renaming a take lane in its header, where the person types the
    /// new name; Return runs `take_lane.rename` with it, and Tab goes on to
    /// the next lane or track's name (WP-6.22; the comping spec, R28).
    RenameTakeLane(bl_model::TakeLaneId),
    /// Bring a floating device's window to the front (the mixer view spec,
    /// R28).
    RaiseDeviceWindow(bl_model::DeviceId),
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

/// What the MPE/Multi-channel Settings dialog edits (the MPE spec, R18).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MpeDialogTarget {
    /// A plug-in device's, or an External Instrument's MIDI To.
    Device(bl_model::DeviceId),
    /// A MIDI track's MIDI To.
    Track(bl_model::TrackId),
}

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
    /// The keys held on the MIDI input and the previews sounding, for the
    /// MIDI Note Editor's step recording and Preview (WP-6.5).
    step: step_input::StepInput,
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
    /// The second window's state (WP-6.29).
    second: windows::SecondWindow,
    /// The main window or the second window, whichever was in front last:
    /// the one the commands that act on one window act on.
    front_window: WindowKind,
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
    /// The Save History panel (IMP-05, WP-6.39): whether it shows and the
    /// rows it lists. View state, like the Undo History's.
    save_history: save_history::SaveHistory,
    /// What the Arrangement View shows (WP-1.8a).
    arrangement: ArrangementView,
    /// What the Clip View shows (WP-2.3): whether it's open, Draw Mode, and
    /// the notes selected in its MIDI Note Editor.
    clip_view: ClipView,
    /// Which of the Clip View and the Device View show, and how tall the
    /// Clip View is (WP-6.27). View state, like the Clip View's own.
    detail: DetailArea,
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
    packs: packs::PackState,
    /// What the MPE/Multi-channel Settings dialog edits, while it is open
    /// (the MPE spec, R18): never part of the set.
    mpe_dialog: Option<MpeDialogTarget>,
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
    /// The cache as the engine reads it for the files' transients: the
    /// live engine's, previews' and exports' (the warp modes spec, R14).
    analysis_slot: crate::analysed_media::AnalysisSlot,
    /// The files whose analysis the live engine hasn't heard is ready.
    analysis_unheard: std::collections::HashSet<bl_media::ContentHash>,
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
    /// A drag of clips between the Session and the Arrangement under way
    /// (WP-6.29b).
    clip_drag: Option<crate::clip_drag::ClipDrag>,
    /// A gesture that edits the set, under way (WP-1.8b).
    edit_gesture: Option<Gesture>,
    /// The edit a breakpoint selected from the keyboard has open (WP-6.26):
    /// the kind of envelope it is in and where the insert marker was. Its
    /// changes are one undo step, held in the transaction `gesture` opens.
    key_edit: Option<(bl_shell_spec::envelope_keys::EnvelopeKeys, Ticks)>,
    /// Whether a control other than the Arrangement View, an automation lane
    /// or the Envelope Editor holds the keyboard focus, so the envelope keys
    /// give way to it (WP-6.26). The UI reports it.
    envelope_control_focused: bool,
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
    /// The LV2 host: LV2 devices' instances, on this thread (WP-5.39).
    lv2: Lv2Host,
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
    /// The clips and tracks the ARA plug-ins' editors were last told are
    /// selected (IMP-20).
    ara_selection: (Vec<bl_model::ClipId>, Vec<bl_model::TrackId>),
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
        let (editor, window_views) = new_set_from(prefs.default_set.as_deref());
        // The engine's clips stream from the set's media (WP-1.4b).
        let media = Arc::new(MediaStore::new());
        audio.set_media(Arc::clone(&media));
        let analysis_slot = crate::analysed_media::AnalysisSlot::default();
        audio.set_analysis(analysis_slot.clone());
        // CLAP plug-ins (WP-2.6): this thread is their main thread.
        let plugins = ClapHost::new();
        audio.set_plugins(plugins.clone());
        // Audio Units (WP-2.8): their units live with this thread too.
        let au = AuHost::new();
        audio.set_au(au.clone());
        // VST3 plug-ins (WP-2.7): this thread is their main thread too.
        let vst3 = Vst3Host::new();
        audio.set_vst3(vst3.clone());
        // ARA plug-ins read the set's files through the same store (IMP-20).
        vst3.set_ara_media(Arc::clone(&media) as Arc<dyn bl_plugin_host::ara::AraMedia>);
        // LV2 plug-ins (WP-5.39): this thread is their main thread too.
        let lv2 = Lv2Host::new();
        audio.set_lv2(lv2.clone());
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
        let note_preview = prefs.note_preview;
        let mut shell = Self {
            browser_view,
            media,
            processed: Vec::new(),
            stems: stems::StemsState::default(),
            export: export_dialog::ExportState::default(),
            packs: packs::PackState::default(),
            mpe_dialog: None,
            autosample: None,
            autosample_dialog: None,
            autosample_result_value: None,
            slice_dialog: None,
            auditioning: None,
            analysis_options: bl_analysis::cache::AnalysisOptions::memory_only(),
            analysis: None,
            analysis_slot,
            analysis_unheard: std::collections::HashSet::new(),
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
            step: step_input::StepInput::default(),
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
            second: windows::SecondWindow::default(),
            front_window: WindowKind::Main,
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
            save_history: save_history::SaveHistory::default(),
            arrangement: ArrangementView::new(),
            clip_view: {
                let mut view = ClipView::default();
                view.set_preview(note_preview);
                view
            },
            detail: DetailArea::default(),
            file_manager: FileManager::default(),
            search: crate::search::Field::default(),
            plugin_list: plugin_list::PluginList::default(),
            document_revision: 0,
            clipboard: None,
            envelope_clipboard: None,
            clip_drag: None,
            edit_gesture: None,
            key_edit: None,
            envelope_control_focused: false,
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
            lv2,
            scanner,
            plugin_scan: None,
            plugin_scan_again: None,
            auto_scan: false,
            kit,
            device_commands: std::collections::BTreeMap::new(),
            device_commands_key: Vec::new(),
            plugins_generation: 0,
            ara_selection: (Vec::new(), Vec::new()),
        };
        // Spec: second-window R14. A saved default set keeps its main view
        // and the second window.
        shell.apply_window_views(window_views);
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

    /// The Status Bar's list of the open MIDI Tools chain (Spec: midi-tools
    /// R30).
    fn chain_status(&self) -> Option<String> {
        self.clip_view.midi_tools().chain_status(
            self.editor.document(),
            &self.clip_view,
            &self.arrangement,
        )
    }

    /// Shows the open chain's list in the Status Bar when a command changed
    /// it from `listed` (Spec: midi-tools R30).
    fn announce_chain(&mut self, listed: Option<String>) {
        if let Some(text) = self.chain_status()
            && Some(&text) != listed.as_ref()
        {
            self.set_message(text, false);
        }
    }

    /// The line the Status Bar shows while it has no message: what the
    /// pointer is over says about itself, such as a drum rack's pad
    /// (WP-5.6).
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// What the Status Bar says of the selected notes' probability groups,
    /// when they belong to any (Spec: note-editing R24).
    #[must_use]
    pub fn note_group_status(&self) -> Option<String> {
        self.clip_view
            .group_status(self.editor.document(), &self.arrangement)
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
        // Spec: ara-hosting R2, R4. The ARA plug-ins' documents follow the
        // set, and the compiler knows which devices are ARA plug-ins.
        self.vst3.know(self.editor.document());
        self.vst3
            .sync_ara(self.editor.document(), self.document_revision);
        self.audio.sync(self.editor.document());
        self.start_analyses();
        self.refresh_device_commands(false);
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

    /// The effects commands have asked for since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
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
        // Spec: automation-editing R23. The edit a selected breakpoint
        // opens ends when another command edits the set.
        if self.key_edit.is_some() && !self.keeps_key_edit(id) {
            self.end_key_edit();
        }
        // Spec: automation-editing R24. After Undo or Redo, the breakpoint
        // it restored is the focused one.
        let history = matches!(id, model::UNDO | model::REDO)
            .then(|| self.envelope_before_history())
            .flatten();
        let arranged = self.arrangement.selected_clips().to_vec();
        let slot = self.selected_slot_clip();
        let slots = self.selected_slot_clips();
        let shown = self.shown_clip();
        let revision = self.document_revision;
        let generation = self.editor_generation;
        let result = self.run_command(id, args);
        self.sync_key_edit();
        if result.is_ok() {
            self.follow_restored_breakpoint(history);
        }
        if self.search.open && !search_ids::handles(id) {
            // Spec: command-palette R7, R10. The rows' availability and the
            // outlines follow the set as it changes.
            self.refresh_search();
        }
        if result.is_ok() {
            self.follow_clip_selection(id, &arranged, slot, &slots);
            // Spec: scales R10. Selecting a clip makes its scale the
            // current scale, and a change to that clip's scale changes it
            // with it.
            self.follow_shown_scale(id, shown);
            // Spec: find-and-select-notes R12. Every filter is switched off
            // when the editor shows another clip.
            if self.shown_clip() != shown {
                self.clip_view.switch_filters_off();
            }
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

    fn run_command(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if history::handles(id) {
            // Spec: crash-recovery R20.
            return self.run_save_history(id, args);
        }
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
                || tool_ids::handles(id)
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
            return self.run_comping(id, args);
        }
        if edit_ids::handles(id) {
            return self.run_edit(id, args);
        }
        if automation_ids::handles(id) {
            // Spec: clip-envelopes R13. Edit ▸ Simplify Envelope acts on the
            // Envelope Editor's time selection while the editor has the
            // keyboard and the Arrangement has no automation lane selected.
            if id == automation_ids::SIMPLIFY
                && args.as_object().is_none_or(serde_json::Map::is_empty)
                && !crate::arrangement::automation::can_run(id, &self.arrangement, None)
                && self.can_clip_envelope_edit(clip_envelope_ids::SIMPLIFY, &self.active_contexts())
            {
                return self.run_clip_envelope_edit(clip_envelope_ids::SIMPLIFY, args);
            }
            return self.run_automation(id, args);
        }
        if envelope_key_ids::handles(id) {
            return self.run_envelope_keys(id, args);
        }
        if clip_envelope_ids::handles(id) {
            return self.run_clip_envelope_edit(id, args);
        }
        if locator_ids::handles(id) {
            return self.run_locator(id, args);
        }
        if note_ids::is_preview_or_mode(id) || id == note_ids::STEP_RECORD {
            // Spec: step-recording. The Preview switch, previews, step
            // recording and Step Input Mode reach the MIDI input, the
            // preferences and the focus.
            return self.run_step_input(id, args);
        }
        if id == note_ids::SPLIT_CLIP {
            return self.split_shown_clip();
        }
        if note_ids::handles(id) {
            let keys = id == note_ids::SELECT_NOTES
                && args["select"]
                    .as_str()
                    .is_some_and(|select| select != "all");
            let result = self.run_note_edit(id, args);
            if keys && result.is_ok() {
                // Spec: step-recording R2. A note selected from the keys
                // sounds, when Preview is on.
                self.preview_selected();
            }
            return result;
        }
        if let Some(clip) = self.audio_apply(id, &args) {
            // Spec: midi-transformations R22. On an audio clip the Transform
            // panel's apply button and Cmd+Enter quantise it with the
            // panel's settings.
            return self.run(
                bl_shell_spec::warping::QUANTIZE_AUDIO,
                json!({ "clip": clip }),
            );
        }
        if tool_ids::handles(id) {
            // Spec: midi-tools R23. The MIDI Tools' panels, choices,
            // settings and applications.
            let before = self.editor.document().clone();
            let listed = self.chain_status();
            let result = midi_tools::run(
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
            if result.is_ok() {
                self.announce_chain(listed);
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
            return self.run_model(id, args);
        }
        if engine_transport::handles(id) {
            return self.run_engine_transport(id, args);
        }
        if engine_session::handles(id) {
            return self.run_engine_session(id, args);
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
            // Spec: piano-roll R35. Follow again lets the note area follow
            // playback again.
            if id == view_ids::FOLLOW {
                self.clip_view
                    .note_follow(args["paused"].as_bool() == Some(true));
            }
            return self.run_arrangement_view(id, args);
        }
        if mixer_ids::handles(id) {
            return self.run_mixer_view(id, args);
        }
        if session_ids::handles(id) {
            return self.run_session_view(id, args);
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
            && let Some((set, rows)) = self.browser_view.selected_set_rows()
        {
            // Spec: set-exchange R4, R8. Enter on tracks of an opened set
            // adds them after the selected track, and on a Devices row adds
            // its chain onto the selected track.
            Self::args::<NoArgs>(id, args)?;
            let set = set.to_string_lossy().into_owned();
            let args = match rows.as_slice() {
                [crate::set_exchange::Row::Devices(track)] => {
                    json!({ "path": set, "devices": track })
                }
                rows => {
                    let tracks: Vec<TrackId> = rows
                        .iter()
                        .filter_map(|row| match row {
                            crate::set_exchange::Row::Track(track) => Some(*track),
                            _ => None,
                        })
                        .collect();
                    json!({ "path": set, "tracks": tracks })
                }
            };
            return self.run(set_exchange_ids::IMPORT, args);
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
            && let Some(path) = self.browser_view.selected_chord_bank()
        {
            // Spec: midi-generators R26. A chord bank chosen in the browser
            // loads into Stacks as `midi_tool.load_bank` loads it.
            Self::args::<NoArgs>(id, args)?;
            return self.run(
                bl_shell_spec::midi_tools::LOAD_BANK,
                json!({ "path": path.to_string_lossy() }),
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
        if bl_shell_spec::groove::handles(id) {
            return self.run_grooves(id, args);
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
        if tuning_ids::handles(id) {
            return self.run_tunings(id, args);
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
        if set_exchange_ids::handles(id) {
            // Spec: set-exchange R3 to R9, R11 to R15. Merging sets and
            // saving tracks and Session clips as a set.
            let result = self.run_set_exchange(id, args);
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

    /// The agent API's last audio previews (WP-1.16).
    #[must_use]
    pub fn previews(&self) -> &crate::perception::Previews {
        &self.previews
    }

    /// The agent API's last audio previews (WP-1.16, WP-2.28).
    pub fn previews_mut(&mut self) -> &mut crate::perception::Previews {
        &mut self.previews
    }

    /// The audio clip `midi_tool.apply` is for, if it is for one.
    fn audio_apply(&self, id: &str, args: &Value) -> Option<bl_model::ClipId> {
        if id != tool_ids::APPLY {
            return None;
        }
        let named = args
            .get("clip")
            .and_then(|clip| serde_json::from_value(clip.clone()).ok());
        midi_tools::audio_clip(
            self.editor.document(),
            &self.clip_view,
            &self.arrangement,
            named,
        )
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

    fn set_result(&self) -> Value {
        to_value(&SetResult {
            path: self
                .set_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            name: self.set_name(),
        })
    }

    fn run_shell(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id.split_once('.').map(|(namespace, _)| namespace) {
            Some("set" | "app" | "window") => self.run_set_files(id, args),
            Some("view") => self.run_view(id, args),
            Some("focus") => self.run_focus(id, args),
            Some("keymap") => self.run_keymap(id, args),
            Some("settings") => self.run_settings(id, args),
            Some("audio") => self.run_audio(id, args),
            Some("agent") => self.run_agent(id, args),
            Some("assistant") => self.run_assistant(id, args),
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}
