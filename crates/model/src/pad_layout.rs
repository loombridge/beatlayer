//! How a note-playing controller lays the set's scale out on its pads
//! (the notes on Push 2 spec, R32 and R37): In Key or Chromatic, Fixed, the
//! interval between rows and the direction they run.
//!
//! Saved with the set as settings of their own, so changing them is no undo
//! step.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The interval between a pad and the one above it (or beside it, with
/// Direction Horizontal): the Layout of the Scale menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(description = "A pad layout's row interval: `fourths`, `thirds` or `sequent`.")]
pub enum PadSpacing {
    /// 3 scale degrees (In Key) or 5 semitones (Chromatic).
    #[default]
    Fourths,
    /// 2 scale degrees or 4 semitones.
    Thirds,
    /// 8 steps: each row starts where the one below ends.
    Sequent,
}

impl PadSpacing {
    /// Every spacing, in the order the Scale menu lists them.
    pub const ALL: [Self; 3] = [Self::Fourths, Self::Thirds, Self::Sequent];

    /// How far up a row is, in degrees In Key or semitones Chromatic.
    #[must_use]
    pub const fn row(self, in_key: bool) -> i32 {
        match (self, in_key) {
            (Self::Fourths, true) => 3,
            (Self::Fourths, false) => 5,
            (Self::Thirds, true) => 2,
            (Self::Thirds, false) => 4,
            (Self::Sequent, _) => 8,
        }
    }

    /// The name the Scale menu shows.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fourths => "4ths",
            Self::Thirds => "3rds",
            Self::Sequent => "Sequent",
        }
    }
}

/// Which way the rows run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(description = "A pad layout's direction: `vertical` or `horizontal`.")]
pub enum PadDirection {
    /// Moving right is one step, moving up a row is the interval.
    #[default]
    Vertical,
    /// Rows and columns swap.
    Horizontal,
}

impl PadDirection {
    /// Both directions, in the order the Scale menu lists them.
    pub const ALL: [Self; 2] = [Self::Vertical, Self::Horizontal];

    /// The name the Scale menu shows.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Vertical => "Vertical",
            Self::Horizontal => "Horizontal",
        }
    }
}

/// The set's pad layout options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "How a controller's pads lay the scale out: `in_key` (pads play the scale's \
                   notes; off is Chromatic), `fixed` (the bottom-left pad stays on C), the row \
                   `spacing` and the `direction`."
)]
pub struct PadLayout {
    /// In Key (true) or Chromatic.
    pub in_key: bool,
    /// Whether the bottom-left pad stays on C whatever the scale's root.
    pub fixed: bool,
    /// The interval between rows.
    pub spacing: PadSpacing,
    /// Which way the rows run.
    pub direction: PadDirection,
}

impl Default for PadLayout {
    /// A new set's: In Key, 4ths, Vertical, Fixed off.
    fn default() -> Self {
        Self {
            in_key: true,
            fixed: false,
            spacing: PadSpacing::Fourths,
            direction: PadDirection::Vertical,
        }
    }
}

impl PadLayout {
    /// Whether these are a new set's, so a set file leaves them out.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
