//! Preferences: what the Settings window changes, and what the app keeps
//! between starts.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use bl_model::ParamId;
use bl_model::PluginFormat;

pub use bl_shell_spec::midi_io::PortDirection;
pub use bl_shell_spec::settings::{
    Appearance, AutomationRecordMode, BRIGHTNESS, BUNDLED_THEME, COLOR_HUE, COLOR_INTENSITY,
    ClipColour, CollectFiles, DefaultWarpMode, GRID_LINE_INTENSITY, LaunchPrefs,
    LoopWarpShortSamples, SessionAutomationTracks, SettingsPage, ThemeLook, Tone, WarpPrefs,
    ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT, ZOOM_STEPS, zoom_factor, zoom_percent, zoom_step,
};

use crate::keymap::UserKeymap;

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
    /// Appearance (themes R3). Preferences saved before the Theme & Colors
    /// page called it `theme` and held `dark` or `light` (R11).
    #[serde(alias = "theme")]
    pub appearance: Appearance,
    /// The theme, tone, high contrast and Customization (themes R1).
    pub look: ThemeLook,
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
    /// The second window's own zoom (interface-zoom R11), where 1 is 100%:
    /// none until it has been opened or set, when it takes the main
    /// window's.
    pub second_zoom: Option<f32>,
    /// Where the second window sat and how big it was when it was last open
    /// (second-window R1).
    pub second_window: Option<bl_model::WindowFrame>,
    /// The Settings page shown last.
    pub settings_page: SettingsPage,
    /// The agent server: whether it runs, its port and token, and the
    /// agents allowed to connect (WP-1.15). Its tokens are in the OS
    /// keychain, never here.
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
    /// What the Export Audio/Video dialog keeps: its choices and the folder
    /// of the last export (WP-6.71; the export spec, R39).
    pub export: crate::ExportPrefs,
    /// The browser's places and preview (WP-1.10).
    pub browser: crate::browser::BrowserPrefs,
    /// Whether the return tracks scroll with the tracks (IMP-01, the mixer
    /// view spec R21): an application setting, kept between launches and
    /// not saved with the set. Off in a new install, so the returns stay
    /// pinned, as in Live.
    pub return_tracks_scroll: bool,
    /// The MIDI Note Editor's Preview switch (Options > MIDI Note Editor
    /// Preview; WP-6.5, the step recording spec, R1): one switch for every
    /// MIDI clip, off in a new install.
    pub note_preview: bool,
    /// Highlight Scales (Options, M-OPT-019; the scales spec, R22): one
    /// switch for every clip, off in a new install.
    pub highlight_scales: bool,
    /// How the note editor spells keys (the piano ruler's menu, C-KEYS-002;
    /// the scales spec, R24): one choice for every editor, Sharps in a new
    /// install.
    pub note_spelling: bl_model::NoteSpelling,
    /// Create Fades on Clip Edges (Record, Warp & Launch, L-SET-018): new
    /// audio clips get 4 ms fades at their edges (WP-1.4b; the audio clips
    /// spec, R7 and R22). On by default, as observed in Live 12.4.6, and on
    /// for preferences saved before the setting existed.
    pub create_fades_on_clip_edges: OnByDefault,
    /// Retune Set On Loading Tuning Systems (Options, M-OPT-022; the
    /// tunings spec, R15). Off in a new install.
    pub retune_on_tuning_load: bool,
    /// Snap Automation to Grid (Options, M-OPT-015; the automation spec,
    /// R31). On in a new install, and on for preferences saved before it
    /// existed.
    pub snap_automation_to_grid: OnByDefault,
    /// Warp recorded audio (Record, Warp & Launch, Warp/Fades; IMP-02, the
    /// recording spec, R13): whether a recorded clip will be warped, once
    /// warping arrives; clips have no warp fields until then. On by default,
    /// as Live warps its recordings, and on for preferences saved before the
    /// setting existed.
    pub warp_recorded_audio: OnByDefault,
    /// Loop/Warp Short Samples, Auto-Warp Long Samples and Default Warp Mode
    /// (Record, Warp & Launch, L-SET-018; WP-4.9): what a new clip of a file
    /// gets (the warping spec, R29 and R33).
    pub warp: WarpPrefs,
    /// The Launch section of Record, Warp & Launch (L-SET-018; WP-3.4),
    /// with the defaults observed in Live 12.4.3.
    pub launch: LaunchPrefs,
    /// Record Session automation in (Record, Warp & Launch, L-SET-018;
    /// WP-3.12): Armed Tracks, as observed in Live 12.4.3.
    pub record_session_automation_in: SessionAutomationTracks,
    /// The mode an Arrangement recording writes control moves in (the
    /// automation spec, R18): Touch, Latch or Write. It's kept between
    /// launches, and isn't part of the set.
    pub automation_record_mode: AutomationRecordMode,
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
    /// Use Audio Units v2 (Plug-Ins, Plug-In Sources, L-SET-017): Audio
    /// Units in `.component` bundles are scanned and listed (WP-2.9b). Off
    /// by default, as L-SET-017 records Live's default.
    pub use_audio_units_v2: bool,
    /// Use Audio Units v3 (L-SET-017): kept and shown; Beatlayer doesn't
    /// scan app-extension Audio Units yet (WP-2.9b). Off by default.
    pub use_audio_units_v3: bool,
    /// Use VST3 Plug-In System Folders (L-SET-017): VST3 plug-ins in the
    /// format's system folders are scanned and listed (WP-2.9b). Off by
    /// default, as L-SET-017 records Live's default.
    pub vst3_system_folders: bool,
    /// Which parameter each knob of a plug-in device's panel shows
    /// (WP-2.10, D19), by `format/plugin_id` and then by knob: absent
    /// entries keep the plug-in's own parameter at that place. Remembered
    /// per plug-in, so a device of a known plug-in keeps its mapping.
    pub plugin_panels:
        std::collections::BTreeMap<String, std::collections::BTreeMap<usize, ParamId>>,
    /// The Temporary Folder (File & Folder, L-SET-015): where the files an
    /// edit writes go while the set has never been saved (WP-4.20, the audio
    /// clip editing spec, R5). `None` is the default, a `Temporary` folder
    /// in Beatlayer's application-data folder.
    pub temporary_folder: Option<PathBuf>,
    /// The sample editor (File & Folder, L-SET-015): the command the Edit
    /// button in the Clip View's Audio Utilities panel opens a clip's file in
    /// (WP-4.10b, the sample editor spec, R16), with the file as its last
    /// argument. `None` names no editor, so Edit opens the File & Folder
    /// page instead.
    pub sample_editor: Option<String>,
    /// Collect Files on Export (Library, L-SET-016): what saving a clip to
    /// the browser does with the audio files it plays (file-management R7).
    pub collect_files_on_export: CollectFiles,
    /// Save Continuously (File & Folder, L-SET-015): whether every finished
    /// edit of a set that has a file is saved within a second (WP-6.62, the
    /// crash-recovery spec, R8 and R24). Off in a new install, as Live has
    /// no such mode.
    pub save_continuously: bool,
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
    /// MPE, for an input; a preferences file from before it has none.
    #[serde(default)]
    pub mpe: bool,
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

