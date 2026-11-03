//! Vocoder (WP-8.45; the vocoder spec): the banks' bands at every count and
//! range, the envelopes, the carriers, the unvoiced detector, the channel
//! modes, the display's data, the latency, and renders that repeat at every
//! block size. Every block runs as a real-time section under the allocation
//! trap.

use std::f64::consts::TAU;

use crate::common;
use bl_devices::vocoder::bank::{BAND_COUNTS, Bank, Coeffs, MAX_BANDS, bandwidth, centre, step};
use bl_devices::vocoder::carrier::{Noise, Oscillator, Wave};
use bl_devices::vocoder::*;
use bl_dsp::pitch::{Config, Tracker};
use bl_engine::{Block, DeviceType, Node, NodeEvent, Registry, TransportInfo};
use bl_model::ParamId;
use common::response::{Kind, rbj};
use common::{RATE, device, node};

const RATE_HZ: f64 = 48_000.0;

/// A registry with the Vocoder alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::vocoder::register(&mut registry);
    registry
}

/// A Vocoder of `params`, taking blocks of up to `max_frames`.
fn vocoder(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = vocoder_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// Streams per-channel `input`, and the sidechain `key` if there is one,
/// through `node` in blocks of `block` frames, under the allocation trap,
/// with `events` (offsets from the start of the run) landing in their
/// blocks. Returns each channel's output.
fn render(
    node: &mut dyn Node,
    input: &[Vec<f32>],
    key: Option<&[Vec<f32>]>,
    block: usize,
    events: &[NodeEvent],
) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut out = vec![Vec::with_capacity(frames); input.len()];
    for start in (0..frames).step_by(block) {
        let end = (start + block).min(frames);
        let n = end - start;
        let mut data: Vec<f32> = input.iter().flat_map(|c| c[start..end].to_vec()).collect();
        let keyed: Option<Vec<f32>> =
            key.map(|key| key.iter().flat_map(|c| c[start..end].to_vec()).collect());
        let here: Vec<NodeEvent> = events
            .iter()
            .filter(|e| (e.offset as usize) >= start && (e.offset as usize) < end)
            .map(|e| NodeEvent {
                offset: e.offset - start as u32,
                ..*e
            })
            .collect();
        {
            let mut block = Block::new(
                &mut data,
                input.len(),
                n,
                n,
                &here,
                TransportInfo::default(),
                RATE,
            );
            if let Some(keyed) = &keyed {
                block = block.with_sidechain(keyed);
            }
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (channel, values) in out.iter_mut().zip(data.chunks(n)) {
            channel.extend_from_slice(values);
        }
    }
    out
}

/// `signal` on both channels.
fn both(signal: &[f32]) -> Vec<Vec<f32>> {
    vec![signal.to_vec(), signal.to_vec()]
}

/// Silence, `frames` long.
fn silence(frames: usize) -> Vec<f32> {
    vec![0.0; frames]
}

/// A sine of `hz` and `amplitude`.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (amplitude * (TAU * hz * n as f64 / RATE_HZ).sin()) as f32)
        .collect()
}

/// The sum of two equally long signals.
fn plus(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(a, b)| a + b).collect()
}

/// `signal`'s root mean square over `from..to`.
fn rms(signal: &[f32], from: usize, to: usize) -> f64 {
    let part = &signal[from..to];
    (part.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / part.len() as f64).sqrt()
}

/// A short xorshift noise source, so a render is the same every run.
fn white(frames: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0) as f32
        })
        .collect()
}

/// A signal through a biquad of the cookbook's `b` and `a`.
fn filtered(signal: &[f32], (b, a): common::response::Biquad) -> Vec<f32> {
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    signal
        .iter()
        .map(|&x| {
            let x = f64::from(x);
            let y = (b[0] * x + b[1] * x1 + b[2] * x2 - a[1] * y1 - a[2] * y2) / a[0];
            (x2, x1, y2, y1) = (x1, x, y1, y);
            y as f32
        })
        .collect()
}

/// A generated vowel: a glottal pulse train at `f0` through resonators at
/// `formants`, each 8 % wide, at a peak of about 0.5.
fn vowel(f0: f64, formants: &[f64], frames: usize) -> Vec<f32> {
    let period = RATE_HZ / f0;
    let mut signal: Vec<f32> = (0..frames)
        .map(|n| {
            // One Rosenberg-style pulse per period: a smooth rise and a
            // quick fall.
            let phase = (n as f64 % period) / period;
            if phase < 0.4 {
                (0.5 * (1.0 - (std::f64::consts::PI * phase / 0.4).cos())) as f32
            } else if phase < 0.5 {
                (std::f64::consts::PI * (phase - 0.4) / 0.2).cos() as f32
            } else {
                0.0
            }
        })
        .collect();
    // The source's slope is its derivative: a pulse of sharp edges.
    let mut previous = 0.0;
    for s in &mut signal {
        let now = *s;
        *s = now - previous;
        previous = now;
    }
    for &formant in formants {
        let r = (-std::f64::consts::PI * 0.08 * formant / RATE_HZ).exp();
        let a1 = -2.0 * r * (TAU * formant / RATE_HZ).cos();
        let a2 = r * r;
        let (mut y1, mut y2) = (0.0, 0.0);
        for s in &mut signal {
            let y = f64::from(*s) * (1.0 - r) - a1 * y1 - a2 * y2;
            (y2, y1) = (y1, y);
            *s = y as f32;
        }
    }
    let peak = signal.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    signal.iter().map(|s| s * 0.5 / peak).collect()
}

/// Noise above 3 kHz: a fricative.
fn fricative(frames: usize) -> Vec<f32> {
    let high = rbj(Kind::HighPass, 3_000.0, 0.0, 0.707);
    let noise = filtered(&white(frames, 0x2545_F491), high);
    filtered(&noise, high).iter().map(|s| s * 2.0).collect()
}

/// The magnitude of `signal`'s response at `hz`, from its impulse response.
fn level_at(signal: &[f32], hz: f64) -> f64 {
    let (re, im) = common::dft_at(signal, hz);
    re.hypot(im)
}

// ---------------------------------------------------------------------------
// R2 to R5: the banks.

