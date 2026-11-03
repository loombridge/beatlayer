//! Meld's modulation sources (WP-8.20; spec: meld R59 to R61): LFO 1's six
//! types, LFO 2's six waves and LFO 1 FX's eighteen effects. Pure and
//! allocation-free: the device runs one [`Lfo`](crate::wavetable_mod::Lfo)
//! and two [`Fx`] per voice and engine, once on every step of the control
//! grid, and reads the waves here.

use crate::wavetable_mod::unit;

/// LFO 1's types (R60), in the order the chooser lists them.
pub const LFO1_TYPES: [&str; 6] = [
    "Basic Shapes",
    "Ramp",
    "Wander",
    "Alternate",
    "Euclid",
    "Pulsate",
];

/// Each LFO 1 type's two macros' names (R60).
pub const LFO1_MACROS: [(&str, &str); 6] = [
    ("Shape", "Fold"),
    ("Shape", "Slope"),
    ("Morph", "Step"),
    ("L1", "L2"),
    ("Steps", "Pulses"),
    ("Chance", "Length"),
];

/// LFO 2's waves (R60).
pub const LFO2_TYPES: [&str; 6] = [
    "Sine",
    "Tri",
    "Saw Up",
    "Saw Down",
    "Rectangle",
    "Random S&H",
];

/// LFO 1 FX's effects (R61): Off, then the seventeen, Scrambler from the
/// release notes and the others Beatlayer's own names.
pub const FX_NAMES: [&str; 18] = [
    "Off",
    "Scrambler",
    "Quantize",
    "Smooth",
    "Slew Up",
    "Slew Down",
    "Rectify",
    "Invert",
    "Fold",
    "Wrap",
    "Bias",
    "Gain",
    "Sample & Hold",
    "Stutter",
    "Reverse",
    "Delay",
    "Swing",
    "Chaos",
];

/// What each effect's one macro is called.
pub const FX_MACROS: [&str; 18] = [
    "Amount", "Order", "Steps", "Time", "Time", "Time", "Mix", "Mix", "Drive", "Drive", "Offset",
    "Gain", "Time", "Length", "Length", "Time", "Swing", "Chaos",
];

/// The effects by place in [`FX_NAMES`] that the voice code branches on.
const SCRAMBLER: usize = 1;
const QUANTIZE: usize = 2;
const SMOOTH: usize = 3;
const SLEW_UP: usize = 4;
const SLEW_DOWN: usize = 5;
const RECTIFY: usize = 6;
const INVERT: usize = 7;
const FOLD: usize = 8;
const WRAP: usize = 9;
const BIAS: usize = 10;
const GAIN: usize = 11;
const SAMPLE_HOLD: usize = 12;
const STUTTER: usize = 13;
const REVERSE: usize = 14;
const DELAY: usize = 15;
const SWING: usize = 16;
const CHAOS: usize = 17;

/// How many steps of the control grid an effect remembers.
pub const FX_RING: usize = 256;
/// The longest Reverse block, in steps.
const REVERSE_MAX: usize = 16;

