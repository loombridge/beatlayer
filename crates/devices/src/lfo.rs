//! The effects LFO (WP-5.42; the device framework spec, R26 and R27): one
//! low-frequency oscillator for every effect that moves something with a
//! wave, with free rates in Hz and in seconds, rates synced to the song's
//! tempo map and position, the Phase and Spin stereo modes, a phase offset
//! against the beat grid, and a duty cycle.
//!
//! It is made off the audio thread and allocates nothing after that. Its
//! value at a frame depends only on the frame (while the song plays and the
//! rate is synced) or on how many frames it has run (otherwise), never on
//! how the blocks fall, so a render is the same at any block size.

use std::f64::consts::TAU;

use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_dsp::Random;

use crate::framework::Audio;

/// A waveform. Every one runs from −1 to 1.
///
/// A device offers the ones its manual lists, under the manual's names, and
/// maps its chooser onto these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    /// A sine, starting at 0 and rising.
    Sine,
    /// A triangle, starting at its bottom.
    Triangle,
    /// A triangle bent so its top is pointed and its bottom rounded.
    SharkTooth,
    /// A rectangle through a low-pass filter, so its edges soften as the
    /// rate rises.
    TriangleAnalog,
    /// A triangle in 8 steps to the cycle.
    Triangle8,
    /// A triangle in 16 steps to the cycle.
    Triangle16,
    /// A ramp up.
    SawUp,
    /// A ramp down.
    SawDown,
    /// A rectangle, high for the first half of the cycle.
    Rectangle,
    /// A new random value each cycle, eased smoothly to the next.
    Random,
    /// A new random value each cycle, joined to the next by a straight
    /// line.
    Wander,
    /// A new random value each cycle, held.
    SampleHold,
}

impl Wave {
    /// Every waveform, in the order of [`Wave::from_index`].
    pub const ALL: [Wave; 12] = [
        Wave::Sine,
        Wave::Triangle,
        Wave::SharkTooth,
        Wave::TriangleAnalog,
        Wave::Triangle8,
        Wave::Triangle16,
        Wave::SawUp,
        Wave::SawDown,
        Wave::Rectangle,
        Wave::Random,
        Wave::Wander,
        Wave::SampleHold,
    ];

    /// Its name in a chooser.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Sine => "Sine",
            Self::Triangle => "Triangle",
            Self::SharkTooth => "Shark Tooth",
            Self::TriangleAnalog => "Triangle Analog",
            Self::Triangle8 => "Triangle 8",
            Self::Triangle16 => "Triangle 16",
            Self::SawUp => "Saw Up",
            Self::SawDown => "Saw Down",
            Self::Rectangle => "Rectangle",
            Self::Random => "Random",
            Self::Wander => "Wander",
            Self::SampleHold => "Sample & Hold",
        }
    }

    /// The waveform at `index` in [`Wave::ALL`], the last for an index past
    /// the end.
    #[must_use]
    pub fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    /// Whether it follows random values: a duty cycle doesn't touch it.
    #[must_use]
    pub fn is_random(self) -> bool {
        matches!(self, Self::Random | Self::Wander | Self::SampleHold)
    }
}

/// How fast the LFO runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rate {
    /// Cycles per second.
    Hertz(f64),
    /// Seconds per cycle.
    Seconds(f64),
    /// Quarter notes per cycle, at the song's tempo: a bar of 4/4 is 4.0,
    /// a dotted eighth 0.75, a triplet quarter 2/3. While the song plays
    /// the cycle starts on the beat grid.
    Beats(f64),
}

/// How the two channels' LFOs differ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stereo {
    /// The right LFO is this many degrees ahead; 180 is antiphase.
    Phase(f64),
    /// The right LFO's rate differs from the left's by this many per cent.
    Spin(f64),
}

/// What an LFO draws at one frame, which a device may move frame by frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// The waveform.
    pub wave: Wave,
    /// A second waveform to blend toward, and how far from 0 to 1.
    pub morph: Option<(Wave, f64)>,
    /// The duty cycle from −1 to 1. Positive squeezes the waveform toward
    /// the front of the cycle and negative toward the back, down to a tenth
    /// of the cycle at ±1; 0 leaves it as it is.
    pub duty: f64,
}

impl Shape {
    /// `wave` as it is.
    #[must_use]
    pub fn of(wave: Wave) -> Self {
        Self {
            wave,
            morph: None,
            duty: 0.0,
        }
    }
}

/// How much of the cycle a waveform keeps at a duty cycle of ±1.
const SQUEEZED: f64 = 0.1;

/// The time constant of Triangle Analog's low-pass, in seconds.
const ANALOG_SECONDS: f64 = 0.010;

