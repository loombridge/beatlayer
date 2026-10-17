//! Filter sections and the spectrum analyser that Contour and the other
//! EQs share: a second-order section in transposed direct form II, the
//! magnitude response of its coefficients, and an FFT analyser of 64
//! logarithmic bands.

use std::f64::consts::TAU;

use bl_model::DisplaySeries;

use crate::framework::Audio;

/// The analyser's window, in frames.
pub(crate) const ANALYSER_WINDOW: usize = 1024;
/// The analyser's bands, from [`ANALYSER_MIN_HZ`] to [`ANALYSER_MAX_HZ`].
pub const ANALYSER_BANDS: usize = 64;
pub(crate) const ANALYSER_MIN_HZ: f64 = 20.0;
pub(crate) const ANALYSER_MAX_HZ: f64 = 20_000.0;
/// What the analyser reads for a band with nothing in it, in dB.
pub(crate) const ANALYSER_FLOOR_DB: f32 = -100.0;

/// How often an EQ's coefficients are worked out, in frames: often enough
/// that a knob's motion is smooth, rarely enough to keep the sin and cos
/// cheap.
pub(crate) const REFRESH_FRAMES: usize = 16;

/// How many places along 20 Hz to 20 kHz each curve of a display is worked
/// out at.
pub const CURVE_POINTS: usize = 96;

/// The levels a display's spectrum shows between its bottom and its top, in
/// dB.
const SPECTRUM_RANGE_DB: [f32; 2] = [-90.0, 0.0];

/// The rate the displays' curves are worked out at. The curves are analytic
/// and the display doesn't know the engine's rate; they differ from the
/// running filters' only near Nyquist at other rates.
/// shortcut: 48 kHz for every set, when the engine's rate reaches the shell.
pub(crate) const DISPLAY_RATE: f64 = 48_000.0;

/// The frequencies a display's curves are worked out at, [`CURVE_POINTS`] of
/// them, 20 Hz to 20 kHz, evenly spaced on a logarithmic axis.
pub(crate) fn curve_freqs() -> Vec<f64> {
    (0..CURVE_POINTS)
        .map(|i| {
            ANALYSER_MIN_HZ
                * (ANALYSER_MAX_HZ / ANALYSER_MIN_HZ).powf(i as f64 / (CURVE_POINTS - 1) as f64)
        })
        .collect()
}

/// A display's curve in dB over 20 Hz to 20 kHz.
pub(crate) fn curve_series(
    name: String,
    y_range: [f32; 2],
    detail: bool,
    values: Vec<f32>,
) -> DisplaySeries {
    DisplaySeries {
        name,
        unit: "dB".to_owned(),
        x_unit: "Hz".to_owned(),
        x_range: [ANALYSER_MIN_HZ as f32, ANALYSER_MAX_HZ as f32],
        log_x: true,
        y_range,
        detail,
        values,
    }
}

/// The analyser's bands as a display's spectrum, each read at its centre.
pub(crate) fn spectrum_series(data: &[f32]) -> DisplaySeries {
    let centre = |band: f64| {
        (ANALYSER_MIN_HZ * (ANALYSER_MAX_HZ / ANALYSER_MIN_HZ).powf(band / ANALYSER_BANDS as f64))
            as f32
    };
    let mut spectrum = curve_series(
        "Spectrum".to_owned(),
        SPECTRUM_RANGE_DB,
        false,
        data.to_vec(),
    );
    spectrum.x_range = [centre(0.5), centre(ANALYSER_BANDS as f64 - 0.5)];
    spectrum
}

/// A biquad's coefficients, normalised so `a0` is 1.
#[derive(Clone, Copy, Default)]
pub(crate) struct Coeffs {
    pub(crate) b0: f32,
    pub(crate) b1: f32,
    pub(crate) b2: f32,
    pub(crate) a1: f32,
    pub(crate) a2: f32,
}

/// One second-order section: its coefficients and its state, transposed
/// direct form II.
#[derive(Clone, Copy, Default)]
pub(crate) struct Biquad {
    pub(crate) c: Coeffs,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// Its output for `x`, advancing its state.
    #[inline]
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = flush(self.c.b0 * x + self.z1);
        self.z1 = flush(self.c.b1 * x - self.c.a1 * y + self.z2);
        self.z2 = flush(self.c.b2 * x - self.c.a2 * y);
        y
    }
}

/// A value far below the audio's noise floor is left as zero rather than as
/// a denormal, whose arithmetic is slow.
pub(crate) fn flush(x: f32) -> f32 {
    if x.abs() < 1e-30 { 0.0 } else { x }
}

/// The spectrum of the output, for the display: a window of samples at a
/// time, at [`ANALYSER_BANDS`] logarithmic bands, in dB. Everything it
/// writes it made with, so it doesn't allocate on the audio thread.
pub(crate) struct Analyser {
    /// The samples of the window being filled.
    samples: Vec<f32>,
    filled: usize,
    /// The last complete window's bands, in dB.
    bands: [f32; ANALYSER_BANDS],
    /// The FFT's working buffers and its twiddles.
    re: Vec<f32>,
    im: Vec<f32>,
    twiddle: Vec<(f32, f32)>,
    /// The Hann window's weights.
    weights: Vec<f32>,
    /// Each band's first and last bin.
    edges: [usize; ANALYSER_BANDS + 1],
}

