//! HTDemucs's short-time Fourier transform and its inverse, which the split
//! model leaves out of its graph (stem separation spec R32).
//!
//! Both are PyTorch's `stft` and `istft` as Demucs calls them (a periodic
//! Hann window, `normalized`, `center`), wrapped in Demucs's own padding
//! (`HTDemucs._spec` and `_ispec`): the mix is reflect-padded so that the
//! spectrogram has exactly one frame per hop. Every setting comes from the
//! model's metadata ([`Settings::from_metadata`]), none from a constant.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// The prefix of the keys the export script writes into the model.
const KEY: &str = "beatlayer.stft.";

/// What the model says about its transform and its window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The window of audio the network takes, in frames.
    pub window_frames: usize,
    /// The FFT size.
    pub nfft: usize,
    /// The hop between frames.
    pub hop: usize,
}

impl Settings {
    /// Reads the settings with `get`, which looks a metadata key up.
    ///
    /// # Errors
    ///
    /// A message if a setting is missing, or isn't one this transform
    /// computes (a window other than Hann, an un-normalised transform, a
    /// rate other than 44.1 kHz, anything but stereo and Demucs's four
    /// stems in order).
    pub fn from_metadata(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let text = |name: &str| {
            get(&format!("{KEY}{name}"))
                .ok_or_else(|| format!("the model doesn't say its transform's `{name}`"))
        };
        let number = |name: &str| {
            text(name)?
                .parse::<usize>()
                .map_err(|_| format!("the model's `{name}` isn't a whole number"))
        };
        let expect = |name: &str, want: &str| match text(name)? {
            got if got == want => Ok(()),
            got => Err(format!(
                "the model's transform has {name} `{got}`, not `{want}`"
            )),
        };
        expect("window", "hann")?;
        expect("normalized", "true")?;
        expect("center_pad", "reflect")?;
        expect("channels", "2")?;
        expect("samplerate", &super::RATE.to_string())?;
        expect("sources", "drums,bass,other,vocals")?;
        let settings = Self {
            window_frames: number("window_frames")?,
            nfft: number("nfft")?,
            hop: number("hop")?,
        };
        if settings.nfft < 8 || settings.hop * 4 != settings.nfft || settings.window_frames == 0 {
            return Err("the model's transform sizes don't fit Demucs's padding".to_owned());
        }
        Ok(settings)
    }
}

/// The transform, planned once for a model.
#[derive(Clone)]
pub struct Transform {
    settings: Settings,
    window: Vec<f32>,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
}

