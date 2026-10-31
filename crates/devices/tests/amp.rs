//! Amp, the seven amplifier models (WP-8.40; the amp spec): the triode curve,
//! each model's tone stack, harmonic balance and DC, Presence, Volume, Mono
//! and Dual, Dry/Wet and the latency, aliasing, renders that don't depend on
//! the block size and a model change without a click. Every block runs as a
//! real-time section under the allocation trap.
//!
//! The expected values come from a second, plain f64 simulation of the spec's
//! chain at 4 times the rate (`Reference`), whose tone stacks are written
//! out again here from the spec's component values.

use std::f64::consts::TAU;

use crate::common::{RATE, device, node, noise, run, run_blocks, set};
use bl_core::time::{Bpm, TempoMap};
use bl_devices::amp::{
    AMP, BASS, DRY_WET, GAIN, MIDDLE, MODEL, OUTPUT, PRESENCE, TREBLE, VOLUME, amp_type, triode,
};
use bl_engine::{Node, Registry};
use bl_model::ParamId;

const HI: f64 = 4.0 * 48_000.0;
/// The oversampler's latency, in frames at the sample rate.
const LATENCY: usize = 48;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::amp::register(&mut registry);
    registry
}

/// An Amp of `params` taking up to `max_frames` at a time. Presence 4 is
/// 0 dB, so a test that isn't about Presence sets it.
fn amp(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let mut all = vec![(PRESENCE, 4.0)];
    all.extend_from_slice(params);
    node(&registry(), &device(&amp_type(), &all), max_frames)
}

fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

