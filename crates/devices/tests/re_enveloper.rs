//! Re-Enveloper, the native three-band envelope processor (WP-8.50; the
//! Re-Enveloper spec): the crossovers, each band's C/E Factor and Gain,
//! Output, the display, and renders that don't depend on the block size.
//! Every block is a real-time section under the allocation trap.

use crate::common;
use core::f64::consts::TAU;

use bl_devices::re_enveloper::{
    HIGH, HIGH_CROSSOVER, LOW, LOW_CROSSOVER, MID, OUTPUT, RE_ENVELOPER, factor, gain,
    re_enveloper_type,
};
use bl_engine::{Block, DeviceType, DisplayReader, Node, Registry, TransportInfo};
use bl_model::param::db_to_gain;
use bl_model::{DeviceId, ParamId};

use common::{RATE, device, node};

const RATE_HZ: f64 = 48_000.0;

/// A registry with Re-Enveloper alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::re_enveloper::register(&mut registry);
    registry
}

/// A Re-Enveloper node of `params`, up to `max_frames` at a time, and the
/// reader of its display.
fn rig(params: &[(ParamId, f64)], max_frames: usize) -> (Box<dyn Node>, DisplayReader) {
    let device_type: DeviceType = re_enveloper_type();
    let mut node = node(&registry(), &device(&device_type, params), max_frames);
    let reader = node.display().expect("it has a display");
    (node, reader)
}

/// One block of `input` on both channels, as a real-time section.
fn run(node: &mut dyn Node, input: &[f32]) -> Vec<f32> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..2).flat_map(|_| input.iter().copied()).collect();
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
    data.truncate(frames);
    data
}

/// `input` through `node` in blocks of `block` frames.
fn stream(node: &mut dyn Node, input: &[f32], block: usize) -> Vec<f32> {
    input
        .chunks(block)
        .flat_map(|chunk| run(node, chunk))
        .collect()
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (amplitude * (TAU * hz * n as f64 / RATE_HZ).sin()) as f32)
        .collect()
}

/// The magnitude of `signal` at `hz`, over the whole of it (a Goertzel
/// filter, on whole cycles).
fn tone_level(signal: &[f32], hz: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, s) in signal.iter().enumerate() {
        let phase = TAU * hz * n as f64 / RATE_HZ;
        re += f64::from(*s) * phase.cos();
        im += f64::from(*s) * phase.sin();
    }
    2.0 * re.hypot(im) / signal.len() as f64
}

/// The level of `signal`'s `from` to `to` milliseconds, as RMS in dB.
fn window_db(signal: &[f32], from: f64, to: f64) -> f64 {
    let (a, b) = ((from * 48.0) as usize, (to * 48.0) as usize);
    let mean = signal[a..b]
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / (b - a) as f64;
    10.0 * mean.max(1e-18).log10()
}

/// Every band's C/E Factor set to `per_cent`.
fn factors(per_cent: f64) -> Vec<(ParamId, f64)> {
    [LOW, MID, HIGH]
        .iter()
        .map(|band| (factor(*band), per_cent))
        .collect()
}

/// Noise with a 1 ms attack and a 300 ms exponential decay, a second long.
fn percussive() -> Vec<f32> {
    common::noise(48_000)
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let ms = n as f64 / 48.0;
            let envelope = (ms / 1.0).min(1.0) * (-ms / 300.0).exp();
            s * envelope as f32
        })
        .collect()
}

/// The attack's level (the loudest 5 ms of the first 200) over the tail's
/// at 100 ms (5 ms about it), in dB.
fn attack_over_tail(out: &[f32]) -> f64 {
    let attack = (0..195)
        .map(|ms| window_db(out, f64::from(ms), f64::from(ms + 5)))
        .fold(f64::MIN, f64::max);
    attack - window_db(out, 97.5, 102.5)
}

// Spec: re-enveloper R1, R2, R5 and R8
#[test]
fn re_enveloper_is_described_with_two_banks_and_no_latency() {
    let device_type = re_enveloper_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Re-Enveloper");
    assert!(matches!(
        device_type.kind,
        bl_model::DeviceKind::Native { ref device_type } if device_type == RE_ENVELOPER
    ));
    let description = &device_type.description;
    assert_eq!(description.params.len(), 9);
    for band in [LOW, MID, HIGH] {
        assert!(description.param(gain(band)).unwrap().is_gain());
        let factor = description.param(factor(band)).unwrap();
        assert_eq!(
            (factor.min(), factor.max(), factor.default),
            (-100.0, 100.0, 0.0)
        );
    }
    assert!(description.param(OUTPUT).unwrap().is_gain());
    let (low, high) = (
        description.param(LOW_CROSSOVER).unwrap(),
        description.param(HIGH_CROSSOVER).unwrap(),
    );
    assert_eq!((low.min(), low.max(), low.default), (50.0, 2_000.0, 200.0));
    assert_eq!(
        (high.min(), high.max(), high.default),
        (500.0, 16_000.0, 2_500.0)
    );
    let banks = description.banks();
    assert_eq!(
        banks.iter().map(|b| b.params.len()).collect::<Vec<_>>(),
        [8, 1]
    );
    let (node, _) = rig(&[], 64);
    assert_eq!(node.latency(), 0);
}

