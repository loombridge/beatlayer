//! Fixed Length's settings, which the preferences keep
//! (`docs/specs/push-2-notes.md`, R50 and R51).

use bl_core::time::{Ticks, TimeSignature};

/// How long the clip a fixed-length recording makes is (R50).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    /// This many beats.
    Beats(u8),
    /// This many bars.
    Bars(u8),
}

/// The lengths the lower display buttons choose, shortest first (R50).
pub const LENGTHS: [Length; 8] = [
    Length::Beats(1),
    Length::Beats(2),
    Length::Bars(1),
    Length::Bars(2),
    Length::Bars(4),
    Length::Bars(8),
    Length::Bars(16),
    Length::Bars(32),
];

impl Length {
    /// The length's name on the display.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Beats(1) => "1 beat".to_owned(),
            Self::Beats(n) => format!("{n} beats"),
            Self::Bars(1) => "1 bar".to_owned(),
            Self::Bars(n) => format!("{n} bars"),
        }
    }

    /// How many ticks it lasts in `signature`.
    #[must_use]
    pub fn ticks(self, signature: TimeSignature) -> Ticks {
        match self {
            Self::Beats(n) => Ticks(signature.beat_length().get() * i64::from(n)),
            Self::Bars(n) => Ticks(signature.bar_length().get() * i64::from(n)),
        }
    }
}

/// Fixed Length's switch, length and Phrase Sync (R50): off at 1 bar with
/// Phrase Sync off in a new install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedLength {
    /// Whether a Session recording makes a clip of the length.
    pub on: bool,
    /// The length, as a place in [`LENGTHS`].
    pub length: u8,
    /// Whether the recording starts where the song position falls in a
    /// phrase of that length (R51).
    pub phrase_sync: bool,
}

impl Default for FixedLength {
    fn default() -> Self {
        Self {
            on: false,
            length: 2,
            phrase_sync: false,
        }
    }
}

impl FixedLength {
    /// The length it holds.
    #[must_use]
    pub fn value(self) -> Length {
        LENGTHS[usize::from(self.length).min(LENGTHS.len() - 1)]
    }

    /// The same with its length in range.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            length: self.length.min(LENGTHS.len() as u8 - 1),
            ..self
        }
    }
}
