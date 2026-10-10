//! Time signatures and the time-signature map.

use core::fmt;
use core::str::FromStr;
use std::sync::Arc;

use super::{BarBeatSixteenth, Ticks, TimeError};

/// A time signature: `numerator` beats to the bar, each one 1/`denominator`
/// of a whole note.
///
/// The numerator runs from 1 to 99 and the denominator is 1, 2, 4, 8, or 16,
/// the range Live accepts (see `docs/specs/musical-time.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(try_from = "serde_impl::RawTimeSignature")
)]
pub struct TimeSignature {
    numerator: u8,
    denominator: u8,
}

impl TimeSignature {
    /// Four quarter notes to the bar, the signature of a new set.
    pub const FOUR_FOUR: Self = Self {
        numerator: 4,
        denominator: 4,
    };

    /// A signature of `numerator` beats of 1/`denominator` notes.
    ///
    /// # Errors
    ///
    /// [`TimeError::InvalidTimeSignature`] unless the numerator is from 1 to
    /// 99 and the denominator is 1, 2, 4, 8, or 16.
    pub const fn new(numerator: u8, denominator: u8) -> Result<Self, TimeError> {
        let numerator_ok = numerator >= 1 && numerator <= 99;
        let denominator_ok = matches!(denominator, 1 | 2 | 4 | 8 | 16);
        if numerator_ok && denominator_ok {
            Ok(Self {
                numerator,
                denominator,
            })
        } else {
            Err(TimeError::InvalidTimeSignature {
                numerator,
                denominator,
            })
        }
    }

    /// Beats to the bar.
    #[must_use]
    pub const fn numerator(self) -> u8 {
        self.numerator
    }

    /// The note value of one beat, as a fraction of a whole note.
    #[must_use]
    pub const fn denominator(self) -> u8 {
        self.denominator
    }

    /// The length of one beat.
    #[must_use]
    pub const fn beat_length(self) -> Ticks {
        Ticks(Ticks::WHOLE.get() / self.denominator as i64)
    }

    /// The length of one full bar.
    #[must_use]
    pub const fn bar_length(self) -> Ticks {
        Ticks(self.beat_length().get() * self.numerator as i64)
    }

    /// Sixteenth notes in one beat: 16 divided by the denominator.
    #[must_use]
    pub const fn sixteenths_per_beat(self) -> u32 {
        16 / self.denominator as u32
    }
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self::FOUR_FOUR
    }
}

impl fmt::Display for TimeSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

/// Reads a signature such as `6/8`. As in Live, the two numbers may be
/// separated by a slash, comma, full stop, or spaces.
impl FromStr for TimeSignature {
    type Err = TimeError;

    fn from_str(text: &str) -> Result<Self, TimeError> {
        let invalid = TimeError::ParseTimeSignature;
        let text = text.trim();
        let (numerator, denominator) = text
            .split_once(['/', ',', '.'])
            .or_else(|| text.split_once(char::is_whitespace))
            .ok_or(invalid)?;
        let parse = |field: &str| field.trim().parse::<u8>().map_err(|_| invalid);
        Self::new(parse(numerator)?, parse(denominator)?)
    }
}

/// A time signature that takes effect at a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TimeSignatureChange {
    /// Where the signature takes effect. A new bar always starts here.
    pub at: Ticks,
    /// The signature from here to the next change.
    pub signature: TimeSignature,
}

/// A change with the number of its first bar, worked out once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Entry {
    change: TimeSignatureChange,
    /// Zero-based index of the bar that starts at `change.at`.
    first_bar: i64,
}

/// Time signatures against musical position: a list of changes from tick
/// zero on. Before tick zero the first signature holds.
///
/// Each change starts a new bar. A change that comes before the previous bar
/// is full cuts that bar short: Live calls it a fragmentary bar, and it still
/// counts as a bar. Bars are indexed from zero at tick zero here; the
/// bar-beat-sixteenth display numbers them from 1.
///
/// Lookups neither allocate nor lock. Cloning is cheap: the changes are
/// shared.
#[derive(Clone, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(
        try_from = "Vec<TimeSignatureChange>",
        into = "Vec<TimeSignatureChange>"
    )
)]
pub struct TimeSignatureMap {
    /// Never empty. The first is at tick zero and positions increase.
    entries: Arc<[Entry]>,
}