// Spec: re-enveloper R2
#[test]
fn with_every_factor_at_zero_the_crossovers_sum_flat_within_a_tenth_of_a_decibel() {
    for (low, high) in [(200.0, 2_500.0), (50.0, 500.0), (2_000.0, 16_000.0)] {
        for hz in [
            50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 2_500.0, 5_000.0, 10_000.0,
        ] {
            let (mut node, _) = rig(&[(LOW_CROSSOVER, low), (HIGH_CROSSOVER, high)], 512);
            let input = sine(hz, 0.5, 24_000);
            let out = stream(node.as_mut(), &input, 512);
            // Every test frequency is a multiple of 50 Hz, so the last
            // 19,200 samples hold whole cycles of it.
            let db = 20.0 * (tone_level(&out[4_800..], hz) / 0.5).log10();
            assert!(
                db.abs() < 0.1,
                "{hz} Hz with crossovers {low}, {high}: {db} dB"
            );
        }
    }
}

// Spec: re-enveloper R3 and R4
#[test]
fn a_positive_factor_flattens_a_percussive_envelope_and_a_negative_one_sharpens_it() {
    let input = percussive();
    let over = |per_cent: f64| {
        let (mut node, _) = rig(&factors(per_cent), 512);
        attack_over_tail(&stream(node.as_mut(), &input, 512))
    };
    let (dry, compressed, expanded) = (over(0.0), over(100.0), over(-100.0));
    assert!(
        compressed < 3.0,
        "F = +100 %: the attack is {compressed} dB over the tail (dry {dry} dB)"
    );
    assert!(
        expanded - dry >= 12.0,
        "F = -100 %: {expanded} dB over the tail, dry {dry} dB"
    );
}

// Spec: re-enveloper R5 and R7. A band's Gain at -inf removes the band,
// and Output at -inf the sum.
#[test]
fn band_gains_and_output_reach_minus_infinity() {
    let low_tone = sine(50.0, 0.5, 24_000);
    let (mut node, _) = rig(&[(gain(LOW), 0.0)], 512);
    let out = stream(node.as_mut(), &low_tone, 512);
    let db = 20.0 * (tone_level(&out[4_800..], 50.0) / 0.5).log10();
    assert!(db < -30.0, "the low band is gone: {db} dB");
    let (mut node, _) = rig(&[(gain(MID), 0.0)], 512);
    let out = stream(node.as_mut(), &sine(1_000.0, 0.5, 24_000), 512);
    let db = 20.0 * (tone_level(&out[4_800..], 1_000.0) / 0.5).log10();
    assert!(db < -30.0, "the mid band is gone: {db} dB");
    let (mut node, _) = rig(&[(OUTPUT, 0.0)], 512);
    let out = stream(node.as_mut(), &common::noise(4_096), 512);
    assert!(out.iter().all(|s| *s == 0.0));
    let (mut node, _) = rig(
        &[(gain(LOW), 0.0), (gain(MID), 0.0), (gain(HIGH), 0.0)],
        512,
    );
    let out = stream(node.as_mut(), &common::noise(4_096), 512);
    assert!(out.iter().all(|s| *s == 0.0));
}

// Spec: re-enveloper R5. Output sets the level of the sum.
#[test]
fn output_sets_the_level_of_the_sum() {
    let (mut node, _) = rig(&[(OUTPUT, db_to_gain(6.0))], 512);
    let out = stream(node.as_mut(), &sine(1_000.0, 0.25, 24_000), 512);
    let db = 20.0 * (tone_level(&out[4_800..], 1_000.0) / 0.25).log10();
    assert!((db - 6.0).abs() < 0.1, "{db} dB");
}

// Spec: re-enveloper R1
#[test]
fn the_display_reads_each_bands_envelope_and_gain() {
    let (mut node, mut reader) = rig(&factors(100.0), 256);
    stream(node.as_mut(), &sine(1_000.0, 0.5, 12_000), 256);
    let published = reader.read().values.clone();
    assert_eq!(published.len(), 6);
    assert!(
        published[2] > -12.0,
        "the mid band hears the tone: {published:?}"
    );

    let id = DeviceId(bl_core::id::Id::new(1).unwrap());
    let description = &re_enveloper_type().description;
    let values = Default::default();
    let shown = bl_devices::display(RE_ENVELOPER, id, description, &values, None)
        .expect("it draws a display");
    assert!(!shown.live && shown.readings.is_empty(), "no engine");
    let shown =
        bl_devices::display(RE_ENVELOPER, id, description, &values, Some(&published)).unwrap();
    assert!(shown.live);
    let reading = |name: &str| {
        shown
            .readings
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.value)
    };
    assert_eq!(reading("Mid Envelope"), Some(published[2]));
    assert_eq!(reading("High Gain"), Some(published[5]));
}

// Spec: re-enveloper R6
#[test]
fn renders_are_the_same_at_any_block_size() {
    let input = percussive();
    let mut params = vec![
        (LOW_CROSSOVER, 120.0),
        (HIGH_CROSSOVER, 4_000.0),
        (gain(MID), db_to_gain(-3.0)),
        (OUTPUT, db_to_gain(2.0)),
    ];
    params.extend([
        (factor(LOW), 60.0),
        (factor(MID), -40.0),
        (factor(HIGH), 100.0),
    ]);
    let render = |block: usize| {
        let (mut node, _) = rig(&params, block);
        stream(node.as_mut(), &input[..12_288], block)
    };
    let reference = render(4_096);
    assert!(reference.iter().any(|s| *s != 0.0));
    for block in [1, 64, 512] {
        assert_eq!(render(block), reference, "block size {block}");
    }
    assert_eq!(render(512), render(512), "run after run");
}