/// One channel's oscillator.
#[derive(Clone, Debug)]
struct Voice {
    /// Where it was in its cycle, from 0 to 1.
    phase: f64,
    random: Random,
    /// The random value this cycle holds and the next.
    held: f64,
    next: f64,
    /// Triangle Analog's filter state.
    analog: f64,
}

impl Voice {
    fn new(seed: u64) -> Self {
        let mut random = Random::seeded(seed);
        let (held, next) = (random.next_bipolar(), random.next_bipolar());
        Self {
            phase: 0.0,
            random,
            held,
            next,
            analog: 0.0,
        }
    }

    /// Moves to `phase`, taking a new random value if the cycle wrapped.
    fn advance(&mut self, phase: f64) {
        if phase < self.phase {
            self.held = self.next;
            self.next = self.random.next_bipolar();
        }
        self.phase = phase;
    }

    /// The value at `phase` of one waveform. `hz` is the LFO's rate in
    /// cycles per second, `a` Triangle Analog's filter coefficient.
    fn raw(&mut self, wave: Wave, phase: f64, hz: f64, a: f64) -> f64 {
        let triangle = |p: f64| 1.0 - 4.0 * (p - 0.5).abs();
        let stepped = |p: f64, steps: f64| triangle(((p * steps).floor() + 0.5) / steps);
        match wave {
            Wave::Sine => (TAU * phase).sin(),
            Wave::Triangle => triangle(phase),
            Wave::SharkTooth => {
                let up = (triangle(phase) + 1.0) / 2.0;
                2.0 * up * up - 1.0
            }
            Wave::TriangleAnalog => {
                let rectangle = if phase < 0.5 { 1.0 } else { -1.0 };
                self.analog += a * (rectangle - self.analog);
                // The filter's steady-state peak, so the wave keeps to ±1.
                let peak = (0.25 / (hz.max(1e-6) * ANALOG_SECONDS)).tanh();
                (self.analog / peak).clamp(-1.0, 1.0)
            }
            Wave::Triangle8 => stepped(phase, 8.0),
            Wave::Triangle16 => stepped(phase, 16.0),
            Wave::SawUp => 2.0 * phase - 1.0,
            Wave::SawDown => 1.0 - 2.0 * phase,
            Wave::Rectangle => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Wave::SampleHold => self.held,
            Wave::Wander => self.held + (self.next - self.held) * phase,
            Wave::Random => {
                let eased = phase * phase * (3.0 - 2.0 * phase);
                self.held + (self.next - self.held) * eased
            }
        }
    }

    /// The value of `shape` at `phase`.
    fn value(&mut self, shape: Shape, phase: f64, hz: f64, a: f64) -> f64 {
        let squeeze = |wave: Wave| {
            if wave.is_random() {
                return phase;
            }
            let kept = 1.0 - shape.duty.abs().min(1.0) * (1.0 - SQUEEZED);
            if shape.duty >= 0.0 {
                (phase / kept).min(1.0)
            } else {
                ((phase - (1.0 - kept)) / kept).max(0.0)
            }
        };
        let first = self.raw(shape.wave, squeeze(shape.wave), hz, a);
        match shape.morph {
            Some((to, amount)) if amount > 0.0 => {
                let second = self.raw(to, squeeze(to), hz, a);
                first + (second - first) * amount.min(1.0)
            }
            _ => first,
        }
    }
}

/// A stereo pair of LFOs: the left channel's and the right's.
#[derive(Clone, Debug)]
pub struct Lfo {
    sample_rate: SampleRate,
    voices: [Voice; 2],
    /// Where the free-running LFO is, for the left and for the right in
    /// Spin; the right in Phase is the left plus the offset.
    free: [f64; 2],
    rate: Rate,
    stereo: Stereo,
    /// The phase offset in cycles, against the beat grid.
    offset: f64,
    /// The song position of the stretch's first frame, and whether the song
    /// plays.
    start: i64,
    playing: bool,
    /// Song frames per frame of the stretch (the transport spec, R24).
    speed: f64,
    /// Cycles per frame, free-running.
    step: f64,
    /// Cycles per second, free-running.
    hz: f64,
    /// Triangle Analog's filter coefficient.
    analog: f64,
}

impl Lfo {
    /// An LFO for a device whose random shapes follow `seed`: the same seed
    /// gives the same values every run, and devices with other seeds other
    /// values. Both channels follow the same random values; Phase and Spin
    /// set where each is in them.
    #[must_use]
    pub fn new(sample_rate: SampleRate, seed: u64) -> Self {
        let rate = sample_rate.as_f64();
        Self {
            sample_rate,
            voices: [Voice::new(seed), Voice::new(seed)],
            free: [0.0; 2],
            rate: Rate::Hertz(1.0),
            stereo: Stereo::Phase(0.0),
            offset: 0.0,
            start: 0,
            playing: false,
            speed: 1.0,
            step: 1.0 / rate,
            hz: 1.0,
            analog: 1.0 - (-1.0 / (ANALOG_SECONDS * rate)).exp(),
        }
    }