/// The magnitude of `samples` at `bin` of a DFT of its length.
fn magnitude(samples: &[f32], bin: usize) -> f64 {
    let n = samples.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in samples.iter().enumerate() {
        let phase = TAU * bin as f64 * i as f64 / n;
        re += f64::from(*s) * phase.cos();
        im -= f64::from(*s) * phase.sin();
    }
    (re * re + im * im).sqrt() * 2.0 / n
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

/// The models' spec rows: stages, k at Gain 0 and 10, bias, the passive
/// stack's R1 to R3 in kΩ and C1 to C3 in nF (none for Bass), sag.
type Row = (usize, f64, f64, f64, Option<([f64; 3], [f64; 3])>, f64);
const ROWS: [Row; 7] = [
    (
        1,
        1.0,
        4.0,
        0.05,
        Some(([250.0, 1000.0, 25.0], [0.25, 20.0, 20.0])),
        0.05,
    ),
    (
        2,
        2.0,
        12.0,
        0.1,
        Some(([220.0, 1000.0, 22.0], [0.47, 22.0, 22.0])),
        0.1,
    ),
    (
        2,
        2.0,
        20.0,
        0.15,
        Some(([250.0, 250.0, 56.0], [0.25, 20.0, 20.0])),
        0.2,
    ),
    (
        3,
        3.0,
        30.0,
        0.1,
        Some(([220.0, 1000.0, 33.0], [0.47, 22.0, 22.0])),
        0.15,
    ),
    (
        3,
        5.0,
        60.0,
        0.2,
        Some(([250.0, 1000.0, 33.0], [0.47, 47.0, 22.0])),
        0.15,
    ),
    (
        4,
        6.0,
        80.0,
        0.2,
        Some(([220.0, 1000.0, 47.0], [0.68, 47.0, 47.0])),
        0.1,
    ),
    (2, 1.0, 10.0, 0.05, None, 0.2),
];

/// The passive stack's analog transfer function at `s`, from the circuit's
/// component values (Yeh and Smith), as complex numbers `(re, im)`.
fn analog_stack(r: [f64; 3], c: [f64; 3], t: f64, m: f64, bass: f64) -> ([f64; 4], [f64; 4]) {
    let (r1, r2, r3, r4) = (r[0] * 1e3, r[1] * 1e3, r[2] * 1e3, 56e3);
    let (c1, c2, c3) = (c[0] * 1e-9, c[1] * 1e-9, c[2] * 1e-9);
    let l = ((bass - 1.0) * 3.4).exp();
    let c = c1 * c2 * c3;
    let b1 = t * c1 * r1 + m * c3 * r3 + l * (c1 + c2) * r2 + (c1 + c2) * r3;
    let b2 = t * (c1 * c2 + c1 * c3) * r1 * r4 - m * m * (c1 + c2) * c3 * r3 * r3
        + m * (c1 * c3 * r1 * r3 + (c1 + c2) * c3 * r3 * r3)
        + l * (c1 * c2 * r1 * r2 + (c1 * c2 + c1 * c3) * r2 * r4)
        + l * m * (c1 + c2) * c3 * r2 * r3
        + c1 * c2 * r1 * r3
        + (c1 * c2 + c1 * c3) * r3 * r4;
    let b3 = l * m * c * (r1 * r2 * r3 + r2 * r3 * r4) - m * m * c * (r1 * r3 * r3 + r3 * r3 * r4)
        + m * c * (r1 * r3 * r3 + r3 * r3 * r4)
        + t * (1.0 - m) * c * r1 * r3 * r4
        + t * l * c * r1 * r2 * r4;
    let a1 = c1 * (r1 + r3) + c2 * (r3 + r4) + c3 * r4 + m * c3 * r3 + l * (c1 + c2) * r2;
    let a2 = m * (c1 * c3 * r1 * r3 - c2 * c3 * r3 * r4 + (c1 + c2) * c3 * r3 * r3)
        + l * m * (c1 + c2) * c3 * r2 * r3
        - m * m * (c1 + c2) * c3 * r3 * r3
        + l * (c1 * c2 * (r2 * r4 + r1 * r2) + (c1 + c2) * c3 * r2 * r4)
        + c1 * c2 * (r1 * r4 + r3 * r4 + r1 * r3)
        + c1 * c3 * (r1 * r4 + r3 * r4)
        + c2 * c3 * r3 * r4;
    let a3 = l * m * c * (r1 * r2 * r3 + r2 * r3 * r4) - m * m * c * (r1 * r3 * r3 + r3 * r3 * r4)
        + m * c * (r3 * r3 * r4 + r1 * r3 * r3 - r1 * r3 * r4)
        + l * c * r1 * r2 * r4
        + c * r1 * r3 * r4;
    ([0.0, b1, b2, b3], [1.0, a1, a2, a3])
}

/// A polynomial in `s` at `s = jw`, as `(re, im)`.
fn at_jw(p: &[f64; 4], w: f64) -> (f64, f64) {
    p.iter().enumerate().fold((0.0, 0.0), |(re, im), (k, c)| {
        let (cos, sin) = match k % 4 {
            0 => (1.0, 0.0),
            1 => (0.0, 1.0),
            2 => (-1.0, 0.0),
            _ => (0.0, -1.0),
        };
        (
            re + c * w.powi(k as i32) * cos,
            im + c * w.powi(k as i32) * sin,
        )
    })
}

fn stack_db(r: [f64; 3], c: [f64; 3], (t, m, bass): (f64, f64, f64), hz: f64) -> f64 {
    let (b, a) = analog_stack(r, c, t, m, bass);
    let w = TAU * hz;
    let (n, d) = (at_jw(&b, w), at_jw(&a, w));
    db(n.0.hypot(n.1) / d.0.hypot(d.1))
}

/// A cookbook biquad's `b` and `a` at `rate`: kind 0 low shelf, 1 peak, 2
/// high shelf.
fn rbj(kind: u8, f0: f64, gain_db: f64, q: f64, rate: f64) -> ([f64; 3], [f64; 3]) {
    let w0 = TAU * f0 / rate;
    let (sin, cos) = w0.sin_cos();
    let a = 10f64.powf(gain_db / 40.0);
    let alpha = sin / (2.0 * q);
    let root = a.sqrt();
    match kind {
        0 => (
            [
                a * ((a + 1.0) - (a - 1.0) * cos + 2.0 * root * alpha),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - 2.0 * root * alpha),
            ],
            [
                (a + 1.0) + (a - 1.0) * cos + 2.0 * root * alpha,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - 2.0 * root * alpha,
            ],
        ),
        1 => (
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        ),
        _ => (
            [
                a * ((a + 1.0) + (a - 1.0) * cos + 2.0 * root * alpha),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - 2.0 * root * alpha),
            ],
            [
                (a + 1.0) - (a - 1.0) * cos + 2.0 * root * alpha,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - 2.0 * root * alpha,
            ],
        ),
    }
}

