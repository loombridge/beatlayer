//! The Vocoder's filter banks (spec: vocoder R2 to R4): bands spaced evenly
//! in log frequency, each a Butterworth band-pass whose design centre and
//! −3 dB bandwidth the digital filter keeps exactly, in structure-of-arrays
//! form so the bands run side by side.
//!
//! A band-pass of `f0` and `bw` hertz is the bilinear transform of the
//! analogue Butterworth band-pass whose centre is `f0` prewarped and whose
//! bandwidth is solved so the digital −3 dB edges are `bw` apart, which has
//! a closed form: `B = 2 fs (1 + u²) tan(π bw / fs)` with `u = tan(π f0 / fs)`.
//! Precise runs the fourth-order one (two staggered biquads); Retro the
//! second-order one (one biquad) with a 2 dB boost at its centre.

use std::f64::consts::PI;

/// The most bands a bank has (R2).
pub const MAX_BANDS: usize = 40;

/// The band counts the Bands chooser offers, by index.
pub const BAND_COUNTS: [usize; 10] = [4, 8, 12, 16, 20, 24, 28, 32, 36, 40];

/// The widest a band's −3 dB bandwidth is made, as a fraction of the sample
/// rate: a band wider than the spectrum has no digital design, and the
/// topmost bands of a bank with few bands would ask for one.
const WIDEST: f64 = 0.4;

/// The highest centre a band has, as a fraction of the sample rate; a band
/// the Formant control moves above it is silent.
const HIGHEST: f64 = 0.49;

/// Retro's boost at each band's centre, in dB.
const RETRO_BOOST_DB: f64 = 2.0;

/// The centre of band `k` of `count`, evenly spaced in log frequency from
/// `low` to `high` (R2).
#[must_use]
pub fn centre(count: usize, low: f64, high: f64, k: usize) -> f64 {
    low * (high / low).powf(k as f64 / (count - 1).max(1) as f64)
}

/// The step between adjacent bands' centres, in natural-log frequency.
#[must_use]
pub fn step(count: usize, low: f64, high: f64) -> f64 {
    (high / low).ln() / (count - 1).max(1) as f64
}

/// The −3 dB bandwidth, in hertz, a band of centre `centre` is designed for
/// at `bw` per cent (R3): the bandwidth of edges `bw`% of a step either side
/// of the centre in log frequency, so at 100 % adjacent bands meet at
/// −3 dB, limited to what the sample rate can hold.
#[must_use]
pub fn bandwidth(centre: f64, step: f64, bw: f64, rate: f64) -> f64 {
    (2.0 * centre * (bw * 0.01 * step * 0.5).sinh()).min(WIDEST * rate)
}

/// The coefficients of a bank: per band and stage the numerator's `b0`
/// (`b1` is 0 and `b2` is `-b0` in a band-pass) and the denominator's `a1`
/// and `a2`.
#[derive(Clone)]
pub struct Coeffs {
    b0: [[f64; MAX_BANDS]; 2],
    a1: [[f64; MAX_BANDS]; 2],
    a2: [[f64; MAX_BANDS]; 2],
    /// Biquads per band: 2 (Precise) or 1 (Retro).
    stages: usize,
    /// Bands in use.
    count: usize,
    /// The design each band was made for: its centre and bandwidth.
    centres: [f64; MAX_BANDS],
    bandwidths: [f64; MAX_BANDS],
}

