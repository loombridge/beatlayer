//! Spectral processing (WP-8.28): a streaming short-time Fourier transform
//! with overlap-add resynthesis, the phase-vocoder helpers a spectral device
//! needs, and a cepstral envelope.
//!
//! An [`Stft`] takes samples one at a time or in slices and, every
//! [`hop`](Stft::hop) samples counted from its creation, windows the last
//! [`size`](Stft::size) samples, transforms them and hands the spectrum to a
//! closure, which may change it. The changed spectrum is transformed back,
//! windowed again and added into the output. Frames fall on the same samples
//! however the host cuts its blocks, so the output never depends on the block
//! size.
//!
//! Resynthesis is exact: the synthesis window is the analysis window divided
//! by the sum of the squared analysis windows of the frames that overlap each
//! sample, so a spectrum left alone gives back the input, delayed by
//! [`Stft::latency`], whatever the window and the overlap.
//!
//! Every buffer and every transform plan is made in [`Stft::new`], off the
//! audio thread; processing allocates nothing.

use std::f64::consts::{PI, TAU};
use std::sync::Arc;

pub use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// The smallest frame size.
pub const MIN_SIZE: usize = 256;
/// The largest frame size.
pub const MAX_SIZE: usize = 16_384;

/// An analysis window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// Raised cosine.
    Hann,
    /// Raised cosine on a pedestal.
    Hamming,
    /// Four terms, with the lowest side lobes of the three.
    BlackmanHarris,
}

impl Window {
    /// The window's value at `n` of `size` (the periodic form, for overlap-add).
    fn at(self, n: usize, size: usize) -> f64 {
        let x = TAU * n as f64 / size as f64;
        match self {
            Self::Hann => 0.5 - 0.5 * x.cos(),
            Self::Hamming => 0.54 - 0.46 * x.cos(),
            Self::BlackmanHarris => {
                0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()
            }
        }
    }
}

/// A streaming STFT with overlap-add resynthesis.
pub struct Stft {
    size: usize,
    hop: usize,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    analysis: Vec<f32>,
    /// The synthesis window, scaled by `1 / size` for the unnormalised
    /// inverse transform.
    synthesis: Vec<f32>,
    /// The last `size` samples in, a ring whose next write is its oldest.
    input: Vec<f32>,
    /// The output not yet emitted, indexed as `input` is.
    output: Vec<f32>,
    pos: usize,
    frame: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch_forward: Vec<Complex32>,
    scratch_inverse: Vec<Complex32>,
}

impl Stft {
    /// An STFT of frames of `size` samples (a power of two from [`MIN_SIZE`]
    /// to [`MAX_SIZE`]) that begin every `size / overlap` samples (`overlap`
    /// is 2, 4 or 8), or `None` for any other. Allocates.
    #[must_use]
    pub fn new(size: usize, overlap: usize, window: Window) -> Option<Self> {
        if !size.is_power_of_two()
            || !(MIN_SIZE..=MAX_SIZE).contains(&size)
            || !matches!(overlap, 2 | 4 | 8)
        {
            return None;
        }
        let hop = size / overlap;
        let w: Vec<f64> = (0..size).map(|n| window.at(n, size)).collect();
        // The squared windows of the frames over each sample, which repeat
        // every hop.
        let covered: Vec<f64> = (0..hop)
            .map(|r| (0..overlap).map(|j| w[r + j * hop].powi(2)).sum())
            .collect();
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        Some(Self {
            size,
            hop,
            analysis: w.iter().map(|&v| v as f32).collect(),
            synthesis: w
                .iter()
                .enumerate()
                .map(|(n, &v)| (v / covered[n % hop] / size as f64) as f32)
                .collect(),
            input: vec![0.0; size],
            output: vec![0.0; size],
            pos: 0,
            frame: forward.make_input_vec(),
            spectrum: forward.make_output_vec(),
            scratch_forward: forward.make_scratch_vec(),
            scratch_inverse: inverse.make_scratch_vec(),
            forward,
            inverse,
        })
    }

    /// The frame size.
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// The samples between two frames.
    #[must_use]
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// The bins in a spectrum: `size / 2 + 1`, from 0 Hz to Nyquist.
    #[must_use]
    pub fn bins(&self) -> usize {
        self.size / 2 + 1
    }