/// A biquad's response size at `hz`, in dB, at `rate`.
fn biquad_db((b, a): ([f64; 3], [f64; 3]), hz: f64, rate: f64) -> f64 {
    let w = TAU * hz / rate;
    let poly = |c: [f64; 3]| {
        (
            c[0] + c[1] * w.cos() + c[2] * (2.0 * w).cos(),
            -c[1] * w.sin() - c[2] * (2.0 * w).sin(),
        )
    };
    let (n, d) = (poly(b), poly(a));
    db(n.0.hypot(n.1) / d.0.hypot(d.1))
}

/// The Bass model's active stack at 0 to 10 knobs: the cookbook filters at
/// the 4 times rate.
fn active_sections((bass, middle, treble): (f64, f64, f64)) -> [([f64; 3], [f64; 3]); 3] {
    [
        rbj(0, 100.0, 15.0 * (bass - 5.0) / 5.0, 0.71, HI),
        rbj(1, 600.0, 12.0 * (middle - 5.0) / 5.0, 0.7, HI),
        rbj(2, 4_000.0, 15.0 * (treble - 5.0) / 5.0, 0.71, HI),
    ]
}

/// The design response of a model's whole small-signal chain at `hz`, in
/// dB: its tone stack at knobs 0 to 10 and the high-passes (the 20 Hz ones
/// between stages, the two 5 Hz ones).
fn design_db(model: usize, knobs: (f64, f64, f64), hz: f64) -> f64 {
    let (stages, _, _, _, stack, _) = ROWS[model];
    let high_pass = |corner: f64| db(hz / hz.hypot(corner));
    let stack_db = match stack {
        Some((r, c)) => stack_db(r, c, (knobs.2 / 10.0, knobs.1 / 10.0, knobs.0 / 10.0), hz),
        None => active_sections(knobs)
            .iter()
            .map(|s| biquad_db(*s, hz, HI))
            .sum(),
    };
    stack_db + (stages - 1) as f64 * high_pass(20.0) + 2.0 * high_pass(5.0)
}

/// The device's response at `hz` from an impulse response, in dB.
fn response_db(ir: &[f32], hz: f64) -> f64 {
    let w = TAU * hz / RATE.as_f64();
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, sample) in ir.iter().enumerate() {
        re += f64::from(*sample) * (w * n as f64).cos();
        im -= f64::from(*sample) * (w * n as f64).sin();
    }
    db(re.hypot(im))
}

/// A very small impulse through `params`: small enough that every stage, the
/// last of Heavy's four included, is in its linear range.
fn small_signal(params: &[(ParamId, f64)]) -> Vec<f32> {
    let frames = 24_000;
    let mut input = vec![0.0f32; frames];
    input[0] = 1e-7;
    run(amp(params, frames).as_mut(), &input, &[])[0].clone()
}

// Spec: amp R1
#[test]
fn amp_is_described_and_laid_out() {
    let device_type = amp_type();
    device_type.description.check().unwrap();
    assert_eq!(
        device_type.kind,
        bl_model::DeviceKind::Native {
            device_type: AMP.to_owned()
        }
    );
    assert_eq!(device_type.name, "Amp");
    let names: Vec<_> = device_type
        .description
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Model", "Gain", "Bass", "Middle", "Treble", "Presence", "Volume", "Dry/Wet", "Output"
        ]
    );
    let model = device_type.description.param(MODEL).unwrap();
    let bl_model::ParamKind::Choice { choices } = &model.kind else {
        panic!("Model is a choice");
    };
    assert_eq!(
        choices,
        &["Clean", "Boost", "Blues", "Rock", "Lead", "Heavy", "Bass"]
    );
    assert!(bl_devices::device_types().iter().any(|t| t.name == "Amp"));
}

