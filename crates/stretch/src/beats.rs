//! Beats (the warp modes spec, R14 to R17): the clip played as segments.
//!
//! A segment begins at the clip's Start marker, at each warp marker and at
//! each of the file's transients (Preserve Transients), or at each line of a
//! grid of the clip's own beats. It begins on the song frame its beginning
//! falls on, reads the file from there at the clip's pitch ratio (1 until
//! WP-4.7 wires Transpose and Detune), and is replaced where the next segment
//! begins, so a song faster than the clip cuts it short (R15). A segment
//! whose own audio runs out first is filled by its transient loop mode,
//! turning at the zero crossing nearest its middle, and the transient
//! envelope shapes its level (R16). Every join is a short linear fade or
//! crossfade, never a step (R15).
//!
//! The plan holds the segments and the laws a mode plays them by: the passes
//! of the song each one reads over ([`Plan::passes`]), which a player turns
//! into the pieces of its table, and what sounds at each song frame
//! ([`Plan::mix`]). It is made off the audio thread, with the clip list
//! (R5), and a frame's output depends only on the song, its media and where
//! the clip's playback began (R6).

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

/// A straight stretch of the song over which a pass's read runs in a straight
/// line: one piece of a player's table. A pass begins as a crossfade from the
/// pass before it, over [`Plan::fade`] frames, or as a fade in from silence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stretch {
    /// The song frame the stretch begins at, perhaps between frames: where
    /// its fade in begins.
    pub frame: f64,
    /// The file position at [`frame`](Self::frame); NaN where the segment
    /// plays nothing.
    pub position: f64,
    /// File frames per song frame, signed: a Back-and-Forth pass reads
    /// backwards.
    pub slope: f64,
    /// The song frame of the transient of the segment the pass belongs to,
    /// which the transient envelope decays from (R16).
    pub begin: f64,
}

impl Stretch {
    /// The file position at song frame `frame`.
    #[must_use]
    pub fn position_at(&self, frame: f64) -> f64 {
        self.position + (frame - self.frame) * self.slope
    }
}

/// What sounds at one song frame (R15, R16): the stretch it falls in at
/// `level`, and where the stretch before it, still fading out, is read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mix {
    /// The level of the frame's own read.
    pub level: f64,
    /// The file position and level of the pass fading out under it, if one
    /// does.
    pub outgoing: Option<(f64, f64)>,
}

/// A Beats clip's plan (R14 to R17): its segments in song order, the loop
/// mode that fills a short one, and the transient envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    segments: Vec<Segment>,
    loop_mode: Loop,
    /// The transient envelope, 0 to 100 (R16): 100 at full level.
    envelope: u32,
    /// Frames a second.
    rate: f64,
    /// The length of the fade at a join (R15), in song frames.
    fade: f64,
    /// How far before a segment's transient its fade in ends (R15).
    lead: f64,
    /// Half the length of a loop turn's crossfade (R15).
    half_turn: f64,
    /// The passes the segments play, as a player reads them: set by
    /// [`with_passes`](Self::with_passes).
    passes: Vec<Stretch>,
}

impl Default for Plan {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// The length of a join's fade at Transient Envelope 100, and at 50 and below
/// (R15): a linear fade between.
const FADE_SECONDS: (f64, f64) = (0.002, 0.010);
/// How long before the transient the fade ends, at Transient Envelope 50 and
/// below, and at 100 (R15).
const LEAD_SECONDS: (f64, f64) = (0.0007, 0.002);
/// The length of a loop turn's crossfade (R15).
const TURN_SECONDS: f64 = 0.010;
/// How long before the segment's end a loop turns (R15).
const LOOP_LEAD_SECONDS: f64 = 0.022;
/// The time constant of the transient envelope's decay at 50 (R16).
const DECAY_SECONDS: f64 = 0.094;

/// The file frame a segment ending at `end` turns its loop at, at the latest
/// (R15), at `rate` frames a second.
#[must_use]
pub fn loop_end(end: f64, rate: f64) -> f64 {
    end - LOOP_LEAD_SECONDS * rate
}

impl Plan {
    /// The plan every mode that isn't Beats is given: no segment at all.
    pub const EMPTY: Self = Self {
        segments: Vec::new(),
        loop_mode: Loop::Forward,
        envelope: 100,
        rate: 0.0,
        fade: 0.0,
        lead: 0.0,
        half_turn: 0.0,
        passes: Vec::new(),
    };

