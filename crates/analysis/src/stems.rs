//! Stem separation (IMP-11): splits a stereo mix into drums, bass,
//! others, and vocals with a Demucs network, as the stem separation spec
//! describes.
//!
//! [`separate`] does what Demucs's own `apply_model` does around the
//! network: it normalises the mix by its mono mean and deviation, cuts it
//! into the network's fixed-length windows with a quarter of each window
//! overlapping the next, runs each window, and blends the windows' outputs
//! with triangular weights. High Quality runs the whole of that twice, the
//! second time half a window's shift later, and averages the two
//! ([`Quality::HighQuality`]). The network itself is behind [`Network`],
//! so the tests run the arithmetic with a stand-in; [`onnx`] runs the real
//! one through ONNX Runtime, loaded at run time from the packages a person
//! downloads on first use.
//!
//! It runs on whatever thread calls it, never an audio thread: it
//! allocates freely.

pub mod generated;
#[cfg(feature = "stems")]
pub mod onnx;
pub mod transform;

/// The rate the network works at, in frames per second.
pub const RATE: u32 = 44_100;

/// One of the four parts a mix separates into, in the network's output
/// order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stem {
    /// Drums and percussion.
    Drums,
    /// Bass.
    Bass,
    /// Everything else: the "other" output.
    Others,
    /// Voices.
    Vocals,
}

impl Stem {
    /// The four, in the network's output order.
    pub const ALL: [Self; 4] = [Self::Drums, Self::Bass, Self::Others, Self::Vocals];

    /// Its position in the network's output.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Drums => 0,
            Self::Bass => 1,
            Self::Others => 2,
            Self::Vocals => 3,
        }
    }
}

/// How hard to work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    /// One pass over the mix.
    #[default]
    HighSpeed,
    /// Two passes, the second shifted by half a second, averaged: about
    /// twice as long, with fewer artefacts at the windows' joins.
    HighQuality,
}

impl Quality {
    const fn passes(self) -> usize {
        match self {
            Self::HighSpeed => 1,
            Self::HighQuality => 2,
        }
    }
}

/// A separation network: fixed-length stereo windows in, four stereo
/// stems out.
pub trait Network {
    /// The window's length in frames.
    fn window(&self) -> usize;

    /// Runs one window. `input` is planar: the left channel's
    /// [`Network::window`] samples, then the right's. The result is planar
    /// too: each stem's left then right, in [`Stem::ALL`]'s order.
    ///
    /// # Errors
    ///
    /// A message if the network fails.
    fn run(&mut self, input: &[f32]) -> Result<Vec<f32>, String>;
}

/// The four stems, each planar stereo at [`RATE`], as long as the mix.
#[derive(Clone, Debug, PartialEq)]
pub struct Stems {
    /// Each stem's left and right channels, in [`Stem::ALL`]'s order.
    pub stems: [[Vec<f32>; 2]; 4],
}

impl Stems {
    /// One stem's channels.
    #[must_use]
    pub fn get(&self, stem: Stem) -> &[Vec<f32>; 2] {
        &self.stems[stem.index()]
    }
}

/// Why a separation stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The progress callback asked it to stop.
    Cancelled,
    /// The network failed, or the audio can't be separated.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("the separation was cancelled"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// How much of a window overlaps the next.
const OVERLAP: f64 = 0.25;

