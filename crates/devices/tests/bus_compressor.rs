//! Glue Compressor, the native bus compressor (WP-5.21; the bus-compressor spec): the
//! static curve at several ratios, the attack and release time constants,
//! Auto Release, Range, Makeup, Soft Clip and its light, Dry/Wet,
//! Oversampling and its latency, and a sidechain key that ducks the input.

use crate::common;
use std::f64::consts::TAU;

use bl_devices::bus_compressor::{
    ATTACK, AUTO_RELEASE, DRY_WET, MAKEUP, OVERSAMPLING, RANGE, RATIO, RELEASE, SC_EQ, SC_FREQ,
    SC_RES, SC_TYPE, SOFT_CLIP, THRESHOLD, bus_compressor_type,
};
use bl_dsp::Oversampler;
use bl_engine::{Block, DeviceType, Node, NodeEvent, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::{RATE, device, node, noise, set};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::bus_compressor::register(&mut registry);
    registry
}

fn bond(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = bus_compressor_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// One block of `input` on both channels, `key` (also on both) as the
/// sidechain when given, under the allocation trap; the left channel.
fn run(node: &mut dyn Node, input: &[f32], key: Option<&[f32]>, events: &[NodeEvent]) -> Vec<f32> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..2).flat_map(|_| input.iter().copied()).collect();
    let keyed: Option<Vec<f32>> = key.map(|key| (0..2).flat_map(|_| key.iter().copied()).collect());
    let mut block = Block::new(
        &mut data,
        2,
        frames,
        frames,
        events,
        TransportInfo::default(),
        RATE,
    );
    if let Some(keyed) = &keyed {
        block = block.with_sidechain(keyed);
    }
    bl_rt::rt_test!({ node.process(&mut block) });
    data[..frames].to_vec()
}

fn stream(node: &mut dyn Node, input: &[f32], key: Option<&[f32]>, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    for start in (0..input.len()).step_by(block) {
        let end = (start + block).min(input.len());
        out.extend_from_slice(&run(
            node,
            &input[start..end],
            key.map(|k| &k[start..end]),
            &[],
        ));
    }
    out
}

fn db_of(amplitude: f64) -> f64 {
    20.0 * amplitude.log10()
}

/// The static curve, written out again: the reduction in dB at `level`.
fn expected_reduction(level: f64, threshold: f64, ratio: f64) -> f64 {
    let (slope, knee) = (1.0 - 1.0 / ratio, 12.0 / ratio);
    let over = level - threshold;
    if over <= -knee / 2.0 {
        0.0
    } else if over >= knee / 2.0 {
        slope * over
    } else {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    }
}

/// The settled gain in dB of a steady `level` dB signal through a Glue Compressor of
/// `params`, the attack short.
fn settled_gain(params: &[(ParamId, f64)], level: f64) -> f64 {
    let mut all = vec![(ATTACK, 0.01)];
    all.extend_from_slice(params);
    let input = vec![db_to_gain(level) as f32; 4096];
    let out = stream(bond(&all, 4096).as_mut(), &input, None, 4096);
    db_of(f64::from(out[4095]) / f64::from(input[4095]))
}

// Spec: bus-compressor R1
#[test]
fn the_bus_compressor_is_described_with_oversampling_as_a_menu_option() {
    let device_type = bus_compressor_type();
    device_type.description.check().unwrap();
    assert!(device_type.sidechain);
    assert_eq!(device_type.description.menu_options, [OVERSAMPLING]);
    let layout = &device_type.description.layout;
    let placed = layout.place(layout.docked_size());
    assert!(
        placed.controls.iter().all(|c| c.param != OVERSAMPLING),
        "no control for a menu option"
    );
    let range = device_type.description.param(RANGE).unwrap();
    assert_eq!((range.max(), range.default), (0.0, -60.0));
}

// Spec: bus-compressor R2
#[test]
fn the_static_curve_matches_at_each_ratio() {
    for ratio in [1.5, 2.0, 4.0, 10.0, 20.0] {
        for level in [-40.0, -25.0, -21.0, -20.0, -19.0, -18.0, -14.0, -6.0, 0.0] {
            let got = settled_gain(&[(THRESHOLD, -20.0), (RATIO, ratio)], level);
            let want = -expected_reduction(level, -20.0, ratio);
            assert!(
                (got - want).abs() < 0.1,
                "{ratio}:1 at {level} dB: {got} against {want}"
            );
        }
    }
}

// Spec: bus-compressor R2
#[test]
fn the_knee_narrows_as_the_ratio_rises() {
    use bl_devices::bus_compressor::knee_db;
    assert!(knee_db(2.0) > knee_db(4.0) && knee_db(4.0) > knee_db(20.0));
}

/// The reduction in dB, frame by frame, of `input` through a Glue Compressor.
fn reductions(params: &[(ParamId, f64)], input: &[f32]) -> Vec<f64> {
    let out = stream(bond(params, 4096).as_mut(), input, None, 4096);
    out.iter()
        .zip(input)
        .map(|(o, i)| -db_of(f64::from(*o) / f64::from(*i)))
        .collect()
}

