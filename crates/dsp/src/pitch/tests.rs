//! The voice pitch tests (spec: tuner R2; the build plan's WP-8.32). The
//! material is made here: tones, and a synthetic voice, a Rosenberg glottal
//! pulse train through three formant resonators.

use std::f64::consts::{PI, TAU};

use super::*;
use crate::Random;

const RATE: f64 = 48_000.0;

/// The first three formants of the vowels, in hertz.
const A: [f64; 3] = [700.0, 1220.0, 2600.0];
const I: [f64; 3] = [270.0, 2290.0, 3010.0];
const U: [f64; 3] = [300.0, 870.0, 2240.0];
const BANDWIDTH: [f64; 3] = [120.0, 150.0, 200.0];
const LEVELS: [f64; 3] = [1.0, 0.8, 0.7];

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn sine(hz: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| (TAU * hz * i as f64 / RATE).sin() as f32 * 0.5)
        .collect()
}

/// One period of the Rosenberg glottal pulse's derivative at `phase` (0 to
/// 1): the flow rises over the first 40 % and falls over the next 16 %.
fn glottal(phase: f64) -> f64 {
    const OPEN: f64 = 0.4;
    const CLOSE: f64 = 0.16;
    if phase < OPEN {
        0.5 * PI / OPEN * (PI * phase / OPEN).sin()
    } else if phase < OPEN + CLOSE {
        -PI / (2.0 * CLOSE) * (PI * (phase - OPEN) / (2.0 * CLOSE)).sin()
    } else {
        0.0
    }
}