/// Separates a stereo mix at [`RATE`] (`left` and `right`, the same
/// length) into its four stems. `progress` hears how far it is, from 0 to
/// 1, after each window, and stops it by returning `false`.
///
/// # Errors
///
/// [`Error::Cancelled`] if `progress` stopped it, or [`Error::Failed`] if
/// the channels differ in length or the network fails.
pub fn separate(
    left: &[f32],
    right: &[f32],
    network: &mut dyn Network,
    quality: Quality,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Stems, Error> {
    if left.len() != right.len() {
        return Err(Error::Failed(
            "the mix's channels aren't the same length".to_owned(),
        ));
    }
    let length = left.len();
    let window = network.window();
    if window < 4 {
        return Err(Error::Failed(
            "the network's window is too short".to_owned(),
        ));
    }
    let mut out: [[Vec<f32>; 2]; 4] =
        std::array::from_fn(|_| [vec![0.0; length], vec![0.0; length]]);
    if length == 0 {
        return Ok(Stems { stems: out });
    }
    // Demucs normalises by the mono mix's mean and standard deviation.
    let (mean, deviation) = mono_statistics(left, right);
    let normalised = |x: f32| (x - mean) / deviation;
    let passes = quality.passes();
    // Each pass after the first starts this much later, through silence
    // put before the mix.
    let max_shift = (RATE / 2) as usize;
    let stride = (((1.0 - OVERLAP) * window as f64).round() as usize).max(1);
    let windows = |padded: usize| padded.div_ceil(stride);
    let total: usize = (0..passes)
        .map(|pass| windows(length + shift_of(pass, passes, max_shift)))
        .sum();
    let weight = triangle(window);
    let mut done = 0;
    for pass in 0..passes {
        let shift = shift_of(pass, passes, max_shift);
        // The mix as this pass sees it: `shift` frames of silence first.
        let padded = length + shift;
        let sample = |channel: &[f32], i: usize| {
            if i < shift || i - shift >= length {
                0.0
            } else {
                normalised(channel[i - shift])
            }
        };
        let mut sums: [[Vec<f32>; 2]; 4] =
            std::array::from_fn(|_| [vec![0.0; padded], vec![0.0; padded]]);
        let mut weights = vec![0.0f32; padded];
        let mut input = vec![0.0f32; 2 * window];
        let mut offset = 0;
        while offset < padded {
            for i in 0..window {
                input[i] = sample(left, offset + i);
                input[window + i] = sample(right, offset + i);
            }
            let output = network.run(&input).map_err(Error::Failed)?;
            if output.len() != 8 * window {
                return Err(Error::Failed(format!(
                    "the network gave {} samples for a window of {window} frames",
                    output.len()
                )));
            }
            let valid = window.min(padded - offset);
            for (stem, sum) in sums.iter_mut().enumerate() {
                for (channel, sum) in sum.iter_mut().enumerate() {
                    let from = (stem * 2 + channel) * window;
                    let source = &output[from..from + valid];
                    for (i, value) in source.iter().enumerate() {
                        sum[offset + i] += value * weight[i];
                    }
                }
            }
            for (i, w) in weight.iter().take(valid).enumerate() {
                weights[offset + i] += w;
            }
            offset += stride;
            done += 1;
            if !progress(done as f32 / total as f32) {
                return Err(Error::Cancelled);
            }
        }
        for (stem, sum) in sums.iter().enumerate() {
            for (channel, sum) in sum.iter().enumerate() {
                let target = &mut out[stem][channel];
                for (i, value) in target.iter_mut().enumerate() {
                    let at = i + shift;
                    *value += sum[at] / weights[at];
                }
            }
        }
    }
    let scale = 1.0 / passes as f32;
    for channels in &mut out {
        for channel in channels {
            for value in channel.iter_mut() {
                *value = *value * scale * deviation + mean;
            }
        }
    }
    Ok(Stems { stems: out })
}

/// How far pass `pass` of `passes` starts after the first, in frames.
fn shift_of(pass: usize, passes: usize, max_shift: usize) -> usize {
    pass * max_shift / passes
}

/// The mono mix's mean and standard deviation, the deviation never 0.
fn mono_statistics(left: &[f32], right: &[f32]) -> (f32, f32) {
    let n = left.len() as f64;
    let mono = left.iter().zip(right).map(|(l, r)| f64::from(l + r) / 2.0);
    let mean = mono.clone().sum::<f64>() / n;
    let variance = mono.map(|x| (x - mean) * (x - mean)).sum::<f64>() / (n - 1.0).max(1.0);
    (mean as f32, (variance.sqrt() as f32).max(1e-8))
}

/// Demucs's window weights: rising from 1 to the middle and falling back,
/// scaled so the peak is 1.
fn triangle(window: usize) -> Vec<f32> {
    let half = window / 2;
    let rising = (1..=half).map(|i| i as f32);
    let falling = (1..=window - half).rev().map(|i| i as f32);
    let weights: Vec<f32> = rising.chain(falling).collect();
    let peak = weights.iter().copied().fold(0.0f32, f32::max);
    weights.into_iter().map(|w| w / peak).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gives each stem a fixed share of the window, so the stems sum to
    /// the (normalised) mix.
    struct Shares {
        window: usize,
        runs: usize,
    }

    const SHARES: [f32; 4] = [0.1, 0.2, 0.3, 0.4];

    impl Network for Shares {
        fn window(&self) -> usize {
            self.window
        }

        fn run(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
            self.runs += 1;
            let mut out = Vec::with_capacity(4 * input.len());
            for share in SHARES {
                out.extend(input.iter().map(|x| x * share));
            }
            Ok(out)
        }
    }

    fn mix(length: usize) -> (Vec<f32>, Vec<f32>) {
        let left = (0..length)
            .map(|i| (i as f32 * 0.013).sin() * 0.5)
            .collect();
        let right = (0..length)
            .map(|i| (i as f32 * 0.029).cos() * 0.25)
            .collect();
        (left, right)
    }

    fn check(quality: Quality, length: usize) {
        let (left, right) = mix(length);
        let (mean, _) = mono_statistics(&left, &right);
        let mut network = Shares {
            window: 1000,
            runs: 0,
        };
        let stems = separate(&left, &right, &mut network, quality, &mut |_| true).unwrap();
        for stem in Stem::ALL {
            let share = SHARES[stem.index()];
            for (channel, source) in [&left, &right].into_iter().enumerate() {
                let got = &stems.get(stem)[channel];
                assert_eq!(got.len(), length);
                for (i, (got, x)) in got.iter().zip(source).enumerate() {
                    // Each stem is its share of the normalised mix, scaled
                    // back: share × (x − mean) + mean.
                    let want = share * (x - mean) + mean;
                    assert!(
                        (got - want).abs() < 1e-4,
                        "{stem:?} {channel} {i}: {got} {want}"
                    );
                }
            }
        }
    }

    // Spec: stem-separation R12.
    #[test]
    fn overlapping_windows_blend_back_into_the_whole_mix() {
        check(Quality::HighSpeed, 4321);
        check(Quality::HighSpeed, 999);
        check(Quality::HighSpeed, 1);
    }

    // Spec: stem-separation R13.
    #[test]
    fn high_quality_averages_a_shifted_second_pass() {
        let (left, right) = mix(3000);
        let mut fast = Shares {
            window: 1000,
            runs: 0,
        };
        separate(&left, &right, &mut fast, Quality::HighSpeed, &mut |_| true).unwrap();
        let mut slow = Shares {
            window: 1000,
            runs: 0,
        };
        separate(&left, &right, &mut slow, Quality::HighQuality, &mut |_| {
            true
        })
        .unwrap();
        assert!(slow.runs > fast.runs);
        check(Quality::HighQuality, 3000);
    }

    #[test]
    fn progress_rises_to_one_and_can_cancel() {
        let (left, right) = mix(5000);
        let mut network = Shares {
            window: 1000,
            runs: 0,
        };
        let mut seen = Vec::new();
        separate(&left, &right, &mut network, Quality::HighSpeed, &mut |p| {
            seen.push(p);
            true
        })
        .unwrap();
        assert!(seen.windows(2).all(|w| w[0] < w[1]));
        assert!((seen.last().unwrap() - 1.0).abs() < 1e-6);
        let mut calls = 0;
        let result = separate(&left, &right, &mut network, Quality::HighSpeed, &mut |_| {
            calls += 1;
            calls < 2
        });
        assert_eq!(result, Err(Error::Cancelled));
    }

    #[test]
    fn the_window_weights_peak_at_one_in_the_middle() {
        let w = triangle(10);
        assert_eq!(w.len(), 10);
        assert!((w[4] - 1.0).abs() < 1e-6);
        assert!(w[0] < w[2] && w[9] < w[7]);
    }
}
