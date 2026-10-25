//! The app shell's commands in the command registry (decisions D16 and
//! D17): files, windows, views, focus, settings, and the audio device.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask`
//! can check them and list them in the command reference without linking a UI
//! crate or an audio host. `bl-shell`'s `Shell::run` runs them by ID.

use bl_commands::Context::{
    ClipView, Dialog, Envelope, Global, NoteEditor, SampleEditor, Settings, TabFocus,
};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use bl_model::{LaunchMode, LaunchQuantization};

use crate::audio::{AudioDevices, AudioStatus, ConfigureAudio};
use crate::notes::{ClipTab, ClipViewState, NoteField};
use crate::settings::{Appearance, ClipColour, SettingsPage, Tone};

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
/// `set.later_recovery`: close the recovery prompt, leaving the sets it
/// listed to be offered at the next launch (WP-6.62, the crash-recovery
/// spec, R22).
pub const SET_LATER_RECOVERY: &str = "set.later_recovery";
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
/// `view.toggle_second_window`: open or close the second window (WP-6.29).
pub const VIEW_TOGGLE_SECOND_WINDOW: &str = "view.toggle_second_window";
/// `view.toggle_browser`: show or hide the browser.
pub const VIEW_TOGGLE_BROWSER: &str = "view.toggle_browser";
/// Runs the browser down to the Status Bar, or back to the detail area's
/// top (Full-Height Browser; spec: browser R26).
pub const VIEW_TOGGLE_FULL_HEIGHT_BROWSER: &str = "view.toggle_full_height_browser";
/// `view.tuning_section`: show or hide the browser's Tuning section (WP-6.10;
/// the tunings spec, R28).
pub const VIEW_TUNING_SECTION: &str = "view.tuning_section";
/// `view.toggle_clip_view`: show or hide the Clip View (WP-2.3).
pub const VIEW_TOGGLE_CLIP_VIEW: &str = "view.toggle_clip_view";
/// `view.toggle_device_view`: show or hide the Device View (WP-6.27).
pub const VIEW_TOGGLE_DEVICE_VIEW: &str = "view.toggle_device_view";
/// `view.show_detail_views`: show the Clip View and the Device View together
/// (WP-6.27).
pub const VIEW_SHOW_DETAIL_VIEWS: &str = "view.show_detail_views";
/// `view.switch_detail_view`: switch between the Clip View and the Device
/// View, or move the focus between them (WP-6.27).
pub const VIEW_SWITCH_DETAIL_VIEW: &str = "view.switch_detail_view";
/// `view.toggle_detail_area`: hide the detail views, or bring them back
/// (WP-6.27).
pub const VIEW_TOGGLE_DETAIL_AREA: &str = "view.toggle_detail_area";
/// `view.expand_clip_view`: give the Clip View its largest height, or put it
/// back (WP-6.27).
pub const VIEW_EXPAND_CLIP_VIEW: &str = "view.expand_clip_view";
/// `view.set_detail_height`: set the Clip View's height (WP-6.27).
pub const VIEW_SET_DETAIL_HEIGHT: &str = "view.set_detail_height";
/// `view.set_browser_width`: set the browser's width (browser R29).
pub const VIEW_SET_BROWSER_WIDTH: &str = "view.set_browser_width";
/// `focus.clip_view`: move the keyboard focus to the Clip View (WP-6.27).
pub const FOCUS_CLIP_VIEW: &str = "focus.clip_view";
/// `focus.device_view`: move the keyboard focus to the Device View
/// (WP-6.27).
pub const FOCUS_DEVICE_VIEW: &str = "focus.device_view";
/// `view.toggle_plugin_list`: show or hide the list of the set's plug-ins
/// (IMP-29, WP-6.41).
pub const VIEW_TOGGLE_PLUGIN_LIST: &str = "view.toggle_plugin_list";
/// `view.set_plugin_list_only_missing`: switch Show Only Missing in the list
/// of the set's plug-ins (IMP-29, WP-6.41).
pub const VIEW_SET_PLUGIN_LIST_ONLY_MISSING: &str = "view.set_plugin_list_only_missing";
/// `view.draw_mode`: switch Draw Mode (B, WP-2.3).
pub const VIEW_DRAW_MODE: &str = "view.draw_mode";
/// `view.fold_notes`: fold the note editor to the rows that hold notes (F,
/// WP-2.3).
pub const VIEW_FOLD_NOTES: &str = "view.fold_notes";
/// `view.fold_scale`: fold the note editor to the rows of the clip's scale
/// and those that hold notes (G, WP-6.7b).
pub const VIEW_FOLD_SCALE: &str = "view.fold_scale";
/// `view.highlight_scales`: switch Highlight Scales (K, WP-6.7b).
pub const VIEW_HIGHLIGHT_SCALES: &str = "view.highlight_scales";
/// `view.note_spelling`: choose how the note editor spells keys (WP-6.7b).
pub const VIEW_NOTE_SPELLING: &str = "view.note_spelling";
/// `view.lane`: choose which value the Clip View's lane shows (WP-2.3).
pub const VIEW_LANE: &str = "view.lane";
/// `view.lanes`: show, hide and swap the Clip View's expression lanes, and
/// set Randomize's amounts (WP-6.1).
pub const VIEW_LANES: &str = "view.lanes";
/// `view.mpe_lanes`: show, hide, swap and resize the MPE tab's lanes
/// (WP-6.18).
pub const VIEW_MPE_LANES: &str = "view.mpe_lanes";
/// `view.mpe_grid`: switch the MPE tab's own grid for a clip (WP-6.18).
pub const VIEW_MPE_GRID: &str = "view.mpe_grid";
/// `view.clip_tab`: show a tab of the Clip View, or go on to the next
/// (S-CLIPMODE-001 to 005, WP-3.16, WP-6.18).
pub const VIEW_CLIP_TAB: &str = "view.clip_tab";
/// `view.clip_envelope`: choose the parameter whose envelope the Clip View's
/// Envelopes tab shows (WP-3.16).
pub const VIEW_CLIP_ENVELOPE: &str = "view.clip_envelope";
/// `view.toggle_info_view`: show or hide the Info View.
pub const VIEW_TOGGLE_INFO_VIEW: &str = "view.toggle_info_view";
/// `edit.edit_info_text`: turn the Info View into a text field for the
/// selected item's info text (Edit ▸ Edit Info Text).
pub const EDIT_INFO_TEXT: &str = "edit.edit_info_text";
/// `view.toggle_plug_in_windows`: show or hide the plug-in windows: the
/// floating devices' windows and the plug-ins' editor windows (View ▸
/// Plug-In Windows, WP-2.5b).
pub const VIEW_TOGGLE_PLUG_IN_WINDOWS: &str = "view.toggle_plug_in_windows";
/// `view.close_plug_in_windows`: close every open plug-in editor window
/// (View ▸ Close All Plug-in Windows, IMP-25).
pub const CLOSE_PLUG_IN_WINDOWS: &str = "view.close_plug_in_windows";
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
/// `view.toggle_undo_history`: show or hide the Undo History panel.
pub const VIEW_TOGGLE_UNDO_HISTORY: &str = "view.toggle_undo_history";
/// `view.toggle_save_history`: show or hide the Save History panel (IMP-05).
pub const VIEW_TOGGLE_SAVE_HISTORY: &str = "view.toggle_save_history";
/// `focus.control_bar`: move the keyboard focus to the Control Bar.
pub const FOCUS_CONTROL_BAR: &str = "focus.control_bar";
/// `focus.tuning`: show the browser's Tuning section and move the keyboard
/// focus to it (WP-6.10; the tunings spec, R28).
pub const FOCUS_TUNING: &str = "focus.tuning";
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
/// `settings.set_permanent_scrub_areas`: switch Permanent Scrub Areas
/// (WP-6.59; the time editing spec, R26).
pub const SETTINGS_SET_PERMANENT_SCRUB_AREAS: &str = "settings.set_permanent_scrub_areas";
/// `settings.next_page`: show the next Settings page.
pub const SETTINGS_NEXT_PAGE: &str = "settings.next_page";
/// `settings.previous_page`: show the previous Settings page.
pub const SETTINGS_PREVIOUS_PAGE: &str = "settings.previous_page";
/// `settings.set_theme`: choose a theme.
pub const SETTINGS_SET_THEME: &str = "settings.set_theme";
/// `settings.set_appearance`: draw light, dark or follow the system.
pub const SETTINGS_SET_APPEARANCE: &str = "settings.set_appearance";
/// `settings.set_tone`: tint the neutral colours.
pub const SETTINGS_SET_TONE: &str = "settings.set_tone";
/// `settings.set_high_contrast`: switch High Contrast.
pub const SETTINGS_SET_HIGH_CONTRAST: &str = "settings.set_high_contrast";
/// `settings.set_grid_line_intensity`: how strong the grid lines are.
pub const SETTINGS_SET_GRID_LINE_INTENSITY: &str = "settings.set_grid_line_intensity";
/// `settings.set_brightness`: how bright the backgrounds are.
pub const SETTINGS_SET_BRIGHTNESS: &str = "settings.set_brightness";
/// `settings.set_color_intensity`: how intense the interface's colours are.
pub const SETTINGS_SET_COLOR_INTENSITY: &str = "settings.set_color_intensity";
/// `settings.set_color_hue`: how far the interface's colours are turned.
pub const SETTINGS_SET_COLOR_HUE: &str = "settings.set_color_hue";
/// `settings.set_auto_assign_track_colours`: switch Auto-Assign Track Colors.
pub const SETTINGS_SET_AUTO_ASSIGN_TRACK_COLOURS: &str = "settings.set_auto_assign_track_colours";
/// `settings.set_default_track_colour`: choose Default Track Color.
pub const SETTINGS_SET_DEFAULT_TRACK_COLOUR: &str = "settings.set_default_track_colour";
/// `settings.set_reduced_automatic_colours`: switch Reduced Automatic Colors.
pub const SETTINGS_SET_REDUCED_AUTOMATIC_COLOURS: &str = "settings.set_reduced_automatic_colours";
/// `settings.set_clip_colour`: choose Clip Color.
pub const SETTINGS_SET_CLIP_COLOUR: &str = "settings.set_clip_colour";
/// `settings.set_create_fades_on_clip_edges`: switch Create Fades on Clip
/// Edges.
pub const SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES: &str = "settings.set_create_fades_on_clip_edges";
/// `settings.set_retune_on_tuning_load`: switch Retune Set On Loading Tuning
/// Systems (WP-6.9; the tunings spec, R15).
pub const SETTINGS_SET_RETUNE_ON_TUNING_LOAD: &str = "settings.set_retune_on_tuning_load";
/// `settings.set_snap_automation_to_grid`: switch Snap Automation to Grid
/// (WP-6.57; the automation spec, R31).
pub const SETTINGS_SET_SNAP_AUTOMATION_TO_GRID: &str = "settings.set_snap_automation_to_grid";
/// `settings.set_temporary_folder`: choose the Temporary Folder on the File &
/// Folder page (WP-4.20).
pub const SETTINGS_SET_TEMPORARY_FOLDER: &str = "settings.set_temporary_folder";
/// `settings.set_sample_editor`: choose the sample editor on the File &
/// Folder page (WP-4.10b).
pub const SETTINGS_SET_SAMPLE_EDITOR: &str = "settings.set_sample_editor";
/// `settings.set_warp_recorded_audio`: switch Warp recorded audio (WP-2.13).
pub const SETTINGS_SET_WARP_RECORDED_AUDIO: &str = "settings.set_warp_recorded_audio";
/// `settings.set_loop_warp_short_samples`: choose Loop/Warp Short Samples
/// (WP-4.9).
pub const SETTINGS_SET_LOOP_WARP_SHORT_SAMPLES: &str = "settings.set_loop_warp_short_samples";
/// `settings.set_auto_warp_long_samples`: switch Auto-Warp Long Samples
/// (WP-4.9).
pub const SETTINGS_SET_AUTO_WARP_LONG_SAMPLES: &str = "settings.set_auto_warp_long_samples";
/// `settings.set_default_warp_mode`: choose Default Warp Mode (WP-4.9).
pub const SETTINGS_SET_DEFAULT_WARP_MODE: &str = "settings.set_default_warp_mode";
/// `settings.set_default_launch_mode`: choose Default Launch Mode (WP-3.4).
pub const SETTINGS_SET_DEFAULT_LAUNCH_MODE: &str = "settings.set_default_launch_mode";
/// `settings.set_default_launch_quantization`: choose Default Launch
/// Quantization (WP-3.4).
pub const SETTINGS_SET_DEFAULT_LAUNCH_QUANTIZATION: &str =
    "settings.set_default_launch_quantization";
