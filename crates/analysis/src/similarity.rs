//! A sound's descriptor, for finding sounds that sound alike (spec:
//! sound-similarity R2, R3).
//!
//! The descriptor is Beatlayer's own vector of [`LEN`] measurements of a
//! sound, computed from its audio summed to mono at [`RATE`] and scaled so
//! its peak is 1 (a louder copy describes the same):
//!
//! - the means and standard deviations of 40 log-mel band energies and of
//!   20 mel-frequency cepstral coefficients, over Hann-windowed frames of
//!   1,024 samples every 512;
//! - the means of the spectral centroid (as a share of the Nyquist
//!   frequency), flatness (geometric over arithmetic mean of the power) and
//!   flux (the mean rise of the log-mel energies since the frame before),
//!   each followed by its mean in eight equal parts of the sound;
//! - the onset density (onsets a second, from [`crate::onsets`]), the
//!   length's natural logarithm, and how pitched the sound is (the highest
//!   normalised autocorrelation between 60 Hz and 2 kHz, averaged over up
//!   to twelve frames).
//!
//! "The sound" is the span from the first frame to the last whose mean
//! square is above −60 dB of the peak. The raw vectors are what is stored;
//! a [`Normaliser`] fitted to the analysed material scales each
//! measurement before two sounds' [`distance`] (the cosine distance) is
//! taken.

use bl_media::SampleData;

use crate::onsets;
use crate::stft::Stft;

/// The rate the descriptor is computed at.
pub const RATE: u32 = 22_050;

/// The longest sound, in seconds, that has a descriptor (R1).
pub const LONGEST_SECONDS: f64 = 60.0;

/// The descriptor's version. A stored descriptor of another version is
/// computed again.
pub const VERSION: u32 = 1;

const FFT: usize = 1_024;
const HOP: usize = 512;
const MELS: usize = 40;
const MFCCS: usize = 20;
const PARTS: usize = 8;
/// A frame whose mean square is below this (−60 dB of a peak of 1) is
/// outside the sound.
const QUIET: f64 = 1e-6;

const MEL_MEAN: usize = 0;
const MEL_STD: usize = MEL_MEAN + MELS;
const MFCC_MEAN: usize = MEL_STD + MELS;
const MFCC_STD: usize = MFCC_MEAN + MFCCS;
const CENTROID: usize = MFCC_STD + MFCCS;
const FLATNESS: usize = CENTROID + 1 + PARTS;
const FLUX: usize = FLATNESS + 1 + PARTS;
const ONSETS: usize = FLUX + 1 + PARTS;
const LENGTH: usize = ONSETS + 1;
const PITCHED: usize = LENGTH + 1;

/// How many numbers a descriptor holds.
pub const LEN: usize = PITCHED + 1;

/// A sound's measurements, as [`LEN`] numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct Descriptor(Vec<f32>);

