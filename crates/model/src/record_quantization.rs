//! Record Quantization: the grid the notes a recording makes are moved onto
//! (Edit ▸ Record Quantization; the recording spec, R17 and R18).

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A Record Quantization value: No Quantization or the note values of the
/// Edit menu's submenu, in the menu's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum RecordQuantization {
    /// No Quantization: recorded notes stay where they were played.
    #[default]
    #[serde(rename = "none")]
    None,
    /// Quarter notes.
    #[serde(rename = "1/4")]
    Quarter,
    /// Eighth notes.
    #[serde(rename = "1/8")]
    Eighth,
    /// Eighth-note triplets.
    #[serde(rename = "1/8t")]
    EighthTriplet,
    /// Eighth notes and eighth-note triplets together.
    #[serde(rename = "1/8+1/8t")]
    EighthAndTriplet,
    /// Sixteenth notes.
    #[serde(rename = "1/16")]
    Sixteenth,
    /// Sixteenth-note triplets.
    #[serde(rename = "1/16t")]
    SixteenthTriplet,
    /// Sixteenth notes and sixteenth-note triplets together.
    #[serde(rename = "1/16+1/16t")]
    SixteenthAndTriplet,
    /// Thirty-second notes.
    #[serde(rename = "1/32")]
    ThirtySecond,
}

impl RecordQuantization {
    /// Every value, in the order of the menu's items.
    pub const ALL: [Self; 9] = [
        Self::None,
        Self::Quarter,
        Self::Eighth,
        Self::EighthTriplet,
        Self::EighthAndTriplet,
        Self::Sixteenth,
        Self::SixteenthTriplet,
        Self::SixteenthAndTriplet,
        Self::ThirtySecond,
    ];

    /// The value a switch that turns record quantising on picks while the
    /// set has never had one chosen (Spec: recording R17).
    pub const FIRST_ON: Self = Self::Sixteenth;

    /// Whether this is No Quantization.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// The spacings of the grids whose lines, laid out from each bar's
    /// start, notes move onto together: none for No Quantization, two for
    /// an "and" value (the recording spec, R18).
    #[must_use]
    pub const fn spacings(self) -> &'static [Ticks] {
        const QUARTER: i64 = Ticks::QUARTER.0;
        match self {
            Self::None => &[],
            Self::Quarter => &[Ticks(QUARTER)],
            Self::Eighth => &[Ticks(QUARTER / 2)],
            Self::EighthTriplet => &[Ticks(QUARTER / 3)],
            Self::EighthAndTriplet => &[Ticks(QUARTER / 2), Ticks(QUARTER / 3)],
            Self::Sixteenth => &[Ticks(QUARTER / 4)],
            Self::SixteenthTriplet => &[Ticks(QUARTER / 6)],
            Self::SixteenthAndTriplet => &[Ticks(QUARTER / 4), Ticks(QUARTER / 6)],
            Self::ThirtySecond => &[Ticks(QUARTER / 8)],
        }
    }
}
