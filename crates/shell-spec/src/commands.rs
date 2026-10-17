//! The app shell's commands in the command registry (decisions D16 and
//! D17): files, windows, views, focus, settings, and the audio device.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask`
//! can check them and list them in the command reference without linking a UI
//! crate or an audio host. `bl-shell`'s `Shell::run` runs them by ID.

use bl_commands::Context::{ClipView, Dialog, Global, NoteEditor, Settings, TabFocus};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audio::{AudioDevices, AudioStatus, ConfigureAudio};
use crate::notes::{ClipViewState, NoteField};
use crate::settings::{SettingsPage, ThemeChoice};

/// `set.new`: replace the set with the factory default set.
pub const SET_NEW: &str = "set.new";
/// `set.open`: open a set file.
pub const SET_OPEN: &str = "set.open";
/// `set.save`: save the set to its file.
pub const SET_SAVE: &str = "set.save";
/// `set.save_as`: save the set to a new file.
pub const SET_SAVE_AS: &str = "set.save_as";
/// `set.discard_changes`: throw away the set's unsaved changes.
pub const SET_DISCARD_CHANGES: &str = "set.discard_changes";
/// `set.save_as_default`: save the set as the default set a new set starts
/// from.
pub const SET_SAVE_AS_DEFAULT: &str = "set.save_as_default";
/// `set.save_as_template`: save the set as a template in the User Library.
pub const SET_SAVE_AS_TEMPLATE: &str = "set.save_as_template";
/// `set.collect_all_and_save`: copy the set's external files into its
/// project folder, then save it.
pub const SET_COLLECT_ALL: &str = "set.collect_all_and_save";
/// `set.relink_missing`: search for the set's missing files and relink them.
pub const SET_RELINK_MISSING: &str = "set.relink_missing";
/// `app.quit`: quit.
pub const APP_QUIT: &str = "app.quit";
/// `app.hide`: hide the app (macOS).
pub const APP_HIDE: &str = "app.hide";
/// `app.about`: show the About window.
pub const APP_ABOUT: &str = "app.about";
/// `app.settings`: open the Settings window.
pub const APP_SETTINGS: &str = "app.settings";
/// `window.close`: close the frontmost secondary window.
pub const WINDOW_CLOSE: &str = "window.close";
/// `view.state`: what the window's view toggles show.
pub const VIEW_STATE: &str = "view.state";
/// `view.full_screen`: toggle full-screen mode.
pub const VIEW_FULL_SCREEN: &str = "view.full_screen";
/// `view.toggle_browser`: show or hide the browser.
pub const VIEW_TOGGLE_BROWSER: &str = "view.toggle_browser";
/// `view.toggle_clip_view`: show or hide the Clip View (WP-2.3).
pub const VIEW_TOGGLE_CLIP_VIEW: &str = "view.toggle_clip_view";
/// `view.draw_mode`: switch Draw Mode (B, WP-2.3).
pub const VIEW_DRAW_MODE: &str = "view.draw_mode";
/// `view.fold_notes`: fold the note editor to the rows that hold notes (F,
/// WP-2.3).
pub const VIEW_FOLD_NOTES: &str = "view.fold_notes";
/// `view.lane`: choose which value the Clip View's lane shows (WP-2.3).
pub const VIEW_LANE: &str = "view.lane";
/// `view.toggle_info_view`: show or hide the Info View.
pub const VIEW_TOGGLE_INFO_VIEW: &str = "view.toggle_info_view";
/// `view.toggle_plug_in_windows`: show or hide the plug-in windows: the
/// floating devices' windows and the plug-ins' editor windows (View ▸
/// Plug-In Windows, WP-2.5b).
pub const VIEW_TOGGLE_PLUG_IN_WINDOWS: &str = "view.toggle_plug_in_windows";
/// `view.toggle_session_arrangement`: switch the main view.
pub const VIEW_TOGGLE_SESSION_ARRANGEMENT: &str = "view.toggle_session_arrangement";
/// `view.zoom_in`: zoom the interface in.
pub const VIEW_ZOOM_IN: &str = "view.zoom_in";
/// `view.zoom_out`: zoom the interface out.
pub const VIEW_ZOOM_OUT: &str = "view.zoom_out";
/// `view.zoom_reset`: reset the interface zoom.
pub const VIEW_ZOOM_RESET: &str = "view.zoom_reset";
/// `view.set_zoom`: set the interface zoom to a percentage.
pub const VIEW_SET_ZOOM: &str = "view.set_zoom";
/// `focus.control_bar`: move the keyboard focus to the Control Bar.
pub const FOCUS_CONTROL_BAR: &str = "focus.control_bar";
/// `focus.arrangement`: move the keyboard focus to the Arrangement View.
pub const FOCUS_ARRANGEMENT: &str = "focus.arrangement";
/// `focus.next`: move the keyboard focus to the next control.
pub const FOCUS_NEXT: &str = "focus.next";
/// `focus.previous`: move the keyboard focus to the previous control.
pub const FOCUS_PREVIOUS: &str = "focus.previous";
/// `focus.toggle_tab_navigation`: switch Use Tab Key to Move Focus.
pub const FOCUS_TOGGLE_TAB_NAVIGATION: &str = "focus.toggle_tab_navigation";
/// `focus.next_neighbor`: move the keyboard focus to the same control on
/// the next track (WP-1.14).
pub const FOCUS_NEXT_NEIGHBOR: &str = "focus.next_neighbor";
/// `focus.previous_neighbor`: move the keyboard focus to the same control
/// on the previous track (WP-1.14).
pub const FOCUS_PREVIOUS_NEIGHBOR: &str = "focus.previous_neighbor";
/// `focus.toggle_wrap_navigation`: switch Wrap Keyboard Navigation, which
/// Settings calls Wrap Tab Navigation (WP-1.14).
pub const FOCUS_TOGGLE_WRAP_NAVIGATION: &str = "focus.toggle_wrap_navigation";
/// `settings.set_outline_view_in_focus`: switch Outline View in Focus
/// (WP-1.14).
pub const SETTINGS_SET_OUTLINE_VIEW_IN_FOCUS: &str = "settings.set_outline_view_in_focus";
/// `settings.next_page`: show the next Settings page.
pub const SETTINGS_NEXT_PAGE: &str = "settings.next_page";
/// `settings.previous_page`: show the previous Settings page.
pub const SETTINGS_PREVIOUS_PAGE: &str = "settings.previous_page";
/// `settings.set_theme`: choose the dark or light theme.
pub const SETTINGS_SET_THEME: &str = "settings.set_theme";
/// `settings.set_create_fades_on_clip_edges`: switch Create Fades on Clip
/// Edges.
pub const SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES: &str = "settings.set_create_fades_on_clip_edges";
/// `settings.set_warp_recorded_audio`: switch Warp recorded audio (WP-2.13).
pub const SETTINGS_SET_WARP_RECORDED_AUDIO: &str = "settings.set_warp_recorded_audio";
/// `settings.set_start_recording_on_scene_launch`: switch Start Recording on
/// Scene Launch (WP-3.11).
pub const SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH: &str =
    "settings.set_start_recording_on_scene_launch";
