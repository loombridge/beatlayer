//! The generator behind Beatlayer's factory wavetables (the wavetable spec,
//! R1, R2): every frame is made from a formula, an additive spectrum or a
//! seeded random sequence.
//!
//! Every transcendental function comes from `libm`, and a sine of a whole
//! harmonic is read from one table of `libm` sines, so a frame is the same
//! on every platform. The rest is IEEE addition, multiplication, division
//! and square roots, which are exact on all of them.

use std::sync::OnceLock;

use lspk_wavetable::FRAME_SIZE;

/// One frame of samples.
pub(super) type Frame = [f32; FRAME_SIZE];

const N: usize = FRAME_SIZE;
const TAU: f64 = std::f64::consts::TAU;

/// The most any frame's peak reaches.
const PEAK: f64 = 0.9;

/// How far a frame is through its table, 0 at the first frame and 1 at the
/// last (0 for a table of one frame).
fn along(frame: usize, count: usize) -> f64 {
    if count < 2 {
        0.0
    } else {
        frame as f64 / (count - 1) as f64
    }
}

/// `sin(2π i / N)` for every `i`: the one place a sine is computed.
fn sines() -> &'static [f64; N] {
    static SINES: OnceLock<[f64; N]> = OnceLock::new();
    SINES.get_or_init(|| std::array::from_fn(|i| libm::sin(TAU * i as f64 / N as f64)))
}

/// Sample `n` of harmonic `k`, starting `phase` samples in.
fn harmonic(k: usize, n: usize, phase: usize) -> f64 {
    sines()[(k * n + phase) % N]
}

/// Takes the DC offset out of `frame`, scales it so its peak is [`PEAK`],
/// and rounds it to samples. A silent frame stays silent.
fn finish(frame: &[f64; N]) -> Frame {
    let mean = frame.iter().sum::<f64>() / N as f64;
    let peak = frame
        .iter()
        .fold(0.0f64, |peak, x| peak.max((x - mean).abs()));
    let scale = if peak > 1e-12 { PEAK / peak } else { 0.0 };
    std::array::from_fn(|n| ((frame[n] - mean) * scale) as f32)
}

/// A frame holding harmonics `1..=count`, harmonic `k` at `amplitude(k)`.
fn additive(count: usize, amplitude: impl Fn(usize) -> f64) -> Frame {
    let mut frame = [0.0; N];
    for k in 1..=count {
        let a = amplitude(k);
        if a != 0.0 {
            for (n, x) in frame.iter_mut().enumerate() {
                *x += a * harmonic(k, n, 0);
            }
        }
    }
    finish(&frame)
}

/// A frame whose sample `n` is `f(n / N)`.
fn drawn(f: impl Fn(f64) -> f64) -> Frame {
    let mut frame = [0.0; N];
    for (n, x) in frame.iter_mut().enumerate() {
        *x = f(n as f64 / N as f64);
    }
    finish(&frame)
}

/// The fractional part of `x`, for `x` at least 0.
fn fract(x: f64) -> f64 {
    x - libm::floor(x)
}

/// A repeatable stream of random numbers: xorshift64*, from a fixed seed.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number from −1 to 1.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }

    /// A whole number below `limit`.
    fn below(&mut self, limit: usize) -> usize {
        (self.next() >> 16) as usize % limit
    }
}

// Shape Morphs: one basic shape turning into another.

pub(super) fn sine_to_saw(frame: usize, count: usize) -> Frame {
    let t = along(frame, count);
    additive(64, |k| if k == 1 { 1.0 } else { t / k as f64 })
}

pub(super) fn saw_to_square(frame: usize, count: usize) -> Frame {
    let t = along(frame, count);
    additive(64, |k| {
        let even = if k % 2 == 0 { 1.0 - t } else { 1.0 };
        even / k as f64
    })
}

pub(super) fn square_to_triangle(frame: usize, count: usize) -> Frame {
    let t = along(frame, count);
    additive(63, |k| {
        if k % 2 == 0 {
            return 0.0;
        }
        let sign = if (k / 2) % 2 == 0 { 1.0 } else { -1.0 };
        (1.0 - t) / k as f64 + t * sign / (k * k) as f64
    })
}

pub(super) fn sine_folds(frame: usize, count: usize) -> Frame {
    let gain = 1.0 + 7.0 * along(frame, count);
    drawn(|x| libm::sin(std::f64::consts::FRAC_PI_2 * gain * libm::sin(TAU * x)))
}

