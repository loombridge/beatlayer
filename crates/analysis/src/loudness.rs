//! Loudness and true peak, as ITU-R BS.1770-4 measures them and EBU R 128
//! reports them.
//!
//! - **K-weighting.** Each channel goes through BS.1770's two filters: a
//!   high shelf of about +4 dB above 1.5 kHz (the head's effect) and a
//!   high-pass at about 38 Hz (the revised low-frequency B-curve). The
//!   coefficients are made for any rate from the filters' analogue
//!   parameters, which give BS.1770's own coefficients at 48 kHz.
//! - **Blocks.** The weighted signal's mean square is taken over 100 ms
//!   steps. A 400 ms block (momentary) or a 3 s block (short-term) sums the
//!   channels' mean squares, each weighted 1 (left, right, and centre;
//!   Beatlayer's engine renders two channels), and reads `−0.691 + 10 log10`
//!   of that, in LUFS. Blocks start every 100 ms.
//! - **Integrated loudness** gates the 400 ms blocks twice: first at −70
//!   LUFS, then at 10 LU below the loudness of what passed, and reads the
//!   loudness of what passes both.
//! - **Loudness range** (EBU Tech 3342) gates the short-term blocks at −70
//!   LUFS and then at 20 LU below their integrated loudness, and is the
//!   distance between the 10th and the 95th percentiles of what's left.
//! - **True peak** is the largest magnitude of the signal oversampled to at
//!   least 192 kHz (four times at 48 kHz, as BS.1770's Annex 2 asks) through
//!   a Kaiser-windowed sinc interpolator that passes the original samples
//!   unchanged, so it's never below the sample peak.

use crate::Audio;

/// BS.1770's offset, which makes a 1 kHz sine read its level.
const OFFSET: f64 = -0.691;

/// The lowest rate the K-weighting shelf (centred at 1.68 kHz) is stable at:
/// below it the shelf lies above Nyquist, so nothing is measured.
const MIN_RATE: u32 = 4_000;

/// The absolute gate, in LUFS.
pub const ABSOLUTE_GATE: f64 = -70.0;

/// The integrated loudness's relative gate, in LU below the loudness of
/// the blocks above the absolute gate.
pub const RELATIVE_GATE: f64 = -10.0;

/// The loudness range's relative gate, in LU.
pub const RANGE_GATE: f64 = -20.0;

/// The interpolator's half-width in original samples, each side.
const HALF_TAPS: usize = 24;

/// The Kaiser window's shape.
const KAISER_BETA: f64 = 8.0;

/// A second-order section, as the two K-weighting filters are, run in
/// double precision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    /// The numerator: b0, b1, b2.
    pub b: [f64; 3],
    /// The denominator without its leading 1: a1, a2.
    pub a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self { b, a, z: [0.0; 2] }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// BS.1770's two K-weighting stages at `rate`: the high shelf, then the
