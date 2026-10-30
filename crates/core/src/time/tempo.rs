//! Tempo and the tempo map.

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use std::sync::Arc;

use super::{Frames, SampleRate, TICKS_PER_QUARTER, Ticks, TimeError};

/// A tempo in quarter notes per minute, from [`Bpm::MIN`] to [`Bpm::MAX`].
///
/// A beat here is always a quarter note, whatever the time signature.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(try_from = "f64", into = "f64")
)]
pub struct Bpm(f64);

impl Bpm {
    /// The slowest tempo the type allows. The UI may allow less.
    pub const MIN: Self = Self(1.0);
    /// The fastest tempo the type allows. The UI may allow less.
    pub const MAX: Self = Self(1_000.0);
    /// The tempo of a new set: 120 BPM.
    pub const DEFAULT: Self = Self(120.0);

    /// A tempo of `bpm` quarter notes per minute.
    ///
    /// # Errors
    ///
    /// [`TimeError::BpmOutOfRange`] if `bpm` is NaN or outside
    /// [`Bpm::MIN`]..=[`Bpm::MAX`].
    pub fn new(bpm: f64) -> Result<Self, TimeError> {
        if (Self::MIN.0..=Self::MAX.0).contains(&bpm) {
            Ok(Self(bpm))
        } else {
            Err(TimeError::BpmOutOfRange)
        }
    }

    /// Quarter notes per minute.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }

    /// Seconds per tick at this tempo.
    fn seconds_per_tick(self) -> f64 {
        60.0 / (self.0 * TICKS_PER_QUARTER as f64)
    }
}

impl Default for Bpm {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// A `Bpm` is never NaN and never zero, so `==` on the `f64` is an
// equivalence relation and agrees with comparing bit patterns.
impl Eq for Bpm {}

impl PartialOrd for Bpm {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Bpm {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Hash for Bpm {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl TryFrom<f64> for Bpm {
    type Error = TimeError;
    fn try_from(bpm: f64) -> Result<Self, TimeError> {
        Self::new(bpm)
    }
}

impl From<Bpm> for f64 {
    fn from(bpm: Bpm) -> f64 {
        bpm.0
    }
}

impl fmt::Display for Bpm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} BPM", self.0)
    }
}

/// One piece of a [`TempoMap`]. A segment runs from its start to the next
/// segment's start; the last segment runs on for ever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "kind", rename_all = "snake_case")
)]
pub enum TempoSegment {
    /// A steady tempo.
    Constant {
        /// Where the segment starts.
        start: Ticks,
        /// The tempo.
        bpm: Bpm,
    },
    /// A tempo that changes in a straight line against musical position,
    /// from `from` at `start` to `to` at the next segment's start. The next
    /// segment may start at a different tempo, which makes a jump.
    Ramp {
        /// Where the segment starts.
        start: Ticks,
        /// The tempo at the start.
        from: Bpm,
        /// The tempo the ramp reaches at the next segment's start.
        to: Bpm,
    },
}

impl TempoSegment {
    /// Where the segment starts.
    #[must_use]
    pub const fn start(self) -> Ticks {
        match self {
            Self::Constant { start, .. } | Self::Ramp { start, .. } => start,
        }
    }

    /// The tempo at the start of the segment.
    #[must_use]
    pub const fn start_bpm(self) -> Bpm {
        match self {
            Self::Constant { bpm, .. } => bpm,
            Self::Ramp { from, .. } => from,
        }
    }

    /// The tempo the segment reaches at its end.
    #[must_use]
    pub const fn end_bpm(self) -> Bpm {
        match self {
            Self::Constant { bpm, .. } => bpm,
            Self::Ramp { to, .. } => to,
        }
    }
}

