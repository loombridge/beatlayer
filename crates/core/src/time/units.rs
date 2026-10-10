//! Ticks, frames, and sample rates.

use core::fmt;
use core::iter::Sum;
use core::num::NonZeroU32;
use core::ops::{Add, AddAssign, Div, Mul, Neg, Rem, Sub, SubAssign};

/// Ticks in one quarter note: 705,600 = 2⁶ × 3² × 5² × 7².
///
/// The factors put 1/64 notes and triplet, quintuplet, and septuplet grids on
/// whole ticks, so musical positions and lengths are exact integers.
pub const TICKS_PER_QUARTER: i64 = 705_600;

/// A musical position or length, in ticks of [`TICKS_PER_QUARTER`] to the
/// quarter note.
///
/// Positions count from tick zero, the start of the arrangement. Negative
/// positions are allowed and lie before it.
///
/// Arithmetic follows `i64`: it panics on overflow in debug builds and wraps
/// in release builds. Use the `checked_` and `saturating_` methods where a
/// value can come from outside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
#[repr(transparent)]
pub struct Ticks(pub i64);

/// A position or length on a device's sample clock, in frames.
///
/// A frame is one sample on every channel. Frame zero is tick zero, so the
/// tempo map converts between the two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
#[repr(transparent)]
pub struct Frames(pub i64);

macro_rules! integer_time {
    ($name:ident, $unit:literal) => {
        impl $name {
            /// Zero.
            pub const ZERO: Self = Self(0);
            /// The smallest value.
            pub const MIN: Self = Self(i64::MIN);
            /// The largest value.
            pub const MAX: Self = Self(i64::MAX);

            /// The raw count.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }

            /// The absolute value.
            #[must_use]
            pub const fn abs(self) -> Self {
                Self(self.0.abs())
            }

            /// Whether the value is below zero.
            #[must_use]
            pub const fn is_negative(self) -> bool {
                self.0 < 0
            }

            /// Addition that returns `None` on overflow.
            #[must_use]
            pub const fn checked_add(self, rhs: Self) -> Option<Self> {
                match self.0.checked_add(rhs.0) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }

            /// Subtraction that returns `None` on overflow.
            #[must_use]
            pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
                match self.0.checked_sub(rhs.0) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }

            /// Addition that stops at [`Self::MIN`] or [`Self::MAX`].
            #[must_use]
            pub const fn saturating_add(self, rhs: Self) -> Self {
                Self(self.0.saturating_add(rhs.0))
            }

            /// Subtraction that stops at [`Self::MIN`] or [`Self::MAX`].
            #[must_use]
            pub const fn saturating_sub(self, rhs: Self) -> Self {
                Self(self.0.saturating_sub(rhs.0))
            }

            /// How many whole `rhs` fit in `self`, rounding towards negative
            /// infinity, so that `self.rem_euclid(rhs)` is never negative.
            ///
            /// # Panics
            ///
            /// If `rhs` is zero.
            #[must_use]
            pub const fn div_euclid(self, rhs: Self) -> i64 {
                self.0.div_euclid(rhs.0)
            }

            /// The remainder after [`Self::div_euclid`], from zero up to
            /// `rhs.abs()`.
            ///
            /// # Panics
            ///
            /// If `rhs` is zero.
            #[must_use]
            pub const fn rem_euclid(self, rhs: Self) -> Self {
                Self(self.0.rem_euclid(rhs.0))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} {}", self.0, $unit)
            }
        }

        impl Add for $name {
            type Output = Self;
            fn add(self, rhs: Self) -> Self {
                Self(self.0 + rhs.0)
            }
        }

        impl AddAssign for $name {
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0;
            }
        }

        impl Sub for $name {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self {
                Self(self.0 - rhs.0)
            }
        }

        impl SubAssign for $name {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0;
            }
        }

        impl Neg for $name {
            type Output = Self;
            fn neg(self) -> Self {
                Self(-self.0)
            }
        }

        impl Mul<i64> for $name {
            type Output = Self;
            fn mul(self, rhs: i64) -> Self {
                Self(self.0 * rhs)
            }
        }

        impl Mul<$name> for i64 {
            type Output = $name;
            fn mul(self, rhs: $name) -> $name {
                $name(self * rhs.0)
            }
        }

        /// Division by a count, rounding towards zero.
        impl Div<i64> for $name {
            type Output = Self;
            fn div(self, rhs: i64) -> Self {
                Self(self.0 / rhs)
            }
        }

        /// How many times one value fits in another, rounding towards zero.
        impl Div for $name {
            type Output = i64;
            fn div(self, rhs: Self) -> i64 {
                self.0 / rhs.0
            }
        }

        /// The remainder of division, with the sign of the dividend.
        impl Rem for $name {
            type Output = Self;
            fn rem(self, rhs: Self) -> Self {
                Self(self.0 % rhs.0)
            }
        }

        impl Sum for $name {
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
                Self(iter.map(|v| v.0).sum())
            }
        }

        impl<'a> Sum<&'a $name> for $name {
            fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
                Self(iter.map(|v| v.0).sum())
            }
        }
    };
}

integer_time!(Ticks, "ticks");
integer_time!(Frames, "frames");