impl Transform {
    /// Plans the transform `settings` describe.
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        let n = settings.nfft;
        let mut planner = RealFftPlanner::<f32>::new();
        Self {
            window: (0..n)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
                .collect(),
            forward: planner.plan_fft_forward(n),
            inverse: planner.plan_fft_inverse(n),
            settings,
        }
    }

    /// The window of audio the network takes, in frames.
    #[must_use]
    pub fn window_frames(&self) -> usize {
        self.settings.window_frames
    }

    /// The bins of a spectrogram: the FFT's, less the Nyquist one, which
    /// Demucs drops.
    #[must_use]
    pub fn bins(&self) -> usize {
        self.settings.nfft / 2
    }

    /// The frames of a spectrogram of `length` audio frames.
    #[must_use]
    pub fn frames(&self, length: usize) -> usize {
        length.div_ceil(self.settings.hop)
    }

    /// The spectrogram the network's frequency branch takes: `mix` is
    /// planar stereo (the left channel, then the right) of `length` frames,
    /// and the result is `[4, bins, frames]`, each channel's real part and
    /// then its imaginary part.
    #[must_use]
    pub fn spectrogram(&self, mix: &[f32], length: usize) -> Vec<f32> {
        let Settings { nfft, hop, .. } = self.settings;
        let (bins, frames) = (self.bins(), self.frames(length));
        let pad = hop / 2 * 3;
        let scale = (nfft as f32).sqrt().recip();
        let mut out = vec![0.0; 4 * bins * frames];
        let mut input = self.forward.make_input_vec();
        let mut output = self.forward.make_output_vec();
        let mut scratch = self.forward.make_scratch_vec();
        for channel in 0..2 {
            let padded = reflect_pad(
                &mix[channel * length..(channel + 1) * length],
                pad,
                pad + frames * hop - length,
            );
            for frame in 0..frames {
                for ((slot, x), w) in input
                    .iter_mut()
                    .zip(&padded[frame * hop..])
                    .zip(&self.window)
                {
                    *slot = x * w;
                }
                self.forward
                    .process_with_scratch(&mut input, &mut output, &mut scratch)
                    .expect("the buffers are the plan's own");
                for (bin, value) in output[..bins].iter().enumerate() {
                    out[((channel * 2) * bins + bin) * frames + frame] = value.re * scale;
                    out[((channel * 2 + 1) * bins + bin) * frames + frame] = value.im * scale;
                }
            }
        }
        out
    }

    /// The waveforms the frequency branch's output spectrograms make:
    /// `spectra` is `[sources, 4, bins, frames]` as the network gives it,
    /// and the result is planar stereo for each source, `length` frames
    /// each.
    #[must_use]
    pub fn waveforms(&self, spectra: &[f32], length: usize) -> Vec<f32> {
        let Settings { nfft, hop, .. } = self.settings;
        let (bins, frames) = (self.bins(), self.frames(length));
        let per_source = 4 * bins * frames;
        let sources = spectra.len() / per_source;
        // Two silent frames before and after, which Demucs pads in.
        let total = frames + 4;
        let span = nfft + (total - 1) * hop;
        let scale = (nfft as f32).sqrt().recip();
        // The windows' squares, summed where the frames overlap: what the
        // inverse divides by.
        let mut envelope = vec![0.0f32; span];
        for frame in 0..total {
            for (e, w) in envelope[frame * hop..].iter_mut().zip(&self.window) {
                *e += w * w;
            }
        }
        let from = nfft / 2 + hop / 2 * 3;
        let mut out = vec![0.0; sources * 2 * length];
        let mut input = self.inverse.make_input_vec();
        let mut output = self.inverse.make_output_vec();
        let mut scratch = self.inverse.make_scratch_vec();
        let mut sum = vec![0.0f32; span];
        for source in 0..sources {
            for channel in 0..2 {
                sum.fill(0.0);
                for frame in 0..frames {
                    for bin in 0..bins {
                        let at = |part: usize| {
                            spectra[source * per_source
                                + ((channel * 2 + part) * bins + bin) * frames
                                + frame]
                        };
                        input[bin] = Complex::new(at(0), at(1));
                    }
                    input[bins] = Complex::new(0.0, 0.0);
                    // A real signal's zero frequency has no imaginary part.
                    input[0].im = 0.0;
                    self.inverse
                        .process_with_scratch(&mut input, &mut output, &mut scratch)
                        .expect("the buffers are the plan's own");
                    // Two silent frames came first.
                    let start = (frame + 2) * hop;
                    for ((s, x), w) in sum[start..].iter_mut().zip(&output).zip(&self.window) {
                        *s += x * w * scale;
                    }
                }
                let target = &mut out[(source * 2 + channel) * length..][..length];
                for (i, value) in target.iter_mut().enumerate() {
                    *value = sum[from + i] / envelope[from + i];
                }
            }
        }
        out
    }
}

/// `signal` with `before` and `after` frames reflected in at its ends, as
/// PyTorch's `reflect` padding does (the edge sample isn't repeated).
fn reflect_pad(signal: &[f32], before: usize, after: usize) -> Vec<f32> {
    let n = signal.len();
    let mut out = Vec::with_capacity(before + n + after);
    out.extend((1..=before).rev().map(|i| signal[reflect(i as isize, n)]));
    out.extend_from_slice(signal);
    out.extend((1..=after).map(|i| signal[reflect((n - 1 + i) as isize, n)]));
    out
}