/// A segment with the values conversions need, worked out once.
#[derive(Clone, Copy, Debug)]
struct Piece {
    segment: TempoSegment,
    /// Seconds from tick zero to the segment's start.
    start_seconds: f64,
    /// Seconds per tick at the segment's start.
    seconds_per_tick: f64,
    /// The tempo's relative change per tick: at `τ` ticks into the segment,
    /// the tempo is `start_bpm × (1 + slope × τ)`. Zero when constant.
    slope: f64,
    /// Ticks to the next segment's start, or `i64::MAX` for the last.
    length: i64,
}

impl Piece {
    /// Seconds from the segment's start to `tau` ticks into it.
    ///
    /// With a tempo of `b₀ (1 + cτ)` quarter notes per minute, the time to
    /// `τ` is the integral of `s₀ / (1 + cx)` from 0 to `τ`, where `s₀` is
    /// seconds per tick at the start: `s₀ ln(1 + cτ) / c`. Written as
    /// `s₀ τ · ln(1 + x) / x` with `x = cτ`, it stays exact as `c` nears zero.
    fn seconds_at(&self, tau: f64) -> f64 {
        self.seconds_per_tick * tau * ln_1p_ratio(self.slope * tau)
    }

    /// The inverse of [`Piece::seconds_at`]: ticks from the segment's start
    /// to `seconds` into it, unrounded.
    ///
    /// With `u = seconds / s₀`, `τ = (e^(cu) − 1) / c = u · (e^y − 1) / y`
    /// where `y = cu`.
    fn ticks_at(&self, seconds: f64) -> f64 {
        let u = seconds / self.seconds_per_tick;
        u * exp_m1_ratio(self.slope * u)
    }

    /// The whole number of ticks from the segment's start that's nearest in
    /// time to `seconds` into it, which must not be negative. Halves round
    /// up. Saturates at `i64::MAX`.
    fn nearest_tick(&self, seconds: f64) -> i64 {
        // Rounding can put a time a hair past the end of its segment.
        let length = self.length as f64;
        let tau = self.ticks_at(seconds).min(length);
        if self.slope == 0.0 {
            return round_to_i64(tau);
        }
        // On a ramp, neighbouring ticks differ in length, so where the tempo
        // changes noticeably within a tick, the tick nearest in time can
        // differ from the whole number nearest to `tau`. Compare the two
        // candidates' times. The segment ends before the next tick past it.
        let below = tau.floor();
        let above = (below + 1.0).min(length);
        let after = self.seconds_at(above) - seconds;
        let before = seconds - self.seconds_at(below);
        round_to_i64(if after <= before { above } else { below })
    }
}

// The ramp's logarithm and exponential come from `libm`, a pure-Rust port of
// musl's maths library, rather than the platform's. Each platform's own
// `ln_1p` and `exp_m1` may round the last bit differently, and on a ramp that
// can move a converted position across a frame boundary. With `libm`, every
// platform converts every position to the same frame, so a golden render's
// timing matches across platforms (WP-0.10). The rest of the arithmetic is
// IEEE 754 operations, which every platform rounds alike.

/// `ln(1 + x) / x`, which tends to 1 as `x` tends to 0.
fn ln_1p_ratio(x: f64) -> f64 {
    if x == 0.0 { 1.0 } else { libm::log1p(x) / x }
}

/// `(e^y − 1) / y`, which tends to 1 as `y` tends to 0.
fn exp_m1_ratio(y: f64) -> f64 {
    if y == 0.0 { 1.0 } else { libm::expm1(y) / y }
}

/// Rounds to the nearest integer, halves away from zero. Saturates beyond
/// the range of `i64`; NaN gives zero.
fn round_to_i64(x: f64) -> i64 {
    x.round() as i64
}