// Spec: amp R3
#[test]
fn the_triode_curve_follows_its_formula_on_a_sweep() {
    let formula = |x: f64| {
        if x >= 0.0 {
            1.5 * x / (1.0 + x.abs().powf(1.5)).powf(1.0 / 1.5)
        } else {
            1.2 * (x / 1.2).tanh() - 0.08 * x * x / (1.0 + x * x)
        }
    };
    let mut worst = 0.0f64;
    for i in 0..=8_000 {
        let x = -4.0 + 8.0 * f64::from(i) / 8_000.0;
        worst = worst.max((triode(x) - formula(x)).abs());
    }
    assert!(worst < 0.01, "{worst}");
    assert_eq!(triode(0.0), 0.0);
    // Asymmetric: the positive half clips higher than the negative one.
    assert!(triode(5.0) > -triode(-5.0));
}

// Spec: amp R2
#[test]
fn the_tone_stack_follows_the_preamp() {
    // A distorted 220 Hz sine: Treble down takes the upper harmonics off the
    // distortion the preamp made, and leaves the fundamental where it was.
    let high_harmonics = |treble: f64| {
        let params = [(MODEL, 3.0), (GAIN, 10.0), (TREBLE, treble)];
        let frames = 96_000;
        let out = run(
            amp(&params, frames).as_mut(),
            &sine(220.0, 0.25, frames),
            &[],
        );
        let tail = &out[0][frames - 48_000..];
        let upper: f64 = (9..=15).map(|h| magnitude(tail, h * 220).powi(2)).sum();
        (db(magnitude(tail, 220)), 10.0 * upper.max(1e-30).log10())
    };
    let (low_fundamental, low) = high_harmonics(0.0);
    let (high_fundamental, high) = high_harmonics(10.0);
    assert!(high - low > 6.0, "{low} dB at Treble 0, {high} dB at 10");
    assert!((high_fundamental - low_fundamental).abs() < 6.0);
}

// Spec: amp R4
#[test]
fn each_models_tone_stack_follows_its_design_at_nine_knob_settings() {
    let settings = [
        (5.0, 5.0, 5.0),
        (0.0, 5.0, 5.0),
        (10.0, 5.0, 5.0),
        (5.0, 0.0, 5.0),
        (5.0, 10.0, 5.0),
        (5.0, 5.0, 0.0),
        (5.0, 5.0, 10.0),
        (0.0, 0.0, 0.0),
        (10.0, 10.0, 10.0),
    ];
    let hertz: Vec<f64> = (0..40)
        .map(|i| 20.0 * 800f64.powf(f64::from(i) / 39.0))
        .collect();
    for model in 0..7 {
        for knobs in settings {
            let ir = small_signal(&[
                (MODEL, model as f64),
                (GAIN, 0.0),
                (BASS, knobs.0),
                (MIDDLE, knobs.1),
                (TREBLE, knobs.2),
            ]);
            // Levels relative to 1 kHz, so the stack's make-up gain drops out.
            let measured_ref = response_db(&ir, 1_000.0);
            let design_ref = design_db(model, knobs, 1_000.0);
            for hz in &hertz {
                let measured = response_db(&ir, *hz) - measured_ref;
                let design = design_db(model, knobs, *hz) - design_ref;
                assert!(
                    (measured - design).abs() < 0.5,
                    "{} at {knobs:?}, {hz:.0} Hz: {measured:.2} dB against {design:.2} dB",
                    ["Clean", "Boost", "Blues", "Rock", "Lead", "Heavy", "Bass"][model]
                );
            }
        }
    }
}

// Spec: amp R5
#[test]
fn presence_is_a_high_shelf_of_minus_six_to_plus_nine_db_at_3_khz() {
    let reference = small_signal(&[(PRESENCE, 4.0)]);
    for (presence, gain_db) in [(0.0, -6.0), (5.0, 1.5), (10.0, 9.0)] {
        let ir = small_signal(&[(PRESENCE, presence)]);
        let shelf = rbj(2, 3_000.0, gain_db, 0.71, HI);
        for hz in [100.0, 1_000.0, 2_000.0, 3_000.0, 4_500.0, 8_000.0, 16_000.0] {
            let measured = response_db(&ir, hz) - response_db(&reference, hz);
            let design = biquad_db(shelf, hz, HI);
            assert!(
                (measured - design).abs() < 0.5,
                "Presence {presence} at {hz} Hz: {measured:.2} dB against {design:.2} dB"
            );
        }
    }
    // The shelf's ends: down 6 dB and up 9 dB.
    let low = small_signal(&[(PRESENCE, 0.0)]);
    let high = small_signal(&[(PRESENCE, 10.0)]);
    assert!(response_db(&low, 12_000.0) - response_db(&reference, 12_000.0) < -5.0);
    assert!(response_db(&high, 12_000.0) - response_db(&reference, 12_000.0) > 8.0);
}