    /// How many samples the output lags the input by, which a node reports
    /// for delay compensation. A sample is complete only once the last frame
    /// over it has ended, and the last frame over a sample can end `size - 1`
    /// samples after it.
    #[must_use]
    pub fn latency(&self) -> usize {
        self.size - 1
    }

    /// What to multiply a bin's magnitude by to read the peak amplitude of a
    /// sine that sits on it, whatever the window.
    #[must_use]
    pub fn amplitude_scale(&self) -> f32 {
        2.0 / self.analysis.iter().sum::<f32>()
    }

    /// Clears the history, as after a jump in the song; the frames count from
    /// here.
    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.pos = 0;
    }

    /// Takes one sample and gives the output sample, [`latency`](Self::latency)
    /// samples late. When the sample ends a frame, `spectrum` is called with
    /// the frame's bins to read or change.
    pub fn process_sample(
        &mut self,
        sample: f32,
        mut spectrum: impl FnMut(&mut [Complex32]),
    ) -> f32 {
        self.input[self.pos] = sample;
        self.pos += 1;
        if self.pos == self.size {
            self.pos = 0;
        }
        // The ring's next write is its oldest sample, the frame's first.
        if self.pos.is_multiple_of(self.hop) {
            self.frame_through(&mut spectrum);
        }
        std::mem::take(&mut self.output[self.pos])
    }

    /// As [`process_sample`](Self::process_sample) for each sample of `input`,
    /// into `output`.
    ///
    /// # Panics
    ///
    /// If the slices differ in length.
    pub fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        mut spectrum: impl FnMut(&mut [Complex32]),
    ) {
        assert_eq!(input.len(), output.len(), "input and output must match");
        for (out, &sample) in output.iter_mut().zip(input) {
            *out = self.process_sample(sample, &mut spectrum);
        }
    }

    fn frame_through(&mut self, spectrum: &mut impl FnMut(&mut [Complex32])) {
        let (older, newer) = self.input.split_at(self.pos);
        let samples = newer.iter().chain(older);
        for ((frame, &sample), &window) in self.frame.iter_mut().zip(samples).zip(&self.analysis) {
            *frame = sample * window;
        }
        // The buffers are made to the plan's size, which is the only way
        // these can fail.
        let _ = self.forward.process_with_scratch(
            &mut self.frame,
            &mut self.spectrum,
            &mut self.scratch_forward,
        );
        spectrum(&mut self.spectrum);
        // A real signal's DC and Nyquist bins have no imaginary part.
        self.spectrum[0].im = 0.0;
        self.spectrum[self.size / 2].im = 0.0;
        let _ = self.inverse.process_with_scratch(
            &mut self.spectrum,
            &mut self.frame,
            &mut self.scratch_inverse,
        );
        let (head, tail) = self.output.split_at_mut(self.pos);
        let slots = tail.iter_mut().chain(head);
        for ((slot, &value), &window) in slots.zip(&self.frame).zip(&self.synthesis) {
            *slot += value * window;
        }
    }
}

impl std::fmt::Debug for Stft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stft")
            .field("size", &self.size)
            .field("hop", &self.hop)
            .finish_non_exhaustive()
    }
}

/// The frequency of `bin` in a frame of `size` samples at `rate` Hz.
#[must_use]
pub fn bin_to_hz(bin: f32, size: usize, rate: f64) -> f32 {
    (f64::from(bin) * rate / size as f64) as f32
}

/// The (fractional) bin of `hz` in a frame of `size` samples at `rate` Hz.
#[must_use]
pub fn hz_to_bin(hz: f32, size: usize, rate: f64) -> f32 {
    (f64::from(hz) * size as f64 / rate) as f32
}

/// The bins a row of a display covers, for each of `rows` rows from `low` Hz
/// to `high` Hz at the middle of the first and last, spaced logarithmically
/// or linearly: the first bin and the last (inclusive). A row narrower than
/// a bin takes the bin it falls in.
#[must_use]
pub fn row_bins(
    rows: usize,
    low: f32,
    high: f32,
    log: bool,
    size: usize,
    rate: f64,
) -> Vec<(usize, usize)> {
    let last = size / 2;
    let centre = |t: f32| {
        if log {
            low * (high / low).powf(t)
        } else {
            low + (high - low) * t
        }
    };
    let at = |t: f32| hz_to_bin(centre(t), size, rate).clamp(0.0, last as f32);
    let steps = rows.saturating_sub(1).max(1) as f32;
    (0..rows)
        .map(|r| {
            let t = r as f32 / steps;
            // Halfway to each neighbour, so the rows tile the axis.
            let from = at(t - 0.5 / steps);
            let to = at(t + 0.5 / steps);
            (
                from.round() as usize,
                (to.round() as usize).max(from.round() as usize),
            )
        })
        .collect()
}

