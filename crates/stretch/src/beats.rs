//! Beats (the warp modes spec, R14 to R17): the clip played as segments.
//!
//! A segment begins at the clip's Start marker, at each warp marker and at
//! each of the file's transients (Preserve Transients), or at each line of a
//! grid of the clip's own beats. It begins on the song frame its beginning
//! falls on, reads the file from there at the clip's pitch ratio (1 until
//! WP-4.7 wires Transpose and Detune), and stops where the next segment
//! begins, so a song faster than the clip cuts it short (R15). A segment
//! whose own audio runs out first is filled by its transient loop mode,
//! turning at the zero crossing nearest its middle, and the transient
//! envelope shapes its level (R16).
//!
//! The plan holds the segments and the laws a mode plays them by: the
//! stretches of the song each one reads over ([`Plan::stretches`]), which a
//! player turns into the pieces of its table, and the level of each song
//! frame ([`Plan::level`]). It is made off the audio thread, with the clip
//! list (R5), and a frame's output depends only on the song, its media and
//! where the clip's playback began (R6).

/// Ticks to the quarter note, as the warp map counts them.
const TICKS_PER_QUARTER: f64 = 705_600.0;

/// What Beats keeps (R14): the file's transients, or the lines of a grid of
/// the clip's own beats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preserve {
    /// A segment at each transient, at each warp marker and at the clip's
    /// Start marker.
    Transients,
    /// A segment at each line of a grid `division` ticks long, counted from
    /// the clip's 1.1.1.
    Grid { division: u64 },
}

/// How a segment that runs out of its own audio is filled (R15).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Loop {
    /// Silence until the next segment.
    Off,
    /// Back to the turn, forwards again, as often as the time needs.
    #[default]
    Forward,
    /// Backwards from the end to the turn, then forwards, and so on.
    BackAndForth,
}

impl Preserve {
    /// The grid's division in ticks, for a signature's bar: a bar is four
    /// quarter notes (Beatlayer's own reading of R2's list).
    #[must_use]
    pub fn division(self) -> Option<u64> {
        match self {
            Self::Transients => None,
            Self::Grid { division } => Some(division),
        }
    }
}

/// The grid division of `ticks` quarter notes.
#[must_use]
pub fn grid(quarters: f64) -> u64 {
    (TICKS_PER_QUARTER * quarters).round() as u64
}

/// One segment (R14, R15): where in the song it begins and the file it reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    /// The song frame the segment begins on: the nearest frame to where its
    /// beginning falls through the warp map.
    pub frame: i64,
    /// The song frame the next segment begins on.
    pub until: i64,
    /// The file position the segment's first frame reads.
    pub position: f64,
    /// The file position the next segment begins at: where this one's own
    /// audio runs out.
    pub end: f64,
    /// The file position the loop returns to: the zero crossing nearest the
    /// segment's middle (R15).
    pub turn: f64,
}

/// A straight stretch of the song over which a segment's read runs in a
/// straight line: one piece of a player's table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stretch {
    /// The song frame the stretch begins at, perhaps between frames.
    pub frame: f64,
    /// The file position at [`frame`](Self::frame); NaN where the segment
    /// plays nothing.
    pub position: f64,
    /// File frames per song frame, signed: a Back-and-Forth pass reads
    /// backwards.
    pub slope: f64,
}

/// A Beats clip's plan (R14 to R17): its segments in song order, the loop
/// mode that fills a short one, and the transient envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    segments: Vec<Segment>,
    loop_mode: Loop,
    /// The transient envelope, 0 to 100 (R16): 100 at full level.
    envelope: u32,
    /// How far a falling segment's level takes to reach silence: 10 ms in
    /// song frames (R16).
    fall: f64,
}

impl Default for Plan {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// How far a transient envelope's fall takes: 10 ms (R16, Beatlayer's own
/// law within the manual's two ends).
pub const FALL_SECONDS: f64 = 0.010;

impl Plan {
    /// The plan every mode that isn't Beats is given: no segment at all.
    pub const EMPTY: Self = Self {
        segments: Vec::new(),
        loop_mode: Loop::Forward,
        envelope: 100,
        fall: 0.0,
    };

    /// A plan of `segments`, which must be in song order and cover the clip's
    /// playback without a gap, at `rate` frames a second.
    #[must_use]
    pub fn new(segments: Vec<Segment>, loop_mode: Loop, envelope: u32, rate: f64) -> Self {
        Self {
            segments,
            loop_mode,
            envelope,
            fall: FALL_SECONDS * rate,
        }
    }