/// `settings.set_auto_hide_plug_in_windows`: switch Auto-Hide Plug-In
/// Windows (WP-2.5b).
pub const SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS: &str = "settings.set_auto_hide_plug_in_windows";
/// `audio.devices`: list drivers and devices.
pub const AUDIO_DEVICES: &str = "audio.devices";
/// `audio.status`: what the audio device runs at.
pub const AUDIO_STATUS: &str = "audio.status";
/// `audio.configure`: change the audio device settings.
pub const AUDIO_CONFIGURE: &str = "audio.configure";
/// `audio.toggle_engine`: switch the audio engine on or off.
pub const AUDIO_TOGGLE_ENGINE: &str = "audio.toggle_engine";
/// `audio.set_test_tone`: switch the Audio settings' test tone.
pub const AUDIO_SET_TEST_TONE: &str = "audio.set_test_tone";

/// Every command [`register`] adds apart from the agent server's and the
/// built-in assistant's ([`crate::agent::IDS`], [`crate::assistant::IDS`]),
/// so a host knows which IDs to send to `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SET_NEW,
    SET_OPEN,
    SET_SAVE,
    SET_SAVE_AS,
    SET_DISCARD_CHANGES,
    SET_SAVE_AS_DEFAULT,
    SET_SAVE_AS_TEMPLATE,
    SET_COLLECT_ALL,
    SET_RELINK_MISSING,
    APP_QUIT,
    APP_HIDE,
    APP_ABOUT,
    APP_SETTINGS,
    WINDOW_CLOSE,
    VIEW_STATE,
    VIEW_FULL_SCREEN,
    VIEW_TOGGLE_BROWSER,
    VIEW_TOGGLE_CLIP_VIEW,
    VIEW_DRAW_MODE,
    VIEW_FOLD_NOTES,
    VIEW_LANE,
    VIEW_TOGGLE_INFO_VIEW,
    VIEW_TOGGLE_PLUG_IN_WINDOWS,
    VIEW_TOGGLE_SESSION_ARRANGEMENT,
    VIEW_ZOOM_IN,
    VIEW_ZOOM_OUT,
    VIEW_ZOOM_RESET,
    VIEW_SET_ZOOM,
    FOCUS_CONTROL_BAR,
    FOCUS_ARRANGEMENT,
    FOCUS_NEXT,
    FOCUS_PREVIOUS,
    FOCUS_TOGGLE_TAB_NAVIGATION,
    FOCUS_NEXT_NEIGHBOR,
    FOCUS_PREVIOUS_NEIGHBOR,
    FOCUS_TOGGLE_WRAP_NAVIGATION,
    SETTINGS_NEXT_PAGE,
    SETTINGS_PREVIOUS_PAGE,
    SETTINGS_SET_THEME,
    SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
    SETTINGS_SET_WARP_RECORDED_AUDIO,
    SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH,
    SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS,
    SETTINGS_SET_OUTLINE_VIEW_IN_FOCUS,
    AUDIO_DEVICES,
    AUDIO_STATUS,
    AUDIO_CONFIGURE,
    AUDIO_TOGGLE_ENGINE,
    AUDIO_SET_TEST_TONE,
];