impl TimeSignatureMap {
    /// A map with one signature throughout.
    #[must_use]
    pub fn constant(signature: TimeSignature) -> Self {
        Self::build(&[TimeSignatureChange {
            at: Ticks::ZERO,
            signature,
        }])
    }

    /// A map from its changes.
    ///
    /// # Errors
    ///
    /// - [`TimeError::EmptyMap`] if there are no changes.
    /// - [`TimeError::FirstNotAtZero`] if the first isn't at tick zero.
    /// - [`TimeError::NotIncreasing`] if a change isn't after the one before
    ///   it.
    pub fn new(changes: impl IntoIterator<Item = TimeSignatureChange>) -> Result<Self, TimeError> {
        let changes: Vec<TimeSignatureChange> = changes.into_iter().collect();
        let Some(first) = changes.first() else {
            return Err(TimeError::EmptyMap);
        };
        if first.at != Ticks::ZERO {
            return Err(TimeError::FirstNotAtZero { start: first.at });
        }
        if let Some(index) = (1..changes.len()).find(|&i| changes[i].at <= changes[i - 1].at) {
            return Err(TimeError::NotIncreasing { index });
        }
        Ok(Self::build(&changes))
    }

    /// Works out the bar numbers of changes that are already valid.
    fn build(changes: &[TimeSignatureChange]) -> Self {
        let mut entries: Vec<Entry> = Vec::with_capacity(changes.len());
        for &change in changes {
            let first_bar = entries.last().map_or(0, |before| {
                let span = (change.at - before.change.at).get();
                let bar = before.change.signature.bar_length().get();
                // A cut-short bar still counts, so round up.
                before.first_bar + span / bar + i64::from(span % bar != 0)
            });
            entries.push(Entry { change, first_bar });
        }
        Self {
            entries: entries.into(),
        }
    }