/// A two-pole resonator at `hz`, `bandwidth` wide, with unit gain at its peak.
struct Resonator {
    a1: f64,
    a2: f64,
    gain: f64,
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn new(hz: f64, bandwidth: f64) -> Self {
        let r = (-PI * bandwidth / RATE).exp();
        let theta = TAU * hz / RATE;
        let (a1, a2) = (2.0 * r * theta.cos(), -r * r);
        let (sin, cos) = theta.sin_cos();
        let (sin2, cos2) = (2.0 * theta).sin_cos();
        let denominator = (1.0 - a1 * cos - a2 * cos2).hypot(a1 * sin + a2 * sin2);
        Self {
            a1,
            a2,
            gain: denominator,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.gain * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// The Fourier series of [`glottal`], harmonic 1 first, as cosine and sine
/// coefficients: the pulse train made of them has no more than 100 harmonics,
/// so it never aliases, however high the voice sings.
fn glottal_series() -> Vec<(f64, f64)> {
    const STEPS: usize = 8192;
    (1..=100)
        .map(|n| {
            let (mut c, mut s) = (0.0, 0.0);
            for step in 0..STEPS {
                let phase = (step as f64 + 0.5) / STEPS as f64;
                let (sin, cos) = (TAU * f64::from(n) * phase).sin_cos();
                c += glottal(phase) * cos;
                s += glottal(phase) * sin;
            }
            (2.0 * c / STEPS as f64, 2.0 * s / STEPS as f64)
        })
        .collect()
}

/// A voice whose fundamental at sample `i` is `f0(i)` hertz, singing the
/// vowel `formants`, at −12 dBFS RMS.
fn voice(f0: impl Fn(usize) -> f64, formants: [f64; 3], len: usize) -> Vec<f32> {
    voice_scaled(f0, formants, 1.0, len)
}

/// As [`voice`], with the formants and their bandwidths `scale` times as
/// high: what moving a voice's formants by `scale` does to them.
fn voice_scaled(f0: impl Fn(usize) -> f64, formants: [f64; 3], scale: f64, len: usize) -> Vec<f32> {
    let series = glottal_series();
    let mut filters = [0, 1, 2].map(|k| Resonator::new(formants[k] * scale, BANDWIDTH[k] * scale));
    let mut phase = 0.0f64;
    let out: Vec<f64> = (0..len)
        .map(|i| {
            let harmonics = ((0.4 * RATE / f0(i)) as usize).min(series.len());
            let source: f64 = series[..harmonics]
                .iter()
                .enumerate()
                .map(|(n, (c, s))| {
                    let (sin, cos) = (TAU * (n + 1) as f64 * phase).sin_cos();
                    c * cos + s * sin
                })
                .sum();
            phase = (phase + f0(i) / RATE).fract();
            // The formants in parallel, the higher the weaker.
            filters
                .iter_mut()
                .zip(LEVELS)
                .map(|(f, level)| level * f.run(source))
                .sum()
        })
        .collect();
    let rms = (out.iter().map(|x| x * x).sum::<f64>() / len as f64).sqrt();
    out.iter().map(|x| (x * 0.25 / rms) as f32).collect()
}

fn noise(seed: u64, len: usize) -> Vec<f32> {
    let mut random = Random::seeded(seed);
    (0..len)
        .map(|_| random.next_bipolar() as f32 * 0.3)
        .collect()
}

/// The noise of an "s": white noise with everything under a few kilohertz
/// taken away.
fn fricative(len: usize) -> Vec<f32> {
    let white = noise(7, len);
    let mut low = 0.0f32;
    white
        .iter()
        .map(|&x| {
            low += (x - low) * 0.5;
            x - low
        })
        .collect()
}

/// Every pitch measurement of `input`, with the sample the result came out on.
fn track(config: &Config, input: &[f32]) -> Vec<(usize, Option<Pitch>)> {
    let mut tracker = Tracker::new(config);
    let mut found = Vec::new();
    for (i, &x) in input.iter().enumerate() {
        tracker.feed(x);
        while let Some(p) = tracker.pop() {
            found.push((i, p));
        }
    }
    found
}

/// A harmonic series with a weak fundamental: the second to the twentieth
/// harmonics at 1/n, the first at a twentieth of its share.
fn weak_fundamental(f0: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let t = i as f64 / RATE;
            let sum: f64 = (1..=20)
                .map(|n| {
                    let level = if n == 1 { 0.05 } else { 1.0 / f64::from(n) };
                    level * (TAU * f0 * f64::from(n) * t).sin()
                })
                .sum();
            (sum * 0.15) as f32
        })
        .collect()
}

// Plan: WP-8.32 (tracker)
#[test]
fn the_tracker_follows_tones_and_a_voice_from_50_hz_to_1_5_khz_within_5_cents() {
    let config = Config::for_range(RATE, 50.0, 2000.0);
    for f0 in [
        50.0, 65.4, 82.4, 110.0, 196.0, 329.6, 523.3, 880.0, 1318.5, 1500.0,
    ] {
        for (what, input) in [
            ("sine", sine(f0, 24_000)),
            ("a", voice(|_| f0, A, 24_000)),
            ("i", voice(|_| f0, I, 24_000)),
            ("u", voice(|_| f0, U, 24_000)),
        ] {
            let found = track(&config, &input);
            // From the first full window of the tone on.
            let late: Vec<_> = found.iter().filter(|(i, _)| *i > 12_000).collect();
            assert!(!late.is_empty());
            for (_, p) in late {
                let p = p.unwrap_or_else(|| panic!("{what} {f0} Hz: nothing heard"));
                assert!(p.voiced, "{what} {f0} Hz was called unvoiced");
                assert!(
                    cents(p.frequency, f0).abs() < 5.0,
                    "{what} {f0} Hz read {:.2} Hz",
                    p.frequency
                );
                assert!(p.confidence > 0.7, "{what} {f0} Hz: {}", p.confidence);
            }
        }
    }
}

/// Samples after `step` before a measurement is within `limit` cents of `to`
/// and every later one is too.
fn settled(found: &[(usize, Option<Pitch>)], step: usize, to: f64, limit: f64) -> usize {
    let near =
        |p: &Option<Pitch>| p.is_some_and(|p| p.voiced && cents(p.frequency, to).abs() < limit);
    let after: Vec<_> = found.iter().filter(|(i, _)| *i >= step).collect();
    let first = (0..after.len())
        .find(|&k| after[k..].iter().all(|(_, p)| near(p)))
        .expect("it settled");
    after[first].0 - step
}

// Plan: WP-8.32 (tracker). A tone is within 5 cents 20 ms after a step. A
// vowel's formant resonators ring on after the step, in the first formant
// for as long as its bandwidth allows (about 10 ms for the /i/ here), and the
// ringing is not the new pitch: a vowel is within 20 cents in 20 ms and
// within 5 in 30.
#[test]
fn a_step_in_pitch_is_settled_within_20_ms() {
    let config = Config::for_range(RATE, 150.0, 2000.0);
    let step = 12_000;
    let ms = |t: f64| (t * 0.001 * RATE) as usize;
    for (from, to) in [
        (200.0, 300.0),
        (300.0, 200.0),
        (440.0, 880.0),
        (1000.0, 700.0),
    ] {
        let mut phase = 0.0f64;
        let tone: Vec<f32> = (0..24_000)
            .map(|i| {
                phase += if i < step { from } else { to } / RATE;
                (TAU * phase).sin() as f32 * 0.5
            })
            .collect();
        let found = track(&config, &tone);
        let took = settled(&found, step, to, 5.0);
        assert!(
            took <= ms(20.0),
            "sine {from} to {to} Hz took {took} samples"
        );
        for (what, formants) in [("a", A), ("i", I), ("u", U)] {
            let input = voice(|i| if i < step { from } else { to }, formants, 24_000);
            let found = track(&config, &input);
            let rough = settled(&found, step, to, 20.0);
            assert!(
                rough <= ms(20.0),
                "{what} {from} to {to} Hz took {rough} samples"
            );
            let fine = settled(&found, step, to, 5.0);
            assert!(
                fine <= ms(30.0),
                "{what} {from} to {to} Hz took {fine} samples"
            );
        }
    }
}

// Plan: WP-8.32 (tracker)
#[test]
fn noise_and_a_fricative_are_unvoiced() {
    let config = Config::for_range(RATE, 50.0, 2000.0);
    for (what, input) in [
        ("white noise", noise(1, 48_000)),
        ("fricative", fricative(48_000)),
    ] {
        let found = track(&config, &input);
        let voiced = found
            .iter()
            .filter(|(_, p)| p.is_some_and(|p| p.voiced))
            .count();
        assert!(!found.is_empty());
        assert_eq!(voiced, 0, "{what} had {voiced} of {} voiced", found.len());
    }
}

// Plan: WP-8.32 (tracker)
#[test]
fn a_weak_fundamental_does_not_jump_an_octave() {
    for f0 in [100.0, 150.0, 220.0, 440.0] {
        let config = Config::for_range(RATE, 50.0, 2000.0);
        let found = track(&config, &weak_fundamental(f0, 24_000));
        let late: Vec<_> = found.iter().filter(|(i, _)| *i > 12_000).collect();
        for (_, p) in late {
            let p = p.expect("heard");
            assert!(
                cents(p.frequency, f0).abs() < 5.0,
                "{f0} Hz read {:.2} Hz",
                p.frequency
            );
        }
    }
}

// Plan: WP-8.32 (tracker). The trap: octave errors on breathy input.
#[test]
fn a_breathy_voice_keeps_its_octave() {
    let config = Config::for_range(RATE, 50.0, 2000.0);
    for f0 in [90.0, 140.0, 260.0, 520.0] {
        for (what, formants) in [("a", A), ("i", I), ("u", U)] {
            let hiss = noise(3, 24_000);
            let input: Vec<f32> = voice(|_| f0, formants, 24_000)
                .iter()
                .zip(&hiss)
                .map(|(v, n)| v + n * 0.15)
                .collect();
            let found = track(&config, &input);
            let late: Vec<_> = found.iter().filter(|(i, _)| *i > 12_000).collect();
            let right = late
                .iter()
                .filter(|(_, p)| p.is_some_and(|p| p.voiced && cents(p.frequency, f0).abs() < 25.0))
                .count();
            let octave = late
                .iter()
                .filter(|(_, p)| {
                    p.is_some_and(|p| p.voiced && cents(p.frequency, f0).abs() > 600.0)
                })
                .count();
            assert_eq!(octave, 0, "{what} {f0} Hz jumped an octave");
            assert!(
                right * 10 >= late.len() * 9,
                "{what} {f0} Hz: {right} of {} right",
                late.len()
            );
        }
    }
}

// Plan: WP-8.32 (tracker)
#[test]
fn a_pitch_that_begins_is_flagged_once() {
    let config = Config::for_range(RATE, 80.0, 2000.0);
    let mut input = vec![0.0; 12_000];
    input.extend(voice(|_| 180.0, A, 12_000));
    let found = track(&config, &input);
    let onsets: Vec<usize> = found
        .iter()
        .filter(|(_, p)| p.is_some_and(|p| p.onset))
        .map(|(i, _)| *i)
        .collect();
    assert_eq!(onsets.len(), 1, "{onsets:?}");
    assert!(
        onsets[0] > 12_000 && onsets[0] < 12_000 + 2_400,
        "{onsets:?}"
    );
}

// Spec: tuner R2. The detector's own cadence stays what the Tuner keeps: the
// window and interval of 2,048, and a lag range of 39 Hz to 2,150 Hz.
#[test]
fn a_measurement_is_ready_within_an_interval_of_starting() {
    let config = Config {
        rate: RATE,
        low: 39.0,
        high: 2150.0,
        window: 2048,
        interval: 2048,
        octave_guard: false,
    };
    let found = track(&config, &sine(110.0, 8192));
    // The first starts when the window fills, at 2,048 samples, and ends
    // within an interval of it.
    let first = found.first().map(|(i, _)| *i).expect("a measurement");
    assert!((2048..4096).contains(&first), "{first}");
    assert_eq!(found.len(), 3);
}

/// The output of a shifter fed `input` at the ratios, a sample at a time,
/// with its latency taken off.
fn shift(low: f64, input: &[f32], pitch: f32, formant: f32) -> Vec<f32> {
    let mut shifter = VoiceShifter::new(RATE, low, 2000.0);
    let latency = shifter.latency();
    let mut out: Vec<f32> = input
        .iter()
        .chain(std::iter::repeat_n(&0.0, latency))
        .map(|&x| shifter.process(x, pitch, formant))
        .collect();
    out.drain(..latency);
    out
}

/// The median pitch a tracker reads from the last half of `x`.
fn pitch_of(x: &[f32]) -> f64 {
    let config = Config::for_range(RATE, 40.0, 2000.0);
    let mut heard: Vec<f64> = track(&config, x)
        .iter()
        .filter(|(i, _)| *i > x.len() / 2)
        .filter_map(|(_, p)| p.filter(|p| p.voiced).map(|p| p.frequency))
        .collect();
    assert!(!heard.is_empty(), "no pitch heard");
    heard.sort_by(f64::total_cmp);
    heard[heard.len() / 2]
}

/// The levels, in decibels, of the harmonics of a voice of `f0` hertz in the
/// last 8,192 samples of `x`, from the first up to 4.5 kHz. That is all of its
/// spectral envelope a voice shows.
fn harmonic_levels(x: &[f32], f0: f64) -> Vec<f64> {
    use crate::stft::Complex32;
    use realfft::RealFftPlanner;
    const SIZE: usize = 8192;
    let mut frame: Vec<f32> = x[x.len() - SIZE..]
        .iter()
        .enumerate()
        .map(|(i, v)| v * (0.5 - 0.5 * (TAU * i as f64 / SIZE as f64).cos()) as f32)
        .collect();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(SIZE);
    let mut spectrum = vec![Complex32::default(); SIZE / 2 + 1];
    fft.process(&mut frame, &mut spectrum).unwrap();
    (1..=(4500.0 / f0) as usize)
        .map(|k| {
            let bin = (k as f64 * f0 * SIZE as f64 / RATE).round() as usize;
            let loudest = (bin - 2..=bin + 2)
                .map(|b| spectrum[b].norm())
                .fold(0.0, f32::max);
            20.0 * f64::from(loudest).max(1e-9).log10()
        })
        .collect()
}

/// How far `out`'s spectral envelope is stretched along the frequency axis
/// from `plain`'s, the same vowel sung at the same pitch, `f0` hertz: the
/// factor `s` that, by moving `plain`'s harmonic levels to `s` times their
/// frequencies, leaves them nearest `out`'s (in decibels, apart from a
/// constant). It is where all three formants have moved to, together: a
/// voice shows its envelope only at its harmonics, which at a high pitch are
/// too far apart to place one formant's peak within 5 % but not to place the
/// three.
fn envelope_stretch(out: &[f32], plain: &[f32], f0: f64) -> f64 {
    let (heard, wanted) = (harmonic_levels(out, f0), harmonic_levels(plain, f0));
    let first = (200.0 / f0).ceil() as usize;
    let last = (3500.0 / f0) as usize;
    // `wanted` at harmonic `h`, a fraction, between its neighbours.
    let at = |h: f64| {
        let h = h.clamp(1.0, wanted.len() as f64);
        let (lo, fraction) = (h.floor() as usize, h.fract());
        let hi = (lo + 1).min(wanted.len());
        wanted[lo - 1] * (1.0 - fraction) + wanted[hi - 1] * fraction
    };
    let cost = |s: f64| {
        let diffs: Vec<f64> = (first..=last)
            .map(|k| heard[k - 1] - at(k as f64 / s))
            .collect();
        let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
        diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / diffs.len() as f64
    };
    (0..=700)
        .map(|i| 0.7 + 0.001 * f64::from(i))
        .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))
        .unwrap()
}

