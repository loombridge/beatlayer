//! Tempo: how many beats a minute the audio suggests.
//!
//! 1. **Autocorrelation.** The onset function ([`crate::onsets`]), less its
//!    mean, is correlated with itself at every lag from 0.25 s (240 BPM) to
//!    1.5 s (40 BPM). Each lag's correlation is weighted by a log-Gaussian
//!    centred on 0.5 s (120 BPM), 1.4 octaves wide, as Ellis (2007) weights
//!    tempos by how people tend to hear them, and the best lag is refined
//!    between frames with a parabola through it and its neighbours.
//! 2. **Refinement through the transients.** Transients within 10% of a
//!    beat of the grid that period makes are fitted with a straight line,
//!    time against beat number, twice; its slope is the beat's length.
//!    With at least four transients spanning three beats this measures a
//!    steady tempo to a small fraction of a BPM.
//!
//! 3. **Beats.** The line also says where the beats fall: [`beats`] lays
//!    its grid from the first transient on it to the end of the audio.
//!
//! The weighting means a tempo far from 120 BPM may read as half or double.

use crate::onsets::OnsetFunction;

/// The slowest tempo reported, in BPM.
pub const SLOWEST: f64 = 40.0;
/// The fastest tempo reported, in BPM.
pub const FASTEST: f64 = 240.0;

/// What [`estimate`] found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tempo {
    /// Beats per minute.
    pub bpm: f64,
    /// How strongly the onsets repeat at that period, from 0 to 1: the
    /// autocorrelation there over its value at no lag.
    pub confidence: f64,
    /// How many transients the refinement fitted (0 if it didn't run).
    pub fitted: usize,
    /// The time in seconds of the first beat on the refinement's line: the
    /// first transient that sits on the grid. `None` if it didn't run.
    pub first_beat: Option<f64>,
}

