//! The voice shifter: pitch-synchronous overlap-add on the tracker's pitch.

use std::collections::VecDeque;
use std::f64::consts::PI;

use super::tracker::{Config, Pitch, Tracker};

/// How far a ratio may take the pitch (two octaves each way) or the formants
/// (an octave each way).
const PITCH_RATIO: (f32, f32) = (0.25, 4.0);
const FORMANT_RATIO: (f32, f32) = (0.5, 2.0);
/// Readings of the tracker kept, enough to cover the shifter's latency.
const READINGS: usize = 64;
/// Seconds the wet and the dry take to trade places.
const FADE: f64 = 0.003;

/// The caller's four numbers for each input sample, which come back to it
/// with the grain that is laid where the sample was (see
/// [`VoiceShifter::next`]).
pub type Ask = [f32; 4];

/// One grain of the output, which the caller steers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grain {
    /// The input sample the grain is centred on, counted from the first.
    pub at: i64,
    /// The voice's pitch there, in hertz, as the tracker measured it.
    pub frequency: f64,
    /// Whether a pitch began here: the first grain of a run of voiced sound,
    /// or one the tracker flagged an onset at.
    pub onset: bool,
}

/// One of the tracker's measurements, placed on the input's time axis.
#[derive(Clone, Copy)]
struct Reading {
    /// The sample the measured window was centred on.
    centre: i64,
    /// The pitch's period in samples; 0 where there was none.
    period: f32,
    /// Whether the tracker flagged a pitch beginning.
    onset: bool,
}

/// A pitch mark: where a period of the voice begins, and how long the
/// periods around it were.
#[derive(Clone, Copy)]
struct Mark {
    at: i64,
    period: f32,
}

/// Moves a voice's pitch, and its formants, one sample at a time.
///
/// The [`Tracker`] hears the voice. While it is voiced, a pitch mark is laid
/// each period, on the place the waveform best repeats itself, and the output
/// is a train of windowed grains of the input, each a period or two wide and
/// centred on the mark nearest to where the grain is to go; the grains go
/// closer together for a higher pitch (so some marks are used twice) and
/// further apart for a lower one (so some are skipped). A grain is taken at
/// the formant ratio's pace, so the formants stay where they were at 1 and
/// move with the ratio otherwise. Unvoiced stretches pass through, delayed as
/// much as the voiced ones, and trade places with them over a few
/// milliseconds.
///
/// The output is the input's delay of [`latency`](Self::latency) samples,
/// shifted; it is the same at every block size, because a block only ever
/// calls [`process`](Self::process) once per sample.
pub struct VoiceShifter {
    rate: f64,
    /// The tracker, unless the caller runs one for several shifters.
    tracker: Option<Tracker>,
    window: usize,
    interval: usize,
    /// The latency now, and the most it can be.
    latency: usize,
    full: usize,
    mask: usize,
    input: Vec<f32>,
    /// What the caller asked for at each sample of the input.
    asks: Vec<Ask>,
    /// The grains added so far, and the places they cover.
    wet: Vec<f32>,
    cover: Vec<bool>,
    readings: [Reading; READINGS],
    next_reading: usize,
    /// The newest reading's centre.
    newest: i64,
    /// The centre of the last reading whose onset a grain has carried, and
    /// whether the next grain is the first of a run.
    flagged: i64,
    fresh: bool,
    latest: Option<Pitch>,
    measurements: u64,
    /// Samples taken.
    heard: i64,
    /// Whether a run of marks is under way, and whether the voice has ended
    /// at its last mark.
    active: bool,
    ended: bool,
    /// Where to start looking for a run.
    scan: i64,
    marks: VecDeque<Mark>,
    /// Where the next grain goes.
    next: f64,
    mix: f32,
    fade: f32,
}

impl VoiceShifter {
    /// A shifter for a voice from `low` to `high` hertz at `rate`. Allocates.
    #[must_use]
    pub fn new(rate: f64, low: f64, high: f64) -> Self {
        let mut shifter = Self::following(rate, low, high);
        shifter.tracker = Some(Tracker::new(&Config::for_range(rate, low, high)));
        shifter
    }