/// A model's chain written out in f64 at 4 times the rate, with the spec's
/// values: the preamp's stages and their high-passes, the 5 Hz blockers, the
/// tone stack at Presence 0 dB, the power stage with sag and Volume's scale.
struct Reference {
    model: usize,
    gain: f64,
    knobs: (f64, f64, f64),
    volume: f64,
    sag: bool,
}

/// A direct form I filter from `b` and `a` (with `a[0]` 1 or not).
struct Direct {
    b: Vec<f64>,
    a: Vec<f64>,
    x: Vec<f64>,
    y: Vec<f64>,
}

impl Direct {
    fn new(b: Vec<f64>, a: Vec<f64>) -> Self {
        let (x, y) = (vec![0.0; b.len()], vec![0.0; a.len()]);
        let a0 = a[0];
        Self {
            b: b.iter().map(|v| v / a0).collect(),
            a: a.iter().map(|v| v / a0).collect(),
            x,
            y,
        }
    }

    fn high_pass(hz: f64) -> Self {
        let pole = (-TAU * hz / HI).exp();
        Self::new(vec![1.0, -1.0], vec![1.0, -pole])
    }

    fn process(&mut self, x: f64) -> f64 {
        self.x.rotate_right(1);
        self.x[0] = x;
        let mut y: f64 = self.b.iter().zip(&self.x).map(|(b, x)| b * x).sum();
        y -= self.a[1..]
            .iter()
            .zip(&self.y)
            .map(|(a, y)| a * y)
            .sum::<f64>();
        self.y.rotate_right(1);
        self.y[0] = y;
        y
    }
}

/// The product of two polynomials in z.
fn multiply(p: &[f64], q: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; p.len() + q.len() - 1];
    for (i, a) in p.iter().enumerate() {
        for (j, b) in q.iter().enumerate() {
            out[i + j] += a * b;
        }
    }
    out
}

/// The bilinear transform of `b(s) / a(s)` at the 4 times rate, in powers of
/// z⁻¹.
fn bilinear(b: &[f64; 4], a: &[f64; 4]) -> (Vec<f64>, Vec<f64>) {
    let c = 2.0 * HI;
    let convert = |p: &[f64; 4]| {
        let mut out = vec![0.0; 4];
        for (k, coefficient) in p.iter().enumerate() {
            let mut term = vec![1.0];
            for _ in 0..k {
                term = multiply(&term, &[1.0, -1.0]);
            }
            for _ in k..3 {
                term = multiply(&term, &[1.0, 1.0]);
            }
            for (o, t) in out.iter_mut().zip(term) {
                *o += coefficient * c.powi(k as i32) * t;
            }
        }
        out
    };
    (convert(b), convert(a))
}

