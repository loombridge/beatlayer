//! A coarse spectrum: the level in each octave band.
//!
//! The bands are IEC 61260's base-2 octaves, centred on 1 kHz × 2^k, from
//! 31.5 Hz to 16 kHz by their nominal names, each from its centre ÷ √2 to
//! its centre × √2 (so 22.1 Hz to 22.6 kHz in all), cut at the Nyquist
//! frequency. A band wholly above it is left out.
//!
//! The power spectrum is Welch's estimate: Hann-windowed FFTs of about
//! 170 ms, overlapping by half, averaged over the frames and the channels,
//! and scaled so the bins sum to the mean square. A band's level is the
//! sum of the bins whose frequencies fall in it, in dB relative to full
//! scale, as the RMS level is: a full-scale sine reads −3.01 dB in its
//! band, and the bands' powers add up to the RMS level's.

use crate::Audio;
use crate::stft::{Stft, size_for};

/// The bands' nominal centres, in hertz.
pub const NOMINAL: [f64; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

/// One band and its level.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    /// Its nominal centre, in hertz.
    pub centre: f64,
    /// Its lower edge, in hertz.
    pub low: f64,
    /// Its upper edge, in hertz (cut at the Nyquist frequency).
    pub high: f64,
    /// Its power: the mean square of what falls in it.
    pub power: f64,
}

/// The octave bands of `audio`, lowest first.
#[must_use]
pub fn octave_bands(audio: &Audio<'_>) -> Vec<Band> {
    let rate = f64::from(audio.rate());
    let nyquist = rate / 2.0;
    let mut bands: Vec<Band> = NOMINAL
        .iter()
        .enumerate()
        .map(|(k, &centre)| {
            let exact = 1_000.0 * 2f64.powi(k as i32 - 5);
            Band {
                centre,
                low: exact / std::f64::consts::SQRT_2,
                high: (exact * std::f64::consts::SQRT_2).min(nyquist),
                power: 0.0,
            }
        })
        .filter(|band| band.low < nyquist)
        .collect();
    let frames = audio.frames();
    if frames == 0 {
        return bands;
    }
    // About 170 ms, or the whole audio if it's shorter.
    let mut size = size_for(0.17, audio.rate());
    while size > 256 && size > frames {
        size /= 2;
    }
    let mut stft = Stft::new(size);
    let hop = size / 2;
    let starts: Vec<usize> = if frames <= size {
        vec![0]
    } else {
        (0..=(frames - size) / hop).map(|i| i * hop).collect()
    };
    let scale = 1.0 / (size as f64 * stft.window_power());
    // Each bin's band, if it has one.
    let band_of: Vec<Option<usize>> = (0..=size / 2)
        .map(|bin| {
            let hz = bin as f64 * rate / size as f64;
            bands.iter().position(|b| hz >= b.low && hz < b.high)
        })
        .collect();
    let mut sums = vec![0.0; bands.len()];
    for channel in 0..audio.channels() {
        let signal: Vec<f64> = audio.channel(channel).map(f64::from).collect();
        for &start in &starts {
            let bins = stft.frame(&signal, start as isize);
            for (bin, value) in bins.iter().enumerate() {
                if let Some(band) = band_of[bin] {
                    // One-sided: every bin but 0 Hz and the Nyquist bin
                    // stands for its negative twin too.
                    let twice = if bin == 0 || bin == size / 2 {
                        1.0
                    } else {
                        2.0
                    };
                    sums[band] += twice * value.norm_sqr() * scale;
                }
            }
        }
    }
    let count = (starts.len() * audio.channels()) as f64;
    for (band, sum) in bands.iter_mut().zip(sums) {
        band.power = sum / count;
    }
    bands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{level, sine};

    #[test]
    fn a_sine_s_power_is_in_its_band() {
        let samples = sine(1_000.0, level(-6.0), 3.0, 48_000, 2, 0.0);
        let bands = octave_bands(&Audio::new(&samples, 2, 48_000).unwrap());
        assert_eq!(bands.len(), 10);
        let khz = bands.iter().find(|b| b.centre == 1_000.0).unwrap();
        let db = crate::power_db(khz.power).unwrap();
        assert!((db + 6.0 + 3.0103).abs() < 0.05, "{db}");
        for band in bands.iter().filter(|b| b.centre != 1_000.0) {
            let other = crate::power_db(band.power).unwrap_or(-300.0);
            assert!(other < db - 60.0, "{}: {other}", band.centre);
        }
    }

    #[test]
    fn bands_above_the_nyquist_frequency_are_left_out() {
        let samples = sine(1_000.0, 0.5, 1.0, 22_050, 1, 0.0);
        let bands = octave_bands(&Audio::new(&samples, 1, 22_050).unwrap());
        assert_eq!(bands.last().unwrap().centre, 8_000.0);
        assert_eq!(bands.last().unwrap().high, 11_025.0);
    }
}