impl Descriptor {
    /// The measurements.
    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.0
    }

    /// The descriptor as little-endian `f32`s, for the database.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// Reads a descriptor [`Descriptor::to_bytes`] wrote; `None` if the
    /// bytes aren't [`LEN`] finite numbers.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != LEN * 4 {
            return None;
        }
        let values: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        values.iter().all(|v| v.is_finite()).then_some(Self(values))
    }

    /// The descriptor of decoded audio: converted to [`RATE`], summed to
    /// mono and measured. Audio longer than [`LONGEST_SECONDS`] has none.
    ///
    /// # Errors
    ///
    /// A message if the audio is empty, too long, or can't be converted.
    pub fn of_sample(data: &SampleData) -> Result<Self, String> {
        if data.frames() == 0 {
            return Err("the audio is empty".to_owned());
        }
        if data.duration_seconds() > LONGEST_SECONDS {
            return Err(format!(
                "the audio is longer than {LONGEST_SECONDS} seconds"
            ));
        }
        let converted = bl_media::convert_sample_rate(data, RATE).map_err(|e| e.to_string())?;
        let scale = 1.0 / converted.channel_count().max(1) as f32;
        let mono: Vec<f32> = (0..converted.frames())
            .map(|i| converted.channels().map(|c| c[i]).sum::<f32>() * scale)
            .collect();
        Ok(Self::of_mono(&mono))
    }

    /// The descriptor of `mono`, at [`RATE`].
    #[must_use]
    pub fn of_mono(mono: &[f32]) -> Self {
        let peak = mono.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        let gain = if peak > 0.0 {
            1.0 / f64::from(peak)
        } else {
            1.0
        };
        let signal: Vec<f64> = mono.iter().map(|&s| f64::from(s) * gain).collect();
        let mut out = vec![0.0f32; LEN];
        let seconds = signal.len() as f64 / f64::from(RATE);
        out[LENGTH] = seconds.max(0.001).ln() as f32;

        let frames = signal.len().div_ceil(HOP).max(1);
        let loud: Vec<bool> = (0..frames)
            .map(|t| {
                let span = &signal[(t * HOP).min(signal.len())..((t + 1) * HOP).min(signal.len())];
                !span.is_empty()
                    && span.iter().map(|s| s * s).sum::<f64>() / span.len() as f64 > QUIET
            })
            .collect();
        let (Some(first), Some(last)) =
            (loud.iter().position(|&l| l), loud.iter().rposition(|&l| l))
        else {
            return Self(out);
        };

        let banks = Banks::new();
        let mut stft = Stft::new(FFT);
        let scale = 1.0 / (FFT as f64 * stft.window_power());
        let (mut mels, mut mfccs) = (Vec::new(), Vec::new());
        let mut shape = Vec::new(); // centroid, flatness, flux per frame
        let mut previous: Option<[f64; MELS]> = None;
        for t in first..=last {
            let bins = stft.frame(&signal, (t * HOP) as isize - (FFT / 2) as isize);
            let power: Vec<f64> = bins.iter().map(|b| b.norm_sqr() * scale).collect();
            let mel = banks.log_mel(&power);
            let total: f64 = power.iter().sum::<f64>() + 1e-12;
            let centroid = power
                .iter()
                .enumerate()
                .map(|(k, p)| k as f64 * p)
                .sum::<f64>()
                / total
                / (FFT / 2) as f64;
            let log_mean = power.iter().skip(1).map(|p| (p + 1e-12).ln()).sum::<f64>()
                / (power.len() - 1) as f64;
            let flatness = log_mean.exp() / (total / power.len() as f64);
            let before = previous.unwrap_or(mel);
            let flux = mel
                .iter()
                .zip(before)
                .map(|(now, was)| (now - was).max(0.0))
                .sum::<f64>()
                / MELS as f64;
            previous = Some(mel);
            mfccs.push(banks.cepstrum(&mel));
            mels.push(mel);
            shape.push([centroid, flatness.min(1.0), flux]);
        }

        let count = mels.len();
        for m in 0..MELS {
            let (mean, std) = mean_std(mels.iter().map(|f| f[m]));
            out[MEL_MEAN + m] = mean as f32;
            out[MEL_STD + m] = std as f32;
        }
        for c in 0..MFCCS {
            let (mean, std) = mean_std(mfccs.iter().map(|f| f[c]));
            out[MFCC_MEAN + c] = mean as f32;
            out[MFCC_STD + c] = std as f32;
        }
        for (k, at) in [CENTROID, FLATNESS, FLUX].into_iter().enumerate() {
            out[at] = (shape.iter().map(|f| f[k]).sum::<f64>() / count as f64) as f32;
            for part in 0..PARTS {
                let (from, to) = (
                    part * count / PARTS,
                    ((part + 1) * count / PARTS).max(part * count / PARTS + 1),
                );
                let slice = &shape[from.min(count - 1)..to.min(count)];
                out[at + 1 + part] =
                    (slice.iter().map(|f| f[k]).sum::<f64>() / slice.len() as f64) as f32;
            }
        }

        let sound_seconds = ((last - first + 1) * HOP) as f64 / f64::from(RATE);
        let function = onsets::onset_function(&signal, RATE);
        out[ONSETS] =
            (onsets::detect(&signal, &function).len() as f64 / sound_seconds.max(0.25)) as f32;
        out[PITCHED] = pitchedness(&signal, first, last) as f32;
        Self(out)
    }
}

/// The mean and the standard deviation of `values`.
fn mean_std(values: impl Iterator<Item = f64> + Clone) -> (f64, f64) {
    let n = values.clone().count().max(1) as f64;
    let mean = values.clone().sum::<f64>() / n;
    let var = values.map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
    (mean, var.sqrt())
}

