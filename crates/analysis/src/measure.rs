//! Measurements of what a warp mode or an interpolator does to a signal
//! (warp-modes R34 to R36): the pitch of a steady tone, the first two
//! formants of a vowel, and a swept tone's level against everything else.
//!
//! Each reads the channels' average, and says its method and units below.

use realfft::RealFftPlanner;

use crate::Audio;

/// Zero padding of the pitch's FFT, as a multiple of its window, which puts
/// the interpolated peak on a bin grid a quarter of a hertz apart.
const PITCH_PADDING: usize = 4;

/// The top of the band the formants are fitted in, in hertz: above the third
/// formant of a voice transposed up an octave.
const FORMANT_BAND: f64 = 6_000.0;

/// The Kaiser window's shape for [`sweep_levels`]: sidelobes below −130 dB,
/// a main lobe about five bins either side.
const KAISER_BETA: f64 = 14.0;

/// The bins [`sweep_levels`] leaves either side of the tone's band, beyond
/// the Kaiser window's main lobe.
const SWEEP_MARGIN_BINS: f64 = 10.0;

/// The pitch of a steady tone, in hertz: the frequency of the strongest
/// peak in the spectrum of each one-second Hann-windowed frame (a half second
/// apart, zero-padded fourfold), refined by a parabola through the peak and
/// its neighbours' log magnitudes, and the median of the frames'. `None` for
/// audio shorter than a second or silent.
#[must_use]
pub fn pitch(audio: &Audio<'_>) -> Option<f64> {
    let mono = audio.mono();
    let rate = f64::from(audio.rate());
    let size = audio.rate() as usize;
    if mono.len() < size {
        return None;
    }
    let padded = size * PITCH_PADDING;
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(padded);
    let (mut input, mut output, mut scratch) = (
        fft.make_input_vec(),
        fft.make_output_vec(),
        fft.make_scratch_vec(),
    );
    let window = hann(size);
    let mut found = Vec::new();
    for start in (0..=mono.len() - size).step_by(size / 2) {
        input.fill(0.0);
        for ((slot, &s), w) in input
            .iter_mut()
            .zip(&mono[start..start + size])
            .zip(&window)
        {
            *slot = s * w;
        }
        fft.process_with_scratch(&mut input, &mut output, &mut scratch)
            .expect("the buffers are the plan's own");
        // From 10 Hz up: below it is a drift, not a pitch.
        let first = (10.0 * padded as f64 / rate).ceil() as usize;
        let (peak, magnitude) = output
            .iter()
            .enumerate()
            .skip(first)
            .map(|(k, v)| (k, v.norm()))
            .fold(
                (0, 0.0),
                |best, now| if now.1 > best.1 { now } else { best },
            );
        if magnitude <= 0.0 || peak == 0 || peak + 1 >= output.len() {
            continue;
        }
        let at = |k: usize| output[k].norm().max(f64::MIN_POSITIVE).ln();
        let (a, b, c) = (at(peak - 1), at(peak), at(peak + 1));
        let denominator = a - 2.0 * b + c;
        let offset = if denominator < 0.0 {
            0.5 * (a - c) / denominator
        } else {
            0.0
        };
        found.push((peak as f64 + offset) * rate / padded as f64);
    }
    found.sort_by(f64::total_cmp);
    found.get(found.len() / 2).copied()
}

