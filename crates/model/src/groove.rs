//! Grooves (WP-6.19): the set's Groove Pool, and how a groove moves a clip's
//! notes and audio.
//!
//! A [`Groove`] is a short pattern of note positions and velocities with five
//! parameters. [`GroovePlay`] is a groove worked out for playing: its grid
//! lines, their offsets and velocities, and the laws of the grooves spec R6 to
//! R13 as plain functions of numbers. The engine plays through them and
//! Commit writes what they give, so what is heard and what is committed are
//! the same arithmetic. Nothing here allocates once a [`GroovePlay`] is made,
//! except where it says so, because the audio thread calls it.

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::{ClipId, GrooveId};

/// The largest Global Amount, in percent.
pub const GLOBAL_AMOUNT_MAX: f64 = 130.0;

/// The Base a new groove, and one extracted from a clip, has.
pub const DEFAULT_BASE: GrooveBase = GrooveBase::Sixteenth;

/// How long a Velocity gain step glides to its new value, in seconds
/// (the grooves spec, R13).
pub const GAIN_GLIDE_SECONDS: f64 = 0.001;

/// The grid a groove's notes are measured against (the grooves spec, R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum GrooveBase {
    /// A quarter note.
    #[serde(rename = "1/4")]
    Quarter,
    /// An eighth note.
    #[serde(rename = "1/8")]
    Eighth,
    /// An eighth-note triplet.
    #[serde(rename = "1/8T")]
    EighthTriplet,
    /// A sixteenth note.
    #[serde(rename = "1/16")]
    Sixteenth,
    /// A sixteenth-note triplet.
    #[serde(rename = "1/16T")]
    SixteenthTriplet,
    /// A thirty-second note.
    #[serde(rename = "1/32")]
    ThirtySecond,
}

impl GrooveBase {
    /// Every Base, in the order the chooser lists them.
    pub const ALL: [Self; 6] = [
        Self::Quarter,
        Self::Eighth,
        Self::EighthTriplet,
        Self::Sixteenth,
        Self::SixteenthTriplet,
        Self::ThirtySecond,
    ];

    /// One step of the grid, in ticks.
    #[must_use]
    pub const fn ticks(self) -> Ticks {
        let quarter = bl_core::time::TICKS_PER_QUARTER;
        Ticks(match self {
            Self::Quarter => quarter,
            Self::Eighth => quarter / 2,
            Self::EighthTriplet => quarter / 3,
            Self::Sixteenth => quarter / 4,
            Self::SixteenthTriplet => quarter / 6,
            Self::ThirtySecond => quarter / 8,
        })
    }
}

/// One note of a groove: where it sits in the groove and how hard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GrooveNote {
    /// From 0 to before the groove's length.
    pub position: Ticks,
    /// From 1 to 127.
    pub velocity: u8,
}

/// A groove (the grooves spec, R1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Groove {
    /// The groove's ID.
    pub id: GrooveId,
    /// Its name.
    pub name: String,
    /// How long it is, in ticks: positive. It repeats along a clip's contents
    /// every length.
    pub length: Ticks,
    /// Its notes, by position, no two at one position.
    pub notes: Vec<GrooveNote>,
    /// The grid its notes are measured against.
    #[serde(default = "default_base")]
    pub base: GrooveBase,
    /// How much straight quantisation to the Base grid is applied first, from
    /// 0 to 100 percent.
    #[serde(default)]
    pub quantize: f64,
    /// How strongly the groove's timing acts, from 0 to 100 percent.
    #[serde(default = "full")]
    pub timing: f64,
    /// How much random timing is added, from 0 to 100 percent.
    #[serde(default)]
    pub random: f64,
    /// How strongly the groove's velocities act, from -100 to +100 percent.
    #[serde(default)]
    pub velocity: f64,
}

fn default_base() -> GrooveBase {
    DEFAULT_BASE
}

fn full() -> f64 {
    100.0
}

impl Groove {
    /// A groove with the parameters an extracted one starts with: Base 1/16,
    /// Quantize 0 %, Timing 100 %, Random 0 % and Velocity 0 % (the grooves
    /// spec, R17). Its notes are put in order.
    #[must_use]
    pub fn new(id: GrooveId, name: String, length: Ticks, mut notes: Vec<GrooveNote>) -> Self {
        notes.sort_by_key(|note| note.position);
        Self {
            id,
            name,
            length,
            notes,
            base: DEFAULT_BASE,
            quantize: 0.0,
            timing: 100.0,
            random: 0.0,
            velocity: 0.0,
        }
    }