    /// A shifter for a voice from `low` to `high` hertz at `rate` that has no
    /// tracker of its own: the caller runs a [`Tracker`] made from
    /// [`Config::for_range`] of the same arguments, and gives each shifter its
    /// measurements ([`take`](Self::take)), so several shifters of one voice
    /// share one tracker. Allocates.
    #[must_use]
    pub fn following(rate: f64, low: f64, high: f64) -> Self {
        let config = Config::for_range(rate, low, high);
        // The marks are placed once the tracker's readings reach them and
        // their neighbours are in; the grains are laid once a mark follows
        // their centre; and a grain reaches a period either side: 3.25 of the
        // longest period.
        let longest = (rate / config.low).ceil() as usize;
        let latency = (13 * longest).div_ceil(4) + 4;
        let size = (latency + 6 * longest).next_power_of_two();
        let shortest = (rate / high).floor().max(1.0) as usize;
        Self {
            rate,
            tracker: None,
            window: config.window,
            interval: config.interval,
            latency,
            full: latency,
            mask: size - 1,
            input: vec![0.0; size],
            asks: vec![[1.0, 1.0, 0.0, 0.0]; size],
            wet: vec![0.0; size],
            cover: vec![false; size],
            readings: [Reading {
                centre: i64::MIN,
                period: 0.0,
                onset: false,
            }; READINGS],
            next_reading: 0,
            newest: i64::MIN,
            flagged: i64::MIN,
            fresh: false,
            latest: None,
            measurements: 0,
            heard: 0,
            active: false,
            ended: false,
            scan: 0,
            marks: VecDeque::with_capacity(size / shortest + 8),
            next: 0.0,
            mix: 0.0,
            fade: (1.0 - (-1.0 / (FADE * rate)).exp()) as f32,
        }
    }

    /// Samples the output lags the input by, [`full_latency`](Self::full_latency)
    /// unless [`set_latency`](Self::set_latency) made it less.
    #[must_use]
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// The most samples the output can lag the input by: what the lowest
    /// pitch of its range needs.
    #[must_use]
    pub fn full_latency(&self) -> usize {
        self.full
    }

    /// How many samples of input the shifter remembers: a caller's own record
    /// of what it pushed, indexed by a grain's [`at`](Grain::at) modulo this,
    /// stays valid for as long as the grain can be asked for.
    #[must_use]
    pub fn span(&self) -> usize {
        self.mask + 1
    }

    /// Makes the output lag the input by `frames` samples, from 1 to
    /// [`full_latency`](Self::full_latency): a lag short of the full one lays
    /// its grains late for a low pitch or a voice that has just begun, and
    /// the dry input stands in for each missing grain. A change starts the
    /// shifter over, so a stretch of the output is dry.
    pub fn set_latency(&mut self, frames: usize) {
        let frames = frames.clamp(1, self.full);
        if frames == self.latency {
            return;
        }
        self.latency = frames;
        self.wet.fill(0.0);
        self.cover.fill(false);
        self.marks.clear();
        self.active = false;
        self.scan = self.heard;
        self.mix = 0.0;
    }

    /// The tracker's latest measurement, or none if the voice was quiet or
    /// held no pitch in the last window. It describes the input
    /// [`Tracker::latency`] samples ago.
    #[must_use]
    pub fn pitch(&self) -> Option<Pitch> {
        self.latest
    }

    /// How many measurements the tracker has made; a caller that needs each
    /// one's onset flag reads [`pitch`](Self::pitch) when this changes.
    #[must_use]
    pub fn measurements(&self) -> u64 {
        self.measurements
    }

    /// Takes `sample`, wanting the pitch moved by `pitch` times and the
    /// formants by `formant` times (the pitch's 0.25 to 4, the formants' 0.5
    /// to 2, clamped; one that isn't a
    /// number counts as 1), and returns the
    /// output sample, that of [`latency`](Self::latency) samples before.
    ///
    /// # Panics
    ///
    /// If the shifter was made by [`following`](Self::following), which has no
    /// tracker to feed.
    pub fn process(&mut self, sample: f32, pitch: f32, formant: f32) -> f32 {
        self.push(sample, [pitch, formant, 0.0, 0.0]);
        self.tracker
            .as_mut()
            .expect("a shifter with a tracker")
            .feed(sample);
        while let Some(found) = self.tracker.as_mut().and_then(Tracker::pop) {
            self.take(found);
        }
        self.next(&mut |_, ask| [ask[0], ask[1]])
    }