    /// Sets the LFO for the stretch of audio in `audio`: its rate, how the
    /// channels differ, and the phase offset in degrees (which a synced
    /// rate takes against the beat grid, and a free rate ignores). A synced
    /// rate takes the tempo at the stretch's first frame, from the song's
    /// tempo map, for the stretch's free-running steps.
    pub fn set(&mut self, audio: &Audio<'_, '_>, rate: Rate, stereo: Stereo, offset_degrees: f64) {
        self.configure(
            audio.song_position().0,
            audio.playing(),
            audio.bpm(),
            rate,
            stereo,
            offset_degrees,
        );
        self.speed = audio.speed();
    }

    fn configure(
        &mut self,
        start: i64,
        playing: bool,
        bpm: f64,
        rate: Rate,
        stereo: Stereo,
        offset_degrees: f64,
    ) {
        self.start = start;
        self.playing = playing;
        self.rate = rate;
        self.stereo = stereo;
        self.offset = offset_degrees / 360.0;
        self.hz = match rate {
            Rate::Hertz(hz) => hz,
            Rate::Seconds(seconds) => 1.0 / seconds.max(1e-9),
            Rate::Beats(beats) => bpm / 60.0 / beats.max(1e-9),
        }
        .max(0.0);
        self.step = self.hz / self.sample_rate.as_f64();
    }

    /// Where each channel's LFO is in its cycle after the last
    /// [`step`](Self::step), from 0 to 1: for a device that holds the LFO in
    /// steps within each cycle.
    #[must_use]
    pub fn phases(&self) -> [f64; 2] {
        [self.voices[0].phase, self.voices[1].phase]
    }

    /// Moves on one frame (the `frame`th of the stretch) and gives the left
    /// and right values of `shape`. `tempo` is the song's tempo map
    /// ([`Audio::tempo`]).
    pub fn step(&mut self, tempo: &TempoMap, frame: usize, shape: Shape) -> [f64; 2] {
        self.step_with_multiplier(tempo, frame, shape, 1.0)
    }