    /// Whether the groove keeps the grooves spec's R1 and R2.
    ///
    /// # Errors
    ///
    /// The first thing wrong with it.
    pub fn check(&self) -> Result<(), ModelError> {
        if self.length <= Ticks::ZERO {
            return Err(ModelError::EmptyLength(self.length));
        }
        for (index, note) in self.notes.iter().enumerate() {
            if note.position < Ticks::ZERO || note.position >= self.length {
                return Err(ModelError::InvalidNote { index });
            }
            if !(1..=127).contains(&note.velocity) {
                return Err(ModelError::InvalidNote { index });
            }
        }
        let mut positions: Vec<Ticks> = self.notes.iter().map(|note| note.position).collect();
        positions.sort_unstable();
        if let Some(pair) = positions.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(ModelError::ChangeInUse {
                what: "groove note",
                at: pair[0],
            });
        }
        check_percent("Quantize", self.quantize, 0.0, 100.0)?;
        check_percent("Timing", self.timing, 0.0, 100.0)?;
        check_percent("Random", self.random, 0.0, 100.0)?;
        check_percent("Velocity", self.velocity, -100.0, 100.0)
    }
}

/// Whether `value` is a percentage from `min` to `max`.
pub(crate) fn check_percent(
    what: &'static str,
    value: f64,
    min: f64,
    max: f64,
) -> Result<(), ModelError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(ModelError::InvalidValue { what, value })
    }
}

/// The set's grooves (the grooves spec, R1 and R20): the Groove Pool, the
/// Global Amount, and Auto Load Groove with its groove.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroovePool {
    /// The grooves, in the order they were added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pool: Vec<Groove>,
    /// The Global Amount, from 0 to 130 percent: it scales Timing, Random and
    /// Velocity for every groove. 100 in a set saved before grooves.
    #[serde(default = "full", skip_serializing_if = "is_full")]
    pub global_amount: f64,
    /// The Auto Load Groove switch: whether a new MIDI clip takes the
    /// auto-load groove.
    #[serde(default, skip_serializing_if = "is_off")]
    pub auto_load: bool,
    /// The groove a new MIDI clip takes while Auto Load Groove is on, one of
    /// the pool's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_load_groove: Option<GrooveId>,
}

fn is_full(value: &f64) -> bool {
    *value == 100.0
}

fn is_off(value: &bool) -> bool {
    !*value
}

impl Default for GroovePool {
    /// An empty pool, a Global Amount of 100 % and Auto Load Groove off.
    fn default() -> Self {
        Self {
            pool: Vec::new(),
            global_amount: 100.0,
            auto_load: false,
            auto_load_groove: None,
        }
    }
}

impl GroovePool {
    /// Whether this is what a set with no grooves holds, which the set's
    /// saved form leaves out.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The groove with this ID.
    #[must_use]
    pub fn get(&self, id: GrooveId) -> Option<&Groove> {
        self.pool.iter().find(|groove| groove.id == id)
    }

    /// The groove a new MIDI clip takes: the auto-load groove while the
    /// switch is on and the pool holds it.
    #[must_use]
    pub fn auto_loaded(&self) -> Option<GrooveId> {
        self.auto_load
            .then_some(self.auto_load_groove)
            .flatten()
            .filter(|id| self.get(*id).is_some())
    }

    /// The groove, worked out for playing at the Global Amount.
    #[must_use]
    pub fn play(&self, id: GrooveId) -> Option<GroovePlay> {
        self.get(id)
            .map(|groove| GroovePlay::new(groove, self.global_amount))
    }
}

/// A line of a groove's grid: where the nearest groove note sits against it
/// and how hard that note is (the grooves spec, R6).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Line {
    offset: f64,
    velocity: Option<u8>,
}

