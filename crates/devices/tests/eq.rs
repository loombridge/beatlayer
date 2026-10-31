//! EQ Eight, the native parametric EQ (WP-2.18a, part 1; the eq spec): each
//! filter type against its analytic curve, the Stereo, L/R, and M/S modes,
//! Adaptive Q, the oversampled path, and the analyser.

use crate::common;
use std::f64::consts::TAU;

use bl_core::time::SampleRate;
use bl_devices::NativeNode;
use bl_devices::eq::{
    ADAPTIVE_Q, ANALYSER_BANDS, ANALYZE, Contour, FREQ, GAIN, GAIN_SLOT, MODE, ON, OVERSAMPLE, Q,
    TYPE, band_param, eq_type,
};
use bl_engine::{Block, DeviceType, Node, NodeConfig, Registry, TransportInfo};
use bl_model::ParamId;

use common::{RATE, config, device, node};

/// The filter types the tests name, by their index in the chooser.
const LOW_SHELF: usize = 2;
const PEAK: usize = 3;
const NOTCH: usize = 4;

/// The block size the tests' nodes take: above the longest response they
/// ask for (the impulse responses run 8,192 frames).
const MAX_FRAMES: usize = 8192;

/// A registry with the EQ alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::eq::register(&mut registry);
    registry
}

