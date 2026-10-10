//! The error type for building time values.

use core::fmt;

use super::Ticks;

/// Why a tempo, time signature, map, or position couldn't be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TimeError {
    /// A tempo that isn't a finite number from [`Bpm::MIN`](super::Bpm::MIN)
    /// to [`Bpm::MAX`](super::Bpm::MAX).
    BpmOutOfRange,
    /// A time signature whose numerator isn't from 1 to 99 or whose
    /// denominator isn't 1, 2, 4, 8, or 16.
    InvalidTimeSignature {
        /// The numerator given.
        numerator: u8,
        /// The denominator given.
        denominator: u8,
    },
    /// Text that isn't a time signature such as 4/4.
    ParseTimeSignature,
    /// A map with nothing in it.
    EmptyMap,
    /// A map whose first entry doesn't start at tick zero.
    FirstNotAtZero {
        /// Where the first entry starts.
        start: Ticks,
    },
    /// A map entry that doesn't start after the entry before it.
    NotIncreasing {
        /// The index of the entry that's out of order.
        index: usize,
    },
    /// A tempo breakpoint before tick zero.
    NegativePosition {
        /// Where the breakpoint is.
        at: Ticks,
    },
    /// A tempo ramp as the last segment, which has nothing to ramp to.
    RampAtEnd,
    /// A bar-beat-sixteenth position whose beat, sixteenth, or tick lies
    /// outside its bar.
    PositionOutOfRange,
    /// Text that isn't a bar-beat-sixteenth position.
    ParsePosition,
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BpmOutOfRange => f.write_str("tempo out of range"),
            Self::InvalidTimeSignature {
                numerator,
                denominator,
            } => write!(
                f,
                "invalid time signature {numerator}/{denominator}: the numerator must be \
                 1 to 99 and the denominator 1, 2, 4, 8, or 16"
            ),
            Self::ParseTimeSignature => f.write_str("not a time signature such as 4/4"),
            Self::EmptyMap => f.write_str("the map has no entries"),
            Self::FirstNotAtZero { start } => {
                write!(f, "the first entry starts at {start}, not at tick zero")
            }
            Self::NotIncreasing { index } => {
                write!(f, "entry {index} doesn't start after the entry before it")
            }
            Self::NegativePosition { at } => write!(f, "breakpoint at {at}, before tick zero"),
            Self::RampAtEnd => f.write_str("the last tempo segment is a ramp with no end"),
            Self::PositionOutOfRange => f.write_str("the position lies outside its bar"),
            Self::ParsePosition => f.write_str("not a bar-beat-sixteenth position such as 1.1.1"),
        }
    }
}

impl std::error::Error for TimeError {}