    /// A plan of `segments`, which must be in song order and cover the clip's
    /// playback without a gap, at `rate` frames a second.
    #[must_use]
    pub fn new(segments: Vec<Segment>, loop_mode: Loop, envelope: u32, rate: f64) -> Self {
        // Spec: warp-modes R15. The join's fade shrinks as the envelope
        // falls from 100 to 50 and stays short below it.
        let t = ((f64::from(envelope) - 50.0) / 50.0).clamp(0.0, 1.0);
        let between = |(low, high): (f64, f64)| ((low + (high - low) * t) * rate).round();
        Self {
            segments,
            loop_mode,
            envelope,
            rate,
            fade: between(FADE_SECONDS),
            lead: between(LEAD_SECONDS),
            half_turn: (TURN_SECONDS / 2.0 * rate).round(),
            passes: Vec::new(),
        }
    }

    /// The plan with its passes made, each segment read at the pitch ratio
    /// `ratio` gives (R17).
    #[must_use]
    pub fn with_passes(mut self, ratio: impl Fn(&Segment) -> f64) -> Self {
        self.passes = self.stretches_by(ratio);
        self
    }

    /// The passes, in song order: the player's table.
    #[must_use]
    pub fn passes(&self) -> &[Stretch] {
        &self.passes
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

    /// The level of a segment `since` frames after its transient under a
    /// transient envelope of `envelope` (R16): 1 at 100 and before the
    /// transient, and otherwise a decay whose time constant grows with the
    /// envelope.
    fn decay(&self, since: f64, envelope: f64) -> f64 {
        if envelope >= 100.0 || since <= 0.0 {
            return 1.0;
        }
        if envelope <= 0.0 {
            return 0.0;
        }
        (-since / (DECAY_SECONDS * self.rate * envelope / (100.0 - envelope))).exp()
    }

    /// What sounds at song frame `frame` under a transient envelope of
    /// `envelope` (0 to 100): the pass it falls in, faded in by the join or
    /// out by the silence after it, and the pass before it while that fades
    /// out (R15, R16). Where the two read the same frames, as a clip at its
    /// own tempo does, they are one read.
    #[must_use]
    pub fn mix(&self, frame: i64, envelope: f64) -> Mix {
        let at = frame as f64;
        let after = self.passes.partition_point(|pass| pass.frame <= at);
        let i = after.saturating_sub(1);
        let silent = Mix {
            level: 0.0,
            outgoing: None,
        };
        let Some(pass) = self.passes.get(i).filter(|pass| !pass.position.is_nan()) else {
            return silent;
        };
        let mut level = self.decay(at - pass.begin, envelope);
        // Fading out into the silence that follows.
        if let Some(next) = self.passes.get(i + 1)
            && next.position.is_nan()
        {
            level *= ((next.frame - at) / self.fade).clamp(0.0, 1.0);
        }
        let came = ((at - pass.frame) / self.fade).clamp(0.0, 1.0);
        if came >= 1.0 || self.fade <= 0.0 {
            return Mix {
                level,
                outgoing: None,
            };
        }
        let Some(before) = i
            .checked_sub(1)
            .map(|before| &self.passes[before])
            .filter(|before| !before.position.is_nan())
        else {
            return Mix {
                level: level * came,
                outgoing: None,
            };
        };
        let out = (
            before.position_at(at),
            (1.0 - came) * self.decay(at - before.begin, envelope),
        );
        if (out.0 - pass.position_at(at)).abs() < 1.0 {
            // One read: the same frames to within one, as where a transient
            // lies between frames (R4), at the two levels' sum.
            let sum = level * came + out.1;
            return Mix {
                level: if level == 1.0 && out.1 == 1.0 - came {
                    1.0
                } else {
                    sum
                },
                outgoing: None,
            };
        }
        Mix {
            level: level * came,
            outgoing: Some(out),
        }
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
        for (i, segment) in self.segments.iter().enumerate() {
            let has_next = i + 1 < self.segments.len();
            self.segment_stretches(&mut out, segment, has_next, ratio(segment));
        }
        out
    }

    fn segment_stretches(
        &self,
        out: &mut Vec<Stretch>,
        segment: &Segment,
        has_next: bool,
        ratio: f64,
    ) {
        let (begin, until) = (segment.frame as f64, segment.until as f64);
        if until <= begin {
            return;
        }
        // The segment's audio starts before its transient and fades in, so
        // it crossfades with what is still sounding (R15). The next
        // segment's does the same, which is where this one's ends.
        let lead = self.fade + self.lead;
        let start = begin - lead;
        let join = if has_next { until - lead } else { until };
        let silent = |frame| Stretch {
            frame,
            position: f64::NAN,
            slope: 1.0,
            begin,
        };
        if segment.end <= segment.position || ratio <= 0.0 {
            out.push(silent(start));
            return;
        }
        let first = segment.position - lead * ratio;
        out.push(Stretch {
            frame: start,
            position: first,
            slope: ratio,
            begin,
        });
        // Audio that lasts until the next segment begins, to within a frame,
        // plays on through the join: no fade out, no loop.
        if (segment.end - segment.position) / ratio + 1.0 >= until - begin {
            return;
        }
        // The segment's own audio ends a moment before the file frame the
        // next begins at, with a fade out that the silence after it ends.
        let own = start + (segment.end - self.lead * ratio - first) / ratio;
        // A loop turns shortly before that and at the turn, which is the
        // middle of what lies between; its crossfade is centred on it.
        let (at, turn) = (loop_end(segment.end, self.rate), segment.turn);
        let pass = (at - turn) / ratio;
        let half = self.half_turn;
        let mut centre = start + (at - first) / ratio;
        if self.loop_mode == Loop::Off || pass < 2.0 * half || centre - half <= start {
            if own < join {
                out.push(silent(own));
            }
            return;
        }
        let back_and_forth = self.loop_mode == Loop::BackAndForth;
        let mut backwards = back_and_forth;
        while centre + half <= join {
            // At the centre the new pass reads the file frame the old one
            // has reached, so the two crossfade over the same audio.
            out.push(if backwards {
                Stretch {
                    frame: centre - half,
                    position: at + half * ratio,
                    slope: -ratio,
                    begin,
                }
            } else {
                Stretch {
                    frame: centre - half,
                    position: turn - half * ratio,
                    slope: ratio,
                    begin,
                }
            });
            centre += pass;
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

    const RATE: f64 = 48_000.0;

    /// A segment of 480 ms of audio from the file's start, on a song second,
    /// whose loop turns at the middle of its first 458 ms.
    fn long(until: i64) -> Segment {
        segment(0, until, 0.0, 24_000.0, 11_472.0)
    }

    // Spec: warp-modes R15. A segment starts 12 ms before its transient, and
    // when its own audio runs out, a Loop Off segment fades out over 10 ms
    // ending 2 ms before the file frame the next would begin at, then is
    // silent.
    #[test]
    fn loop_off_fades_out_then_falls_silent() {
        let plan = Plan::new(vec![long(48_000)], Loop::Off, 100, RATE);
        let passes = plan.stretches(1.0);
        assert_eq!(passes.len(), 2);
        assert_eq!(
            (passes[0].frame, passes[0].position, passes[0].slope),
            (-576.0, -576.0, 1.0),
            "12 ms before the transient, reading the file 12 ms before it"
        );
        assert_eq!(
            passes[1].frame, 23_904.0,
            "2 ms before the file's frame 24,000"
        );
        assert!(passes[1].position.is_nan());
        let plan = plan.with_passes(|_| 1.0);
        assert_eq!(
            plan.mix(-576, 100.0).level,
            0.0,
            "the fade in begins silent"
        );
        assert!((plan.mix(-336, 100.0).level - 0.5).abs() < 1e-12);
        assert_eq!(plan.mix(-96, 100.0).level, 1.0, "full level 2 ms early");
        assert_eq!(plan.mix(23_424, 100.0).level, 1.0);
        assert!((plan.mix(23_664, 100.0).level - 0.5).abs() < 1e-12);
        assert_eq!(plan.mix(23_904, 100.0).level, 0.0);
        assert_eq!(plan.mix(40_000, 100.0).level, 0.0);
    }

    // Spec: warp-modes R15. Loop Forward turns about 22 ms before the file
    // frame the next segment would begin at, back to the middle, in
    // crossfades of 10 ms centred on the turn, identical on every pass.
    #[test]
    fn loop_forward_jumps_back_to_the_middle_in_a_crossfade() {
        let plan = Plan::new(vec![long(48_000)], Loop::Forward, 100, RATE);
        let passes = plan.stretches(1.0);
        // Turns at song frame 22,944 (file frame 22,944), then every 11,472.
        let frames: Vec<f64> = passes.iter().map(|pass| pass.frame).collect();
        assert_eq!(frames, [-576.0, 22_704.0, 34_176.0, 45_648.0]);
        for pass in &passes[1..] {
            assert_eq!((pass.position, pass.slope), (11_472.0 - 240.0, 1.0));
        }
        let plan = plan.with_passes(|_| 1.0);
        // Midway through the first turn both ends sound at half level, the
        // old one at the frame it has reached and the new one 5 ms before the
        // middle.
        let mix = plan.mix(22_944, 100.0);
        assert!((mix.level - 0.5).abs() < 1e-12);
        let (position, level) = mix.outgoing.unwrap();
        assert_eq!(position, 22_944.0);
        assert!((level - 0.5).abs() < 1e-12);
        assert_eq!(plan.stretches(1.0)[1].position_at(22_944.0), 11_472.0);
    }

    // Spec: warp-modes R15. Back-and-Forth turns at the same places, reading
    // backwards from the end to the middle and forwards again.
    #[test]
    fn back_and_forth_turns_in_crossfades_with_the_same_passes() {
        let passes = Plan::new(vec![long(48_000)], Loop::BackAndForth, 100, RATE).stretches(1.0);
        let frames: Vec<f64> = passes.iter().map(|pass| pass.frame).collect();
        assert_eq!(frames, [-576.0, 22_704.0, 34_176.0, 45_648.0]);
        let slopes: Vec<f64> = passes.iter().map(|pass| pass.slope).collect();
        assert_eq!(slopes, [1.0, -1.0, 1.0, -1.0]);
        // The backward pass is at the end's turn at its centre, 5 ms on from
        // where it begins.
        assert_eq!(passes[1].position, 22_944.0 + 240.0);
        assert_eq!(passes[1].position_at(22_944.0), 22_944.0);
        assert_eq!(passes[2].position_at(34_416.0), 11_472.0);
    }

    // Spec: warp-modes R15. A song faster than the clip cuts a segment short
    // with a crossfade of 10 ms into the next, which began 12 ms before its
    // transient.
    #[test]
    fn a_segment_cut_short_crossfades_into_the_next() {
        let plan = Plan::new(
            vec![
                long(12_000),
                segment(12_000, 24_000, 24_000.0, 48_000.0, 36_000.0),
            ],
            Loop::BackAndForth,
            100,
            RATE,
        )
        .with_passes(|_| 1.0);
        assert_eq!(plan.passes().len(), 2, "no turn fits before the join");
        assert_eq!(plan.passes()[1].frame, 12_000.0 - 576.0);
        assert_eq!(
            plan.mix(11_423, 100.0).outgoing,
            None,
            "the old read is alone until then"
        );
        let mix = plan.mix(11_424 + 240, 100.0);
        assert!((mix.level - 0.5).abs() < 1e-12);
        let (position, level) = mix.outgoing.unwrap();
        assert!((level - 0.5).abs() < 1e-12);
        assert_eq!(position, 11_664.0 + 576.0 - 576.0);
        assert_eq!(plan.mix(11_424 + 480, 100.0).outgoing, None);
        assert_eq!(plan.mix(11_424 + 480, 100.0).level, 1.0);
    }

    // Spec: warp-modes R4, R15. Segments that follow one another in the file
    // read the same frames through a join, which is no crossfade at all.
    #[test]
    fn a_continuous_join_is_untouched() {
        let plan = Plan::new(
            vec![
                segment(0, 24_000, 0.0, 24_000.0, 12_000.0),
                segment(24_000, 48_000, 24_000.0, 48_000.0, 36_000.0),
            ],
            Loop::BackAndForth,
            100,
            RATE,
        )
        .with_passes(|_| 1.0);
        for frame in 23_000..24_500 {
            let mix = plan.mix(frame, 100.0);
            assert_eq!((mix.level, mix.outgoing), (1.0, None), "frame {frame}");
        }
        // A transient between frames starts its segment on the nearest one:
        // the two reads are a fraction of a frame apart, still one read.
        let plan = Plan::new(
            vec![
                segment(0, 24_000, 0.0, 23_999.993, 12_000.0),
                segment(24_000, 48_000, 23_999.993, 48_000.0, 36_000.0),
            ],
            Loop::BackAndForth,
            100,
            RATE,
        )
        .with_passes(|_| 1.0);
        for frame in 23_000..24_500 {
            let mix = plan.mix(frame, 100.0);
            assert_eq!((mix.level, mix.outgoing), (1.0, None), "frame {frame}");
        }
    }

    // Spec: warp-modes R16. The envelope decays each segment from its
    // transient by a time constant that grows with it, and the join's fade in
    // shortens to 2 ms.
    #[test]
    fn transient_envelope_decays_from_the_transient() {
        let at = |envelope, frame| {
            Plan::new(vec![long(48_000)], Loop::Off, envelope, RATE)
                .with_passes(|_| 1.0)
                .mix(frame, f64::from(envelope))
                .level
        };
        assert_eq!(at(100, 20_000), 1.0, "nothing decays at 100");
        // Half level 65 ms after the transient, a quarter at 133 ms.
        assert!((at(50, 3_120) - 0.5).abs() < 0.01);
        assert!((at(50, 6_384) - 0.25).abs() < 0.01);
        assert_eq!(at(50, 0), 1.0);
        assert_eq!(at(0, 1), 0.0, "at 0 a segment dies at once");
        let plan = Plan::new(vec![long(48_000)], Loop::Off, 50, RATE).with_passes(|_| 1.0);
        assert_eq!(
            plan.passes()[0].frame,
            -130.0,
            "a 2 ms fade in, ending 0.7 ms early"
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