    /// The changes, in order.
    pub fn changes(&self) -> impl ExactSizeIterator<Item = TimeSignatureChange> + '_ {
        self.entries.iter().map(|entry| entry.change)
    }

    /// The entry in force at `ticks`, which must not be negative.
    fn entry_at(&self, ticks: Ticks) -> &Entry {
        // The first entry is at zero, so the partition point is at least 1.
        &self.entries[self.entries.partition_point(|e| e.change.at <= ticks) - 1]
    }

    /// The entry in force in the bar with zero-based `index`, which must not
    /// be negative.
    fn entry_for_bar(&self, index: i64) -> &Entry {
        &self.entries[self.entries.partition_point(|e| e.first_bar <= index) - 1]
    }

    /// The signature in force at `ticks`.
    #[must_use]
    pub fn signature_at(&self, ticks: Ticks) -> TimeSignature {
        if ticks < Ticks::ZERO {
            return self.entries[0].change.signature;
        }
        self.entry_at(ticks).change.signature
    }

    /// The zero-based index of the bar holding `ticks`, and how far into
    /// that bar `ticks` is. Bars before tick zero have negative indexes.
    #[must_use]
    pub fn bar_at(&self, ticks: Ticks) -> (i64, Ticks) {
        let entry = if ticks < Ticks::ZERO {
            &self.entries[0]
        } else {
            self.entry_at(ticks)
        };
        let offset = ticks - entry.change.at;
        let bar = entry.change.signature.bar_length();
        (
            entry.first_bar + offset.div_euclid(bar),
            offset.rem_euclid(bar),
        )
    }

    /// Where the bar with zero-based `index` starts. Saturates far beyond
    /// the range of [`Ticks`].
    #[must_use]
    pub fn bar_start(&self, index: i64) -> Ticks {
        let entry = if index < 0 {
            &self.entries[0]
        } else {
            self.entry_for_bar(index)
        };
        let bar = entry.change.signature.bar_length().get();
        let bars = index.saturating_sub(entry.first_bar);
        entry
            .change
            .at
            .saturating_add(Ticks(bars.saturating_mul(bar)))
    }

    /// `ticks` as bars, beats, and sixteenths, numbered from 1 as Live
    /// displays them. A beat is one note of the denominator; anything finer
    /// than a sixteenth is kept in [`BarBeatSixteenth::ticks`].
    #[must_use]
    pub fn to_bbs(&self, ticks: Ticks) -> BarBeatSixteenth {
        let (bar, into_bar) = self.bar_at(ticks);
        let signature = self.signature_at(ticks);
        let beat = signature.beat_length();
        let into_beat = into_bar % beat;
        BarBeatSixteenth {
            // Bar indexes are at most i64::MAX / 176,400, so this can't
            // overflow.
            bar: bar + 1,
            // These quotients are below 99, 16, and 176,400.
            beat: (into_bar / beat) as u32 + 1,
            sixteenth: (into_beat / Ticks::SIXTEENTH) as u32 + 1,
            ticks: (into_beat % Ticks::SIXTEENTH).get() as u32,
        }
    }

    /// The position of a bar-beat-sixteenth value.
    ///
    /// # Errors
    ///
    /// [`TimeError::PositionOutOfRange`] if the beat, sixteenth, or tick
    /// doesn't exist in that bar: a beat of zero or past the numerator, a
    /// sixteenth of zero or past the beat, a tick past the sixteenth, or a
    /// point past the end of a cut-short bar. Also if the position is beyond
    /// the range of [`Ticks`].
    pub fn from_bbs(&self, position: BarBeatSixteenth) -> Result<Ticks, TimeError> {
        let out_of_range = TimeError::PositionOutOfRange;
        let index = position.bar.checked_sub(1).ok_or(out_of_range)?;
        let start = self.bar_start(index);
        if start == Ticks::MIN || start == Ticks::MAX {
            return Err(out_of_range);
        }
        let signature = self.signature_at(start);
        let BarBeatSixteenth {
            beat,
            sixteenth,
            ticks,
            ..
        } = position;
        if !(1..=u32::from(signature.numerator())).contains(&beat)
            || !(1..=signature.sixteenths_per_beat()).contains(&sixteenth)
            || i64::from(ticks) >= Ticks::SIXTEENTH.get()
        {
            return Err(out_of_range);
        }
        let into_bar = signature.beat_length() * i64::from(beat - 1)
            + Ticks::SIXTEENTH * i64::from(sixteenth - 1)
            + Ticks(i64::from(ticks));
        let at = start.checked_add(into_bar).ok_or(out_of_range)?;
        // A cut-short bar ends where the next one starts.
        if index < i64::MAX && at >= self.bar_start(index + 1) {
            return Err(out_of_range);
        }
        Ok(at)
    }

    /// Reads a position such as `5.2.3` and returns its ticks. See
    /// [`BarBeatSixteenth`]'s `FromStr` for the forms it accepts.
    ///
    /// # Errors
    ///
    /// [`TimeError::ParsePosition`] if the text isn't a position, or
    /// [`TimeError::PositionOutOfRange`] as for
    /// [`TimeSignatureMap::from_bbs`].
    pub fn parse_position(&self, text: &str) -> Result<Ticks, TimeError> {
        self.from_bbs(text.parse()?)
    }
}

impl Default for TimeSignatureMap {
    /// 4/4 throughout.
    fn default() -> Self {
        Self::constant(TimeSignature::FOUR_FOUR)
    }
}

impl fmt::Debug for TimeSignatureMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.changes()).finish()
    }
}

impl TryFrom<Vec<TimeSignatureChange>> for TimeSignatureMap {
    type Error = TimeError;
    fn try_from(changes: Vec<TimeSignatureChange>) -> Result<Self, TimeError> {
        Self::new(changes)
    }
}