/// A groove worked out for playing: its grid lines and the laws that move a
/// clip's notes and audio (the grooves spec, R6 to R13).
///
/// Positions are ticks in a clip's contents, as `f64` so the audio's
/// fractional beats pass through unrounded.
#[derive(Clone, Debug, PartialEq)]
pub struct GroovePlay {
    base: f64,
    length: f64,
    /// The groove's notes, by position: each position and velocity.
    notes: Vec<(f64, u8)>,
    /// The mean velocity of the groove's notes.
    mean: f64,
    quantize: f64,
    /// Timing, Random and Velocity as the groove sets them, from 0 to 1 (and
    /// -1 to 1 for Velocity).
    own: [f64; 3],
    /// Timing, Random and Velocity with the Global Amount applied.
    timing: f64,
    random: f64,
    velocity: f64,
}

impl GroovePlay {
    /// `groove` at `global_amount` percent.
    #[must_use]
    pub fn new(groove: &Groove, global_amount: f64) -> Self {
        let base = groove.base.ticks().get() as f64;
        let length = groove.length.get().max(1) as f64;
        let mut notes: Vec<(f64, u8)> = groove
            .notes
            .iter()
            .map(|note| (note.position.get() as f64, note.velocity))
            .collect();
        notes.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mean = if groove.notes.is_empty() {
            0.0
        } else {
            groove
                .notes
                .iter()
                .map(|note| f64::from(note.velocity))
                .sum::<f64>()
                / groove.notes.len() as f64
        };
        let mut play = Self {
            base,
            length,
            notes,
            mean,
            quantize: groove.quantize / 100.0,
            own: [groove.timing, groove.random, groove.velocity].map(|percent| percent / 100.0),
            timing: 0.0,
            random: 0.0,
            velocity: 0.0,
        };
        play.set_global_amount(global_amount);
        play
    }

    /// Scales Timing, Random and Velocity to `global_amount` percent, as
    /// [`GroovePlay::new`] does, without allocating: a device whose own
    /// groove follows the Global Amount calls it with each block.
    pub fn set_global_amount(&mut self, global_amount: f64) {
        let amount = global_amount / 100.0;
        [self.timing, self.random, self.velocity] = self.own.map(|own| own * amount);
    }

    /// One step of the Base grid, in ticks.
    #[must_use]
    pub fn base(&self) -> f64 {
        self.base
    }

    /// Whether the groove changes nothing for a note: no Quantize, no Timing,
    /// no Random and no Velocity.
    #[must_use]
    pub fn is_inert(&self) -> bool {
        self.quantize == 0.0 && self.timing == 0.0 && self.random == 0.0 && self.velocity == 0.0
    }

    /// Whether the groove's Velocity acts, so audio needs gain steps.
    #[must_use]
    pub fn has_velocity(&self) -> bool {
        self.velocity != 0.0
    }

    /// Whether Quantize acts.
    #[must_use]
    pub fn has_quantize(&self) -> bool {
        self.quantize != 0.0
    }

    /// The position of grid line `n`, in ticks from the start of the
    /// contents: the Base's lines, counted from there (R6).
    #[must_use]
    pub fn line_position(&self, n: i64) -> f64 {
        n as f64 * self.base
    }

    /// Line `n`'s offset and groove velocity (R6): the groove note nearest
    /// it, round the groove's circle, if one lies within half a Base step.
    fn line(&self, n: i64) -> Line {
        let none = Line {
            offset: 0.0,
            velocity: None,
        };
        let (Some(&first), Some(&last)) = (self.notes.first(), self.notes.last()) else {
            return none;
        };
        let at = self.line_position(n).rem_euclid(self.length);
        let i = self.notes.partition_point(|note| note.0 < at);
        let after = self
            .notes
            .get(i)
            .copied()
            .unwrap_or((first.0 + self.length, first.1));
        let before = i
            .checked_sub(1)
            .map_or((last.0 - self.length, last.1), |j| self.notes[j]);
        let (position, velocity) = if at - before.0 <= after.0 - at {
            before
        } else {
            after
        };
        let offset = position - at;
        if offset.abs() <= self.base / 2.0 + 1e-9 {
            Line {
                offset,
                velocity: Some(velocity),
            }
        } else {
            none
        }
    }

    /// The last line at or before `p`.
    #[must_use]
    pub fn line_at_or_before(&self, p: f64) -> i64 {
        let mut n = (p / self.base).floor() as i64;
        // Rounding can leave a line too far either way.
        while self.line_position(n) > p {
            n -= 1;
        }
        while self.line_position(n + 1) <= p {
            n += 1;
        }
        n
    }

