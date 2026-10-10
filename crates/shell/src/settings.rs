//! Preferences: what the Settings window changes, and what the app keeps
//! between starts.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audio::AudioSettings;

/// A page of the Settings window, in Live's order (L-SET-002).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsPage {
    /// Display & Input (L-SET-010).
    DisplayInput,
    /// Theme & Colors (L-SET-011).
    ThemeColors,
    /// Audio (L-SET-012). The page the window first opens on.
    #[default]
    Audio,
    /// Link (L-SET-013).
    Link,
    /// Tempo & MIDI (L-SET-014).
    TempoMidi,
    /// File & Folder (L-SET-015).
    FileFolder,
    /// Library (L-SET-016).
    Library,
    /// Plug-Ins (L-SET-017).
    PlugIns,
    /// Record, Warp & Launch (L-SET-018).
    RecordWarpLaunch,
    /// Licenses & Updates (L-SET-019).
    LicensesUpdates,
}

impl SettingsPage {
    /// Every page, in the page chooser's order.
    pub const ALL: [Self; 10] = [
        Self::DisplayInput,
        Self::ThemeColors,
        Self::Audio,
        Self::Link,
        Self::TempoMidi,
        Self::FileFolder,
        Self::Library,
        Self::PlugIns,
        Self::RecordWarpLaunch,
        Self::LicensesUpdates,
    ];

    /// The page's name in the page chooser.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DisplayInput => "Display & Input",
            Self::ThemeColors => "Theme & Colors",
            Self::Audio => "Audio",
            Self::Link => "Link",
            Self::TempoMidi => "Tempo & MIDI",
            Self::FileFolder => "File & Folder",
            Self::Library => "Library",
            Self::PlugIns => "Plug-Ins",
            Self::RecordWarpLaunch => "Record, Warp & Launch",
            Self::LicensesUpdates => "Licenses & Updates",
        }
    }

    /// The UI parity reference's row for the page.
    #[must_use]
    pub const fn row(self) -> &'static str {
        match self {
            Self::DisplayInput => "L-SET-010",
            Self::ThemeColors => "L-SET-011",
            Self::Audio => "L-SET-012",
            Self::Link => "L-SET-013",
            Self::TempoMidi => "L-SET-014",
            Self::FileFolder => "L-SET-015",
            Self::Library => "L-SET-016",
            Self::PlugIns => "L-SET-017",
            Self::RecordWarpLaunch => "L-SET-018",
            Self::LicensesUpdates => "L-SET-019",
        }
    }

    /// Whether this build has the page's settings, or only its place in the
    /// chooser. The shell (WP-1.7) builds Audio and the theme choice.
    #[must_use]
    pub const fn is_built(self) -> bool {
        matches!(self, Self::Audio | Self::ThemeColors)
    }

    /// The page `steps` places on, wrapping round.
    #[must_use]
    pub fn step(self, steps: isize) -> Self {
        let len = Self::ALL.len() as isize;
        let index = Self::ALL.iter().position(|p| *p == self).unwrap_or(0) as isize;
        Self::ALL[(index + steps).rem_euclid(len) as usize]
    }
}

/// Beatlayer's two themes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    /// Light text on dark panels, the default.
    #[default]
    Dark,
    /// Dark text on light panels.
    Light,
}

impl ThemeChoice {
    /// The theme's name in the Settings window.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }
}

/// Everything the app keeps between starts, saved as JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// The audio device settings.
    pub audio: AudioSettings,
    /// The theme.
    pub theme: ThemeChoice,
    /// Whether Tab moves the keyboard focus in the main window.
    pub tab_navigation: bool,
    /// The interface zoom, where 1 is 100%.
    pub zoom: Option<f32>,
    /// The main window's last size in points, as width and height.
    pub window_size: Option<(f32, f32)>,
    /// The Settings page shown last.
    pub settings_page: SettingsPage,
}

/// The file name of the preferences, inside the config directory.
pub const PREFERENCES_FILE: &str = "preferences.json";

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
    fn pages_step_round_in_order() {
        assert_eq!(SettingsPage::Audio.step(1), SettingsPage::Link);
        assert_eq!(
            SettingsPage::DisplayInput.step(-1),
            SettingsPage::LicensesUpdates
        );
        assert_eq!(
            SettingsPage::LicensesUpdates.step(1),
            SettingsPage::DisplayInput
        );
        let ids: Vec<String> = SettingsPage::ALL
            .iter()
            .map(|p| {
                serde_json::to_value(p)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(ids[2], "audio");
        assert_eq!(ids[8], "record-warp-launch");
    }

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
        prefs.save(&dir).unwrap();
        assert_eq!(Preferences::load(&dir).unwrap(), prefs);
        std::fs::write(dir.join(PREFERENCES_FILE), "not json").unwrap();
        assert!(Preferences::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
