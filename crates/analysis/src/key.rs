//! Key: the tonic and mode the audio's pitches suggest.
//!
//! 1. **A chromagram.** Hann-windowed FFTs of about 170 ms of the channels'
//!    average, overlapping by half; each bin from 65 Hz (C2) to 2.1 kHz
//!    (about C7) adds its magnitude to the pitch class nearest its
//!    frequency (A4 = 440 Hz, equal temperament). The frames' sums make one
//!    12-value profile.
//! 2. **Key profiles.** The profile is correlated (Pearson's r) with
//!    Krumhansl and Kessler's major and minor key profiles (1982), turned
//!    to each of the 12 tonics. The best of the 24 is the key, as the
//!    Krumhansl–Schmuckler algorithm reads it.
//!
//! Relative keys (C major and A minor) share their notes, so the margin
//! over the runner-up says how clear the choice was.

use crate::Audio;
use crate::stft::{Stft, size_for};

/// Krumhansl and Kessler's major key profile, from the tonic up.
pub const MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];

/// Krumhansl and Kessler's minor key profile, from the tonic up.
pub const MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// The pitch classes' names, from C, with sharps.
pub const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// A key: a tonic and a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyName {
    /// The tonic's pitch class, 0 for C to 11 for B.
    pub tonic: usize,
    /// Minor rather than major.
    pub minor: bool,
}

impl std::fmt::Display for KeyName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}",
            NAMES[self.tonic % 12],
            if self.minor { "minor" } else { "major" }
        )
    }
}

/// What [`estimate`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    /// The best key.
    pub key: KeyName,
    /// Its correlation with the audio's pitch profile, from −1 to 1.
    pub correlation: f64,
    /// The second-best key.
    pub runner_up: KeyName,
    /// Its correlation.
    pub runner_up_correlation: f64,
    /// The audio's pitch profile, from C, scaled so its largest is 1.
    pub chroma: [f64; 12],
}

/// The pitch profile of `audio`: each pitch class's share of the
/// magnitude between 65 Hz and 2.1 kHz, or `None` if there's none.
#[must_use]
pub fn chroma(audio: &Audio<'_>) -> Option<[f64; 12]> {
    let mono = audio.mono();
    if mono.is_empty() {
        return None;
    }
    let rate = f64::from(audio.rate());
    let mut size = size_for(0.17, audio.rate());
    while size > 1_024 && size > mono.len() {
        size /= 2;
    }
    let mut stft = Stft::new(size);
    let hop = size / 2;
    let class_of: Vec<Option<usize>> = (0..=size / 2)
        .map(|bin| {
            let hz = bin as f64 * rate / size as f64;
            (65.0..=2_100.0).contains(&hz).then(|| {
                let midi = 69.0 + 12.0 * (hz / 440.0).log2();
                (midi.round() as i64).rem_euclid(12) as usize
            })
        })
        .collect();
    let mut profile = [0.0; 12];
    let mut start = 0;
    while start < mono.len() {
        let bins = stft.frame(&mono, start as isize);
        for (bin, value) in bins.iter().enumerate() {
            if let Some(class) = class_of[bin] {
                profile[class] += value.norm();
            }
        }
        start += hop;
    }
    let largest = profile.iter().copied().fold(0.0, f64::max);
    if largest <= 1e-9 {
        return None;
    }
    for value in &mut profile {
        *value /= largest;
    }
    Some(profile)
}

/// Pearson's correlation of two profiles.
fn correlation(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let mean_a = a.iter().sum::<f64>() / 12.0;
    let mean_b = b.iter().sum::<f64>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        num += (x - mean_a) * (y - mean_b);
        da += (x - mean_a) * (x - mean_a);
        db += (y - mean_b) * (y - mean_b);
    }
    if da <= 0.0 || db <= 0.0 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

/// The key a pitch profile suggests: every key's correlation, best first.
#[must_use]
pub fn rank(profile: &[f64; 12]) -> Vec<(KeyName, f64)> {
    let mut keys: Vec<(KeyName, f64)> = (0..24)
        .map(|k| {
            let key = KeyName {
                tonic: k % 12,
                minor: k >= 12,
            };
            let template = if key.minor { &MINOR } else { &MAJOR };
            // The template turned so its tonic is at the key's pitch class.
            let turned: [f64; 12] =
                std::array::from_fn(|class| template[(class + 12 - key.tonic) % 12]);
            (key, correlation(profile, &turned))
        })
        .collect();
    keys.sort_by(|a, b| b.1.total_cmp(&a.1));
    keys
}

/// The key of `audio`, or `None` if it has no pitched content in range.
#[must_use]
pub fn estimate(audio: &Audio<'_>) -> Option<Key> {
    let profile = chroma(audio)?;
    let ranked = rank(&profile);
    let (key, correlation) = ranked[0];
    let (runner_up, runner_up_correlation) = ranked[1];
    Some(Key {
        key,
        correlation,
        runner_up,
        runner_up_correlation,
        chroma: profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{A_MINOR_CADENCE, C_MAJOR_CADENCE, progression};

    fn key_of(samples: &[f32]) -> Key {
        estimate(&Audio::new(samples, 1, 48_000).unwrap()).unwrap()
    }

    #[test]
    fn cadences_are_heard_in_their_keys() {
        let found = key_of(&progression(&C_MAJOR_CADENCE, 1.0, 48_000));
        assert_eq!(found.key.to_string(), "C major", "{found:?}");
        let found = key_of(&progression(&A_MINOR_CADENCE, 1.0, 48_000));
        assert_eq!(found.key.to_string(), "A minor", "{found:?}");
        // The C major cadence a tritone up: F sharp major.
        let f_sharp: Vec<Vec<u8>> = C_MAJOR_CADENCE
            .iter()
            .map(|chord| chord.iter().map(|n| n + 6).collect())
            .collect();
        let f_sharp: Vec<&[u8]> = f_sharp.iter().map(Vec::as_slice).collect();
        let found = key_of(&progression(&f_sharp, 1.0, 48_000));
        assert_eq!(found.key.to_string(), "F# major", "{found:?}");
        assert!(found.correlation > found.runner_up_correlation);
    }

    #[test]
    fn silence_has_no_key() {
        assert_eq!(
            estimate(&Audio::new(&vec![0.0; 48_000], 1, 48_000).unwrap()),
            None
        );
    }
}