    /// The grid line nearest `p`: the earlier of two equally near.
    #[must_use]
    pub fn nearest_line(&self, p: f64) -> i64 {
        let before = self.line_at_or_before(p);
        if p - self.line_position(before) <= self.line_position(before + 1) - p {
            before
        } else {
            before + 1
        }
    }

    /// The groove's displacement at `x`: the offset of each grid line,
    /// changing in a straight line between neighbouring lines (R7).
    #[must_use]
    pub fn displacement(&self, x: f64) -> f64 {
        let n = self.line_at_or_before(x);
        let (a, b) = (self.line_position(n), self.line_position(n + 1));
        let t = (x - a) / (b - a);
        self.line(n).offset + t * (self.line(n + 1).offset - self.line(n).offset)
    }

    /// The most a note or a line moves from its place, in ticks: less than
    /// half a Base step (R7).
    fn reach(&self) -> f64 {
        (self.base / 2.0 - 1.0).max(0.0)
    }

    /// Where a note that starts at `start` plays, in ticks, before it is held
    /// inside its pass and rounded (R7, R8): quantised by Quantize, moved by
    /// the groove's displacement by Timing, and by Random. `key` and the
    /// note's start pick Random's number from `seed` and `pass`.
    #[must_use]
    pub fn note_start(&self, start: Ticks, key: u8, seed: u64, pass: u64) -> f64 {
        let p = start.get() as f64;
        let line = self.line_position(self.nearest_line(p));
        let q = p + self.quantize * (line - p);
        let noise = noise(seed, pass, ((start.get() as u64) << 8) | u64::from(key));
        let moved = self.timing * self.displacement(q) + self.random * noise * self.base / 4.0;
        q + moved.clamp(-self.reach(), self.reach())
    }

    /// The velocity a note that starts at `start` plays at (R9), held to 1 to
    /// 127.
    #[must_use]
    pub fn note_velocity(&self, start: Ticks, velocity: u8) -> u8 {
        let line = self.line(self.nearest_line(start.get() as f64));
        match line.velocity {
            Some(groove) if self.velocity != 0.0 => (f64::from(velocity)
                + self.velocity * (f64::from(groove) - self.mean))
                .round()
                .clamp(1.0, 127.0) as u8,
            _ => velocity,
        }
    }

    /// How far audio on line `n` moves in a pass (R12): Timing's share of the
    /// line's offset and Random's draw for the line, held to less than half a
    /// Base step.
    #[must_use]
    pub fn line_shift(&self, n: i64, seed: u64, pass: u64) -> f64 {
        let moved = self.timing * self.line(n).offset
            + self.random * noise(seed, pass, n as u64) * self.base / 4.0;
        moved.clamp(-self.reach(), self.reach())
    }

    /// The gain Velocity gives the audio from line `n` to the next (R13).
    #[must_use]
    pub fn line_gain(&self, n: i64) -> f64 {
        match self.line(n).velocity {
            Some(groove) => {
                (1.0 + self.velocity * (f64::from(groove) - self.mean) / 127.0).max(0.0)
            }
            None => 1.0,
        }
    }
}

