//! Short-time Fourier transforms of a mono signal, frame by frame, with a
//! periodic Hann window.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// The FFT size nearest `seconds` at `rate`, as a power of two from 256
/// to 65,536.
pub(crate) fn size_for(seconds: f64, rate: u32) -> usize {
    let target = seconds * f64::from(rate);
    let exponent = target.max(1.0).log2().round().clamp(8.0, 16.0);
    1 << exponent as u32
}

/// A periodic Hann window of `size` points.
pub(crate) fn hann(size: usize) -> Vec<f64> {
    (0..size)
        .map(|n| 0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / size as f64).cos())
        .collect()
}

/// Windowed FFTs of one signal, reusing their buffers.
pub(crate) struct Stft {
    fft: Arc<dyn RealToComplex<f64>>,
    window: Vec<f64>,
    input: Vec<f64>,
    output: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl Stft {
    /// Transforms of `size` points (a power of two).
    pub(crate) fn new(size: usize) -> Self {
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(size);
        Self {
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            window: hann(size),
            fft,
        }
    }

    /// The transform's size.
    pub(crate) fn size(&self) -> usize {
        self.window.len()
    }

    /// The window's sum of squares, which scales a power spectrum so its
    /// bins sum to the frame's mean square.
    pub(crate) fn window_power(&self) -> f64 {
        self.window.iter().map(|w| w * w).sum()
    }

    /// The window's sum, which scales a magnitude spectrum so a full-scale
    /// sine on a bin reads 1.
    pub(crate) fn window_sum(&self) -> f64 {
        self.window.iter().sum()
    }

    /// Transforms the frame of `signal` that starts at `start` (which may
    /// be before 0 or run past the end: the signal is silent there), and
    /// returns its bins, from 0 Hz to the Nyquist frequency.
    pub(crate) fn frame(&mut self, signal: &[f64], start: isize) -> &[Complex<f64>] {
        for (n, (slot, w)) in self.input.iter_mut().zip(&self.window).enumerate() {
            let at = start + n as isize;
            let sample = usize::try_from(at)
                .ok()
                .and_then(|at| signal.get(at))
                .copied()
                .unwrap_or(0.0);
            *slot = sample * w;
        }
        self.fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .expect("the buffers are the plan's own");
        &self.output
    }
}