/// A band's response in dB at `hz`.
fn band_db(coeffs: &Coeffs, k: usize, hz: f64) -> f64 {
    let (re, im) = coeffs.response(k, hz, RATE_HZ);
    20.0 * re.hypot(im).max(1e-12).log10()
}

/// Where band `k` peaks and how wide it is at −3 dB below its peak, by
/// golden-section search for the peak and bisection for the edges.
fn measured(coeffs: &Coeffs, k: usize) -> (f64, f64) {
    let design = coeffs.centre(k);
    let (mut low, mut high) = (design / 1.6, design * 1.6);
    let golden = 0.618_033_988_75;
    for _ in 0..80 {
        let a = high / (high / low).powf(golden);
        let b = low * (high / low).powf(golden);
        if band_db(coeffs, k, a) > band_db(coeffs, k, b) {
            high = b;
        } else {
            low = a;
        }
    }
    let peak_hz = (low * high).sqrt();
    let target = band_db(coeffs, k, peak_hz) - 3.0103;
    let edge = |mut inside: f64, mut outside: f64| {
        for _ in 0..60 {
            let mid = (inside * outside).sqrt();
            if band_db(coeffs, k, mid) > target {
                inside = mid;
            } else {
                outside = mid;
            }
        }
        (inside * outside).sqrt()
    };
    let below = edge(peak_hz, 1.0);
    let above = edge(peak_hz, RATE_HZ / 2.0 - 1.0);
    (peak_hz, above - below)
}

const RANGES: [(f64, f64); 3] = [(80.0, 12_000.0), (20.0, 20_000.0), (200.0, 5_000.0)];

// Spec: vocoder R2
#[test]
fn each_bands_centre_and_bandwidth_keep_their_design_at_every_count_and_range() {
    for retro in [false, true] {
        for count in BAND_COUNTS {
            for (low, high) in RANGES {
                let mut coeffs = Coeffs::new();
                coeffs.design((count, low, high), 100.0, 1.0, retro, RATE_HZ);
                assert_eq!(coeffs.count(), count);
                for k in 0..count {
                    let (peak, width) = measured(&coeffs, k);
                    let (c, w) = (coeffs.centre(k), coeffs.bandwidth(k));
                    assert!(
                        (c - centre(count, low, high, k)).abs() < 1e-9,
                        "centres are evenly spaced in log frequency"
                    );
                    assert!(
                        (peak / c - 1.0).abs() < 0.01,
                        "{count} bands {low}-{high} retro {retro} band {k}: centre {peak} against {c}"
                    );
                    assert!(
                        (width / w - 1.0).abs() < 0.05,
                        "{count} bands {low}-{high} retro {retro} band {k}: width {width} against {w}"
                    );
                }
            }
        }
    }
}

// Spec: vocoder R2
#[test]
fn the_banks_run_the_bands_the_response_describes() {
    let mut coeffs = Coeffs::new();
    coeffs.design((20, 80.0, 12_000.0), 100.0, 1.0, false, RATE_HZ);
    let mut bank = Bank::new();
    let frames = 1 << 15;
    let mut impulse = vec![vec![0.0f32; frames]; MAX_BANDS];
    let mut out = [0.0; MAX_BANDS];
    for n in 0..frames {
        let x = if n == 0 { 1.0 } else { 0.0 };
        bank.tick(&coeffs, (x, x), 20, &mut out);
        for (band, value) in impulse.iter_mut().zip(out).take(20) {
            band[n] = value as f32;
        }
    }
    for k in [0, 7, 13, 19] {
        for factor in [0.8, 1.0, 1.25] {
            let hz = coeffs.centre(k) * factor;
            let (re, im) = common::response::response_at(&impulse[k], hz);
            let ran = re.hypot(im);
            let (r, i) = coeffs.response(k, hz, RATE_HZ);
            let told = r.hypot(i);
            assert!(
                (ran / told - 1.0).abs() < 0.01,
                "band {k} at {hz}: ran {ran}, response {told}"
            );
        }
    }
}

/// The bank's summed power response at `hz`.
fn summed_db(coeffs: &Coeffs, hz: f64) -> f64 {
    let power: f64 = (0..coeffs.count())
        .map(|k| {
            let (re, im) = coeffs.response(k, hz, RATE_HZ);
            re * re + im * im
        })
        .sum();
    10.0 * power.log10()
}

// Spec: vocoder R3
#[test]
fn at_bw_100_adjacent_bands_meet_at_minus_3_db_and_the_sum_is_flat_within_1_db() {
    for count in [8, 16, 20, 28, 40] {
        let (low, high) = (100.0, 5_000.0);
        let mut coeffs = Coeffs::new();
        coeffs.design((count, low, high), 100.0, 1.0, false, RATE_HZ);
        // Adjacent bands cross at −3 dB, halfway between their centres in
        // log frequency.
        for k in 0..count - 1 {
            let cross = (centre(count, low, high, k) * centre(count, low, high, k + 1)).sqrt();
            let (a, b) = (band_db(&coeffs, k, cross), band_db(&coeffs, k + 1, cross));
            assert!(
                (a + 3.0).abs() < 0.3 && (b + 3.0).abs() < 0.3,
                "{count}: {a} {b}"
            );
        }
        // Flat across Range.
        let from = centre(count, low, high, 0);
        let to = centre(count, low, high, count - 1);
        let levels: Vec<f64> = (0..400)
            .map(|i| summed_db(&coeffs, from * (to / from).powf(f64::from(i) / 399.0)))
            .collect();
        let most = levels.iter().copied().fold(f64::MIN, f64::max);
        let least = levels.iter().copied().fold(f64::MAX, f64::min);
        assert!(most - least < 1.0, "{count} bands: {least} to {most} dB");
    }
    // BW scales the bandwidth against the spacing: 50 % is half as wide in
    // octaves, 200 % twice.
    let spacing = step(20, 80.0, 12_000.0);
    let c = centre(20, 80.0, 12_000.0, 8);
    let w = |bw| bandwidth(c, spacing, bw, RATE_HZ);
    assert!((w(100.0) - 2.0 * c * (spacing / 2.0).sinh()).abs() < 1e-9);
    assert!(w(50.0) < w(100.0) && w(100.0) < w(200.0));
}

