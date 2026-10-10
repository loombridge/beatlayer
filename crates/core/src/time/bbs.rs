//! Bar-beat-sixteenth positions.

use core::fmt;
use core::str::FromStr;

use super::TimeError;

/// A position as bars, beats, and sixteenths, numbered from 1 the way Live's
/// position fields show it: tick zero is `1.1.1`.
///
/// There's no bar 0. The bar before bar 1 is bar −1, then −2, and so on, so
/// two beats before `1.1.1` in 4/4 is `-1.3.1`. As in Live, a bar of 0 typed
/// or given to [`TimeSignatureMap::from_bbs`](super::TimeSignatureMap::from_bbs)
/// is read as bar −1.
///
/// A beat is one note of the time signature's denominator, so 6/8 has six
/// eighth-note beats of two sixteenths each. Turn ticks into this with
/// [`TimeSignatureMap::to_bbs`](super::TimeSignatureMap::to_bbs) and back
/// with [`TimeSignatureMap::from_bbs`](super::TimeSignatureMap::from_bbs).
///
/// The display shows the three fields and drops [`BarBeatSixteenth::ticks`],
/// so a position partway through a sixteenth shows the sixteenth it's in.
/// The ordering is by bar, beat, sixteenth, then ticks, which is time order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BarBeatSixteenth {
    /// The bar, from 1 at tick zero. Bars before tick zero are numbered −1,
    /// −2, and so on; there's no bar 0.
    pub bar: i64,
    /// The beat in the bar, from 1 up to the signature's numerator.
    pub beat: u32,
    /// The sixteenth note in the beat, from 1 up to 16 divided by the
    /// signature's denominator.
    pub sixteenth: u32,
    /// Ticks past the start of the sixteenth, from 0 to 176,399.
    pub ticks: u32,
}

impl BarBeatSixteenth {
    /// The start of the arrangement, `1.1.1`.
    pub const START: Self = Self::new(1, 1, 1);

    /// The start of sixteenth `sixteenth` of beat `beat` of bar `bar`.
    #[must_use]
    pub const fn new(bar: i64, beat: u32, sixteenth: u32) -> Self {
        Self {
            bar,
            beat,
            sixteenth,
            ticks: 0,
        }
    }

    /// The zero-based index of the bar: 0 for bar 1, −1 for bar −1. Bar 0
    /// doesn't exist and is read as bar −1, as Live reads it.
    pub(crate) const fn bar_index(self) -> i64 {
        if self.bar > 0 {
            self.bar - 1
        } else if self.bar == 0 {
            -1
        } else {
            self.bar
        }
    }

    /// The bar number Live shows for the bar with zero-based `index`: one
    /// more from tick zero on, the index itself before it, so no bar is
    /// numbered 0.
    pub(crate) const fn bar_number(index: i64) -> i64 {
        if index >= 0 { index + 1 } else { index }
    }
}

impl fmt::Display for BarBeatSixteenth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.bar, self.beat, self.sixteenth)
    }
}

/// Reads `bar`, `bar.beat`, or `bar.beat.sixteenth`, such as `5.2.3`. Missing
/// fields are 1. The bar may be negative; the beat and sixteenth may not.
/// Bar 0 reads as bar −1, as Live reads it: `0.3.1` is `-1.3.1`. Whether the
/// beat and sixteenth exist in that bar is checked when the position is
/// turned into ticks.
impl FromStr for BarBeatSixteenth {
    type Err = TimeError;

    fn from_str(text: &str) -> Result<Self, TimeError> {
        let mut fields = text.trim().split('.');
        let bar = match fields.next().map(str::parse::<i64>) {
            Some(Ok(0)) => -1,
            Some(Ok(bar)) => bar,
            _ => return Err(TimeError::ParsePosition),
        };
        let mut next = || match fields.next() {
            None => Ok(1),
            Some(field) if field.starts_with('+') => Err(TimeError::ParsePosition),
            Some(field) => field.parse::<u32>().map_err(|_| TimeError::ParsePosition),
        };
        let beat = next()?;
        let sixteenth = next()?;
        if fields.next().is_some() {
            return Err(TimeError::ParsePosition);
        }
        Ok(Self::new(bar, beat, sixteenth))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        assert_eq!(BarBeatSixteenth::START.to_string(), "1.1.1");
        assert_eq!(BarBeatSixteenth::new(-2, 3, 4).to_string(), "-2.3.4");
        let mut late = BarBeatSixteenth::new(12, 4, 2);
        late.ticks = 100;
        assert_eq!(late.to_string(), "12.4.2");
    }

    #[test]
    fn parse() {
        assert_eq!("5.2.3".parse(), Ok(BarBeatSixteenth::new(5, 2, 3)));
        assert_eq!(" 5.2 ".parse(), Ok(BarBeatSixteenth::new(5, 2, 1)));
        assert_eq!("5".parse(), Ok(BarBeatSixteenth::new(5, 1, 1)));
        assert_eq!("-1.4.4".parse(), Ok(BarBeatSixteenth::new(-1, 4, 4)));
        for text in [
            "", ".", "5.", "5..1", "a.1.1", "1.-1.1", "1.+1.1", "1.1.1.1", "1 .1",
        ] {
            assert_eq!(
                text.parse::<BarBeatSixteenth>(),
                Err(TimeError::ParsePosition),
                "{text}"
            );
        }
    }

    /// Live has no bar 0: typing 0.3.1 gives -1.3.1 (observed in 12.4.3,
    /// 10 October 2026).
    #[test]
    fn bar_zero_reads_as_bar_minus_one() {
        assert_eq!("0.3.1".parse(), Ok(BarBeatSixteenth::new(-1, 3, 1)));
        assert_eq!("0".parse(), Ok(BarBeatSixteenth::new(-1, 1, 1)));
        assert_eq!("-0.2".parse(), Ok(BarBeatSixteenth::new(-1, 2, 1)));
        assert_eq!(
            "0.3.1"
                .parse::<BarBeatSixteenth>()
                .map(|position| position.to_string()),
            Ok("-1.3.1".to_owned())
        );
    }

    #[test]
    fn bar_indexes_skip_zero() {
        for (bar, index) in [(2, 1), (1, 0), (0, -1), (-1, -1), (-2, -2)] {
            assert_eq!(BarBeatSixteenth::new(bar, 1, 1).bar_index(), index, "{bar}");
        }
        for (index, bar) in [(1, 2), (0, 1), (-1, -1), (-2, -2)] {
            assert_eq!(BarBeatSixteenth::bar_number(index), bar, "{index}");
        }
    }

    #[test]
    fn order_is_time_order() {
        let mut a = BarBeatSixteenth::new(1, 4, 4);
        a.ticks = 176_399;
        assert!(a < BarBeatSixteenth::new(2, 1, 1));
        assert!(BarBeatSixteenth::new(-1, 4, 4) < BarBeatSixteenth::START);
        assert!(BarBeatSixteenth::new(-2, 4, 4) < BarBeatSixteenth::new(-1, 1, 1));
    }
}