/// The index `i` reflects to in a signal of `n` samples.
fn reflect(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let period = 2 * (n as isize - 1);
    let i = i.rem_euclid(period);
    (if i < n as isize { i } else { period - i }) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(nfft: usize, window_frames: usize) -> Settings {
        Settings {
            window_frames,
            nfft,
            hop: nfft / 4,
        }
    }

    fn mix(length: usize) -> Vec<f32> {
        (0..2 * length)
            .map(|i| (i as f32 * 0.07).sin() * 0.4 + (i as f32 * 0.011).cos() * 0.2)
            .collect()
    }

    // Spec: stem-separation R32. What the forward transform makes, the
    // inverse turns back into the mix, whatever the length.
    #[test]
    fn the_inverse_returns_the_mix_the_transform_took() {
        for length in [1000, 1024, 1500, 4097] {
            let transform = Transform::new(settings(64, length));
            let mix = mix(length);
            let spec = transform.spectrogram(&mix, length);
            assert_eq!(spec.len(), 4 * transform.bins() * transform.frames(length));
            let back = transform.waveforms(&spec, length);
            assert_eq!(back.len(), 2 * length);
            // Demucs's inverse leaves the first and last frames' share of the
            // edges out, as PyTorch's does, so the edges aren't compared.
            let edge = 3 * transform.settings.hop;
            for channel in 0..2 {
                for i in edge..length - edge {
                    let (got, want) = (back[channel * length + i], mix[channel * length + i]);
                    assert!(
                        (got - want).abs() < 1e-4,
                        "{length} {channel} {i}: {got} {want}"
                    );
                }
            }
        }
    }

    // Spec: stem-separation R32. PyTorch's reflect padding, which Demucs
    // pads its mix with.
    #[test]
    fn reflect_padding_does_not_repeat_the_edge() {
        let padded = reflect_pad(&[1.0, 2.0, 3.0, 4.0], 2, 3);
        assert_eq!(padded, [3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0]);
    }

    // Spec: stem-separation R32. A sine on a bin lands on it: the
    // transform's scaling and layout are the network's.
    #[test]
    fn a_sine_on_a_bin_fills_that_bin() {
        let (nfft, length) = (64, 640);
        let transform = Transform::new(settings(nfft, length));
        let bin = 5;
        let mut mix = vec![0.0; 2 * length];
        for (i, x) in mix[..length].iter_mut().enumerate() {
            *x = (std::f32::consts::TAU * bin as f32 * i as f32 / nfft as f32).cos();
        }
        let spec = transform.spectrogram(&mix, length);
        let (bins, frames) = (transform.bins(), transform.frames(length));
        let magnitude = |bin: usize| {
            let at = |part: usize| spec[(part * bins + bin) * frames + frames / 2];
            at(0).hypot(at(1))
        };
        // A cosine's half amplitude, times the Hann window's sum and the
        // transform's 1/sqrt(nfft).
        let want = 0.5 * (nfft as f32 / 2.0) / (nfft as f32).sqrt();
        assert!((magnitude(bin) - want).abs() < 1e-3, "{}", magnitude(bin));
        assert!(magnitude(bin + 9) < 1e-3);
    }

    #[test]
    fn settings_come_from_the_model_and_are_checked() {
        let all = |overrides: &[(&str, &str)]| {
            let mut keys: std::collections::HashMap<String, String> = [
                ("window", "hann"),
                ("normalized", "true"),
                ("center_pad", "reflect"),
                ("channels", "2"),
                ("samplerate", "44100"),
                ("sources", "drums,bass,other,vocals"),
                ("window_frames", "343980"),
                ("nfft", "4096"),
                ("hop", "1024"),
            ]
            .map(|(k, v)| (format!("{KEY}{k}"), v.to_owned()))
            .into();
            for (k, v) in overrides {
                keys.insert(format!("{KEY}{k}"), (*v).to_owned());
            }
            move |key: &str| keys.get(key).cloned()
        };
        let settings = Settings::from_metadata(all(&[])).unwrap();
        assert_eq!(settings.window_frames, 343_980);
        assert_eq!((settings.nfft, settings.hop), (4096, 1024));
        let bad = Settings::from_metadata(all(&[("window", "kaiser")])).unwrap_err();
        assert!(bad.contains("window"), "{bad}");
        assert!(
            Settings::from_metadata(|_| None)
                .unwrap_err()
                .contains("doesn't say")
        );
    }
}
