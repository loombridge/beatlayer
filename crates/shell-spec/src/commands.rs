//! The app shell's commands in the command registry (decisions D16 and
//! D17): files, windows, views, focus, settings, and the audio device.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask`
//! can check them and list them in `docs/commands.md` without linking a UI
//! crate or an audio host. `bl-shell`'s `Shell::run` runs them by ID.

use bl_commands::Context::{Dialog, Global, Settings, TabFocus};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audio::{AudioDevices, AudioStatus, ConfigureAudio};
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
/// `view.full_screen`: toggle full-screen mode.
pub const VIEW_FULL_SCREEN: &str = "view.full_screen";
/// `view.toggle_browser`: show or hide the browser.
pub const VIEW_TOGGLE_BROWSER: &str = "view.toggle_browser";
/// `view.toggle_session_arrangement`: switch the main view.
pub const VIEW_TOGGLE_SESSION_ARRANGEMENT: &str = "view.toggle_session_arrangement";
/// `view.zoom_in`: zoom the interface in.
pub const VIEW_ZOOM_IN: &str = "view.zoom_in";
/// `view.zoom_out`: zoom the interface out.
pub const VIEW_ZOOM_OUT: &str = "view.zoom_out";
/// `view.zoom_reset`: reset the interface zoom.
pub const VIEW_ZOOM_RESET: &str = "view.zoom_reset";
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
/// `settings.next_page`: show the next Settings page.
pub const SETTINGS_NEXT_PAGE: &str = "settings.next_page";
/// `settings.previous_page`: show the previous Settings page.
pub const SETTINGS_PREVIOUS_PAGE: &str = "settings.previous_page";
/// `settings.set_theme`: choose the dark or light theme.
pub const SETTINGS_SET_THEME: &str = "settings.set_theme";
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

/// Every command [`register`] adds apart from the agent server's
/// ([`crate::agent::IDS`]), so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SET_NEW,
    SET_OPEN,
    SET_SAVE,
    SET_SAVE_AS,
    SET_DISCARD_CHANGES,
    APP_QUIT,
    APP_HIDE,
    APP_ABOUT,
    APP_SETTINGS,
    WINDOW_CLOSE,
    VIEW_FULL_SCREEN,
    VIEW_TOGGLE_BROWSER,
    VIEW_TOGGLE_SESSION_ARRANGEMENT,
    VIEW_ZOOM_IN,
    VIEW_ZOOM_OUT,
    VIEW_ZOOM_RESET,
    FOCUS_CONTROL_BAR,
    FOCUS_ARRANGEMENT,
    FOCUS_NEXT,
    FOCUS_PREVIOUS,
    FOCUS_TOGGLE_TAB_NAVIGATION,
    SETTINGS_NEXT_PAGE,
    SETTINGS_PREVIOUS_PAGE,
    SETTINGS_SET_THEME,
    AUDIO_DEVICES,
    AUDIO_STATUS,
    AUDIO_CONFIGURE,
    AUDIO_TOGGLE_ENGINE,
    AUDIO_SET_TEST_TONE,
];

/// Whether `id` is one of the shell's commands: those in [`IDS`] and the
/// agent server's ([`crate::agent::IDS`]).
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id) || crate::agent::handles(id)
}

/// Arguments of `set.open` and `set.save_as`: a set file's path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathArgs {
    /// The set file's path, absolute or relative to the app's working
    /// directory.
    pub path: String,
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
    /// The set's file, if it has one.
    pub path: Option<String>,
    /// The set's name, as the title bar shows it.
    pub name: String,
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
    /// Whether the main window is in full-screen mode.
    pub full_screen: bool,
    /// The interface zoom, where 1 is 100%.
    pub zoom: f32,
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
    crate::agent::register(registry)
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
                 kept in the `Backup` folder beside it (the last 10 saves). Fails if the set has \
                 never been saved; use `set.save_as` to give it a file first.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .binding("S-FILE-003")
            .args::<NoArgs>()
            .result::<SetResult>(),
        CommandSpec::new(SET_SAVE_AS, "Save Beatlayer Set As...")
            .description(
                "Saves the set to a new file at `path` (a `.blset` file; its folder is \
                 created if needed), which becomes the set's file. An existing file at `path` \
                 is replaced, and kept in the `Backup` folder beside it. A person choosing the \
                 menu item or pressing the shortcut picks the path in a dialog.",
            )
            .permission(Permission::External)
            .context(Global)
            .binding("S-FILE-004")
            .args::<PathArgs>()
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
        CommandSpec::new(VIEW_ZOOM_IN, "Zoom In")
            .description(
                "Makes the whole interface larger by one step, up to 200%. Changes nothing in \
                 the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-ZOOM-001")
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_ZOOM_OUT, "Zoom Out")
            .description(
                "Makes the whole interface smaller by one step, down to 50%. Changes nothing \
                 in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-ZOOM-002")
            .args::<NoArgs>()
            .result::<ViewResult>(),
        CommandSpec::new(VIEW_ZOOM_RESET, "Reset Zoom to 100%")
            .description("Returns the interface zoom to 100%. Changes nothing in the set.")
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
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
        assert_eq!(registry.len(), IDS.len() + crate::agent::IDS.len());
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