/// Checks that `out`, a voice of `f0` hertz, has the formants of the vowel
/// `plain` moved by `stretch`, to within 5 %, and that the levels of its
/// harmonics then match the ones `reference`, the vowel so moved sung at
/// that pitch, has within 3 dB on average up to 3.5 kHz.
fn assert_formants(
    what: &str,
    out: &[f32],
    plain: &[f32],
    reference: &[f32],
    f0: f64,
    stretch: f64,
) {
    let found = envelope_stretch(out, plain, f0);
    assert!(
        (found / stretch - 1.0).abs() < 0.05,
        "{what}: the formants moved by {found:.3}, wanted {stretch:.3}"
    );
    let (heard, wanted) = (harmonic_levels(out, f0), harmonic_levels(reference, f0));
    let used = (3500.0 / f0) as usize;
    let error = heard[..used]
        .iter()
        .zip(&wanted)
        .map(|(a, b)| (a - b).abs())
        .sum::<f64>()
        / used as f64;
    assert!(error < 3.0, "{what}: the envelope is {error:.1} dB off");
}

// Plan: WP-8.32 (shifter). What the shifter makes has the formants of the
// same vowel sung at the new pitch.
#[test]
fn the_shifter_moves_a_vowel_from_minus_12_to_plus_12_semitones_keeping_its_formants() {
    for f0 in [85.0, 140.0] {
        let input = voice(|_| f0, A, 60_000);
        for semitones in [-12.0, -7.0, -3.0, 0.0, 3.0, 7.0, 12.0] {
            let ratio = 2.0f64.powf(semitones / 12.0);
            let out = shift(40.0, &input, ratio as f32, 1.0);
            let heard = pitch_of(&out);
            assert!(
                cents(heard, f0 * ratio).abs() < 5.0,
                "{semitones:+} st: {heard:.2} Hz, wanted {:.2}",
                f0 * ratio
            );
            let sung = voice(|_| f0 * ratio, A, 60_000);
            assert_formants(
                &format!("{f0} Hz {semitones:+} st"),
                &out,
                &sung,
                &sung,
                f0 * ratio,
                1.0,
            );
        }
    }
}