/// How pitched `signal` is between frames `first` and `last`: the highest
/// normalised autocorrelation at lags of 60 Hz to 2 kHz, averaged over up
/// to twelve frames spread across the span.
fn pitchedness(signal: &[f64], first: usize, last: usize) -> f64 {
    let lags = (RATE as usize / 2_000)..=(RATE as usize / 60);
    let picks = (last - first + 1).min(12);
    let mut sum = 0.0;
    let mut used = 0;
    for i in 0..picks {
        let t = first + i * (last - first) / picks.max(1);
        let from = (t * HOP).min(signal.len());
        let frame = &signal[from..(from + FFT).min(signal.len())];
        if frame.len() <= *lags.end() + 64 {
            continue;
        }
        let mean = frame.iter().sum::<f64>() / frame.len() as f64;
        let x: Vec<f64> = frame.iter().map(|s| s - mean).collect();
        if x.iter().map(|s| s * s).sum::<f64>() < 1e-9 {
            continue;
        }
        let best = lags
            .clone()
            .map(|lag| {
                let (a, b) = (&x[..x.len() - lag], &x[lag..]);
                let r: f64 = a.iter().zip(b).map(|(p, q)| p * q).sum();
                let e: f64 =
                    a.iter().map(|p| p * p).sum::<f64>() * b.iter().map(|q| q * q).sum::<f64>();
                if e > 0.0 { r / e.sqrt() } else { 0.0 }
            })
            .fold(0.0, f64::max);
        sum += best.clamp(0.0, 1.0);
        used += 1;
    }
    if used == 0 {
        0.0
    } else {
        sum / f64::from(used)
    }
}

/// The mel filter bank and the cepstrum's cosines, made once.
struct Banks {
    /// For each band, its triangle over the spectrum's bins.
    filters: Vec<Vec<f64>>,
    /// `cos(pi j (m + 0.5) / MELS)` for each coefficient `j` and band `m`.
    cosines: Vec<[f64; MELS]>,
}

impl Banks {
    fn new() -> Self {
        let mel = |hz: f64| 2_595.0 * (1.0 + hz / 700.0).log10();
        let hz = |mel: f64| 700.0 * (10f64.powf(mel / 2_595.0) - 1.0);
        let top = mel(f64::from(RATE) / 2.0);
        let edges: Vec<f64> = (0..MELS + 2)
            .map(|i| hz(top * i as f64 / (MELS + 1) as f64))
            .collect();
        let bins = FFT / 2 + 1;
        let at = |bin: usize| bin as f64 * f64::from(RATE) / FFT as f64;
        let filters = (0..MELS)
            .map(|m| {
                let (lo, mid, hi) = (edges[m], edges[m + 1], edges[m + 2]);
                (0..bins)
                    .map(|k| {
                        let f = at(k);
                        if f <= lo || f >= hi {
                            0.0
                        } else if f <= mid {
                            (f - lo) / (mid - lo)
                        } else {
                            (hi - f) / (hi - mid)
                        }
                    })
                    .collect()
            })
            .collect();
        let cosines = (0..MFCCS)
            .map(|j| {
                std::array::from_fn(|m| {
                    (std::f64::consts::PI * j as f64 * (m as f64 + 0.5) / MELS as f64).cos()
                })
            })
            .collect();
        Self { filters, cosines }
    }

    /// The natural logarithm of the energy in each band.
    fn log_mel(&self, power: &[f64]) -> [f64; MELS] {
        std::array::from_fn(|m| {
            let energy: f64 = self.filters[m].iter().zip(power).map(|(w, p)| w * p).sum();
            (energy + 1e-10).ln()
        })
    }

    /// The cepstral coefficients of a frame's log-mel energies.
    fn cepstrum(&self, mel: &[f64; MELS]) -> [f64; MFCCS] {
        std::array::from_fn(|j| self.cosines[j].iter().zip(mel).map(|(c, m)| c * m).sum())
    }
}

/// Each measurement's mean and spread over the analysed material, to scale
/// descriptors before they're compared (R3).
#[derive(Clone, Debug, PartialEq)]
pub struct Normaliser {
    mean: Vec<f32>,
    scale: Vec<f32>,
}