    /// The segments, in song order.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The loop mode filling a short segment.
    #[must_use]
    pub fn loop_mode(&self) -> Loop {
        self.loop_mode
    }

    /// The transient envelope, 0 to 100.
    #[must_use]
    pub fn envelope(&self) -> u32 {
        self.envelope
    }

    /// Whether it holds no segment: every mode but Beats.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The segment covering song frame `frame`.
    #[must_use]
    pub fn at(&self, frame: i64) -> Option<&Segment> {
        let after = self
            .segments
            .partition_point(|segment| segment.frame <= frame);
        let segment = self.segments.get(after.checked_sub(1)?)?;
        (frame < segment.until).then_some(segment)
    }

    /// The level of song frame `frame`, shaped by the transient envelope
    /// (R16): 1 at 100, and below it the segment's full level for its share
    /// of the segment, then a straight fall to silence over 10 ms, or by the
    /// next segment's beginning if that comes first.
    #[must_use]
    pub fn level(&self, frame: i64) -> f64 {
        self.level_with(frame, f64::from(self.envelope))
    }

    /// The same for a transient envelope of `envelope` (0 to 100) instead of
    /// the plan's own: what a clip whose Transient Envelope is automated
    /// plays.
    #[must_use]
    pub fn level_with(&self, frame: i64, envelope: f64) -> f64 {
        let Some(segment) = self.at(frame) else {
            return 1.0;
        };
        let span = (segment.until - segment.frame) as f64;
        let hold = span * envelope / 100.0;
        let offset = (frame - segment.frame) as f64;
        if offset <= hold {
            return 1.0;
        }
        let fall = self.fall.min((span - hold).max(0.0));
        if fall <= 0.0 {
            return 0.0;
        }
        (1.0 - (offset - hold) / fall).max(0.0)
    }

    /// The stretches the plan reads over at the clip's pitch ratio `ratio`
    /// (R14, R15, R17): each segment's own audio, then the loop mode filling
    /// what the next segment leaves, in song order. A stretch whose position
    /// is NaN plays silence.
    #[must_use]
    pub fn stretches(&self, ratio: f64) -> Vec<Stretch> {
        self.stretches_by(|_| ratio)
    }

    /// The same where each segment reads at its own pitch ratio, `ratio`
    /// says: a clip whose Transpose or Detune moves is read at the ratio it
    /// has where each segment begins (the warp modes spec, R17, R29).
    #[must_use]
    pub fn stretches_by(&self, ratio: impl Fn(&Segment) -> f64) -> Vec<Stretch> {
        let mut out = Vec::new();
        for segment in &self.segments {
            self.segment_stretches(&mut out, segment, ratio(segment));
        }
        out
    }