// Plan: WP-8.32 (shifter)
#[test]
fn the_formant_ratio_moves_the_formants_and_leaves_the_pitch() {
    let f0 = 140.0;
    let input = voice(|_| f0, A, 60_000);
    for (pitch, formant) in [(1.0, 0.8), (1.0, 1.25), (1.4, 0.7), (0.7, 1.4)] {
        let out = shift(80.0, &input, pitch, formant);
        let f0_out = f0 * f64::from(pitch);
        let heard = pitch_of(&out);
        assert!(
            cents(heard, f0_out).abs() < 5.0,
            "{pitch} {formant}: {heard:.2} Hz, wanted {f0_out:.2}"
        );
        let scale = f64::from(formant);
        let plain = voice(|_| f0_out, A, 60_000);
        let moved = voice_scaled(|_| f0_out, A, scale, 60_000);
        assert_formants(
            &format!("{pitch} {formant}"),
            &out,
            &plain,
            &moved,
            f0_out,
            scale,
        );
    }
}

// Plan: WP-8.32 (shifter)
#[test]
fn every_vowel_is_moved_within_5_cents() {
    let f0 = 120.0;
    for (what, formants) in [("i", I), ("u", U)] {
        let input = voice(|_| f0, formants, 48_000);
        for ratio in [0.5, 0.7071, 1.3348, 2.0] {
            let out = shift(40.0, &input, ratio as f32, 1.0);
            let heard = pitch_of(&out);
            assert!(
                cents(heard, f0 * ratio).abs() < 5.0,
                "{what} x{ratio}: {heard:.2} Hz, wanted {:.2}",
                f0 * ratio
            );
        }
    }
}

