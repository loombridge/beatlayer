//! Metronome settings and choices (WP-2.15).

use bl_core::time::{Ticks, TimeSignature};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The count-in length before playback or recording starts (Live manual 19.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CountIn {
    /// No count-in: playback/recording starts immediately.
    #[default]
    None,
    /// 1 bar count-in.
    #[serde(rename = "1_bar")]
    Bars1,
    /// 2 bars count-in.
    #[serde(rename = "2_bars")]
    Bars2,
    /// 4 bars count-in.
    #[serde(rename = "4_bars")]
    Bars4,
}

impl CountIn {
    /// How many full bars this count-in lasts.
    #[must_use]
    pub const fn bars(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Bars1 => 1,
            Self::Bars2 => 2,
            Self::Bars4 => 4,
        }
    }
}

/// The synthesized click sound style (Live manual 19.4.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MetronomeSound {
    /// Pure electronic beep tone.
    #[default]
    Classic,
    /// Short, sharp digital click transient.
    Click,
    /// Resonant woodblock percussion.
    Wood,
}

/// The beat division at which the metronome clicks (Live manual 19.4.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MetronomeRhythm {
    /// Automatically follows the current time signature's denominator.
    #[default]
    Auto,
    /// Once per bar on the downbeat.
    #[serde(rename = "1_bar")]
    Bar1,
    /// Half notes.
    #[serde(rename = "1/2")]
    Half,
    /// Half note triplets.
    #[serde(rename = "1/2t")]
    HalfTriplet,
    /// Quarter notes.
    #[serde(rename = "1/4")]
    Quarter,
    /// Quarter note triplets.
    #[serde(rename = "1/4t")]
    QuarterTriplet,
    /// Eighth notes.
    #[serde(rename = "1/8")]
    Eighth,
    /// Eighth note triplets.
    #[serde(rename = "1/8t")]
    EighthTriplet,
    /// Sixteenth notes.
    #[serde(rename = "1/16")]
    Sixteenth,
    /// Sixteenth note triplets.
    #[serde(rename = "1/16t")]
    SixteenthTriplet,
}

impl MetronomeRhythm {
    /// The nominal interval in ticks for this rhythm under `signature`.
    #[must_use]
    pub fn interval(self, signature: TimeSignature) -> Ticks {
        match self {
            Self::Auto => signature.beat_length(),
            Self::Bar1 => signature.bar_length(),
            Self::Half => Ticks(Ticks::WHOLE.get() / 2),
            Self::HalfTriplet => Ticks(Ticks::WHOLE.get() / 3),
            Self::Quarter => Ticks::QUARTER,
            Self::QuarterTriplet => Ticks(Ticks::WHOLE.get() / 6),
            Self::Eighth => Ticks(Ticks::QUARTER.get() / 2),
            Self::EighthTriplet => Ticks(Ticks::QUARTER.get() / 3),
            Self::Sixteenth => Ticks(Ticks::QUARTER.get() / 4),
            Self::SixteenthTriplet => Ticks(Ticks::QUARTER.get() / 6),
        }
    }

    /// Whether this rhythm division fits into one bar of `signature` (Live manual 19.4.1).
    #[must_use]
    pub fn fits_in_bar(self, signature: TimeSignature) -> bool {
        self.interval(signature) <= signature.bar_length()
    }

    /// The effective interval in ticks, falling back to `Auto` if the division does not fit in one bar.
    #[must_use]
    pub fn effective_interval(self, signature: TimeSignature) -> Ticks {
        if self.fits_in_bar(signature) {
            self.interval(signature)
        } else {
            Self::Auto.interval(signature)
        }
    }
}

/// Metronome settings saved with the set (WP-2.15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MetronomeSettings {
    /// Whether the metronome is active.
    #[serde(default)]
    pub enabled: bool,
    /// The count-in length before playback or recording starts.
    #[serde(default)]
    pub count_in: CountIn,
    /// The sound character of the click.
    #[serde(default)]
    pub sound: MetronomeSound,
    /// The rhythm division.
    #[serde(default)]
    pub rhythm: MetronomeRhythm,
    /// When true, clicks are only audible while recording.
    #[serde(default)]
    pub only_while_recording: bool,
}

impl Default for MetronomeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            count_in: CountIn::None,
            sound: MetronomeSound::Classic,
            rhythm: MetronomeRhythm::Auto,
            only_while_recording: false,
        }
    }
}

impl MetronomeSettings {
    /// Whether this is the default configuration.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