/// Whether `id` is one of the shell's commands: those in [`IDS`], the agent
/// server's ([`crate::agent::IDS`]), and the built-in assistant's
/// ([`crate::assistant::IDS`]).
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id) || crate::agent::handles(id) || crate::assistant::handles(id)
}

/// Arguments of `set.open`: a set file's path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathArgs {
    /// The set file's path, absolute or relative to the app's working
    /// directory.
    pub path: String,
}

/// Arguments of `set.save_as`: where to save the set, as a save dialog
/// gives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveAsArgs {
    /// A folder and the set's name, as a set file's path such as
    /// `/Music/Tango.blset`, absolute or relative to the app's working
    /// directory. The set goes in that folder if it's a project folder (it
    /// holds a set already), and otherwise in a new `<name> Project`
    /// folder inside it; the result says where.
    pub path: String,
}

/// Arguments of `set.save_as_default`: where the default set goes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefaultSetArgs {
    /// The file to save the default set as, absolute or relative to the
    /// app's working directory. Left out, it goes in the User Library's
    /// `Templates` folder, or beside the app's preferences when there is no
    /// User Library. `.blset` is added if it's missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Arguments of `set.save_as_template`: the template's name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateArgs {
    /// The template's name, without the `.blset` extension; left out uses
    /// the open set's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Arguments of `set.relink_missing`: folders to search besides the set's
/// project folder and the User Library.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelinkArgs {
    /// Folders to search, each with its subfolders, absolute or relative to
    /// the app's working directory. May be empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
}

/// What `set.collect_all_and_save` and `set.relink_missing` return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileReport {
    /// The set's file, if it has one.
    pub path: Option<String>,
    /// The number of files the command copied (Collect All and Save) or
    /// relinked (automatic search).
    pub files: usize,
}

/// Arguments of `app.settings`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsArgs {
    /// The page to show, or nothing for the page shown last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<SettingsPage>,
}

/// Arguments of `settings.set_theme`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemeArgs {
    /// The theme.
    pub theme: ThemeChoice,
}

/// Arguments of a command that switches a setting on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchArgs {
    /// Whether the setting is on.
    pub on: bool,
}

/// Arguments of `view.set_zoom`: the Display & Input page's Zoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZoomArgs {
    /// The interface zoom as a whole percentage, from 50 to 200; 100 is
    /// the size layout.md measures Live at.
    #[schemars(range(min = 50, max = 200))]
    pub percent: u16,
}

/// Arguments of `audio.set_test_tone`: what to change. Fields left out
/// keep their values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestToneArgs {
    /// Whether the test tone plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// The tone's level in dBFS, from -60 to 0 (Tone Volume). Values
    /// outside the range are brought within it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_db: Option<f64>,
    /// The tone's frequency in hertz, from 20 to 20,000 (Tone Frequency).
    /// Values outside the range are brought within it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_hz: Option<f64>,
}

/// What `audio.set_test_tone` returns: the test tone's state afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestToneState {
    /// Whether the test tone plays.
    pub on: bool,
    /// Its level in dBFS.
    pub volume_db: f64,
    /// Its frequency in hertz.
    pub frequency_hz: f64,
}

/// What `set.open`, `set.save`, and `set.save_as` return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetResult {
    /// The set's file, if it has one: after `set.save_as`, where the set
    /// was saved, which can be inside a new project folder.
    pub path: Option<String>,
    /// The set's name, as the title bar shows it.
    pub name: String,
}

/// Arguments of `view.toggle_plug_in_windows`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TogglePlugInWindowsArgs {
    /// Show (true) or hide (false) them; switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
}

/// Arguments of `view.fold_notes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FoldNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<bl_model::ClipId>,
    /// Fold (true) or unfold (false); switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folded: Option<bool>,
}

/// Arguments of `view.lane`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneArgs {
    /// Which value the lane shows.
    pub lane: NoteField,
}

/// What a command that switches something returns: its new state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwitchResult {
    /// Whether it's now on.
    pub on: bool,
}

/// What the view commands return: the window's view state afterwards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewResult {
    /// The main view: `arrangement` or `session`.
    pub main_view: String,
    /// Whether the browser shows.
    pub browser: bool,
    /// Whether the Info View shows.
    pub info_view: bool,
    /// Whether the main window is in full-screen mode.
    pub full_screen: bool,
    /// The interface zoom, where 1 is 100%.
    pub zoom: f32,
}

