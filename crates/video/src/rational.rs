//! Exact times (R2).

use std::cmp::Ordering;
use std::fmt;

/// A time in seconds as an exact fraction, so a frame time such as
/// 1001/30000 s never rounds and a comparison of two times never errs.
///
/// Always in lowest terms with a positive denominator. Numerators and
/// denominators that a movie's 32-bit time scales and 64-bit durations can
/// produce fit; a result that doesn't fit saturates, which only a damaged
/// file reaches.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i64,
    den: u64,
}

impl Rational {
    /// Zero.
    pub const ZERO: Self = Self { num: 0, den: 1 };

    /// `num / den` seconds. A zero denominator gives zero.
    #[must_use]
    pub fn new(num: i128, den: i128) -> Self {
        if den == 0 {
            return Self::ZERO;
        }
        let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
        let divisor = gcd(num.unsigned_abs(), den.unsigned_abs()).max(1);
        let num = num / divisor as i128;
        let den = den / divisor as i128;
        Self {
            num: i64::try_from(num).unwrap_or(if num < 0 { i64::MIN } else { i64::MAX }),
            den: u64::try_from(den).unwrap_or(u64::MAX),
        }
    }

    /// The numerator, in lowest terms.
    #[must_use]
    pub fn numer(self) -> i64 {
        self.num
    }

    /// The denominator, in lowest terms, above zero.
    #[must_use]
    pub fn denom(self) -> u64 {
        self.den
    }

    /// The time in seconds, rounded.
    #[must_use]
    pub fn seconds(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// The sum of two times.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        let num = (i128::from(self.num) * i128::from(other.den))
            .checked_add(i128::from(other.num) * i128::from(self.den));
        match num {
            Some(num) => Self::new(num, i128::from(self.den) * i128::from(other.den)),
            // Only two saturated times from a damaged file add up past 2^127.
            None => Self {
                num: if self.num < 0 { i64::MIN } else { i64::MAX },
                den: 1,
            },
        }
    }

    /// The difference of two times.
    #[must_use]
    pub fn minus(self, other: Self) -> Self {
        self.plus(Self::new(-i128::from(other.num), i128::from(other.den)))
    }

    /// This time in units of `1 / scale` second, rounded down.
    #[must_use]
    pub fn floor_units(self, scale: u32) -> i128 {
        (i128::from(self.num) * i128::from(scale)).div_euclid(i128::from(self.den))
    }

    /// This time in units of `1 / scale` second, rounded to nearest, halves up.
    #[must_use]
    pub fn round_units(self, scale: u32) -> i128 {
        let den = i128::from(self.den);
        (2 * i128::from(self.num) * i128::from(scale) + den).div_euclid(2 * den)
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        (i128::from(self.num) * i128::from(other.den))
            .cmp(&(i128::from(other.num) * i128::from(self.den)))
    }
}

impl fmt::Debug for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{} s", self.num, self.den)
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6} s", self.seconds())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_and_compares_exactly() {
        assert_eq!(Rational::new(2002, 60000), Rational::new(1001, 30000));
        assert!(Rational::new(1, 3) > Rational::new(333_333_333, 1_000_000_000));
        assert_eq!(Rational::new(-1, -2), Rational::new(1, 2));
    }

    #[test]
    fn converts_to_units() {
        let t = Rational::new(1001, 30000);
        assert_eq!(t.floor_units(30000), 1001);
        assert_eq!(t.floor_units(1000), 33);
        assert_eq!(t.round_units(1000), 33);
        assert_eq!(Rational::new(-1, 3).floor_units(1), -1);
        assert_eq!(Rational::new(1, 2).round_units(1), 1);
    }
}