    /// Takes `sample` and the caller's `ask` for it, the first step of a
    /// sample of a shifter that [follows](Self::following) a tracker: then
    /// [`take`](Self::take) each measurement the tracker finishes on this
    /// sample, and last [`next`](Self::next).
    pub fn push(&mut self, sample: f32, ask: Ask) {
        let at = self.heard as usize & self.mask;
        self.input[at] = sample;
        self.asks[at] = ask;
        self.heard += 1;
    }

    /// Takes the tracker's newest measurement, or `None` for a window that
    /// was too quiet or held no lag to read a pitch from.
    pub fn take(&mut self, found: Option<Pitch>) {
        self.latest = found;
        self.measurements += 1;
        let (ended, period, onset) = match found {
            Some(p) if p.voiced => (p.ended, (self.rate / p.frequency) as f32, p.onset),
            Some(p) => (p.ended, 0.0, false),
            // A quiet window ended when the tracker last began.
            None => (self.heard as u64 - self.interval as u64, 0.0, false),
        };
        let centre = ended as i64 - (self.window / 2) as i64;
        self.readings[self.next_reading] = Reading {
            centre,
            period,
            onset,
        };
        self.next_reading = (self.next_reading + 1) % READINGS;
        self.newest = centre;
    }

    /// Lays the marks and grains the readings and the input reach, and returns
    /// the output sample of [`latency`](Self::latency) samples ago. Each grain
    /// is the voice shifted by the two ratios `steer` gives for it, the pitch's
    /// and the formants' (0.25 to 4 and 0.5 to 2, clamped; one that isn't a number
    /// counts as 1), from the grain and the [`Ask`] pushed with the input
    /// sample the grain is centred on.
    pub fn next(&mut self, steer: &mut impl FnMut(&Grain, &Ask) -> [f32; 2]) -> f32 {
        self.place_marks();
        self.lay_grains(steer);
        self.emit()
    }

    /// The newest reading centred at or before `time`.
    fn reading(&self, time: i64) -> Reading {
        let mut best = self.readings[0];
        for reading in &self.readings[1..] {
            if reading.centre <= time && (best.centre > time || reading.centre > best.centre) {
                best = *reading;
            }
        }
        if best.centre > time {
            Reading {
                centre: i64::MIN,
                period: 0.0,
                onset: false,
            }
        } else {
            best
        }
    }

    fn at(&self, index: i64) -> f32 {
        self.input[index as usize & self.mask]
    }

    /// Lays pitch marks as far as the readings and the input reach.
    fn place_marks(&mut self) {
        loop {
            if !self.active {
                let from = self.scan;
                if self.newest <= from {
                    return;
                }
                let period = self.reading(from).period;
                if period == 0.0 {
                    self.scan += 1;
                    continue;
                }
                let reach = (1.625 * period).ceil() as i64;
                if self.heard < from + reach {
                    return;
                }
                // The first mark: the loudest sample of the first period.
                let first = (from..from + period.ceil() as i64)
                    .max_by(|a, b| self.at(*a).abs().total_cmp(&self.at(*b).abs()))
                    .unwrap_or(from);
                self.marks.clear();
                self.marks.push_back(Mark { at: first, period });
                self.active = true;
                self.fresh = true;
                self.ended = false;
                self.next = first as f64;
                continue;
            }
            if self.ended {
                return;
            }
            let Some(&last) = self.marks.back() else {
                return;
            };
            if self.newest <= last.at {
                return;
            }
            let period = self.reading(last.at).period;
            if period == 0.0 {
                self.ended = true;
                return;
            }
            if self.heard < last.at + (1.625 * period).ceil() as i64 {
                return;
            }
            let at = self.follow(last.at, period);
            if self.marks.len() == self.marks.capacity() {
                self.marks.pop_front();
            }
            self.marks.push_back(Mark { at, period });
        }
    }