impl From<TimeSignatureMap> for Vec<TimeSignatureChange> {
    fn from(map: TimeSignatureMap) -> Self {
        map.changes().collect()
    }
}

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{TimeError, TimeSignature};

    /// The serialised form of a signature, checked on the way in.
    #[derive(serde::Deserialize)]
    pub(super) struct RawTimeSignature {
        numerator: u8,
        denominator: u8,
    }

    impl TryFrom<RawTimeSignature> for TimeSignature {
        type Error = TimeError;
        fn try_from(raw: RawTimeSignature) -> Result<Self, TimeError> {
            Self::new(raw.numerator, raw.denominator)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(numerator: u8, denominator: u8) -> TimeSignature {
        TimeSignature::new(numerator, denominator).unwrap()
    }

    fn q(quarters: i64) -> Ticks {
        Ticks::from_quarters(quarters)
    }

    fn bbs(bar: i64, beat: u32, sixteenth: u32) -> BarBeatSixteenth {
        BarBeatSixteenth::new(bar, beat, sixteenth)
    }

    #[test]
    fn signatures() {
        assert_eq!(sig(4, 4).bar_length(), Ticks::WHOLE);
        assert_eq!(sig(6, 8).beat_length(), Ticks::QUARTER / 2);
        assert_eq!(sig(6, 8).bar_length(), q(3));
        assert_eq!(sig(7, 16).sixteenths_per_beat(), 1);
        assert_eq!(sig(3, 1).sixteenths_per_beat(), 16);
        assert_eq!(sig(99, 16).to_string(), "99/16");
        for (numerator, denominator) in [(0, 4), (100, 4), (4, 3), (4, 32), (4, 0)] {
            assert_eq!(
                TimeSignature::new(numerator, denominator),
                Err(TimeError::InvalidTimeSignature {
                    numerator,
                    denominator
                })
            );
        }
    }

    #[test]
    fn parse_signatures() {
        for text in ["6/8", "6,8", "6.8", "6 8", " 6 / 8 "] {
            assert_eq!(text.parse(), Ok(sig(6, 8)), "{text}");
        }
        for text in ["", "6", "6/", "/8", "6/9", "x/8", "300/4"] {
            assert!(text.parse::<TimeSignature>().is_err(), "{text}");
        }
    }

    #[test]
    fn four_four() {
        let map = TimeSignatureMap::default();
        assert_eq!(map.to_bbs(Ticks::ZERO), bbs(1, 1, 1));
        assert_eq!(map.to_bbs(Ticks::ZERO).to_string(), "1.1.1");
        assert_eq!(map.to_bbs(q(1)), bbs(1, 2, 1));
        assert_eq!(map.to_bbs(Ticks::SIXTEENTH * 3), bbs(1, 1, 4));
        assert_eq!(map.to_bbs(q(4)), bbs(2, 1, 1));
        assert_eq!(
            map.to_bbs(q(4 * 7 + 2) + Ticks::SIXTEENTH * 2).to_string(),
            "8.3.3"
        );
        let late = map.to_bbs(Ticks::SIXTEENTH + Ticks(5));
        assert_eq!((late.sixteenth, late.ticks), (2, 5));
        assert_eq!(map.from_bbs(bbs(8, 3, 3)), Ok(q(30) + Ticks::SIXTEENTH * 2));
        assert_eq!(
            map.parse_position("8.3.3"),
            Ok(q(30) + Ticks::SIXTEENTH * 2)
        );
        assert_eq!(map.parse_position("3"), Ok(q(8)));
    }

    #[test]
    fn before_zero() {
        let map = TimeSignatureMap::default();
        assert_eq!(map.to_bbs(Ticks(-1)).to_string(), "0.4.4");
        assert_eq!(map.to_bbs(q(-4)), bbs(0, 1, 1));
        assert_eq!(map.to_bbs(q(-5)), bbs(-1, 4, 1));
        assert_eq!(map.bar_start(-1), q(-4));
        assert_eq!(map.from_bbs(bbs(-1, 4, 1)), Ok(q(-5)));
    }

    #[test]
    fn other_denominators() {
        let six_eight = TimeSignatureMap::constant(sig(6, 8));
        assert_eq!(six_eight.to_bbs(q(1)), bbs(1, 3, 1));
        assert_eq!(six_eight.to_bbs(Ticks::SIXTEENTH * 11), bbs(1, 6, 2));
        assert_eq!(six_eight.to_bbs(q(3)), bbs(2, 1, 1));
        assert_eq!(
            six_eight.from_bbs(bbs(1, 6, 3)),
            Err(TimeError::PositionOutOfRange)
        );

        let cut_time = TimeSignatureMap::constant(sig(2, 2));
        assert_eq!(cut_time.to_bbs(Ticks::SIXTEENTH * 15), bbs(1, 2, 8));
    }

    #[test]
    fn changes_start_bars() {
        // 4/4 for two bars, then 3/4 for two bars, then 7/8.
        let map = TimeSignatureMap::new([
            TimeSignatureChange {
                at: Ticks::ZERO,
                signature: sig(4, 4),
            },
            TimeSignatureChange {
                at: q(8),
                signature: sig(3, 4),
            },
            TimeSignatureChange {
                at: q(14),
                signature: sig(7, 8),
            },
        ])
        .unwrap();
        assert_eq!(map.to_bbs(q(8)), bbs(3, 1, 1));
        assert_eq!(map.to_bbs(q(11)), bbs(4, 1, 1));
        assert_eq!(map.to_bbs(q(14)), bbs(5, 1, 1));
        assert_eq!(map.to_bbs(q(17)), bbs(5, 7, 1));
        assert_eq!(map.bar_start(4), q(14));
        assert_eq!(map.bar_start(5), q(14) + Ticks::QUARTER * 7 / 2);
        assert_eq!(map.signature_at(q(13)), sig(3, 4));
    }

    #[test]
    fn fragmentary_bars() {
        // 4/4, then 3/4 two beats into bar 2: bar 2 is cut to two beats.
        let map = TimeSignatureMap::new([
            TimeSignatureChange {
                at: Ticks::ZERO,
                signature: sig(4, 4),
            },
            TimeSignatureChange {
                at: q(6),
                signature: sig(3, 4),
            },
        ])
        .unwrap();
        assert_eq!(map.to_bbs(q(5)), bbs(2, 2, 1));
        assert_eq!(map.to_bbs(q(6)), bbs(3, 1, 1));
        assert_eq!(map.bar_start(1), q(4));
        assert_eq!(map.bar_start(2), q(6));
        assert_eq!(map.from_bbs(bbs(2, 2, 4)), Ok(q(6) - Ticks::SIXTEENTH));
        assert_eq!(
            map.from_bbs(bbs(2, 3, 1)),
            Err(TimeError::PositionOutOfRange)
        );
    }

    #[test]
    fn out_of_range_positions() {
        let map = TimeSignatureMap::default();
        let mut tick_too_far = bbs(1, 1, 1);
        tick_too_far.ticks = 176_400;
        for position in [
            bbs(1, 0, 1),
            bbs(1, 5, 1),
            bbs(1, 1, 0),
            bbs(1, 1, 5),
            tick_too_far,
        ] {
            assert_eq!(map.from_bbs(position), Err(TimeError::PositionOutOfRange));
        }
        assert_eq!(
            map.from_bbs(bbs(i64::MAX, 1, 1)),
            Err(TimeError::PositionOutOfRange)
        );
        assert_eq!(
            map.from_bbs(bbs(i64::MIN, 1, 1)),
            Err(TimeError::PositionOutOfRange)
        );
    }

    #[test]
    fn invalid_maps() {
        let at = |at| TimeSignatureChange {
            at,
            signature: sig(4, 4),
        };
        assert_eq!(TimeSignatureMap::new([]), Err(TimeError::EmptyMap));
        assert_eq!(
            TimeSignatureMap::new([at(q(1))]),
            Err(TimeError::FirstNotAtZero { start: q(1) })
        );
        assert_eq!(
            TimeSignatureMap::new([at(Ticks::ZERO), at(Ticks::ZERO)]),
            Err(TimeError::NotIncreasing { index: 1 })
        );
    }
}