// Spec: vocoder R4
#[test]
fn retro_runs_second_order_bands_with_a_gentler_skirt_and_a_2_db_boost() {
    let (count, low, high) = (20, 100.0, 5_000.0);
    let (mut precise, mut retro) = (Coeffs::new(), Coeffs::new());
    precise.design((count, low, high), 100.0, 1.0, false, RATE_HZ);
    retro.design((count, low, high), 100.0, 1.0, true, RATE_HZ);
    let k = 10;
    let c = precise.centre(k);
    let boost = band_db(&retro, k, c) - band_db(&precise, k, c);
    assert!((boost - 2.0).abs() < 0.05, "boost {boost}");
    // An octave and a half away, the second-order skirt is higher.
    for factor in [0.35, 2.8] {
        let (p, r) = (
            band_db(&precise, k, c * factor),
            band_db(&retro, k, c * factor) - 2.0,
        );
        assert!(r > p + 6.0, "at {factor}: precise {p}, retro {r}");
    }
    // The asymptotic slope: second order falls 6 dB an octave a side, fourth
    // order 12.
    // (Read well below Nyquist, where the bilinear transform's warp is small.)
    let c = precise.centre(2);
    let slope = |coeffs: &Coeffs| band_db(coeffs, 2, c * 8.0) - band_db(coeffs, 2, c * 16.0);
    assert!((slope(&retro) - 6.0).abs() < 1.0, "{}", slope(&retro));
    assert!((slope(&precise) - 12.0).abs() < 1.5, "{}", slope(&precise));
}

