//! Transients: where notes and hits start.
//!
//! 1. **The onset function** is the spectral flux of the channels' average:
//!    Hann-windowed FFTs of about 21 ms every quarter of that (about 5 ms),
//!    each bin's magnitude compressed as `log10(1 + 1000 m)` (with `m` 1 for
//!    a full-scale sine, less a floor of −80 dB so a steady tone's faint
//!    side lobes add nothing), and the sum of each bin's rise over the
//!    largest of its own and its two neighbours' values in the frame before
//!    (the maximum filter of Böck and Widmer's SuperFlux, 2013, which
//!    keeps a steady or gliding tone from reading as onsets). Its frames
//!    are centred on their times.
//! 2. **Peak picking** follows Böck, Krebs and Schedl (2012): a frame is an
//!    onset where the function is the largest within 15 ms either side, at
//!    least a threshold above its mean over the 50 ms before and 15 ms
//!    after, and at least 30 ms after the last onset. The threshold is 5%
//!    of the function's largest value. Frames where the audio is quieter
//!    than −60 dBFS are passed over.
//! 3. **Refinement** finds each onset in the waveform: within the frame's
//!    window, the sample where the energy over the next 2 ms most exceeds
//!    the energy over the 2 ms before. A step or a click is found to within
//!    a sample or two of where it starts.

use crate::Audio;
use crate::stft::{Stft, size_for};

/// The onset function's compression.
const COMPRESSION: f64 = 1_000.0;

/// The magnitude under which a bin counts as silent: −80 dB.
const FLOOR: f64 = 1e-4;

/// The onset function: one value per frame, `hop` frames of audio apart.
#[derive(Clone, Debug, PartialEq)]
pub struct OnsetFunction {
    /// The audio's rate.
    pub rate: u32,
    /// Frames of audio between values.
    pub hop: usize,
    /// The FFT's size, in frames of audio.
    pub size: usize,
    /// Each frame's spectral flux. Value `t` is centred on frame `t × hop`.
    pub values: Vec<f64>,
    /// Each frame's mean square, in the channels' average.
    pub energy: Vec<f64>,
}

impl OnsetFunction {
    /// Seconds between values.
    #[must_use]
    pub fn hop_seconds(&self) -> f64 {
        self.hop as f64 / f64::from(self.rate)
    }
}

/// The onset function of `mono`, the channels' average at `rate`.
#[must_use]
pub fn onset_function(mono: &[f64], rate: u32) -> OnsetFunction {
    onset_function_at(mono, rate, size_for(0.021, rate) / 4)
}

/// The onset function of `mono` with a value every `hop` frames of audio.
#[must_use]
pub fn onset_function_at(mono: &[f64], rate: u32, hop: usize) -> OnsetFunction {
    let size = size_for(0.021, rate);
    let mut stft = Stft::new(size);
    let norm = 2.0 / stft.window_sum();
    let count = mono.len().div_ceil(hop);
    let mut values = Vec::with_capacity(count);
    let mut energy = Vec::with_capacity(count);
    let mut previous: Vec<f64> = vec![0.0; size / 2 + 1];
    let mut current = previous.clone();
    for t in 0..count {
        let start = (t * hop) as isize - (size / 2) as isize;
        let bins = stft.frame(mono, start);
        let mut flux = 0.0;
        let mut power = 0.0;
        for (value, now) in bins.iter().zip(current.iter_mut()) {
            let m = value.norm() * norm;
            power += value.norm_sqr();
            *now = (1.0 + COMPRESSION * (m - FLOOR).max(0.0)).log10();
        }
        for (k, now) in current.iter().enumerate() {
            let before = previous[k.saturating_sub(1)..(k + 2).min(previous.len())]
                .iter()
                .copied()
                .fold(0.0, f64::max);
            flux += (now - before).max(0.0);
        }
        values.push(flux);
        energy.push(2.0 * power / (stft.size() as f64 * stft.window_power()));
        std::mem::swap(&mut previous, &mut current);
    }
    OnsetFunction {
        rate,
        hop,
        size,
        values,
        energy,
    }
}

/// The transients in `audio`, in seconds from its start, in order.
#[must_use]
pub fn transients(audio: &Audio<'_>) -> Vec<f64> {
    let mono = audio.mono();
    let function = onset_function(&mono, audio.rate());
    detect(&mono, &function)
}