/// Each bin's magnitude, into `out`.
pub fn magnitudes(spectrum: &[Complex32], out: &mut [f32]) {
    for (out, bin) in out.iter_mut().zip(spectrum) {
        *out = bin.norm();
    }
}

/// Each bin's phase in radians, from −π to π, into `out`.
pub fn phases(spectrum: &[Complex32], out: &mut [f32]) {
    for (out, bin) in out.iter_mut().zip(spectrum) {
        *out = bin.arg();
    }
}

/// `phase` wrapped into [−π, π).
#[must_use]
pub fn wrap_phase(phase: f64) -> f64 {
    phase - TAU * ((phase + PI) / TAU).floor()
}

/// `phase` moved by whole turns to the value nearest `previous`, so a run of
/// wrapped phases becomes a continuous one.
#[must_use]
pub fn unwrap_phase(phase: f32, previous: f32) -> f32 {
    (f64::from(previous) + wrap_phase(f64::from(phase) - f64::from(previous))) as f32
}

/// The frequency in Hz of the partial in `bin`, from its `phase` and the
/// `previous` frame's, `hop` samples earlier in a frame of `size` samples at
/// `rate` Hz: the phase vocoder's instantaneous frequency.
#[must_use]
pub fn instantaneous_frequency(
    phase: f32,
    previous: f32,
    bin: usize,
    size: usize,
    hop: usize,
    rate: f64,
) -> f32 {
    // The turn the bin's own centre makes over a hop; what the phase moves
    // beyond it is the partial's distance from the centre.
    let expected = TAU * bin as f64 * hop as f64 / size as f64;
    let deviation = wrap_phase(f64::from(phase) - f64::from(previous) - expected);
    ((bin as f64 + deviation * size as f64 / (TAU * hop as f64)) * rate / size as f64) as f32
}

/// `cepstrum` (of a real, even spectrum, so even itself) scaled by `1 / len`
/// and by a raised-cosine lifter that is 1 at quefrency 0 and 0 from `cutoff`.
pub fn lifter(cepstrum: &mut [f32], cutoff: usize) {
    let len = cepstrum.len();
    let scale = 1.0 / len as f32;
    for (n, value) in cepstrum.iter_mut().enumerate() {
        let quefrency = n.min(len - n);
        *value *= if quefrency < cutoff {
            scale * (0.5 + 0.5 * (std::f32::consts::PI * quefrency as f32 / cutoff as f32).cos())
        } else {
            0.0
        };
    }
}

/// The smooth outline of a spectrum, made by cepstral liftering: the log
/// amplitude of the bins, with the detail finer than a quefrency removed.
pub struct CepstralEnvelope {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    cepstrum: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch_forward: Vec<Complex32>,
    scratch_inverse: Vec<Complex32>,
}

/// The quietest a bin's power may be, against the loudest in its frame:
/// without it the valleys between harmonics, which no formant lives in, would
/// weigh on the envelope.
const FLOOR: f32 = 1e-6;

impl CepstralEnvelope {
    /// An envelope for spectra of a frame of `size` samples. Allocates.
    #[must_use]
    pub fn new(size: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        Self {
            cepstrum: forward.make_input_vec(),
            spectrum: forward.make_output_vec(),
            scratch_forward: forward.make_scratch_vec(),
            scratch_inverse: inverse.make_scratch_vec(),
            forward,
            inverse,
        }
    }