/// The file name of the browser's database (WP-6.31), inside the config
/// directory: the index of the places' files, the tags and tag groups, and
/// the items that carry them.
pub const BROWSER_DATABASE_FILE: &str = "browser.db";
/// The file name of the person's own keymap, inside the config directory,
/// beside the preferences (R1).
pub const KEYMAP_FILE: &str = "keymap.toml";

/// Reads the person's own keymap from `dir`, or an empty keymap when the
/// file isn't there: without the file, the keymap is Live's exactly (R1).
///
/// # Errors
///
/// A message if the file exists but can't be read or parsed; the caller
/// shows it and carries on with Live's keymap.
pub fn load_keymap(dir: &Path) -> Result<UserKeymap, String> {
    let path = dir.join(KEYMAP_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => UserKeymap::parse(&text)
            .map_err(|e| format!("couldn't read the keymap in {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserKeymap::default()),
        Err(e) => Err(format!(
            "couldn't read the keymap in {}: {e}",
            path.display()
        )),
    }
}

/// Writes the person's own keymap to `dir`, creating it if needed. The
/// file is written beside its final name and renamed into place, so a
/// crash never leaves half a file.
///
/// # Errors
///
/// A message if the directory or file can't be written.
pub fn save_keymap(dir: &Path, keymap: &UserKeymap) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let path = dir.join(KEYMAP_FILE);
    let temporary = dir.join(format!(".{KEYMAP_FILE}.tmp"));
    std::fs::write(&temporary, keymap.to_file())
        .map_err(|e| format!("couldn't write {}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, &path)
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

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
            appearance: Appearance::Light,
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

    // Spec: warping R29
    #[test]
    fn the_warp_settings_start_as_observed_and_are_kept() {
        let defaults = Preferences::default().warp;
        assert_eq!(defaults.loop_warp_short_samples, LoopWarpShortSamples::Auto);
        assert!(defaults.auto_warp_long_samples);
        assert_eq!(defaults.default_warp_mode, DefaultWarpMode::Beats);
        // Preferences saved before the settings existed take the defaults.
        let older: Preferences = serde_json::from_str(r#"{"tab_navigation": true}"#).unwrap();
        assert_eq!(older.warp, defaults);
        let dir = std::env::temp_dir().join(format!("bl-shell-warp-prefs-{}", std::process::id()));
        let kept = Preferences {
            warp: WarpPrefs {
                loop_warp_short_samples: LoopWarpShortSamples::WarpedOneShot,
                auto_warp_long_samples: false,
                default_warp_mode: DefaultWarpMode::ComplexPro,
            },
            ..Preferences::default()
        };
        kept.save(&dir).unwrap();
        assert_eq!(Preferences::load(&dir).unwrap(), kept);
        let _ = std::fs::remove_dir_all(&dir);
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