impl Reference {
    /// The output for `input`, at 4 times the rate.
    fn render(&self, input: &[f64]) -> Vec<f64> {
        let (stages, lo, hi, beta, stack, sag) = ROWS[self.model];
        let k = lo * (hi / lo).powf(self.gain / 10.0);
        let triode = |x: f64| {
            if x >= 0.0 {
                1.5 * x / (1.0 + x.powf(1.5)).powf(1.0 / 1.5)
            } else {
                1.2 * (x / 1.2).tanh() - 0.08 * x * x / (1.0 + x * x)
            }
        };
        let (bass, middle, treble) = self.knobs;
        let mut coupling: Vec<Direct> = (1..stages).map(|_| Direct::high_pass(20.0)).collect();
        let mut dc = [Direct::high_pass(5.0), Direct::high_pass(5.0)];
        let (mut sections, makeup): (Vec<Direct>, f64) = match stack {
            Some((r, c)) => {
                let (b, a) = analog_stack(r, c, treble / 10.0, middle / 10.0, bass / 10.0);
                let (num, den) = bilinear(&b, &a);
                let (b5, a5) = analog_stack(r, c, 0.5, 0.5, 0.5);
                let w = TAU * 1_000.0;
                let (n, d) = (at_jw(&b5, w), at_jw(&a5, w));
                (vec![Direct::new(num, den)], d.0.hypot(d.1) / n.0.hypot(n.1))
            }
            None => (
                active_sections(self.knobs)
                    .iter()
                    .map(|(b, a)| Direct::new(b.to_vec(), a.to_vec()))
                    .collect(),
                1.0,
            ),
        };
        let volume = self.volume;
        let comp = 1.0 / volume.max(0.01);
        let coef = |seconds: f64| 1.0 - (-1.0 / (seconds * HI)).exp();
        let (attack, release) = (coef(0.03), coef(0.3));
        let mut level = 0.0f64;
        let bias = triode(beta);
        input
            .iter()
            .map(|x| {
                let mut u = *x;
                u = triode(k * u + beta) - bias;
                for filter in &mut coupling {
                    u = triode(k * filter.process(u) + beta) - bias;
                }
                u = dc[0].process(u) * makeup;
                for section in &mut sections {
                    u = section.process(u);
                }
                let share = if self.sag { sag } else { 0.0 };
                let y = (volume * (1.0 - share * level.min(1.0)) * u).tanh();
                let rate = if y.abs() > level { attack } else { release };
                level += (y.abs() - level) * rate;
                dc[1].process(y * comp)
            })
            .collect()
    }
}

/// A 220 Hz sine of `amplitude`, `seconds` long, at the 4 times rate.
fn hi_sine(hz: f64, amplitude: f64, seconds: usize) -> Vec<f64> {
    (0..seconds * HI as usize)
        .map(|i| amplitude * (TAU * hz * i as f64 / HI).sin())
        .collect()
}

fn reference_tail(reference: &Reference, hz: f64, amplitude: f64) -> Vec<f32> {
    let out = reference.render(&hi_sine(hz, amplitude, 3));
    out[2 * HI as usize..].iter().map(|v| *v as f32).collect()
}

/// A 220 Hz sine at -12 dBFS through a model at Gain 5, three seconds, and
/// the last second of it.
fn device_tail(model: usize, extra: &[(ParamId, f64)]) -> Vec<f32> {
    let frames = 144_000;
    let mut params = vec![(MODEL, model as f64)];
    params.extend_from_slice(extra);
    let out = run(
        amp(&params, frames).as_mut(),
        &sine(220.0, 0.2512, frames),
        &[],
    );
    out[0][frames - 48_000..].to_vec()
}

// Spec: amp R7
#[test]
fn each_models_second_and_third_harmonic_follow_the_design() {
    for model in 0..7 {
        let reference = Reference {
            model,
            gain: 5.0,
            knobs: (5.0, 5.0, 5.0),
            volume: 1.0,
            sag: true,
        };
        let expected = reference_tail(&reference, 220.0, 0.2512);
        let measured = device_tail(model, &[]);
        let balance = |s: &[f32]| db(magnitude(s, 440)) - db(magnitude(s, 660));
        let (m, e) = (balance(&measured), balance(&expected));
        assert!(
            (m - e).abs() < 1.0,
            "model {model}: {m:.2} dB against {e:.2} dB"
        );
        // The level is the design's too.
        let (m, e) = (db(magnitude(&measured, 220)), db(magnitude(&expected, 220)));
        assert!(
            (m - e).abs() < 1.0,
            "model {model}: fundamental {m:.2} dB against {e:.2} dB"
        );
    }
}

// Spec: amp R7
#[test]
fn no_model_leaves_dc_above_minus_80_dbfs() {
    for model in 0..7 {
        for gain in [0.0, 10.0] {
            let tail = device_tail(model, &[(GAIN, gain)]);
            let mean: f64 = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
            assert!(
                db(mean.abs()) < -80.0,
                "model {model} at Gain {gain}: {} dBFS",
                db(mean.abs())
            );
        }
    }
}

