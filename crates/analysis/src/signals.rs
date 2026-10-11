//! Test signals with known answers, generated in code: sines at a level,
//! pink noise, click tracks at a tempo, and chord progressions in a key.
//! Beatlayer's tests check the analysis against them; no recorded audio is
//! used. Every generator is deterministic: noise comes from a seeded
//! generator.

use std::f64::consts::TAU;

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

/// The amplitude of a level in dBFS.
#[must_use]
pub fn level(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// A sine of `hz` at peak amplitude `amplitude` (linear), `seconds` long,
/// the same in each of `channels` (interleaved), starting at `phase`
/// radians.
#[must_use]
pub fn sine(
    hz: f64,
    amplitude: f64,
    seconds: f64,
    rate: u32,
    channels: usize,
    phase: f64,
) -> Vec<f32> {
    let frames = (seconds * f64::from(rate)).round() as usize;
    (0..frames)
        .flat_map(|n| {
            let s = (amplitude * (TAU * hz * n as f64 / f64::from(rate) + phase).sin()) as f32;
            std::iter::repeat_n(s, channels)
        })
        .collect()
}

/// Fades interleaved `samples` in and out over `frames` frames each, with
/// a raised cosine.
pub fn fade(samples: &mut [f32], channels: usize, frames: usize) {
    let total = samples.len() / channels;
    for n in 0..frames.min(total / 2) {
        let gain = (0.5 - 0.5 * (std::f64::consts::PI * n as f64 / frames as f64).cos()) as f32;
        for c in 0..channels {
            samples[n * channels + c] *= gain;
            samples[(total - 1 - n) * channels + c] *= gain;
        }
    }
}

/// The next number in [0, 1) from a SplitMix64 generator.
fn uniform(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Pink noise, mono: equal power in every octave from 20 Hz to the Nyquist
/// frequency and none below, at an RMS level of `rms_db` dBFS, `seconds`
/// long. It's shaped exactly, in the frequency domain: every bin's
/// magnitude falls as one over the square root of its frequency, with a
/// random phase from `seed`.
#[must_use]
pub fn pink_noise(rms_db: f64, seconds: f64, rate: u32, seed: u64) -> Vec<f32> {
    let frames = (seconds * f64::from(rate)).round() as usize;
    let size = frames.next_power_of_two().max(2);
    let ifft = RealFftPlanner::<f64>::new().plan_fft_inverse(size);
    let mut spectrum = ifft.make_input_vec();
    let mut state = seed;
    let lowest = 20.0 * size as f64 / f64::from(rate);
    let last = spectrum.len() - 1;
    for (k, bin) in spectrum.iter_mut().enumerate() {
        let phase = TAU * uniform(&mut state);
        *bin = if (k as f64) < lowest || k == last {
            Complex::new(0.0, 0.0)
        } else {
            Complex::from_polar(1.0 / (k as f64).sqrt(), phase)
        };
    }
    let mut out = ifft.make_output_vec();
    ifft.process(&mut spectrum, &mut out)
        .expect("the buffers are the plan's own");
    out.truncate(frames);
    let rms = (out.iter().map(|x| x * x).sum::<f64>() / frames.max(1) as f64).sqrt();
    let gain = if rms > 0.0 { level(rms_db) / rms } else { 0.0 };
    out.iter().map(|x| (x * gain) as f32).collect()
}

/// A click track, mono: a 10 ms click (a 1.5 kHz sine decaying with a
/// 2 ms time constant) on every beat of `bpm`, the first `offset` seconds
/// in, louder on each bar's first beat of four, in `seconds` of audio.
/// Returns the samples and each click's start in seconds.
#[must_use]
pub fn click_track(bpm: f64, offset: f64, seconds: f64, rate: u32) -> (Vec<f32>, Vec<f64>) {
    let mut samples = vec![0.0f32; (seconds * f64::from(rate)).round() as usize];
    let period = 60.0 / bpm;
    let mut starts = Vec::new();
    for beat in 0.. {
        let at = offset + f64::from(beat) * period;
        let start = (at * f64::from(rate)).round() as usize;
        if start >= samples.len() {
            break;
        }
        starts.push(start as f64 / f64::from(rate));
        let gain = if beat % 4 == 0 { 0.9 } else { 0.5 };
        for n in 0..(0.01 * f64::from(rate)) as usize {
            let t = n as f64 / f64::from(rate);
            let s = gain * (-t / 0.002).exp() * (TAU * 1_500.0 * t).sin();
            if let Some(slot) = samples.get_mut(start + n) {
                *slot = s as f32;
            }
        }
    }
    (samples, starts)
}

/// A chord progression, mono: each chord's MIDI notes held for `seconds`,
/// each note a sine with its second harmonic 6 dB down, with 10 ms fades
/// at each chord's edges.
#[must_use]
pub fn progression(chords: &[&[u8]], seconds: f64, rate: u32) -> Vec<f32> {
    let frames = (seconds * f64::from(rate)) as usize;
    let mut samples = Vec::with_capacity(frames * chords.len());
    for chord in chords {
        for n in 0..frames {
            let t = n as f64 / f64::from(rate);
            let edge = (n.min(frames - 1 - n) as f64 / (0.01 * f64::from(rate))).min(1.0);
            let s: f64 = chord
                .iter()
                .map(|&note| {
                    let hz = 440.0 * 2f64.powf((f64::from(note) - 69.0) / 12.0);
                    let w = TAU * hz * t;
                    w.sin() + 0.5 * (2.0 * w).sin()
                })
                .sum();
            samples.push((0.12 * edge * s) as f32);
        }
    }
    samples
}

/// I–IV–V–I in C major, with the bass: MIDI notes for [`progression`].
pub const C_MAJOR_CADENCE: [&[u8]; 4] = [
    &[48, 60, 64, 67],
    &[41, 60, 65, 69],
    &[43, 59, 62, 67],
    &[48, 60, 64, 67],
];

/// i–iv–V–i in A minor, the V with its G sharp: MIDI notes for
/// [`progression`].
pub const A_MINOR_CADENCE: [&[u8]; 4] = [
    &[45, 57, 60, 64],
    &[50, 57, 62, 65],
    &[40, 56, 59, 64],
    &[45, 57, 60, 64],
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Audio;

    #[test]
    fn pink_noise_has_equal_power_in_every_octave() {
        let noise = pink_noise(-20.0, 10.0, 48_000, 7);
        let audio = Audio::new(&noise, 1, 48_000).unwrap();
        let rms = crate::amplitude_db(crate::levels::measure(&audio).rms_all).unwrap();
        assert!((rms + 20.0).abs() < 0.01, "{rms}");
        let bands = crate::spectrum::octave_bands(&audio);
        let levels: Vec<f64> = bands
            .iter()
            .map(|b| crate::power_db(b.power).unwrap())
            .collect();
        // Ten octave bands hold the power from 22 Hz up, so each holds
        // about a tenth of it: 10 dB under the RMS level.
        for (band, level) in bands.iter().zip(&levels).skip(1) {
            assert!(
                (level - (rms - 10.0)).abs() < 1.0,
                "{}: {level}",
                band.centre
            );
        }
    }

    #[test]
    fn the_same_seed_makes_the_same_noise() {
        assert_eq!(
            pink_noise(-20.0, 0.5, 48_000, 3),
            pink_noise(-20.0, 0.5, 48_000, 3)
        );
        assert_ne!(
            pink_noise(-20.0, 0.5, 48_000, 3),
            pink_noise(-20.0, 0.5, 48_000, 4)
        );
    }
}