/// How far below a harmonic tone's harmonics everything else in it lies, in
/// dB (negative when the harmonics dominate): the power of one Hann-windowed
/// frame of the first second, from 20 Hz to the Nyquist frequency, outside
/// four bins (4 Hz) either side of every multiple of `fundamental_hz`, against
/// the power inside them. `None` for audio shorter than a second or silent.
#[must_use]
pub fn non_harmonic_db(audio: &Audio<'_>, fundamental_hz: f64) -> Option<f64> {
    let mono = audio.mono();
    let size = audio.rate() as usize;
    if mono.len() < size || fundamental_hz <= 0.0 {
        return None;
    }
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(size);
    let mut input: Vec<f64> = mono[..size]
        .iter()
        .zip(hann(size))
        .map(|(s, w)| s * w)
        .collect();
    let mut output = fft.make_output_vec();
    fft.process(&mut input, &mut output)
        .expect("the buffers are the plan's own");
    // One bin is one hertz.
    let (mut harmonic, mut other) = (0.0, 0.0);
    for (bin, v) in output.iter().enumerate().skip(20) {
        let hz = bin as f64;
        let harmonics = (hz / fundamental_hz).round().max(1.0);
        if (hz - harmonics * fundamental_hz).abs() <= 4.0 {
            harmonic += v.norm_sqr();
        } else {
            other += v.norm_sqr();
        }
    }
    Some(crate::power_db(other).unwrap_or(-300.0) - crate::power_db(harmonic)?)
}

/// The first two formants of a vowel, in hertz: the first two peaks of its
/// spectral envelope, which is the all-pole fit (linear prediction, order 10,
/// by the Levinson-Durbin recursion) to the audio's average power spectrum
/// below 6 kHz (the Hann-windowed 8,192-point power spectrum of half-
/// overlapping frames), evaluated every 1.5 Hz. `None` if the envelope has
/// fewer than two peaks.
#[must_use]
pub fn formants(audio: &Audio<'_>) -> Option<[f64; 2]> {
    const SIZE: usize = 8_192;
    const ORDER: usize = 10;
    let mono = audio.mono();
    let rate = f64::from(audio.rate());
    if mono.len() < SIZE {
        return None;
    }
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(SIZE);
    let (mut input, mut output, mut scratch) = (
        fft.make_input_vec(),
        fft.make_output_vec(),
        fft.make_scratch_vec(),
    );
    let window = hann(SIZE);
    let mut power = vec![0.0; SIZE / 2 + 1];
    for start in (0..=mono.len() - SIZE).step_by(SIZE / 2) {
        for ((slot, &s), w) in input
            .iter_mut()
            .zip(&mono[start..start + SIZE])
            .zip(&window)
        {
            *slot = s * w;
        }
        fft.process_with_scratch(&mut input, &mut output, &mut scratch)
            .expect("the buffers are the plan's own");
        for (sum, v) in power.iter_mut().zip(&output) {
            *sum += v.norm_sqr();
        }
    }
    // The autocorrelation of the spectrum cut at 6 kHz, as of a signal
    // sampled at 12 kHz: a cosine transform of the power.
    let top = (FORMANT_BAND * SIZE as f64 / rate) as usize;
    let correlation: Vec<f64> = (0..=ORDER)
        .map(|lag| {
            power[..=top]
                .iter()
                .enumerate()
                .map(|(bin, p)| {
                    let edge = if bin == 0 || bin == top { 0.5 } else { 1.0 };
                    edge * p * (std::f64::consts::PI * (bin * lag) as f64 / top as f64).cos()
                })
                .sum()
        })
        .collect();
    let mut predictor = vec![0.0; ORDER + 1];
    predictor[0] = 1.0;
    let mut error = correlation[0];
    for i in 1..=ORDER {
        let k = -(correlation[i]
            + (1..i)
                .map(|m| predictor[m] * correlation[i - m])
                .sum::<f64>())
            / error;
        let before = predictor.clone();
        for m in 1..i {
            predictor[m] = before[m] + k * before[i - m];
        }
        predictor[i] = k;
        error *= 1.0 - k * k;
    }
    let points = top * 4;
    let envelope: Vec<f64> = (0..points)
        .map(|n| {
            let angle = std::f64::consts::PI * n as f64 / points as f64;
            let (re, im) = predictor
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(re, im), (m, a)| {
                    (
                        re + a * (angle * m as f64).cos(),
                        im - a * (angle * m as f64).sin(),
                    )
                });
            1.0 / (re * re + im * im)
        })
        .collect();
    let mut peaks = (1..points - 1)
        .filter(|&n| envelope[n] > envelope[n - 1] && envelope[n] >= envelope[n + 1])
        .map(|n| n as f64 * FORMANT_BAND / points as f64);
    Some([peaks.next()?, peaks.next()?])
}

