//! Sample peak and RMS level.
//!
//! The sample peak is the largest sample magnitude; the RMS level is the
//! root of the mean square. Both are relative to full scale (1.0), so a
//! full-scale square wave reads 0 dBFS RMS and a full-scale sine −3.01 dBFS.

use crate::Audio;

/// What [`measure`] found, as linear magnitudes.
#[derive(Clone, Debug, PartialEq)]
pub struct Levels {
    /// Each channel's largest sample magnitude.
    pub peak: Vec<f64>,
    /// Each channel's RMS level.
    pub rms: Vec<f64>,
    /// The largest sample magnitude in any channel.
    pub peak_all: f64,
    /// The RMS level of every sample in every channel.
    pub rms_all: f64,
}

/// Measures the sample peak and RMS level of `audio`.
#[must_use]
pub fn measure(audio: &Audio<'_>) -> Levels {
    let frames = audio.frames().max(1) as f64;
    let mut peak = Vec::with_capacity(audio.channels());
    let mut squares = Vec::with_capacity(audio.channels());
    for channel in 0..audio.channels() {
        let (p, s) = audio.channel(channel).fold((0.0f64, 0.0f64), |(p, s), x| {
            let x = f64::from(x);
            (p.max(x.abs()), s + x * x)
        });
        peak.push(p);
        squares.push(s);
    }
    let total: f64 = squares.iter().sum();
    Levels {
        peak_all: peak.iter().copied().fold(0.0, f64::max),
        rms_all: (total / (frames * audio.channels() as f64)).sqrt(),
        rms: squares.iter().map(|s| (s / frames).sqrt()).collect(),
        peak,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{level, sine};

    #[test]
    fn a_sine_s_rms_is_3_db_under_its_peak() {
        let samples = sine(1_000.0, level(-6.0), 2.0, 48_000, 2, 0.5);
        let levels = measure(&Audio::new(&samples, 2, 48_000).unwrap());
        let peak = crate::amplitude_db(levels.peak_all).unwrap();
        let rms = crate::amplitude_db(levels.rms_all).unwrap();
        assert!((peak + 6.0).abs() < 0.01, "{peak}");
        assert!((rms + 6.0 + 3.0103).abs() < 0.01, "{rms}");
        assert_eq!(levels.peak.len(), 2);
    }

    #[test]
    fn each_channel_is_measured_on_its_own() {
        // Left at 0.5, right silent.
        let samples: Vec<f32> = (0..1_000).flat_map(|_| [0.5, 0.0]).collect();
        let levels = measure(&Audio::new(&samples, 2, 48_000).unwrap());
        assert_eq!(levels.peak, vec![0.5, 0.0]);
        assert_eq!(levels.rms, vec![0.5, 0.0]);
        assert!((levels.rms_all - (0.125f64).sqrt()).abs() < 1e-12);
    }
}