    /// The place about `period` samples after `mark` where the waveform best
    /// repeats the one around `mark`.
    fn follow(&self, mark: i64, period: f32) -> i64 {
        let half = (period / 2.0) as i64;
        let expected = mark + period.round() as i64;
        let spread = (period / 8.0).ceil().max(1.0) as i64;
        let energy: f32 = (-half..=half).map(|j| self.at(mark + j).powi(2)).sum();
        let mut best = (f32::MIN, expected);
        for candidate in expected - spread..=expected + spread {
            let (mut dot, mut other) = (0.0f32, 0.0f32);
            for j in -half..=half {
                let x = self.at(candidate + j);
                dot += self.at(mark + j) * x;
                other += x * x;
            }
            let similarity = dot / (energy * other).sqrt().max(1e-12);
            if similarity > best.0 {
                best = (similarity, candidate);
            }
        }
        best.1
    }

    /// Adds the grains whose marks are in.
    fn lay_grains(&mut self, steer: &mut impl FnMut(&Grain, &Ask) -> [f32; 2]) {
        while self.active {
            let at = self.next;
            let (mut nearest, mut distance) = (None, f64::MAX);
            for (index, mark) in self.marks.iter().enumerate() {
                let away = (mark.at as f64 - at).abs();
                if away < distance {
                    (nearest, distance) = (Some(index), away);
                }
            }
            let Some(index) = nearest else {
                return;
            };
            let mark = self.marks[index];
            let last = self.marks.back().map_or(mark.at, |m| m.at);
            if self.ended {
                if at > last as f64 {
                    self.active = false;
                    self.scan = last + 1;
                    return;
                }
            } else if (last as f64) < at + f64::from(mark.period) / 2.0 {
                return;
            }

            let centre = at.round() as i64;
            let period = f64::from(mark.period);
            let reading = self.reading(centre);
            let onset = self.fresh || (reading.onset && reading.centre != self.flagged);
            self.fresh = false;
            if reading.onset {
                self.flagged = reading.centre;
            }
            let grain = Grain {
                at: centre,
                frequency: self.rate / period,
                onset,
            };
            let [pitch, formant] = steer(&grain, &self.asks[centre as usize & self.mask]);
            // A ratio that isn't a number is no change.
            let sane = |ratio: f32, (low, high): (f32, f32)| {
                if ratio.is_nan() {
                    1.0
                } else {
                    ratio.clamp(low, high)
                }
            };
            let (pitch, formant) = (sane(pitch, PITCH_RATIO), sane(formant, FORMANT_RATIO));
            let spacing = period / f64::from(pitch);
            // The grain's half-width, in output samples: no wider than a
            // period of the output or of the grain's own, nor than the
            // input's period.
            let half = spacing.min(period / f64::from(formant)).min(period);
            for out in centre - half.ceil() as i64..=centre + half.ceil() as i64 {
                let away = out as f64 - at;
                if away.abs() >= half {
                    continue;
                }
                let window = 0.5 + 0.5 * (PI * away / half).cos();
                let from = mark.at as f64 + away * f64::from(formant);
                let floor = from.floor();
                let fraction = (from - floor) as f32;
                let (a, b) = (self.at(floor as i64), self.at(floor as i64 + 1));
                let slot = out as usize & self.mask;
                self.wet[slot] += window as f32 * (a + (b - a) * fraction);
                self.cover[slot] = true;
            }
            self.next += spacing;
            let keep = self.next as i64 - 2 * mark.period.ceil() as i64;
            while self.marks.len() > 1 && self.marks[0].at < keep {
                self.marks.pop_front();
            }
        }
    }

    /// The output sample of `latency` samples ago.
    fn emit(&mut self) -> f32 {
        let time = self.heard - 1 - self.latency as i64;
        if time < 0 {
            return 0.0;
        }
        let slot = time as usize & self.mask;
        let target = if self.cover[slot] { 1.0 } else { 0.0 };
        self.mix += (target - self.mix) * self.fade;
        let out = self.mix * self.wet[slot] + (1.0 - self.mix) * self.input[slot];
        self.wet[slot] = 0.0;
        self.cover[slot] = false;
        out
    }
}
