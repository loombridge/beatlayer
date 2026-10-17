//! Preferences: what the Settings window changes, and what the app keeps
//! between starts.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use bl_model::ParamId;
use bl_model::PluginFormat;

pub use bl_shell_spec::midi_io::PortDirection;
pub use bl_shell_spec::settings::{
    LaunchPrefs, SettingsPage, ThemeChoice, ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT, ZOOM_STEPS,
    zoom_factor, zoom_percent, zoom_step,
};

use crate::agent::AgentSettings;
use crate::assistant::{AssistantPanelPrefs, AssistantSettings};
use crate::audio::{AudioSettings, ToneSettings};

/// Everything the app keeps between starts, saved as JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// The audio device settings.
    pub audio: AudioSettings,
    /// The test tone's level and frequency (the Audio page's Tone Volume and
    /// Tone Frequency, D23).
    pub tone: ToneSettings,
    /// The theme.
    pub theme: ThemeChoice,
    /// Whether Tab moves the keyboard focus in the main window.
    pub tab_navigation: bool,
    /// Wrap Tab Navigation (Navigate ▸ Wrap Keyboard Navigation): Tab and
    /// Shift-Tab wrap round at the ends of the focused area or track
    /// (WP-1.14; the accessibility spec, R5). On in a new install, as
    /// observed in Live 12.4.6.
    pub wrap_navigation: OnByDefault,
    /// Outline View in Focus (Display & Input, L-SET-010): a border round
    /// the focused area and an outline round the focused control. On in a
    /// new install, as observed in Live 12.4.6 (WP-1.14; the accessibility
    /// spec, R7).
    pub outline_view_in_focus: OnByDefault,
    /// The interface zoom, where 1 is 100%, or none for 100% (WP-1.17).
    /// Read through `Shell::zoom`, which keeps it within 50% to 200%.
    pub zoom: Option<f32>,
    /// The main window's last size in points, as width and height.
    pub window_size: Option<(f32, f32)>,
    /// The Settings page shown last.
    pub settings_page: SettingsPage,
    /// The agent server: whether it runs, its port and token, and the
    /// agents allowed to connect (WP-1.15).
    pub agent: AgentSettings,
    /// The built-in assistant's provider, base URL and model (WP-2.23,
    /// part 4). Its API key is in the OS keychain, never here.
    pub assistant: AssistantSettings,
    /// Where the assistant's panel is: its docked width, and whether it
    /// floats in a window of its own (WP-2.23, part 3; D24).
    pub assistant_panel: AssistantPanelPrefs,
    /// The assistant commands the person chose "always allow" for, so a
    /// destructive or external command it calls doesn't ask again
    /// (WP-2.23, part 4).
    pub assistant_always: BTreeSet<String>,
    /// The browser's places and preview (WP-1.10).
    pub browser: crate::browser::BrowserPrefs,
    /// Create Fades on Clip Edges (Record, Warp & Launch, L-SET-018): new
    /// audio clips get 4 ms fades at their edges (WP-1.4b; the audio clips
    /// spec, R7 and R22). On by default, as observed in Live 12.4.6, and on
    /// for preferences saved before the setting existed.
    pub create_fades_on_clip_edges: OnByDefault,
    /// Warp recorded audio (Record, Warp & Launch, Warp/Fades; IMP-02, the
    /// recording spec, R13): whether a recorded clip will be warped, once
    /// warping arrives; clips have no warp fields until then. On by default,
    /// as Live warps its recordings, and on for preferences saved before the
    /// setting existed.
    pub warp_recorded_audio: OnByDefault,
    /// The Launch section of Record, Warp & Launch (L-SET-018; WP-3.4),
    /// with the defaults observed in Live 12.4.3.
    pub launch: LaunchPrefs,
    /// Auto-Hide Plug-In Windows (Plug-Ins, Plug-In Windows, L-SET-017):
    /// a floating device's window or a plug-in's editor hides while
    /// another track is selected and comes back with its track (WP-2.5b),
    /// as Live's plug-in windows do. On by default, as observed in Live
    /// 12.4.6.
    pub auto_hide_plug_in_windows: OnByDefault,
    /// The plug-ins the browser hides, as `format/plugin_id` keys
    /// (WP-2.9, IMP-07). A set that uses a hidden plug-in still loads it.
    pub hidden_plugins: BTreeSet<String>,
    /// Whether the browser shows hidden plug-ins (WP-2.9, IMP-07).
    pub show_hidden_plugins: bool,
    /// The format to prefer when a plug-in exists in several formats
    /// (WP-2.9, IMP-07).
    pub plugin_format_preference: Option<PluginFormat>,
    /// Which parameter each knob of a plug-in device's panel shows
    /// (WP-2.10, D19), by `format/plugin_id` and then by knob: absent
    /// entries keep the plug-in's own parameter at that place. Remembered
    /// per plug-in, so a device of a known plug-in keeps its mapping.
    pub plugin_panels:
        std::collections::BTreeMap<String, std::collections::BTreeMap<usize, ParamId>>,
    /// The saved default set (File ▸ Save Beatlayer Set As Default Set),
    /// which a new set starts from (WP-2.25). `None` uses the factory
    /// default set.
    pub default_set: Option<PathBuf>,
    /// The MIDI ports' switches and the computer MIDI keyboard (WP-2.2).
    pub midi: MidiPrefs,
}