impl Analyser {
    pub(crate) fn new(rate: f64) -> Self {
        let half = ANALYSER_WINDOW / 2;
        let mut edges = [0usize; ANALYSER_BANDS + 1];
        for (band, edge) in edges.iter_mut().enumerate() {
            let ratio =
                (ANALYSER_MAX_HZ / ANALYSER_MIN_HZ).powf(band as f64 / ANALYSER_BANDS as f64);
            *edge = (((ANALYSER_MIN_HZ * ratio) * ANALYSER_WINDOW as f64 / rate).round() as usize)
                .min(half);
        }
        Self {
            samples: vec![0.0; ANALYSER_WINDOW],
            filled: 0,
            bands: [ANALYSER_FLOOR_DB; ANALYSER_BANDS],
            re: vec![0.0; ANALYSER_WINDOW],
            im: vec![0.0; ANALYSER_WINDOW],
            twiddle: (0..ANALYSER_WINDOW / 2)
                .map(|k| {
                    let angle = -TAU * k as f64 / ANALYSER_WINDOW as f64;
                    (angle.cos() as f32, angle.sin() as f32)
                })
                .collect(),
            weights: (0..ANALYSER_WINDOW)
                .map(|i| {
                    let x = i as f64 / ANALYSER_WINDOW as f64;
                    (0.5 - 0.5 * (TAU * x).cos()) as f32
                })
                .collect(),
            edges,
        }
    }

    /// Adds one output sample; a full window is measured and starts again.
    pub(crate) fn push(&mut self, sample: f32) {
        self.samples[self.filled] = sample;
        self.filled += 1;
        if self.filled == ANALYSER_WINDOW {
            self.filled = 0;
            self.measure();
        }
    }

    /// Measures the window into [`bands`](Self::bands).
    fn measure(&mut self) {
        let weights = &self.weights;
        for (i, (sample, weight)) in self.samples.iter().zip(weights).enumerate() {
            self.re[i] = sample * weight;
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im, &self.twiddle);
        // A one-sided spectrum's amplitude: twice the bin over the window's
        // weight, which a full-scale sine reads 0 dB at.
        let window: f32 = weights.iter().sum();
        let scale = 2.0 / window;
        for (band, value) in self.bands.iter_mut().enumerate() {
            // A band whose edges round to one bin reads that bin.
            let low = self.edges[band];
            let high = self.edges[band + 1].max(low + 1);
            let peak = (low..high)
                .map(|bin| self.re[bin].hypot(self.im[bin]))
                .fold(0.0f32, f32::max);
            let amplitude = peak * scale;
            *value = if amplitude > 1.0e-5 {
                20.0 * amplitude.log10()
            } else {
                ANALYSER_FLOOR_DB
            };
        }
    }

    /// Adds the stretch's output, both channels mixed to mono.
    pub(crate) fn push_mix(&mut self, audio: &mut Audio<'_, '_>) {
        if audio.channels() >= 2 {
            let mut iter = audio.channels_mut();
            let left = iter.next().expect("two channels");
            let right = iter.next().expect("two channels");
            for (l, r) in left.iter().zip(right.iter()) {
                self.push((l + r) * 0.5);
            }
        } else {
            for sample in audio.channel_mut(0) {
                self.push(*sample);
            }
        }
    }

    /// The last window's bands, in dB, [`ANALYSER_MIN_HZ`] to
    /// [`ANALYSER_MAX_HZ`], lowest first.
    pub(crate) fn bands(&self) -> &[f32; ANALYSER_BANDS] {
        &self.bands
    }
}

/// An in-place radix-2 FFT of `re` (with `im` zero for real input), the
/// twiddles precomputed.
pub(crate) fn fft(re: &mut [f32], im: &mut [f32], twiddle: &[(f32, f32)]) {
    let n = re.len();
    // The bit-reversal permutation.
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let step = n / len;
        let mut start = 0;
        while start < n {
            for k in 0..len / 2 {
                let (wr, wi) = twiddle[k * step];
                let (a, b) = (start + k, start + k + len / 2);
                let (xr, xi) = (re[a], im[a]);
                let (yr, yi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[a] = xr + yr;
                im[a] = xi + yi;
                re[b] = xr - yr;
                im[b] = xi - yi;
            }
            start += len;
        }
        len <<= 1;
    }
}

/// A stage's response at `hz`, in dB.
pub(crate) fn response_db(c: Coeffs, hz: f64, rate: f64) -> f64 {
    let w = TAU * hz / rate;
    let (s1, c1) = w.sin_cos();
    let (s2, c2) = (2.0 * w).sin_cos();
    let (b0, b1, b2) = (f64::from(c.b0), f64::from(c.b1), f64::from(c.b2));
    let (a1, a2) = (f64::from(c.a1), f64::from(c.a2));
    let num = (b0 + b1 * c1 + b2 * c2).powi(2) + (b1 * s1 + b2 * s2).powi(2);
    let den = (1.0 + a1 * c1 + a2 * c2).powi(2) + (a1 * s1 + a2 * s2).powi(2);
    10.0 * (num / den.max(1e-30)).max(1e-30).log10()
}