/// The transients the onset function `function` shows in `mono`, in
/// seconds.
#[must_use]
pub fn detect(mono: &[f64], function: &OnsetFunction) -> Vec<f64> {
    let rate = f64::from(function.rate);
    let frames_of = |seconds: f64| ((seconds * rate / function.hop as f64).round() as usize).max(1);
    let around = frames_of(0.015);
    let before = frames_of(0.05);
    let gap = frames_of(0.03);
    let values = &function.values;
    let largest = values.iter().copied().fold(0.0, f64::max);
    if largest <= 0.0 {
        return Vec::new();
    }
    let threshold = 0.05 * largest;
    let quiet = 1e-6; // −60 dBFS as a mean square.
    let mut picked: Vec<usize> = Vec::new();
    for t in 0..values.len() {
        let value = values[t];
        let window = &values[t.saturating_sub(around)..(t + around + 1).min(values.len())];
        if value <= 0.0 || window.iter().any(|&v| v > value) {
            continue;
        }
        // On a plateau, the first frame counts.
        if t > 0 && values[t - 1] == value {
            continue;
        }
        let local = &values[t.saturating_sub(before)..(t + around + 1).min(values.len())];
        let mean = local.iter().sum::<f64>() / local.len() as f64;
        if value < mean + threshold {
            continue;
        }
        let loud = function.energy[t.saturating_sub(1)..(t + 2).min(values.len())]
            .iter()
            .any(|&e| e > quiet);
        if !loud {
            continue;
        }
        if picked.last().is_some_and(|&last| t - last < gap) {
            continue;
        }
        picked.push(t);
    }
    let min_gap = 0.02 * rate;
    let mut times: Vec<f64> = Vec::with_capacity(picked.len());
    for t in picked {
        let at = refine(mono, function, t);
        if times.last().is_none_or(|&last| at - last * rate >= min_gap) {
            times.push(at / rate);
        }
    }
    times
}

/// Where in the waveform the onset frame `t` saw starts: the sample, in the
/// frame's window, where the energy of the next 2 ms most exceeds that of
/// the 2 ms before. Returns a sample index.
fn refine(mono: &[f64], function: &OnsetFunction, t: usize) -> f64 {
    let rate = f64::from(function.rate);
    let width = ((0.002 * rate).round() as usize).max(1);
    let centre = t * function.hop;
    let from = centre.saturating_sub(function.size / 2);
    let to = (centre + function.size / 2).min(mono.len());
    if to <= from {
        return centre as f64;
    }
    // Prefix sums of the squares, from `width` before `from` to `width`
    // after `to`.
    let lo = from.saturating_sub(width);
    let hi = (to + width).min(mono.len());
    let mut prefix = Vec::with_capacity(hi - lo + 1);
    prefix.push(0.0);
    let mut sum = 0.0;
    for &x in &mono[lo..hi] {
        sum += x * x;
        prefix.push(sum);
    }
    let energy = |a: usize, b: usize| prefix[b.min(hi) - lo] - prefix[a.max(lo) - lo];
    let mut best = (f64::NEG_INFINITY, centre);
    for i in from..to {
        let rise = energy(i, i + width) - energy(i.saturating_sub(width), i);
        if rise > best.0 {
            best = (rise, i);
        }
    }
    best.1 as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_are_found_where_they_start() {
        for rate in [44_100, 48_000] {
            let (samples, at) = crate::signals::click_track(60.0 / 0.37, 0.25, 5.0, rate);
            let found = transients(&Audio::new(&samples, 1, rate).unwrap());
            assert_eq!(found.len(), at.len(), "{rate} Hz: {found:?}");
            for (got, want) in found.iter().zip(&at) {
                assert!((got - want).abs() < 0.001, "{rate} Hz: {got} vs {want}");
            }
        }
    }

    #[test]
    fn silence_and_a_steady_tone_have_no_transients() {
        let silent = vec![0.0f32; 48_000];
        assert!(transients(&Audio::new(&silent, 1, 48_000).unwrap()).is_empty());
        // A tone that fades in over 50 ms has one onset, at its start.
        let mut tone = crate::signals::sine(440.0, 0.5, 3.0, 48_000, 1, 0.0);
        crate::signals::fade(&mut tone, 1, 2_400);
        let found = transients(&Audio::new(&tone, 1, 48_000).unwrap());
        assert!(found.len() <= 1, "{found:?}");
    }
}