// Plan: WP-8.32 (shifter). At no shift the voice comes out as it went in, a
// latency later.
#[test]
fn unshifted_the_voice_is_the_input_delayed_by_the_latency() {
    let input = voice(|_| 150.0, A, 48_000);
    // `shift` has the latency off already: out[i] answers input[i].
    let out = shift(80.0, &input, 1.0, 1.0);
    let (mut dot, mut a, mut b) = (0.0f64, 0.0f64, 0.0f64);
    for i in 30_000..40_000 {
        let (x, y) = (f64::from(input[i]), f64::from(out[i]));
        dot += x * y;
        a += x * x;
        b += y * y;
    }
    let similarity = dot / (a * b).sqrt();
    assert!(
        similarity > 0.98,
        "the output matches the input by {similarity:.3}"
    );
    assert!(
        (0.9..1.1).contains(&(b / a).sqrt()),
        "level {:.3}",
        (b / a).sqrt()
    );
}

// Plan: WP-8.32 (shifter). The trap: unvoiced frames pass through without
// pitch marks, as the input, whatever the ratios.
#[test]
fn unvoiced_sound_passes_through_unchanged() {
    for input in [noise(5, 30_000), fricative(30_000), vec![0.0; 30_000]] {
        let mut shifter = VoiceShifter::new(RATE, 80.0, 2000.0);
        let latency = shifter.latency();
        for (i, &x) in input.iter().enumerate() {
            let y = shifter.process(x, 1.5, 0.8);
            let want = if i >= latency {
                input[i - latency]
            } else {
                0.0
            };
            assert_eq!(y, want, "sample {i}");
        }
    }
}