/// `settings.set_select_on_launch`: switch Select on Launch (WP-3.4).
pub const SETTINGS_SET_SELECT_ON_LAUNCH: &str = "settings.set_select_on_launch";
/// `settings.set_select_next_scene_on_launch`: switch Select Next Scene on
/// Launch (WP-3.4).
pub const SETTINGS_SET_SELECT_NEXT_SCENE_ON_LAUNCH: &str =
    "settings.set_select_next_scene_on_launch";
/// `settings.set_start_recording_on_scene_launch`: switch Start Recording on
/// Scene Launch (WP-3.4).
pub const SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH: &str =
    "settings.set_start_recording_on_scene_launch";
/// `settings.set_record_session_automation_in`: choose which tracks Session
/// recordings write automation for (WP-3.12).
pub const SETTINGS_SET_RECORD_SESSION_AUTOMATION_IN: &str =
    "settings.set_record_session_automation_in";
/// `settings.set_automation_record_mode`: choose the mode an Arrangement
/// recording writes control moves in (WP-6.46).
pub const SETTINGS_SET_AUTOMATION_RECORD_MODE: &str = "settings.set_automation_record_mode";
/// `settings.set_auto_hide_plug_in_windows`: switch Auto-Hide Plug-In
/// Windows (WP-2.5b).
pub const SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS: &str = "settings.set_auto_hide_plug_in_windows";
/// `settings.set_collect_files_on_export`: choose Collect Files on Export
/// (WP-6.34).
pub const SETTINGS_SET_COLLECT_FILES_ON_EXPORT: &str = "settings.set_collect_files_on_export";
/// `settings.set_user_library`: choose the User Library's folder (WP-6.34).
pub const SETTINGS_SET_USER_LIBRARY: &str = "settings.set_user_library";
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
/// `keymap.bind`: give a command other keys.
pub use crate::keymap::BIND as KEYMAP_BIND;
/// `keymap.reset`: give a command Live's keys back.
pub use crate::keymap::RESET as KEYMAP_RESET;
/// `keymap.state`: the keymap in use.
pub use crate::keymap::STATE as KEYMAP_STATE;
/// `keymap.unbind`: take a shortcut's keys away.
pub use crate::keymap::UNBIND as KEYMAP_UNBIND;

/// Every command [`register`] adds apart from the agent server's and the
/// built-in assistant's ([`crate::agent::IDS`], [`crate::assistant::IDS`]),
/// so a host knows which IDs to send to `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SET_NEW,
    SET_OPEN,
    SET_SAVE,
    SET_SAVE_AS,
    SET_DISCARD_CHANGES,
    SET_LATER_RECOVERY,
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
    VIEW_TOGGLE_SECOND_WINDOW,
    VIEW_TOGGLE_BROWSER,
    VIEW_TOGGLE_FULL_HEIGHT_BROWSER,
    VIEW_TUNING_SECTION,
    VIEW_TOGGLE_CLIP_VIEW,
    VIEW_TOGGLE_DEVICE_VIEW,
    VIEW_SHOW_DETAIL_VIEWS,
    VIEW_SWITCH_DETAIL_VIEW,
    VIEW_TOGGLE_DETAIL_AREA,
    VIEW_EXPAND_CLIP_VIEW,
    VIEW_SET_DETAIL_HEIGHT,
    VIEW_SET_BROWSER_WIDTH,
    FOCUS_CLIP_VIEW,
    FOCUS_DEVICE_VIEW,
    VIEW_TOGGLE_PLUGIN_LIST,
    VIEW_SET_PLUGIN_LIST_ONLY_MISSING,
    VIEW_DRAW_MODE,
    VIEW_FOLD_NOTES,
    VIEW_FOLD_SCALE,
    VIEW_HIGHLIGHT_SCALES,
    VIEW_NOTE_SPELLING,
    VIEW_LANE,
    VIEW_LANES,
    VIEW_MPE_LANES,
    VIEW_MPE_GRID,
    VIEW_CLIP_TAB,
    VIEW_CLIP_ENVELOPE,
    VIEW_TOGGLE_INFO_VIEW,
    EDIT_INFO_TEXT,
    VIEW_TOGGLE_PLUG_IN_WINDOWS,
    CLOSE_PLUG_IN_WINDOWS,
    VIEW_TOGGLE_SESSION_ARRANGEMENT,
    VIEW_ZOOM_IN,
    VIEW_ZOOM_OUT,
    VIEW_ZOOM_RESET,
    VIEW_SET_ZOOM,
    VIEW_TOGGLE_UNDO_HISTORY,
    VIEW_TOGGLE_SAVE_HISTORY,
    FOCUS_CONTROL_BAR,
    FOCUS_TUNING,
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
    SETTINGS_SET_APPEARANCE,
    SETTINGS_SET_TONE,
    SETTINGS_SET_HIGH_CONTRAST,
    SETTINGS_SET_GRID_LINE_INTENSITY,
    SETTINGS_SET_BRIGHTNESS,
    SETTINGS_SET_COLOR_INTENSITY,
    SETTINGS_SET_COLOR_HUE,
    SETTINGS_SET_AUTO_ASSIGN_TRACK_COLOURS,
    SETTINGS_SET_DEFAULT_TRACK_COLOUR,
    SETTINGS_SET_REDUCED_AUTOMATIC_COLOURS,
    SETTINGS_SET_CLIP_COLOUR,
    SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
    SETTINGS_SET_RETUNE_ON_TUNING_LOAD,
    SETTINGS_SET_SNAP_AUTOMATION_TO_GRID,
    SETTINGS_SET_TEMPORARY_FOLDER,
    SETTINGS_SET_SAMPLE_EDITOR,
    SETTINGS_SET_WARP_RECORDED_AUDIO,
    SETTINGS_SET_LOOP_WARP_SHORT_SAMPLES,
    SETTINGS_SET_AUTO_WARP_LONG_SAMPLES,
    SETTINGS_SET_DEFAULT_WARP_MODE,
    SETTINGS_SET_DEFAULT_LAUNCH_MODE,
    SETTINGS_SET_DEFAULT_LAUNCH_QUANTIZATION,
    SETTINGS_SET_SELECT_ON_LAUNCH,
    SETTINGS_SET_SELECT_NEXT_SCENE_ON_LAUNCH,
    SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH,
    SETTINGS_SET_RECORD_SESSION_AUTOMATION_IN,
    SETTINGS_SET_AUTOMATION_RECORD_MODE,
    SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS,
    SETTINGS_SET_OUTLINE_VIEW_IN_FOCUS,
    SETTINGS_SET_PERMANENT_SCRUB_AREAS,
    SETTINGS_SET_COLLECT_FILES_ON_EXPORT,
    SETTINGS_SET_USER_LIBRARY,
    AUDIO_DEVICES,
    AUDIO_STATUS,
    AUDIO_CONFIGURE,
    AUDIO_TOGGLE_ENGINE,
    AUDIO_SET_TEST_TONE,
    KEYMAP_STATE,
    KEYMAP_BIND,
    KEYMAP_UNBIND,
    KEYMAP_RESET,
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

