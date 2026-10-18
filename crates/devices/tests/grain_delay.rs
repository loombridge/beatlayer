//! Grain Delay, the grain delay (WP-5.25; the grain delay spec): the delay time
//! (synced and free), Frequency, Pitch, Random Pitch, Spray, Feedback and
//! Dry/Wet. Every block runs as a real-time section under the allocation
//! trap.

bl_rt::install_trap!();

mod common;

use bl_core::id::Id;
use bl_core::time::{Bpm, TempoMap};
use bl_devices::grain_delay::{
    DIVISION, DRY_WET, FEEDBACK, FREQUENCY, PITCH, RANDOM_PITCH, SPRAY, SYNC, TIME,
    grain_delay_type,
};
use bl_engine::{Registry, TransportInfo};
use bl_model::{DeviceId, ParamId};
use common::{node, noise, run_at, run_blocks, set};

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

fn tempo() -> TempoMap {
    TempoMap::constant(Bpm::new(120.0).unwrap())
}

/// Grain Delay as device number `id`, fully wet, with `params`.
fn scatter(id: u64, params: &[(ParamId, f64)]) -> Box<dyn bl_engine::Node> {
    let mut device = grain_delay_type().device(DeviceId(Id::new(id).unwrap()));
    device.params.insert(DRY_WET, 100.0);
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    node(&registry(), &device, 65_536)
}

fn run(node: &mut dyn bl_engine::Node, input: &[f32]) -> Vec<f32> {
    let both = [input.to_vec(), input.to_vec()];
    run_at(node, &both, &[], &tempo(), TransportInfo::default()).swap_remove(0)
}

fn impulse(frames: usize) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    input[0] = 1.0;
    input
}

// Spec: grain-delay R1 and R2
#[test]
fn an_impulses_grain_lands_on_the_set_delay_free_and_synced() {
    // Free: 100 ms is 4,800 frames.
    let mut free = scatter(1, &[(SYNC, 0.0), (TIME, 100.0), (FREQUENCY, 20.0)]);
    let out = run(free.as_mut(), &impulse(10_000));
    assert!((out[4_800] - 1.0).abs() < 1e-3, "free: {}", out[4_800]);
    assert!(out[4_799].abs() < 1e-3 && out[4_801].abs() < 1e-3);
    assert!(
        out[..4_790].iter().all(|s| s.abs() < 1e-3),
        "nothing before it"
    );

    // Synced: four sixteenths is a beat, 24,000 frames at 120 BPM.
    let mut synced = scatter(1, &[(SYNC, 1.0), (DIVISION, 4.0), (FREQUENCY, 20.0)]);
    let out = run(synced.as_mut(), &impulse(30_000));
    assert!((out[24_000] - 1.0).abs() < 1e-3, "synced: {}", out[24_000]);
    assert!(
        out[23_990..24_010]
            .iter()
            .enumerate()
            .all(|(n, s)| n == 10 || s.abs() < 1e-3)
    );
}

/// The frequency of a sine, from the spacing of its rising zero crossings
/// over `from..`.
fn frequency(signal: &[f32], from: usize) -> f64 {
    let mut crossings = Vec::new();
    for n in from..signal.len() - 1 {
        if signal[n] <= 0.0 && signal[n + 1] > 0.0 {
            crossings.push(n as f64 + f64::from(-signal[n]) / f64::from(signal[n + 1] - signal[n]));
        }
    }
    (crossings.len() - 1) as f64 * 48_000.0 / (crossings[crossings.len() - 1] - crossings[0])
}

// Spec: grain-delay R3
#[test]
fn pitch_moves_a_sine_an_octave_up_and_down_within_a_cent() {
    let sine: Vec<f32> = (0..40_000)
        .map(|n| (0.5 * (std::f64::consts::TAU * 1_000.0 * n as f64 / 48_000.0).sin()) as f32)
        .collect();
    for (semitones, ratio) in [(12.0, 2.0), (-12.0, 0.5)] {
        let mut node = scatter(1, &[(PITCH, semitones), (FREQUENCY, 20.0), (TIME, 100.0)]);
        let out = run(node.as_mut(), &sine);
        let measured = frequency(&out, 10_000);
        let cents = 1_200.0 * (measured / (1_000.0 * ratio)).log2();
        assert!(
            cents.abs() < 1.0,
            "{semitones} semitones: {measured} Hz, {cents} cents"
        );
    }
}

// Spec: grain-delay R4
#[test]
fn spray_smears_the_grain_later_within_its_range() {
    let mut node = scatter(1, &[(SPRAY, 200.0), (FREQUENCY, 20.0), (TIME, 100.0)]);
    let out = run(node.as_mut(), &impulse(30_000));
    // 100 ms to 300 ms: frames 4,800 to 14,400.
    let first = out.iter().position(|s| s.abs() > 1e-4).unwrap();
    let last = out.iter().rposition(|s| s.abs() > 1e-4).unwrap();
    assert!(first >= 4_790 && last <= 14_410, "{first}..{last}");
    let mut plain = scatter(1, &[(FREQUENCY, 20.0), (TIME, 100.0)]);
    let reference = run(plain.as_mut(), &impulse(30_000));
    assert_ne!(out, reference, "spray moves the grains");
}

// Spec: grain-delay R5
#[test]
fn spray_and_random_pitch_repeat_run_after_run_and_differ_between_devices() {
    let input = noise(40_000);
    let params = [
        (SPRAY, 80.0),
        (RANDOM_PITCH, 60.0),
        (FREQUENCY, 30.0),
        (TIME, 50.0),
        (FEEDBACK, 40.0),
    ];
    let render = |id: u64, block: usize| {
        let mut node = scatter(id, &params);
        let both = vec![input.clone(), input.clone()];
        let events = [set(9_000, SPRAY, 20.0)];
        run_blocks(node.as_mut(), &both, block, &events, &tempo())
    };
    let first = render(1, 4096);
    assert_eq!(render(1, 4096), first, "run after run");
    assert_ne!(render(2, 4096), first, "another device draws other values");
    for block in [1, 64, 512] {
        assert_eq!(render(1, block), first, "block size {block}");
    }
    assert!(first[0].iter().any(|s| *s != 0.0));
}

// Spec: grain-delay R6
#[test]
fn feedbacks_repeats_decay_by_its_gain() {
    let mut node = scatter(1, &[(FEEDBACK, 50.0), (TIME, 100.0), (FREQUENCY, 20.0)]);
    let out = run(node.as_mut(), &impulse(20_000));
    for (n, want) in [
        (4_800, 1.0f32),
        (9_600, 0.5),
        (14_400, 0.25),
        (19_200, 0.125),
    ] {
        let db = 20.0 * f64::from(out[n] / want).log10();
        assert!(db.abs() < 0.01, "repeat at {n}: {} against {want}", out[n]);
    }
}

// Spec: grain-delay R7
#[test]
fn dry_wet_mixes_the_input_with_the_grains() {
    let input = noise(8_000);
    let mix = |percent: f64| {
        let mut node = scatter(1, &[(DRY_WET, percent), (TIME, 100.0)]);
        run(node.as_mut(), &input)
    };
    assert_eq!(mix(0.0), input, "fully dry passes the input");
    let wet = mix(100.0);
    assert!(
        wet[..4_000].iter().all(|s| s.abs() < 1e-3),
        "fully wet has no dry"
    );
    let half = mix(50.0);
    assert!((half[100] - 0.5 * input[100]).abs() < 1e-3);
}