    fn segment_stretches(&self, out: &mut Vec<Stretch>, segment: &Segment, ratio: f64) {
        let (frame, until) = (segment.frame as f64, segment.until as f64);
        if until <= frame {
            return;
        }
        // The segment's own audio: the file up to where the next begins (R15).
        let mut at = frame;
        if segment.end > segment.position && ratio > 0.0 {
            let own = (segment.end - segment.position) / ratio;
            let stop = (frame + own).min(until);
            if stop > frame {
                out.push(Stretch {
                    frame,
                    position: segment.position,
                    slope: ratio,
                });
                at = stop;
            }
        }
        if at >= until {
            return;
        }
        let length = segment.end - segment.turn;
        if self.loop_mode == Loop::Off || length <= 0.0 || ratio <= 0.0 {
            out.push(Stretch {
                frame: at,
                position: f64::NAN,
                slope: 1.0,
            });
            return;
        }
        let pass = length / ratio;
        let back_and_forth = self.loop_mode == Loop::BackAndForth;
        let mut backwards = back_and_forth;
        while at < until {
            out.push(if backwards {
                Stretch {
                    frame: at,
                    position: segment.end,
                    slope: -ratio,
                }
            } else {
                Stretch {
                    frame: at,
                    position: segment.turn,
                    slope: ratio,
                }
            });
            at += pass;
            if back_and_forth {
                backwards = !backwards;
            }
        }
    }
}

/// The file frame a segment's loop turns at (R15): the sign change of the
/// channels' sum nearest the middle of the frames `from..to`, or the middle
/// where the segment holds none. The two channels' sum is the audio's own,
/// so a turn joins without a step.
#[must_use]
pub fn turn(channels: &[&[f32]], from: usize, to: usize) -> usize {
    if to <= from {
        return from;
    }
    let middle = from + (to - from) / 2;
    let sum = |frame: usize| -> f64 {
        channels
            .iter()
            .filter_map(|channel| channel.get(frame))
            .sum::<f32>() as f64
    };
    let changes = |frame: usize| -> bool {
        if frame <= from || frame >= to {
            return false;
        }
        let (before, here) = (sum(frame - 1), sum(frame));
        (before < 0.0 && here >= 0.0) || (before >= 0.0 && here < 0.0)
    };
    // Outwards from the middle: the nearest sign change either side of it.
    let mut below = middle;
    while below > from && !changes(below) {
        below -= 1;
    }
    let mut above = middle;
    while above < to - 1 && !changes(above) {
        above += 1;
    }
    let (below, above) = (
        changes(below).then_some(below),
        changes(above).then_some(above),
    );
    match (below, above) {
        (Some(low), Some(high)) => {
            if middle - low <= high - middle {
                low
            } else {
                high
            }
        }
        (Some(low), None) => low,
        (None, Some(high)) => high,
        (None, None) => middle,
    }
}

/// The grid lines of `division` ticks inside a pass of the clip's contents
/// from beat `beat` for `ticks` ticks, in ticks from the clip's 1.1.1,
/// counted from its own 1.1.1.
#[must_use]
pub fn grid_lines(beat: f64, ticks: f64, division: u64) -> Vec<f64> {
    let division = division as f64;
    if division <= 0.0 || ticks <= 0.0 {
        return Vec::new();
    }
    let first = (beat / division).ceil() * division;
    let mut out = Vec::new();
    let mut line = first;
    while line - beat < ticks {
        out.push(line);
        line += division;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(frame: i64, until: i64, position: f64, end: f64, turn: f64) -> Segment {
        Segment {
            frame,
            until,
            position,
            end,
            turn,
        }
    }

    // Spec: warp-modes R15. A segment that runs out of its own audio is
    // filled by its loop mode, turning at the zero crossing.
    #[test]
    fn each_loop_mode_fills_a_short_segment() {
        // 1,000 file frames from 100, the next segment beginning at 600: its
        // own audio lasts 500 song frames at ratio 1; the segment spans 2,000.
        let segment = segment(0, 2_000, 100.0, 600.0, 350.0);
        let plan = |loop_mode| Plan::new(vec![segment], loop_mode, 100, 48_000.0);
        let off = plan(Loop::Off).stretches(1.0);
        assert_eq!(off.len(), 2);
        assert_eq!(
            (off[0].frame, off[0].position, off[0].slope),
            (0.0, 100.0, 1.0)
        );
        assert_eq!((off[1].frame, off[1].slope), (500.0, 1.0));
        assert!(off[1].position.is_nan(), "silence until the next segment");
        let forward = plan(Loop::Forward).stretches(1.0);
        // The pass is 600 - 350 = 250 frames long: from 500, 750, 1,000, ...
        assert_eq!(forward.len(), 1 + 6);
        assert_eq!(forward[1].position, 350.0);
        assert_eq!(forward[1].frame, 500.0);
        assert_eq!(forward[2].position, 350.0);
        assert_eq!(forward[2].frame, 750.0);
        let forth = plan(Loop::BackAndForth).stretches(1.0);
        // Backwards from the end, then forwards to it, and so on.
        assert_eq!(forth[1].position, 600.0);
        assert_eq!(forth[1].slope, -1.0);
        assert_eq!(forth[2].position, 350.0);
        assert_eq!(forth[2].slope, 1.0);
        assert_eq!(forth[3].slope, -1.0);
    }

    // Spec: warp-modes R15. A song faster than the clip cuts each segment at
    // the next one's beginning.
    #[test]
    fn a_far_along_next_segment_cuts_the_own_audio_short() {
        // 1,000 file frames of own audio, but the next segment begins 200
        // song frames later: the own audio is cut there.
        let plan = Plan::new(
            vec![segment(0, 200, 0.0, 1_000.0, 500.0)],
            Loop::Forward,
            100,
            48_000.0,
        );
        assert_eq!(
            plan.stretches(1.0),
            vec![Stretch {
                frame: 0.0,
                position: 0.0,
                slope: 1.0
            }]
        );
        // Twice as fast reads the file twice as fast, so the same 1,000
        // frames run out 500 song frames in: the fill starts there, and the
        // segment's span cuts it before that.
        let plan = Plan::new(
            vec![segment(0, 600, 0.0, 1_000.0, 500.0)],
            Loop::Forward,
            100,
            48_000.0,
        );
        let stretches = plan.stretches(2.0);
        assert_eq!(
            stretches[0],
            Stretch {
                frame: 0.0,
                position: 0.0,
                slope: 2.0
            }
        );
        assert_eq!(
            stretches[1].frame, 500.0,
            "the own audio runs out at 1000/2"
        );
        assert_eq!(stretches[1].position, 500.0);
        assert_eq!(stretches[1].slope, 2.0);
    }

    // Spec: warp-modes R16. The envelope holds the segment's level, then
    // falls to silence over 10 ms.
    #[test]
    fn transient_envelope_holds_then_falls_over_10_ms() {
        let plan = Plan::new(
            vec![segment(0, 48_000, 0.0, 48_000.0, 24_000.0)],
            Loop::Off,
            100,
            48_000.0,
        );
        assert_eq!(plan.level(0), 1.0);
        assert_eq!(plan.level(47_999), 1.0);
        let plan = Plan::new(
            vec![segment(0, 48_000, 0.0, 48_000.0, 24_000.0)],
            Loop::Off,
            50,
            48_000.0,
        );
        // Held for the first half, then falling over 480 frames (10 ms).
        assert_eq!(plan.level(24_000), 1.0, "the hold ends there");
        assert_eq!(plan.level(23_999), 1.0);
        assert!((plan.level(24_240) - 0.5).abs() < 1e-9);
        assert_eq!(plan.level(24_480), 0.0);
        assert_eq!(plan.level(47_000), 0.0);
        // At 0 the fall starts at once.
        let plan = Plan::new(
            vec![segment(0, 48_000, 0.0, 48_000.0, 24_000.0)],
            Loop::Off,
            0,
            48_000.0,
        );
        assert_eq!(plan.level(0), 1.0, "it starts falling at once");
        assert!((plan.level(240) - 0.5).abs() < 1e-9);
        assert_eq!(plan.level(480), 0.0);
    }

    // Spec: warp-modes R16. The fall ends at the next segment's beginning
    // when that comes first.
    #[test]
    fn a_short_segment_falls_by_its_next_beginning() {
        let plan = Plan::new(
            vec![segment(0, 100, 0.0, 100.0, 50.0)],
            Loop::Off,
            0,
            48_000.0,
        );
        assert_eq!(plan.level(0), 1.0);
        assert!((plan.level(50) - 0.5).abs() < 1e-9);
        assert!(
            (plan.level(99) - 0.01).abs() < 1e-9,
            "the fall is cut at the segment's end"
        );
    }

    // Spec: warp-modes R15. The turn is the sign change of the channels' sum
    // nearest the segment's middle.
    #[test]
    fn the_turn_is_the_zero_crossing_nearest_the_middle() {
        let ramp: Vec<f32> = (0..100).map(|i| i as f32 - 40.0).collect();
        // The sign change is at 40; the middle of 0..100 is 50, and the only
        // crossing is 40.
        let mono = [ramp.as_slice()];
        assert_eq!(turn(&mono, 0, 100), 40);
        assert_eq!(turn(&mono, 0, 100), 40);
        // Two crossings: the one nearer the middle wins.
        let mut wavy = vec![1.0_f32; 100];
        for (i, value) in wavy.iter_mut().enumerate() {
            *value = if i < 30 { -1.0 } else { 1.0 };
        }
        assert_eq!(turn(&[wavy.as_slice()], 0, 100), 30);
        // The channels' sum decides: a second channel flips the sign change.
        let up: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let down: Vec<f32> = (0..100).map(|i| -(i as f32)).collect();
        assert_eq!(
            turn(&[up.as_slice(), down.as_slice()], 0, 100),
            50,
            "the sum is flat, so the middle"
        );
        // No crossing at all: the middle.
        let steady = vec![1.0_f32; 100];
        assert_eq!(turn(&[steady.as_slice()], 0, 100), 50);
    }

    // Spec: warp-modes R14. The grid lines of a pass are counted from the
    // clip's own 1.1.1.
    #[test]
    fn grid_lines_count_from_the_clips_one() {
        let quarter = TICKS_PER_QUARTER as u64;
        let lines = grid_lines(0.0, 4.0 * quarter as f64, quarter);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], 0.0);
        assert_eq!(lines[3], 3.0 * quarter as f64);
        // A pass that begins between lines starts at the next one.
        let lines = grid_lines(quarter as f64 * 1.5, quarter as f64 * 2.0, quarter);
        assert_eq!(lines, vec![2.0 * quarter as f64, 3.0 * quarter as f64]);
    }
}