// Spec: amp R5
#[test]
fn sag_follows_the_power_stage_envelope() {
    // A 1 kHz sine at Volume 0 dB, loud enough to load the supply: the device follows the reference with
    // sag, and the reference without it is further off than the tolerance.
    let model = 2;
    let level = |tail: &[f32]| db(magnitude(tail, 1_000));
    let frames = 144_000;
    let params = [(MODEL, model as f64), (GAIN, 3.0), (VOLUME, 1.0)];
    let out = run(
        amp(&params, frames).as_mut(),
        &sine(1_000.0, 0.3, frames),
        &[],
    );
    let measured = level(&out[0][frames - 48_000..]);
    let make = |sag| Reference {
        model,
        gain: 3.0,
        knobs: (5.0, 5.0, 5.0),
        volume: 1.0,
        sag,
    };
    let with = level(&reference_tail(&make(true), 1_000.0, 0.3));
    let without = level(&reference_tail(&make(false), 1_000.0, 0.3));
    assert!(
        (measured - with).abs() < 0.2,
        "{measured:.2} dB against {with:.2} dB"
    );
    assert!(
        (without - with).abs() > 0.3,
        "sag changes the level by {:.2} dB",
        without - with
    );
}

// Spec: amp R8
#[test]
fn volume_at_minus_infinity_is_silent_and_a_clean_signal_keeps_its_level() {
    let input = sine(1_000.0, 0.03, 48_000);
    let silent = run(
        amp(&[(MODEL, 0.0), (GAIN, 0.0), (VOLUME, 0.0)], 48_000).as_mut(),
        &input,
        &[],
    );
    assert!(silent[0].iter().all(|s| *s == 0.0));
    let level = |volume_db: f64| {
        let volume = 10f64.powf(volume_db / 20.0);
        let out = run(
            amp(&[(MODEL, 0.0), (GAIN, 0.0), (VOLUME, volume)], 48_000).as_mut(),
            &input,
            &[],
        );
        db(rms(&out[0][24_000..]))
    };
    let reference = level(0.0);
    for volume_db in [-40.0, -24.0, -12.0, -6.0, 6.0, 12.0] {
        let l = level(volume_db);
        assert!(
            (l - reference).abs() < 1.0,
            "Volume {volume_db} dB: {l:.2} against {reference:.2}"
        );
    }
    // Driven harder, a loud signal distorts more.
    let loud = sine(1_000.0, 0.5, 72_000);
    let distortion = |volume: f64| {
        let out = run(
            amp(&[(MODEL, 0.0), (GAIN, 0.0), (VOLUME, volume)], 72_000).as_mut(),
            &loud,
            &[],
        );
        let tail = &out[0][24_000..];
        let harmonics: f64 = (2..=9).map(|h| magnitude(tail, h * 1_000).powi(2)).sum();
        10.0 * (harmonics / magnitude(tail, 1_000).powi(2)).log10()
    };
    assert!(distortion(4.0) > distortion(0.25) + 1.0);
}

/// Runs `left` and `right` through an Amp of `params`, `block` frames at a
/// time with `events`.
fn stereo(
    params: &[(ParamId, f64)],
    left: &[f32],
    right: &[f32],
    block: usize,
    events: &[bl_engine::NodeEvent],
) -> Vec<Vec<f32>> {
    let tempo = TempoMap::constant(Bpm::DEFAULT);
    let mut node = amp(params, block);
    run_blocks(
        node.as_mut(),
        &[left.to_vec(), right.to_vec()],
        block,
        events,
        &tempo,
    )
}

// Spec: amp R9
#[test]
fn dual_keeps_a_left_only_signal_off_the_right_and_mono_sums() {
    let left = sine(330.0, 0.3, 9_600);
    let silence = vec![0.0f32; 9_600];
    let dual = stereo(&[(MODEL, 3.0), (OUTPUT, 1.0)], &left, &silence, 9_600, &[]);
    assert!(
        dual[1].iter().all(|s| *s == 0.0),
        "the right channel stays silent"
    );
    assert!(rms(&dual[0]) > 0.01);
    // Mono on a left-only signal gives both channels the amp of half of it.
    let mono = stereo(&[(MODEL, 3.0), (OUTPUT, 0.0)], &left, &silence, 9_600, &[]);
    assert!(mono[0] == mono[1], "Mono plays one signal on both channels");
    let half: Vec<f32> = left.iter().map(|s| s * 0.5).collect();
    let same = stereo(&[(MODEL, 3.0), (OUTPUT, 1.0)], &half, &half, 9_600, &[]);
    assert!(
        mono[0] == same[0],
        "Mono runs the amp on the mean of the channels"
    );
}