/// What one frame of a swept tone holds, as [`sweep_levels`] measures it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepFrame {
    /// The frame's centre, in seconds from the start of the audio.
    pub seconds: f64,
    /// The frequency the tone should have at the frame's centre, in hertz;
    /// above the Nyquist frequency, there is no tone to find.
    pub expected_hz: f64,
    /// The mean square, in dB relative to full scale, of what lies within the
    /// frequencies the tone crosses in the frame; `None` where none does.
    pub tone_db: Option<f64>,
    /// The mean square, in dB relative to full scale, of everything else in
    /// the frame (the aliases, the noise and the distortion); `None` for
    /// exact silence.
    pub other_db: Option<f64>,
}

/// A swept tone's level against the rest of what is in the output, frame
/// by frame. The audio is cut into 8,192-point frames a quarter of a frame
/// apart, each under a Kaiser window (β 14) and transformed; `expected_hz`
/// gives the tone's frequency at a time in seconds, and the tone is whatever
/// lies from the frequency at the frame's start to the one at its end, ten
/// bins wider either side. Everything else in the spectrum, 0 Hz to the
/// Nyquist frequency, is the "other" level; where the expected frequency is
/// above the Nyquist frequency, all of the frame is other. Both levels are
/// mean squares in dB, scaled so a full-scale sine reads −3.01 dB.
#[must_use]
pub fn sweep_levels(audio: &Audio<'_>, expected_hz: impl Fn(f64) -> f64) -> Vec<SweepFrame> {
    const SIZE: usize = 8_192;
    let mono = audio.mono();
    let rate = f64::from(audio.rate());
    let nyquist = rate / 2.0;
    let window = kaiser(SIZE, KAISER_BETA);
    let window_power: f64 = window.iter().map(|w| w * w).sum();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(SIZE);
    let (mut input, mut output, mut scratch) = (
        fft.make_input_vec(),
        fft.make_output_vec(),
        fft.make_scratch_vec(),
    );
    let bin_hz = rate / SIZE as f64;
    let mut frames = Vec::new();
    if mono.len() < SIZE {
        return frames;
    }
    for start in (0..=mono.len() - SIZE).step_by(SIZE / 4) {
        for ((slot, &s), w) in input
            .iter_mut()
            .zip(&mono[start..start + SIZE])
            .zip(&window)
        {
            *slot = s * w;
        }
        fft.process_with_scratch(&mut input, &mut output, &mut scratch)
            .expect("the buffers are the plan's own");
        let seconds = (start + SIZE / 2) as f64 / rate;
        let begins = expected_hz(start as f64 / rate);
        let ends = expected_hz((start + SIZE) as f64 / rate);
        let (low, high) = (begins.min(ends), begins.max(ends));
        let margin = SWEEP_MARGIN_BINS * bin_hz;
        let (mut tone, mut other, mut in_tone) = (0.0, 0.0, false);
        for (k, v) in output.iter().enumerate() {
            let hz = k as f64 * bin_hz;
            let one_sided = if k == 0 || k == SIZE / 2 { 1.0 } else { 2.0 };
            let power = one_sided * v.norm_sqr() / (SIZE as f64 * window_power);
            if hz >= low - margin && hz <= high + margin && low <= nyquist {
                tone += power;
                in_tone = true;
            } else {
                other += power;
            }
        }
        frames.push(SweepFrame {
            seconds,
            expected_hz: expected_hz(seconds),
            tone_db: in_tone.then(|| crate::power_db(tone)).flatten(),
            other_db: crate::power_db(other),
        });
    }
    frames
}

/// A periodic Hann window of `size` points.
fn hann(size: usize) -> Vec<f64> {
    (0..size)
        .map(|n| 0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / size as f64).cos())
        .collect()
}