/// What the app keeps of MIDI input between starts (the MIDI I/O spec, R3
/// and R17).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiPrefs {
    /// Each port's switches, by direction and name, so they apply again
    /// whenever the port is seen.
    pub ports: Vec<PortPrefs>,
    /// Whether the computer MIDI keyboard is on.
    pub keyboard_on: bool,
}

/// One MIDI port's switches in the preferences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortPrefs {
    /// Input or output.
    pub direction: PortDirection,
    /// The name the MIDI system gives the port.
    pub name: String,
    /// Track.
    pub track: bool,
    /// Sync.
    pub sync: bool,
    /// Remote.
    pub remote: bool,
}

/// A switch that's on unless the preferences say otherwise, read and
/// written as a plain `true` or `false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OnByDefault(pub bool);

impl Default for OnByDefault {
    fn default() -> Self {
        Self(true)
    }
}

/// The file name of the preferences, inside the config directory.
pub const PREFERENCES_FILE: &str = "preferences.json";

/// The file name of the plug-in scanner's cache and blocklist (WP-2.9),
/// inside the config directory.
pub const PLUGIN_SCAN_FILE: &str = "plugin-scan.json";

impl Preferences {
    /// Reads preferences from `dir`, or the defaults if there are none.
    ///
    /// # Errors
    ///
    /// A message if the file exists but can't be read or parsed; the caller
    /// shows it and carries on with the defaults.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(PREFERENCES_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("couldn't read the preferences in {}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!(
                "couldn't read the preferences in {}: {e}",
                path.display()
            )),
        }
    }

    /// Writes preferences to `dir`, creating it if needed. The file is
    /// written beside its final name and renamed into place, so a crash
    /// never leaves half a file.
    ///
    /// # Errors
    ///
    /// A message if the directory or file can't be written.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("couldn't encode the preferences: {e}"))?;
        let path = dir.join(PREFERENCES_FILE);
        let temporary = dir.join(format!(".{PREFERENCES_FILE}.tmp"));
        std::fs::write(&temporary, text)
            .map_err(|e| format!("couldn't write {}: {e}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .map_err(|e| format!("couldn't write {}: {e}", path.display()))
    }
}

/// Where Beatlayer keeps its preferences on this platform:
/// `~/Library/Application Support/Beatlayer` on macOS, `%APPDATA%\Beatlayer`
/// on Windows, and `$XDG_CONFIG_HOME/beatlayer` (or `~/.config/beatlayer`)
/// on Linux. `None` if the home directory isn't known.
#[must_use]
pub fn default_config_dir() -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    if cfg!(target_os = "macos") {
        env("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Beatlayer")
        })
    } else if cfg!(windows) {
        env("APPDATA").map(|dir| PathBuf::from(dir).join("Beatlayer"))
    } else {
        env("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|dir| dir.join("beatlayer"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip_through_a_directory() {
        let dir = std::env::temp_dir().join(format!("bl-shell-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Preferences::load(&dir).unwrap(), Preferences::default());
        let mut prefs = Preferences {
            theme: ThemeChoice::Light,
            tab_navigation: true,
            zoom: Some(1.25),
            window_size: Some((1600.0, 977.0)),
            settings_page: SettingsPage::ThemeColors,
            ..Preferences::default()
        };
        prefs.audio.buffer_size = Some(128);
        prefs.tone.volume_db = -48.0;
        prefs.create_fades_on_clip_edges = OnByDefault(false);
        prefs.save(&dir).unwrap();
        assert_eq!(Preferences::load(&dir).unwrap(), prefs);
        std::fs::write(dir.join(PREFERENCES_FILE), "not json").unwrap();
        assert!(Preferences::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_hide_plug_in_windows_is_on_unless_saved_off() {
        // Spec: device-chains R31. Live's default, observed in Live 12.4.6.
        assert!(Preferences::default().auto_hide_plug_in_windows.0);
        let older: Preferences = serde_json::from_str(r#"{"tab_navigation": true}"#).unwrap();
        assert!(older.auto_hide_plug_in_windows.0);
        let off: Preferences =
            serde_json::from_str(r#"{"auto_hide_plug_in_windows": false}"#).unwrap();
        assert!(!off.auto_hide_plug_in_windows.0);
    }

    #[test]
    fn warp_recorded_audio_is_on_unless_saved_off() {
        // Spec: recording R13.
        assert!(Preferences::default().warp_recorded_audio.0);
        let older: Preferences = serde_json::from_str(r#"{"tab_navigation": true}"#).unwrap();
        assert!(older.warp_recorded_audio.0);
        let off: Preferences = serde_json::from_str(r#"{"warp_recorded_audio": false}"#).unwrap();
        assert!(!off.warp_recorded_audio.0);
        let saved = serde_json::to_value(&off).unwrap();
        assert_eq!(saved["warp_recorded_audio"], false);
    }

    #[test]
    fn create_fades_on_clip_edges_is_on_unless_saved_off() {
        // Spec: audio-clips R22. Live's default, observed in Live 12.4.6.
        assert!(Preferences::default().create_fades_on_clip_edges.0);
        let older: Preferences = serde_json::from_str(r#"{"tab_navigation": true}"#).unwrap();
        assert!(older.create_fades_on_clip_edges.0);
        let off: Preferences =
            serde_json::from_str(r#"{"create_fades_on_clip_edges": false}"#).unwrap();
        assert!(!off.create_fades_on_clip_edges.0);
        let saved = serde_json::to_value(&off).unwrap();
        assert_eq!(saved["create_fades_on_clip_edges"], false);
    }
}