// Spec: amp R10
#[test]
fn every_model_reports_the_same_latency_and_dry_wet_aligns_the_dry_signal() {
    let latencies: Vec<u32> = (0..7)
        .flat_map(|model| {
            [0.0, 10.0].map(|gain| {
                amp(
                    &[(MODEL, f64::from(model)), (GAIN, gain), (DRY_WET, 30.0)],
                    64,
                )
                .latency()
            })
        })
        .collect();
    assert!(latencies[0] as usize == LATENCY);
    assert!(
        latencies.iter().all(|l| *l == latencies[0]),
        "{latencies:?}"
    );
    let input = noise(4_096);
    let dry = run(amp(&[(DRY_WET, 0.0)], 4_096).as_mut(), &input, &[]);
    assert!(dry[0][..LATENCY].iter().all(|s| *s == 0.0));
    assert!(
        dry[0][LATENCY..] == input[..input.len() - LATENCY],
        "dry, delayed by the latency"
    );
    let wet = run(
        amp(&[(DRY_WET, 100.0), (MODEL, 1.0)], 4_096).as_mut(),
        &input,
        &[],
    );
    let half = run(
        amp(&[(DRY_WET, 50.0), (MODEL, 1.0)], 4_096).as_mut(),
        &input,
        &[],
    );
    for n in LATENCY..4_096 {
        let expected = 0.5 * dry[0][n] + 0.5 * wet[0][n];
        assert!((half[0][n] - expected).abs() < 1e-6, "frame {n}");
    }
}

// Spec: amp R11
#[test]
fn aliasing_from_a_5_khz_sine_at_gain_10_stays_70_db_below_the_fundamental() {
    for model in 0..7 {
        let frames = 96_000;
        let out = run(
            amp(&[(MODEL, model as f64), (GAIN, 10.0)], frames).as_mut(),
            &sine(5_000.0, 0.2512, frames),
            &[],
        );
        let tail = &out[0][frames - 48_000..];
        let fundamental = magnitude(tail, 5_000);
        // The folded harmonics land on whole kHz that aren't harmonics of
        // 5 kHz, below 20 kHz.
        for khz in (1..20).filter(|k| k % 5 != 0) {
            let alias = db(magnitude(tail, khz * 1_000) / fundamental);
            assert!(alias < -70.0, "model {model}: {alias:.1} dB at {khz} kHz");
        }
    }
}

// Spec: amp R11
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let left = noise(8_192);
    let right: Vec<f32> = noise(8_192).iter().rev().copied().collect();
    let params = [
        (MODEL, 4.0),
        (GAIN, 8.0),
        (BASS, 3.0),
        (TREBLE, 8.0),
        (PRESENCE, 7.0),
        (VOLUME, 2.0),
        (DRY_WET, 80.0),
        (OUTPUT, 1.0),
    ];
    let events = [
        set(2_500, MODEL, 5.0),
        set(3_001, GAIN, 4.0),
        set(5_000, OUTPUT, 0.0),
        set(6_000, MODEL, 6.0),
    ];
    let render = |block: usize| stereo(&params, &left, &right, block, &events);
    let reference = render(4_096);
    assert!(reference[0].iter().any(|s| *s != 0.0));
    for block in [1, 64, 512] {
        assert!(render(block) == reference, "block size {block}");
    }
}

// Spec: amp R12
#[test]
fn a_model_change_crossfades_without_a_click() {
    let frames = 24_000;
    let input = sine(220.0, 0.2, frames);
    let out = stereo(
        &[(MODEL, 3.0), (GAIN, 6.0)],
        &input,
        &input,
        512,
        &[set(12_000, MODEL, 5.0)],
    );
    let jump = |range: std::ops::Range<usize>| {
        out[0][range.clone()]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let before = jump(8_000..12_000);
    let after = jump(20_000..24_000);
    let across = jump(11_900..13_500);
    assert!(
        across <= 1.5 * before.max(after),
        "{across} across the change, {before} before and {after} after"
    );
    // The new model is playing at the end: it differs from the old one.
    let old = stereo(&[(MODEL, 3.0), (GAIN, 6.0)], &input, &input, 512, &[]);
    assert!(out[0][20_000..] != old[0][20_000..]);
}