/// high-pass.
#[must_use]
pub fn k_weighting(rate: u32) -> [Biquad; 2] {
    let rate = f64::from(rate);
    // Stage 1, the shelf: its centre, gain in dB, and Q.
    let (f0, gain, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(gain / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::new(
        [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    // Stage 2, the high-pass: its corner and Q.
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad::new(
        [1.0, -2.0, 1.0],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    [shelf, high_pass]
}

/// What [`measure`] found. Loudness is `None` where there's too little
/// audio for its window, or nothing passes its gates (silence).
#[derive(Clone, Debug, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, in LUFS: the whole audio, gated.
    pub integrated: Option<f64>,
    /// The loudest 400 ms block, in LUFS.
    pub momentary_max: Option<f64>,
    /// The loudest 3 s block, in LUFS.
    pub short_term_max: Option<f64>,
    /// Every 3 s block's loudness, a block every 100 ms: when it ends, in
    /// seconds from the start, and its loudness in LUFS (minus infinity for
    /// silence).
    pub short_term: Vec<(f64, f64)>,
    /// Loudness range, in LU.
    pub range: Option<f64>,
    /// True peak, in dBTP: `None` for silence.
    pub true_peak: Option<f64>,
}

/// Measures the loudness and true peak of `audio`.
#[must_use]
pub fn measure(audio: &Audio<'_>) -> Loudness {
    let steps = if audio.rate() < MIN_RATE {
        Vec::new()
    } else {
        step_powers(audio)
    };
    let momentary = blocks(&steps, 4);
    let short = blocks(&steps, 30);
    let lufs = |power: f64| {
        if power > 0.0 {
            OFFSET + 10.0 * power.log10()
        } else {
            f64::NEG_INFINITY
        }
    };
    let max = |powers: &[f64]| {
        powers
            .iter()
            .copied()
            .fold(None, |max: Option<f64>, p| {
                Some(max.map_or(p, |m| m.max(p)))
            })
            .map(lufs)
            .filter(|l| l.is_finite())
    };
    let short_term: Vec<(f64, f64)> = short
        .iter()
        .enumerate()
        .map(|(j, &p)| ((j + 30) as f64 * 0.1, lufs(p)))
        .collect();
    Loudness {
        integrated: gated(&momentary, RELATIVE_GATE).map(lufs),
        momentary_max: max(&momentary),
        short_term_max: max(&short),
        range: range(&short),
        true_peak: true_peak(audio).and_then(crate::amplitude_db),
        short_term,
    }
}

/// Each 100 ms step's K-weighted sum of squares over the channels, and how
/// many frames it holds. A last, partial step is dropped.
fn step_powers(audio: &Audio<'_>) -> Vec<(f64, usize)> {
    let rate = f64::from(audio.rate());
    let frames = audio.frames();
    let bound = |j: usize| ((j as f64 * rate / 10.0).round() as usize).min(frames);
    let count = (frames as f64 / (rate / 10.0)).floor() as usize;
    let mut steps: Vec<(f64, usize)> = (0..count).map(|j| (0.0, bound(j + 1) - bound(j))).collect();
    for channel in 0..audio.channels() {
        let [mut shelf, mut high_pass] = k_weighting(audio.rate());
        let mut samples = audio.channel(channel);
        for (j, step) in steps.iter_mut().enumerate() {
            for _ in bound(j)..bound(j + 1) {
                let x = f64::from(samples.next().unwrap_or(0.0));
                let y = high_pass.process(shelf.process(x));
                step.0 += y * y;
            }
        }
    }
    steps
}

/// The power (the channels' summed mean squares) of each block of `width`
/// steps, a block starting at every step.
fn blocks(steps: &[(f64, usize)], width: usize) -> Vec<f64> {
    steps
        .windows(width)
        .map(|block| {
            let (sum, frames) = block
                .iter()
                .fold((0.0, 0), |(s, n), &(power, count)| (s + power, n + count));
            if frames == 0 {
                0.0
            } else {
                sum / frames as f64
            }
        })
        .collect()
}

/// The mean power of the blocks that pass the absolute gate and then the
/// relative gate `relative` LU below the mean of those.
fn gated(powers: &[f64], relative: f64) -> Option<f64> {
    let absolute = 10f64.powf((ABSOLUTE_GATE - OFFSET) / 10.0);
    let passed: Vec<f64> = powers.iter().copied().filter(|&p| p > absolute).collect();
    if passed.is_empty() {
        return None;
    }
    let mean = passed.iter().sum::<f64>() / passed.len() as f64;
    let threshold = mean * 10f64.powf(relative / 10.0);
    let kept: Vec<f64> = passed.into_iter().filter(|&p| p > threshold).collect();
    (!kept.is_empty()).then(|| kept.iter().sum::<f64>() / kept.len() as f64)
}

/// EBU Tech 3342's loudness range from the short-term blocks' powers.
fn range(short: &[f64]) -> Option<f64> {
    let absolute = 10f64.powf((ABSOLUTE_GATE - OFFSET) / 10.0);
    let passed: Vec<f64> = short.iter().copied().filter(|&p| p > absolute).collect();
    if passed.is_empty() {
        return None;
    }
    let mean = passed.iter().sum::<f64>() / passed.len() as f64;
    let threshold = mean * 10f64.powf(RANGE_GATE / 10.0);
    let mut kept: Vec<f64> = passed
        .into_iter()
        .filter(|&p| p > threshold)
        .map(|p| OFFSET + 10.0 * p.log10())
        .collect();
    if kept.len() < 2 {
        return None;
    }
    kept.sort_by(f64::total_cmp);
    Some(percentile(&kept, 0.95) - percentile(&kept, 0.10))
}

/// The `p` quantile of sorted values, interpolated between neighbours.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let at = p * (sorted.len() - 1) as f64;
    let below = at.floor() as usize;
    let above = (below + 1).min(sorted.len() - 1);
    sorted[below] + (sorted[above] - sorted[below]) * (at - below as f64)
}

/// The oversampling factor that brings `rate` to at least 192 kHz.
#[must_use]
pub fn oversampling(rate: u32) -> usize {
    (192_000_u32.div_ceil(rate.max(1))).max(1) as usize
}

/// The zeroth-order modified Bessel function of the first kind, by its
/// power series, for the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..64 {
        term *= half / k as f64;
        let add = term * term;
        sum += add;
        if add < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// The interpolator's coefficients: for each phase `p` from 1 to
/// `factor − 1`, the weights of the `2 × HALF_TAPS` samples around a point
/// `p / factor` of the way to the next sample.
fn interpolator(factor: usize) -> Vec<Vec<f64>> {
    let half = HALF_TAPS as f64;
    let norm = bessel_i0(KAISER_BETA);
    (1..factor)
        .map(|p| {
            let fraction = p as f64 / factor as f64;
            (0..2 * HALF_TAPS)
                .map(|j| {
                    // Sample `n − HALF_TAPS + 1 + j` is this far behind the
                    // point.
                    let d = fraction + half - 1.0 - j as f64;
                    let x = d / half;
                    if x.abs() >= 1.0 {
                        return 0.0;
                    }
                    let window = bessel_i0(KAISER_BETA * (1.0 - x * x).sqrt()) / norm;
                    let pd = std::f64::consts::PI * d;
                    let sinc = if d == 0.0 { 1.0 } else { pd.sin() / pd };
                    sinc * window
                })
                .collect()
        })
        .collect()
}

/// The largest magnitude of `audio` oversampled to at least 192 kHz, in
/// every channel, or `None` for silence.
#[must_use]
pub fn true_peak(audio: &Audio<'_>) -> Option<f64> {
    let factor = oversampling(audio.rate());
    let phases = interpolator(factor);
    let mut peak = 0.0f64;
    for channel in 0..audio.channels() {
        let x: Vec<f64> = audio.channel(channel).map(f64::from).collect();
        let at = |i: isize| {
            usize::try_from(i)
                .ok()
                .and_then(|i| x.get(i))
                .copied()
                .unwrap_or(0.0)
        };
        for (n, &sample) in x.iter().enumerate() {
            peak = peak.max(sample.abs());
            for taps in &phases {
                let first = n as isize - HALF_TAPS as isize + 1;
                let mut y = 0.0;
                for (j, &c) in taps.iter().enumerate() {
                    y += c * at(first + j as isize);
                }
                peak = peak.max(y.abs());
            }
        }
    }
    (peak > 0.0).then_some(peak)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{fade, level, sine};

    fn loudness(samples: &[f32]) -> Loudness {
        measure(&Audio::new(samples, 2, 48_000).unwrap())
    }

    #[test]
    fn the_filters_at_48_khz_are_bs_1770_s() {
        // BS.1770-4, Table 1 and Table 2.
        let [shelf, high_pass] = k_weighting(48_000);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-8;
        for (got, want) in shelf.b.iter().zip([
            1.535_124_859_586_97,
            -2.691_696_189_406_38,
            1.198_392_810_852_85,
        ]) {
            assert!(close(*got, want), "{got} vs {want}");
        }
        for (got, want) in shelf
            .a
            .iter()
            .zip([-1.690_659_293_182_41, 0.732_480_774_215_85])
        {
            assert!(close(*got, want), "{got} vs {want}");
        }
        assert_eq!(high_pass.b, [1.0, -2.0, 1.0]);
        for (got, want) in high_pass
            .a
            .iter()
            .zip([-1.990_047_454_833_98, 0.990_072_250_366_21])
        {
            assert!(close(*got, want), "{got} vs {want}");
        }
    }

    #[test]
    fn ebu_tech_3341_s_steady_and_gated_sines_read_their_level() {
        // EBU Tech 3341, the minimum-requirements test signals 1 to 5:
        // stereo 1 kHz sines, at levels in dBFS, each segment `seconds`
        // long. Each reads −23.0 LUFS (−33.0 for the second) ± 0.1 LU.
        let cases: [(&[(f64, f64)], f64); 5] = [
            (&[(-23.0, 20.0)], -23.0),
            (&[(-33.0, 20.0)], -33.0),
            (&[(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)], -23.0),
            (
                &[
                    (-72.0, 10.0),
                    (-36.0, 10.0),
                    (-23.0, 60.0),
                    (-36.0, 10.0),
                    (-72.0, 10.0),
                ],
                -23.0,
            ),
            (&[(-26.0, 20.0), (-20.0, 20.1), (-26.0, 20.0)], -23.0),
        ];
        for (segments, expected) in cases {
            let mut samples = Vec::new();
            for &(db, seconds) in segments {
                samples.extend(sine(1_000.0, level(db), seconds, 48_000, 2, 0.0));
            }
            let got = loudness(&samples).integrated.unwrap();
            assert!((got - expected).abs() <= 0.1, "{segments:?}: {got} LUFS");
        }
    }

    #[test]
    fn short_term_and_momentary_follow_their_windows() {
        // A steady −23 dBFS sine reads −23 LUFS in every full window.
        let samples = sine(1_000.0, level(-23.0), 10.0, 48_000, 2, 0.0);
        let measured = loudness(&samples);
        assert!((measured.momentary_max.unwrap() + 23.0).abs() <= 0.1);
        assert!((measured.short_term_max.unwrap() + 23.0).abs() <= 0.1);
        // 10 s holds blocks ending at 3.0 s, 3.1 s, ... 10.0 s.
        assert_eq!(measured.short_term.len(), 71);
        assert!((measured.short_term[0].0 - 3.0).abs() < 1e-9);
        for (_, l) in &measured.short_term {
            assert!((l + 23.0).abs() <= 0.1, "{l}");
        }
    }

    #[test]
    fn ebu_tech_3342_s_first_signal_has_a_range_of_10_lu() {
        // EBU Tech 3342, test signal 1: 20 s at −20 dBFS then 20 s at −30
        // dBFS, a 1 kHz stereo sine. Loudness range 10 LU ± 1.
        let mut samples = sine(1_000.0, level(-20.0), 20.0, 48_000, 2, 0.0);
        samples.extend(sine(1_000.0, level(-30.0), 20.0, 48_000, 2, 0.0));
        let range = loudness(&samples).range.unwrap();
        assert!((range - 10.0).abs() <= 1.0, "{range} LU");
    }

    #[test]
    fn true_peak_finds_peaks_between_samples() {
        // Sines whose samples miss their peaks, as EBU Tech 3341's
        // true-peak signals do, each with a true peak of −6 dBFS: fs/4 at
        // 45°, fs/6 at 60°, fs/8 at 67.5°, and fs/4 on its peaks. Tech
        // 3341 allows +0.2 and −0.4 dB.
        for (divisor, degrees) in [(4.0, 0.0), (4.0, 45.0), (6.0, 60.0), (8.0, 67.5)] {
            let hz = 48_000.0 / divisor;
            let mut samples = sine(
                hz,
                level(-6.0),
                1.0,
                48_000,
                2,
                f64::to_radians(degrees) + std::f64::consts::FRAC_PI_2,
            );
            // Faded in and out over 50 ms, so the edges add no overshoot
            // of their own.
            fade(&mut samples, 2, 2_400);
            let peak = loudness(&samples).true_peak.unwrap();
            assert!(
                (-6.4..=-5.8).contains(&peak),
                "fs/{divisor} at {degrees}°: {peak} dBTP"
            );
            // Tighter than the standard asks, for the interpolator here.
            assert!(
                (peak + 6.0).abs() < 0.05,
                "fs/{divisor} at {degrees}°: {peak}"
            );
        }
    }

    #[test]
    fn silence_has_no_loudness_or_peak() {
        let samples = vec![0.0; 48_000 * 2 * 5];
        let measured = loudness(&samples);
        assert_eq!(measured.integrated, None);
        assert_eq!(measured.momentary_max, None);
        assert_eq!(measured.range, None);
        assert_eq!(measured.true_peak, None);
        // Under 400 ms, there's no block at all.
        let short = sine(1_000.0, 0.5, 0.3, 48_000, 2, 0.0);
        assert_eq!(loudness(&short).integrated, None);
    }

    #[test]
    fn a_rate_too_low_for_the_weighting_has_no_loudness() {
        let samples = sine(100.0, 0.5, 5.0, 1_000, 2, 0.0);
        let measured = measure(&Audio::new(&samples, 2, 1_000).unwrap());
        assert_eq!(measured.integrated, None);
        assert_eq!(measured.momentary_max, None);
        assert_eq!(measured.short_term_max, None);
        assert!(measured.short_term.is_empty());
    }

    #[test]
    fn other_rates_read_the_same() {
        for rate in [44_100, 96_000] {
            let samples = sine(1_000.0, level(-23.0), 10.0, rate, 2, 0.0);
            let measured = measure(&Audio::new(&samples, 2, rate).unwrap());
            let got = measured.integrated.unwrap();
            assert!((got + 23.0).abs() <= 0.1, "{rate} Hz: {got}");
        }
    }
}
