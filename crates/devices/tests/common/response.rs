//! Frequency responses read from a node's impulse response, and the
//! textbook filters (RBJ's audio-EQ cookbook, written out again here so a
//! test leans on the curve and not on the device's code) to compare them
//! with.

use std::f64::consts::TAU;

use bl_engine::Node;

use super::{RATE, run};

/// A complex number, real and imaginary parts.
pub type Complex = (f64, f64);

/// The node's answer to an impulse on the left channel, `frames` long (the
/// node takes blocks that long).
pub fn impulse_response(node: &mut dyn Node, frames: usize) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    input[0] = 1.0;
    run(node, &input, &[])[0].clone()
}

/// The response at `hz` of a system whose impulse response is `ir`.
pub fn response_at(ir: &[f32], hz: f64) -> Complex {
    let w = TAU * hz / RATE.as_f64();
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, sample) in ir.iter().enumerate() {
        re += f64::from(*sample) * (w * n as f64).cos();
        im -= f64::from(*sample) * (w * n as f64).sin();
    }
    (re, im)
}

/// A response's size, in dB.
pub fn db((re, im): Complex) -> f64 {
    20.0 * re.hypot(im).max(1e-12).log10()
}

/// The sum of two responses.
pub fn add(a: Complex, b: Complex) -> Complex {
    (a.0 + b.0, a.1 + b.1)
}

/// The product of two responses.
pub fn mul(a: Complex, b: Complex) -> Complex {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// The cookbook filters the tests use.
#[derive(Clone, Copy)]
pub enum Kind {
    LowShelf,
    HighShelf,
    Peak,
    LowPass,
    HighPass,
}

/// A biquad's `b` and `a` coefficients (unnormalised).
pub type Biquad = ([f64; 3], [f64; 3]);

/// The cookbook's `kind` at `f0`, `gain_db` and `q`.
pub fn rbj(kind: Kind, f0: f64, gain_db: f64, q: f64) -> Biquad {
    let w0 = TAU * f0 / RATE.as_f64();
    let (sin, cos) = w0.sin_cos();
    let a = 10f64.powf(gain_db / 40.0);
    let alpha = sin / (2.0 * q);
    let root = a.sqrt();
    match kind {
        Kind::LowShelf => (
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
        Kind::HighShelf => (
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
        Kind::Peak => (
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        ),
        Kind::LowPass => (
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        ),
        Kind::HighPass => (
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        ),
    }
}

/// A biquad's response at `hz`.
pub fn biquad_response((b, a): Biquad, hz: f64) -> Complex {
    let w = TAU * hz / RATE.as_f64();
    let z = |n: f64| ((-w * n).cos(), (-w * n).sin());
    let poly = |c: [f64; 3]| {
        let (z1, z2) = (z(1.0), z(2.0));
        (c[0] + c[1] * z1.0 + c[2] * z2.0, c[1] * z1.1 + c[2] * z2.1)
    };
    let (n, d) = (poly(b), poly(a));
    let size = d.0 * d.0 + d.1 * d.1;
    (
        (n.0 * d.0 + n.1 * d.1) / size,
        (n.1 * d.0 - n.0 * d.1) / size,
    )
}

/// The frequency in `low..=high` where `level_db(hz)` falls through `target`
/// (it must fall across the range), by bisection.
pub fn crossing(mut low: f64, mut high: f64, target: f64, level_db: impl Fn(f64) -> f64) -> f64 {
    for _ in 0..40 {
        let mid = (low * high).sqrt();
        if level_db(mid) > target {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low * high).sqrt()
}