// Additive: harmonics added, moved and weighted frame by frame.

pub(super) fn odd_rise(frame: usize, _: usize) -> Frame {
    let top = 1 + 2 * frame;
    additive(top, |k| if k % 2 == 1 { 1.0 / k as f64 } else { 0.0 })
}

pub(super) fn harmonic_ladder(frame: usize, _: usize) -> Frame {
    additive(frame + 1, |k| match k {
        1 => 1.0,
        k if k == frame + 1 => 0.8,
        _ => 0.0,
    })
}

pub(super) fn falling_bells(frame: usize, count: usize) -> Frame {
    let slope = 0.4 + 2.6 * along(frame, count);
    additive(96, |k| libm::pow(k as f64, -slope))
}

pub(super) fn octave_stack(frame: usize, count: usize) -> Frame {
    let place = 5.0 * along(frame, count);
    additive(32, |k| {
        if !k.is_power_of_two() {
            return 0.0;
        }
        let octave = f64::from(k.trailing_zeros());
        (1.0 - (place - octave).abs()).max(0.0)
    })
}

// Pulse and Sync: pulse widths and hard-synced oscillators.

pub(super) fn pulse_width_sweep(frame: usize, count: usize) -> Frame {
    let width = 0.5 - 0.46 * along(frame, count);
    drawn(|x| if x < width { 1.0 - width } else { -width })
}

pub(super) fn sync_saw_sweep(frame: usize, count: usize) -> Frame {
    let ratio = 1.0 + 7.0 * along(frame, count);
    drawn(|x| 2.0 * fract(ratio * x) - 1.0)
}

pub(super) fn sync_sine_sweep(frame: usize, count: usize) -> Frame {
    let ratio = 1.0 + 7.0 * along(frame, count);
    drawn(|x| libm::sin(TAU * fract(ratio * x)))
}

pub(super) fn twin_pulses(frame: usize, count: usize) -> Frame {
    let gap = 0.15 + 0.35 * along(frame, count);
    drawn(|x| {
        let in_pulse = |start: f64| x >= start && x < start + 0.1;
        if in_pulse(0.0) || in_pulse(gap) {
            0.8
        } else {
            -0.2
        }
    })
}

// FM: a sine phase-modulated by another.

pub(super) fn index_sweep(frame: usize, count: usize) -> Frame {
    let index = 8.0 * along(frame, count);
    drawn(|x| libm::sin(TAU * x + index * libm::sin(TAU * x)))
}

pub(super) fn ratio_steps(frame: usize, _: usize) -> Frame {
    let ratio = (frame + 1) as f64;
    drawn(|x| libm::sin(TAU * x + 3.0 * libm::sin(TAU * ratio * x)))
}

pub(super) fn feedback_bloom(frame: usize, count: usize) -> Frame {
    let feedback = 2.5 * along(frame, count);
    drawn(|x| {
        let phase = TAU * x;
        let mut y = libm::sin(phase);
        for _ in 0..4 {
            y = libm::sin(phase + feedback * y);
        }
        y
    })
}

// Voices: vowel-like formant spectra. The formant frequencies are the
// averages Peterson and Barney measured for men (1952).

/// A vowel's first three formants, in hertz.
type Formants = [f64; 3];

const I: Formants = [270.0, 2290.0, 3010.0];
const SHORT_I: Formants = [390.0, 1990.0, 2550.0];
const E: Formants = [530.0, 1840.0, 2480.0];
const AE: Formants = [660.0, 1720.0, 2410.0];
const A: Formants = [730.0, 1090.0, 2440.0];
const OPEN_O: Formants = [570.0, 840.0, 2410.0];
const SHORT_U: Formants = [440.0, 1020.0, 2240.0];
const U: Formants = [300.0, 870.0, 2240.0];
const TURNED_V: Formants = [640.0, 1190.0, 2390.0];
const R_COLOURED: Formants = [490.0, 1350.0, 1690.0];

/// The pitch the vowels are sung at, in hertz.
const SUNG: f64 = 110.0;

/// A frame of the harmonics of [`SUNG`] shaped by `formants`, falling at
/// 6 dB per octave as a voice does.
fn vowel(formants: Formants) -> Frame {
    additive(48, |k| {
        let hz = SUNG * k as f64;
        let resonance: f64 = formants
            .iter()
            .map(|&f| {
                let off = (hz - f) / (60.0 + 0.06 * f);
                1.0 / (1.0 + off * off)
            })
            .sum();
        resonance / k as f64
    })
}