/// An EQ Eight of `params`, up to `max_frames` at a time.
fn contour(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = eq_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// An EQ Eight whose analyser the test can read.
fn analysed(params: &[(ParamId, f64)], max_frames: usize) -> NativeNode<Contour> {
    let device_type: DeviceType = eq_type();
    let values = device(&device_type, params);
    let config = config(max_frames);
    NativeNode::new(
        Contour::new(&config),
        &device_type.description,
        &values,
        &config,
    )
}

/// `globals` and `bands` as one parameter list.
fn with_globals(globals: &[(ParamId, f64)], bands: Vec<(ParamId, f64)>) -> Vec<(ParamId, f64)> {
    let mut out = globals.to_vec();
    out.extend(bands);
    out
}

/// Runs `left` and `right` through `node` as one block under the trap.
fn run_stereo(node: &mut dyn Node, left: &[f32], right: &[f32]) -> [Vec<f32>; 2] {
    let frames = left.len();
    let mut data: Vec<f32> = left.iter().chain(right.iter()).copied().collect();
    {
        let mut block = Block::new(
            &mut data,
            2,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            RATE,
        );
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    let (l, r) = data.split_at(frames);
    [l.to_vec(), r.to_vec()]
}

/// Works the node's filters past their first block.
fn settle(node: &mut dyn Node) {
    run_stereo(node, &[0.0; 512], &[0.0; 512]);
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine(hz: f64, amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| amplitude * (TAU * hz * i as f64 / RATE.as_f64()).sin() as f32)
        .collect()
}

/// The node's magnitude response at `hz`, in dB, from its answer to an
/// impulse.
fn response_db(node: &mut dyn Node, hz: f64) -> f64 {
    settle(node);
    let frames = 8192;
    let mut input = vec![0.0f32; frames];
    input[0] = 1.0;
    let out = run_stereo(node, &input, &input)[0].clone();
    let w = TAU * hz / RATE.as_f64();
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, sample) in out.iter().enumerate() {
        re += f64::from(*sample) * (w * n as f64).cos();
        im -= f64::from(*sample) * (w * n as f64).sin();
    }
    20.0 * re.hypot(im).log10()
}

/// The type `kind`'s coefficients, the same cookbook the device follows,
/// written out again so the test leans on the curve and not on the code.
fn analytic_coeffs(kind: usize, freq: f64, gain_db: f64, q: f64, rate: f64) -> [f64; 5] {
    let w0 = TAU * freq / rate;
    let (sin, cos) = w0.sin_cos();
    let a = 10f64.powf(gain_db / 40.0);
    let alpha = sin / (2.0 * q);
    let root = a.sqrt();
    let (b0, b1, b2, a0, a1, a2) = match kind {
        // Low Cut
        0 | 1 => {
            let half = (1.0 + cos) / 2.0;
            (
                half,
                -(1.0 + cos),
                half,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            )
        }
        // Low Shelf
        2 => (
            a * ((a + 1.0) - (a - 1.0) * cos + 2.0 * root * alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - 2.0 * root * alpha),
            (a + 1.0) + (a - 1.0) * cos + 2.0 * root * alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - 2.0 * root * alpha,
        ),
        // Peak
        3 => (
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        ),
        // Notch
        4 => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        // High Shelf
        5 => (
            a * ((a + 1.0) + (a - 1.0) * cos + 2.0 * root * alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - 2.0 * root * alpha),
            (a + 1.0) - (a - 1.0) * cos + 2.0 * root * alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - 2.0 * root * alpha,
        ),
        // High Cut
        _ => {
            let half = (1.0 - cos) / 2.0;
            (half, 1.0 - cos, half, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
        }
    };
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// Whether `kind` reads its Gain parameter.
fn uses_gain(kind: usize) -> bool {
    matches!(kind, 2 | 3 | 5)
}

/// How many stages `kind` runs.
fn stages(kind: usize) -> usize {
    if kind == 0 || kind == 7 { 4 } else { 1 }
}

/// The analytic magnitude response of `kind` at `hz`, its stages in cascade.
fn analytic_db(kind: usize, freq: f64, gain_db: f64, q: f64, hz: f64, rate: f64) -> f64 {
    let [b0, b1, b2, a1, a2] = analytic_coeffs(kind, freq, gain_db, q, rate);
    let w = TAU * hz / rate;
    let (c1, s1) = (w.cos(), -w.sin());
    let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
    let (nr, ni) = (b0 + b1 * c1 + b2 * c2, b1 * s1 + b2 * s2);
    let (dr, di) = (1.0 + a1 * c1 + a2 * c2, a1 * s1 + a2 * s2);
    let magnitude = (nr.hypot(ni) / dr.hypot(di)).powi(stages(kind) as i32);
    20.0 * magnitude.log10()
}

/// The frequencies every type is checked at.
const CHECK_HZ: [f64; 7] = [30.0, 100.0, 300.0, 1_000.0, 3_000.0, 10_000.0, 18_000.0];

/// One band's parameters, band 1 of the first curve.
fn band0(kind: usize, freq: f64, gain_db: f64, q: f64) -> Vec<(ParamId, f64)> {
    vec![
        (band_param(0, 0, ON), 1.0),
        (band_param(0, 0, TYPE), kind as f64),
        (band_param(0, 0, FREQ), freq),
        (band_param(0, 0, GAIN_SLOT), gain_db),
        (band_param(0, 0, Q), q),
    ]
}

/// The same, band 2 of the second curve.
fn band1(kind: usize, freq: f64, gain_db: f64, q: f64) -> Vec<(ParamId, f64)> {
    vec![
        (band_param(1, 1, ON), 1.0),
        (band_param(1, 1, TYPE), kind as f64),
        (band_param(1, 1, FREQ), freq),
        (band_param(1, 1, GAIN_SLOT), gain_db),
        (band_param(1, 1, Q), q),
    ]
}

/// How far `out` is from `reference`, sample by sample.
fn worst_error(out: &[f32], reference: &[f32]) -> f32 {
    out.iter()
        .zip(reference)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max)
}

// Spec: eq R1
#[test]
fn the_eq_is_described_and_laid_out() {
    let device_type: DeviceType = eq_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "EQ Eight");
    assert_eq!(device_type.description.params.len(), 85);
    assert!(matches!(
        device_type.kind,
        bl_model::DeviceKind::Native { ref device_type } if device_type == bl_devices::EQ
    ));
    let gain = device_type
        .description
        .params
        .iter()
        .find(|p| p.id == GAIN)
        .expect("the output gain");
    assert!(gain.is_gain(), "the output gain reaches -inf dB");
}

// Spec: eq R3
#[test]
fn every_filter_type_matches_its_analytic_curve() {
    for kind in 0..8 {
        for gain_db in [-12.0, 0.0, 12.0] {
            if !uses_gain(kind) && gain_db != 0.0 {
                continue;
            }
            let mut node = contour(&band0(kind, 1_000.0, gain_db, 0.71), MAX_FRAMES);
            for hz in CHECK_HZ {
                let expected = analytic_db(kind, 1_000.0, gain_db, 0.71, hz, RATE.as_f64());
                if expected < -80.0 {
                    // Below what f32 arithmetic holds: a stopped band reads
                    // its own rounding floor, not the curve.
                    continue;
                }
                let measured = response_db(node.as_mut(), hz);
                assert!(
                    (measured - expected).abs() < 0.1,
                    "type {kind} at {gain_db} dB, {hz} Hz: measured {measured} dB, \
                     analytic {expected} dB"
                );
            }
        }
    }
}

// Spec: eq R3
#[test]
fn the_cut_filters_slope_as_their_names_say() {
    let mut gentle = contour(&band0(6, 1_000.0, 0.0, 0.71), MAX_FRAMES);
    let mut steep = contour(&band0(7, 1_000.0, 0.0, 0.71), MAX_FRAMES);
    let (a, b) = (
        response_db(gentle.as_mut(), 2_000.0),
        response_db(steep.as_mut(), 2_000.0),
    );
    assert!(
        (-14.0..-11.0).contains(&a),
        "12 dB/octave reads {a} dB an octave up"
    );
    assert!(
        (-54.0..-45.0).contains(&b),
        "48 dB/octave reads {b} dB there"
    );
}

// Spec: eq R4
#[test]
fn stereo_mode_gives_both_channels_the_first_curve() {
    // The second curve would move the right channel if Stereo read it.
    let params = with_globals(
        &[],
        [
            band0(PEAK, 1_000.0, 12.0, 0.71),
            band1(PEAK, 1_000.0, -12.0, 0.71),
        ]
        .concat(),
    );
    let mut node = contour(&params, MAX_FRAMES);
    settle(node.as_mut());
    let input = sine(1_000.0, 0.5, MAX_FRAMES);
    let out = run_stereo(node.as_mut(), &input, &input);
    assert_eq!(out[0], out[1], "both channels take the first curve");
    let boosted = out[0][2000].abs() / input[2000].abs();
    assert!(
        boosted > 3.0,
        "the first curve boosts both channels: {boosted}"
    );
}

// Spec: eq R4
#[test]
fn left_right_mode_gives_each_channel_its_own_curve() {
    let mid = [(MODE, 1.0)];
    let mut node = contour(
        &with_globals(&mid, band0(PEAK, 1_000.0, 12.0, 0.71)),
        MAX_FRAMES,
    );
    settle(node.as_mut());
    let input = sine(1_000.0, 0.5, MAX_FRAMES);
    let out = run_stereo(node.as_mut(), &input, &input);
    let left = out[0][2000].abs() / input[2000].abs();
    assert!(left > 3.0, "the first curve boosts the left: {left}");
    assert!(
        worst_error(&out[1], &input) < 1e-6,
        "the second curve is off, so the right passes"
    );

    // The second curve's own band, a cut at 1 kHz, reaches the right alone.
    let params = with_globals(
        &mid,
        [
            band0(PEAK, 1_000.0, 12.0, 0.71),
            band1(PEAK, 1_000.0, -12.0, 0.71),
        ]
        .concat(),
    );
    let mut node = contour(&params, MAX_FRAMES);
    settle(node.as_mut());
    let out = run_stereo(node.as_mut(), &input, &input);
    let right = out[1][2000].abs() / input[2000].abs();
    assert!(right < 0.4, "the second curve cuts the right: {right}");
    let left = out[0][2000].abs() / input[2000].abs();
    assert!(left > 3.0, "the first curve still boosts the left: {left}");
}

// Spec: eq R5
#[test]
fn adaptive_q_tightens_the_band_as_the_gain_rises() {
    let at = |params: &[(ParamId, f64)], hz: f64| {
        let mut node = contour(params, MAX_FRAMES);
        response_db(node.as_mut(), hz)
    };
    let fixed = at(&band0(PEAK, 1_000.0, 12.0, 1.0), 2_000.0);
    let lifted = at(
        &with_globals(&[(ADAPTIVE_Q, 1.0)], band0(PEAK, 1_000.0, 12.0, 1.0)),
        2_000.0,
    );
    assert!(
        lifted < fixed - 0.5,
        "adaptive reads {lifted} dB against fixed {fixed} dB"
    );

    // At no gain there is nothing to rise with, and a cut rises like a
    // boost does.
    let none = at(&band0(PEAK, 1_000.0, 0.0, 1.0), 2_000.0);
    let lifted_none = at(
        &with_globals(&[(ADAPTIVE_Q, 1.0)], band0(PEAK, 1_000.0, 0.0, 1.0)),
        2_000.0,
    );
    assert!(
        (lifted_none - none).abs() < 1e-6,
        "adaptive reads {lifted_none} dB against {none} dB"
    );

    // A band without a gain is left alone.
    let notch = at(&band0(NOTCH, 1_000.0, 12.0, 1.0), 1_000.0);
    let lifted_notch = at(
        &with_globals(&[(ADAPTIVE_Q, 1.0)], band0(NOTCH, 1_000.0, 12.0, 1.0)),
        1_000.0,
    );
    assert!(
        (lifted_notch - notch).abs() < 1e-6,
        "adaptive reads {lifted_notch} dB against {notch} dB"
    );
}

// Spec: eq R6
#[test]
fn a_shelfs_boost_and_cut_meet_their_plateaus() {
    let mut node = contour(&band0(LOW_SHELF, 500.0, 12.0, 0.71), MAX_FRAMES);
    assert!((response_db(node.as_mut(), 50.0) - 12.0).abs() < 0.1);
    assert!(response_db(node.as_mut(), 10_000.0).abs() < 0.1);

    let mut node = contour(&band0(LOW_SHELF, 500.0, -12.0, 0.71), MAX_FRAMES);
    assert!((response_db(node.as_mut(), 50.0) + 12.0).abs() < 0.1);
    assert!(response_db(node.as_mut(), 10_000.0).abs() < 0.1);
}

// Spec: eq R7
#[test]
fn mid_side_mode_leaves_the_other_curve_untouched() {
    // Only the first curve works, on the mid; a pure side signal passes.
    let params = with_globals(&[(MODE, 2.0)], band0(PEAK, 1_000.0, 12.0, 0.71));
    let mut node = contour(&params, MAX_FRAMES);
    settle(node.as_mut());
    let left = sine(1_000.0, 0.5, MAX_FRAMES);
    let right: Vec<f32> = left.iter().map(|s| -s).collect();
    let out = run_stereo(node.as_mut(), &left, &right);
    assert!(
        worst_error(&out[0], &left) < 1e-5,
        "the side passes the mid curve's chain untouched"
    );

    // Only the second curve, on the side; a pure mid signal passes.
    let params = with_globals(&[(MODE, 2.0)], band1(PEAK, 1_000.0, 12.0, 0.71));
    let mut node = contour(&params, MAX_FRAMES);
    settle(node.as_mut());
    let mid = sine(1_000.0, 0.5, MAX_FRAMES);
    let out = run_stereo(node.as_mut(), &mid, &mid);
    assert!(
        worst_error(&out[0], &mid) < 1e-5,
        "the mid passes the side curve's chain untouched"
    );
}

// Spec: eq R8
#[test]
fn a_saved_oversample_reports_its_latency_before_the_first_block() {
    let on = contour(&[(OVERSAMPLE, 1.0)], MAX_FRAMES);
    assert_eq!(
        on.latency(),
        bl_dsp::Oversampler::new(1, 1, MAX_FRAMES).latency(),
    );
    assert_eq!(contour(&[], MAX_FRAMES).latency(), 0);
}

// Spec: eq R8
#[test]
fn the_oversampled_path_passes_its_bands_and_reports_its_latency() {
    let params = with_globals(&[(OVERSAMPLE, 1.0)], band0(PEAK, 1_000.0, 12.0, 0.71));
    let mut node = contour(&params, MAX_FRAMES);
    settle(node.as_mut());
    let delay = node.latency();
    assert!(delay > 0, "the oversampled path reports its delay");
    let measured = response_db(node.as_mut(), 1_000.0);
    assert!(
        (measured - 12.0).abs() < 0.2,
        "the peak still boosts through the oversampled path: {measured} dB"
    );
    // The delay it reports is the oversampler's own (the framework's R8).
    assert_eq!(
        delay,
        bl_dsp::Oversampler::new(1, 1, MAX_FRAMES).latency(),
        "the delay is the oversampler's"
    );
    // A high band passes too: that is what the switch is for.
    let high = with_globals(&[(OVERSAMPLE, 1.0)], band0(PEAK, 10_000.0, 12.0, 0.71));
    let mut node = contour(&high, MAX_FRAMES);
    let measured = response_db(node.as_mut(), 10_000.0);
    assert!(
        (measured - 12.0).abs() < 0.3,
        "a high band boosts through the oversampled path: {measured} dB"
    );

    let mut plain = contour(&band0(PEAK, 1_000.0, 12.0, 0.71), MAX_FRAMES);
    assert_eq!(plain.latency(), 0, "the plain path delays nothing");
    settle(plain.as_mut());
}

/// The gain in dB of a unit sine at `hz` through an EQ Eight of `params` at
/// `rate`, from the last second of two.
fn sine_gain_db(rate: SampleRate, params: &[(ParamId, f64)], hz: f64) -> f64 {
    let device_type: DeviceType = eq_type();
    let values = device(&device_type, params);
    let config = NodeConfig {
        round_trip_latency: 0,
        sample_rate: rate,
        max_frames: 1024,
        channels: 2,
    };
    let mut node = NativeNode::new(
        Contour::new(&config),
        &device_type.description,
        &values,
        &config,
    );
    let total = 2 * rate.as_f64() as usize;
    let (mut input, mut output) = (0.0f64, 0.0f64);
    for start in (0..total).step_by(1024) {
        let mut data: Vec<f32> = (0..2 * 1024)
            .map(|i| (TAU * hz * (start + i % 1024) as f64 / rate.as_f64()).sin() as f32)
            .collect();
        let sent = data.clone();
        {
            let mut block = Block::new(
                &mut data,
                2,
                1024,
                1024,
                &[],
                TransportInfo::default(),
                rate,
            );
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        if start >= total / 2 {
            input += sent[..1024]
                .iter()
                .map(|x| f64::from(*x).powi(2))
                .sum::<f64>();
            output += data[..1024]
                .iter()
                .map(|x| f64::from(*x).powi(2))
                .sum::<f64>();
        }
    }
    10.0 * (output / input).log10()
}

// Spec: eq R3
// Spec: eq R8
#[test]
fn low_bands_hold_their_curve_at_high_rates() {
    // A 192 kHz set, and a 96 kHz set whose Oversample runs the stages at 192 kHz.
    let cases = [
        (SampleRate::HZ_192000, 0.0, 192_000.0),
        (SampleRate::HZ_96000, 1.0, 192_000.0),
    ];
    let bands = [
        (PEAK, 25.0, 12.0, 4.0, [10.0, 25.0, 50.0]),
        (LOW_SHELF, 20.0, 15.0, 0.71, [5.0, 20.0, 50.0]),
    ];
    for (rate, oversample, stage_rate) in cases {
        for (kind, freq, gain, q, checks) in bands {
            let params = with_globals(
                &[(OVERSAMPLE, oversample), (ADAPTIVE_Q, 0.0)],
                band0(kind, freq, gain, q),
            );
            for hz in checks {
                let measured = sine_gain_db(rate, &params, hz);
                let expected = analytic_db(kind, freq, gain, q, hz, stage_rate);
                assert!(
                    (measured - expected).abs() < 0.1,
                    "type {kind} at {} Hz, {hz} Hz: {measured} dB, not {expected} dB",
                    rate.as_f64()
                );
            }
        }
    }
}

// Spec: eq R9
#[test]
fn the_analyser_reports_the_outputs_spectrum() {
    let mut node = analysed(&[], MAX_FRAMES);
    settle(&mut node);
    let input = sine(1_000.0, 0.5, MAX_FRAMES);
    let out = run_stereo(&mut node, &input, &input);
    assert_eq!(out[0].len(), input.len());
    let bands = *node.device().analyser();
    let (loudest, level) = bands
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .expect("bands");
    let ratio = (20_000.0f64 / 20.0).powf((loudest as f64 + 0.5) / ANALYSER_BANDS as f64);
    let centre = 20.0 * ratio;
    assert!(
        (600.0..1_600.0).contains(&centre),
        "the peak sits at {centre} Hz, not near 1 kHz"
    );
    assert!(
        (level + 6.0).abs() < 2.0,
        "a half-scale sine reads {level} dB"
    );

    // Switched off, the analyser measures nothing and the input passes.
    let mut node = analysed(&[(ANALYZE, 0.0)], MAX_FRAMES);
    settle(&mut node);
    let out = run_stereo(&mut node, &input, &input);
    assert_eq!(out[0], input);
    assert!(
        node.device()
            .analyser()
            .iter()
            .all(|level| *level == -100.0),
        "with Analyze off the analyser holds its floor"
    );
}

// Spec: eq R10
#[test]
fn the_output_gain_scales_the_finished_signal() {
    let params = with_globals(&[(GAIN, 0.5)], band0(PEAK, 1_000.0, 12.0, 0.71));
    let mut node = contour(&params, MAX_FRAMES);
    settle(node.as_mut());
    let input = sine(1_000.0, 0.5, MAX_FRAMES);
    let out = run_stereo(node.as_mut(), &input, &input);
    let reference = f64::from(input[2000]) * 3.981; // the peak's own +12 dB
    let measured = f64::from(out[0][2000]);
    let gain = measured / reference;
    assert!(
        (gain - 0.5).abs() < 0.01,
        "the gain halves the output: {gain}"
    );
}
