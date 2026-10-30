//! Audio as the suite measures it: what the null device played, or a file
//! an export wrote, as one `Vec` of samples per channel at a known rate;
//! and the measurements the test families assert on.
//!
//! Levels are in dBFS, where a full-scale sine's peak is 0 dBFS (and its
//! RMS −3 dBFS). Silence measures as negative infinity.

use std::path::Path;

/// Audio: one `Vec` of samples per channel, all the same length, at `rate`
/// frames a second.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Audio {
    /// Frames a second.
    pub rate: u32,
    /// The samples, one `Vec` per channel.
    pub channels: Vec<Vec<f32>>,
}

impl Audio {
    /// Audio of these channels at `rate`.
    ///
    /// # Panics
    ///
    /// If the channels differ in length.
    #[must_use]
    pub fn new(rate: u32, channels: Vec<Vec<f32>>) -> Self {
        if let Some(first) = channels.first() {
            assert!(
                channels.iter().all(|c| c.len() == first.len()),
                "every channel holds the same number of frames"
            );
        }
        Self { rate, channels }
    }

    /// Decodes an audio file, any format Beatlayer reads.
    ///
    /// # Errors
    ///
    /// Why the file couldn't be decoded.
    pub fn decode(path: &Path) -> Result<Self, String> {
        let data = bl_media::decode_file(path)
            .map_err(|error| format!("couldn't decode {}: {error}", path.display()))?;
        let channels = (0..data.channel_count())
            .map(|channel| data.channel(channel).unwrap_or_default().to_vec())
            .collect();
        Ok(Self::new(data.sample_rate(), channels))
    }

    /// How many frames it holds.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// How long it lasts, in seconds.
    #[must_use]
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.rate.max(1))
    }

    /// Frames `from` up to `to` (clamped to what there is).
    #[must_use]
    pub fn slice(&self, from: usize, to: usize) -> Self {
        let to = to.min(self.frames());
        let from = from.min(to);
        Self::new(
            self.rate,
            self.channels.iter().map(|c| c[from..to].to_vec()).collect(),
        )
    }

    /// The channels summed and divided by their number.
    #[must_use]
    pub fn mono(&self) -> Vec<f32> {
        let count = self.channels.len().max(1) as f32;
        (0..self.frames())
            .map(|frame| self.channels.iter().map(|c| c[frame]).sum::<f32>() / count)
            .collect()
    }

    /// The samples interleaved, frame by frame.
    #[must_use]
    pub fn interleaved(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.frames() * self.channels.len());
        for frame in 0..self.frames() {
            out.extend(self.channels.iter().map(|c| c[frame]));
        }
        out
    }

    /// The first sample that is NaN or infinite, as `(channel, frame)`.
    #[must_use]
    pub fn first_non_finite(&self) -> Option<(usize, usize)> {
        self.channels.iter().enumerate().find_map(|(channel, c)| {
            c.iter()
                .position(|s| !s.is_finite())
                .map(|frame| (channel, frame))
        })
    }

    /// The largest absolute sample over every channel, in dBFS.
    #[must_use]
    pub fn peak_dbfs(&self) -> f64 {
        let peak = self
            .channels
            .iter()
            .flatten()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        amplitude_db(f64::from(peak))
    }

    /// The RMS level over every channel and frame, in dBFS.
    #[must_use]
    pub fn rms_dbfs(&self) -> f64 {
        let count = self.channels.iter().map(Vec::len).sum::<usize>().max(1);
        let sum: f64 = self
            .channels
            .iter()
            .flatten()
            .map(|s| f64::from(*s) * f64::from(*s))
            .sum();
        amplitude_db((sum / count as f64).sqrt())
    }

    /// Integrated loudness in LUFS (ITU-R BS.1770, gated), as
    /// `bl-analysis` measures it: `None` for silence or audio too short to
    /// measure (under 400 ms).
    #[must_use]
    pub fn integrated_lufs(&self) -> Option<f64> {
        let samples = self.interleaved();
        let audio = bl_analysis::Audio::new(&samples, self.channels.len(), self.rate).ok()?;
        bl_analysis::loudness::measure(&audio).integrated
    }

    /// Whether every sample is below `threshold_dbfs`.
    #[must_use]
    pub fn is_silent(&self, threshold_dbfs: f64) -> bool {
        self.peak_dbfs() < threshold_dbfs
    }
}