/// What `view.state` returns: what the view commands switch, without
/// switching any of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewState {
    /// The main window: the main view, the browser, the Info View,
    /// full-screen mode and the interface zoom.
    pub window: ViewResult,
    /// The Clip View and its note editor: whether it shows, Draw Mode, the
    /// clip it holds, the selected notes, the lane and Fold.
    pub clip_view: ClipViewState,
    /// The assistant's panel.
    pub assistant: crate::assistant::AssistantPanelState,
    /// Whether floating devices' and plug-ins' windows are shown, as
    /// `view.toggle_plug_in_windows` sets it.
    pub plug_in_windows: bool,
}

/// Adds the shell's commands to a registry, the agent server's
/// ([`crate::agent`]) among them.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    crate::agent::register(registry)?;
    crate::assistant::register(registry)
}

#[expect(
    clippy::too_many_lines,
    reason = "one spec per command, written out in full"
)]
fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(SET_NEW, "New Beatlayer Set")
            .description(
                "Replaces the open set with the factory default set: four tracks (two MIDI, \
                 two audio) and two return tracks, shown in the Arrangement View. Unsaved \
                 changes to the open set are lost, and its undo history is cleared, so save \
                 first if they matter.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .binding("S-FILE-001")
            .args::<NoArgs>()
            .result::<SetResult>(),
        CommandSpec::new(SET_OPEN, "Open Beatlayer Set...")
            .description(
                "Opens the set file at `path` (or the only set in the project folder at \
                 `path`), replacing the open set and clearing its undo history. It refuses \
                 while the open set has unsaved changes: save them first with `set.save` or \
                 `set.save_as`, or throw them away with `set.discard_changes`, so losing work \
                 is always its own step. A person choosing the menu item or pressing the \
                 shortcut is asked whether to save, then picks the file in a dialog.",
            )
            .permission(Permission::External)
            .context(Global)
            .binding("S-FILE-002")
            .args::<PathArgs>()
            .result::<SetResult>(),
        CommandSpec::new(SET_SAVE, "Save Beatlayer Set")
            .description(
                "Saves the set to its file, replacing the file's previous contents, which are \
                 kept in the `Backup` folder of the set's project folder (the last 10 saves). \
                 Fails if the set has never been saved; use `set.save_as` to give it a file \
                 and a project folder first.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .binding("S-FILE-003")
            .args::<NoArgs>()
            .result::<SetResult>(),
        CommandSpec::new(SET_SAVE_AS, "Save Beatlayer Set As...")
            .description(
                "Saves the set under a new name or in a new place, always inside a project \
                 folder, and that file becomes the set's. `path` gives a folder and the set's \
                 name, such as `/Music/Tango.blset` (`.blset` is added if it's missing). If the \
                 folder is a project folder, one that already holds a `.blset` set, the set \
                 joins that project as `<name>.blset` there; a set of that name there is \
                 replaced, and the replaced version kept in that project's `Backup` folder. \
                 Otherwise a new project folder, `<name> Project`, is made in the folder (or \
                 `<name> Project 2` and so on if that name is taken, so nothing is \
                 overwritten), and the set is saved inside it, as `/Music/Tango \
                 Project/Tango.blset`. The result's `path` is where the set was saved. Moving \
                 to another project folder keeps the set using the same media files. A person \
                 choosing the menu item or pressing the shortcut picks the path in a dialog \
                 that opens in the set's project folder.",
            )
            .permission(Permission::External)
            .context(Global)
            .binding("S-FILE-004")
            .args::<SaveAsArgs>()
            .result::<SetResult>(),
        CommandSpec::new(SET_DISCARD_CHANGES, "Don't Save")
            .description(
                "Throws away the open set's unsaved changes, as choosing Don't Save when asked \
                 to save does: the set goes back to how it was when it was last opened or \
                 saved (a set never saved goes back to the factory default set), and its undo \
                 history is cleared. The changes can't be brought back. Nothing on disk \
                 changes. Does nothing if there are no unsaved changes.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<NoArgs>()
            .result::<SetResult>(),
        CommandSpec::new(SET_SAVE_AS_DEFAULT, "Save Beatlayer Set As Default Set...")
            .description(
                "Saves the open set as the default set, which every new set starts from, as \
                 File ▸ Save Beatlayer Set As Default Set does in Live. `path` names the \
                 default set's file; left out it goes in the User Library's `Templates` folder, \
                 or beside the app's preferences when there is no User Library. The open set \
                 itself doesn't change and keeps its own file. The result's `path` is the \
                 default set's file.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<DefaultSetArgs>()
            .result::<SetResult>(),
        CommandSpec::new(SET_SAVE_AS_TEMPLATE, "Save Beatlayer Set As Template...")
            .description(
                "Saves the open set as a template in the User Library's `Templates` folder, \
                 making that folder if it isn't there, as File ▸ Save Beatlayer Set As \
                 Template does in Live. `name` names the template; left out it takes the open \
                 set's name. The open set itself doesn't change and keeps its own file. The \
                 result's `path` is the template's file.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<TemplateArgs>()
            .result::<SetResult>(),
        CommandSpec::new(SET_COLLECT_ALL, "Collect All and Save")
            .description(
                "Copies every file the open set uses that lives outside its project folder into \
                 the project's `Samples/Imported` folder, re-points the set's media table at \
                 the copies, and saves the set, as File ▸ Collect All and Save does in Live, so \
                 the set no longer depends on files elsewhere. Fails if the set has never been \
                 saved; use `set.save_as` to give it a project folder first. The result's \
                 `files` is how many files were copied.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<NoArgs>()
            .result::<FileReport>(),
        CommandSpec::new(SET_RELINK_MISSING, "Relink Missing Files")
            .description(
                "Searches for the files the open set uses that are missing or changed, and \
                 relinks each one to the first file found with the same content, as Live's \
                 automatic search does. The search covers the set's project folder, the User \
                 Library, the folders the files were last in, and any `folders` given, each \
                 with its subfolders. The result's `files` is how many files were relinked. It \
                 also runs by itself when a set is opened.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<RelinkArgs>()
            .result::<FileReport>(),
        CommandSpec::new(APP_QUIT, "Quit Beatlayer")
            .description(
                "Quits Beatlayer. Unsaved changes are lost unless the person saves them when \
                 asked.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .binding("S-FILE-005")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(APP_HIDE, "Hide Beatlayer")
            .description(
                "Hides Beatlayer's windows on macOS, as the application menu's Hide item does. \
                 Nothing else changes, and audio keeps running.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-FILE-006")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(APP_ABOUT, "About Beatlayer")
            .description(
                "Shows Beatlayer's version, licence, and credits in a window. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(APP_SETTINGS, "Settings...")
            .description(
                "Opens the Settings window on `page`, or on the page shown last. Changes no \
                 setting.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-019")
            .args::<SettingsArgs>()
            .result::<()>(),
        CommandSpec::new(WINDOW_CLOSE, "Close Window")
            .description(
                "Closes the frontmost secondary window, such as the Settings window. The main \
                 window stays open; quit to close it. From the keyboard or the File menu it \
                 acts only while a secondary window is in front, as in Live, where Close Window \
                 is disabled with only the main window open.",
            )
            .permission(Permission::Read)
            .contexts([Global, Dialog, Settings])
            .binding("S-FILE-009")
            .binding("S-VIEW-020")
            .binding("S-SET-007")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(VIEW_FULL_SCREEN, "Full Screen")
            .description(
                "Switches the main window into or out of full-screen mode. Changes nothing in \
                 the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-001")
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_TOGGLE_PLUG_IN_WINDOWS, "Plug-In Windows")
            .description(
                "Shows or hides every floating device's window and every plug-in's editor \
                 window at once, as View ▸ Plug-In Windows does for Live's plug-in windows \
                 (Cmd-Option-P); switches unless `shown` says which. Hidden windows' devices \
                 still float, and a hidden editor stays open: it isn't docking or closing, \
                 isn't an undo step, and changes nothing in the set. Floating a device shows \
                 the windows again. Only runs while a device floats or a plug-in window is \
                 open. The result's `on` is whether they're shown afterwards.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-DEV-009")
            .args::<TogglePlugInWindowsArgs>()
            .result::<SwitchResult>(),
        CommandSpec::new(VIEW_TOGGLE_BROWSER, "Browser")
            .description(
                "Shows or hides the browser at the left of the main window; the main view \
                 widens to fill its place. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-010")
            .binding("S-BRW-002")
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_STATE, "View State")
            .description(
                "Reports what the view commands switch: the main view (Arrangement or \
                 Session), whether the browser and the Info View show, full-screen mode and \
                 the zoom (`view.toggle_session_arrangement`, `view.toggle_browser`, \
                 `view.toggle_info_view`, `view.full_screen`, `view.set_zoom`), the Clip View \
                 with Draw Mode, the lane, Fold and the selected notes (`view.toggle_clip_view`, \
                 `view.draw_mode`, `view.lane`, `view.fold_notes`), the assistant's panel and \
                 whether plug-in windows are shown (`view.toggle_plug_in_windows`). Changes \
                 nothing. The Arrangement's own view is in `arrangement.state`, the Mixer's in \
                 `mixer_view.state`, the Session View's in `session.state` and the Device \
                 View's in `device_view.state`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<ViewState>(),
        CommandSpec::new(VIEW_TOGGLE_CLIP_VIEW, "Clip View")
            .description(
                "Shows or hides the Clip View in the detail area, which holds the MIDI Note \
                 Editor for the Arrangement's selected MIDI clip; while it shows, it takes \
                 the detail area in place of the Device View. It shows the clip the \
                 Arrangement has selected, with its notes, and selecting another MIDI clip \
                 switches the editor to it, clearing the note selection. Changes nothing in \
                 the set and isn't an undo step. The result says whether it shows, what clip \
                 the editor holds, and which notes are selected.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-015")
            .args::<NoArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_DRAW_MODE, "Draw Mode")
            .description(
                "Switches Draw Mode (B; Options ▸ Draw Mode), which changes what a click in \
                 the MIDI Note Editor's note area does: with it on, clicking empty grid adds \
                 a note one grid step long, and clicking a note removes it; with it off, \
                 clicking selects. Changes nothing in the set and isn't an undo step. The \
                 result says whether it's on.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-GRID-001")
            .args::<NoArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_FOLD_NOTES, "Fold to Notes")
            .description(
                "Folds or unfolds the MIDI Note Editor (F, View ▸ Fold to Notes): while it's \
                 folded, the editor shows only the key rows that hold notes, one under the \
                 next, and not the empty rows between them. `clip` names the clip; left out, \
                 the one the editor shows. `folded` says which way; left out, it switches. \
                 Folding changes nothing in the set and isn't an undo step: it's saved with \
                 the set beside the song, so it's there again when the set is reopened. The \
                 result says whether the editor is folded now.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor])
            .binding("S-NOTE-038")
            .args::<FoldNotesArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_LANE, "Note Editor Lane")
            .description(
                "Chooses which of a note's values the lane under the MIDI Note Editor shows \
                 and edits: `velocity` (the lane's default), `release_velocity`, `chance`, or \
                 `velocity_deviation`. Dragging a note's stem in the lane sets that value on \
                 that note, and dragging across several notes ramps it; the same edits are \
                 the `note.set_value` and `note.ramp_value` commands. Changes only the view, \
                 so it's no undo step and isn't saved with the set.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor])
            .args::<LaneArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_TOGGLE_INFO_VIEW, "Info")
            .description(
                "Shows or hides the Info View at the bottom left of the main window, which \
                 reads whatever the pointer is over: the control's name, the help text of \
                 the command it runs, and its key on a separate line, and shows nothing \
                 while the pointer is over empty space. A new set opens with it shown, as \
                 in Live. Changes nothing in the set and isn't an undo step.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-008")
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(
            VIEW_TOGGLE_SESSION_ARRANGEMENT,
            "Toggle Arrangement/Session View",
        )
        .description(
            "Switches the main view between the Arrangement View and the Session View. A new \
             set opens in the Arrangement View. Changes nothing in the set.",
        )
        .permission(Permission::Read)
        .context(Global)
        .binding("S-VIEW-003")
        .args::<NoArgs>()
        .result::<ViewResult>(),
        // The interface zoom (WP-1.17; spec interface-zoom R5): edit-class
        // preferences, since each writes the preferences file and is
        // trivially changed back (D17's permission classes).
        CommandSpec::new(VIEW_ZOOM_IN, "Zoom In")
            .description(
                "Makes the whole interface larger by one step of 10%, up to 200%: every \
                 window's panes, controls, and text scale together and keep their places. \
                 From a zoom between steps it goes to the next step up. The zoom is kept for \
                 the next start. Changes nothing in the set and isn't an undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .binding("S-ZOOM-001")
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_ZOOM_OUT, "Zoom Out")
            .description(
                "Makes the whole interface smaller by one step of 10%, down to 50%: every \
                 window's panes, controls, and text scale together and keep their places. \
                 From a zoom between steps it goes to the next step down. The zoom is kept for \
                 the next start. Changes nothing in the set and isn't an undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .binding("S-ZOOM-002")
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_ZOOM_RESET, "Reset Zoom to 100%")
            .description(
                "Returns the interface zoom to 100%, the size Live's layout is measured at. \
                 The zoom is kept for the next start. Changes nothing in the set and isn't an \
                 undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_SET_ZOOM, "Zoom")
            .description(
                "Sets the interface zoom to `percent`, a whole percentage from 50 to 200, as \
                 the Zoom setting on the Settings window's Display & Input page does: every \
                 window's panes, controls, and text scale together and keep their places, and \
                 the main window grows if it's smaller than its minimum size at that zoom (as \
                 far as the screen allows). The result's `zoom` is the new zoom as a factor, \
                 1.5 for 150%. The zoom is kept for the next start. Changes nothing in the set \
                 and isn't an undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Settings])
            .args::<ZoomArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(FOCUS_CONTROL_BAR, "Control Bar")
            .description(
                "Moves the keyboard focus to the Control Bar at the top of the main window, \
                 onto the control focused there last. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-FOCUS-001")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(FOCUS_ARRANGEMENT, "Arrangement View")
            .description(
                "Shows the Arrangement View if the Session View is in front, and moves the \
                 keyboard focus to it, so its shortcuts apply. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-FOCUS-003")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(FOCUS_NEXT, "Next")
            .description(
                "Moves the keyboard focus to the next control in the focused area or window. \
                 Tab does this in the Settings window, and in the main window while Use Tab \
                 Key to Move Focus is on.",
            )
            .permission(Permission::Read)
            .contexts([TabFocus, Settings])
            .binding("S-FOCUS-011")
            .binding("S-SET-001")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(FOCUS_PREVIOUS, "Previous")
            .description(
                "Moves the keyboard focus to the previous control in the focused area or \
                 window, the reverse of `focus.next`.",
            )
            .permission(Permission::Read)
            .contexts([TabFocus, Settings])
            .binding("S-FOCUS-012")
            .binding("S-SET-002")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(FOCUS_TOGGLE_TAB_NAVIGATION, "Use Tab Key to Move Focus")
            .description(
                "Switches whether Tab and Shift-Tab move the keyboard focus between controls in \
                 the main window. While it's off, as it is by default, Tab switches between the \
                 Session and Arrangement Views. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SwitchResult>(),
        // The Navigate menu's neighbours (M-NAV-014, M-NAV-015; S-FOCUS-013,
        // S-FOCUS-014), and its wrap switch (M-NAV-019), WP-1.14.
        CommandSpec::new(FOCUS_NEXT_NEIGHBOR, "Next Neighbor")
            .description(
                "Moves the keyboard focus from a track's control to the same control on the \
                 next track, such as from one track's volume to the next track's volume, in \
                 the mixers and the Arrangement's track headers, without changing the \
                 selected track. Navigate > Next Neighbor runs it whenever there's a next \
                 track; Option-Tab (Ctrl-Tab) does while Use Tab Key to Move Focus is on. \
                 Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Global, TabFocus])
            .binding("S-FOCUS-013")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(FOCUS_PREVIOUS_NEIGHBOR, "Previous Neighbor")
            .description(
                "Moves the keyboard focus from a track's control to the same control on the \
                 previous track, the reverse of `focus.next_neighbor`. Changes nothing in the \
                 set.",
            )
            .permission(Permission::Read)
            .contexts([Global, TabFocus])
            .binding("S-FOCUS-014")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(FOCUS_TOGGLE_WRAP_NAVIGATION, "Wrap Keyboard Navigation")
            .description(
                "Switches whether Tab and Shift-Tab wrap round at the ends of the focused area \
                 or track: from its last control to its first, and from its first to its last. \
                 It's on in a new install, as Live's is; while it's off, they stop at the ends. \
                 The Settings window calls it Wrap Tab Navigation. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SwitchResult>(),
        CommandSpec::new(SETTINGS_NEXT_PAGE, "Next Settings Page")
            .description(
                "Shows the next page of the Settings window, wrapping from the last to the \
                 first. Changes no setting.",
            )
            .permission(Permission::Read)
            .context(Settings)
            .binding("S-SET-005")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_PREVIOUS_PAGE, "Previous Settings Page")
            .description(
                "Shows the previous page of the Settings window, wrapping from the first to \
                 the last. Changes no setting.",
            )
            .permission(Permission::Read)
            .context(Settings)
            .binding("S-SET-006")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_THEME, "Theme")
            .description(
                "Chooses Beatlayer's dark or light theme, which every window uses straight \
                 away. The choice is kept for the next start. Changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<ThemeArgs>()
            .result::<()>(),
        // Record, Warp & Launch, Warp/Fades (L-SET-018; WP-1.4b): an
        // edit-class preference, like the theme.
        CommandSpec::new(
            SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
            "Create Fades on Clip Edges",
        )
        .description(
            "Switches Create Fades on Clip Edges, on the Settings window's Record, Warp & Launch \
             page; it's on in a new install, as Live's is. While it's on, each audio clip added \
             to the Arrangement from now on gets fades of 4 ms (0.004 s) at both edges, so a \
             clip that starts or ends mid-waveform doesn't click; clips already in the set keep \
             their fades, which `clip.set_fades` changes. The setting is kept for the next \
             start, and isn't part of the set or an undo step. The result's `on` is the setting \
             afterwards.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<SwitchArgs>()
        .result::<SwitchResult>(),
        // Record, Warp & Launch, Warp/Fades (IMP-02; WP-2.13).
        CommandSpec::new(SETTINGS_SET_WARP_RECORDED_AUDIO, "Warp Recorded Audio")
            .description(
                "Switches Warp recorded audio, on the Settings window's Record, Warp & Launch \
                 page, a Beatlayer addition (IMP-02); it's on in a new install, as Live warps \
                 its recordings. Clips have no warp settings yet, so for now recorded clips play \
                 at their file's own speed whether it's on or off; warping will read the setting \
                 for the clips a recording makes. The setting is kept for the next start, and \
                 isn't part of the set or an undo step. The result's `on` is the setting \
                 afterwards.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SwitchArgs>()
            .result::<SwitchResult>(),
        // Record, Warp & Launch, Launch (WP-3.11).
        CommandSpec::new(
            SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH,
            "Start Recording on Scene Launch",
        )
        .description(
            "Switches Start Recording on Scene Launch, on the Settings window's Record, Warp \
             & Launch page; it's off in a new install, as Live's is. While it's off, \
             launching a scene leaves the empty slots of armed tracks alone; while it's on, \
             launching a scene also starts recording into each of them, on the scene's launch \
             grid line, as Trigger Recording on Launch in a scene's menu does. The setting is \
             kept for the next start, and isn't part of the set or an undo step. The result's \
             `on` is the setting afterwards.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<SwitchArgs>()
        .result::<SwitchResult>(),
        // Plug-Ins, Plug-In Windows (L-SET-017; WP-2.5b).
        CommandSpec::new(
            SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS,
            "Auto-Hide Plug-In Windows",
        )
        .description(
            "Switches Auto-Hide Plug-In Windows, on the Settings window's Plug-Ins page; it's on \
             in a new install, as Live's is. While it's on, a floating device's window or a \
             plug-in's editor window hides while another track is selected and comes back \
             when its own track is selected again, as Live's plug-in windows do. A hidden \
             window's device still floats, and a hidden editor stays open: hiding isn't \
             docking or closing, isn't an undo step, and the set saves the device as \
             floating. `device.windows` and `plugin.info` say which windows show. The \
             setting is kept for the next start, and isn't part of the set. The result's \
             `on` is the setting afterwards.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<SwitchArgs>()
        .result::<SwitchResult>(),
        // Display & Input, Display Options (L-SET-010; WP-1.14).
        CommandSpec::new(SETTINGS_SET_OUTLINE_VIEW_IN_FOCUS, "Outline View in Focus")
            .description(
                "Switches Outline View in Focus, on the Settings window's Display & Input page: \
             while it's on, as it is in a new install (as Live's is), a border is drawn round \
             the part of the main window that has the keyboard focus and an outline round the \
             focused control; while it's off, nothing shows where the focus is. The setting is \
             kept for the next start, and isn't part of the set or an undo step. The result's \
             `on` is the setting afterwards.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SwitchArgs>()
            .result::<SwitchResult>(),
        CommandSpec::new(AUDIO_DEVICES, "List Audio Devices")
            .description(
                "Lists the driver types this computer offers and, for the chosen one, its \
                 input and output devices with the sample rates, buffer sizes, and channels \
                 each accepts. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Settings)
            .args::<NoArgs>()
            .result::<AudioDevices>(),
        CommandSpec::new(AUDIO_STATUS, "Audio Status")
            .description(
                "Reports whether the audio engine is on, the driver and devices in use, the \
                 sample rate and buffer size the device actually runs at, the CPU load of \
                 audio processing, and any device error. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<AudioStatus>(),
        CommandSpec::new(AUDIO_CONFIGURE, "Configure Audio Device")
            .description(
                "Changes any of the audio driver type, input and output devices, sample rate, \
                 buffer size, and output channels, and applies them straight away without a \
                 restart. Fields left out keep their values. If the device refuses the new \
                 settings, the previous ones are restored and the error is returned. Changing \
                 the sample rate can change the device's own setting for other apps too. The \
                 settings are kept for the next start.",
            )
            .permission(Permission::External)
            .context(Settings)
            .args::<ConfigureAudio>()
            .result::<AudioStatus>(),
        CommandSpec::new(AUDIO_TOGGLE_ENGINE, "Audio Engine On")
            .description(
                "Switches the audio engine off, closing the audio device, or on again, \
                 reopening it with the current settings. While it's off, nothing plays and the \
                 CPU meter reads OFF.",
            )
            .permission(Permission::External)
            .context(Global)
            .binding("S-ENGINE-001")
            .args::<NoArgs>()
            .result::<SwitchResult>(),
        CommandSpec::new(AUDIO_SET_TEST_TONE, "Test Tone")
            .description(
                "Plays or stops the Audio settings' test tone, a sine wave on every output \
                 channel for checking that the device works, and sets its level (Tone Volume, \
                 -60 to 0 dBFS; -36 on a new install) and frequency (Tone Frequency, 20 Hz to \
                 20 kHz; 440 Hz on a new install). Fields left out keep their values; the \
                 level and frequency are kept for the next start. The tone stops when the \
                 Settings window closes. Set a low level before switching it on: a loud tone \
                 can hurt ears and speakers.",
            )
            .permission(Permission::External)
            .context(Settings)
            .args::<TestToneArgs>()
            .result::<TestToneState>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(
            registry.len(),
            IDS.len() + crate::agent::IDS.len() + crate::assistant::IDS.len()
        );
        for id in IDS {
            assert!(registry.contains(id), "{id}");
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn the_shell_and_the_document_register_side_by_side() {
        let mut registry = Registry::new();
        bl_model::registry::register(&mut registry).unwrap();
        register(&mut registry).unwrap();
        assert!(registry.problems().is_empty());
    }
}
