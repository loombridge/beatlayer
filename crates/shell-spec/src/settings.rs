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
    /// Agents (L-SET-020). Beatlayer's own page: the agent server's
    /// settings (WP-1.15), which Live doesn't have. It follows Plug-Ins in
    /// the chooser (a Beatlayer addition, decided 14 October 2026).
    Agents,
    /// Record, Warp & Launch (L-SET-018).
    RecordWarpLaunch,
    /// Licenses & Updates (L-SET-019).
    LicensesUpdates,
}

impl SettingsPage {
    /// Every page, in the page chooser's order.
    pub const ALL: [Self; 11] = [
        Self::DisplayInput,
        Self::ThemeColors,
        Self::Audio,
        Self::Link,
        Self::TempoMidi,
        Self::FileFolder,
        Self::Library,
        Self::PlugIns,
        Self::Agents,
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
            Self::Agents => "Agents",
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
            Self::Agents => "L-SET-020",
            Self::RecordWarpLaunch => "L-SET-018",
            Self::LicensesUpdates => "L-SET-019",
        }
    }

    /// Whether this build has any of the page's settings, or only its place
    /// in the chooser. The shell (WP-1.7) builds Audio and the theme choice,
    /// WP-1.17 Display & Input's Zoom, WP-1.4b Record, Warp & Launch's
    /// Create Fades on Clip Edges, WP-2.5b Plug-Ins' Auto-Hide Plug-In
    /// Windows, and WP-1.15 the Agents page's agent server settings.
    #[must_use]
    pub const fn is_built(self) -> bool {
        matches!(
            self,
            Self::Audio
                | Self::ThemeColors
                | Self::DisplayInput
                | Self::RecordWarpLaunch
                | Self::PlugIns
                | Self::Agents
        )
    }

    /// The page `steps` places on, wrapping round.
    #[must_use]
    pub fn step(self, steps: isize) -> Self {
        let len = Self::ALL.len() as isize;
        let index = Self::ALL.iter().position(|p| *p == self).unwrap_or(0) as isize;
        Self::ALL[(index + steps).rem_euclid(len) as usize]
    }
}

/// The interface zoom's smallest setting, in percent (WP-1.17; spec
/// interface-zoom R3).
pub const ZOOM_MIN_PERCENT: u16 = 50;

/// The interface zoom's largest setting, in percent.
pub const ZOOM_MAX_PERCENT: u16 = 200;

/// The steps Zoom In and Zoom Out move through, and the Display & Input
/// page's Zoom box steps through, in percent: every 10% from 50% to 200%.
/// Live's steps aren't recorded (interface-zoom, "To observe in Live").
pub const ZOOM_STEPS: [u16; 16] = [
    50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160, 170, 180, 190, 200,
];

/// A zoom factor (1 is 100%) as a whole percentage within the zoom's range.
/// Anything that isn't a finite number is 100%.
#[must_use]
pub fn zoom_percent(factor: f32) -> u16 {
    if !factor.is_finite() {
        return 100;
    }
    let percent = (factor * 100.0).round();
    percent.clamp(f32::from(ZOOM_MIN_PERCENT), f32::from(ZOOM_MAX_PERCENT)) as u16
}

/// A percentage as a zoom factor, brought within the zoom's range.
#[must_use]
pub fn zoom_factor(percent: u16) -> f32 {
    f32::from(percent.clamp(ZOOM_MIN_PERCENT, ZOOM_MAX_PERCENT)) / 100.0
}

/// The zoom `steps` steps up (or, negative, down) from `percent`: each step
/// goes to the next of [`ZOOM_STEPS`] in its direction, so a zoom between
/// steps goes to the nearest step that way first. It stops at 50% and 200%.
#[must_use]
pub fn zoom_step(percent: u16, steps: i32) -> u16 {
    let mut percent = percent.clamp(ZOOM_MIN_PERCENT, ZOOM_MAX_PERCENT);
    for _ in 0..steps.unsigned_abs() {
        let next = if steps > 0 {
            ZOOM_STEPS.iter().copied().find(|&step| step > percent)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|&step| step < percent)
        };
        match next {
            Some(next) => percent = next,
            None => break,
        }
    }
    percent
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
        assert_eq!(ids[8], "agents");
        assert_eq!(ids[9], "record-warp-launch");
    }

    #[test]
    fn zoom_steps_run_every_ten_percent_from_50_to_200() {
        // Spec: interface-zoom R3.
        assert_eq!(ZOOM_STEPS.first(), Some(&ZOOM_MIN_PERCENT));
        assert_eq!(ZOOM_STEPS.last(), Some(&ZOOM_MAX_PERCENT));
        assert!(ZOOM_STEPS.windows(2).all(|pair| pair[1] - pair[0] == 10));
        assert!(ZOOM_STEPS.contains(&100));
        assert_eq!(zoom_step(100, 1), 110);
        assert_eq!(zoom_step(100, -1), 90);
        assert_eq!(zoom_step(115, 1), 120, "between steps, to the next one up");
        assert_eq!(
            zoom_step(115, -1),
            110,
            "between steps, to the next one down"
        );
        assert_eq!(zoom_step(190, 3), 200, "stops at the top");
        assert_eq!(zoom_step(60, -3), 50, "stops at the bottom");
        assert_eq!(zoom_step(130, 0), 130);
    }

    #[test]
    fn zoom_factors_and_percentages_stay_in_range() {
        assert_eq!(zoom_percent(1.0), 100);
        assert_eq!(zoom_percent(1.254), 125);
        assert_eq!(zoom_percent(0.1), 50);
        assert_eq!(zoom_percent(9.0), 200);
        assert_eq!(zoom_percent(f32::NAN), 100);
        assert_eq!(zoom_percent(f32::INFINITY), 100);
        assert!((zoom_factor(150) - 1.5).abs() < f32::EPSILON);
        assert!((zoom_factor(20) - 0.5).abs() < f32::EPSILON);
        assert!((zoom_factor(500) - 2.0).abs() < f32::EPSILON);
        for step in ZOOM_STEPS {
            assert_eq!(zoom_percent(zoom_factor(step)), step);
        }
    }
}
