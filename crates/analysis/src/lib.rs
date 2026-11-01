//! Transient, tempo, key, and loudness analysis, and on-device ML features.
//!
//! WP-1.16 built the analysis the agent API reads audio with (the
//! architecture's "Perception"): each module measures one thing in a block
//! of rendered audio, with a published, standard method, and returns plain
//! numbers in stated units.
//!
//! | Module | Measures | Method |
//! |---|---|---|
//! | [`loudness`] | Integrated, momentary, and short-term loudness (LUFS), loudness range (LU), and true peak (dBTP) | ITU-R BS.1770-4 with EBU R 128's gating and windows (EBU Tech 3341, 3342) |
//! | [`levels`] | Sample peak and RMS (dBFS), per channel and overall | Their definitions |
//! | [`spectrum`] | The level in each octave band, 31.5 Hz to 16 kHz | Welch-averaged Hann-windowed FFTs, summed per band (IEC 61260's base-2 band edges) |
//! | [`measure`] | A steady tone's pitch, a vowel's first two formants, how far below its harmonics the rest of a tone lies, and a swept tone's level against the rest of the output (warp-modes R34 to R36) | The strongest peak of one-second Hann frames with a parabolic refinement; the peaks of an all-pole fit to the average spectrum; the power outside the harmonics' bins; Kaiser-windowed frames around the swept frequency |
//! | [`onsets`] | Transient times | Spectral flux with adaptive peak picking (after Böck, Krebs and Schedl, 2012), refined in the waveform |
//! | [`tempo`] | Tempo (BPM) | The onset function's autocorrelation, weighted towards 120 BPM (after Ellis, 2007), refined by a line through the transients |
//! | [`tempo_tracker`] | Tempo, beat phase and confidence in real time, for Tempo Follower (WP-9.12) | A comb-filter bank over the last 6 s of spectral-flux onset strength, a causal beat grid, and a lock-free copy of the input off the audio thread |
//! | [`key`] | Key and mode | A chromagram correlated with Krumhansl and Kessler's key profiles (1982) |
//! | [`similarity`] | A sound's descriptor, for finding sounds that sound alike (sound-similarity R3) | Log-mel and cepstral statistics, spectral shape envelopes, onset density, length and pitchedness, compared by cosine distance |
//! | [`autotag`] | A sample's automatic tags, for the browser (browser-tags R53) | Loop by onsets and length; a class by a softmax regression over the descriptor, fitted from generated sounds |
//! | [`loops`] | The span of a sample that repeats, for the autosampler | Rising zero crossings of the left channel, matched over 10 ms |
//!
//! [`stems`] separates a mix into drums, bass, others, and vocals with a
//! Demucs network (IMP-11).
//!
//! [`cache`] keeps what is found in each audio file, by the file's content
//! hash (WP-4.3).
//!
//! Analysis runs offline, on whatever thread calls it, never on an audio
//! thread: it allocates freely.

pub mod autotag;
pub mod cache;
pub mod key;
pub mod levels;
pub mod loops;
pub mod loudness;
pub mod measure;
pub mod onsets;
pub mod similarity;
pub mod spectrum;
pub mod stems;
pub mod tempo;
pub mod tempo_tracker;

#[cfg(test)]
bl_rt::install_trap!();

mod stft;

/// Interleaved audio to analyse: each frame's channels in turn.
#[derive(Clone, Copy, Debug)]
pub struct Audio<'a> {
    samples: &'a [f32],
    channels: usize,
    rate: u32,
}

impl<'a> Audio<'a> {
    /// `samples`, interleaved in frames of `channels`, at `rate` frames per
    /// second.
    ///
    /// # Errors
    ///
    /// A message if there are no channels, the rate is under 1 kHz, or the
    /// samples aren't whole frames.
    pub fn new(samples: &'a [f32], channels: usize, rate: u32) -> Result<Self, String> {
        if channels == 0 {
            return Err("audio needs at least one channel".to_owned());
        }
        if rate < 1_000 {
            return Err(format!("a sample rate of {rate} Hz is too low to analyse"));
        }
        if !samples.len().is_multiple_of(channels) {
            return Err(format!(
                "{} samples aren't whole frames of {channels} channels",
                samples.len()
            ));
        }
        Ok(Self {
            samples,
            channels,
            rate,
        })
    }

    /// The interleaved samples.
    #[must_use]
    pub fn samples(&self) -> &'a [f32] {
        self.samples
    }

    /// Channels in each frame.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames per second.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// How many frames there are.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    /// How long the audio is, in seconds.
    #[must_use]
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.rate)
    }

    /// One channel's samples, in order.
    pub fn channel(&self, channel: usize) -> impl Iterator<Item = f32> + 'a {
        self.samples
            .iter()
            .skip(channel)
            .step_by(self.channels)
            .copied()
    }

    /// The channels' average, frame by frame: what the onset, tempo, and
    /// key analyses listen to.
    #[must_use]
    pub fn mono(&self) -> Vec<f64> {
        let scale = 1.0 / self.channels as f64;
        self.samples
            .chunks_exact(self.channels)
            .map(|frame| frame.iter().map(|&s| f64::from(s)).sum::<f64>() * scale)
            .collect()
    }
}

/// A power ratio in decibels, or `None` for zero.
#[must_use]
pub fn power_db(power: f64) -> Option<f64> {
    (power > 0.0 && power.is_finite()).then(|| 10.0 * power.log10())
}

/// An amplitude ratio in decibels, or `None` for zero.
#[must_use]
pub fn amplitude_db(amplitude: f64) -> Option<f64> {
    (amplitude > 0.0 && amplitude.is_finite()).then(|| 20.0 * amplitude.log10())
}

pub mod signals;