/// Quiet for `quiet` frames, loud for `loud`, quiet again for `after`.
fn pulse(quiet: usize, loud: usize, after: usize) -> Vec<f32> {
    let level = |n: usize| {
        if (quiet..quiet + loud).contains(&n) {
            1.0
        } else {
            db_to_gain(-60.0) as f32
        }
    };
    (0..quiet + loud + after).map(level).collect()
}

/// The first frame at or after `from` where `test` holds.
fn first(values: &[f64], from: usize, test: impl Fn(f64) -> bool) -> usize {
    from + values[from..]
        .iter()
        .position(|v| test(*v))
        .expect("it happens")
}

// Spec: bus-compressor R3
#[test]
fn attack_and_release_are_time_constants_of_the_reduction() {
    // A full-scale pulse over a -20 dB threshold at 20:1, so the reduction
    // heads for 19 dB; Attack 10 ms, Release 0.1 s.
    let params = [
        (THRESHOLD, -20.0),
        (RATIO, 20.0),
        (ATTACK, 10.0),
        (RELEASE, 0.1),
    ];
    let (quiet, loud) = (2_000, 48_000);
    let r = reductions(&params, &pulse(quiet, loud, 48_000));
    let target = r[quiet + loud - 1];
    assert!((target - 19.0).abs() < 0.2, "{target}");
    let rises = first(&r, quiet, |v| v >= 0.632 * target) - quiet;
    assert!(
        (rises as f64 / 480.0 - 1.0).abs() < 0.05,
        "attack {rises} frames"
    );
    let falls = first(&r, quiet + loud, |v| v <= 0.368 * target) - (quiet + loud);
    assert!(
        (falls as f64 / 4_800.0 - 1.0).abs() < 0.05,
        "release {falls} frames"
    );
}

// Spec: bus-compressor R4
#[test]
fn auto_release_lets_go_in_a_fast_part_and_a_slow_one() {
    let params = [
        (THRESHOLD, -20.0),
        (RATIO, 20.0),
        (ATTACK, 1.0),
        (AUTO_RELEASE, 1.0),
        (RELEASE, 0.05),
    ];
    let (quiet, loud) = (2_000, 96_000);
    let r = reductions(&params, &pulse(quiet, loud, 96_000));
    let start = quiet + loud;
    let target = r[start - 1];
    for (seconds, share) in [(0.1, 0.636), (1.0, 0.184)] {
        let at = start + (seconds * 48_000.0) as usize;
        let got = r[at] / target;
        assert!(
            (got / share - 1.0).abs() < 0.05,
            "{seconds} s: {got} against {share}"
        );
    }
}

// Spec: bus-compressor R5
#[test]
fn range_caps_the_reduction_and_zero_applies_none() {
    let loud = [(THRESHOLD, -20.0), (RATIO, 20.0)];
    let with = |range: f64| {
        let mut params = loud.to_vec();
        params.push((RANGE, range));
        settled_gain(&params, 0.0)
    };
    assert!((with(-6.0) + 6.0).abs() < 0.1);
    assert!((with(-12.0) + 12.0).abs() < 0.1);
    assert!(with(0.0).abs() < 1e-4, "0 dB applies no compression");
}

// Spec: bus-compressor R6
#[test]
fn makeup_adds_gain_after_the_compression_down_to_silence() {
    assert!((settled_gain(&[(MAKEUP, db_to_gain(6.0))], -30.0) - 6.0).abs() < 0.01);
    let silent = stream(bond(&[(MAKEUP, 0.0)], 256).as_mut(), &noise(256), None, 256);
    assert!(silent.iter().all(|s| *s == 0.0));
}

/// A sine at `hz`, `amplitude` and `frames` long.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (amplitude * (TAU * hz * n as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

/// The light and gain reduction a block of `input` leaves published.
fn published(params: &[(ParamId, f64)], input: &[f32]) -> (f32, f32) {
    let mut node = bond(params, input.len());
    let mut reader = node.display().expect("Glue Compressor publishes a display");
    run(node.as_mut(), input, None, &[]);
    let values = reader.read().values.clone();
    (values[0], values[1])
}

// Spec: bus-compressor R7
#[test]
fn soft_clip_holds_the_peak_under_minus_half_a_db_and_lights_the_clip() {
    let loud = sine(1_000.0, 2.0, 4_800);
    // Threshold 0 dB, ratio 1: no compression, so the clip is all there is.
    let plain = [(RATIO, 1.0), (THRESHOLD, 0.0)];
    let mut soft = plain.to_vec();
    soft.push((SOFT_CLIP, 1.0));
    let out = run(bond(&soft, 4_800).as_mut(), &loud, None, &[]);
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(f64::from(peak) <= db_to_gain(-0.5) + 1e-6, "{peak}");
    let out = run(bond(&plain, 4_800).as_mut(), &loud, None, &[]);
    assert!(
        out.iter().fold(0.0f32, |m, s| m.max(s.abs())) > 1.9,
        "plain passes it"
    );
    // A quiet signal passes the shaper nearly unchanged.
    let quiet = sine(1_000.0, 0.01, 4_800);
    let out = run(bond(&soft, 4_800).as_mut(), &quiet, None, &[]);
    assert!(out.iter().zip(&quiet).all(|(o, i)| (o - i).abs() < 1e-5));

    // The light: red above 0 dB, yellow while Soft Clip rounds peaks, off
    // below.
    assert_eq!(published(&plain, &loud).1, 1.0);
    assert_eq!(published(&soft, &loud).1, 2.0);
    assert_eq!(published(&soft, &quiet).1, 0.0);
    assert!(
        published(&[(THRESHOLD, -20.0)], &loud).0 < -1.0,
        "the reduction is read"
    );
}