    /// Moves on one frame with an instantaneous rate multiplier, for dynamic
    /// frequency modulation.
    pub fn step_with_multiplier(
        &mut self,
        tempo: &TempoMap,
        frame: usize,
        shape: Shape,
        multiplier: f64,
    ) -> [f64; 2] {
        let spin = match self.stereo {
            Stereo::Spin(percent) => 1.0 + percent / 100.0,
            Stereo::Phase(_) => 1.0,
        };
        let synced = match self.rate {
            Rate::Beats(beats) if self.playing => Some(beats),
            _ => None,
        };
        let mut phases = if let Some(beats) = synced {
            // Locked to the song: the phase is a function of the frame, so
            // a tempo change and a jump of the playhead keep the grid.
            let at = Frames(self.start + (frame as f64 * self.speed).round() as i64);
            let quarters = tempo.frames_to_ticks(at, self.sample_rate).as_quarters();
            let cycles = quarters / beats;
            if (multiplier - 1.0).abs() < 1e-9 {
                self.free = [cycles.rem_euclid(1.0), (cycles * spin).rem_euclid(1.0)];
                [cycles + self.offset, cycles * spin + self.offset]
            } else {
                let bpm = tempo
                    .bpm_at(tempo.frames_to_ticks(at, self.sample_rate))
                    .get()
                    * self.speed;
                let step = (bpm / 60.0 / beats.max(1e-9) / self.sample_rate.as_f64()) * multiplier;
                let now = self.free;
                self.free = [
                    (now[0] + step).rem_euclid(1.0),
                    (now[1] + step * spin).rem_euclid(1.0),
                ];
                [now[0] + self.offset, now[1] + self.offset]
            }
        } else {
            let step = self.step * multiplier;
            let now = self.free;
            self.free = [
                (now[0] + step).rem_euclid(1.0),
                (now[1] + step * spin).rem_euclid(1.0),
            ];
            [now[0], now[1]]
        };
        if let Stereo::Phase(degrees) = self.stereo {
            phases[1] = phases[0] + degrees / 360.0;
        }
        let mut out = [0.0; 2];
        for ((value, voice), phase) in out.iter_mut().zip(&mut self.voices).zip(phases) {
            let phase = phase.rem_euclid(1.0);
            voice.advance(phase);
            *value = voice.value(shape, phase, self.hz * multiplier, self.analog);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, TempoMap};

    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;

    fn lfo(rate: Rate, stereo: Stereo) -> Lfo {
        let mut lfo = Lfo::new(RATE, 7);
        lfo.configure(0, false, 120.0, rate, stereo, 0.0);
        lfo
    }

    // Spec: device-framework R26
    #[test]
    fn every_wave_stays_between_minus_one_and_one_and_repeats_at_its_rate() {
        let tempo = TempoMap::constant(Bpm::DEFAULT);
        for wave in Wave::ALL {
            for duty in [-1.0, -0.4, 0.0, 0.7, 1.0] {
                let mut lfo = lfo(Rate::Hertz(10.0), Stereo::Phase(0.0));
                let shape = Shape {
                    wave,
                    morph: Some((Wave::Sine, 0.3)),
                    duty,
                };
                let values: Vec<f64> = (0..4_800)
                    .map(|frame| lfo.step(&tempo, frame, shape)[0])
                    .collect();
                assert!(
                    values.iter().all(|v| (-1.0..=1.0).contains(v)),
                    "{wave:?} at duty {duty}"
                );
            }
            // Ten cycles in 48,000 frames: 4,800 frames per cycle. A wave
            // that jumps may land a frame either side of its jump, on the
            // other side of a rounding error in the phase.
            if !wave.is_random() && wave != Wave::TriangleAnalog {
                let mut lfo = lfo(Rate::Hertz(10.0), Stereo::Phase(0.0));
                let values: Vec<f64> = (0..9_600)
                    .map(|frame| lfo.step(&tempo, frame, Shape::of(wave))[0])
                    .collect();
                let apart = (0..4_800)
                    .filter(|&frame| (values[frame] - values[frame + 4_800]).abs() > 1e-9)
                    .count();
                assert!(apart <= 16, "{wave:?} repeats: {apart} frames differ");
            }
        }
    }

    // Spec: device-framework R26
    #[test]
    fn the_shapes_are_where_the_spec_puts_them() {
        let mut voice = Voice::new(1);
        let mut at = |wave, phase| voice.raw(wave, phase, 1.0, 1.0);
        assert!((at(Wave::Sine, 0.25) - 1.0).abs() < 1e-12);
        assert!((at(Wave::Sine, 0.75) + 1.0).abs() < 1e-12);
        assert_eq!(at(Wave::Triangle, 0.0), -1.0);
        assert_eq!(at(Wave::Triangle, 0.5), 1.0);
        assert_eq!(at(Wave::SawUp, 0.0), -1.0);
        assert_eq!(at(Wave::SawDown, 0.0), 1.0);
        assert_eq!(at(Wave::Rectangle, 0.49), 1.0);
        assert_eq!(at(Wave::Rectangle, 0.5), -1.0);
        // 8 steps to the cycle: 8 distinct values over it.
        let mut steps: Vec<i64> = (0..800)
            .map(|i| (at(Wave::Triangle8, f64::from(i) / 800.0) * 1e6).round() as i64)
            .collect();
        steps.dedup();
        steps.sort_unstable();
        steps.dedup();
        assert_eq!(
            steps.len(),
            4,
            "a triangle's 8 steps take 4 distinct heights"
        );
    }

    // Spec: device-framework R26
    #[test]
    fn a_duty_cycle_moves_the_wave_toward_the_front_or_the_back() {
        let mut voice = Voice::new(1);
        let mut at = |duty, phase| {
            voice.value(
                Shape {
                    duty,
                    ..Shape::of(Wave::SawUp)
                },
                phase,
                1.0,
                1.0,
            )
        };
        assert_eq!(at(0.0, 0.5), 0.0, "unchanged");
        assert!(
            at(1.0, 0.5) > 0.99,
            "squeezed to the front, it has finished"
        );
        assert!(
            at(1.0, 0.05).abs() < 1e-9,
            "a tenth of the cycle holds all of it"
        );
        assert!(
            at(-1.0, 0.5) < -0.99,
            "squeezed to the back, it has not begun"
        );
        assert!(at(-1.0, 0.95).abs() < 1e-9);
    }

    // Spec: device-framework R29
    #[test]
    fn random_shapes_follow_the_seed() {
        let tempo = TempoMap::constant(Bpm::DEFAULT);
        let run = |seed| {
            let mut lfo = Lfo::new(RATE, seed);
            lfo.configure(0, false, 120.0, Rate::Hertz(100.0), Stereo::Phase(0.0), 0.0);
            (0..4_800)
                .map(|frame| lfo.step(&tempo, frame, Shape::of(Wave::SampleHold))[0])
                .collect::<Vec<_>>()
        };
        assert_eq!(run(1), run(1));
        assert_ne!(run(1), run(2));
    }
}