/// Arguments of `set.relink_missing`: where to search for the set's missing
/// files (file-management R14).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelinkArgs {
    /// Folders to search, each with its subfolders, absolute or relative to
    /// the app's working directory. May be empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    /// Whether the set's project folder is searched. On unless asked
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<bool>,
    /// Whether the User Library is searched. On unless asked otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<bool>,
    /// The folder the Automatic Search's Search Folder names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// Arguments of `set.collect_all_and_save`: the groups of external files to
/// collect (file-management R15).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectArgs {
    /// The groups to collect from: "other-project", "user-library" or
    /// "elsewhere". All of them when none are given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<crate::file_manager::ExternalGroup>,
    /// The project folder to collect for, when it isn't the open set's:
    /// every set in it is rewritten, each keeping the version it replaces
    /// in its project's `Backup` folder (R16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// What `set.collect_all_and_save` and `set.relink_missing` return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileReport {
    /// The set's file, if it has one.
    pub path: Option<String>,
    /// The number of files the command copied (Collect All and Save) or
    /// relinked (automatic search).
    pub files: usize,
    /// The files the search found for each missing file, when there was more
    /// than one candidate (R14).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<crate::file_manager::Candidate>,
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
    /// A theme's name, as `settings.state`'s `theme_colors.themes` lists
    /// it. `dark` and `light`, which earlier versions took, set the
    /// Appearance instead.
    pub theme: String,
}

/// Arguments of `settings.set_appearance`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppearanceArgs {
    /// `light`, `dark` or `follow-system`.
    pub appearance: Appearance,
}

/// Arguments of `settings.set_tone`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToneArgs {
    /// `neutral`, `warm` or `cool`.
    pub tone: Tone,
}

/// Arguments of `settings.set_grid_line_intensity` and
/// `settings.set_color_intensity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PercentArgs {
    /// A whole percentage from 0 to 200, in steps of 5.
    #[schemars(range(min = 0, max = 200))]
    pub percent: i16,
}

/// Arguments of `settings.set_default_track_colour`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwatchArgs {
    /// A swatch of the palette of 5 rows of 14, from 0 to 69.
    #[schemars(range(min = 0, max = 69))]
    pub colour: u8,
}

/// Arguments of `settings.set_clip_colour`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipColourArgs {
    /// `track-color` or `random`.
    pub clip_colour: ClipColour,
}

/// Arguments of `settings.set_brightness`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrightnessArgs {
    /// Points of lightness added to the backgrounds, from -50 to 50.
    #[schemars(range(min = -50, max = 50))]
    pub value: i16,
}

/// Arguments of `settings.set_color_hue`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HueArgs {
    /// Degrees the interface's colours are turned, from -180 to 180.
    #[schemars(range(min = -180, max = 180))]
    pub degrees: i16,
}

/// Arguments of `settings.set_loop_warp_short_samples`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoopWarpShortSamplesArgs {
    /// What a new clip of a short sample gets.
    pub mode: crate::settings::LoopWarpShortSamples,
}

/// Arguments of `settings.set_default_warp_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefaultWarpModeArgs {
    /// The warp mode a new warped clip takes.
    pub mode: crate::settings::DefaultWarpMode,
}

/// Arguments of `settings.set_default_launch_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefaultLaunchModeArgs {
    /// The launch mode new clips start with.
    pub mode: LaunchMode,
}

/// Arguments of `settings.set_record_session_automation_in`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordSessionAutomationInArgs {
    /// `armed_tracks` or `all_tracks`.
    pub tracks: crate::settings::SessionAutomationTracks,
}

/// Arguments of `settings.set_automation_record_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationRecordModeArgs {
    /// `touch`, `latch` or `write`.
    pub mode: crate::settings::AutomationRecordMode,
}

/// Arguments of `settings.set_default_launch_quantization`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefaultLaunchQuantizationArgs {
    /// The quantisation new clips start with; `null` (or nothing) follows
    /// the global one, Live's "Global".
    #[serde(default)]
    pub quantization: Option<LaunchQuantization>,
}

/// Arguments of `settings.set_collect_files_on_export`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectFilesArgs {
    /// `always`, `never` or `ask`.
    pub mode: crate::settings::CollectFiles,
}

/// Arguments of `settings.set_user_library`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserLibraryArgs {
    /// The folder's absolute path, or `null` for the default: a `User
    /// Library` folder in a Beatlayer folder in the Music folder (the
    /// Documents folder on Windows).
    pub path: Option<String>,
}

/// What `settings.set_user_library` returns: the User Library in use
/// afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserLibraryResult {
    /// The User Library's path; empty when there is none.
    pub path: String,
}

/// Arguments of `settings.set_temporary_folder`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemporaryFolderArgs {
    /// The folder's absolute path, or `null` for the default: a `Temporary`
    /// folder in Beatlayer's application-data folder.
    pub path: Option<String>,
}

/// What `settings.set_temporary_folder` returns: the folder in use afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemporaryFolderResult {
    /// The Temporary Folder's path.
    pub path: String,
}

/// Arguments of `settings.set_sample_editor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleEditorArgs {
    /// The editor's command: the program, its arguments, and the clip's
    /// file as the last one. `null` for none, when Edit opens the File &
    /// Folder page instead.
    pub path: Option<String>,
}

/// What `settings.set_sample_editor` returns: the editor in use afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SampleEditorResult {
    /// The sample editor's path, empty when none is named.
    pub path: String,
}

/// Arguments of a command that switches a setting on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchArgs {
    /// Whether the setting is on.
    pub on: bool,
}

/// One of the two main windows (second-window R15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowName {
    /// The main window.
    Main,
    /// The second window.
    Second,
}

impl WindowName {
    /// The window's ID in command results.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Second => "second",
        }
    }
}

/// Whether `id` is a command that takes `window`: the view, browser,
/// detail-view and zoom commands (second-window R15).
#[must_use]
pub fn acts_on_a_window(id: &str) -> bool {
    matches!(
        id,
        VIEW_FULL_SCREEN
            | VIEW_TOGGLE_BROWSER
            | VIEW_TOGGLE_CLIP_VIEW
            | VIEW_TOGGLE_DEVICE_VIEW
            | VIEW_SHOW_DETAIL_VIEWS
            | VIEW_SWITCH_DETAIL_VIEW
            | VIEW_TOGGLE_DETAIL_AREA
            | VIEW_EXPAND_CLIP_VIEW
            | VIEW_SET_DETAIL_HEIGHT
            | FOCUS_CLIP_VIEW
            | FOCUS_DEVICE_VIEW
            | VIEW_ZOOM_IN
            | VIEW_ZOOM_OUT
            | VIEW_ZOOM_RESET
            | VIEW_SET_ZOOM
    )
}

/// Arguments of the commands that act on one window (second-window R15).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowArgs {
    /// The window to act on, `main` or `second`. Left out, the window with
    /// the focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowName>,
}

/// Arguments of `view.set_zoom`: the Display & Input page's Zoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZoomArgs {
    /// The interface zoom as a whole percentage, from 50 to 200; 100 is
    /// the size layout.md measures Live at.
    #[schemars(range(min = 50, max = 200))]
    pub percent: u16,
    /// The window to zoom, `main` or `second`; `second` while the second
    /// window is closed sets the zoom it opens at. Left out, the window
    /// with the focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowName>,
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

/// What `set.later_recovery` returns: the recovery prompt's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoveryPrompt {
    /// Whether the prompt is still open.
    pub open: bool,
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

/// Arguments of `view.highlight_scales`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HighlightScalesArgs {
    /// Whether Highlight Scales is on. Left out, the switch flips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `view.note_spelling`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteSpellingArgs {
    /// How the note editor spells keys.
    pub spelling: bl_model::NoteSpelling,
}

/// Arguments of `view.lane`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneArgs {
    /// Which value the lane shows.
    pub lane: NoteField,
}

/// Arguments of `view.lanes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanesArgs {
    /// The lane the other fields but `all` act on: `velocity`, `chance` or
    /// `release_velocity` (`velocity_deviation` is the Velocity lane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<NoteField>,
    /// With `lane`: show it (`true`) or hide it (`false`); left out, the Lane
    /// Selector's choice, which switches it. Showing a lane makes it the
    /// active lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<bool>,
    /// With `lane`: hide it and show in its place the first hidden lane
    /// after it, going round to the first (the lane header's Swap Lane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap: Option<bool>,
    /// With `lane`: set its Randomize amount, 0 to 127, or 0 to 100 (a
    /// percentage) for `chance`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u8>,
    /// The Show/Hide All Expression Editors switch: `false` hides every lane
    /// and `true` shows them again as they were; left out, nothing changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
}