impl Coeffs {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            b0: [[0.0; MAX_BANDS]; 2],
            a1: [[0.0; MAX_BANDS]; 2],
            a2: [[0.0; MAX_BANDS]; 2],
            stages: 2,
            count: 0,
            centres: [0.0; MAX_BANDS],
            bandwidths: [0.0; MAX_BANDS],
        }
    }

    /// Designs `count` bands from `low` to `high` hertz at `bw` per cent,
    /// with every centre moved by `shift` (a frequency ratio; the synthesis
    /// bank's Formant), Retro's or Precise's.
    pub fn design(
        &mut self,
        (count, low, high): (usize, f64, f64),
        bw: f64,
        shift: f64,
        retro: bool,
        rate: f64,
    ) {
        self.count = count.min(MAX_BANDS);
        self.stages = if retro { 1 } else { 2 };
        let spacing = step(count, low, high);
        for k in 0..self.count {
            let nominal = centre(count, low, high, k);
            let f0 = nominal * shift;
            // The design bandwidth follows the unshifted spacing, in
            // proportion to the centre: a shifted band keeps its width in
            // octaves.
            let width = bandwidth(f0, spacing, bw, rate);
            self.centres[k] = f0;
            self.bandwidths[k] = width;
            self.band(k, f0, width, retro, rate);
        }
    }

    /// Band `k`'s biquads for centre `f0` and −3 dB bandwidth `width`.
    fn band(&mut self, k: usize, f0: f64, width: f64, retro: bool, rate: f64) {
        for stage in 0..2 {
            self.b0[stage][k] = 0.0;
            self.a1[stage][k] = 0.0;
            self.a2[stage][k] = 0.0;
        }
        if f0 >= HIGHEST * rate || f0 <= 0.0 {
            return;
        }
        let c = 2.0 * rate;
        let u = (PI * f0 / rate).tan();
        let omega = c * u;
        let b = c * (1.0 + u * u) * (PI * width / rate).tan();
        if retro {
            // B s / (s² + B s + ω0²).
            let boost = 10f64.powf(RETRO_BOOST_DB / 20.0);
            self.biquad(0, k, b, -b * 0.5, omega * omega, c, boost);
            return;
        }
        // The prototype's pole (−1 + j) / √2 becomes, under the band-pass
        // transform, the pair s = (B p ± √(B² p² − 4 ω0²)) / 2, one pole for
        // each biquad (with its conjugate).
        let p = (-1.0 / 2f64.sqrt(), 1.0 / 2f64.sqrt());
        let bp = (b * p.0, b * p.1);
        let square = (
            bp.0 * bp.0 - bp.1 * bp.1 - 4.0 * omega * omega,
            2.0 * bp.0 * bp.1,
        );
        let root = complex_sqrt(square);
        for (stage, sign) in [1.0, -1.0].into_iter().enumerate() {
            let pole = ((bp.0 + sign * root.0) * 0.5, (bp.1 + sign * root.1) * 0.5);
            self.biquad(
                stage,
                k,
                b,
                pole.0,
                pole.0 * pole.0 + pole.1 * pole.1,
                c,
                1.0,
            );
        }
    }

    /// Stage `stage` of band `k`: `B s / (s² − 2 σ s + m)` by the bilinear
    /// transform with `s = c (z − 1) / (z + 1)`, scaled by `gain`.
    #[expect(clippy::too_many_arguments, reason = "a pole's parts, and the stage")]
    fn biquad(&mut self, stage: usize, k: usize, b: f64, sigma: f64, m: f64, c: f64, gain: f64) {
        let d0 = c * c - 2.0 * sigma * c + m;
        self.b0[stage][k] = gain * b * c / d0;
        self.a1[stage][k] = 2.0 * (m - c * c) / d0;
        self.a2[stage][k] = (c * c + 2.0 * sigma * c + m) / d0;
    }

    /// The bands in use.
    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Band `k`'s design centre, in hertz.
    #[must_use]
    pub fn centre(&self, k: usize) -> f64 {
        self.centres[k]
    }

    /// Band `k`'s design −3 dB bandwidth, in hertz.
    #[must_use]
    pub fn bandwidth(&self, k: usize) -> f64 {
        self.bandwidths[k]
    }

    /// Band `k`'s response at `hz`, as a complex number (real, imaginary),
    /// worked out from its coefficients.
    #[must_use]
    pub fn response(&self, k: usize, hz: f64, rate: f64) -> (f64, f64) {
        let w = 2.0 * PI * hz / rate;
        let z1 = (w.cos(), -w.sin());
        let z2 = ((2.0 * w).cos(), -(2.0 * w).sin());
        let mut total = (1.0, 0.0);
        for stage in 0..self.stages {
            let b0 = self.b0[stage][k];
            let num = (b0 * (1.0 - z2.0), -b0 * z2.1);
            let den = (
                1.0 + self.a1[stage][k] * z1.0 + self.a2[stage][k] * z2.0,
                self.a1[stage][k] * z1.1 + self.a2[stage][k] * z2.1,
            );
            let norm = den.0 * den.0 + den.1 * den.1;
            let h = (
                (num.0 * den.0 + num.1 * den.1) / norm,
                (num.1 * den.0 - num.0 * den.1) / norm,
            );
            total = (total.0 * h.0 - total.1 * h.1, total.0 * h.1 + total.1 * h.0);
        }
        total
    }
}

