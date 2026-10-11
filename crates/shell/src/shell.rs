//! The shell's state and the one place every command runs: [`Shell`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_commands::{Context, NoArgs, Registry};
use bl_media::MediaStore;
use bl_model::{Editor, TrackId, TrackKind, registry as model};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::arrangement::{ArrangementView, TransportView, ViewError, Viewport};
use crate::audio::{AudioHost, ConfigureAudio, ToneSettings};
use crate::browser::{self, BrowserError, BrowserView, Waker};
use crate::commands::{
    self as ids, PathArgs, SaveAsArgs, SetResult, SettingsArgs, SwitchResult, TestToneArgs,
    TestToneState, ThemeArgs, ViewResult, ZoomArgs,
};
use crate::files::{ProjectFiles, SetStore, factory_default_set};
use crate::settings::{
    Preferences, SettingsPage, ThemeChoice, ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT, zoom_factor,
    zoom_percent, zoom_step,
};
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::browser as browser_ids;

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
    /// Move the keyboard focus to the next or previous control.
    MoveFocus {
        /// Whether it moves backwards.
        backwards: bool,
    },
    /// Ask for a folder, then add it to the browser's Places with
    /// `browser.add_folder` (WP-1.10).
    PickFolder,
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
    full_screen: bool,
    settings_open: bool,
    focused_window: WindowKind,
    focused_area: Area,
    effects: Vec<Effect>,
    message: Option<Message>,
    revision: u64,
    /// What the Arrangement View shows (WP-1.8a).
    arrangement: ArrangementView,
    /// A number that changes whenever the document may have changed.
    document_revision: u64,
    /// The browser (WP-1.10).
    browser_view: BrowserView,
    /// The media the set's clips play, which the browser fills and the
    /// Arrangement's waveforms read.
    media: Arc<MediaStore>,
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
        debug_assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        audio.set_tone(prefs.tone);
        let browser_view = BrowserView::new(&prefs.browser, Waker::default());
        audio.set_preview_gain(prefs.browser.preview_volume as f32);
        Self {
            browser_view,
            media: Arc::new(MediaStore::new()),
            registry,
            editor: factory_default_set(),
            saved: factory_default_set(),
            set_path: None,
            dirty: false,
            store,
            audio,
            settings_open: false,
            prefs,
            config_dir,
            main_view: MainView::Arrangement,
            browser: true,
            full_screen: false,
            focused_window: WindowKind::Main,
            focused_area: Area::MainView,
            effects: Vec::new(),
            message: None,
            revision: 0,
            arrangement: ArrangementView::new(),
            document_revision: 0,
        }
    }

    /// A shell for tests and headless runs: default preferences saved
    /// nowhere, the null audio device, and Beatlayer's set files.
    #[must_use]
    pub fn headless() -> Self {
        Self::new(
            Preferences::default(),
            None,
            AudioHost::null(Default::default()),
            Box::new(ProjectFiles::new()),
        )
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

    /// The audio device.
    #[must_use]
    pub fn audio(&self) -> &AudioHost {
        &self.audio
    }

    /// The audio device, to sample its CPU load.
    pub fn audio_mut(&mut self) -> &mut AudioHost {
        &mut self.audio
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

    /// The theme.
    #[must_use]
    pub fn theme(&self) -> ThemeChoice {
        self.prefs.theme
    }

    /// Whether Tab moves the keyboard focus in the main window.
    #[must_use]
    pub fn tab_navigation(&self) -> bool {
        self.prefs.tab_navigation
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
            self.touch();
        }
    }

    /// Records which area of the main window has the keyboard focus, as
    /// clicking in it does (I-FOCUS-001).
    pub fn set_focused_area(&mut self, area: Area) {
        if self.focused_area != area {
            self.focused_area = area;
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
    /// tracks that are gone.
    fn document_changed(&mut self) {
        self.document_revision = self.document_revision.wrapping_add(1);
        self.arrangement
            .forget_missing_tracks(self.editor.document());
    }

    /// What the Arrangement View shows (WP-1.8a).
    #[must_use]
    pub fn arrangement(&self) -> &ArrangementView {
        &self.arrangement
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

    /// Takes the transport's latest state from the engine (the playhead,
    /// whether it plays, the loop brace), for the Arrangement View to draw
    /// and, with Follow on, to keep in view. The host calls it as the
    /// engine's UI state changes; the transport package (WP-1.5) wires it.
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
        if polled.changed {
            self.touch();
        }
        polled.changed
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

    /// Runs a browser command and carries out what it asks of the rest of
    /// the shell.
    fn run_browser(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let context = browser::Context {
            insert_marker: self.arrangement.insert_marker(),
            selected_track: self.arrangement.selected_tracks().first().copied(),
            editor: &mut self.editor,
            audio: &mut self.audio,
            media: &self.media,
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
            ids::FOCUS_TOGGLE_TAB_NAVIGATION => Some(self.prefs.tab_navigation),
            ids::AUDIO_TOGGLE_ENGINE => Some(self.audio.engine_on()),
            browser_ids::SET_PREVIEW => Some(self.browser_view.preview_on()),
            _ => self.arrangement.switch_state(id),
        }
    }

    /// The label a menu shows for a command now: Undo and Redo name the step
    /// they act on, or say there's nothing (M-EDIT-001, M-EDIT-002).
    #[must_use]
    pub fn dynamic_label(&self, id: &str) -> Option<String> {
        match id {
            model::UNDO => Some(self.editor.undo_history().last().map_or_else(
                || "Nothing to Undo".to_owned(),
                |step| format!("Undo {step}"),
            )),
            model::REDO => Some(self.editor.redo_history().last().map_or_else(
                || "Nothing to Redo".to_owned(),
                |step| format!("Redo {step}"),
            )),
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
            ids::WINDOW_CLOSE => {
                return self.settings_open && self.focused_window == WindowKind::Settings;
            }
            _ => {}
        }
        if ids::handles(id) {
            return true;
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
            let (index, name) = if kind == TrackKind::Return {
                let index = doc.returns().len();
                let letter = char::from(b'A' + (index % 26) as u8);
                (index, format!("{letter} Return"))
            } else {
                let index = doc.top_level().len();
                let word = match kind {
                    TrackKind::Midi => "MIDI",
                    TrackKind::Group => "Group",
                    _ => "Audio",
                };
                (index, format!("{} {word}", index + 1))
            };
            // A placeholder that fits the schema; `invoke` issues the real ID.
            fields.insert("id".to_owned(), json!(1));
            fields.entry("name").or_insert_with(|| json!(name));
            fields.entry("index").or_insert_with(|| json!(index));
        }
        Ok(Value::Object(fields))
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
            "track.create" if !has("id") => {
                let mut args = self.filled_args(id, args)?;
                let new: TrackId = self.editor.new_id();
                args["id"] = json!(new);
                self.run(id, args)
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
        if model::handles(id) {
            let changes =
                model::run(&mut self.editor, id, args).map_err(|error| ShellError::Failed {
                    command: id.to_owned(),
                    message: error.to_string(),
                })?;
            self.dirty = true;
            self.document_changed();
            self.touch();
            return Ok(to_value(&changes));
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
            return result.map(|state| to_value(&state));
        }
        if browser_ids::handles(id) {
            return self.run_browser(id, args);
        }
        if !ids::handles(id) {
            return Err(ShellError::UnknownCommand(id.to_owned()));
        }
        let result = self.run_shell(id, args);
        self.touch();
        result
    }

    fn args<T: DeserializeOwned>(id: &str, args: Value) -> Result<T, ShellError> {
        let args = if args.is_null() { json!({}) } else { args };
        serde_json::from_value(args).map_err(|error| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: error.to_string(),
        })
    }

    fn view_result(&self) -> Value {
        to_value(&ViewResult {
            main_view: self.main_view.id().to_owned(),
            browser: self.browser,
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
                self.editor = factory_default_set();
                self.saved = factory_default_set();
                self.store.close();
                self.set_path = None;
                self.dirty = false;
                self.main_view = MainView::Arrangement;
                self.message = None;
                self.arrangement.reset();
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
                self.saved = snapshot(&editor);
                self.editor = editor;
                self.set_path = Some(opened);
                self.dirty = false;
                self.message = None;
                self.arrangement.reset();
                self.document_changed();
                // The set's files go into the media store, so its clips'
                // waveforms build (browser R15).
                for (hash, path, size) in self.store.media_files() {
                    self.media
                        .add_reference(bl_media::ContentHash::from_bytes(hash.0), path, size);
                }
                self.set_file_changed();
                Ok(self.set_result())
            }
            ids::SET_SAVE => {
                Self::args::<NoArgs>(id, args)?;
                let path = self.set_path.clone().ok_or_else(|| {
                    unavailable("the set has never been saved; use `set.save_as` to give it a file")
                })?;
                let warnings = self.store.save(&path, &self.editor).map_err(failed)?;
                self.saved_as(path, &warnings);
                Ok(self.set_result())
            }
            ids::SET_SAVE_AS => {
                // Spec: app-shell R17 to R21. The store decides where in a
                // project folder the set goes; the result says where.
                let SaveAsArgs { path } = Self::args(id, args)?;
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
                    self.editor = snapshot(&self.saved);
                    self.dirty = false;
                    self.document_changed();
                }
                Ok(self.set_result())
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
            ids::VIEW_TOGGLE_BROWSER => {
                Self::args::<NoArgs>(id, args)?;
                self.browser = !self.browser;
                if !self.browser && self.focused_area == Area::Browser {
                    self.focused_area = Area::MainView;
                }
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
                });
                Ok(Value::Null)
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
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
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
