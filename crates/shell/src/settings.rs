//! Preferences: what the Settings window changes, and what the app keeps
//! between starts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use bl_shell_spec::settings::{SettingsPage, ThemeChoice};

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
        prefs.save(&dir).unwrap();
        assert_eq!(Preferences::load(&dir).unwrap(), prefs);
        std::fs::write(dir.join(PREFERENCES_FILE), "not json").unwrap();
        assert!(Preferences::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