impl Default for Coeffs {
    fn default() -> Self {
        Self::new()
    }
}

/// The square root of a complex number with the non-negative real part.
fn complex_sqrt((re, im): (f64, f64)) -> (f64, f64) {
    let modulus = re.hypot(im);
    let real = ((modulus + re) * 0.5).max(0.0).sqrt();
    let imag = ((modulus - re) * 0.5).max(0.0).sqrt();
    (real, if im < 0.0 { -imag } else { imag })
}

/// The state of one bank: every band's two biquads, in direct form II
/// transposed, a band to a lane.
#[derive(Clone)]
pub struct Bank {
    z1: [[f64; MAX_BANDS]; 2],
    z2: [[f64; MAX_BANDS]; 2],
}

impl Bank {
    /// A silent bank.
    #[must_use]
    pub fn new() -> Self {
        Self {
            z1: [[0.0; MAX_BANDS]; 2],
            z2: [[0.0; MAX_BANDS]; 2],
        }
    }

    /// Runs one sample through every band: bands below `split` take `low`
    /// and the rest take `high`, and each band's output is written to `out`.
    pub fn tick(
        &mut self,
        c: &Coeffs,
        (low, high): (f64, f64),
        split: usize,
        out: &mut [f64; MAX_BANDS],
    ) {
        let n = c.count;
        let split = split.min(n);
        self.run(c, 0..split, low, out);
        self.run(c, split..n, high, out);
    }

    fn run(
        &mut self,
        c: &Coeffs,
        bands: std::ops::Range<usize>,
        x: f64,
        out: &mut [f64; MAX_BANDS],
    ) {
        let [z1a, z1b] = &mut self.z1;
        let [z2a, z2b] = &mut self.z2;
        if c.stages == 1 {
            for k in bands {
                let y = c.b0[0][k] * x + z1a[k];
                z1a[k] = z2a[k] - c.a1[0][k] * y;
                z2a[k] = -c.b0[0][k] * x - c.a2[0][k] * y;
                out[k] = y;
            }
            return;
        }
        for k in bands {
            let y = c.b0[0][k] * x + z1a[k];
            z1a[k] = z2a[k] - c.a1[0][k] * y;
            z2a[k] = -c.b0[0][k] * x - c.a2[0][k] * y;
            let w = c.b0[1][k] * y + z1b[k];
            z1b[k] = z2b[k] - c.a1[1][k] * w;
            z2b[k] = -c.b0[1][k] * y - c.a2[1][k] * w;
            out[k] = w;
        }
    }

    /// Sets states that have decayed below audibility to zero, so a long
    /// silence never reaches the denormals.
    pub fn flush(&mut self) {
        for state in self.z1.iter_mut().chain(self.z2.iter_mut()) {
            for z in state {
                if z.abs() < 1e-30 {
                    *z = 0.0;
                }
            }
        }
    }
}

impl Default for Bank {
    fn default() -> Self {
        Self::new()
    }
}