/// Tempo against musical position: a list of constant and linear-ramp
/// segments from tick zero on. Before tick zero the first segment's starting
/// tempo holds.
///
/// Tick zero is second zero and frame zero. Conversions integrate the
/// segments in closed form, so they're exact up to `f64` rounding, and they
/// neither allocate nor lock, so the audio thread can call them. Cloning is
/// cheap: the segments are shared. Dropping the last clone frees them, so
/// the audio thread hands its maps to the drop thread rather than dropping
/// them.
///
/// Positions far from zero lose precision in seconds. Within a million
/// quarter notes of zero (139 hours at 120 BPM), ticks to seconds to ticks
/// is exact at every tempo the type allows.
#[derive(Clone)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(try_from = "Vec<TempoSegment>", into = "Vec<TempoSegment>")
)]
pub struct TempoMap {
    /// Never empty. The first starts at tick zero, starts increase, and the
    /// last is constant.
    pieces: Arc<[Piece]>,
}

impl TempoMap {
    /// A map with one steady tempo.
    #[must_use]
    pub fn constant(bpm: Bpm) -> Self {
        Self::build(&[TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm,
        }])
    }

    /// A map from its segments.
    ///
    /// # Errors
    ///
    /// - [`TimeError::EmptyMap`] if there are no segments.
    /// - [`TimeError::FirstNotAtZero`] if the first doesn't start at tick zero.
    /// - [`TimeError::NotIncreasing`] if a segment doesn't start after the one
    ///   before it.
    /// - [`TimeError::RampAtEnd`] if the last segment is a ramp.
    pub fn new(segments: impl IntoIterator<Item = TempoSegment>) -> Result<Self, TimeError> {
        let segments: Vec<TempoSegment> = segments.into_iter().collect();
        let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
            return Err(TimeError::EmptyMap);
        };
        if first.start() != Ticks::ZERO {
            return Err(TimeError::FirstNotAtZero {
                start: first.start(),
            });
        }
        if let Some(index) =
            (1..segments.len()).find(|&i| segments[i].start() <= segments[i - 1].start())
        {
            return Err(TimeError::NotIncreasing { index });
        }
        if matches!(last, TempoSegment::Ramp { .. }) {
            return Err(TimeError::RampAtEnd);
        }
        Ok(Self::build(&segments))
    }

    /// A map from tempo breakpoints, the way an automation envelope draws
    /// tempo: a straight line from each breakpoint to the next, and a jump
    /// where two breakpoints share a position.
    ///
    /// The first breakpoint's tempo holds before it, and the last one's holds
    /// after it. Where more than two breakpoints share a position, the first
    /// sets the tempo arriving there and the last the tempo leaving it.
    ///
    /// # Errors
    ///
    /// - [`TimeError::EmptyMap`] if there are no breakpoints.
    /// - [`TimeError::NegativePosition`] if a breakpoint is before tick zero.
    /// - [`TimeError::NotIncreasing`] if a breakpoint comes before the one
    ///   listed ahead of it.
    pub fn from_breakpoints(
        breakpoints: impl IntoIterator<Item = (Ticks, Bpm)>,
    ) -> Result<Self, TimeError> {
        let points: Vec<(Ticks, Bpm)> = breakpoints.into_iter().collect();
        let (Some(&(first_at, first_bpm)), Some(&(last_at, last_bpm))) =
            (points.first(), points.last())
        else {
            return Err(TimeError::EmptyMap);
        };
        for (index, &(at, _)) in points.iter().enumerate() {
            if at < Ticks::ZERO {
                return Err(TimeError::NegativePosition { at });
            }
            if index > 0 && at < points[index - 1].0 {
                return Err(TimeError::NotIncreasing { index });
            }
        }

        let mut segments = Vec::with_capacity(points.len() + 1);
        let mut push = |segment: TempoSegment| {
            // Merge a steady stretch into the steady segment before it.
            if let (
                Some(TempoSegment::Constant { bpm: before, .. }),
                TempoSegment::Constant { bpm, .. },
            ) = (segments.last(), segment)
                && *before == bpm
            {
                return;
            }
            segments.push(segment);
        };
        if first_at > Ticks::ZERO {
            push(TempoSegment::Constant {
                start: Ticks::ZERO,
                bpm: first_bpm,
            });
        }
        for pair in points.windows(2) {
            let ((start, from), (end, to)) = (pair[0], pair[1]);
            if start == end {
                continue;
            }
            push(if from == to {
                TempoSegment::Constant { start, bpm: from }
            } else {
                TempoSegment::Ramp { start, from, to }
            });
        }
        push(TempoSegment::Constant {
            start: last_at,
            bpm: last_bpm,
        });
        Self::new(segments)
    }

    /// Works out the pieces of segments that are already valid.
    fn build(segments: &[TempoSegment]) -> Self {
        let mut pieces = Vec::with_capacity(segments.len());
        let mut start_seconds = 0.0;
        for (index, &segment) in segments.iter().enumerate() {
            let start_bpm = segment.start_bpm().get();
            let length = segments
                .get(index + 1)
                .map_or(i64::MAX, |next| (next.start() - segment.start()).get());
            let slope = match segment {
                TempoSegment::Constant { .. } => 0.0,
                TempoSegment::Ramp { to, .. } => {
                    (to.get() - start_bpm) / (start_bpm * length as f64)
                }
            };
            let piece = Piece {
                segment,
                start_seconds,
                seconds_per_tick: segment.start_bpm().seconds_per_tick(),
                slope,
                length,
            };
            if length != i64::MAX {
                start_seconds += piece.seconds_at(length as f64);
            }
            pieces.push(piece);
        }
        Self {
            pieces: pieces.into(),
        }
    }

    /// The segments, in order.
    pub fn segments(&self) -> impl ExactSizeIterator<Item = TempoSegment> + '_ {
        self.pieces.iter().map(|piece| piece.segment)
    }

    /// The index of the piece holding `ticks`, which must not be negative.
    fn piece_index_at(&self, ticks: Ticks) -> usize {
        // The first piece starts at zero, so the partition point is at least 1.
        self.pieces
            .partition_point(|piece| piece.segment.start() <= ticks)
            - 1
    }

    /// The segment in force at `ticks`. Before tick zero, that's the first.
    #[must_use]
    pub fn segment_at(&self, ticks: Ticks) -> TempoSegment {
        if ticks < Ticks::ZERO {
            return self.pieces[0].segment;
        }
        self.pieces[self.piece_index_at(ticks)].segment
    }

    /// The tempo at `ticks`.
    #[must_use]
    pub fn bpm_at(&self, ticks: Ticks) -> Bpm {
        if ticks < Ticks::ZERO {
            return self.pieces[0].segment.start_bpm();
        }
        let piece = &self.pieces[self.piece_index_at(ticks)];
        match piece.segment {
            TempoSegment::Constant { bpm, .. } => bpm,
            TempoSegment::Ramp { start, from, to } => {
                let tau = (ticks - start).get() as f64;
                let bpm = from.get() + (to.get() - from.get()) * tau / piece.length as f64;
                let (low, high) = (from.min(to).get(), from.max(to).get());
                Bpm(bpm.clamp(low, high))
            }
        }
    }

    /// The time of `ticks` in seconds from tick zero.
    #[must_use]
    pub fn ticks_to_seconds(&self, ticks: Ticks) -> f64 {
        if ticks < Ticks::ZERO {
            return ticks.get() as f64 * self.pieces[0].seconds_per_tick;
        }
        let piece = &self.pieces[self.piece_index_at(ticks)];
        let tau = (ticks - piece.segment.start()).get() as f64;
        piece.start_seconds + piece.seconds_at(tau)
    }

    /// The time of the position `ticks` in seconds from tick zero, where
    /// `ticks` isn't rounded to a whole tick: the inverse of
    /// [`TempoMap::seconds_to_ticks_exact`]. A player that follows an outside
    /// tempo (the transport spec, R24) advances a fractional position and
    /// reads the song's frames through this.
    #[must_use]
    pub fn ticks_to_seconds_exact(&self, ticks: f64) -> f64 {
        if ticks.is_nan() {
            return 0.0;
        }
        if ticks < 0.0 {
            return ticks * self.pieces[0].seconds_per_tick;
        }
        let index = self
            .pieces
            .partition_point(|piece| piece.segment.start().get() as f64 <= ticks)
            - 1;
        let piece = &self.pieces[index];
        let tau = ticks - piece.segment.start().get() as f64;
        piece.start_seconds + piece.seconds_at(tau)
    }

    /// The tick nearest in time to `seconds` from tick zero, with halves
    /// rounded away from zero. Values beyond the range of [`Ticks`] saturate,
    /// and NaN gives tick zero.
    #[must_use]
    pub fn seconds_to_ticks(&self, seconds: f64) -> Ticks {
        if seconds.is_nan() {
            return Ticks::ZERO;
        }
        if seconds < 0.0 {
            return Ticks(round_to_i64(seconds / self.pieces[0].seconds_per_tick));
        }
        // The first piece starts at second zero, so the partition point is at
        // least 1.
        let index = self
            .pieces
            .partition_point(|piece| piece.start_seconds <= seconds)
            - 1;
        let piece = &self.pieces[index];
        let tau = piece.nearest_tick(seconds - piece.start_seconds);
        piece.segment.start().saturating_add(Ticks(tau))
    }

    /// The position `seconds` from tick zero falls on, in ticks and not
    /// rounded: what a player that follows the tempo frame by frame (a
    /// warped clip launched from a Session slot) reads. Within half a tick of
    /// [`TempoMap::seconds_to_ticks`], and as exact as the arithmetic is.
    #[must_use]
    pub fn seconds_to_ticks_exact(&self, seconds: f64) -> f64 {
        if seconds.is_nan() {
            return 0.0;
        }
        if seconds < 0.0 {
            return seconds / self.pieces[0].seconds_per_tick;
        }
        let index = self
            .pieces
            .partition_point(|piece| piece.start_seconds <= seconds)
            - 1;
        let piece = &self.pieces[index];
        piece.segment.start().get() as f64 + piece.ticks_at(seconds - piece.start_seconds)
    }

    /// The frame nearest to `ticks` at `rate`, with halves rounded away from
    /// zero.
    ///
    /// A frame usually spans many ticks (about 29 at 120 BPM and 48 kHz), so
    /// converting back with [`TempoMap::frames_to_ticks`] can land up to half
    /// a frame away.
    #[must_use]
    pub fn ticks_to_frames(&self, ticks: Ticks, rate: SampleRate) -> Frames {
        rate.seconds_to_frames(self.ticks_to_seconds(ticks))
    }

    /// The tick nearest in time to `frames` at `rate`, with halves rounded
    /// away from zero.
    ///
    /// At every tempo of 20 BPM or more and every rate up to 192 kHz, a tick
    /// is shorter than a frame, so [`TempoMap::ticks_to_frames`] brings the
    /// result back to the same frame.
    #[must_use]
    pub fn frames_to_ticks(&self, frames: Frames, rate: SampleRate) -> Ticks {
        self.seconds_to_ticks(rate.frames_to_seconds(frames))
    }
}