/// A periodic Kaiser window of `size` points and shape `beta`.
fn kaiser(size: usize, beta: f64) -> Vec<f64> {
    let denominator = bessel_i0(beta);
    (0..size)
        .map(|n| {
            let x = 2.0 * n as f64 / size as f64 - 1.0;
            bessel_i0(beta * (1.0 - x * x).max(0.0).sqrt()) / denominator
        })
        .collect()
}

/// The modified Bessel function of the first kind, order 0, by its series.
fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let (mut term, mut sum) = (1.0, 1.0);
    for k in 1..60 {
        term *= quarter / (f64::from(k) * f64::from(k));
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{level, sine};

    #[test]
    fn pitch_is_read_to_a_hundredth_of_a_cent() {
        for hz in [55.0, 110.0, 220.3, 440.0, 1_234.5, 9_000.0] {
            let samples = sine(hz, level(-6.0), 3.0, 48_000, 1, 0.3);
            let found = pitch(&Audio::new(&samples, 1, 48_000).unwrap()).unwrap();
            let cents = 1200.0 * (found / hz).log2();
            assert!(cents.abs() < 0.01, "{hz} Hz read as {found}: {cents} cents");
        }
    }

    #[test]
    fn a_pure_harmonic_tone_has_nothing_else_in_it() {
        let samples = sine(220.0, level(-6.0), 2.0, 48_000, 1, 0.0);
        let audio = Audio::new(&samples, 1, 48_000).unwrap();
        assert!(non_harmonic_db(&audio, 220.0).unwrap() < -80.0);
        assert!(non_harmonic_db(&audio, 150.0).unwrap() > 0.0);
    }

    #[test]
    fn pitch_of_silence_is_none() {
        let samples = vec![0.0; 96_000];
        assert_eq!(pitch(&Audio::new(&samples, 1, 48_000).unwrap()), None);
    }

    #[test]
    fn formants_of_a_vowel_are_its_resonators() {
        let rate = 48_000.0;
        let mut out = vec![0.0f64; 4 * 48_000];
        for (i, s) in out.iter_mut().enumerate() {
            if i % 320 < 4 {
                *s = 1.0;
            }
        }
        for (freq, bandwidth) in [(700.0, 80.0), (1_220.0, 90.0), (2_600.0, 120.0)] {
            let r = (-std::f64::consts::PI * bandwidth / rate).exp();
            let two_cos = 2.0 * r * (std::f64::consts::TAU * freq / rate).cos();
            let (mut a1, mut a2) = (0.0, 0.0);
            for s in out.iter_mut() {
                let y = (1.0 - r) * *s + two_cos * a1 - r * r * a2;
                a2 = a1;
                a1 = y;
                *s = y;
            }
        }
        let samples: Vec<f32> = out.iter().map(|&s| (s * 0.5) as f32).collect();
        let [first, second] = formants(&Audio::new(&samples, 1, 48_000).unwrap()).unwrap();
        assert!((600.0..800.0).contains(&first), "{first}");
        assert!((1_150.0..1_350.0).contains(&second), "{second}");
    }

    #[test]
    fn a_clean_sweep_reads_far_below_its_tone() {
        // 20 Hz to 20 kHz over 10 s.
        let ratio: f64 = 1000.0;
        let seconds = 10.0;
        let samples: Vec<f32> = (0..480_000)
            .map(|n| {
                let t = f64::from(n) / 48_000.0;
                let phase = std::f64::consts::TAU * 20.0 * seconds / ratio.ln()
                    * (ratio.powf(t / seconds) - 1.0);
                (0.5 * phase.sin()) as f32
            })
            .collect();
        let frames = sweep_levels(&Audio::new(&samples, 1, 48_000).unwrap(), |t| {
            20.0 * ratio.powf(t / seconds)
        });
        for frame in frames
            .iter()
            .filter(|f| (100.0..20_000.0).contains(&f.expected_hz))
        {
            let tone = frame.tone_db.unwrap();
            let other = frame.other_db.unwrap_or(-300.0);
            assert!((tone + 9.03).abs() < 0.1, "{frame:?}");
            assert!(other < tone - 100.0, "{frame:?}");
        }
    }
}
