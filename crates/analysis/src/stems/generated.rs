//! Mixes made from known sources, and the measure of how well a separation
//! finds them again (stem separation spec R32, R36).
//!
//! A mix is four generated parts, drums, bass, others and vocals, each as
//! stereo audio at [`RATE`], summed. The same seed makes the same mix on
//! every platform.

use super::{RATE, Stem, Stems};

/// A mix and the parts it was made of.
pub struct Generated {
    /// The sum of the parts, planar stereo.
    pub mix: [Vec<f32>; 2],
    /// The parts, each planar stereo, in [`Stem::ALL`]'s order.
    pub sources: [[Vec<f32>; 2]; 4],
}

/// A small deterministic noise source.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// A generated mix of `seconds`, at 120 beats a minute, made from `seed`.
#[must_use]
pub fn mix(seconds: f64, seed: u32) -> Generated {
    let frames = (seconds * f64::from(RATE)) as usize;
    let rate = f64::from(RATE);
    let mut noise = Noise(seed.wrapping_mul(2_654_435_761).wrapping_add(1));
    let tau = std::f64::consts::TAU;
    // Each part's left and right gains, and its sample at `t` seconds.
    let pans = [[0.9, 0.9], [0.8, 0.8], [0.7, 0.45], [0.55, 0.75]];
    let mut sources: [[Vec<f32>; 2]; 4] =
        std::array::from_fn(|_| [Vec::with_capacity(frames), Vec::with_capacity(frames)]);
    let mut previous = 0.0f32;
    for i in 0..frames {
        let t = i as f64 / rate;
        let beat = t * 2.0;
        let (in_beat, number) = (beat.fract() / 2.0, beat as usize);
        // Drums: a kick on every beat, a snare on the second and fourth,
        // a hat on every half beat.
        let kick = (-in_beat * 14.0).exp()
            * (tau * (50.0 * in_beat + 70.0 * (1.0 - (-in_beat * 30.0).exp()) / 30.0)).sin();
        let snare = if number % 2 == 1 {
            (-in_beat * 22.0).exp()
                * (0.6 * f64::from(noise.next()) + 0.3 * (tau * 190.0 * t).sin())
        } else {
            0.0
        };
        let raw = noise.next();
        let hat = (-(in_beat % 0.25) * 90.0).exp() * f64::from(raw - previous) * 0.25;
        previous = raw;
        let drums = 0.6 * kick + 0.35 * snare + hat;
        // Bass: a plucked note per beat, on a four-note line.
        let root = [55.0, 55.0, 65.4, 49.0][(number / 2) % 4];
        let pluck = (-in_beat * 3.0).exp();
        let bass = 0.45 * pluck * ((tau * root * t).sin() + 0.4 * (tau * 2.0 * root * t).sin());
        // Others: a three-note chord that changes every four beats.
        let chord = [[220.0, 277.2, 329.6], [196.0, 246.9, 293.7]][(number / 4) % 2];
        let others: f64 = chord
            .iter()
            .map(|f| (tau * f * t).sin() + 0.3 * (tau * 3.0 * f * t).sin())
            .sum::<f64>()
            * 0.07;
        // Vocals: harmonics of a gliding pitch weighted by two formants,
        // sung for three beats in every four.
        let pitch = 300.0 + 40.0 * (tau * 5.0 * t).sin() + 25.0 * (t * 0.5).floor() % 4.0;
        let gate = if number % 4 < 3 { 1.0 } else { 0.0 };
        let vocals: f64 = (1..=12)
            .map(|h| {
                let f = pitch * f64::from(h);
                let weight = (-((f - 700.0) / 300.0).powi(2)).exp()
                    + 0.6 * (-((f - 1800.0) / 400.0).powi(2)).exp();
                weight * (tau * f * t).sin() / f64::from(h).sqrt()
            })
            .sum::<f64>()
            * 0.12
            * gate;
        for (stem, value) in [drums, bass, others, vocals].into_iter().enumerate() {
            for (channel, source) in sources[stem].iter_mut().enumerate() {
                source.push((value * pans[stem][channel]) as f32);
            }
        }
    }
    // Scaled so the mix peaks at 0.9, the parts by the same amount.
    let peak = (0..frames)
        .flat_map(|i| (0..2).map(move |c| (i, c)))
        .map(|(i, c)| sources.iter().map(|s| s[c][i]).sum::<f32>().abs())
        .fold(1e-6, f32::max);
    let gain = 0.9 / peak;
    for stem in &mut sources {
        for channel in stem {
            for x in channel {
                *x *= gain;
            }
        }
    }
    let mix = std::array::from_fn(|c| {
        (0..frames)
            .map(|i| sources.iter().map(|s| s[c][i]).sum())
            .collect()
    });
    Generated { mix, sources }
}

/// The signal-to-distortion ratio of `estimate` against the true `source`,
/// in decibels, over both channels (Vincent et al. 2006, without the
/// allowed distortions): how far the part's energy is above the error's.
#[must_use]
pub fn sdr(estimate: &[Vec<f32>; 2], source: &[Vec<f32>; 2]) -> f64 {
    let (mut signal, mut error) = (0.0f64, 1e-12f64);
    for (e, s) in estimate.iter().zip(source) {
        for (e, s) in e.iter().zip(s) {
            signal += f64::from(*s) * f64::from(*s);
            error += f64::from(e - s) * f64::from(e - s);
        }
    }
    10.0 * (signal.max(1e-12) / error).log10()
}

impl Generated {
    /// How well `stems` find each part, in decibels, in [`Stem::ALL`]'s
    /// order.
    #[must_use]
    pub fn scores(&self, stems: &Stems) -> [f64; 4] {
        Stem::ALL.map(|stem| sdr(stems.get(stem), &self.sources[stem.index()]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_makes_the_same_mix_and_the_parts_sum_to_it() {
        let (a, b) = (mix(2.0, 7), mix(2.0, 7));
        assert_eq!(a.mix, b.mix);
        assert_ne!(a.mix, mix(2.0, 8).mix);
        let frames = 2 * RATE as usize;
        assert_eq!(a.mix[0].len(), frames);
        let peak = a.mix.iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - 0.9).abs() < 1e-3, "{peak}");
        for i in [0, 1000, frames - 1] {
            let sum: f32 = a.sources.iter().map(|s| s[1][i]).sum();
            assert!((sum - a.mix[1][i]).abs() < 1e-5);
        }
        assert!(a.mix.iter().flatten().all(|x| x.is_finite()));
    }

    #[test]
    fn the_measure_is_infinite_for_the_source_and_zero_for_silence() {
        let g = mix(1.0, 1);
        let drums = &g.sources[Stem::Drums.index()];
        assert!(sdr(drums, drums) > 100.0);
        let silence = [vec![0.0; drums[0].len()], vec![0.0; drums[0].len()]];
        assert!(sdr(&silence, drums).abs() < 1e-3);
    }
}