impl Default for TempoMap {
    /// A steady 120 BPM.
    fn default() -> Self {
        Self::constant(Bpm::DEFAULT)
    }
}

impl PartialEq for TempoMap {
    fn eq(&self, other: &Self) -> bool {
        self.segments().eq(other.segments())
    }
}

impl Eq for TempoMap {}

impl Hash for TempoMap {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.pieces.len().hash(state);
        for segment in self.segments() {
            segment.hash(state);
        }
    }
}

impl fmt::Debug for TempoMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.segments()).finish()
    }
}

impl TryFrom<Vec<TempoSegment>> for TempoMap {
    type Error = TimeError;
    fn try_from(segments: Vec<TempoSegment>) -> Result<Self, TimeError> {
        Self::new(segments)
    }
}

impl From<TempoMap> for Vec<TempoSegment> {
    fn from(map: TempoMap) -> Self {
        map.segments().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bpm(value: f64) -> Bpm {
        Bpm::new(value).unwrap()
    }

    fn q(quarters: i64) -> Ticks {
        Ticks::from_quarters(quarters)
    }

    #[test]
    fn bpm_range() {
        assert_eq!(Bpm::new(0.5), Err(TimeError::BpmOutOfRange));
        assert_eq!(Bpm::new(1_000.5), Err(TimeError::BpmOutOfRange));
        assert_eq!(Bpm::new(f64::NAN), Err(TimeError::BpmOutOfRange));
        assert_eq!(Bpm::new(f64::INFINITY), Err(TimeError::BpmOutOfRange));
        assert_eq!(Bpm::new(1.0), Ok(Bpm::MIN));
        assert_eq!(Bpm::new(1_000.0), Ok(Bpm::MAX));
        assert!(bpm(90.0) < bpm(120.0));
        assert_eq!(bpm(128.0).to_string(), "128 BPM");
    }

    #[test]
    fn constant_tempo() {
        let map = TempoMap::constant(bpm(120.0));
        assert_eq!(map.ticks_to_seconds(q(1)), 0.5);
        assert_eq!(map.ticks_to_seconds(q(-2)), -1.0);
        assert_eq!(map.seconds_to_ticks(0.5), q(1));
        assert_eq!(map.seconds_to_ticks(-1.0), q(-2));
        assert_eq!(
            map.ticks_to_frames(q(1), SampleRate::HZ_48000),
            Frames(24_000)
        );
        assert_eq!(
            map.frames_to_ticks(Frames(24_000), SampleRate::HZ_48000),
            q(1)
        );
        assert_eq!(map.bpm_at(q(100)), bpm(120.0));
        assert_eq!(map, TempoMap::default());
    }

    #[test]
    fn exact_ticks_agree_with_the_rounded_ones_to_half_a_tick() {
        let map =
            TempoMap::from_breakpoints([(Ticks::ZERO, bpm(60.0)), (q(8), bpm(120.0))]).unwrap();
        for i in 0..200 {
            let seconds = f64::from(i) * 0.0731 - 0.5;
            let exact = map.seconds_to_ticks_exact(seconds);
            let rounded = map.seconds_to_ticks(seconds).get() as f64;
            assert!(
                (exact - rounded).abs() <= 0.5 + 1e-6,
                "{seconds}: {exact} {rounded}"
            );
            if seconds >= 0.0 {
                // The inverse of ticks to seconds, with no rounding.
                let back = map.ticks_to_seconds(Ticks(exact.round() as i64));
                assert!((back - seconds).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn steps() {
        // 120 BPM for four quarters (2 s), then 60 BPM.
        let map = TempoMap::new([
            TempoSegment::Constant {
                start: Ticks::ZERO,
                bpm: bpm(120.0),
            },
            TempoSegment::Constant {
                start: q(4),
                bpm: bpm(60.0),
            },
        ])
        .unwrap();
        assert_eq!(map.ticks_to_seconds(q(4)), 2.0);
        assert_eq!(map.ticks_to_seconds(q(6)), 4.0);
        assert_eq!(map.seconds_to_ticks(4.0), q(6));
        assert_eq!(map.seconds_to_ticks(1.0), q(2));
        assert_eq!(map.bpm_at(q(4) - Ticks(1)), bpm(120.0));
        assert_eq!(map.bpm_at(q(4)), bpm(60.0));
    }

    #[test]
    fn ramp_in_closed_form() {
        // 60 to 120 BPM over 8 quarters: the tempo is 60 (1 + x / 8) at x
        // quarters, so the time is 8 ln 2 seconds.
        let map = TempoMap::new([
            TempoSegment::Ramp {
                start: Ticks::ZERO,
                from: bpm(60.0),
                to: bpm(120.0),
            },
            TempoSegment::Constant {
                start: q(8),
                bpm: bpm(120.0),
            },
        ])
        .unwrap();
        let expected = 8.0 * 2f64.ln();
        assert!((map.ticks_to_seconds(q(8)) - expected).abs() < 1e-12);
        assert!((map.ticks_to_seconds(q(10)) - (expected + 1.0)).abs() < 1e-12);
        assert_eq!(map.seconds_to_ticks(expected), q(8));
        assert_eq!(map.bpm_at(q(4)), bpm(90.0));
        assert_eq!(map.bpm_at(q(-1)), bpm(60.0));
    }

    #[test]
    fn a_flat_ramp_is_constant() {
        let flat = TempoMap::new([
            TempoSegment::Ramp {
                start: Ticks::ZERO,
                from: bpm(100.0),
                to: bpm(100.0),
            },
            TempoSegment::Constant {
                start: q(4),
                bpm: bpm(100.0),
            },
        ])
        .unwrap();
        let constant = TempoMap::constant(bpm(100.0));
        for quarters in [0, 1, 4, 9] {
            assert_eq!(
                flat.ticks_to_seconds(q(quarters)),
                constant.ticks_to_seconds(q(quarters))
            );
        }
    }

    #[test]
    fn invalid_maps() {
        let at = |start, value| TempoSegment::Constant {
            start,
            bpm: bpm(value),
        };
        assert_eq!(TempoMap::new([]), Err(TimeError::EmptyMap));
        assert_eq!(
            TempoMap::new([at(q(1), 120.0)]),
            Err(TimeError::FirstNotAtZero { start: q(1) })
        );
        assert_eq!(
            TempoMap::new([at(Ticks::ZERO, 120.0), at(q(2), 90.0), at(q(2), 80.0)]),
            Err(TimeError::NotIncreasing { index: 2 })
        );
        assert_eq!(
            TempoMap::new([TempoSegment::Ramp {
                start: Ticks::ZERO,
                from: bpm(60.0),
                to: bpm(120.0),
            }]),
            Err(TimeError::RampAtEnd)
        );
    }

    #[test]
    fn breakpoints() {
        let map = TempoMap::from_breakpoints([
            (q(4), bpm(100.0)),
            (q(8), bpm(100.0)),
            (q(12), bpm(140.0)),
            (q(12), bpm(90.0)),
            (q(12), bpm(80.0)),
            (q(16), bpm(80.0)),
        ])
        .unwrap();
        assert_eq!(
            map.segments().collect::<Vec<_>>(),
            [
                TempoSegment::Constant {
                    start: Ticks::ZERO,
                    bpm: bpm(100.0),
                },
                TempoSegment::Ramp {
                    start: q(8),
                    from: bpm(100.0),
                    to: bpm(140.0),
                },
                TempoSegment::Constant {
                    start: q(12),
                    bpm: bpm(80.0),
                },
            ]
        );
        assert_eq!(TempoMap::from_breakpoints([]), Err(TimeError::EmptyMap));
        assert_eq!(
            TempoMap::from_breakpoints([(q(-1), bpm(100.0))]),
            Err(TimeError::NegativePosition { at: q(-1) })
        );
        assert_eq!(
            TempoMap::from_breakpoints([(q(2), bpm(100.0)), (q(1), bpm(100.0))]),
            Err(TimeError::NotIncreasing { index: 1 })
        );
    }

    #[test]
    fn odd_inputs_saturate() {
        let map = TempoMap::default();
        assert_eq!(map.seconds_to_ticks(f64::NAN), Ticks::ZERO);
        assert_eq!(map.seconds_to_ticks(f64::INFINITY), Ticks::MAX);
        assert_eq!(map.seconds_to_ticks(f64::NEG_INFINITY), Ticks::MIN);
        assert_eq!(map.seconds_to_ticks(1e300), Ticks::MAX);
    }
}