/// An amplitude in dB, negative infinity for 0.
#[must_use]
pub fn amplitude_db(amplitude: f64) -> f64 {
    if amplitude > 0.0 {
        20.0 * amplitude.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// Panics, naming `what` and where, if any sample is NaN or infinite.
///
/// # Panics
///
/// As it says.
pub fn assert_finite(audio: &Audio, what: &str) {
    if let Some((channel, frame)) = audio.first_non_finite() {
        panic!(
            "{what}: sample {frame} of channel {channel} is {}",
            audio.channels[channel][frame]
        );
    }
}

/// Panics, naming `what`, unless the audio peaks at `threshold_dbfs` or
/// above and is finite throughout.
///
/// # Panics
///
/// As it says.
pub fn assert_sounds(audio: &Audio, what: &str, threshold_dbfs: f64) {
    assert_finite(audio, what);
    let peak = audio.peak_dbfs();
    assert!(
        peak >= threshold_dbfs,
        "{what} is silent: it peaks at {peak:.1} dBFS over {} frames",
        audio.frames()
    );
}

/// How two signals line up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    /// How many frames later the same sound comes in `other` than in
    /// `reference`: positive when `other` lags.
    pub lag: isize,
    /// The normalised cross-correlation at that lag, −1 to 1: 1 is the same
    /// waveform at any level.
    pub correlation: f64,
}

/// The lag, within ±`max_lag` frames, at which `other` best matches
/// `reference`, by normalised cross-correlation of their mono mixes over
/// `window` frames of `reference` starting at its loudest stretch. The
/// search is direct (window × lags multiplications), so keep both bounded:
/// a window of 8,192 and lags of a few thousand take milliseconds.
#[must_use]
pub fn best_alignment(
    reference: &Audio,
    other: &Audio,
    max_lag: usize,
    window: usize,
) -> Alignment {
    let a = reference.mono();
    let b = other.mono();
    let window = window.min(a.len());
    // The loudest window of the reference, by energy, in steps of a
    // quarter window, so the match isn't made on silence.
    let step = (window / 4).max(1);
    let start = (0..=a.len().saturating_sub(window))
        .step_by(step)
        .max_by(|&x, &y| {
            let energy = |at: usize| a[at..at + window].iter().map(|s| s * s).sum::<f32>();
            energy(x).total_cmp(&energy(y))
        })
        .unwrap_or(0);
    let segment = &a[start..start + window];
    let energy_a: f64 = segment.iter().map(|s| f64::from(*s).powi(2)).sum();
    let max_lag = max_lag as isize;
    let mut best = Alignment {
        lag: 0,
        correlation: f64::NEG_INFINITY,
    };
    for lag in -max_lag..=max_lag {
        let from = start as isize + lag;
        if from < 0 || from as usize + window > b.len() {
            continue;
        }
        let other = &b[from as usize..from as usize + window];
        let mut dot = 0.0f64;
        let mut energy_b = 0.0f64;
        for (x, y) in segment.iter().zip(other) {
            dot += f64::from(*x) * f64::from(*y);
            energy_b += f64::from(*y) * f64::from(*y);
        }
        let norm = (energy_a * energy_b).sqrt();
        let correlation = if norm > 0.0 { dot / norm } else { 0.0 };
        if correlation > best.correlation {
            best = Alignment { lag, correlation };
        }
    }
    best
}

/// The two signals' overlap once `other` is moved back by `lag` frames
/// ([`Alignment::lag`]): `(reference part, other part)`, the same length
/// and channels (the fewer of the two).
#[must_use]
pub fn overlap(reference: &Audio, other: &Audio, lag: isize) -> (Audio, Audio) {
    let (ref_from, other_from) = if lag >= 0 {
        (0, lag as usize)
    } else {
        ((-lag) as usize, 0)
    };
    let length = reference
        .frames()
        .saturating_sub(ref_from)
        .min(other.frames().saturating_sub(other_from));
    let channels = reference.channels.len().min(other.channels.len());
    let part = |audio: &Audio, from: usize| {
        Audio::new(
            audio.rate,
            audio.channels[..channels]
                .iter()
                .map(|c| c[from..from + length].to_vec())
                .collect(),
        )
    };
    (part(reference, ref_from), part(other, other_from))
}

/// How much is left when `other`, moved back by `lag` frames, is taken from
/// `reference`, in dB relative to `reference`'s level over the overlap:
/// −∞ for a perfect null, about 0 for unrelated signals of equal level, and
/// +6 for the same signal inverted.
#[must_use]
pub fn aligned_residual_db(reference: &Audio, other: &Audio, lag: isize) -> f64 {
    let (a, b) = overlap(reference, other, lag);
    let mut residual = 0.0f64;
    let mut energy = 0.0f64;
    for (x, y) in a.channels.iter().zip(&b.channels) {
        for (p, q) in x.iter().zip(y) {
            residual += f64::from(p - q).powi(2);
            energy += f64::from(*p).powi(2);
        }
    }
    if energy == 0.0 {
        return if residual == 0.0 {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    10.0 * (residual / energy).log10()
}

/// `other`'s RMS level less `reference`'s, in dB.
#[must_use]
pub fn level_difference_db(reference: &Audio, other: &Audio) -> f64 {
    other.rms_dbfs() - reference.rms_dbfs()
}

/// A sine of `hz` at `amplitude` peak, `frames` long, on `channels`
/// identical channels: a test signal.
#[must_use]
pub fn sine(rate: u32, hz: f64, amplitude: f32, frames: usize, channels: usize) -> Audio {
    let wave: Vec<f32> = (0..frames)
        .map(|n| {
            let phase = n as f64 * hz / f64::from(rate);
            (phase.fract() * std::f64::consts::TAU).sin() as f32 * amplitude
        })
        .collect();
    Audio::new(rate, vec![wave; channels])
}