/// A number from -1 to 1 that depends on `seed`, `pass` and `key` alone
/// (the grooves spec, R8): the same every time and at any block size.
#[must_use]
pub fn noise(seed: u64, pass: u64, key: u64) -> f64 {
    let mut z = mix(seed ^ pass.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = mix(z ^ key.wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    (z >> 11) as f64 * (2.0 / (1u64 << 53) as f64) - 1.0
}

/// SplitMix64's finaliser.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The seed Random draws from for a clip (the grooves spec, R8): its ID.
#[must_use]
pub fn clip_seed(clip: ClipId) -> u64 {
    clip.0.get()
}

/// A clip's transients as Quantize moves them (the grooves spec, R12): each
/// its share of the way to its nearest grid line, the audio between
/// neighbouring transients stretching to follow.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MovedTransients {
    /// The transients, as beats of the clip's contents, in order.
    src: Vec<f64>,
    /// Where each one goes, in the same order. A transient whose move would
    /// reach or pass its neighbour stays where it is.
    moved: Vec<f64>,
}

impl MovedTransients {
    /// `beats` moved by `play`'s Quantize.
    #[must_use]
    pub fn new(play: &GroovePlay, mut beats: Vec<f64>) -> Self {
        beats.retain(|beat| beat.is_finite());
        beats.sort_by(f64::total_cmp);
        beats.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        let mut moved: Vec<f64> = Vec::with_capacity(beats.len());
        for (i, &beat) in beats.iter().enumerate() {
            let line = play.line_position(play.nearest_line(beat));
            let mut to = beat + play.quantize * (line - beat);
            // A transient after the contents' first beat stays after it: the
            // map's start is fixed, and a transient can't land on it.
            if beat > 0.0 {
                to = to.max(1.0);
            }
            let after_previous = moved.last().is_none_or(|last| to > *last + 1.0);
            let before_next = beats.get(i + 1).is_none_or(|next| to < *next - 1.0);
            moved.push(if after_previous && before_next {
                to
            } else {
                beat
            });
        }
        Self { src: beats, moved }
    }

    /// The transients strictly inside `lo..hi`, before and after the move:
    /// those whose both places are inside, which are in order in both.
    #[must_use]
    pub fn within(&self, lo: f64, hi: f64) -> (&[f64], &[f64]) {
        let first = self
            .src
            .partition_point(|b| *b <= lo)
            .max(self.moved.partition_point(|b| *b <= lo));
        let last = self
            .src
            .partition_point(|b| *b < hi)
            .min(self.moved.partition_point(|b| *b < hi));
        let last = last.max(first);
        (&self.src[first..last], &self.moved[first..last])
    }
}

/// One pass through a clip's contents, from `lo` to `hi` (infinite where the
/// contents run on): the map from a beat of the audio to the beat it plays on
/// under a groove, and back (the grooves spec, R12). Both ends stay where
/// they are.
///
/// The audio first goes through Quantize (its transients move), then through
/// the groove (each grid line moves by [`GroovePlay::line_shift`], the audio
/// between lines following in a straight line). All of it is arithmetic on
/// what the view borrows, so the audio thread can use it.
#[derive(Clone, Copy, Debug)]
pub struct AudioPass<'a> {
    play: &'a GroovePlay,
    lo: f64,
    hi: f64,
    seed: u64,
    pass: u64,
    src: &'a [f64],
    moved: &'a [f64],
}

impl<'a> AudioPass<'a> {
    /// The pass from `lo` to `hi`, number `pass` since the clip started, whose
    /// Random draws come from `seed`.
    #[must_use]
    pub fn new(
        play: &'a GroovePlay,
        transients: &'a MovedTransients,
        (lo, hi): (f64, f64),
        seed: u64,
        pass: u64,
    ) -> Self {
        let (src, moved) = transients.within(lo, hi);
        Self {
            play,
            lo,
            hi,
            seed,
            pass,
            src,
            moved,
        }
    }

    /// Line `n` as a knot of the second map: where the audio on it is, and
    /// where it plays. Lines outside the pass are its ends.
    fn knot(&self, n: i64) -> (f64, f64) {
        let p = self.play.line_position(n);
        if p <= self.lo {
            (self.lo, self.lo)
        } else if p >= self.hi {
            (self.hi, self.hi)
        } else {
            let at = p + self.play.line_shift(n, self.seed, self.pass);
            (p, at.max(self.lo + 1.0).min(self.hi - 1.0))
        }
    }

    /// The beat after Quantize that the audio on beat `b` is on.
    fn quantised(&self, b: f64) -> f64 {
        let i = self.src.partition_point(|t| *t <= b);
        let left = match i.checked_sub(1) {
            Some(j) => (self.src[j], self.moved[j]),
            None => (self.lo, self.lo),
        };
        match self.src.get(i) {
            Some(&t) => lerp(left, (t, self.moved[i]), b),
            None if self.hi.is_finite() => lerp(left, (self.hi, self.hi), b),
            None => b + left.1 - left.0,
        }
    }

    /// The beat before Quantize that is on beat `x` after it.
    fn unquantised(&self, x: f64) -> f64 {
        let i = self.moved.partition_point(|t| *t <= x);
        let left = match i.checked_sub(1) {
            Some(j) => (self.moved[j], self.src[j]),
            None => (self.lo, self.lo),
        };
        match self.moved.get(i) {
            Some(&t) => lerp(left, (t, self.src[i]), x),
            None if self.hi.is_finite() => lerp(left, (self.hi, self.hi), x),
            None => x + left.1 - left.0,
        }
    }

