//! Analog's per-voice LFOs and vibrato (WP-8.15; spec: Analog R18, R19,
//! R29). A voice's LFO and vibrato each hold a phase and a random source
//! seeded when its note starts, from the device's seed, so a render repeats
//! run after run and is the same at any block size: nothing here depends on
//! how blocks fall, only on frames counted from the note's.

use core::f64::consts::TAU;

use bl_dsp::random::Random;

pub const SINE: usize = 0;
pub const TRIANGLE: usize = 1;
pub const RECTANGLE: usize = 2;
pub const SAMPLE_AND_HOLD: usize = 3;
pub const NOISE: usize = 4;

/// Spec: analog R19. Vibrato's reach at Amount 100 %, in semitones, and how
/// far Error varies a voice's rate and amount.
pub const VIBRATO_SEMITONES: f32 = 1.0;
pub const VIBRATO_ERROR: f64 = 0.5;

/// Spec: analog R18. The LFO's value, from −1 to 1, `phase` of the way
/// (0 to 1) through a cycle of `kind`, with `held` and `next` the random
/// waves' values at the cycle's start and end. `width` (0 to 1) skews the
/// triangle's peak and is the rectangle's duty cycle.
#[must_use]
pub fn wave(kind: usize, phase: f32, width: f32, held: f32, next: f32) -> f32 {
    let width = width.clamp(0.001, 0.999);
    match kind {
        SINE => (core::f32::consts::TAU * phase).sin(),
        TRIANGLE if phase < width => -1.0 + 2.0 * phase / width,
        TRIANGLE => 1.0 - 2.0 * (phase - width) / (1.0 - width),
        RECTANGLE if phase < width => 1.0,
        RECTANGLE => -1.0,
        SAMPLE_AND_HOLD => held,
        NOISE => held + (next - held) * phase,
        _ => 0.0,
    }
}

/// Spec: analog R18, R19. How far a delayed and faded-in signal has come
/// `seconds` into a note: 0 before `delay`, then rising straight to 1 over
/// `attack`.
#[must_use]
pub fn fade(seconds: f64, delay: f64, attack: f64) -> f32 {
    if seconds < delay {
        0.0
    } else if attack > 0.0 {
        ((seconds - delay) / attack).min(1.0) as f32
    } else {
        1.0
    }
}

/// One voice's LFO.
#[derive(Clone, Debug)]
pub struct Lfo {
    phase: f64,
    held: f32,
    next: f32,
    random: Random,
}

impl Lfo {
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            held: 0.0,
            next: 0.0,
            random: Random::seeded(0),
        }
    }

    /// Starts at `phase` cycles (taken modulo 1), drawing its random values
    /// from `seed`.
    pub fn start(&mut self, seed: u64, phase: f64) {
        self.random = Random::seeded(seed);
        self.phase = phase.rem_euclid(1.0);
        self.held = self.random.next_bipolar() as f32;
        self.next = self.random.next_bipolar() as f32;
    }

    /// The value now, then a frame on at `step` cycles a frame.
    pub fn step(&mut self, kind: usize, width: f32, step: f64) -> f32 {
        let value = wave(kind, self.phase as f32, width, self.held, self.next);
        self.phase += step;
        if self.phase >= 1.0 {
            self.phase = self.phase.fract();
            self.held = self.next;
            self.next = self.random.next_bipolar() as f32;
        }
        value
    }

    /// Where in its cycle it is, 0 to 1.
    #[cfg(test)]
    #[must_use]
    pub fn phase(&self) -> f64 {
        self.phase
    }
}

impl Default for Lfo {
    fn default() -> Self {
        Self::new()
    }
}

/// One voice's vibrato: a sine whose rate and amount this voice's note
/// varied by Error.
#[derive(Clone, Copy, Debug)]
pub struct Vibrato {
    phase: f64,
    rate: f64,
    amount: f64,
}

impl Vibrato {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            phase: 0.0,
            rate: 1.0,
            amount: 1.0,
        }
    }

    /// Starts a note's vibrato with Error `error` (0 to 1), drawing the
    /// factors it varies rate and amount by from `random`.
    pub fn start(&mut self, error: f64, random: &mut Random) {
        self.phase = 0.0;
        self.rate = 1.0 + error * VIBRATO_ERROR * random.next_bipolar();
        self.amount = 1.0 + error * VIBRATO_ERROR * random.next_bipolar();
    }

    /// The pitch move now, in semitones, for `amount` (0 to 1) already
    /// faded in; then a frame on at `rate` hertz.
    pub fn step(&mut self, amount: f32, rate: f64, frame: f64) -> f32 {
        let moved = VIBRATO_SEMITONES
            * (amount * self.amount as f32).clamp(0.0, 1.0)
            * (TAU * self.phase).sin() as f32;
        self.phase = (self.phase + rate * self.rate * frame).fract();
        moved
    }
}

