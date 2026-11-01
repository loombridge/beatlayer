//! Roar, the native Roar, part 1 (WP-8.38; the roar spec R1 to R20): its
//! description and defaults, each shaper's curve and the DC after a stage,
//! each filter type's response, the Tone tilt and Color Compensation, the seven
//! routing modes, the oversampled path's aliasing and latency, the output
//! section, the shaper display, and renders that don't depend on the block
//! size. Every block runs under the allocation trap.

use std::f64::consts::{PI, TAU};

use crate::common::response::{Complex, db, mul, response_at};
use crate::common::{self, RATE, device, node, noise, run, set};
use bl_core::id::Id;
use bl_core::time::{Bpm, SampleRate, TempoMap};
use bl_devices::roar::{
    AMOUNT, BIAS, BLEND, COLOR_COMPENSATION, CURVE_POINTS, DRIVE, DRY_WET, FILTER, FILTERS,
    FREQUENCY, HIGH_CROSSOVER, LEVEL, LOW_CROSSOVER, MODES, MORPH, ON, OUTPUT_GAIN, PEAK_GAIN, PRE,
    RESONANCE, ROAR, ROUTING_MODE, SHAPER, SHAPERS, STAGES, TONE_AMOUNT, TONE_FREQUENCY,
    TRACE_POINTS, roar_type, stage_param,
};
use bl_engine::{Block, DeviceType, Node, NodeConfig, Registry, TransportInfo};
use bl_model::{DeviceId, ParamId};

/// The latency of the 4 times oversampler, in frames.
const LATENCY: usize = 48;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::roar::register(&mut registry);
    registry
}