impl Ticks {
    /// One quarter note.
    pub const QUARTER: Self = Self(TICKS_PER_QUARTER);
    /// One sixteenth note, the smallest unit of a bar-beat-sixteenth position.
    pub const SIXTEENTH: Self = Self(TICKS_PER_QUARTER / 4);
    /// One whole note.
    pub const WHOLE: Self = Self(TICKS_PER_QUARTER * 4);

    /// `quarters` quarter notes.
    #[must_use]
    pub const fn from_quarters(quarters: i64) -> Self {
        Self(quarters * TICKS_PER_QUARTER)
    }

    /// The value in quarter notes, for display and tests. Exact while the
    /// tick count fits in an `f64` mantissa (below 2⁵³).
    #[must_use]
    pub fn as_quarters(self) -> f64 {
        self.0 as f64 / TICKS_PER_QUARTER as f64
    }
}

/// A device sample rate in hertz, never zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
pub struct SampleRate(NonZeroU32);

impl SampleRate {
    /// 44.1 kHz.
    pub const HZ_44100: Self = Self::from_const(44_100);
    /// 48 kHz.
    pub const HZ_48000: Self = Self::from_const(48_000);
    /// 88.2 kHz.
    pub const HZ_88200: Self = Self::from_const(88_200);
    /// 96 kHz.
    pub const HZ_96000: Self = Self::from_const(96_000);
    /// 176.4 kHz.
    pub const HZ_176400: Self = Self::from_const(176_400);
    /// 192 kHz.
    pub const HZ_192000: Self = Self::from_const(192_000);

    const fn from_const(hz: u32) -> Self {
        match NonZeroU32::new(hz) {
            Some(hz) => Self(hz),
            None => panic!("a sample rate can't be zero"),
        }
    }

    /// A rate of `hz` frames per second, or `None` if `hz` is zero.
    #[must_use]
    pub const fn new(hz: u32) -> Option<Self> {
        match NonZeroU32::new(hz) {
            Some(hz) => Some(Self(hz)),
            None => None,
        }
    }

    /// Frames per second.
    #[must_use]
    pub const fn hz(self) -> u32 {
        self.0.get()
    }

    /// Frames per second as an `f64`.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.0.get())
    }

    /// The frame nearest to `seconds`, with halves rounded away from zero.
    /// Values beyond the range of [`Frames`] saturate, and NaN gives frame
    /// zero.
    #[must_use]
    pub fn seconds_to_frames(self, seconds: f64) -> Frames {
        // `as` saturates and maps NaN to zero, as documented.
        Frames((seconds * self.as_f64()).round() as i64)
    }

    /// The time of `frames` in seconds.
    #[must_use]
    pub fn frames_to_seconds(self, frames: Frames) -> f64 {
        frames.0 as f64 / self.as_f64()
    }
}

impl fmt::Display for SampleRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} Hz", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_per_quarter_divides_the_common_grids() {
        // 1/64 notes, and triplet, quintuplet, and septuplet divisions of
        // every power-of-two note down to a 1/64, land on whole ticks.
        let whole = Ticks::WHOLE.get();
        for power in [1, 2, 4, 8, 16, 32, 64] {
            for tuplet in [1, 3, 5, 7] {
                let division = power * tuplet;
                assert_eq!(whole % division, 0, "1/{power} in groups of {tuplet}");
            }
        }
        assert_eq!(Ticks::SIXTEENTH.get(), 176_400);
    }

    #[test]
    fn arithmetic() {
        let q = Ticks::QUARTER;
        assert_eq!(q + q, Ticks::from_quarters(2));
        assert_eq!(q * 3 - q, 2 * q);
        assert_eq!(-q, Ticks(-TICKS_PER_QUARTER));
        assert_eq!(Ticks::WHOLE / 4, q);
        assert_eq!(Ticks::WHOLE / q, 4);
        assert_eq!(Ticks(7) % Ticks(4), Ticks(3));
        assert_eq!(Ticks(-1).div_euclid(Ticks(4)), -1);
        assert_eq!(Ticks(-1).rem_euclid(Ticks(4)), Ticks(3));
        assert_eq!(Ticks::MAX.checked_add(Ticks(1)), None);
        assert_eq!(Ticks::MAX.saturating_add(Ticks(1)), Ticks::MAX);
        assert_eq!([q, q, q].iter().sum::<Ticks>(), Ticks::from_quarters(3));
        assert_eq!(Ticks::from_quarters(3).as_quarters(), 3.0);
        assert_eq!(Ticks(5).to_string(), "5 ticks");
        assert_eq!(Frames(5).to_string(), "5 frames");
    }

    #[test]
    fn sample_rate() {
        assert_eq!(SampleRate::new(0), None);
        let rate = SampleRate::HZ_48000;
        assert_eq!(rate.hz(), 48_000);
        assert_eq!(rate.to_string(), "48000 Hz");
        assert_eq!(rate.seconds_to_frames(1.5), Frames(72_000));
        assert_eq!(rate.seconds_to_frames(-0.5 / 48_000.0), Frames(-1));
        assert_eq!(rate.seconds_to_frames(f64::NAN), Frames::ZERO);
        assert_eq!(rate.seconds_to_frames(f64::INFINITY), Frames::MAX);
        assert_eq!(rate.frames_to_seconds(Frames(24_000)), 0.5);
    }
}