impl Default for Vibrato {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: analog R18
    #[test]
    fn each_wave_runs_its_shape_over_a_cycle() {
        assert!(wave(SINE, 0.25, 0.5, 0.0, 0.0) > 0.999);
        assert_eq!(wave(TRIANGLE, 0.0, 0.5, 0.0, 0.0), -1.0);
        assert_eq!(wave(TRIANGLE, 0.5, 0.5, 0.0, 0.0), 1.0);
        assert!((wave(TRIANGLE, 0.25, 0.5, 0.0, 0.0)).abs() < 1e-6);
        assert_eq!(wave(TRIANGLE, 0.1, 0.1, 0.0, 0.0), 1.0, "width moves the peak");
        assert_eq!(wave(RECTANGLE, 0.2, 0.25, 0.0, 0.0), 1.0);
        assert_eq!(wave(RECTANGLE, 0.3, 0.25, 0.0, 0.0), -1.0, "width is the duty cycle");
        assert_eq!(wave(SAMPLE_AND_HOLD, 0.9, 0.5, 0.3, -0.7), 0.3);
        assert_eq!(wave(NOISE, 0.0, 0.5, 0.3, -0.7), 0.3);
        assert!((wave(NOISE, 0.5, 0.5, 0.3, -0.7) + 0.2).abs() < 1e-6, "ramps between");
    }

    // Spec: analog R18
    #[test]
    fn a_delay_holds_the_lfo_at_zero_and_the_attack_fades_it_in() {
        assert_eq!(fade(0.1, 0.5, 1.0), 0.0);
        assert_eq!(fade(0.5, 0.5, 1.0), 0.0);
        assert!((fade(1.0, 0.5, 1.0) - 0.5).abs() < 1e-6);
        assert_eq!(fade(2.0, 0.5, 1.0), 1.0);
        assert_eq!(fade(0.6, 0.5, 0.0), 1.0, "no attack: full at once");
    }

    // Spec: analog R18, R29
    #[test]
    fn the_random_waves_hold_a_value_a_cycle_and_a_seed_repeats_them() {
        let run = |seed| {
            let mut lfo = Lfo::new();
            lfo.start(seed, 0.0);
            (0..40)
                .map(|_| lfo.step(SAMPLE_AND_HOLD, 0.5, 0.125))
                .collect::<Vec<_>>()
        };
        let a = run(7);
        assert_eq!(a, run(7), "the same seed, the same values");
        assert_ne!(a, run(8));
        assert!(a[..8].iter().all(|&x| x == a[0]), "held for a cycle");
        assert_ne!(a[0], a[8], "a new one each cycle");
    }

    // Spec: analog R18
    #[test]
    fn an_lfo_starts_at_its_offset_and_runs_at_its_rate() {
        let mut lfo = Lfo::new();
        lfo.start(1, 0.25);
        assert!(lfo.step(SINE, 0.5, 0.01) > 0.999, "a quarter cycle in");
        for _ in 0..99 {
            lfo.step(SINE, 0.5, 0.01);
        }
        assert!((lfo.phase() - 0.25).abs() < 1e-9, "a cycle in a hundred frames");
    }

    // Spec: analog R19, R29
    #[test]
    fn error_varies_each_notes_vibrato_from_the_seed() {
        let draw = |seed| {
            let mut random = Random::seeded(seed);
            let mut vibrato = Vibrato::new();
            vibrato.start(1.0, &mut random);
            (vibrato.rate, vibrato.amount)
        };
        assert_eq!(draw(3), draw(3));
        assert_ne!(draw(3), draw(4));
        let (rate, amount) = draw(3);
        assert!((0.5..=1.5).contains(&rate) && (0.5..=1.5).contains(&amount));
        let mut random = Random::seeded(3);
        let mut steady = Vibrato::new();
        steady.start(0.0, &mut random);
        assert_eq!((steady.rate, steady.amount), (1.0, 1.0), "no error, no variation");
    }
}