// Plan: WP-8.32 (shifter). A vowel, an "s" and a vowel again, shifted: the
// output is the vowels and the "s", with no break or burst between them.
#[test]
fn a_voice_with_an_s_in_it_comes_out_whole() {
    let mut input = voice(|_| 160.0, A, 20_000);
    input.extend(fricative(10_000).iter().map(|x| x * 0.3));
    input.extend(voice(|_| 160.0, I, 20_000));
    let out = shift(80.0, &input, 1.5, 1.0);
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    for part in input.chunks(2_000).zip(out.chunks(2_000)).skip(1) {
        let (heard, wanted) = (rms(part.1), rms(part.0));
        assert!(
            heard > 0.3 * wanted && heard < 3.0 * wanted,
            "a stretch of {wanted:.3} came out at {heard:.3}"
        );
    }
    assert!(out.iter().all(|x| x.abs() < 1.5));
}

// Plan: WP-8.32 (shifter). Block sizes only cut the same samples.
#[test]
fn renders_are_identical_at_block_sizes_1_64_512_and_4096() {
    let mut input = voice(|i| 140.0 + i as f64 * 0.002, A, 24_000);
    input.extend(fricative(4_000));
    let ratio = |i: usize| 1.0 + 0.4 * (i as f32 * 0.0003).sin();
    let render = |block: usize| {
        let mut shifter = VoiceShifter::new(RATE, 80.0, 2000.0);
        let mut out = Vec::new();
        for (n, chunk) in input.chunks(block).enumerate() {
            for (k, &x) in chunk.iter().enumerate() {
                let i = n * block + k;
                out.push(shifter.process(x, ratio(i), 1.0 / ratio(i)));
            }
        }
        out
    };
    let first = render(1);
    for block in [64, 512, 4096] {
        assert_eq!(render(block), first, "block of {block}");
    }
}

// Plan: WP-8.32 (real-time). Nothing allocates, locks or blocks.
#[test]
fn the_tracker_and_the_shifter_run_under_the_real_time_trap() {
    let input = voice(|i| 100.0 + i as f64 * 0.01, A, 40_000);
    let mut tracker = Tracker::new(&Config::for_range(RATE, 60.0, 2000.0));
    let mut shifter = VoiceShifter::new(RATE, 60.0, 2000.0);
    let mut sum = 0.0f32;
    for block in input.chunks(256) {
        lspk_rt::rt_test!({
            for (i, &x) in block.iter().enumerate() {
                tracker.feed(x);
                while tracker.pop().is_some() {}
                sum += shifter.process(x, 0.5 + i as f32 / 128.0, 2.0 - i as f32 / 256.0);
            }
        });
    }
    assert!(sum.is_finite());
}

// Plan: WP-8.32 (shifter)
#[test]
fn a_ratio_that_is_not_a_number_is_no_change() {
    let input = voice(|_| 150.0, A, 30_000);
    assert_eq!(
        shift(80.0, &input, f32::NAN, f32::NAN),
        shift(80.0, &input, 1.0, 1.0)
    );
}