/// A wave from −1 to 1 at `phase` cycles: sine, triangle, saw and square,
/// each starting where its name says.
fn shape(kind: usize, phase: f32) -> f32 {
    match kind {
        0 => (core::f32::consts::TAU * phase).sin(),
        1 => {
            if phase < 0.25 {
                4.0 * phase
            } else if phase < 0.75 {
                2.0 - 4.0 * phase
            } else {
                4.0 * phase - 4.0
            }
        }
        2 => 2.0 * (phase + 0.5).fract() - 1.0,
        _ => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

/// A triangle fold of `y` into −1 to 1: the identity inside it, mirrored at
/// its ends.
fn fold(y: f32) -> f32 {
    let t = (y + 1.0) * 0.25;
    4.0 * (t - t.round()).abs() - 1.0
}

/// Smoothstep of `t` from 0 to 1.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The value of the random draw `index` of an LFO with `salt`, from −1 to 1.
fn draw(salt: u64, index: u64) -> f32 {
    unit(salt ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// LFO 1 of type `kind` at `phase` cycles into cycle `cycles`, with macros
/// `m` (each 0 to 1), from −1 to 1 (R60). Wander and Pulsate draw their
/// values from `salt`, the device's seed.
///
/// - Basic Shapes: Shape morphs sine, triangle, saw and square; Fold
///   triangle-folds the wave up to four times its height.
/// - Ramp: Shape sets how much of the cycle rises (0 falls, one half is a
///   triangle, 1 rises); Slope bends each side from straight to a cube.
/// - Wander: draws 1 to 16 random values a cycle (Step); Morph glides to the
///   next over the last share of the step it names, 0 stepping straight.
/// - Alternate: L1 is the first half's level, falling from 1 as it rises,
///   and L2 the second's, rising from −1.
/// - Euclid: Steps (1 to 16) steps a cycle, Pulses of them (1 to Steps)
///   spread evenly, each a high first half-step and low second.
/// - Pulsate: Chance is the share of cycles that stay silent; Length is how
///   much of the cycle a played pulse takes to fall from 1 to −1.
#[must_use]
pub fn lfo1(kind: usize, phase: f32, cycles: u64, salt: u64, m: [f32; 2]) -> f32 {
    let [m1, m2] = m.map(|x| x.clamp(0.0, 1.0));
    match kind {
        0 => {
            let at = m1 * 3.0;
            let low = (at as usize).min(2);
            let t = at - low as f32;
            let x = shape(low, phase) * (1.0 - t) + shape(low + 1, phase) * t;
            if m2 > 0.0 {
                fold(x * (1.0 + 3.0 * m2))
            } else {
                x
            }
        }
        1 => {
            let rise = m1.clamp(0.001, 0.999);
            let bend = 1.0 + 3.0 * m2;
            if phase < rise {
                -1.0 + 2.0 * (phase / rise).powf(bend)
            } else {
                1.0 - 2.0 * ((phase - rise) / (1.0 - rise)).powf(bend)
            }
        }
        2 => {
            let n = 1 + (m2 * 15.0).round() as u64;
            let at = phase * n as f32;
            let step = (at as u64).min(n - 1);
            let index = cycles.wrapping_mul(n).wrapping_add(step);
            let (from, to) = (draw(salt, index), draw(salt, index.wrapping_add(1)));
            let t = if m1 <= 0.0 {
                0.0
            } else {
                smooth((at - step as f32 - (1.0 - m1)) / m1)
            };
            from + (to - from) * t
        }
        3 => {
            if phase < 0.5 {
                1.0 - 2.0 * m1
            } else {
                -1.0 + 2.0 * m2
            }
        }
        4 => {
            let steps = 1 + (m1 * 15.0).round() as usize;
            let pulses = 1 + (m2 * (steps - 1) as f32).round() as usize;
            let at = phase * steps as f32;
            let step = (at as usize).min(steps - 1);
            let hit = (step * pulses) % steps < pulses;
            if hit && at - (step as f32) < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        _ => {
            let skipped = (draw(salt, cycles) + 1.0) * 0.5 < m1;
            if skipped {
                -1.0
            } else {
                let length = 0.05 + 0.95 * m2;
                1.0 - 2.0 * (phase / length).min(1.0)
            }
        }
    }
}

/// LFO 2's wave `kind` at `phase` cycles into cycle `cycles` (R60), from −1
/// to 1; Random S&H holds one draw of `salt` a cycle.
#[must_use]
pub fn lfo2(kind: usize, phase: f32, cycles: u64, salt: u64) -> f32 {
    match kind {
        0 => shape(0, phase),
        1 => shape(1, phase),
        2 => 2.0 * phase - 1.0,
        3 => 1.0 - 2.0 * phase,
        4 => shape(3, phase),
        _ => draw(salt, cycles),
    }
}

/// One effect slot of LFO 1 FX for one voice: what it remembers of its
/// input, a step of the control grid at a time.
#[derive(Clone, Copy)]
pub struct Fx {
    y: f32,
    held: f32,
    count: u32,
    pos: usize,
    ring: [f32; FX_RING],
    block: [f32; REVERSE_MAX],
    last: [f32; REVERSE_MAX],
}

impl Fx {
    pub const IDLE: Self = Self {
        y: 0.0,
        held: 0.0,
        count: 0,
        pos: 0,
        ring: [0.0; FX_RING],
        block: [0.0; REVERSE_MAX],
        last: [0.0; REVERSE_MAX],
    };

    /// A note starts the effect over; Chaos starts from a point `salt` draws.
    pub fn start(&mut self, salt: u64) {
        *self = Self::IDLE;
        self.y = (unit(salt) + 1.0) * 0.5 * 0.9 + 0.05;
    }

    /// The effect `kind` of macro `a` (0 to 1) on the next step's `x`
    /// (−1 to 1) (R61), from −1 to 1.
    pub fn step(&mut self, kind: usize, a: f32, x: f32) -> f32 {
        let a = a.clamp(0.0, 1.0);
        self.ring[self.pos] = x;
        self.pos = (self.pos + 1) % FX_RING;
        self.count = self.count.wrapping_add(1);
        let lag = (1.0 - a) * (1.0 - a);
        let out = match kind {
            SCRAMBLER => scramble(x, a),
            QUANTIZE => {
                let half = 1.0 + 31.0 * lag;
                (x * half).round() / half
            }
            SMOOTH => {
                self.y += (x - self.y) * lag.max(0.01);
                self.y
            }
            SLEW_UP => {
                self.y = if x > self.y {
                    self.y + (x - self.y) * lag.max(0.01)
                } else {
                    x
                };
                self.y
            }
            SLEW_DOWN => {
                self.y = if x < self.y {
                    self.y + (x - self.y) * lag.max(0.01)
                } else {
                    x
                };
                self.y
            }
            RECTIFY => x * (1.0 - a) + x.abs() * a,
            INVERT => x * (1.0 - 2.0 * a),
            FOLD => fold(x * (1.0 + 3.0 * a)),
            WRAP => (x * (1.0 + 3.0 * a) + 1.0).rem_euclid(2.0) - 1.0,
            BIAS => (x + a).clamp(-1.0, 1.0),
            GAIN => (x * (1.0 + 3.0 * a)).clamp(-1.0, 1.0),
            SAMPLE_HOLD => {
                let every = 1 + (a * 31.0).round() as u32;
                if self.count.wrapping_sub(1).is_multiple_of(every) {
                    self.held = x;
                }
                self.held
            }
            STUTTER => {
                let len = 1 + (a * 15.0).round() as u32;
                if self.count.wrapping_sub(1) % (2 * len) < len {
                    x
                } else {
                    self.ring[(self.pos + FX_RING - 1 - len as usize) % FX_RING]
                }
            }
            REVERSE => {
                let len = 2 + (a * 14.0).round() as usize;
                let at = self.count.wrapping_sub(1) as usize % len;
                self.block[at] = x;
                let out = self.last[len - 1 - at];
                if at == len - 1 {
                    self.last = self.block;
                }
                out
            }
            DELAY => {
                let back = (a * (FX_RING - 1) as f32).round() as usize;
                self.ring[(self.pos + FX_RING - 1 - back) % FX_RING]
            }
            SWING => {
                if self.count.is_multiple_of(2) {
                    x * (1.0 - a) + self.ring[(self.pos + FX_RING - 2) % FX_RING] * a
                } else {
                    x
                }
            }
            CHAOS => {
                let r = 3.57 + 0.43 * a;
                self.y = (r * self.y * (1.0 - self.y)).clamp(0.001, 0.999);
                x * (1.0 - a) + (2.0 * self.y - 1.0) * a
            }
            _ => x,
        };
        out.clamp(-1.0, 1.0)
    }
}

/// Scrambler: the input's amplitude range cut into four, each quarter played
/// where another stood, by the permutation macro `a` picks from six.
fn scramble(x: f32, a: f32) -> f32 {
    const ORDERS: [[usize; 4]; 6] = [
        [0, 1, 2, 3],
        [2, 3, 0, 1],
        [1, 0, 3, 2],
        [3, 2, 1, 0],
        [1, 2, 3, 0],
        [0, 2, 1, 3],
    ];
    let order = ORDERS[((a * 5.0).round() as usize).min(5)];
    let at = (x.clamp(-1.0, 1.0) + 1.0) * 2.0;
    let band = (at as usize).min(3);
    let within = at - band as f32;
    (order[band] as f32 + within) * 0.5 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LFO 1 over one cycle in `n` points.
    fn cycle(kind: usize, m: [f32; 2]) -> Vec<f32> {
        (0..400)
            .map(|i| lfo1(kind, i as f32 / 400.0, 0, 9, m))
            .collect()
    }

    fn effect(kind: &str, amount: f32, input: &[f32]) -> Vec<f32> {
        let kind = FX_NAMES.iter().position(|n| *n == kind).unwrap();
        let mut fx = Fx::IDLE;
        fx.start(5);
        input.iter().map(|&x| fx.step(kind, amount, x)).collect()
    }

    // Spec: meld R60
    #[test]
    fn every_wave_stays_inside_its_range() {
        for kind in 0..6 {
            for m in [[0.0, 0.0], [1.0, 1.0], [0.3, 0.8]] {
                for i in 0..400 {
                    let v = lfo1(kind, (i % 100) as f32 / 100.0, i / 100, 7, m);
                    assert!((-1.0..=1.0).contains(&v), "type {kind} at {i}: {v}");
                    let w = lfo2(kind, (i % 100) as f32 / 100.0, i / 100, 7);
                    assert!((-1.0..=1.0).contains(&w));
                }
            }
        }
    }

    // Spec: meld R60
    #[test]
    fn basic_shapes_morph_sine_triangle_saw_and_square_and_fold_folds() {
        let at = |m1: f32, phase: f32| lfo1(0, phase, 0, 0, [m1, 0.0]);
        assert!(
            (at(0.0, 0.25) - 1.0).abs() < 1e-6,
            "a sine peaks at a quarter"
        );
        assert!((at(1.0 / 3.0, 0.25) - 1.0).abs() < 1e-5, "a triangle too");
        assert!((at(1.0 / 3.0, 0.125) - 0.5).abs() < 1e-5);
        assert!(
            at(2.0 / 3.0, 0.0).abs() < 1e-5 && (at(2.0 / 3.0, 0.25) - 0.5).abs() < 1e-5,
            "a saw"
        );
        assert!(
            (at(1.0, 0.25) - 1.0).abs() < 1e-6 && at(1.0, 0.75) == -1.0,
            "a square"
        );
        // Fold bends the peak back down: the sine's top reaches past 1 and
        // folds under it.
        let folded = lfo1(0, 0.25, 0, 0, [0.0, 0.5]);
        assert!(folded < 1.0 - 1e-3, "{folded}");
    }

    // Spec: meld R60
    #[test]
    fn ramp_rises_for_shape_of_the_cycle_and_slope_bends_each_side() {
        let rising = cycle(1, [1.0, 0.0]);
        assert!(
            rising.windows(2).take(390).all(|w| w[1] >= w[0] - 1e-6),
            "a rising saw"
        );
        let falling = cycle(1, [0.0, 0.0]);
        assert!(
            falling.windows(2).skip(10).all(|w| w[1] <= w[0] + 1e-6),
            "a falling saw"
        );
        let triangle = cycle(1, [0.5, 0.0]);
        assert!((triangle[200] - 1.0).abs() < 0.02 && (triangle[100] - 0.0).abs() < 0.02);
        // Slope at 100 % cubes... to the fourth power: the middle of a rise
        // sits lower than a straight line's.
        let bent = cycle(1, [1.0, 1.0]);
        assert!(bent[200] < -0.5, "{}", bent[200]);
    }

    // Spec: meld R60
    #[test]
    fn alternate_switches_between_two_levels_and_euclid_spreads_its_pulses() {
        let alternate = cycle(3, [0.25, 0.5]);
        assert!((alternate[10] - 0.5).abs() < 1e-6 && alternate[300].abs() < 1e-6);
        // Steps 9 of 16 (m1 = 8/15) with 3 pulses (m2 = 2/8): three high
        // stretches over the cycle.
        let euclid = cycle(4, [7.0 / 15.0, 2.0 / 7.0]);
        let highs = euclid
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] > 0.0)
            .count()
            + usize::from(euclid[0] > 0.0);
        assert_eq!(highs, 3);
        assert!(euclid.iter().all(|&x| x == 1.0 || x == -1.0));
    }

    // Spec: meld R60
    #[test]
    fn wander_steps_or_glides_and_pulsate_skips_by_chance() {
        // One draw a cycle: stepped, a value holds the whole cycle.
        let stepped: Vec<f32> = (0..100)
            .map(|i| lfo1(2, i as f32 / 100.0, 3, 1, [0.0, 0.0]))
            .collect();
        assert!(stepped.iter().all(|&x| x == stepped[0]));
        let next = lfo1(2, 0.0, 4, 1, [0.0, 0.0]);
        assert!(
            (next - stepped[0]).abs() > 1e-3,
            "the next cycle draws again"
        );
        let gliding = lfo1(2, 0.99, 3, 1, [1.0, 0.0]);
        assert!(
            (gliding - next).abs() < 0.05,
            "Morph reaches the next value by the step's end"
        );
        // Pulsate: Chance 100 % never plays, 0 % always starts at 1 and
        // falls to -1 by its Length.
        assert!((0..50).all(|c| lfo1(5, 0.0, c, 3, [1.0, 0.5]) == -1.0));
        assert_eq!(lfo1(5, 0.0, 0, 3, [0.0, 0.0]), 1.0);
        assert_eq!(lfo1(5, 0.06, 0, 3, [0.0, 0.0]), -1.0);
    }

    // Spec: meld R60
    #[test]
    fn lfo_2_plays_its_six_waves() {
        assert_eq!(lfo2(0, 0.25, 0, 0), 1.0);
        assert!((lfo2(1, 0.25, 0, 0) - 1.0).abs() < 1e-6);
        assert!((lfo2(2, 0.75, 0, 0) - 0.5).abs() < 1e-6);
        assert!((lfo2(3, 0.75, 0, 0) + 0.5).abs() < 1e-6);
        assert_eq!((lfo2(4, 0.2, 0, 0), lfo2(4, 0.7, 0, 0)), (1.0, -1.0));
        assert_eq!(lfo2(5, 0.1, 4, 8), lfo2(5, 0.9, 4, 8), "one draw a cycle");
        assert_ne!(lfo2(5, 0.1, 4, 8), lfo2(5, 0.1, 5, 8));
    }

    // Spec: meld R61
    #[test]
    fn the_effects_do_what_their_names_say() {
        let ramp: Vec<f32> = (0..16).map(|i| i as f32 / 8.0 - 1.0).collect();
        // Off passes it; Invert at 100 % flips it, at 50 % flattens it.
        assert_eq!(effect("Off", 0.5, &ramp), ramp);
        assert!(
            effect("Invert", 1.0, &ramp)
                .iter()
                .zip(&ramp)
                .all(|(y, x)| (y + x).abs() < 1e-6)
        );
        assert!(effect("Invert", 0.5, &ramp).iter().all(|y| y.abs() < 1e-6));
        // Rectify at 100 % is the absolute value; Bias moves it up; Gain
        // scales and clips.
        assert!(effect("Rectify", 1.0, &ramp).iter().all(|&y| y >= 0.0));
        assert!((effect("Bias", 0.25, &[0.0])[0] - 0.25).abs() < 1e-6);
        assert_eq!(effect("Gain", 1.0, &[0.5])[0], 1.0);
        // Quantize at 100 % snaps to two levels; Fold and Wrap keep it
        // inside the range.
        let quantized = effect("Quantize", 1.0, &ramp);
        assert!(
            quantized
                .iter()
                .all(|y| (y.abs() - 1.0).abs() < 1e-6 || *y == 0.0)
        );
        assert!(effect("Fold", 1.0, &ramp).iter().all(|y| y.abs() <= 1.0));
        assert!(effect("Wrap", 1.0, &ramp).iter().all(|y| y.abs() <= 1.0));
        // Smooth lags a step; Slew Up follows a fall at once and a rise
        // slowly, Slew Down the reverse.
        let step = [vec![0.0; 4], vec![1.0; 8]].concat();
        let smooth = effect("Smooth", 0.5, &step);
        assert!(smooth[4] < 1.0 && smooth[11] > smooth[4]);
        let up = effect("Slew Up", 0.5, &step);
        assert!(up[4] < 1.0);
        let down = effect("Slew Down", 0.5, &step);
        assert_eq!(down[4], 1.0);
        // Sample & Hold takes every fourth value at 10 % of its range;
        // Delay shifts by a number of steps; Reverse plays a block backwards.
        let held = effect("Sample & Hold", 0.1, &ramp);
        assert_eq!(held[0], held[1]);
        assert_eq!(effect("Delay", 0.0, &ramp), ramp, "no delay at 0");
        let delayed = effect("Delay", 5.0 / 255.0, &ramp);
        assert_eq!(delayed[8], ramp[3]);
        let reversed = effect(
            "Reverse",
            0.0,
            &(0..8).map(|i| i as f32 / 8.0).collect::<Vec<_>>(),
        );
        assert_eq!(
            &reversed[2..4],
            &[1.0 / 8.0, 0.0],
            "block 2 plays block 1 backwards"
        );
        // Stutter repeats; Chaos and Swing change the signal; the
        // Scrambler moves a quarter.
        let stutter = effect("Stutter", 0.0, &ramp);
        assert_eq!(stutter[1], ramp[0], "a length of 1 repeats the last step");
        assert_eq!(stutter[3], ramp[2]);
        assert_ne!(effect("Chaos", 0.8, &[0.0; 8]), vec![0.0; 8]);
        assert_ne!(effect("Swing", 1.0, &ramp), ramp);
        assert_ne!(effect("Scrambler", 0.2, &ramp), ramp);
        // The same start gives the same effect.
        assert_eq!(effect("Chaos", 0.8, &ramp), effect("Chaos", 0.8, &ramp));
    }

    #[test]
    fn the_fold_keeps_its_inside_and_mirrors_past_the_ends() {
        assert!((fold(0.5) - 0.5).abs() < 1e-6);
        assert!((fold(1.5) - 0.5).abs() < 1e-6);
        assert!((fold(-1.5) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_scrambler_keeps_a_quarter_and_moves_it() {
        // Order 0 is the identity; order 1 swaps the lower and upper halves.
        assert!((scramble(0.3, 0.0) - 0.3).abs() < 1e-6);
        assert!((scramble(-0.9, 0.2) - 0.1).abs() < 1e-5);
    }
}