/// A Roar of `params`, up to `max_frames` at a time.
fn roar(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = roar_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

fn p(stage: usize, offset: u32) -> ParamId {
    stage_param(stage, offset)
}

/// The shaper and filter numbers.
const DIGITAL_CLIP: f64 = 1.0;
const BIT_CRUSHER: f64 = 2.0;
const NOISE_INJECTION: f64 = 10.0;
const PEAK: f64 = 4.0;
const MORPH_FILTER: f64 = 5.0;
const COMB: f64 = 6.0;
const RESAMPLING: f64 = 7.0;
const DISPERSION: f64 = 8.0;

/// What makes stage `stage` transparent for a small signal: a Digital Clip at
/// Amount 0 (the signal itself within ±1) and a Peak filter at 0 dB.
fn clean(stage: usize) -> Vec<(ParamId, f64)> {
    vec![
        (p(stage, SHAPER), DIGITAL_CLIP),
        (p(stage, AMOUNT), 0.0),
        (p(stage, FILTER), PEAK),
        (p(stage, PEAK_GAIN), 0.0),
    ]
}

fn with(mut params: Vec<(ParamId, f64)>, more: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    params.extend_from_slice(more);
    params
}

fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

/// The response at each of `hz` of the device `params` set up, from its
/// impulse response to an impulse of a small size, `frames` long.
fn responses(params: &[(ParamId, f64)], hz: &[f64], frames: usize) -> Vec<Complex> {
    let mut node = roar(params, frames);
    let mut input = vec![0.0f32; frames];
    input[0] = SMALL;
    let ir = run(node.as_mut(), &input, &[])[0].clone();
    hz.iter()
        .map(|hz| {
            let (re, im) = response_at(&ir, *hz);
            (re / f64::from(SMALL), im / f64::from(SMALL))
        })
        .collect()
}

/// The impulse that responses are measured with: small enough that no
/// shaper clips what a filter or the tilt does to it.
const SMALL: f32 = 0.05;

fn level_db(response: Complex) -> f64 {
    db(response)
}

// Spec: roar R1, R3, R20
#[test]
fn roar_is_described_with_part_ones_parameters_and_defaults() {
    let device_type = roar_type();
    let description = &device_type.description;
    description.check().unwrap();
    assert_eq!(
        device_type.kind,
        bl_model::DeviceKind::Native {
            device_type: ROAR.to_owned()
        }
    );
    assert_eq!(ROAR, "beatlayer.roar");
    assert_eq!(device_type.name, "Roar");
    assert!(bl_devices::device_types().iter().any(|t| t.name == "Roar"));
    let value = |id: ParamId| description.param(id).unwrap();
    let range = |id: ParamId| (value(id).min(), value(id).max());
    assert_eq!(range(DRIVE), (0.0, 36.0));
    assert_eq!(range(TONE_AMOUNT), (-100.0, 100.0));
    assert_eq!(range(TONE_FREQUENCY), (100.0, 10_000.0));
    assert_eq!(value(TONE_FREQUENCY).default, 1_000.0);
    assert_eq!(range(LOW_CROSSOVER), (50.0, 2_000.0));
    assert_eq!(value(LOW_CROSSOVER).default, 250.0);
    assert_eq!(range(HIGH_CROSSOVER), (500.0, 16_000.0));
    assert_eq!(value(HIGH_CROSSOVER).default, 2_500.0);
    assert_eq!(range(DRY_WET), (0.0, 100.0));
    assert_eq!(value(DRY_WET).default, 100.0);
    assert!(value(OUTPUT_GAIN).is_gain());
    assert_eq!(
        value(OUTPUT_GAIN).format(value(OUTPUT_GAIN).default),
        "0.00 dB"
    );
    assert_eq!(value(OUTPUT_GAIN).max(), 10f64.powf(12.0 / 20.0));
    assert_eq!(MODES.len(), 7);
    assert_eq!(SHAPERS.len(), 12);
    assert_eq!(FILTERS.len(), 9);
    assert_eq!(STAGES, 3);
    assert_eq!(description.views, ["Stage 1", "Stage 2", "Stage 3"]);
    for stage in 0..STAGES {
        let n = stage + 1;
        assert_eq!(value(p(stage, ON)).default, 1.0, "stage {n} is on");
        assert_eq!(value(p(stage, SHAPER)).format(0.0), "Soft Sine");
        assert_eq!(range(p(stage, AMOUNT)), (0.0, 100.0));
        assert_eq!(value(p(stage, AMOUNT)).default, 0.0);
        assert_eq!(range(p(stage, BIAS)), (-100.0, 100.0));
        assert_eq!(value(p(stage, BIAS)).default, 0.0);
        assert!(value(p(stage, LEVEL)).is_gain());
        assert_eq!(value(p(stage, LEVEL)).default, 1.0, "0 dB");
        assert_eq!(value(p(stage, LEVEL)).max(), 10f64.powf(12.0 / 20.0));
        assert_eq!(value(p(stage, FILTER)).format(0.0), "LP");
        assert_eq!(range(p(stage, FREQUENCY)), (20.0, 20_000.0));
        assert_eq!(value(p(stage, FREQUENCY)).default, 20_000.0);
        assert_eq!(range(p(stage, RESONANCE)), (0.0, 100.0));
        assert_eq!(range(p(stage, PEAK_GAIN)), (-24.0, 24.0));
        assert_eq!(value(p(stage, PEAK_GAIN)).default, 0.0);
        assert_eq!(range(p(stage, MORPH)), (0.0, 100.0));
        assert_eq!(
            value(p(stage, PRE)).default,
            0.0,
            "the filter after the shaper"
        );
    }
    // Part 1's parameters, 10 and 11 for each stage, keep their numbers; part
    // 2's follow them (R20).
    assert!(description.params.len() > 10 + 11 * STAGES);
    for (i, param) in description.params.iter().enumerate() {
        assert_eq!(param.id.0 as usize, i, "descriptor numbers run in order");
    }
}

// Spec: roar R2
#[test]
fn drive_raises_the_signal_into_the_stage() {
    let input = sine(1_000.0, 0.05, 4_800);
    let peak = |drive: f64| {
        let out = run(
            roar(&with(clean(0), &[(DRIVE, drive)]), 4_800).as_mut(),
            &input,
            &[],
        );
        out[0][3_000..4_800]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    };
    let flat = f64::from(peak(0.0));
    assert!((flat - 0.05).abs() < 0.001, "{flat}");
    let raised = f64::from(peak(20.0));
    assert!((raised - 0.5).abs() < 0.01, "+20 dB makes 0.5: {raised}");
}

// Spec: roar R2
#[test]
fn tone_tilts_the_spectrum_and_color_compensation_undoes_it_after_the_stages() {
    let hz = [100.0, 300.0, 1_000.0, 3_000.0, 8_000.0];
    for tone in [-100.0, -50.0, 50.0, 100.0] {
        let params = with(clean(0), &[(TONE_AMOUNT, tone), (TONE_FREQUENCY, 1_000.0)]);
        let tilted: Vec<f64> = responses(&params, &hz, 4_096)
            .into_iter()
            .map(level_db)
            .collect();
        let slope = tilted[4] - tilted[0];
        assert!(
            slope * tone.signum() > 0.18 * tone.abs(),
            "tone {tone}: highs against lows is {slope} dB"
        );
        assert!(tilted[2].abs() < 3.0, "about the centre it is level");
        let compensated = with(params, &[(COLOR_COMPENSATION, 1.0)]);
        for (at, response) in hz.iter().zip(responses(&compensated, &hz, 4_096)) {
            let level = level_db(response);
            assert!(level.abs() < 0.5, "tone {tone} at {at} Hz: {level} dB");
        }
    }
}

/// The spec's curves (R4): `kind` of `g x + bias` less the curve at the bias.
fn formula(kind: usize, amount: f64, bias: f64, x: f64) -> f64 {
    let gain = if kind == 2 {
        1.0
    } else {
        10f64.powf(amount * 36.0 / 20.0)
    };
    let f = |u: f64| {
        let c = u.clamp(-1.0, 1.0);
        match kind {
            0 => (PI * c / 2.0).sin(),
            1 => c,
            2 => {
                let steps = 2f64.powf(16.0 - 14.0 * amount - 1.0);
                (c * steps).round() / steps
            }
            3 => u / (1.0 + u.abs().powf(2.5)).powf(1.0 / 2.5),
            4 => {
                if u >= 0.0 {
                    u.tanh()
                } else {
                    0.7 * (u / 0.7).tanh()
                }
            }
            5 => u.max(0.0).tanh(),
            6 => u.abs().tanh(),
            7 => 1.5 * (c - c.powi(3) / 3.0),
            8 => {
                (0..4)
                    .map(|k| 0.5f64.powi(k) * (2f64.powi(k) * PI * c / 2.0).sin())
                    .sum::<f64>()
                    / 1.875
            }
            9 => 2.0 / PI * (PI * u / 2.0).sin().asin(),
            _ => ((8.0 * c).floor() + (8.0 * c - (8.0 * c).floor()).powi(4)) / 8.0,
        }
    };
    f(gain * x + bias) - f(bias)
}

/// A 30 Hz sine swept slowly from silence up to `top` through a stage with
/// shaper `kind` at `amount` and `bias`, at Level 0.4 (so the output's hard
/// clip is never reached): the stage's output, after the latency, and the
/// formula's curve over the input, both as the 5 Hz blocker leaves them (the
/// formula's has the same blocker applied) and in the curve's own units.
fn curve_run(kind: usize, amount: f64, bias: f64, top: f64) -> (Vec<f64>, Vec<f64>) {
    let frames = 24_000;
    let input: Vec<f32> = (0..frames)
        .map(|i| {
            let swell = top * i as f64 / frames as f64;
            (swell * (TAU * 30.0 * i as f64 / RATE.as_f64()).sin()) as f32
        })
        .collect();
    let params = with(
        clean(0),
        &[
            (p(0, SHAPER), kind as f64),
            (p(0, AMOUNT), amount * 100.0),
            (p(0, BIAS), bias * 100.0),
            (p(0, LEVEL), 0.4),
        ],
    );
    let out = run(roar(&params, frames).as_mut(), &input, &[]);
    let pole = (-TAU * 5.0 / RATE.as_f64()).exp();
    let (mut previous, mut held) = (0.0f64, 0.0f64);
    let (mut measured, mut expected) = (Vec::new(), Vec::new());
    for n in 0..frames - LATENCY {
        let curve = formula(kind, amount, bias, f64::from(input[n]));
        held = curve - previous + pole * held;
        previous = curve;
        if n > 1_000 {
            measured.push(f64::from(out[0][n + LATENCY]) / 0.4);
            expected.push(held);
        }
    }
    (measured, expected)
}

// Spec: roar R4
#[test]
fn each_shapers_curve_lies_within_a_hundredth_of_its_formula() {
    for kind in (0..SHAPERS.len()).filter(|k| *k != NOISE_INJECTION as usize) {
        // The downsampler's passband ends near 16 kHz, so a curve's kinks are
        // measured slowly, and up to the input where the curve is steepest.
        for (amount, bias, top) in [(0.5, 0.0, 0.5), (0.2, 0.0, 1.2), (0.5, 0.3, 0.5)] {
            let (measured, expected) = curve_run(kind, amount, bias, top);
            let error = measured
                .iter()
                .zip(&expected)
                .map(|(m, e)| (m - e).abs())
                .fold(0.0, f64::max);
            assert!(
                error < 0.01,
                "{} at amount {amount}, bias {bias}, up to {top}: off by {error}",
                SHAPERS[kind]
            );
        }
    }
}

// Spec: roar R4
#[test]
fn bit_crusher_amount_sets_the_bits_and_noise_injection_follows_the_devices_seed() {
    // Amount 100 % is two bits: steps of one half either side of zero; the
    // output is the formula's but at the steps' edges, where the downsampler
    // rings.
    let (measured, expected) = curve_run(2, 1.0, 0.0, 0.9);
    let near = measured
        .iter()
        .zip(&expected)
        .filter(|(m, e)| (*m - *e).abs() < 0.05)
        .count();
    assert!(
        near * 100 > measured.len() * 98,
        "{near} of {}",
        measured.len()
    );
    // Noise Injection: the same device gives the same noise, another gives
    // another; at Amount 0 it is a plain tanh.
    let input = sine(2_000.0, 0.5, 4_800);
    let render = |id: u64, amount: f64| {
        let device_type = roar_type();
        let mut device = device_type.device(DeviceId(Id::new(id).unwrap()));
        for (param, value) in clean(0) {
            device.params.insert(param, value);
        }
        device.params.insert(p(0, SHAPER), NOISE_INJECTION);
        device.params.insert(p(0, AMOUNT), amount);
        let mut node = registry()
            .create(&device, &common::config(4_800))
            .expect("the registry makes the device");
        run(node.as_mut(), &input, &[])[0].clone()
    };
    assert!(render(1, 50.0) == render(1, 50.0), "run after run");
    assert!(
        render(1, 50.0) != render(2, 50.0),
        "another device, another noise"
    );
    let plain = render(1, 0.0);
    let error = (2_400..4_800 - LATENCY)
        .map(|n| (f64::from(plain[n + LATENCY]) - f64::from(input[n]).tanh()).abs())
        .fold(0.0, f64::max);
    assert!(error < 0.01, "{error}");
    let noisy = render(1, 50.0);
    let spread = (2_400..4_800 - LATENCY)
        .map(|n| {
            let gain = 10f64.powf(0.5 * 36.0 / 20.0);
            (f64::from(noisy[n + LATENCY]) - (gain * f64::from(input[n])).tanh()).abs()
        })
        .fold(0.0, f64::max);
    assert!(spread > 0.05, "the noise is heard: {spread}");
}

// Spec: roar R5
#[test]
fn a_stage_leaves_no_dc_after_a_shaper_that_adds_some() {
    let frames = 48_000;
    let input = sine(1_000.0, 0.7, frames);
    // -80 dBFS is 1e-4.
    for (kind, bias) in [(5, 0.0), (6, 0.0), (4, 0.0), (0, 0.5), (3, 0.4)] {
        let params = with(
            clean(0),
            &[
                (p(0, SHAPER), kind as f64),
                (p(0, AMOUNT), 60.0),
                (p(0, BIAS), bias * 100.0),
                // Under the output's hard clip, which a stage's DC isn't
                // measured past.
                (p(0, LEVEL), 0.4),
            ],
        );
        let out = run(roar(&params, frames).as_mut(), &input, &[]);
        let tail = &out[0][frames - 4_800..];
        let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
        assert!(mean.abs() < 1e-4, "{}: DC {mean}", SHAPERS[kind]);
        let raw: f64 = (0..4_800)
            .map(|n| formula(kind, 0.6, bias, f64::from(input[n])))
            .sum::<f64>()
            / 4_800.0;
        assert!(
            raw.abs() > 0.02,
            "{}: the curve itself has DC {raw}",
            SHAPERS[kind]
        );
    }
}

/// The spec's Auto Filter section at `hz` for a filter at `fc` with quality
/// `q` at `rate`: its low-pass, band-pass (0 dB at `fc`), high-pass and notch.
fn section(hz: f64, fc: f64, q: f64, rate: f64) -> [Complex; 4] {
    let x = (PI * hz / rate).tan() / (PI * fc / rate).tan();
    let k = 1.0 / q;
    let div = |n: Complex, d: Complex| {
        let size = d.0 * d.0 + d.1 * d.1;
        (
            (n.0 * d.0 + n.1 * d.1) / size,
            (n.1 * d.0 - n.0 * d.1) / size,
        )
    };
    let den = (1.0 - x * x, k * x);
    [
        div((1.0, 0.0), den),
        div((0.0, k * x), den),
        div((-x * x, 0.0), den),
        div((1.0 - x * x, 0.0), den),
    ]
}

/// The designed response of filter `kind` at `hz`, for a filter at 1 kHz with
/// resonance `res` and morph `morph` (fractions) and `peak_db`, at the stage's
/// four times rate.
fn designed(kind: usize, hz: f64, res: f64, morph: f64, peak_db: f64) -> Complex {
    let rate = 4.0 * RATE.as_f64();
    let fc = 1_000.0;
    let q = std::f64::consts::FRAC_1_SQRT_2 * (10.0 / std::f64::consts::FRAC_1_SQRT_2).powf(res);
    let [lp, bp, hp, notch] = section(hz, fc, q, rate);
    let mixed = |a: Complex, b: Complex, t: f64| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    match kind {
        0 => lp,
        1 => bp,
        2 => hp,
        3 => notch,
        4 => {
            let g = 10f64.powf(peak_db / 20.0);
            (1.0 + (g - 1.0) * bp.0, (g - 1.0) * bp.1)
        }
        5 => {
            if morph < 0.5 {
                mixed(lp, bp, morph * 2.0)
            } else {
                mixed(bp, hp, morph * 2.0 - 1.0)
            }
        }
        6 => {
            // A feedback comb whose delay is one cycle of 1 kHz.
            let c = 0.95 * res;
            let w = TAU * hz / fc;
            let (re, im) = (1.0 - c * w.cos(), c * w.sin());
            let size = re * re + im * im;
            ((1.0 - c) * re / size, -(1.0 - c) * im / size)
        }
        _ => {
            // Eight allpass sections of Q 0.5 to 8.
            let q = 0.5 + 7.5 * res;
            let [_, bp, _, _] = section(hz, fc, q, rate);
            let all = (1.0 - 2.0 * bp.0, -2.0 * bp.1);
            (0..7).fold(all, |acc, _| mul(acc, all))
        }
    }
}

// Spec: roar R6
#[test]
fn each_filter_types_response_is_within_half_a_decibel_of_its_design() {
    let hz = [100.0, 400.0, 700.0, 1_500.0, 2_500.0, 6_000.0];
    for kind in [0, 1, 2, 3, 4, 5, 6, 8] {
        for (res, morph, peak_db) in [(0.0, 0.2, 9.0), (0.6, 0.7, -12.0)] {
            let params = with(
                clean(0),
                &[
                    (p(0, FILTER), kind as f64),
                    (p(0, FREQUENCY), 1_000.0),
                    (p(0, RESONANCE), res * 100.0),
                    (p(0, MORPH), morph * 100.0),
                    (p(0, PEAK_GAIN), peak_db),
                ],
            );
            let measured = responses(&params, &hz, 8_192);
            for (at, response) in hz.iter().zip(measured) {
                let design = designed(kind, *at, res, morph, peak_db);
                if db(design) < -45.0 {
                    continue;
                }
                let error = (level_db(response) - db(design)).abs();
                assert!(
                    error < 0.5,
                    "{} at {at} Hz, resonance {res}, morph {morph}: {error} dB off",
                    FILTERS[kind]
                );
                if kind == 8 {
                    // Dispersion smears the phase: the response, less the
                    // oversampler's latency, is the allpasses'.
                    let w = TAU * at / RATE.as_f64();
                    let lag = (w * LATENCY as f64).cos();
                    let shifted = mul(response, (lag, (w * LATENCY as f64).sin()));
                    let off = (shifted.0 - design.0).hypot(shifted.1 - design.1);
                    assert!(off < 0.06, "Dispersion at {at} Hz: phase off by {off}");
                }
            }
        }
    }
    // Morph's ends are the shared types'.
    let at_end = |morph: f64| {
        let params = with(
            clean(0),
            &[
                (p(0, FILTER), MORPH_FILTER),
                (p(0, FREQUENCY), 1_000.0),
                (p(0, MORPH), morph),
            ],
        );
        responses(&params, &[100.0, 8_000.0], 4_096)
            .into_iter()
            .map(level_db)
            .collect::<Vec<_>>()
    };
    assert!(at_end(0.0)[0] > at_end(0.0)[1] + 30.0, "low-pass at 0 %");
    assert!(
        at_end(100.0)[1] > at_end(100.0)[0] + 30.0,
        "high-pass at 100 %"
    );
}

// Spec: roar R6
#[test]
fn resampling_holds_the_signal_so_a_high_tone_aliases_and_a_low_one_passes() {
    let frames = 14_448;
    let amplitude_at = |hz: f64, out_hz: f64| {
        let input = sine(hz, 0.5, frames);
        let params = with(
            clean(0),
            &[(p(0, FILTER), RESAMPLING), (p(0, FREQUENCY), 2_000.0)],
        );
        let out = run(roar(&params, frames).as_mut(), &input, &[]);
        let window = &out[0][LATENCY + 9_600..LATENCY + 9_600 + 4_800];
        magnitude(window, (out_hz / 10.0) as usize)
    };
    let low = amplitude_at(200.0, 200.0);
    assert!((low - 0.5).abs() < 0.03, "a low tone passes: {low}");
    let tone = amplitude_at(3_000.0, 3_000.0);
    let alias = amplitude_at(3_000.0, 1_000.0);
    assert!(
        alias > 2.0 * tone,
        "3 kHz held at 2 kHz folds to 1 kHz: {alias} against {tone}"
    );
}

/// The magnitude of `samples` at `bin` of a DFT of its length.
fn magnitude(samples: &[f32], bin: usize) -> f64 {
    let n = samples.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in samples.iter().enumerate() {
        let phase = TAU * bin as f64 * i as f64 / n;
        re += f64::from(*s) * phase.cos();
        im -= f64::from(*s) * phase.sin();
    }
    (re * re + im * im).sqrt() * 2.0 / n
}

/// The size of the largest peak of a steady sine's output per period: the
/// samples a quarter period in, every `period` frames, from `first`.
fn envelope(out: &[f32], first: usize, period: usize, count: usize) -> Vec<f64> {
    (0..count)
        .map(|k| f64::from(out[first + k * period + period / 4].abs()))
        .collect()
}

// Spec: roar R7
#[test]
fn amount_glides_over_twenty_milliseconds() {
    let frames = 4_800;
    let input = sine(1_000.0, 0.01, frames);
    let mut node = roar(&clean(0), frames);
    let out = run(node.as_mut(), &input, &[set(0, p(0, AMOUNT), 100.0)]);
    let peak = envelope(&out[0], LATENCY, 48, 80);
    // The gain rises along a straight line in dB: 0.01 to 0.63 over 960 frames.
    assert!(peak[0] < 0.02, "it starts at the old value: {}", peak[0]);
    assert!(
        (peak[10] - 0.01 * 10f64.powf(0.5 * 36.0 / 20.0)).abs() < 0.02,
        "half-way at 480 frames: {}",
        peak[10]
    );
    assert!(
        (peak[30] - 0.631).abs() < 0.01,
        "and there at 960: {}",
        peak[30]
    );
    assert!(peak.windows(2).all(|w| w[1] >= w[0] - 1e-4), "rising");
}

// Spec: roar R7
#[test]
fn a_change_of_shaper_or_filter_crossfades_over_ten_milliseconds() {
    // A square wave of 0.5 at 1 kHz: Digital Clip at Amount 100 % makes it
    // ±1, Bit Crusher at 100 % keeps ±0.5.
    let frames = 4_800;
    let square: Vec<f32> = (0..frames)
        .map(|i| if (i / 24) % 2 == 0 { 0.5 } else { -0.5 })
        .collect();
    let params = with(clean(0), &[(p(0, AMOUNT), 100.0)]);
    let mut node = roar(&params, frames);
    let out = run(
        node.as_mut(),
        &square,
        &[set(2_400, p(0, SHAPER), BIT_CRUSHER)],
    );
    // The middle of each high half.
    let plateau = |k: usize| f64::from(out[0][LATENCY + k * 48 + 12]);
    assert!((plateau(45) - 1.0).abs() < 0.03, "before: {}", plateau(45));
    assert!(
        (plateau(55) - 0.75).abs() < 0.08,
        "half-way: {}",
        plateau(55)
    );
    assert!((plateau(65) - 0.5).abs() < 0.03, "after: {}", plateau(65));
    assert!(
        (50..61).all(|k| plateau(k + 1) <= plateau(k) + 0.01),
        "falling steadily"
    );
    // A filter change: from a low-pass at 20 kHz, which passes 1 kHz, to a
    // high-pass at 20 kHz, which doesn't.
    let tone = sine(1_000.0, 0.3, frames);
    let params = [
        (p(0, FILTER), 0.0),
        (p(0, SHAPER), DIGITAL_CLIP),
        (p(0, AMOUNT), 0.0),
    ];
    let mut node = roar(&params, frames);
    let out = run(node.as_mut(), &tone, &[set(2_400, p(0, FILTER), 2.0)]);
    let peak = envelope(&out[0], LATENCY, 48, 80);
    assert!((peak[45] - 0.3).abs() < 0.02, "before: {}", peak[45]);
    assert!((peak[55] - 0.15).abs() < 0.05, "half-way: {}", peak[55]);
    assert!(peak[65] < 0.02, "after: {}", peak[65]);
}

/// The size of the device's gain at `hz` for the settings `params`.
fn gain_at(params: &[(ParamId, f64)], hz: f64) -> f64 {
    let response = responses(params, &[hz], 4_096)[0];
    response.0.hypot(response.1)
}

// Spec: roar R8, R9
#[test]
fn single_runs_stage_one_alone() {
    let input = noise(2_048);
    let render = |second: f64| {
        let params = with(
            clean(0),
            &[
                (p(1, AMOUNT), second),
                (p(1, SHAPER), 4.0),
                (p(0, LEVEL), 0.5),
            ],
        );
        run(roar(&params, 2_048).as_mut(), &input, &[])
    };
    assert!(render(0.0) == render(100.0), "stages 2 and 3 aren't heard");
    assert!((gain_at(&with(clean(0), &[(p(0, LEVEL), 0.5)]), 1_000.0) - 0.5).abs() < 0.01);
}

/// Stage gains that tell the stages apart: each transparent stage a `Level`.
fn leveled(levels: &[f64]) -> Vec<(ParamId, f64)> {
    let mut params = Vec::new();
    for (stage, level) in levels.iter().enumerate() {
        params.extend(clean(stage));
        params.push((p(stage, LEVEL), *level));
    }
    params
}

// Spec: roar R8, R9
#[test]
fn serial_and_parallel_crossfade_their_two_signals_by_equal_power_laws() {
    for (mode, second) in [(1.0, 0.125), (2.0, 0.25)] {
        for (blend, first_gain, second_gain) in [
            (0.0, 1.0, 0.0),
            (
                50.0,
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ),
            (100.0, 0.0, 1.0),
        ] {
            let params = with(
                leveled(&[0.5, 0.25]),
                &[(ROUTING_MODE, mode), (BLEND, blend)],
            );
            let expected = first_gain * 0.5 + second_gain * second;
            let got = gain_at(&params, 1_000.0);
            assert!(
                (got - expected).abs() < 0.01,
                "{} at {blend} %: {got}, not {expected}",
                MODES[mode as usize]
            );
        }
    }
}

// Spec: roar R8
#[test]
fn multi_band_runs_the_stages_on_three_bands_and_sums_flat_with_them_off() {
    let hz: Vec<f64> = (0..16)
        .map(|i| 50.0 * 1.6f64.powi(i))
        .filter(|f| *f < 12_000.0)
        .collect();
    let off: Vec<(ParamId, f64)> = (0..STAGES).map(|s| (p(s, ON), 0.0)).collect();
    let flat = with(off, &[(ROUTING_MODE, 3.0)]);
    for (at, response) in hz.iter().zip(responses(&flat, &hz, 8_192)) {
        assert!(
            level_db(response).abs() < 0.1,
            "{at} Hz: {} dB",
            level_db(response)
        );
    }
    let bands = with(leveled(&[0.5, 0.25, 0.125]), &[(ROUTING_MODE, 3.0)]);
    for (hz, level) in [(80.0, 0.5), (800.0, 0.25), (9_000.0, 0.125)] {
        let got = gain_at(&bands, hz);
        assert!(
            (got / level - 1.0).abs() < 0.06,
            "{hz} Hz: {got}, not {level}"
        );
    }
    // The crossovers move with their parameters.
    let moved = with(
        bands,
        &[(LOW_CROSSOVER, 1_000.0), (HIGH_CROSSOVER, 8_000.0)],
    );
    assert!(
        (gain_at(&moved, 500.0) / 0.5 - 1.0).abs() < 0.1,
        "500 Hz is low now"
    );
}

/// Runs stereo `left` and `right` through `node`.
fn run_stereo(node: &mut dyn Node, left: &[f32], right: &[f32]) -> Vec<Vec<f32>> {
    common::run_at(
        node,
        &[left.to_vec(), right.to_vec()],
        &[],
        &TempoMap::constant(Bpm::DEFAULT),
        TransportInfo::default(),
    )
}

// Spec: roar R8
#[test]
fn mid_side_round_trips_and_runs_stage_one_on_the_mid_and_stage_two_on_the_side() {
    let frames = 4_096;
    let left = sine(700.0, 0.4, frames);
    let right = sine(1_900.0, 0.3, frames);
    let off: Vec<(ParamId, f64)> = (0..STAGES).map(|s| (p(s, ON), 0.0)).collect();
    let params = with(off, &[(ROUTING_MODE, 4.0)]);
    let out = run_stereo(roar(&params, frames).as_mut(), &left, &right);
    for n in 256..frames - LATENCY {
        assert!((out[0][n + LATENCY] - left[n]).abs() < 2e-3, "left at {n}");
        assert!(
            (out[1][n + LATENCY] - right[n]).abs() < 2e-3,
            "right at {n}"
        );
    }
    // With stage 1 silent only the side is left, with stage 2 silent the mid.
    let silent = |stage: usize| {
        let params = with(
            leveled(&[1.0, 1.0]),
            &[(ROUTING_MODE, 4.0), (p(stage, LEVEL), 0.0)],
        );
        run_stereo(roar(&params, frames).as_mut(), &left, &right)
    };
    let side = silent(0);
    let mid = silent(1);
    for n in 256..frames - LATENCY {
        let (l, r) = (f64::from(left[n]), f64::from(right[n]));
        // The stage's 5 Hz blocker takes a little off the lows.
        assert!((f64::from(side[0][n + LATENCY]) - (l - r) / 2.0).abs() < 8e-3);
        assert!((f64::from(side[1][n + LATENCY]) + (l - r) / 2.0).abs() < 8e-3);
        assert!((f64::from(mid[0][n + LATENCY]) - (l + r) / 2.0).abs() < 8e-3);
        assert!((f64::from(mid[1][n + LATENCY]) - (l + r) / 2.0).abs() < 8e-3);
    }
}

/// The index of the largest value of `samples` from `from`, and the value.
fn peak_after(samples: &[f32], from: usize) -> (usize, f64) {
    let (at, value) = samples
        .iter()
        .enumerate()
        .skip(from)
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .expect("samples");
    (at, f64::from(*value))
}

/// Renders `frames` frames of `input` through a Roar of `params` in blocks of
/// `block`.
fn render(params: &[(ParamId, f64)], input: &[f32], block: usize) -> Vec<f32> {
    let mut node = roar(params, block);
    input
        .chunks(block)
        .flat_map(|chunk| run(node.as_mut(), chunk, &[])[0].clone())
        .collect()
}

// Spec: roar R8, R10
#[test]
fn the_delay_and_feedback_loops_are_a_fixed_number_of_frames_at_any_block_size() {
    let frames = 4_800 * 3 + 200;
    let mut impulse = vec![0.0f32; frames];
    impulse[0] = 0.5;
    // 100 ms: 4,800 frames at 48 kHz, whole frames at the high rate too.
    let loop_frames = 4_800;
    // What the impulse becomes through the stages once, the trip's own
    // filtering included.
    let through = peak_after(&render(&leveled(&[1.0, 1.0]), &impulse, 4_096), 0).1;
    let delay = with(leveled(&[1.0, 1.0]), &[(ROUTING_MODE, 6.0), (BLEND, 100.0)]);
    let feedback = with(leveled(&[1.0, 1.0]), &[(ROUTING_MODE, 5.0), (BLEND, 50.0)]);
    for (params, name) in [(delay, "Delay"), (feedback, "Feedback")] {
        let reference = render(&params, &impulse, 4_096);
        for block in [1, 64, 512] {
            assert!(
                render(&params, &impulse, block) == reference,
                "{name} at {block}"
            );
        }
        let (first, size) = peak_after(&reference, LATENCY + loop_frames - 50);
        if name == "Delay" {
            assert_eq!(
                first,
                LATENCY + loop_frames,
                "{name}: a loop after the input"
            );
            assert!(
                (size / through - 1.0).abs() < 0.03,
                "heard once at its level: {size}"
            );
            // What returns passes the feedback band-pass (R24), which adds
            // a few frames and takes some off.
            let (second, size) = peak_after(&reference, LATENCY + loop_frames + 50);
            let late = second - (LATENCY + 2 * loop_frames);
            assert!(
                (0..40).contains(&late),
                "then fed back round the loop: {late}"
            );
            assert!((0.05..0.5).contains(&(size / through)), "{size}");
        } else {
            // The loop's length makes up for the band-pass's delay, so the
            // first return lands within a few frames of the loop's time.
            let late = first as i64 - (LATENCY + loop_frames) as i64;
            assert!(late.abs() < 12, "{name}: a loop after the input: {late}");
            // An equal-power half each way, and a half returning, less the
            // band-pass's share.
            assert!((0.02..0.25).contains(&(size / through)), "{size}");
        }
    }
}

/// How far below the loudest of the 5, 10 and 15 kHz components the loudest
/// other whole-kilohertz component below 20 kHz of `samples` (a whole number
/// of the sine's periods) is: the fold-back of harmonics past the rate.
fn alias_db(samples: &[f32], rate: f64) -> f64 {
    let bin = |hz: f64| (hz / (rate / samples.len() as f64)).round() as usize;
    let at = |k: u32| magnitude(samples, bin(f64::from(k) * 1_000.0));
    let loudest_harmonic = [5, 10, 15].into_iter().map(at).fold(0.0, f64::max);
    let loudest = (1..20)
        .filter(|k| ![5, 10, 15].contains(k))
        .map(at)
        .fold(0.0, f64::max);
    20.0 * (loudest / loudest_harmonic).log10()
}

/// Runs `input` (the same on both channels) through a Roar of `params` at
/// `rate`, one block, and returns the left channel.
fn run_at_rate(params: &[(ParamId, f64)], rate: SampleRate, input: &[f32]) -> Vec<f32> {
    let frames = input.len();
    let config = NodeConfig {
        round_trip_latency: 0,
        sample_rate: rate,
        max_frames: frames,
        channels: common::CHANNELS,
    };
    let device_type = roar_type();
    let mut node = registry()
        .create(&device(&device_type, params), &config)
        .expect("the registry makes the device");
    let mut data: Vec<f32> = input.iter().chain(input).copied().collect();
    {
        let mut block = Block::new(
            &mut data,
            common::CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            rate,
        );
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data[..frames].to_vec()
}

// Spec: roar R11
#[test]
fn a_5_khz_sine_at_amount_100_leaves_no_alias_through_a_shaper_the_four_times_path_can_hold() {
    // Past 96 kHz a shaper's harmonics fold back inside the four times rate,
    // and no filter takes them out again: a curve with a kink or a step
    // (the rectifiers, Shards, Bit Crusher) leaves them above -70 dB, and
    // Noise Injection is noise. The others hold -70 dB at -36 dBFS.
    const KINKED: [&str; 5] = [
        "Half Wave Rectifier",
        "Full Wave Rectifier",
        "Shards",
        "Bit Crusher",
        "Noise Injection",
    ];
    for (rate, window) in [
        (SampleRate::HZ_48000, 4_800usize),
        (SampleRate::HZ_44100, 4_410),
    ] {
        let hz = rate.as_f64();
        let frames = 9_600 + LATENCY + window;
        let input: Vec<f32> = (0..frames)
            .map(|i| (0.016 * (TAU * 5_000.0 * i as f64 / hz).sin()) as f32)
            .collect();
        for (kind, name) in SHAPERS.iter().enumerate() {
            if KINKED.contains(name) {
                continue;
            }
            let params = vec![
                (p(0, SHAPER), kind as f64),
                (p(0, AMOUNT), 100.0),
                (p(0, LEVEL), 0.4),
            ];
            let out = run_at_rate(&params, rate, &input);
            let from = LATENCY + 9_600;
            let alias = alias_db(&out[from..from + window], hz);
            assert!(
                alias < -70.0,
                "{name} at {hz} Hz: {alias} dB below the harmonics"
            );
        }
    }
}

// Spec: roar R11
#[test]
fn the_latency_is_reported_and_the_dry_signal_is_delayed_by_it() {
    assert_eq!(roar(&[], 64).latency() as usize, LATENCY);
    let input = noise(1_024);
    let out = run(
        roar(&[(DRY_WET, 0.0), (p(0, AMOUNT), 100.0)], 1_024).as_mut(),
        &input,
        &[],
    );
    assert!(out[0][..LATENCY].iter().all(|s| *s == 0.0));
    assert_eq!(out[0][LATENCY..], input[..1_024 - LATENCY]);
}

// Spec: roar R13
#[test]
fn output_gain_sets_the_wet_level_into_a_hard_clip_and_dry_wet_balances_it() {
    let input = sine(1_000.0, 0.5, 4_800);
    let peak = |params: &[(ParamId, f64)]| {
        let out = run(roar(&with(clean(0), params), 4_800).as_mut(), &input, &[]);
        out[0][2_400..].iter().fold(0.0f32, |m, s| m.max(s.abs()))
    };
    assert!((peak(&[]) - 0.5).abs() < 0.01);
    assert!((peak(&[(OUTPUT_GAIN, 0.5)]) - 0.25).abs() < 0.01);
    let loud = peak(&[(OUTPUT_GAIN, 10f64.powf(12.0 / 20.0))]);
    assert_eq!(
        loud, 1.0,
        "+12 dB on 0.5 is past full scale, and clips there"
    );
    assert_eq!(peak(&[(OUTPUT_GAIN, 0.0)]), 0.0, "-inf dB is silence");
    // Half dry, half silent wet: the dry signal, delayed, at half.
    let mut node = roar(
        &with(clean(0), &[(p(0, LEVEL), 0.0), (DRY_WET, 50.0)]),
        4_800,
    );
    let out = run(node.as_mut(), &input, &[]);
    for n in LATENCY..4_800 {
        assert!(
            (out[0][n] - 0.5 * input[n - LATENCY]).abs() < 1e-6,
            "at {n}"
        );
    }
}

fn shown(values: &[(ParamId, f64)], view: &str, data: Option<&[f32]>) -> bl_model::DeviceDisplay {
    let device_type = roar_type();
    let mut set = bl_model::OrdMap::new();
    for (id, value) in values {
        set.insert(*id, *value);
    }
    let id = DeviceId(Id::new(1).unwrap());
    bl_devices::display_for_view(ROAR, id, &device_type.description, &set, data, Some(view))
        .expect("Roar draws a display")
}

// Spec: roar R12
#[test]
fn the_display_draws_each_stages_curve_with_its_bias_and_the_level_on_it() {
    let two = [
        (p(0, SHAPER), DIGITAL_CLIP),
        (p(0, AMOUNT), 0.0),
        (p(1, SHAPER), 6.0),
        (p(1, AMOUNT), 50.0),
        (p(1, BIAS), 0.0),
    ];
    let first = shown(&two, "Stage 1", None);
    let curve = first.series("Shaper Curve").unwrap();
    assert_eq!(curve.values.len(), CURVE_POINTS);
    assert_eq!(curve.values.len(), 128);
    assert_eq!(curve.values[0], -1.0);
    assert_eq!(curve.values[127], 1.0, "a straight line at Amount 0");
    assert!(!first.live);
    let second = shown(&two, "Stage 2", None);
    let curve = second.series("Shaper Curve").unwrap();
    assert!(
        curve.values.iter().all(|v| *v >= 0.0),
        "Full Wave Rectifier"
    );
    assert!((curve.values[0] - (10f64.powf(0.5 * 36.0 / 20.0)).tanh() as f32).abs() < 1e-4);
    // A bias shifts the curve and keeps silence silent.
    let biased = shown(
        &[
            (p(0, SHAPER), 0.0),
            (p(0, AMOUNT), 30.0),
            (p(0, BIAS), 40.0),
        ],
        "Stage 1",
        None,
    );
    let curve = biased.series("Shaper Curve").unwrap();
    assert!(curve.values[63].abs() < 0.2 && curve.values[64].abs() < 0.2);
    let middle = (curve.values[63] + curve.values[64]) / 2.0;
    assert!(middle.abs() < 0.05, "silence in is silence out: {middle}");
    assert!(
        (curve.values[0] + curve.values[127]).abs() > 0.05,
        "lopsided"
    );
    // The node publishes the level that reached each stage.
    let mut node = roar(&clean(0), 256);
    let mut reader = node.display().expect("the device has a display");
    run(node.as_mut(), &[0.25; 256], &[]);
    let data = reader.read();
    assert_eq!(data.published, 1);
    assert_eq!(data.values.len(), STAGES + 4 * TRACE_POINTS);
    assert!((0.25..0.3).contains(&data.values[0]), "{:?}", data.values);
    let live = shown(&[], "Stage 1", Some(&data.values));
    assert!(live.live);
    assert_eq!(live.markers.len(), 1);
    assert!((live.reading("Input").unwrap().value + 11.0).abs() < 1.5);
}

// Spec: roar R15
#[test]
fn renders_are_bit_identical_at_four_block_sizes_and_run_after_run() {
    let frames = 4_096;
    let left = noise(frames);
    let right: Vec<f32> = left.iter().rev().map(|s| s * 0.7).collect();
    let events = [
        set(1_000, p(0, SHAPER), 3.0),
        set(2_000, p(1, FILTER), 6.0),
        set(3_000, p(0, AMOUNT), 20.0),
    ];
    let base = with(
        leveled(&[0.8, 0.8, 0.8]),
        &[
            (p(0, SHAPER), NOISE_INJECTION),
            (p(0, AMOUNT), 60.0),
            (p(0, FILTER), MORPH_FILTER),
            (p(0, FREQUENCY), 2_000.0),
            (p(1, SHAPER), 4.0),
            (p(1, AMOUNT), 40.0),
            (p(1, FILTER), COMB),
            (p(1, FREQUENCY), 600.0),
            (p(1, RESONANCE), 50.0),
            (p(2, SHAPER), 8.0),
            (p(2, AMOUNT), 30.0),
            (p(2, FILTER), DISPERSION),
            (p(2, FREQUENCY), 3_000.0),
            (DRIVE, 6.0),
            (TONE_AMOUNT, 40.0),
            (COLOR_COMPENSATION, 1.0),
            (BLEND, 35.0),
            (DRY_WET, 85.0),
        ],
    );
    for (mode, name) in MODES.iter().enumerate() {
        let params = with(base.clone(), &[(ROUTING_MODE, mode as f64)]);
        let tempo = TempoMap::constant(Bpm::DEFAULT);
        let render = |block: usize| {
            let mut node = roar(&params, block);
            common::run_blocks(
                node.as_mut(),
                &[left.clone(), right.clone()],
                block,
                &events,
                &tempo,
            )
        };
        let reference = render(4_096);
        assert!(render(4_096) == reference, "{}: run after run", name);
        for block in [1, 64, 512] {
            assert!(render(block) == reference, "{} at block size {block}", name);
        }
        assert!(
            reference[0].iter().any(|s| *s != 0.0),
            "{} makes sound",
            name
        );
    }
}