// Spec: vocoder R5
#[test]
fn formant_moves_the_synthesis_bands_by_its_interval() {
    for semitones in [-12.0, -5.0, 7.0, 12.0] {
        let ratio = f64::powf(2.0, semitones / 12.0);
        let (mut plain, mut shifted) = (Coeffs::new(), Coeffs::new());
        plain.design((16, 100.0, 5_000.0), 100.0, 1.0, false, RATE_HZ);
        shifted.design((16, 100.0, 5_000.0), 100.0, ratio, false, RATE_HZ);
        for k in 0..16 {
            let (a, _) = measured(&plain, k);
            let (b, _) = measured(&shifted, k);
            assert!(
                (b / a / ratio - 1.0).abs() < 0.02,
                "band {k} at {semitones}: {a} to {b}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// R5 on a vowel: the formants through a sawtooth carrier.

/// A sawtooth of `hz` and `amplitude`.
fn saw(hz: f64, amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| amplitude * (2.0 * (hz * n as f64 / RATE_HZ).fract() as f32 - 1.0))
        .collect()
}

/// Where a spectral peak sits near `expected`: the power-weighted mean, in
/// log frequency, of the harmonics of `f0` within the ratio window `window`
/// of it. `signal` is a second long, so a harmonic is on a bin.
fn peak_near(signal: &[f32], f0: f64, expected: f64, window: (f64, f64)) -> f64 {
    let spectrum = common::spectrum(signal);
    let (mut weight, mut sum) = (0.0, 0.0);
    let mut h = 1;
    while (h as f64) * f0 < expected * window.1 {
        let hz = h as f64 * f0;
        if hz > expected * window.0 {
            let a = spectrum[(hz * signal.len() as f64 / RATE_HZ).round() as usize];
            weight += a * a;
            sum += a * a * hz.ln();
        }
        h += 1;
    }
    (sum / weight).exp()
}

// Spec: vocoder R5
#[test]
fn a_vowel_on_a_saw_carrier_keeps_its_formants_and_formant_moves_them_by_its_amount() {
    let frames = 96_000;
    let formants = [600.0, 2_400.0];
    let window = (0.8, 1.25);
    let modulator = vowel(120.0, &formants, frames);
    let carrier = saw(50.0, 0.5, frames);
    let peaks_at = |semitones: f64| -> Vec<f64> {
        let mut node = vocoder(&[(CARRIER, 1.0), (FORMANT, semitones), (BANDS, 9.0)], 512);
        let out = render(
            node.as_mut(),
            &both(&modulator),
            Some(&both(&carrier)),
            512,
            &[],
        );
        let tail = &out[0][frames - 48_000..];
        formants
            .iter()
            .map(|f| peak_near(tail, 50.0, f * 2f64.powf(semitones / 12.0), window))
            .collect()
    };
    let plain = peaks_at(0.0);
    for (n, f) in formants.iter().enumerate() {
        // The vowel's own peak, found the same way, within 5 %.
        let heard = peak_near(&modulator[frames - 48_000..], 120.0, *f, window);
        assert!(
            (plain[n] / heard - 1.0).abs() < 0.05,
            "formant {f}: {} against the vowel's {heard}",
            plain[n]
        );
    }
    for semitones in [-5.0, 7.0, 12.0] {
        let moved = peaks_at(semitones);
        for n in 0..2 {
            let ratio = 2f64.powf(semitones / 12.0);
            assert!(
                (moved[n] / plain[n] / ratio - 1.0).abs() < 0.05,
                "formant {n} at {semitones}: {} to {}",
                plain[n],
                moved[n]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// R6 to R9: the envelopes.

/// The device's published display values after `input` and `key` have run
/// through a node of `params`.
fn published(
    params: &[(ParamId, f64)],
    input: &[Vec<f32>],
    key: Option<&[Vec<f32>]>,
) -> (Vec<Vec<f32>>, Vec<f32>) {
    let mut node = vocoder(params, 512);
    let mut reader = node.display().expect("Vocoder has a display");
    let out = render(node.as_mut(), input, key, 512, &[]);
    let values = reader.read().values.clone();
    (out, values)
}

/// `signal`'s amplitude at `hz` in a window of `cycles` cycles centred on
/// each frame in `at`.
fn amplitude_at(signal: &[f32], hz: f64, cycles: f64, at: &[usize]) -> Vec<f64> {
    let width = (cycles * RATE_HZ / hz).round() as usize;
    at.iter()
        .map(|&n| {
            let from = n.saturating_sub(width / 2);
            let to = (from + width).min(signal.len());
            let (mut re, mut im) = (0.0, 0.0);
            for (i, s) in signal[from..to].iter().enumerate() {
                let angle = TAU * hz * (from + i) as f64 / RATE_HZ;
                re += f64::from(*s) * angle.cos();
                im += f64::from(*s) * angle.sin();
            }
            2.0 * re.hypot(im) / (to - from) as f64
        })
        .collect()
}

/// The time, in ms, at which `levels` (read at the frames `at`) have gone
/// `fraction` of the way from where they started to `end`.
fn crossing_ms(levels: &[f64], at: &[usize], fraction: f64, end: f64) -> f64 {
    let start = levels[0];
    let target = start + (end - start) * fraction;
    let i = levels
        .windows(2)
        .position(|w| (w[0] - target) * (w[1] - target) <= 0.0)
        .expect("the level crosses");
    let t = (target - levels[i]) / (levels[i + 1] - levels[i]);
    (at[i] as f64 + t * (at[i + 1] - at[i]) as f64) * 1_000.0 / RATE_HZ
}

// Spec: vocoder R6
#[test]
fn attack_and_release_are_the_time_to_cover_63_percent_within_10_percent() {
    // Four wide bands, so the filters are fast against the envelopes; a
    // tone at the centre of the third, once as the modulator and as the
    // carrier, so the output's amplitude is the band's gain.
    let hz = centre(4, 80.0, 12_000.0, 2);
    let (on, off, frames) = (4_800, 60_000, 120_000);
    let tone = sine(hz, 0.4, frames);
    let burst: Vec<f32> = tone
        .iter()
        .enumerate()
        .map(|(n, s)| if (on..off).contains(&n) { *s } else { 0.0 })
        .collect();
    for (attack, release) in [(30.0, 120.0), (10.0, 60.0), (80.0, 300.0)] {
        let mut node = vocoder(
            &[
                (CARRIER, 1.0),
                (BANDS, 0.0),
                (ATTACK, attack),
                (RELEASE, release),
            ],
            512,
        );
        let out = render(node.as_mut(), &both(&burst), Some(&both(&tone)), 512, &[]);
        // First order: from 63.2 % to 86.5 % of the way takes one time
        // constant, whenever the step began.
        let rise: Vec<usize> = (on..on + (attack * 48.0 * 6.0) as usize)
            .step_by(24)
            .collect();
        let levels = amplitude_at(&out[0], hz, 4.0, &rise);
        let end = levels[levels.len() - 1];
        let measured =
            crossing_ms(&levels, &rise, 0.8647, end) - crossing_ms(&levels, &rise, 0.6321, end);
        assert!(
            (measured / attack - 1.0).abs() < 0.1,
            "attack {attack}: {measured}"
        );
        let fall: Vec<usize> = (off..off + (release * 48.0 * 3.0) as usize)
            .step_by(24)
            .collect();
        let levels = amplitude_at(&out[0], hz, 4.0, &fall);
        let measured =
            crossing_ms(&levels, &fall, 0.8647, 0.0) - crossing_ms(&levels, &fall, 0.6321, 0.0);
        assert!(
            (measured / release - 1.0).abs() < 0.1,
            "release {release}: {measured}"
        );
    }
}

/// The published level, in dB, of the band nearest `hz` among 20 from 80 Hz
/// to 12 kHz.
fn level_of(values: &[f32], hz: f64) -> f64 {
    let k = (0..20)
        .min_by(|a, b| {
            let d = |k: &usize| (centre(20, 80.0, 12_000.0, *k).ln() - hz.ln()).abs();
            d(a).total_cmp(&d(b))
        })
        .unwrap();
    f64::from(values[k])
}

/// Two tones at the centres of bands 4 and 14 of 20, the second `drop` dB
/// below the first.
fn two_tones(drop: f64, frames: usize) -> (f64, f64, Vec<Vec<f32>>) {
    let (a, b) = (
        centre(20, 80.0, 12_000.0, 4),
        centre(20, 80.0, 12_000.0, 14),
    );
    let quiet = 0.4 * 10f64.powf(-drop / 20.0);
    (
        a,
        b,
        both(&plus(&sine(a, 0.4, frames), &sine(b, quiet, frames))),
    )
}

// Spec: vocoder R7
#[test]
fn depth_gives_the_mean_at_0_the_level_at_100_and_the_level_squared_at_200() {
    let (a, b, input) = two_tones(12.0, 48_000);
    let gap = |depth: f64| {
        let (_, values) = published(&[(DEPTH, depth)], &input, None);
        (level_of(&values, a) - level_of(&values, b), values)
    };
    // No imprint: every band plays at the bands' mean.
    let (none, values) = gap(0.0);
    assert!(none.abs() < 0.01, "{none}");
    assert!(values[..20].iter().all(|v| (v - values[0]).abs() < 0.01));
    // In proportion to its level: the 12 dB the tones are apart.
    let (classic, _) = gap(100.0);
    assert!((classic - 12.0).abs() < 1.0, "{classic}");
    // To its level squared: twice the gap, in dB.
    let (peaks, _) = gap(200.0);
    assert!((peaks - 2.0 * classic).abs() < 0.5, "{peaks}");
}

// Spec: vocoder R8
#[test]
fn gate_silences_bands_below_it() {
    let (a, b, input) = two_tones(12.0, 48_000);
    // The louder tone's band is about −8 dBFS, the quieter's about −20.
    let (_, open) = published(&[(GATE, -80.0)], &input, None);
    let (_, gated) = published(&[(GATE, -14.0)], &input, None);
    assert!(level_of(&open, b) > -40.0);
    assert_eq!(level_of(&gated, b), -80.0, "the quiet band is silenced");
    assert!((level_of(&gated, a) - level_of(&open, a)).abs() < 0.01);
    // Heard: the quiet tone as the carrier plays only with the gate open.
    let carrier = both(&sine(b, 0.4, 48_000));
    let loud = |gate| {
        let (out, _) = published(&[(CARRIER, 1.0), (GATE, gate)], &input, Some(&carrier));
        rms(&out[0], 36_000, 48_000)
    };
    // (What is left is the tone leaking through the open bands far away.)
    assert!(loud(-80.0) > 1e-3);
    assert!(loud(-14.0) < loud(-80.0) * 0.01);
}

// Spec: vocoder R8
#[test]
fn level_is_a_gain_that_reaches_minus_infinity() {
    let (_, _, input) = two_tones(12.0, 24_000);
    let carrier = both(&sine(500.0, 0.4, 24_000));
    let at = |level: f64| {
        let (out, _) = published(&[(CARRIER, 1.0), (LEVEL, level)], &input, Some(&carrier));
        rms(&out[0], 12_000, 24_000)
    };
    let base = at(1.0);
    assert!((at(2.0) / base - 2.0).abs() < 0.02);
    assert_eq!(at(0.0), 0.0);
}

// Spec: vocoder R9
#[test]
fn enhance_brings_quiet_bands_up_against_their_neighbours() {
    let (a, b, input) = two_tones(40.0, 48_000);
    let gap = |enhance: f64| {
        let (_, values) = published(&[(ENHANCE, enhance)], &input, None);
        level_of(&values, a) - level_of(&values, b)
    };
    let plain = gap(0.0);
    let enhanced = gap(1.0);
    assert!((plain - 40.0).abs() < 1.5, "{plain}");
    assert!(enhanced.abs() < 3.0, "{enhanced}");
}

// ---------------------------------------------------------------------------
// R10 to R12: the carriers.

// Spec: vocoder R10
#[test]
fn noise_is_white_with_its_x_lowering_the_hold_rate_and_its_y_the_density() {
    let take = |x: f64, y: f64| {
        let mut noise = Noise::new(7);
        (0..48_000)
            .map(|_| noise.next(x, y, RATE_HZ))
            .collect::<Vec<f64>>()
    };
    // X and Y at 0: a new value every sample, white and centred.
    let plain = take(0.0, 0.0);
    assert!(plain.iter().all(|v| v.abs() <= 0.5));
    assert!(plain.windows(2).all(|w| w[0] != w[1]));
    let mean = plain.iter().sum::<f64>() / plain.len() as f64;
    let lag1 = plain.windows(2).map(|w| w[0] * w[1]).sum::<f64>() / plain.len() as f64;
    let power = plain.iter().map(|v| v * v).sum::<f64>() / plain.len() as f64;
    assert!(mean.abs() < 0.01 && (lag1 / power).abs() < 0.02);
    // X lowers the hold's rate from the sample rate to 1 kHz.
    let changes = |v: &[f64]| v.windows(2).filter(|w| w[0] != w[1]).count() as f64;
    assert!((changes(&take(1.0, 0.0)) / 1_000.0 - 1.0).abs() < 0.05);
    let halfway = 48_000.0 * (1_000.0f64 / 48_000.0).sqrt();
    assert!((changes(&take(0.5, 0.0)) / halfway - 1.0).abs() < 0.05);
    // Y thins the impulses from every sample to one in 64.
    let share = |y: f64| take(0.0, y).iter().filter(|v| **v != 0.0).count() as f64 / 48_000.0;
    assert!((share(0.0) - 1.0).abs() < 1e-9);
    assert!((share(1.0) * 64.0 - 1.0).abs() < 0.15, "{}", share(1.0));
    assert!(share(0.5) > share(1.0) && share(0.5) < share(0.0));
    // The seed gives the stream.
    assert_eq!(take(0.3, 0.3), take(0.3, 0.3));
    let mut other = Noise::new(8);
    assert_ne!(other.next(0.0, 0.0, RATE_HZ), take(0.0, 0.0)[0]);
}

// Spec: vocoder R10
#[test]
fn external_with_no_source_plays_silence_and_with_one_plays_it_through_the_bands() {
    let frames = 24_000;
    let modulator = both(&vowel(120.0, &[600.0, 2_400.0], frames));
    let mut node = vocoder(&[(CARRIER, 1.0)], 512);
    let silent = render(node.as_mut(), &modulator, None, 512, &[]);
    assert!(
        silent.iter().flatten().all(|s| *s == 0.0),
        "no source, no sound"
    );
    let carrier = both(&saw(110.0, 0.5, frames));
    let mut node = vocoder(&[(CARRIER, 1.0)], 512);
    let heard = render(node.as_mut(), &modulator, Some(&carrier), 512, &[]);
    assert!(rms(&heard[0], 12_000, frames) > 1e-3);
}

// Spec: vocoder R10
#[test]
fn the_modulator_carrier_plays_the_input_through_the_bands() {
    let frames = 48_000;
    let f0 = 120.0;
    let modulator = both(&vowel(f0, &[600.0, 2_400.0], frames));
    let mut node = vocoder(&[(CARRIER, 2.0)], 512);
    let out = render(node.as_mut(), &modulator, None, 512, &[]);
    assert!(rms(&out[0], 24_000, frames) > 1e-4);
    // It is the input shaped by its own levels: still a line spectrum at
    // the voice's harmonics.
    let tail = &out[0][frames - 24_000..];
    let spectrum = common::spectrum(tail);
    let bin = |hz: f64| (hz * tail.len() as f64 / RATE_HZ).round() as usize;
    let on: f64 = (1..=20).map(|h| spectrum[bin(h as f64 * f0)].powi(2)).sum();
    let between: f64 = (1..=20)
        .map(|h| spectrum[bin((h as f64 + 0.5) * f0)].powi(2))
        .sum();
    assert!(on > between * 100.0, "{on} against {between}");
}

/// The oscillator's spectrum at the harmonics of `f0` after a second of
/// `wave`, and the share of its power that is not on one.
fn harmonics(wave: Wave, f0: f64) -> (Vec<f64>, f64) {
    let mut oscillator = Oscillator::new(f0);
    let signal: Vec<f32> = (0..96_000)
        .map(|_| oscillator.next(wave, f0, RATE_HZ) as f32)
        .collect();
    let spectrum = common::spectrum(&signal[48_000..]);
    let bin = |hz: f64| hz.round() as usize;
    let top = (RATE_HZ / 2.0 / f0) as usize;
    let on: Vec<f64> = (1..=top).map(|h| spectrum[bin(h as f64 * f0)]).collect();
    let total: f64 = spectrum.iter().map(|a| a * a).sum();
    // (The Hann window spreads a line over three bins.)
    let line: f64 = (1..=top)
        .flat_map(|h| (-1..=1).map(move |d| (h as f64 * f0).round() as i64 + d))
        .filter_map(|b| spectrum.get(b as usize))
        .map(|a| a * a)
        .sum();
    (on, (total - line) / total)
}

// Spec: vocoder R11
#[test]
fn the_oscillators_waveforms_are_a_saw_and_pulses_of_50_25_and_12_5_percent_without_aliasing() {
    let (saw, _) = harmonics(Wave::Saw, 500.0);
    for h in 1..=8 {
        // 2 / (π h) of a level of 0.5.
        let expected = 0.5 * 2.0 / (std::f64::consts::PI * h as f64);
        assert!((saw[h - 1] / expected - 1.0).abs() < 0.05, "saw {h}");
    }
    // A pulse's harmonics follow 4 A sin(π h duty) / (π h), for its level A
    // of 0.5, and are missing where that is 0.
    for (n, duty) in [(1, 0.5), (2, 0.25), (3, 0.125)] {
        let (pulse, _) = harmonics(Wave::of(n), 500.0);
        for h in 1..=12usize {
            let pi_h = std::f64::consts::PI * h as f64;
            let expected = 2.0 * (pi_h * duty).sin().abs() / pi_h;
            assert!(
                (pulse[h - 1] - expected).abs() < 0.02 * 0.64,
                "pulse {n} harmonic {h}: {} against {expected}",
                pulse[h - 1]
            );
        }
    }
    assert_eq!(Wave::of(0), Wave::Saw);
    // Band-limited: at High's top, 2 kHz, a naive saw would fold a twentieth of
    // its power back; almost none of this one's is off its harmonics.
    for wave in [Wave::Saw, Wave::of(1), Wave::of(2), Wave::of(3)] {
        for f0 in [500.0, 1_000.0, 2_000.0] {
            let (_, aliased) = harmonics(wave, f0);
            assert!(aliased < 1e-4, "{wave:?} at {f0}: {aliased}");
        }
    }
}

/// The pitch of `signal` near `expected` hertz, from its autocorrelation
/// with a parabola through the peak.
fn pitch_of(signal: &[f32], expected: f64) -> f64 {
    let lag = |tau: usize| -> f64 {
        signal[tau..]
            .iter()
            .zip(signal)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum()
    };
    let centre = (RATE_HZ / expected).round() as usize;
    let (from, to) = (centre * 9 / 10, centre * 11 / 10 + 1);
    let best = (from..to)
        .max_by(|a, b| lag(*a).total_cmp(&lag(*b)))
        .unwrap();
    let (a, b, c) = (lag(best - 1), lag(best), lag(best + 1));
    let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
    RATE_HZ / (best as f64 + shift)
}

/// The error of `heard` against `wanted`, in cents.
fn cents(heard: f64, wanted: f64) -> f64 {
    1_200.0 * (heard / wanted).log2()
}

/// A voice at `f0`: a vowel with three formants.
fn voice(f0: f64, frames: usize) -> Vec<f32> {
    vowel(f0, &[500.0, 1_500.0, 2_500.0], frames)
}

// Spec: vocoder R11
#[test]
fn pitch_tracking_follows_a_synthetic_voice_within_5_cents() {
    let frames = 96_000;
    for f0 in [80.0, 150.0, 300.0, 600.0, 1_000.0] {
        // Depth 0 % leaves the bands' mean as every gain, so what comes
        // out is the oscillator through a flat bank.
        let mut node = vocoder(&[(CARRIER, 3.0), (DEPTH, 0.0)], 512);
        let out = render(node.as_mut(), &both(&voice(f0, frames)), None, 512, &[]);
        let heard = pitch_of(&out[0][frames - 24_000..], f0);
        assert!(cents(heard, f0).abs() < 5.0, "{f0}: heard {heard}");
    }
}

// Spec: vocoder R11
#[test]
fn the_oscillator_is_limited_by_high_and_low_moved_by_pitch_and_holds_through_unvoiced_sound() {
    let frames = 96_000;
    let play = |params: &[(ParamId, f64)], input: &[f32]| {
        let mut all = vec![(CARRIER, 3.0), (DEPTH, 0.0)];
        all.extend_from_slice(params);
        let mut node = vocoder(&all, 512);
        render(node.as_mut(), &both(input), None, 512, &[])
    };
    let tail = |out: &[Vec<f32>]| out[0][frames - 24_000..].to_vec();
    let high = play(&[(PITCH_HIGH, 200.0)], &voice(300.0, frames));
    assert!(cents(pitch_of(&tail(&high), 200.0), 200.0).abs() < 5.0);
    let low = play(&[(PITCH_LOW, 400.0)], &voice(300.0, frames));
    assert!(cents(pitch_of(&tail(&low), 400.0), 400.0).abs() < 5.0);
    let up = play(&[(PITCH, 12.0)], &voice(150.0, frames));
    assert!(cents(pitch_of(&tail(&up), 300.0), 300.0).abs() < 5.0);
    let down = play(&[(PITCH, -7.0)], &voice(300.0, frames));
    let wanted = 300.0 * 2f64.powf(-7.0 / 12.0);
    assert!(cents(pitch_of(&tail(&down), wanted), wanted).abs() < 5.0);
    // Every waveform keeps the pitch.
    for waveform in [0.0, 1.0, 2.0, 3.0] {
        let out = play(&[(WAVEFORM, waveform)], &voice(220.0, frames));
        assert!(cents(pitch_of(&tail(&out), 220.0), 220.0).abs() < 5.0);
    }
    // While the tracker calls a frame unvoiced, the oscillator keeps its
    // last pitch: a voice, then a hiss.
    let mut input = voice(200.0, frames / 2);
    input.extend(fricative(frames / 2));
    let out = play(&[], &input);
    let held = &out[0][frames - 24_000..];
    assert!(rms(held, 0, held.len()) > 1e-4, "it plays on");
    assert!(cents(pitch_of(held, 200.0), 200.0).abs() < 5.0);
}

// Spec: vocoder R12
#[test]
fn unvoiced_adds_noise_to_the_upper_bands_on_a_hiss_and_none_on_a_vowel() {
    let frames = 48_000;
    let hiss = both(&fricative(frames));
    let ah = both(&voice(120.0, frames));
    // External with no source: the carrier is silent, so what comes out is
    // the added noise alone.
    let play = |input: &[Vec<f32>], params: &[(ParamId, f64)]| {
        let mut all = vec![(CARRIER, 1.0)];
        all.extend_from_slice(params);
        let mut node = vocoder(&all, 512);
        render(node.as_mut(), input, None, 512, &[])
    };
    let off = play(&hiss, &[(UNVOICED, 0.0)]);
    assert!(off[0].iter().all(|s| *s == 0.0));
    let on = play(&hiss, &[(UNVOICED, 100.0)]);
    let heard = rms(&on[0], 24_000, frames);
    assert!(heard > 1e-3, "{heard}");
    // The noise is in the upper half of the bands: above the centre of the
    // eleventh of twenty, 1.1 kHz.
    let tail = &on[0][frames - 24_000..];
    let spectrum = common::spectrum(tail);
    let power = |from: f64, to: f64| -> f64 {
        let bin = |hz: f64| (hz * tail.len() as f64 / RATE_HZ) as usize;
        spectrum[bin(from)..bin(to)].iter().map(|a| a * a).sum()
    };
    assert!(power(1_500.0, 20_000.0) > power(20.0, 700.0) * 100.0);
    // A vowel adds none, at any Sens. a vowel's zero crossings stay under.
    let vowel_on = play(&ah, &[(UNVOICED, 100.0), (SENS, 40.0)]);
    assert!(rms(&vowel_on[0], 12_000, frames) < heard * 0.01);
    // Sens. at 0 % never calls a frame unvoiced; the Slow frames agree.
    let never = play(&hiss, &[(UNVOICED, 100.0), (SENS, 0.0)]);
    assert!(never[0].iter().all(|s| *s == 0.0));
    let slow = play(&hiss, &[(UNVOICED, 100.0), (FAST_SLOW, 1.0)]);
    assert!(rms(&slow[0], 24_000, frames) > 1e-3);
    // Under Gate, a quiet hiss is not heard either.
    let quiet: Vec<Vec<f32>> = hiss
        .iter()
        .map(|c| c.iter().map(|s| s * 0.001).collect())
        .collect();
    let gated = play(&quiet, &[(UNVOICED, 100.0), (GATE, -40.0)]);
    assert!(gated[0].iter().all(|s| *s == 0.0));
}

// ---------------------------------------------------------------------------
// R14: the channel modes.

// Spec: vocoder R14
#[test]
fn each_channel_mode_runs_its_own_banks() {
    let frames = 48_000;
    // A carrier with a tone on each side, a modulator broad enough to open
    // the bands of both.
    let key = vec![sine(1_000.0, 0.5, frames), sine(3_000.0, 0.5, frames)];
    let noise = white(frames, 99);
    let at = |out: &[f32], hz| level_at(&out[frames - 24_000..], hz);
    let play = |mode: f64, modulator: &[Vec<f32>]| {
        let mut node = vocoder(&[(CARRIER, 1.0), (CHANNEL_MODE, mode)], 512);
        render(node.as_mut(), modulator, Some(&key), 512, &[])
    };
    let both_sides = both(&noise);
    let left_only = vec![noise.clone(), silence(frames)];

    // Mono: the carrier's sum through one bank, the same on both channels.
    let mono = play(0.0, &both_sides);
    assert_eq!(mono[0], mono[1]);
    assert!(at(&mono[0], 1_000.0) > 1e-3 && at(&mono[0], 3_000.0) > 1e-3);

    // Stereo: one analysis of the input's sum drives a bank for each of the
    // carrier's channels, so each side plays its own tone, even from a
    // modulator on the left alone.
    for modulator in [&both_sides, &left_only] {
        let stereo = play(1.0, modulator);
        assert!(at(&stereo[0], 1_000.0) > 1e-3);
        assert!(at(&stereo[0], 3_000.0) < at(&stereo[0], 1_000.0) * 0.01);
        assert!(at(&stereo[1], 3_000.0) > 1e-3);
        assert!(at(&stereo[1], 1_000.0) < at(&stereo[1], 3_000.0) * 0.01);
    }

    // L/R: a bank pair for each channel, so a silent right modulator
    // silences the right side whatever its carrier.
    let lr = play(2.0, &left_only);
    assert!(at(&lr[0], 1_000.0) > 1e-3);
    assert!(lr[1].iter().all(|s| *s == 0.0));
    let both_lr = play(2.0, &both_sides);
    assert!(at(&both_lr[0], 1_000.0) > 1e-3 && at(&both_lr[1], 3_000.0) > 1e-3);
}

// Spec: vocoder R14
#[test]
fn dry_wet_balances_the_input_against_the_result() {
    let frames = 24_000;
    let input = both(&white(frames, 5));
    let key = both(&sine(1_000.0, 0.5, frames));
    let run = |wet: f64| {
        let mut node = vocoder(&[(CARRIER, 1.0), (DRY_WET, wet)], 512);
        render(node.as_mut(), &input, Some(&key), 512, &[]).remove(0)
    };
    let (dry, wet, half) = (run(0.0), run(100.0), run(50.0));
    let latency = reported_latency();
    for n in latency..frames {
        assert_eq!(dry[n], input[0][n - latency]);
        let mixed = 0.5 * wet[n] + 0.5 * dry[n];
        assert!((half[n] - mixed).abs() < 1e-5);
    }
}

// ---------------------------------------------------------------------------
// R15 to R17: latency, the real-time rules, repeatable renders.

/// What the tracker's latency is for the range the Vocoder builds it for.
fn reported_latency() -> usize {
    Tracker::new(&Config::for_range(RATE_HZ, 40.0, 2_000.0)).latency()
}

// Spec: vocoder R15
#[test]
fn the_latency_is_the_trackers_in_every_carrier() {
    for carrier in [0.0, 1.0, 2.0, 3.0] {
        let node = vocoder(&[(CARRIER, carrier)], 512);
        assert_eq!(
            node.latency() as usize,
            reported_latency(),
            "carrier {carrier}"
        );
    }
    assert!(reported_latency() > 0);
}

/// The renders the blocks-and-runs tests compare, one per carrier, with
/// parameter changes along the way.
fn renders(block: usize, frames: usize) -> Vec<Vec<Vec<f32>>> {
    let modulator = both(&plus(&voice(140.0, frames), &fricative(frames)));
    let key = both(&saw(110.0, 0.5, frames));
    let events = [
        common::set(5_000, FORMANT, 5.0),
        common::set(7_300, DEPTH, 150.0),
        common::set(9_100, LEVEL, 0.5),
    ];
    (0..4)
        .map(|carrier| {
            let mut node = vocoder(
                &[
                    (CARRIER, f64::from(carrier)),
                    (UNVOICED, 50.0),
                    (NOISE_X, 30.0),
                    (NOISE_Y, 30.0),
                    (CHANNEL_MODE, 2.0),
                ],
                block,
            );
            render(node.as_mut(), &modulator, Some(&key), block, &events)
        })
        .collect()
}

// Spec: vocoder R16, R17
#[test]
fn a_render_is_bit_identical_at_every_block_size_and_run_after_run() {
    let frames = 14_000;
    let reference = renders(512, frames);
    assert!(reference.iter().all(|r| r[0].iter().any(|s| *s != 0.0)));
    assert_eq!(renders(512, frames), reference, "run after run");
    for block in [1, 64, 4_096] {
        assert_eq!(renders(block, frames), reference, "blocks of {block}");
    }
}

// Spec: vocoder R16
#[test]
fn a_bank_of_40_bands_in_l_r_with_every_carrier_allocates_nothing() {
    // Every block of every run above, `render`'s, is under the allocation
    // trap; this one is the largest the device makes.
    let frames = 8_192;
    let modulator = vec![voice(160.0, frames), fricative(frames)];
    let key = both(&saw(80.0, 0.5, frames));
    for carrier in [0.0, 1.0, 2.0, 3.0] {
        let mut node = vocoder(
            &[
                (CARRIER, carrier),
                (BANDS, 9.0),
                (CHANNEL_MODE, 2.0),
                (UNVOICED, 100.0),
                (ENHANCE, 1.0),
            ],
            4_096,
        );
        let out = render(node.as_mut(), &modulator, Some(&key), 4_096, &[]);
        assert!(out.iter().flatten().all(|s| s.is_finite()));
    }
}

// ---------------------------------------------------------------------------
// R13, R18 to R19: the display and the parameters.

// Spec: vocoder R13
#[test]
fn the_display_reads_each_bands_level_centre_range_and_gate_headless() {
    let description = vocoder_type().description;
    let id = bl_model::DeviceId(bl_core::id::Id::new(1).unwrap());
    let mut values = bl_model::OrdMap::new();
    values.insert(BANDS, 1.0);
    values.insert(RANGE_LOW, 100.0);
    values.insert(RANGE_HIGH, 6_400.0);
    values.insert(GATE, -30.0);
    let shown = bl_devices::display(VOCODER, id, &description, &values, None).unwrap();
    assert!(!shown.live);
    let bands: Vec<_> = shown
        .markers
        .iter()
        .filter(|m| m.handle.is_none())
        .collect();
    assert_eq!(bands.len(), 8);
    // Eight bands from 100 Hz to 6.4 kHz: a factor of 2 between them.
    for (k, band) in bands.iter().enumerate() {
        assert_eq!(band.name, format!("Band {}", k + 1));
        assert!((f64::from(band.x) / (100.0 * 64f64.powf(k as f64 / 7.0)) - 1.0).abs() < 1e-5);
    }
    assert_eq!(shown.series("Gate").unwrap().values, [-30.0, -30.0]);
    let low = shown.markers.iter().find(|m| m.name == "Low End").unwrap();
    assert_eq!(low.handle.unwrap().x, RANGE_LOW);
    assert_eq!(low.x, 100.0);
    let high = shown.markers.iter().find(|m| m.name == "High End").unwrap();
    assert_eq!(high.handle.unwrap().x, RANGE_HIGH);

    // From an engine: the levels as published, a tone in the fifth band at
    // 8 dB under full scale (Depth 100 % plays each band at its level) and
    // little in the rest.
    let hz = centre(8, 100.0, 6_400.0, 4);
    let mut node = vocoder(
        &[(BANDS, 1.0), (RANGE_LOW, 100.0), (RANGE_HIGH, 6_400.0)],
        512,
    );
    let mut reader = node.display().unwrap();
    render(node.as_mut(), &both(&sine(hz, 0.4, 24_000)), None, 512, &[]);
    let data = reader.read().values.clone();
    let live = bl_devices::display(VOCODER, id, &description, &values, Some(&data)).unwrap();
    assert!(live.live);
    let level = |k: usize| live.markers[k].y;
    assert!(level(4) > -12.0 && level(4) < -6.0, "{}", level(4));
    assert!(level(4) > level(0) + 30.0);
    assert!(live.markers[4].text.contains("dB"));
}

// Spec: vocoder R18
#[test]
fn the_parameters_have_the_names_defaults_and_ranges_the_spec_gives() {
    let t = vocoder_type();
    assert_eq!(t.name, "Vocoder");
    assert_eq!(t.role, bl_model::DeviceRole::AudioEffect);
    assert!(t.sidechain);
    let param = |id| t.description.param(id).unwrap();
    assert_eq!(param(BANDS).default, 4.0, "20 bands in a new device");
    assert_eq!(param(RANGE_LOW).default, 80.0);
    assert_eq!(param(RANGE_HIGH).default, 12_000.0);
    assert_eq!(param(BW).default, 100.0);
    assert_eq!(param(DEPTH).default, 100.0);
    assert_eq!(param(ATTACK).default, 5.0);
    assert_eq!(param(RELEASE).default, 100.0);
    assert_eq!(param(GATE).default, -80.0);
    assert_eq!(param(PITCH_LOW).default, 50.0);
    assert_eq!(param(PITCH_HIGH).default, 1_000.0);
    assert_eq!(param(UNVOICED).default, 0.0);
    assert_eq!(param(WAVEFORM).default, 0.0, "a saw");
    let cells = t
        .description
        .layout
        .place(t.description.layout.docked_size())
        .cells;
    assert_eq!(cells.len(), 1, "the band display");
}