const VOWELS: [Formants; 10] = [
    I, SHORT_I, E, AE, TURNED_V, A, OPEN_O, SHORT_U, U, R_COLOURED,
];

pub(super) fn vowel_walk(frame: usize, _: usize) -> Frame {
    vowel(VOWELS[frame])
}

pub(super) fn front_vowels(frame: usize, _: usize) -> Frame {
    vowel([I, SHORT_I, E, AE][frame])
}

pub(super) fn back_vowels(frame: usize, _: usize) -> Frame {
    vowel([A, OPEN_O, SHORT_U, U][frame])
}

pub(super) fn formant_glide(frame: usize, count: usize) -> Frame {
    vowel([300.0 + 600.0 * along(frame, count), 1200.0, 2500.0])
}

// Bit and Step: quantised and held waves.

pub(super) fn bit_crush_sweep(frame: usize, count: usize) -> Frame {
    let levels = libm::pow(2.0, 7.0 - 6.0 * along(frame, count));
    drawn(|x| libm::floor((2.0 * x - 1.0) * levels + 0.5) / levels)
}

pub(super) fn stair_rise(frame: usize, _: usize) -> Frame {
    let steps = (frame + 2) as f64;
    drawn(|x| libm::floor(x * steps) / (steps - 1.0) * 2.0 - 1.0)
}

pub(super) fn hold_squeeze(frame: usize, count: usize) -> Frame {
    let holds = 2.0 + 62.0 * along(frame, count);
    let length = (N as f64 / holds) as usize;
    let mut held = [0.0; N];
    for (n, x) in held.iter_mut().enumerate() {
        let start = n - n % length;
        *x = harmonic(1, start, 0);
    }
    finish(&held)
}

// Grain and Noise: seeded random spectra and sequences.

pub(super) fn noise_smear(frame: usize, _: usize) -> Frame {
    // Each drawn frame has its own stream, so frames don't share harmonics.
    let mut random = Random(0x9E37_79B9_7F4A_7C15 ^ (frame as u64 + 1));
    let mut frame = [0.0; N];
    for k in 1..=96 {
        let amplitude = (random.unit() + 1.0) / libm::pow(k as f64, 0.7);
        let phase = random.below(N);
        for (n, x) in frame.iter_mut().enumerate() {
            *x += amplitude * harmonic(k, n, phase);
        }
    }
    finish(&frame)
}

pub(super) fn rough_sine(frame: usize, count: usize) -> Frame {
    let roughness = 0.5 * along(frame, count);
    let mut random = Random(0x1234_5678_9ABC_DEF1);
    let noise: Vec<f64> = (0..N).map(|_| random.unit()).collect();
    let mut rough = [0.0; N];
    for (n, x) in rough.iter_mut().enumerate() {
        // Three neighbouring samples, so the noise isn't all treble.
        let smooth = (noise[(n + N - 1) % N] + noise[n] + noise[(n + 1) % N]) / 3.0;
        *x = harmonic(1, n, 0) + roughness * smooth;
    }
    finish(&rough)
}

pub(super) fn scatter_pulses(frame: usize, _: usize) -> Frame {
    // One list of places for every frame, so each frame adds a pulse.
    let mut random = Random(0x0F1E_2D3C_4B5A_6978);
    let mut scattered = [0.0; N];
    for _ in 0..=frame {
        let at = random.below(N);
        let height = random.unit();
        for width in 0..16 {
            scattered[(at + width) % N] += height * (1.0 - width as f64 / 16.0);
        }
    }
    finish(&scattered)
}

pub(super) fn grit_drift(frame: usize, count: usize) -> Frame {
    let t = along(frame, count);
    let mut random = Random(0xDEAD_BEEF_CAFE_F00D);
    let mut level = 0.0;
    let mut walk = [0.0; N];
    for x in &mut walk {
        level = (level + 0.12 * random.unit()).clamp(-1.0, 1.0);
        *x = level;
    }
    let mean = walk.iter().sum::<f64>() / N as f64;
    let mut mixed = [0.0; N];
    for (n, x) in mixed.iter_mut().enumerate() {
        *x = (1.0 - t) * harmonic(1, n, 0) + t * (walk[n] - mean);
    }
    finish(&mixed)
}