/// The tempo the onset function `function` and the `transients` (seconds)
/// suggest, or `None` if the audio is shorter than two of the slowest
/// beats or has no rhythm to measure.
#[must_use]
pub fn estimate(function: &OnsetFunction, transients: &[f64]) -> Option<Tempo> {
    let hop = function.hop_seconds();
    let values = &function.values;
    let lag_of = |bpm: f64| 60.0 / (bpm * hop);
    let shortest = lag_of(FASTEST).floor() as usize;
    let longest = lag_of(SLOWEST).ceil() as usize;
    if values.len() < 2 * longest || shortest < 2 {
        return None;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let centred: Vec<f64> = values.iter().map(|v| v - mean).collect();
    let correlate = |lag: usize| -> f64 {
        centred[lag..]
            .iter()
            .zip(&centred)
            .map(|(a, b)| a * b)
            .sum::<f64>()
            / (centred.len() - lag) as f64
    };
    let zero = correlate(0);
    if zero <= 0.0 {
        return None;
    }
    let correlations: Vec<f64> = (shortest - 1..=longest + 1).map(correlate).collect();
    let at = |lag: usize| correlations[lag + 1 - shortest];
    let preferred = lag_of(120.0);
    let weight = |lag: f64| {
        let octaves = (lag / preferred).log2() / 1.4;
        (-0.5 * octaves * octaves).exp()
    };
    let best = (shortest..=longest)
        .filter(|&lag| at(lag) >= at(lag - 1) && at(lag) >= at(lag + 1))
        .max_by(|&a, &b| (at(a) * weight(a as f64)).total_cmp(&(at(b) * weight(b as f64))))?;
    if at(best) <= 0.0 {
        return None;
    }
    // A parabola through the peak and its neighbours.
    let (l, c, r) = (at(best - 1), at(best), at(best + 1));
    let curve = l - 2.0 * c + r;
    let shift = if curve < 0.0 {
        (0.5 * (l - r) / curve).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let mut period = (best as f64 + shift) * hop;
    let mut fitted = 0;
    let mut first_beat = None;
    for _ in 0..2 {
        if let Some(line) = fit(transients, period) {
            period = line.period;
            fitted = line.count;
            first_beat = Some(line.first_beat);
        } else {
            break;
        }
    }
    Some(Tempo {
        bpm: 60.0 / period,
        confidence: (c / zero).clamp(0.0, 1.0),
        fitted,
        first_beat,
    })
}

/// The beats of `tempo` in audio `seconds` long, as times in seconds: a
/// grid of its period from its first beat, or from the first transient if
/// the refinement found none, to the end. Empty if there is neither.
#[must_use]
pub fn beats(tempo: &Tempo, transients: &[f64], seconds: f64) -> Vec<f64> {
    let Some(first) = tempo.first_beat.or_else(|| transients.first().copied()) else {
        return Vec::new();
    };
    let period = 60.0 / tempo.bpm;
    (0..)
        .map(|n| first + f64::from(n) * period)
        .take_while(|&t| t < seconds)
        .collect()
}

/// A line through transients on a beat grid.
struct Line {
    /// Seconds a beat lasts.
    period: f64,
    /// How many transients it fits.
    count: usize,
    /// Where it puts the first of them.
    first_beat: f64,
}

/// A line through the transients that sit on the grid `period` makes,
/// anchored on the transient most others agree with, or `None` if fewer
/// than four fit, they span fewer than three beats, or the line's period is
/// more than 5% from `period`.
fn fit(transients: &[f64], period: f64) -> Option<Line> {
    let tolerance = 0.1 * period;
    let on_grid = |anchor: f64| -> Vec<(f64, f64)> {
        transients
            .iter()
            .filter_map(|&t| {
                let beats = (t - anchor) / period;
                let n = beats.round();
                ((beats - n).abs() * period <= tolerance).then_some((n, t))
            })
            .collect()
    };
    let anchor = transients
        .iter()
        .copied()
        .max_by_key(|&a| on_grid(a).len())?;
    let points = on_grid(anchor);
    if points.len() < 4 {
        return None;
    }
    let (first, last) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), &(n, _)| {
            (lo.min(n), hi.max(n))
        });
    if last - first < 3.0 {
        return None;
    }
    let count = points.len() as f64;
    let mean_n = points.iter().map(|p| p.0).sum::<f64>() / count;
    let mean_t = points.iter().map(|p| p.1).sum::<f64>() / count;
    let (mut num, mut den) = (0.0, 0.0);
    for &(n, t) in &points {
        num += (n - mean_n) * (t - mean_t);
        den += (n - mean_n) * (n - mean_n);
    }
    let slope = num / den;
    ((slope - period).abs() <= 0.05 * period).then_some(Line {
        period: slope,
        count: points.len(),
        // A beat a hair before the start of the audio is at the start.
        first_beat: (mean_t + slope * (first - mean_n)).max(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Audio;
    use crate::onsets::{detect, onset_function};
    use crate::signals::click_track;

    fn tempo_of(samples: &[f32], rate: u32) -> Option<Tempo> {
        let audio = Audio::new(samples, 1, rate).unwrap();
        let mono = audio.mono();
        let function = onset_function(&mono, rate);
        let transients = detect(&mono, &function);
        estimate(&function, &transients)
    }

    #[test]
    fn a_click_track_s_tempo_is_measured() {
        for bpm in [97.0, 120.0, 128.0, 140.0, 74.0] {
            let tempo = tempo_of(&click_track(bpm, 0.1, 12.0, 48_000).0, 48_000).unwrap();
            assert!((tempo.bpm - bpm).abs() < 0.05, "{bpm}: {tempo:?}");
            assert!(tempo.fitted >= 4, "{bpm}: {tempo:?}");
            assert!(tempo.confidence > 0.3, "{bpm}: {tempo:?}");
        }
    }

    #[test]
    fn too_little_audio_or_none_has_no_tempo() {
        assert_eq!(
            tempo_of(&click_track(120.0, 0.1, 2.0, 48_000).0, 48_000),
            None
        );
        assert_eq!(tempo_of(&vec![0.0; 48_000 * 8], 48_000), None);
    }
}