impl Normaliser {
    /// The normaliser of `descriptors`: each measurement centred on its
    /// mean and divided by its standard deviation (1 where it doesn't
    /// vary).
    #[must_use]
    pub fn fit<'a>(descriptors: impl Iterator<Item = &'a Descriptor> + Clone) -> Self {
        let mut mean = vec![0.0f32; LEN];
        let mut scale = vec![1.0f32; LEN];
        for i in 0..LEN {
            let (m, s) = mean_std(descriptors.clone().map(|d| f64::from(d.0[i])));
            mean[i] = m as f32;
            if s > 1e-9 {
                scale[i] = (1.0 / s) as f32;
            }
        }
        Self { mean, scale }
    }

    /// `descriptor`, scaled.
    #[must_use]
    pub fn apply(&self, descriptor: &Descriptor) -> Vec<f32> {
        descriptor
            .0
            .iter()
            .zip(&self.mean)
            .zip(&self.scale)
            .map(|((v, m), s)| (v - m) * s)
            .collect()
    }
}

/// The cosine distance between two scaled descriptors: 0 for the same
/// direction, 1 for unrelated and 2 for opposite. Two zero vectors are 0
/// apart.
#[must_use]
pub fn distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return if na == nb { 0.0 } else { 1.0 };
    }
    1.0 - dot / (na * nb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{level, sine};

    fn tone(hz: f64, seconds: f64) -> Vec<f32> {
        sine(hz, level(-6.0), seconds, RATE, 1, 0.0)
    }

    #[test]
    fn a_descriptor_survives_its_bytes_and_a_louder_copy_matches() {
        // Spec: sound-similarity R3.
        let quiet = tone(440.0, 0.5);
        let loud: Vec<f32> = quiet.iter().map(|s| s * 1.9).collect();
        let a = Descriptor::of_mono(&quiet);
        let b = Descriptor::of_mono(&loud);
        assert_eq!(a.values().len(), LEN);
        assert_eq!(Descriptor::from_bytes(&a.to_bytes()), Some(a.clone()));
        assert!(Descriptor::from_bytes(&[0; 7]).is_none());
        for (x, y) in a.values().iter().zip(b.values()) {
            assert!((x - y).abs() < 1e-3, "{x} {y}");
        }
    }

    #[test]
    fn a_tone_is_pitched_and_noise_is_not() {
        // Spec: sound-similarity R3.
        let noise = crate::signals::pink_noise(-6.0, 0.5, RATE, 3);
        let tonal = Descriptor::of_mono(&tone(220.0, 0.5));
        let noisy = Descriptor::of_mono(&noise);
        assert!(tonal.values()[PITCHED] > 0.9, "{}", tonal.values()[PITCHED]);
        assert!(noisy.values()[PITCHED] < 0.5, "{}", noisy.values()[PITCHED]);
        assert!(tonal.values()[CENTROID] < noisy.values()[CENTROID]);
        assert!(tonal.values()[FLATNESS] < noisy.values()[FLATNESS]);
    }

    #[test]
    fn a_sound_is_nearer_a_like_sound_than_an_unlike_one() {
        // Spec: sound-similarity R3.
        let low = Descriptor::of_mono(&tone(110.0, 0.5));
        let near = Descriptor::of_mono(&tone(123.0, 0.5));
        let far = Descriptor::of_mono(&crate::signals::pink_noise(-6.0, 0.5, RATE, 9));
        let all = [low.clone(), near.clone(), far.clone()];
        let norm = Normaliser::fit(all.iter());
        let d = |a: &Descriptor, b: &Descriptor| distance(&norm.apply(a), &norm.apply(b));
        assert!(d(&low, &near) < d(&low, &far));
        assert_eq!(d(&low, &low), 0.0);
    }

    #[test]
    fn silence_and_one_sample_have_descriptors() {
        let silent = Descriptor::of_mono(&[0.0; 4_000]);
        assert_eq!(silent.values().len(), LEN);
        assert!(silent.values().iter().all(|v| v.is_finite()));
        assert_eq!(Descriptor::of_mono(&[0.5]).values().len(), LEN);
    }

    #[test]
    fn audio_over_sixty_seconds_has_no_descriptor() {
        // Spec: sound-similarity R1.
        let long = SampleData::new(8_000, vec![vec![0.1; 8_000 * 61]]).unwrap();
        assert!(Descriptor::of_sample(&long).is_err());
        let short = SampleData::new(48_000, vec![vec![0.1; 4_800], vec![0.2; 4_800]]).unwrap();
        assert!(Descriptor::of_sample(&short).is_ok());
    }
}