    /// The beat a grid-line position `x` plays on.
    fn shifted(&self, x: f64) -> f64 {
        let n = self.play.line_at_or_before(x);
        lerp(self.knot(n), self.knot(n + 1), x)
    }

    /// The last line whose knot plays at or before `y`, for `y` inside the
    /// pass.
    fn line_playing(&self, y: f64) -> i64 {
        let mut n = self.play.line_at_or_before(y);
        while self.knot(n).1 > y {
            n -= 1;
        }
        while self.knot(n + 1).1 <= y {
            n += 1;
        }
        n
    }

    /// The beat of the audio that plays on beat `y`.
    #[must_use]
    pub fn unmap(&self, y: f64) -> f64 {
        if y <= self.lo || y >= self.hi {
            return y;
        }
        let n = self.line_playing(y);
        let (a, b) = (self.knot(n), self.knot(n + 1));
        let x = lerp((a.1, a.0), (b.1, b.0), y);
        self.unquantised(x)
    }

    /// The beat the audio on beat `b` plays on.
    #[must_use]
    pub fn map(&self, b: f64) -> f64 {
        if b <= self.lo || b >= self.hi {
            return b;
        }
        self.shifted(self.quantised(b))
    }

    /// The gain Velocity gives beat `y`: the gain before the step that
    /// starts it, the gain after it, and how far past the step `y` is, in
    /// ticks, for the caller to glide over a millisecond of them (R13).
    /// `(1, 1, ∞)` where Velocity does nothing.
    #[must_use]
    pub fn gain_at(&self, y: f64) -> (f64, f64, f64) {
        if !self.play.has_velocity() || y < self.lo || y >= self.hi {
            return (1.0, 1.0, f64::INFINITY);
        }
        let n = self.line_playing(y);
        let now = self.play.line_gain(n);
        // The pass's start is no step: its first gain starts where it is.
        if self.play.line_position(n) <= self.lo {
            return (now, now, f64::INFINITY);
        }
        (self.play.line_gain(n - 1), now, y - self.knot(n).1)
    }

    /// Adds to `out` the beats at which the map `unmap` bends, from `lo` to
    /// `upto`: where each grid line plays, each moved transient, and each of
    /// `markers`, which are beats of the audio.
    pub fn bends(&self, upto: f64, markers: impl Iterator<Item = f64>, out: &mut Vec<f64>) {
        let end = upto.min(self.hi);
        for n in self.play.line_at_or_before(self.lo)..=self.play.line_at_or_before(end) {
            let (x, y) = self.knot(n);
            if x > self.lo && x < self.hi && y < end {
                out.push(y);
            }
        }
        for &t in self.moved {
            out.push(self.shifted(t));
        }
        out.extend(markers.map(|marker| self.map(marker)).filter(|y| *y < end));
    }

    /// The beats of the audio that sit on grid lines, with the beat each
    /// plays on, from `lo` to `upto`: for Commit's warp markers.
    pub fn lines(&self, upto: f64, out: &mut Vec<(f64, f64)>) {
        let end = upto.min(self.hi);
        for n in self.play.line_at_or_before(self.lo)..=self.play.line_at_or_before(end) {
            let (x, y) = self.knot(n);
            if x > self.lo && x < self.hi && y < end {
                out.push((self.unquantised(x), y));
            }
        }
    }

    /// Adds to `out` where each grid line that starts a stretch of the pass
    /// plays, from the one at or before `lo` (which starts with the pass, so
    /// it is `lo`) to `upto`, with the gain Velocity gives from it (R13).
    pub fn gain_steps(&self, upto: f64, out: &mut Vec<(f64, f64)>) {
        let end = upto.min(self.hi);
        for n in self.play.line_at_or_before(self.lo)..=self.play.line_at_or_before(end) {
            let y = self.knot(n).1;
            if y < end {
                out.push((y, self.play.line_gain(n)));
            }
        }
    }

    /// The transients inside the pass, with the beat each plays on.
    pub fn moved(&self, out: &mut Vec<(f64, f64)>) {
        out.extend(
            self.src
                .iter()
                .zip(self.moved)
                .map(|(&b, &x)| (b, self.shifted(x))),
        );
    }
}