    /// The natural log of the amplitude of `spectrum` ([`Stft::bins`] bins),
    /// smoothed to `cutoff` samples of quefrency (the period of the finest
    /// detail it keeps), into `out`.
    pub fn smooth(&mut self, spectrum: &[Complex32], cutoff: usize, out: &mut [f32]) {
        let loudest = spectrum.iter().map(|b| b.norm_sqr()).fold(0.0, f32::max);
        let floor = loudest * FLOOR + 1e-30;
        for (log, bin) in self.spectrum.iter_mut().zip(spectrum) {
            *log = Complex32::new(0.5 * (bin.norm_sqr() + floor).ln(), 0.0);
        }
        let last = self.spectrum.len() - 1;
        self.spectrum[0].im = 0.0;
        self.spectrum[last].im = 0.0;
        let _ = self.inverse.process_with_scratch(
            &mut self.spectrum,
            &mut self.cepstrum,
            &mut self.scratch_inverse,
        );
        lifter(&mut self.cepstrum, cutoff);
        let _ = self.forward.process_with_scratch(
            &mut self.cepstrum,
            &mut self.spectrum,
            &mut self.scratch_forward,
        );
        for (out, bin) in out.iter_mut().zip(&self.spectrum) {
            *out = bin.re;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: [Window; 3] = [Window::Hann, Window::Hamming, Window::BlackmanHarris];
    const SIZES: [usize; 7] = [256, 512, 1024, 2048, 4096, 8192, 16384];

    fn noise(seed: u64, len: usize) -> Vec<f32> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ((state >> 40) as f32 / (1u64 << 23) as f32) - 1.0
            })
            .collect()
    }

    #[test]
    fn analysis_then_resynthesis_returns_the_input_delayed_by_the_latency() {
        for size in SIZES {
            for overlap in [2, 4, 8] {
                for window in WINDOWS {
                    let mut stft = Stft::new(size, overlap, window).unwrap();
                    let input = noise(size as u64, 3 * size);
                    let mut output = vec![0.0; input.len()];
                    stft.process(&input, &mut output, |_| {});
                    let latency = stft.latency();
                    let (mut error, mut power) = (0.0f64, 0.0f64);
                    for (t, &out) in output.iter().enumerate() {
                        let expected = t.checked_sub(latency).map_or(0.0, |m| input[m]);
                        error += f64::from(out - expected).powi(2);
                        power += f64::from(expected).powi(2);
                    }
                    let db = 10.0 * (error / power).log10();
                    assert!(db < -100.0, "{size} {overlap} {window:?}: {db:.1} dB");
                }
            }
        }
    }

    /// Scales each bin by a ramp, so the closure's frames show in the output.
    fn tilt(spectrum: &mut [Complex32]) {
        let top = spectrum.len() as f32;
        for (k, bin) in spectrum.iter_mut().enumerate() {
            *bin *= 1.0 - k as f32 / top;
        }
    }

    fn render(input: &[f32], block: usize) -> Vec<f32> {
        let mut stft = Stft::new(1024, 4, Window::Hann).unwrap();
        let mut output = vec![0.0; input.len()];
        for (i, o) in input.chunks(block).zip(output.chunks_mut(block)) {
            stft.process(i, o, tilt);
        }
        output
    }

    #[test]
    fn renders_are_identical_to_the_bit_at_any_block_size() {
        let input = noise(7, 10_000);
        let bits = |v: Vec<f32>| v.iter().map(|s| s.to_bits()).collect::<Vec<_>>();
        let one = bits(render(&input, 1));
        for block in [64, 511, 4096] {
            assert_eq!(one, bits(render(&input, block)), "block {block}");
        }
    }

    proptest::proptest! {
        #[test]
        fn frames_land_on_the_same_hops_whatever_the_slicing(
            cuts in proptest::collection::vec(1usize..700, 1..40),
        ) {
            let input = noise(3, 6_000);
            let mut stft = Stft::new(256, 4, Window::Hann).unwrap();
            let mut seen = 0usize;
            let (mut start, mut cut) = (0, cuts.iter().cycle());
            while start < input.len() {
                let end = (start + cut.next().unwrap()).min(input.len());
                let mut out = vec![0.0; end - start];
                let before = seen;
                stft.process(&input[start..end], &mut out, |_| seen += 1);
                // Each frame ends on a sample whose index, plus one, is a
                // multiple of the hop.
                let hop = stft.hop();
                let expected = (end / hop) - (start / hop);
                proptest::prop_assert_eq!(seen - before, expected);
                start = end;
            }
            proptest::prop_assert_eq!(seen, input.len() / stft.hop());
        }
    }

    #[test]
    fn a_frame_ends_on_every_hop_counted_from_creation() {
        let mut stft = Stft::new(256, 2, Window::Hann).unwrap();
        let mut ends = Vec::new();
        for n in 0..400 {
            let mut hit = false;
            stft.process_sample(0.0, |_| hit = true);
            if hit {
                ends.push(n);
            }
        }
        assert_eq!(ends, [127, 255, 383]);
    }

    #[test]
    fn invalid_shapes_are_refused() {
        for (size, overlap) in [(128, 4), (300, 4), (32_768, 4), (1024, 3), (1024, 1)] {
            assert!(Stft::new(size, overlap, Window::Hann).is_none());
        }
    }

    #[test]
    fn the_instantaneous_frequency_of_a_steady_sine_is_within_a_tenth_of_a_hertz() {
        let (size, rate) = (2048, 48_000.0);
        for hz in [440.0f64, 1_000.0, 3_217.5, 12_345.6] {
            let mut stft = Stft::new(size, 4, Window::Hann).unwrap();
            let hop = stft.hop();
            let mut phases = Vec::new();
            for n in 0..size * 4 {
                let x = (TAU * hz * n as f64 / rate).sin() as f32;
                stft.process_sample(x, |spectrum| {
                    let (bin, _) = spectrum
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
                        .unwrap();
                    phases.push((bin, spectrum[bin].arg()));
                });
            }
            // A full frame of the sine on each side.
            for pair in phases[size / hop..].windows(2) {
                let ((bin, previous), (_, phase)) = (pair[0], pair[1]);
                let found = instantaneous_frequency(phase, previous, bin, size, hop, rate);
                assert!((f64::from(found) - hz).abs() < 0.1, "{hz}: {found}");
            }
        }
    }

    #[test]
    fn rows_tile_the_axis_and_a_narrow_row_takes_the_bin_it_falls_in() {
        let rows = row_bins(64, 20.0, 20_000.0, true, 4096, 48_000.0);
        assert_eq!(rows.len(), 64);
        assert!(rows.iter().all(|&(a, b)| a <= b && b <= 2048));
        // Rising, and the lowest rows share a bin.
        assert!(
            rows.windows(2)
                .all(|w| w[0].0 <= w[1].0 && w[0].1 <= w[1].1)
        );
        assert_eq!(rows[0], rows[1]);
        // A tone on bin 100 lands in a row that covers it.
        let hz = bin_to_hz(100.0, 4096, 48_000.0);
        assert!(rows.iter().any(|&(a, b)| a <= 100 && 100 <= b), "{hz}");
        let scale = Stft::new(1024, 4, Window::Hann).unwrap().amplitude_scale();
        assert!((scale - 2.0 / 512.0).abs() < 1e-6);
    }

    #[test]
    fn phases_wrap_and_unwrap_and_bins_are_hertz() {
        assert!((wrap_phase(2.5 * PI) - 0.5 * PI).abs() < 1e-12);
        assert!((wrap_phase(-3.5 * PI) - 0.5 * PI).abs() < 1e-12);
        assert!((unwrap_phase(-3.0, 3.0) - (TAU as f32 - 3.0)).abs() < 1e-5);
        assert_eq!(bin_to_hz(10.0, 1024, 48_000.0), 468.75);
        assert_eq!(hz_to_bin(468.75, 1024, 48_000.0), 10.0);
    }

    #[test]
    fn the_cepstral_envelope_keeps_the_outline_and_loses_the_ripple() {
        let size = 1024;
        let mut envelope = CepstralEnvelope::new(size);
        // Amplitude falling across the spectrum, with a ripple every other bin.
        let spectrum: Vec<Complex32> = (0..=size / 2)
            .map(|k| {
                let ripple = if k % 2 == 0 { 1.5 } else { 0.5 };
                Complex32::new((-0.002 * k as f32).exp() * ripple, 0.0)
            })
            .collect();
        let mut out = vec![0.0; size / 2 + 1];
        envelope.smooth(&spectrum, 64, &mut out);
        let mean_ripple = 0.5 * (1.5f32.ln() + 0.5f32.ln());
        for (k, &value) in out.iter().enumerate().take(400).skip(100) {
            let outline = -0.002 * k as f32 + mean_ripple;
            assert!((value - outline).abs() < 0.02, "{k}: {value} vs {outline}");
        }
    }
}
