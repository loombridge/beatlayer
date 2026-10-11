//! The Settings window's pages and the theme choice, as the shell's
//! commands name them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
}