/// Arguments of `view.mpe_lanes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MpeLanesArgs {
    /// The lane the `show`, `swap` and `height` fields act on: `slide`,
    /// `pressure`, `velocity` or `release_velocity`; `pitch_bend` too for
    /// `height`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<bl_model::MpeLane>,
    /// With `lane`: switch its toggle on (`true`) or off (`false`); left
    /// out, it switches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<bool>,
    /// With `lane`: switch it off and the first lane after it that is off
    /// on, going round (the lane header's swap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap: Option<bool>,
    /// With `lane`: its height in points, 8 to 400.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    /// The triangle: `false` hides the lanes and their toggles, `true` shows
    /// them again. Left out, nothing changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// `true` switches every toggle on and shows the lanes, as the triangle
    /// clicked with Option does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
}

/// Arguments of `view.mpe_grid`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MpeGridArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<bl_model::ClipId>,
    /// Switch the grid on or off; left out, it switches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `view.set_plugin_list_only_missing`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginListOnlyMissingArgs {
    /// Show only the plug-ins this computer doesn't have (true) or all of
    /// them (false); switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_missing: Option<bool>,
}

/// Arguments of `view.clip_tab`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipTabArgs {
    /// The tab to show. Left out, the Clip View goes on to the next one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<ClipTab>,
}

/// Arguments of `view.clip_envelope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipEnvelopeArgs {
    /// The parameter of the shown clip's track whose envelope the Envelopes
    /// tab shows, or `null` to show none.
    pub target: Option<bl_model::AutomationTarget>,
}

/// What a command that switches something returns: its new state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwitchResult {
    /// Whether it's now on.
    pub on: bool,
}

/// What `view.close_plug_in_windows` returns: how many editor windows it
/// closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClosedPlugInWindows {
    /// The editor windows it closed.
    pub closed: usize,
}

/// The Device View's height in the detail area, in points at 100%
/// (detail-views R7).
pub const DEVICE_VIEW_HEIGHT: f32 = 188.0;
/// The Clip View's height until it's resized, in points at 100%
/// (detail-views R7).
pub const CLIP_VIEW_HEIGHT: f32 = 225.0;
/// The Clip View's smallest height, in points at 100% (detail-views R10).
pub const CLIP_VIEW_MIN_HEIGHT: f32 = 100.0;
/// The least height the main view keeps, in points at 100% (detail-views
/// R10).
pub const MAIN_VIEW_MIN_HEIGHT: f32 = 80.0;
/// The gap between the Clip View and the Device View stacked, and between
/// the main view and the detail area, in points at 100% (detail-views R8).
pub const DETAIL_GAP: f32 = 8.0;
/// The browser's width until its right edge is dragged, in points at 100%
/// (L-WIN-004; browser R29).
pub const BROWSER_WIDTH: f32 = 302.0;
/// The browser's narrowest width, in points at 100%: its content pane at
/// its narrowest beside the narrowest sidebar (browser R29).
pub const BROWSER_MIN_WIDTH: f32 = 70.0;
/// The widest width `view.set_browser_width` keeps, which no window shows.
pub const BROWSER_MAX_WIDTH: f32 = 4000.0;
/// The least width the main view keeps beside a widened browser, in points
/// at 100% (browser R29).
pub const MAIN_VIEW_MIN_WIDTH: f32 = 160.0;

/// `points` brought within the browser's narrowest and widest widths; a
/// width that isn't a number is the default.
#[must_use]
pub fn clamp_browser_width(points: f32) -> f32 {
    if points.is_finite() {
        points.clamp(BROWSER_MIN_WIDTH, BROWSER_MAX_WIDTH)
    } else {
        BROWSER_WIDTH
    }
}

/// One of the detail area's two views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DetailView {
    /// The Clip View, which edits the selected clip.
    Clip,
    /// The Device View, which shows the selected track's devices.
    Device,
}

/// What the detail area's commands return, and `view.state` reports under
/// `detail` (detail-views R17): which views show, where the focus is and
/// how tall the Clip View is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DetailState {
    /// Whether the Clip View shows.
    pub clip_view: bool,
    /// Whether the Device View shows.
    pub device_view: bool,
    /// The view that showed alone last, which `view.switch_detail_view`
    /// brings back when neither shows.
    pub alone_last: DetailView,
    /// The views `view.toggle_detail_area` hid, which it brings back; empty
    /// when it hid nothing.
    pub hidden: Vec<DetailView>,
    /// The Clip View's height in points at 100%, as it was set; it is
    /// squeezed in a window too short for it.
    pub clip_height: f64,
    /// Whether the Clip View is expanded to its largest height.
    pub expanded: bool,
    /// The view the detail area's keys go to while both show.
    pub focus: DetailView,
}

/// Arguments of `view.set_detail_height`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetailHeightArgs {
    /// The Clip View's height in points at 100%, at least 100.
    pub points: f64,
    /// The window whose detail area it sets, `main` or `second`. Left out,
    /// the window with the focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowName>,
}

/// Arguments of `view.set_browser_width`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserWidthArgs {
    /// The browser's width in points at 100%, from 70 to 4000.
    pub points: f64,
}

/// What the view commands return: the window's view state afterwards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewResult {
    /// The main view: `arrangement` or `session`.
    pub main_view: String,
    /// Whether the browser shows.
    pub browser: bool,
    /// Whether the browser runs down to the Status Bar (Full-Height
    /// Browser, spec: browser R26).
    #[serde(default)]
    pub full_height_browser: bool,
    /// Whether the browser's Tuning section shows (the tunings spec, R28);
    /// hidden in a new set.
    #[serde(default)]
    pub tuning_section: bool,
    /// The browser's width in points at 100%, as it was set; a window too
    /// narrow for it shows the browser narrower (browser R29).
    #[serde(default = "default_browser_width")]
    pub browser_width: f32,
    /// Whether the Info View shows.
    pub info_view: bool,
    /// Whether the main window is in full-screen mode.
    pub full_screen: bool,
    /// The interface zoom, where 1 is 100%.
    pub zoom: f32,
}

fn default_browser_width() -> f32 {
    BROWSER_WIDTH
}

/// What `view.toggle_second_window` returns and `view.state` reports under
/// `second_window` (second-window R15).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SecondWindowState {
    /// Whether the second window is open.
    pub open: bool,
    /// The main view it shows, or would show: the one the main window
    /// doesn't.
    pub main_view: String,
    /// Whether its browser shows; it opens without.
    pub browser: bool,
    /// Whether it is in full-screen mode.
    pub full_screen: bool,
    /// Its interface zoom, where 1 is 100%, kept while it is closed.
    pub zoom: f32,
    /// Where it sat and how big it was when it was last open, in points;
    /// none before it has been opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<bl_model::WindowFrame>,
    /// Its detail area.
    pub detail: DetailState,
}

/// What `view.state` returns: what the view commands switch, without
/// switching any of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewState {
    /// The main window: the main view, the browser, the Info View,
    /// full-screen mode and the interface zoom.
    pub window: ViewResult,
    /// The second window: whether it is open, the main view it shows, its
    /// browser, zoom, frame and detail area.
    pub second_window: SecondWindowState,
    /// The window the commands that act on one window act on when they
    /// aren't told which: the one with the focus.
    pub focused_window: WindowName,
    /// The Clip View and its note editor: whether it shows, Draw Mode, the
    /// clip it holds, the selected notes, the lane and Fold.
    pub clip_view: ClipViewState,
    /// The detail area: which of the Clip View and the Device View show, the
    /// Clip View's height, and where the focus is.
    pub detail: DetailState,
    /// The list of the set's plug-ins: whether it shows, and whether Show
    /// Only Missing is on.
    pub plugin_list: crate::plugins::PluginListState,
    /// The assistant's panel.
    pub assistant: crate::assistant::AssistantPanelState,
    /// Whether the Undo History panel shows.
    pub undo_history: bool,
    /// Whether the Save History panel shows.
    pub save_history: bool,
    /// Whether floating devices' and plug-ins' windows are shown, as
    /// `view.toggle_plug_in_windows` sets it.
    pub plug_in_windows: bool,
}

/// What `view.toggle_undo_history` returns: the Undo History panel's state
/// afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UndoHistoryState {
    /// Whether the panel shows.
    pub open: bool,
    /// How many steps the history holds.
    pub steps: usize,
    /// The current row's place in the history, counted up from the set's
    /// start: 0 before any step.
    pub current: usize,
}