// Spec: bus-compressor R8
#[test]
fn dry_wet_at_zero_nulls_against_the_input() {
    let input: Vec<f32> = noise(4_096).iter().map(|s| s * 1.2).collect();
    let params = [
        (THRESHOLD, -30.0),
        (RATIO, 10.0),
        (SOFT_CLIP, 1.0),
        (DRY_WET, 0.0),
    ];
    let out = run(bond(&params, 4_096).as_mut(), &input, None, &[]);
    assert_eq!(out, input);
    let half = run(
        bond(&[(THRESHOLD, -30.0), (DRY_WET, 50.0)], 4_096).as_mut(),
        &input,
        None,
        &[],
    );
    assert_ne!(half, input);
}

// Spec: bus-compressor R9
#[test]
fn oversampling_reports_the_oversamplers_latency_and_keeps_the_dry_signal_aligned() {
    let trip = Oversampler::new(1, 1, 64).latency();
    assert_eq!(bond(&[(OVERSAMPLING, 1.0)], 64).latency(), trip);
    assert_eq!(bond(&[], 64).latency(), 0);
    // Below the threshold the oversampled path passes a tone, late by its
    // latency; Dry/Wet at 50 % is then the same tone, still aligned.
    let tone = sine(1_000.0, 0.5, 8_192);
    for wet in [100.0, 50.0] {
        let params = [(OVERSAMPLING, 1.0), (RATIO, 1.0), (DRY_WET, wet)];
        let out = stream(bond(&params, 4_096).as_mut(), &tone, None, 4_096);
        let late = trip as usize;
        let worst = (4_096..8_192 - late)
            .map(|n| (out[n + late] - tone[n]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 2e-3, "{wet} % wet: {worst}");
    }
}

// Spec: bus-compressor R10
#[test]
fn a_key_from_another_track_ducks_the_input_and_its_eq_hears_only_its_band() {
    let params = [
        (THRESHOLD, -30.0),
        (RATIO, 20.0),
        (ATTACK, 0.1),
        (RELEASE, 0.05),
    ];
    let input = vec![0.01f32; 9_600];
    let own = *stream(bond(&params, 4_800).as_mut(), &input, None, 4_800)
        .last()
        .unwrap();
    assert!(
        (own - 0.01).abs() < 1e-6,
        "its own input is under the threshold"
    );
    let key = vec![0.5f32; 9_600];
    let ducked = *stream(bond(&params, 4_800).as_mut(), &input, Some(&key), 4_800)
        .last()
        .unwrap();
    assert!(ducked < 0.002, "ducked to {ducked}");

    let mut band = params.to_vec();
    band.extend([
        (SC_EQ, 1.0),
        (SC_TYPE, 1.0),
        (SC_FREQ, 1_000.0),
        (SC_RES, 5.0),
    ]);
    let low = sine(100.0, 0.5, 9_600);
    let mid = sine(1_000.0, 0.5, 9_600);
    let heard_low = stream(bond(&band, 4_800).as_mut(), &input, Some(&low), 4_800);
    let heard_mid = stream(bond(&band, 4_800).as_mut(), &input, Some(&mid), 4_800);
    let floor = |out: &[f32]| out[4_800..].iter().copied().fold(1.0f32, f32::min);
    assert!(
        floor(&heard_low) > 0.0099,
        "a 100 Hz key is left out: {}",
        floor(&heard_low)
    );
    assert!(
        floor(&heard_mid) < 0.002,
        "a 1 kHz key ducks it: {}",
        floor(&heard_mid)
    );
}

// Spec: bus-compressor R11
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let input: Vec<f32> = noise(16_384).iter().map(|s| s * 1.1).collect();
    let mut renders = Vec::new();
    for block in [1usize, 64, 512, 4096] {
        let params = [
            (THRESHOLD, -24.0),
            (RATIO, 8.0),
            (SOFT_CLIP, 1.0),
            (OVERSAMPLING, 1.0),
            (AUTO_RELEASE, 1.0),
            (DRY_WET, 80.0),
        ];
        let mut node = bond(&params, block);
        let mut out = Vec::new();
        for (n, chunk) in input.chunks(block).enumerate() {
            let events = if n == 0 {
                vec![set(0, MAKEUP, db_to_gain(3.0)), set(0, ATTACK, 2.0)]
            } else {
                Vec::new()
            };
            out.extend_from_slice(&run(node.as_mut(), chunk, None, &events));
        }
        renders.push(out);
    }
    for render in &renders[1..] {
        assert_eq!(render, &renders[0]);
    }
}