/// The point at `x` on the line through `a` and `b`; `a`'s place where they
/// coincide.
fn lerp(a: (f64, f64), b: (f64, f64), x: f64) -> f64 {
    if b.0 <= a.0 {
        a.1
    } else {
        a.1 + (x - a.0) / (b.0 - a.0) * (b.1 - a.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_core::id::Id;

    fn id(n: u64) -> GrooveId {
        GrooveId(Id::new(n).unwrap())
    }

    /// Sixteenths: every second one a thirty-second late, one bar long.
    fn swing(timing: f64, quantize: f64) -> GroovePlay {
        let step = GrooveBase::Sixteenth.ticks();
        let notes = (0..16)
            .map(|i| GrooveNote {
                position: step * i + if i % 2 == 1 { step / 2 } else { Ticks::ZERO },
                velocity: if i % 4 == 0 { 127 } else { 80 },
            })
            .collect();
        let mut groove = Groove::new(id(1), "Swing".to_owned(), Ticks::WHOLE, notes);
        groove.timing = timing;
        groove.quantize = quantize;
        GroovePlay::new(&groove, 100.0)
    }

    #[test]
    fn a_groove_with_a_note_off_the_grid_offsets_the_lines_it_is_near() {
        let play = swing(100.0, 0.0);
        let step = play.base();
        assert_eq!(play.line(0).offset, 0.0);
        assert_eq!(play.line(1).offset, step / 2.0);
        assert_eq!(play.line(1).velocity, Some(80));
    }

    #[test]
    fn rescaling_to_a_global_amount_gives_what_making_it_there_gives() {
        let step = GrooveBase::Sixteenth.ticks();
        for amount in [0.0, 50.0, 130.0, 100.0] {
            let mut groove = Groove::new(
                id(1),
                "Swing".to_owned(),
                Ticks::WHOLE,
                vec![GrooveNote {
                    position: step / 2,
                    velocity: 100,
                }],
            );
            groove.velocity = 40.0;
            let made = GroovePlay::new(&groove, amount);
            let mut rescaled = GroovePlay::new(&groove, 100.0);
            rescaled.set_global_amount(amount);
            assert_eq!(rescaled, made, "{amount} %");
        }
    }

    #[test]
    fn the_lines_stay_on_the_base_grid_when_the_length_is_not_whole_steps() {
        let step = GrooveBase::Sixteenth.ticks();
        let length = step * 4 + Ticks(step.get() / 6);
        let note = GrooveNote {
            position: Ticks::ZERO,
            velocity: 100,
        };
        let mut groove = Groove::new(id(1), "Odd".to_owned(), length, vec![note]);
        groove.random = 100.0;
        let play = GroovePlay::new(&groove, 100.0);
        let s = step.get() as f64;
        for n in 0..40 {
            assert_eq!(play.line_position(n), n as f64 * s);
            assert_eq!(play.line_at_or_before(n as f64 * s + 1.0), n);
        }
        // Lines a step apart keep their order under Random (R12).
        let transients = MovedTransients::new(&play, Vec::new());
        let pass = AudioPass::new(&play, &transients, (0.0, s * 40.0), 3, 0);
        let mut last = -1.0;
        for i in 0..=400 {
            let y = pass.map(f64::from(i) * s / 10.0);
            assert!(y >= last, "{i}: {y} before {last}");
            last = y;
        }
    }

    #[test]
    fn a_note_on_a_line_moves_by_timings_share_of_the_lines_offset() {
        let step = GrooveBase::Sixteenth.ticks();
        for (timing, share) in [(0.0, 0.0), (50.0, 0.5), (100.0, 1.0)] {
            let play = swing(timing, 0.0);
            let at = play.note_start(step, 60, 1, 0);
            // Line 1's offset is half a step, which the move stays just under.
            let want = step.get() as f64 + (step.get() as f64 / 2.0 * share).min(play.reach());
            assert!((at - want).abs() < 1.0, "timing {timing}: {at} vs {want}");
        }
    }

    #[test]
    fn quantize_pulls_a_note_to_its_line_before_the_groove_moves_it() {
        let step = GrooveBase::Sixteenth.ticks().get() as f64;
        let play = swing(0.0, 100.0);
        let at = play.note_start(Ticks((step * 2.0 + 1000.0) as i64), 60, 1, 0);
        assert!((at - step * 2.0).abs() < 1e-6);
        let half = swing(0.0, 50.0);
        let at = half.note_start(Ticks((step * 2.0 + 1000.0) as i64), 60, 1, 0);
        assert!((at - (step * 2.0 + 500.0)).abs() < 1e-6);
    }

    #[test]
    fn random_stays_inside_a_quarter_step_and_is_the_same_each_time() {
        let step = GrooveBase::Sixteenth.ticks();
        let mut groove = Groove::new(id(1), "R".to_owned(), Ticks::WHOLE, Vec::new());
        groove.random = 100.0;
        let play = GroovePlay::new(&groove, 100.0);
        let first: Vec<f64> = (0..16)
            .map(|i| play.note_start(step * i, 60, 7, 0) - (step * i).get() as f64)
            .collect();
        let again: Vec<f64> = (0..16)
            .map(|i| play.note_start(step * i, 60, 7, 0) - (step * i).get() as f64)
            .collect();
        assert_eq!(first, again);
        assert!(first.iter().all(|m| m.abs() <= step.get() as f64 / 4.0));
        assert!(first.iter().any(|m| m.abs() > 1.0));
        let next: Vec<f64> = (0..16)
            .map(|i| play.note_start(step * i, 60, 7, 1) - (step * i).get() as f64)
            .collect();
        assert_ne!(first, next);
    }

    #[test]
    fn velocity_pushes_towards_the_grooves_accents_and_negative_reverses_it() {
        let play = |velocity: f64| {
            let mut groove = Groove::new(
                id(1),
                "V".to_owned(),
                Ticks::WHOLE,
                (0..16)
                    .map(|i| GrooveNote {
                        position: GrooveBase::Sixteenth.ticks() * i,
                        velocity: if i % 2 == 0 { 127 } else { 65 },
                    })
                    .collect(),
            );
            groove.velocity = velocity;
            GroovePlay::new(&groove, 100.0)
        };
        let step = GrooveBase::Sixteenth.ticks();
        // The mean is 96: the loud line gains 31 at +100 %, the soft one loses 31.
        assert_eq!(play(100.0).note_velocity(Ticks::ZERO, 80), 111);
        assert_eq!(play(100.0).note_velocity(step, 80), 49);
        assert_eq!(play(-100.0).note_velocity(Ticks::ZERO, 80), 49);
        assert_eq!(play(0.0).note_velocity(Ticks::ZERO, 80), 80);
        assert_eq!(play(100.0).note_velocity(Ticks::ZERO, 120), 127);
    }

    #[test]
    fn the_global_amount_scales_timing_and_not_quantize() {
        let step = GrooveBase::Sixteenth.ticks();
        let mut groove = Groove::new(
            id(1),
            "G".to_owned(),
            Ticks::WHOLE,
            vec![GrooveNote {
                position: step / 4,
                velocity: 100,
            }],
        );
        groove.quantize = 100.0;
        groove.timing = 100.0;
        let at = |amount: f64| {
            GroovePlay::new(&groove, amount).note_start(Ticks(step.get() / 8), 60, 1, 0)
        };
        // Quantize puts the note on line 0; the line's offset is a quarter step.
        assert!((at(100.0) - step.get() as f64 / 4.0).abs() < 1.0);
        assert!(at(0.0).abs() < 1.0);
        assert!((at(50.0) - step.get() as f64 / 8.0).abs() < 1.0);
    }

    #[test]
    fn audio_maps_back_to_where_it_came_from() {
        let play = swing(100.0, 100.0);
        let transients = MovedTransients::new(&play, vec![1000.0, 190_000.0, 360_000.0]);
        let pass = AudioPass::new(&play, &transients, (0.0, 2_822_400.0), 3, 0);
        for b in [0.0, 500.0, 190_000.0, 300_000.0, 1_000_000.0, 2_822_400.0] {
            let y = pass.map(b);
            assert!((pass.unmap(y) - b).abs() < 1e-6, "{b} -> {y}");
        }
        // The pass's ends stay.
        assert_eq!(pass.map(0.0), 0.0);
        assert_eq!(pass.map(2_822_400.0), 2_822_400.0);
        // Order is kept.
        let mut last = -1.0;
        for i in 0..200 {
            let y = pass.map(f64::from(i) * 14_000.0);
            assert!(y >= last);
            last = y;
        }
    }
}