/// What `view.toggle_save_history` returns: the Save History panel's state
/// afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SaveHistoryState {
    /// Whether the panel shows.
    pub open: bool,
    /// How many rows the history lists (`set.save_history` lists them).
    pub rows: usize,
    /// Whether Save Continuously is on.
    pub continuous: bool,
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
    crate::assistant::register(registry)?;
    crate::keymap::register(registry)
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
        CommandSpec::new(SET_LATER_RECOVERY, "Later Recovery")
            .description(
                "Closes the recovery prompt the app shows at launch, leaving every set it \
                 listed to be offered again at the next launch: nothing is recovered and \
                 nothing is thrown away. The prompt's Later button runs it, and Esc does the \
                 same. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<RecoveryPrompt>(),
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
                "Copies the files the open set uses that live outside its project folder into \
                 the project's `Samples/Imported` folder, re-points the set's media table at \
                 the copies, and saves the set, as File ▸ Collect All and Save does in Live, so \
                 the set no longer depends on files elsewhere. `groups` chooses which places to \
                 collect from: \"other-project\", \"user-library\" and \"elsewhere\"; all of them \
                 when none are given. Fails if the set has never been saved; use `set.save_as` \
                 to give it a project folder first. The result's `files` is how many files were \
                 copied.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<CollectArgs>()
            .result::<FileReport>(),
        CommandSpec::new(SET_RELINK_MISSING, "Relink Missing Files")
            .description(
                "Searches for the files the open set uses that are missing or changed, and \
                 relinks each one to the file found with the same content, as Live's automatic \
                 search does. The search covers the set's project folder, the User Library, the \
                 folders the files were last in, and any `folders` given, each with its \
                 subfolders; `project` and `library` switch the first two off, and `folder` \
                 names the folder the Automatic Search's Search Folder chooses. A file with one \
                 candidate is relinked to it; a file with several keeps its row, and the \
                 candidates are in the result's `candidates`, so the person chooses. The result's \
                 `files` is how many files were relinked. It also runs by itself when a set is \
                 opened.",
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
                 the set. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-001")
            .args::<WindowArgs>()
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
        CommandSpec::new(CLOSE_PLUG_IN_WINDOWS, "Close All Plug-in Windows")
            .description(
                "Closes every open plug-in editor window at once, as View ▸ Close All Plug-in \
                 Windows does. It closes editors only: floating native device windows stay as \
                 they are, so it changes nothing in the set, isn't an undo step, and is in the \
                 read class. It has no key. Returns how many editor windows it closed.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<ClosedPlugInWindows>(),
        CommandSpec::new(VIEW_TOGGLE_SECOND_WINDOW, "Second Window")
            .description(
                "Opens the second window, or closes it if it is open: a window with its own \
                 Control Bar, main view, detail area and Status Bar, showing the main view the \
                 main window doesn't (the Arrangement View while the main window shows the \
                 Session View), without the browser. It opens at 990 by 703 points the first \
                 time and afterwards where and as big as it was. Both windows show one set. \
                 Closing it moves its Clip and Device Views into the main window. Whether it \
                 is open is saved with the set, but isn't an undo step and doesn't change the \
                 set. The result is the second window's state, as `view.state` reports it \
                 under `second_window`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-002")
            .args::<NoArgs>()
            .result::<SecondWindowState>(),
        CommandSpec::new(VIEW_TOGGLE_BROWSER, "Browser")
            .description(
                "Shows or hides the browser at the left of the main window; the main view \
                 widens to fill its place. Changes nothing in the set. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-010")
            .binding("S-BRW-002")
            .args::<WindowArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_TOGGLE_FULL_HEIGHT_BROWSER, "Full-Height Browser")
            .description(
                "Runs the browser column from under the Control Bar down to the Status Bar, \
                 instead of to the detail area's top; the Info View and the Clip or Device View \
                 stay open, under the main view to the browser's right. Calling it again puts \
                 the column back. Kept for the next start. Read in `view.state`'s \
                 `window.full_height_browser`. Changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_TUNING_SECTION, "Tuning")
            .description(
                "Shows or hides the Tuning section at the bottom of the browser's content pane, which \
                 shows the set's tuning: its name (or that the set plays twelve equal steps), \
                 and under its triangle Octave, Note, Ref. Pitch, Lowest Note and Highest Note, \
                 then the save and link buttons. It is hidden in a new set, and loading a \
                 tuning from the browser shows it. Read in `view.state`'s \
                 `window.tuning_section`. Changes nothing in the set and isn't an undo step.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_SET_BROWSER_WIDTH, "Set Browser Width")
            .description(
                "Sets the browser's width to `points`, in points at 100% zoom, as dragging \
                 the browser's right edge does one frame at a time; a width outside 70 to \
                 4000 points is brought within them, and a window too narrow for it shows the \
                 browser narrower, leaving the main view 160 points. Both windows' browsers \
                 take it, and it is kept for the next start. Read in `view.state`'s \
                 `window.browser_width`. Changes nothing in the set and isn't an undo step.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<BrowserWidthArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_STATE, "View State")
            .description(
                "Reports what the view commands switch: the main view (Arrangement or \
                 Session), whether the browser and the Info View show, full-screen mode and \
                 the zoom (`view.toggle_session_arrangement`, `view.toggle_browser`, \
                 `view.toggle_info_view`, `view.full_screen`, `view.set_zoom`), the Clip View \
                 with Draw Mode, the lane, Fold and the selected notes (`view.toggle_clip_view`, \
                 `view.draw_mode`, `view.lane`, `view.fold_notes`), the detail area under \
                 `detail`: which of the Clip View and the Device View show, the Clip View's \
                 height and where the focus is (`view.toggle_clip_view`, \
                 `view.toggle_device_view`, `view.show_detail_views`, \
                 `view.switch_detail_view`, `view.toggle_detail_area`, \
                 `view.expand_clip_view`, `view.set_detail_height`, `focus.clip_view`, \
                 `focus.device_view`), the assistant's panel, the \
                 Undo History (`view.toggle_undo_history`), the Save History \
                 (`view.toggle_save_history`), whether plug-in windows are \
                 shown (`view.toggle_plug_in_windows`), the second window under \
                 `second_window`: whether it is open, its main view, browser, full-screen \
                 mode, zoom, frame and detail area (`view.toggle_second_window`, and the \
                 commands above with `window` set to `second`), and `focused_window`, `main` \
                 or `second`, the window the commands that act on one window act on without \
                 `window`. Changes nothing. The Arrangement's own view is in `arrangement.state`, the Mixer's in \
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
                 Editor for the Arrangement's selected MIDI clip, and leaves the Device View \
                 as it is: with the Device View showing, showing the Clip View stacks it above \
                 the Device View. It shows the clip the Arrangement has selected, with its \
                 notes, and selecting another MIDI clip switches the editor to it, clearing \
                 the note selection. Changes nothing in the set and isn't an undo step. The \
                 result is the detail area's state, as `view.state` reports it under \
                 `detail`. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-015")
            .args::<WindowArgs>()
            .result::<DetailState>(),
        CommandSpec::new(VIEW_TOGGLE_DEVICE_VIEW, "Device View")
            .description(
                "Shows or hides the Device View in the detail area, which shows the selected \
                 track's devices, and leaves the Clip View as it is: with the Clip View \
                 showing, showing the Device View stacks it below the Clip View. Changes \
                 nothing in the set and isn't an undo step. The result is the detail area's \
                 state, as `view.state` reports it under `detail`. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-016")
            .args::<WindowArgs>()
            .result::<DetailState>(),
        CommandSpec::new(VIEW_SHOW_DETAIL_VIEWS, "Show Clip and Device View")
            .description(
                "Shows the Clip View and the Device View together, the Clip View above the \
                 Device View, as Option-click (Alt-click) on either view's toggle does. With \
                 both showing already it changes nothing. Changes nothing in the set and isn't \
                 an undo step. The result is the detail area's state. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-005")
            .args::<WindowArgs>()
            .result::<DetailState>(),
        CommandSpec::new(VIEW_SWITCH_DETAIL_VIEW, "Toggle Clip/Device View")
            .description(
                "With one of the Clip View and the Device View showing, shows the other in its \
                 place; with both showing, moves the keyboard focus to the Clip View's editor if \
                 the Device View has it, and to the Device View otherwise; with neither \
                 showing, shows the view that showed alone last, the Device View in a new \
                 session. Shift+Tab does this while Use Tab Key to Move Focus is off; F12 does \
                 it always. Changes nothing in the set and isn't an undo step. The result is \
                 the detail area's state. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-004")
            .args::<WindowArgs>()
            .result::<DetailState>(),
        CommandSpec::new(VIEW_TOGGLE_DETAIL_AREA, "Hide Device View")
            .description(
                "Hides whichever of the Clip View and the Device View show, and run again shows \
                 the same ones again; with neither showing and nothing hidden this way, shows \
                 the Device View. The View menu's item reads Hide or Show followed by the views \
                 it would hide or bring back. Changes nothing in the set and isn't an undo \
                 step. The result is the detail area's state. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-022")
            .args::<WindowArgs>()
            .result::<DetailState>(),
        CommandSpec::new(VIEW_EXPAND_CLIP_VIEW, "Expand Clip View")
            .description(
                "Gives the Clip View its largest height, the one that leaves the main view \
                 80 points, and run again gives back the height it had before. Refused while \
                 the Clip View is hidden. Dragging the detail area's top edge, or hiding the \
                 Clip View, ends the expansion. Changes nothing in the set and isn't an undo \
                 step. The result is the detail area's state, whose `expanded` says which way \
                 it went. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(ClipView)
            .binding("S-NOTE-030")
            .args::<WindowArgs>()
            .result::<DetailState>(),
        CommandSpec::new(VIEW_SET_DETAIL_HEIGHT, "Set Clip View Height")
            .description(
                "Sets the Clip View's height to `points`, in points at 100% zoom and at least \
                 100, as dragging the detail area's top edge does one frame at a time; a \
                 window too short for it shows the Clip View less tall, down to 100 points. \
                 Ends an expansion. The Device View's height doesn't change. Changes nothing \
                 in the set and isn't an undo step. The result is the detail area's state. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<DetailHeightArgs>()
            .result::<DetailState>(),
        CommandSpec::new(FOCUS_CLIP_VIEW, "Clip View")
            .description(
                "Moves the keyboard focus to the Clip View's editor, so the Clip View's \
                 shortcuts apply, showing the Clip View first if it is hidden. Changes nothing \
                 in the set and isn't an undo step. The result is the detail area's state. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-FOCUS-004")
            .args::<WindowArgs>()
            .result::<DetailState>(),
        CommandSpec::new(FOCUS_DEVICE_VIEW, "Device View")
            .description(
                "Moves the keyboard focus to the Device View, so its shortcuts apply, showing \
                 the Device View first if it is hidden. Changes nothing in the set and isn't \
                 an undo step. The result is the detail area's state. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-FOCUS-005")
            .args::<WindowArgs>()
            .result::<DetailState>(),
        CommandSpec::new(VIEW_TOGGLE_PLUGIN_LIST, "Plug-In List")
            .description(
                "Shows or hides the list of the set's plug-ins in the detail area's place, \
                 where the File Manager opens (L-OTH-002), in front of the Clip or Device \
                 View. The list holds every plug-in the set uses, with its format, vendor, \
                 version, the preset names its devices hold, the tracks it is on, and whether \
                 this computer has it. Whether it shows is view state: it isn't saved with \
                 the set and isn't an undo step. The result says whether it shows and \
                 whether Show Only Missing is on.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<crate::plugins::PluginListState>(),
        CommandSpec::new(
            VIEW_SET_PLUGIN_LIST_ONLY_MISSING,
            "Show Only Missing Plug-Ins",
        )
        .description(
            "Switches Show Only Missing in the list of the set's plug-ins: while it is on, \
                 only the plug-ins this computer doesn't have show. `only_missing` says which \
                 way; left out, it switches. Changes only the view: it isn't saved with the \
                 set and isn't an undo step. The result says whether it shows and whether Show \
                 Only Missing is on.",
        )
        .permission(Permission::Read)
        .context(Global)
        .args::<PluginListOnlyMissingArgs>()
        .result::<crate::plugins::PluginListState>(),
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
        CommandSpec::new(VIEW_FOLD_SCALE, "Fold to Scale")
            .description(
                "Folds or unfolds the MIDI Note Editor to the clip's scale (G, View \u{25b8} Fold \
                 to Scale, the toolbar's Scale button): while it's folded, the editor shows \
                 only the key rows of the clip's scale and the rows that hold notes, one \
                 under the next. With Fold to Notes on too, a row shows only if each fold \
                 that is on shows it. It works only while the clip's Scale Mode is on, and \
                 turning Scale Mode off shows every row until it is on again. `clip` names \
                 the clip; left out, the one the editor shows. `folded` says which way; left \
                 out, it switches. Folding changes nothing in the set and isn't an undo step: \
                 it's saved with the set beside the song. The result lists the clips folded \
                 to their scale in `scale_folds`.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor])
            .binding("S-NOTE-039")
            .args::<FoldNotesArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_HIGHLIGHT_SCALES, "Highlight Scales")
            .description(
                "Switches Highlight Scales (K, Options \u{25b8} Highlight Scales, the toolbar's \
                 Highlight Scale button): in every clip with Scale Mode on, the note area \
                 shades the rows of the scale and the piano ruler marks the root more \
                 strongly than the scale's other keys. `on` says which way; left out, it \
                 flips. It is one switch for all clips, off in a new install and kept between \
                 launches as a preference; it isn't part of the set and is never an undo \
                 step. The result reads it in `highlight_scales`.",
            )
            .permission(Permission::Edit)
            .contexts([Global, ClipView, NoteEditor])
            .binding("S-NOTE-035")
            .args::<HighlightScalesArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_NOTE_SPELLING, "Note Spelling")
            .description(
                "Chooses how the note editor spells keys (the piano ruler's context menu): \
                 `sharps`, `flats`, `sharps_and_flats`, `auto` (flats when the clip's scale \
                 has a root of F, B\u{266d}, E\u{266d}, A\u{266d}, D\u{266d} or G\u{266d}) or \
                 `note_numbers`. A seven-note scale that is on spells its own keys with one \
                 letter a degree, and the choice names the keys outside it. The rows, the \
                 Root Note choosers follow the choice. It is one choice for every editor, \
                 sharps in a new install and kept between launches as a preference; it isn't \
                 part of the set and is never an undo step. The result reads it in \
                 `note_spelling`.",
            )
            .permission(Permission::Edit)
            .contexts([Global, ClipView, NoteEditor])
            .args::<NoteSpellingArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_LANE, "Note Editor Lane")
            .description(
                "Makes one of the MIDI Note Editor's expression lanes the active lane, whose \
                 controls the footer shows, and shows it: `velocity` (the default), `chance` \
                 or `release_velocity`; `velocity_deviation` is the Velocity lane, which shows \
                 it. Choosing a lane in the Lane Selector does the same. Changes only the \
                 view, so it's no undo step and isn't saved with the set.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor])
            .args::<LaneArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_LANES, "Expression Lanes")
            .description(
                "Shows, hides and swaps the MIDI Note Editor's expression lanes (`velocity`, \
                 `chance`, `release_velocity`, always in that order from the top), and holds \
                 Randomize's amounts. With `lane`: `show` shows or hides it as the Lane \
                 Selector does (left out, it switches), `swap` hides it and shows in its \
                 place the first hidden lane after it, going round to the first (Swap Lane in \
                 the lane header's menu), and `amount` sets its Randomize amount, 0 to 127, or \
                 0 to 100 (a percentage) for `chance`. `all` is the Show/Hide All Expression \
                 Editors switch: `false` hides every lane and `true` shows them again as they \
                 were. A new editor shows the Velocity lane alone, with every amount 0. \
                 Changes only the view, so it's no undo step and isn't saved with the set. \
                 The result is the Clip View's state, which lists the lanes shown.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor])
            .args::<LanesArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_MPE_LANES, "MPE Lanes")
            .description(
                "Shows, hides, swaps and resizes the lanes of the Clip View's MPE tab \
                 (`slide`, `pressure`, `velocity`, `release_velocity`, always in that order \
                 from the top, with `pitch_bend` first on a track whose instrument is a Drum \
                 Rack). With `lane`: `show` switches its selector toggle (left out, it \
                 switches), `swap` switches it off and the first lane after it that is off \
                 on, and `height` sets its height in points. `visible` is the triangle: \
                 `false` hides the lanes and their toggles, `true` shows them again; `all` \
                 switches every toggle on and shows the lanes, as the triangle does with \
                 Option held or with every toggle off. A new set shows Slide and Pressure. \
                 Changes only the view: no undo step, and it is saved with the set. The \
                 result is the Clip View's state, whose `mpe` lists the lanes.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor])
            .args::<MpeLanesArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_MPE_GRID, "MPE Grid")
            .description(
                "Switches the MPE tab's own grid for a clip (Cmd+4, Ctrl+4 on Windows, while \
                 the tab has the focus). With it on, a breakpoint or segment being moved \
                 snaps to its neighbours' positions, and a new breakpoint near a grid line \
                 lands on the line. It is off in a new clip, saved with the set, and neither \
                 the Arrangement's grid nor an undo step. The result is the Clip View's \
                 state, whose `mpe.grid` says whether it is on.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor])
            .binding("S-GRID-005")
            .args::<MpeGridArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_CLIP_TAB, "Clip View Tab")
            .description(
                "Chooses the tab the Clip View shows for its clip: `notes` (the MIDI Note \
                 Editor, Option+Shift+1), `envelopes` (the clip's envelopes, \
                 Option+Shift+2) or `mpe` (the Expression Editor, for a MIDI clip only, \
                 Option+Shift+3). Left out, `tab` goes on to the next tab and round to the \
                 first after the last (Option+Tab). The Clip View shows the clip most \
                 recently selected, in the Arrangement or in a Session slot. Changes only \
                 the view, so it's no undo step and isn't saved with the set. The result \
                 says which tab shows.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor, SampleEditor, Envelope])
            .binding(Binding::new("S-CLIPMODE-001"))
            .binding(Binding::new("S-CLIPMODE-002"))
            .binding(Binding::new("S-CLIPMODE-003").arg("tab", "notes"))
            .binding(Binding::new("S-CLIPMODE-004").arg("tab", "envelopes"))
            .binding(Binding::new("S-CLIPMODE-005").arg("tab", "mpe"))
            .args::<ClipTabArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(VIEW_CLIP_ENVELOPE, "Clip View Envelope")
            .description(
                "Chooses the parameter whose envelope the Clip View's Envelopes tab shows \
                 and edits, as its device and control choosers do: `target` is a control of \
                 the shown clip's track's mixer, one of its sends, or a parameter of a \
                 device in its chain, named as `clip.set_envelope_breakpoint` names it; \
                 `null` shows none. Changes only the view, so it's no undo step. Editing \
                 the envelope is done by the `clip.*envelope*` commands. The result says \
                 which parameter shows.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, Envelope])
            .args::<ClipEnvelopeArgs>()
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
        CommandSpec::new(EDIT_INFO_TEXT, "Edit Info Text")
            .description(
                "Turns the Info View into a text field for the info text of the selected \
                 item, showing the Info View first if it is hidden: a locator, clip or track \
                 in the Arrangement, a scene, clip or track in the Session View, or a macro, \
                 chain or device in the Device View, whichever the area with the keyboard \
                 focus has selected. Return keeps the text, Option-Return (Alt-Return) starts \
                 a new line, and Esc drops the change. Changes nothing until the text is \
                 kept; to set it directly, run the item's own command with its text.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(
            VIEW_TOGGLE_SESSION_ARRANGEMENT,
            "Toggle Arrangement/Session View",
        )
        .description(
            "Switches the main view between the Arrangement View and the Session View. A new \
             set opens in the Arrangement View. With the second window open, the two windows \
             swap their main views. Which main view the main window shows is saved with the \
             set, but switching it isn't an undo step and doesn't change the set.",
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
                 the next start. Changes nothing in the set and isn't an undo step. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .binding("S-ZOOM-001")
            .args::<WindowArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_ZOOM_OUT, "Zoom Out")
            .description(
                "Makes the whole interface smaller by one step of 10%, down to 50%: every \
                 window's panes, controls, and text scale together and keep their places. \
                 From a zoom between steps it goes to the next step down. The zoom is kept for \
                 the next start. Changes nothing in the set and isn't an undo step. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .binding("S-ZOOM-002")
            .args::<WindowArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_ZOOM_RESET, "Reset Zoom to 100%")
            .description(
                "Returns the interface zoom to 100%, the size Live's layout is measured at. \
                 The zoom is kept for the next start. Changes nothing in the set and isn't an \
                 undo step. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<WindowArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_SET_ZOOM, "Zoom")
            .description(
                "Sets the interface zoom to `percent`, a whole percentage from 50 to 200, as \
                 the Zoom setting on the Settings window's Display & Input page does: every \
                 window's panes, controls, and text scale together and keep their places, and \
                 the main window grows if it's smaller than its minimum size at that zoom (as \
                 far as the screen allows). The result's `zoom` is the new zoom as a factor, \
                 1.5 for 150%. The zoom is kept for the next start. Changes nothing in the set \
                 and isn't an undo step. With the second window open it acts on the window `window` names, `main` or `second`, and on the window with the focus without it.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Settings])
            .args::<ZoomArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_TOGGLE_UNDO_HISTORY, "Undo History")
            .description(
                "Shows or hides the Undo History panel at the right of the main window, in the \
                 column the Learn View uses. The panel lists the set's steps, newest first, with \
                 the ones that can be redone greyed, and clicking a row returns the set to that \
                 point. Whether it shows is view state: it isn't saved with the set and isn't an \
                 undo step. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-021")
            .args::<NoArgs>()
            .result::<UndoHistoryState>(),
        CommandSpec::new(VIEW_TOGGLE_SAVE_HISTORY, "Save History")
            .description(
                "Shows or hides the Save History panel at the right of the main window, where \
                 the Undo History opens (showing one hides the other). The panel lists the open \
                 set's save points, then its saved versions, newest first; `set.save_history` \
                 lists the same rows and `set.restore` restores one. Whether it shows is view \
                 state: it isn't saved with the set and isn't an undo step. Changes nothing in \
                 the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SaveHistoryState>(),
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
        CommandSpec::new(FOCUS_TUNING, "Tuning")
            .description(
                "Shows the browser, and in it the Tuning section, if either is hidden, and \
                 moves the keyboard focus to the section's name, so Delete there removes the \
                 set's tuning. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(FOCUS_ARRANGEMENT, "Arrangement View")
            .description(
                "Shows the Arrangement View if the Session View is in front, and moves the \
                 keyboard focus to it, so its shortcuts apply. With the second window open, \
                 moves the focus to the window that shows the Arrangement View and brings it \
                 to the front, without changing either window's view. Changes nothing in the \
                 set.",
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
                "Chooses the theme every window draws, by its name: Beatlayer, or a theme from \
                 the User Library's Themes folder; `settings.state` lists them in \
                 `theme_colors.themes`. `dark` and `light`, which earlier versions took, set the \
                 Appearance instead. The choice is kept for the next start, and a theme that \
                 can't be read is drawn as Beatlayer. Changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<ThemeArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_APPEARANCE, "Appearance")
            .description(
                "Draws the theme in its light or dark colours, or follows the operating \
                 system's mode while Beatlayer runs (`follow-system`); a new install draws \
                 dark. Kept for the next start; changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<AppearanceArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_TONE, "Tone")
            .description(
                "Tints the theme's neutral colours warm or cool, or leaves them as the theme \
                 gives them (`neutral`, the default); the chromatic colours aren't toned. Kept \
                 for the next start; changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<ToneArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_HIGH_CONTRAST, "High Contrast")
            .description(
                "Switches High Contrast, off in a new install. On, text stands at a contrast \
                 ratio of at least 7:1 against its background, dimmed text at 4.5:1, and \
                 separators, outlines and the focus ring at 3:1, whatever the other Theme & \
                 Colors settings are. Kept for the next start; changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SwitchArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_GRID_LINE_INTENSITY, "Grid Line Intensity")
            .description(
                "Sets how strong the grid lines of the Arrangement, the Clip View's editors and \
                 the automation lanes are, from 0 to 200 percent in steps of 5; 100, the default, \
                 draws the theme's grid colour, 0 shows no line. Kept for the next start; \
                 changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<PercentArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_BRIGHTNESS, "Brightness")
            .description(
                "Adds points of lightness to every background colour, from -50 to 50; 0 is the \
                 default. Chromatic colours and the track palette don't change. Kept for the \
                 next start; changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<BrightnessArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_COLOR_INTENSITY, "Color Intensity")
            .description(
                "Multiplies the chroma of the interface's chromatic colours, from 0 to 200 \
                 percent in steps of 5; 100 is the default. Backgrounds and the track palette \
                 don't change. Kept for the next start; changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<PercentArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_AUTO_ASSIGN_TRACK_COLOURS, "Auto-Assign Track Colors")
            .description(
                "Switches Auto-Assign Track Colors, on in a new install. On, a new track takes \
                 swatch `id mod n` of the automatic colours, `id` being its ID; off, every new \
                 track takes Default Track Color. Kept for the next start; changes nothing in \
                 the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SwitchArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_DEFAULT_TRACK_COLOUR, "Default Track Color")
            .description(
                "Chooses the swatch (0 to 69, from the palette's top row's left, row by row) \
                 that every new track takes while Auto-Assign Track Colors is off; 12 in a new \
                 install. Kept for the next start; changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SwatchArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_REDUCED_AUTOMATIC_COLOURS, "Reduced Automatic Colors")
            .description(
                "Switches Reduced Automatic Colors, off in a new install. Off, Auto-Assign \
                 Track Colors cycles the palette's top row of 14 swatches; on, only its seven \
                 even-numbered swatches, 0, 2, 4 and so on to 12. Kept for the next start; \
                 changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SwitchArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_CLIP_COLOUR, "Clip Color")
            .description(
                "Chooses what colour a new clip takes: `track-color`, its track's, or `random`, \
                 a swatch from a hash of the clip's ID. `track-color` in a new install. Kept for \
                 the next start; changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<ClipColourArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_COLOR_HUE, "Color Hue")
            .description(
                "Turns the hue of the interface's chromatic colours by this many degrees, from \
                 -180 to 180 in steps of 5; 0 is the default. Backgrounds and the track palette \
                 don't change. Kept for the next start; changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<HueArgs>()
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
        // Spec: tunings R15 (M-OPT-022): an edit-class preference.
        CommandSpec::new(
            SETTINGS_SET_RETUNE_ON_TUNING_LOAD,
            "Retune Set On Loading Tuning Systems",
        )
        .description(
            "Switches Retune Set On Loading Tuning Systems, the Options menu's item; it's off \
             in a new install. With it off, loading, changing or removing a tuning leaves every \
             note on its key. With it on, `tuning.load` and `tuning.remove` called without \
             `retune` move every note of the MIDI clips on tracks that play the tuning to the \
             key whose new pitch is nearest the pitch it had, and a person who loads or removes \
             a tuning is asked first (Retune, or Cancel to change nothing). The setting is kept \
             for the next start, and isn't part of the set or an undo step. The result's `on` \
             is the setting afterwards.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<SwitchArgs>()
        .result::<SwitchResult>(),
        // Spec: automation R31 (M-OPT-015): an edit-class preference.
        CommandSpec::new(
            SETTINGS_SET_SNAP_AUTOMATION_TO_GRID,
            "Snap Automation to Grid",
        )
        .description(
            "Switches Snap Automation to Grid, the Options menu's item; it's on in a new \
             install. With it on, breakpoints made or moved with the mouse on an automation \
             lane snap to the editing grid. With it off they land where the pointer is, \
             snapping only to their neighbours' positions, and holding Cmd (Alt on Windows) \
             snaps that drag to the grid. The editing grid's own Snap to Grid still turns all \
             snapping off. The setting is kept for the next start, and isn't part of the set \
             or an undo step. The result's `on` is the setting afterwards.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<SwitchArgs>()
        .result::<SwitchResult>(),
        // Library (L-SET-016; WP-6.34): an edit-class preference.
        CommandSpec::new(
            SETTINGS_SET_COLLECT_FILES_ON_EXPORT,
            "Collect Files on Export",
        )
        .description(
            "Chooses Collect Files on Export on the Settings window's Library page: what \
                 happens to the audio files a clip or a set uses when it is saved to the \
                 browser as a clip file or a set. `always` (a new install's choice) copies \
                 every such file that isn't already in the saved file's project into its \
                 `Samples/Imported` folder; `never` copies none, and the saved file names each \
                 file where it is; `ask` asks first, with Copy, Don't Copy and Cancel, and the \
                 commands that save to the browser refuse to run under it without their `copy` \
                 argument, so an agent asks the person first. Collect All and Save and device \
                 presets don't follow it. The setting is kept for the next start, and isn't \
                 part of the set or an undo step.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<CollectFilesArgs>()
        .result::<()>(),
        CommandSpec::new(SETTINGS_SET_USER_LIBRARY, "User Library")
            .description(
                "Chooses the User Library's folder on the Settings window's Library page. \
                 `path` is an absolute path, or `null` for the default. The folder becomes the \
                 User Library at once: its starting folders (Clips, Defaults, Grooves, \
                 Presets, Samples, Tunings) are made in it if it has none, and it is indexed \
                 in the background. Nothing is moved from the folder before. Run by a person \
                 from the Browse button without a `path`, it asks for a folder. The setting is \
                 kept for the next start, and isn't part of the set or an undo step. The \
                 result's `path` is the User Library in use afterwards.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<UserLibraryArgs>()
            .result::<UserLibraryResult>(),
        // File & Folder (L-SET-015; WP-4.20): an edit-class preference.
        CommandSpec::new(SETTINGS_SET_TEMPORARY_FOLDER, "Temporary Folder")
            .description(
                "Chooses the Temporary Folder on the Settings window's File & Folder page: where \
                 the files Reverse, Crop, Consolidate, Freeze and Bounce write go while the set \
                 has never been saved (the first save moves them into the project's \
                 `Samples/Processed` folders). `path` is an absolute path, made when it is first \
                 needed, or `null` for the default, a `Temporary` folder in Beatlayer's \
                 application-data folder. Files already written stay where they are. Run by a \
                 person from the Browse button without a `path`, it asks for a folder. The \
                 setting is kept for the next start, and isn't part of the set or an undo step. \
                 The result's `path` is the folder in use afterwards.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<TemporaryFolderArgs>()
            .result::<TemporaryFolderResult>(),
        // File & Folder (L-SET-015; WP-4.10b): the sample editor Edit opens
        // a clip's file in.
        CommandSpec::new(SETTINGS_SET_SAMPLE_EDITOR, "Sample Editor")
            .description(
                "Chooses the sample editor on the Settings window's File & Folder page: the \
                 command the Edit button in the Clip View's Audio Utilities panel opens a clip's \
                 file in. `path` is the command: the program, its arguments, and the clip's \
                 file as the last one, or `null` for none, when Edit opens the File & Folder \
                 page instead. Run by a person from the Browse button without a `path`, it asks \
                 for a program. The setting is kept for the next start, and isn't part of the \
                 set or an undo step. The result's `path` is the editor in use afterwards.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SampleEditorArgs>()
            .result::<SampleEditorResult>(),
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
        // Record, Warp & Launch, Launch (L-SET-018; WP-3.4): edit-class
        // preferences, kept between starts and not saved with a set.
        // Record, Warp & Launch, Warp/Fades (L-SET-018; WP-4.9): edit-class
        // preferences, like the theme.
        CommandSpec::new(
            SETTINGS_SET_LOOP_WARP_SHORT_SAMPLES,
            "Loop/Warp Short Samples",
        )
        .description(
            "Chooses Loop/Warp Short Samples on the Settings window's Record, Warp & Launch \
             page: what a new audio clip of a short sample, one under 60 seconds, gets when it \
             is dropped from the browser, loaded with Enter or imported. `unwarped_one_shot` \
             leaves Warp and the loop off; `warped_one_shot` warps it as a loop of whole bars, \
             the loop off; `warped_loop` does the same with the loop on; `auto`, what a new \
             install has, makes a warped loop when the file's analysis finds a steady beat \
             that fits 1, 2, 4, 8 or 16 bars, and an unwarped one-shot otherwise. Clips \
             already in the set keep their settings. The setting is kept for the next start, \
             and isn't part of the set or an undo step.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<LoopWarpShortSamplesArgs>()
        .result::<()>(),
        CommandSpec::new(
            SETTINGS_SET_AUTO_WARP_LONG_SAMPLES,
            "Auto-Warp Long Samples",
        )
        .description(
            "Switches Auto-Warp Long Samples on the Settings window's Record, Warp & Launch \
             page; it's on in a new install, as Live's is. While it's on, a new audio clip of \
             a long sample, one of 60 seconds or more, is warped with a warp marker on the \
             first beat of every bar its analysis finds, and its 1.1.1 on the first beat; off, \
             it is left unwarped. Clips already in the set keep their settings. The setting is \
             kept for the next start, and isn't part of the set or an undo step. The result's \
             `on` is the setting afterwards.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<SwitchArgs>()
        .result::<SwitchResult>(),
        CommandSpec::new(SETTINGS_SET_DEFAULT_WARP_MODE, "Default Warp Mode")
            .description(
                "Chooses Default Warp Mode on the Settings window's Record, Warp & Launch \
                 page: the warp mode (`beats`, `tones`, `texture`, `re_pitch`, `complex` or \
                 `complex_pro`) a new warped clip takes. It's `beats` in a new install, as \
                 Live's is. Clips already in the set keep their modes. The setting is \
                 kept for the next start, and isn't part of the set or an undo step.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<DefaultWarpModeArgs>()
            .result::<()>(),
        CommandSpec::new(SETTINGS_SET_DEFAULT_LAUNCH_MODE, "Default Launch Mode")
            .description(
                "Chooses Default Launch Mode on the Settings window's Record, Warp & Launch \
                 page: the launch mode (`trigger`, `gate`, `toggle` or `repeat`) a new Session \
                 clip starts with. It's `trigger` in a new install, as Live's is. Clips made \
                 so far keep their modes, and launch modes aren't built into clips yet, so for \
                 now it only keeps the choice. The setting is kept for the next start, and \
                 isn't part of the set or an undo step.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<DefaultLaunchModeArgs>()
            .result::<()>(),
        CommandSpec::new(
            SETTINGS_SET_DEFAULT_LAUNCH_QUANTIZATION,
            "Default Launch Quantization",
        )
        .description(
            "Chooses Default Launch Quantization on the Settings window's Record, Warp & \
             Launch page: the launch quantisation a new Session clip starts with, one of the \
             global quantisation's steps (see `clip.set_launch_quantization`), or `null` to \
             follow the global quantisation (the default, Live's \"Global\"). Clips already \
             in the set keep theirs. The setting is kept for the next start, and isn't part \
             of the set or an undo step.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<DefaultLaunchQuantizationArgs>()
        .result::<()>(),
        CommandSpec::new(
            SETTINGS_SET_RECORD_SESSION_AUTOMATION_IN,
            "Record Session Automation in",
        )
        .description(
            "Chooses Record Session automation in on the Settings window's Record, Warp & \
             Launch page: `armed_tracks` (the default, Live's) records the control moves of \
             a Session recording into the clips of the armed tracks, `all_tracks` into the \
             playing clip of every track. Moves are recorded only while Automation Arm is \
             on (`arrangement.automation_arm`). The setting is kept for the next start, and \
             isn't part of the set or an undo step.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<RecordSessionAutomationInArgs>()
        .result::<()>(),
        CommandSpec::new(
            SETTINGS_SET_AUTOMATION_RECORD_MODE,
            "Automation Record Mode",
        )
        .description(
            "Chooses the mode an Arrangement recording writes control moves in, the one \
                 the Control Bar's Automation Arm button's context menu chooses: `touch` \
                 (the default, Live's) writes a control's values while it is held, `latch` \
                 holds the last value a control was moved to until the recording stops, and \
                 `write` writes a moved control's envelope from where the recording started. \
                 A Session recording is unaffected. The mode is kept for the next start, and \
                 isn't part of the set or an undo step.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<AutomationRecordModeArgs>()
        .result::<()>(),
        CommandSpec::new(SETTINGS_SET_SELECT_ON_LAUNCH, "Select on Launch")
            .description(
                "Switches Select on Launch on the Settings window's Record, Warp & Launch \
                 page; it's on in a new install, as Live's is. While it's on, launching a clip \
                 that sits in a slot (`session.launch`) selects that slot. The setting is kept \
                 for the next start, and isn't part of the set or an undo step. The result's \
                 `on` is the setting afterwards.",
            )
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SwitchArgs>()
            .result::<SwitchResult>(),
        CommandSpec::new(
            SETTINGS_SET_SELECT_NEXT_SCENE_ON_LAUNCH,
            "Select Next Scene on Launch",
        )
        .description(
            "Switches Select Next Scene on Launch on the Settings window's Record, Warp & \
             Launch page; it's on in a new install, as Live's is. While it's on, launching a \
             scene (`session.launch_scene`) selects the scene after it, when there is one. The \
             setting is kept for the next start, and isn't part of the set or an undo step. \
             The result's `on` is the setting afterwards.",
        )
        .permission(Permission::Edit)
        .context(Settings)
        .args::<SwitchArgs>()
        .result::<SwitchResult>(),
        CommandSpec::new(
            SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH,
            "Start Recording on Scene Launch",
        )
        .description(
            "Switches Start Recording on Scene Launch on the Settings window's Record, Warp & \
             Launch page; it's off in a new install, as Live's is. Session recording isn't \
             built yet, so for now it only keeps the choice. The setting is kept for the next \
             start, and isn't part of the set or an undo step. The result's `on` is the \
             setting afterwards.",
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
        // Display & Input, Mouse and Pen Input (L-SET-010; WP-6.59).
        CommandSpec::new(
            SETTINGS_SET_PERMANENT_SCRUB_AREAS,
            "Permanent Scrub Areas",
        )
        .description(
            "Switches Permanent Scrub Areas, on the Settings window's Display & Input page: \
             while it's on, as it is in a new install (as Live's is), a click in the \
             Arrangement's scrub area plays from that point and holding the mouse button there \
             loops a short span (`transport.scrub` does both); while it's off, a click there \
             places the insert marker, as a click in a track does, and a hold loops nothing, \
             but Shift-click and Shift-hold still scrub. The setting is kept for the next \
             start, and isn't part of the set or an undo step. The result's `on` is the \
             setting afterwards.",
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
