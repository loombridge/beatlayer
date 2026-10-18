//! Shutter, the native gate (WP-5.21; the gate spec): where a stepped tone
//! opens and closes it, Attack, Hold and Release, Floor, Flip, Lookahead
//! and its latency, and a sidechain key with its EQ.

bl_rt::install_trap!();

mod common;

use std::f64::consts::TAU;

use bl_devices::gate::{
    ATTACK, FLIP, FLOOR, HOLD, LOOKAHEAD, RELEASE, RETURN, SC_EQ, SC_FREQ, SC_RES, SC_TYPE,
    THRESHOLD, gate_type,
};
use bl_engine::{Block, DeviceType, Node, NodeEvent, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::{RATE, device, node, noise, set};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::gate::register(&mut registry);
    registry
}

fn shutter(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = gate_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// Runs one block of `input` (on both channels) with `key` (also on both)
/// as the sidechain when given, under the allocation trap.
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

/// `input` through `node` in blocks of `block`, with `key` alongside it,
/// returning the left channel.
fn stream(node: &mut dyn Node, input: &[f32], key: Option<&[f32]>, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    for start in (0..input.len()).step_by(block) {
        let end = (start + block).min(input.len());
        let key = key.map(|key| &key[start..end]);
        out.extend_from_slice(&run(node, &input[start..end], key, &[]));
    }
    out
}

/// A sine of `hz` at `amplitude` over `frames` frames, from frame `from`
/// of a phase-continuous tone.
fn tone(hz: f64, amplitude: f64, from: usize, frames: usize) -> Vec<f32> {
    (from..from + frames)
        .map(|n| (amplitude * (TAU * hz * n as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

/// Frames in a step of the stepped tones.
const STEP: usize = 1920;

/// A 500 Hz tone, phase-continuous, whose level in dB is `levels[i]` for
/// step `i`.
fn stepped(levels: &[f64]) -> Vec<f32> {
    levels
        .iter()
        .enumerate()
        .flat_map(|(i, db)| tone(500.0, db_to_gain(*db), i * STEP, STEP))
        .collect()
}

/// Whether anything gets through in the second half of step `i` of `out`.
fn passes(out: &[f32], i: usize) -> bool {
    out[i * STEP + STEP / 2..(i + 1) * STEP]
        .iter()
        .any(|s| *s != 0.0)
}

/// The first frame of `out` at or after `from` where `test` holds.
fn first(out: &[f32], from: usize, test: impl Fn(f32) -> bool) -> usize {
    from + out[from..]
        .iter()
        .position(|s| test(*s))
        .expect("it happens")
}

// Spec: gate R1
#[test]
fn the_gate_is_described_and_declares_a_sidechain() {
    let device_type = gate_type();
    device_type.description.check().unwrap();
    assert!(device_type.sidechain, "a sidechain input");
    let floor = device_type.description.param(FLOOR).unwrap();
    assert!(floor.is_gain() && floor.default == 0.0 && floor.max() == 1.0);
    let look = device_type.description.param(LOOKAHEAD).unwrap();
    assert_eq!(look.format(2.0), "10 ms");
}

// Spec: gate R2
#[test]
fn a_stepped_tone_opens_it_at_the_threshold_and_closes_it_return_below() {
    let fast = [
        (THRESHOLD, -30.0),
        (RETURN, 6.0),
        (ATTACK, 0.01),
        (HOLD, 1.0),
        (RELEASE, 1.0),
    ];
    // Rising from -31 dB in steps of 0.05 dB: the first step that gets
    // through is the threshold's.
    let rising: Vec<f64> = (0..40).map(|i| -31.0 + 0.05 * f64::from(i)).collect();
    let out = stream(shutter(&fast, 4096).as_mut(), &stepped(&rising), None, 4096);
    let opened = rising[(0..40).find(|i| passes(&out, *i)).expect("it opens")];
    assert!((opened + 30.0).abs() < 0.1, "opened at {opened} dB");

    // Open at -20 dB, then falling from -35 dB: the last step that gets
    // through is Threshold less Return.
    let mut falling = vec![-20.0];
    falling.extend((0..40).map(|i| -35.0 - 0.05 * f64::from(i)));
    let out = stream(
        shutter(&fast, 4096).as_mut(),
        &stepped(&falling),
        None,
        4096,
    );
    let last_open = (1..41)
        .rev()
        .find(|i| passes(&out, *i))
        .expect("it was open");
    assert!(last_open < 40, "it closed");
    assert!(
        (falling[last_open] + 36.0).abs() < 0.1,
        "closed below {} dB",
        falling[last_open]
    );
}

// Spec: gate R3
#[test]
fn attack_hold_and_release_are_the_times_they_say() {
    // A steady 0.5 signal, keyed by 0.1 (-20 dB, above the threshold) for
    // 0.5 s and then silence: the output is 0.5 times the gate's gain.
    let params = [
        (THRESHOLD, -30.0),
        (RETURN, 6.0),
        (ATTACK, 10.0),
        (HOLD, 40.0),
        (RELEASE, 100.0),
    ];
    let frames = 48_000;
    let on = 24_000;
    let input = vec![0.5f32; frames];
    let key: Vec<f32> = (0..frames)
        .map(|n| if n < on { 0.1 } else { 0.0 })
        .collect();
    let out = stream(shutter(&params, 4096).as_mut(), &input, Some(&key), 4096);

    let begins = first(&out, 0, |s| s > 0.0);
    let full = first(&out, begins, |s| s >= 0.4995);
    let attack = (full - begins + 1) as f64;
    assert!(
        (attack / 480.0 - 1.0).abs() < 0.05,
        "attack {attack} frames"
    );

    let falls = first(&out, on, |s| s < 0.4995);
    let hold = (falls - on) as f64;
    assert!((hold / 1_920.0 - 1.0).abs() < 0.05, "hold {hold} frames");
    let closed = first(&out, falls, |s| s == 0.0);
    let release = (closed - falls) as f64;
    assert!(
        (release / 4_800.0 - 1.0).abs() < 0.05,
        "release {release} frames"
    );
}

// Spec: gate R4
#[test]
fn floor_sets_a_closed_gates_gain_and_minus_infinity_is_silent() {
    let input = vec![0.5f32; 4096];
    let silent = vec![0.0f32; 4096];
    for (floor, want) in [
        (db_to_gain(-12.0), 0.5 * db_to_gain(-12.0)),
        (1.0, 0.5),
        (0.0, 0.0),
    ] {
        let mut node = shutter(&[(FLOOR, floor)], 4096);
        let out = run(node.as_mut(), &input, Some(&silent), &[]);
        let level = f64::from(out[4095]);
        if want == 0.0 {
            assert!(out.iter().all(|s| *s == 0.0), "-inf is silent");
        } else {
            let error = 20.0 * (level / want).log10();
            assert!(error.abs() < 0.1, "floor {floor}: {level} against {want}");
        }
    }
}

// Spec: gate R5
#[test]
fn flip_passes_only_what_is_below_the_threshold() {
    let params = [
        (THRESHOLD, -30.0),
        (FLIP, 1.0),
        (ATTACK, 1.0),
        (RELEASE, 1.0),
        (HOLD, 1.0),
    ];
    let loud = vec![0.1f32; 4800];
    let quiet = vec![0.001f32; 4800];
    let input = vec![0.5f32; 4800];
    let mut node = shutter(&params, 4800);
    // Above the threshold the flipped gate is shut...
    assert!(
        run(node.as_mut(), &input, Some(&loud), &[])
            .last()
            .is_some_and(|s| *s == 0.0)
    );
    // ... below it, open.
    assert_eq!(
        *run(node.as_mut(), &input, Some(&quiet), &[])
            .last()
            .unwrap(),
        0.5
    );
}

// Spec: gate R6
#[test]
fn lookahead_delays_the_output_and_reports_the_latency() {
    for (choice, delay) in [(0.0, 0usize), (1.0, 48), (2.0, 480)] {
        let mut node = shutter(&[(LOOKAHEAD, choice), (FLOOR, 1.0)], 2048);
        assert_eq!(node.latency(), delay as u32);
        let mut input = vec![0.0f32; 2048];
        input[100] = 1.0;
        let out = run(node.as_mut(), &input, None, &[]);
        let at = out.iter().position(|s| *s != 0.0).expect("the impulse");
        assert_eq!(at, 100 + delay, "lookahead choice {choice}");
    }
    // The gate has begun to open by the time a delayed onset comes out.
    let onset: Vec<f32> = (0..4096)
        .map(|n| if n >= 2000 { 0.5 } else { 0.0 })
        .collect();
    let params = |look: f64| [(THRESHOLD, -30.0), (ATTACK, 1.0), (LOOKAHEAD, look)];
    let plain = stream(shutter(&params(0.0), 4096).as_mut(), &onset, None, 4096);
    let ahead = stream(shutter(&params(2.0), 4096).as_mut(), &onset, None, 4096);
    assert!(
        plain[2000] < 0.02,
        "no lookahead: the onset is cut by the attack"
    );
    assert_eq!(
        ahead[2000 + 480],
        0.5,
        "lookahead: the gate is open before it"
    );
}

// Spec: gate R7
#[test]
fn a_key_opens_it_on_a_quiet_input_and_the_sc_eq_hears_only_its_band() {
    let params = [
        (THRESHOLD, -30.0),
        (ATTACK, 0.1),
        (HOLD, 1.0),
        (RELEASE, 1.0),
    ];
    let quiet = vec![0.001f32; 4800];
    // Without a key the input (-60 dB) is below the threshold: shut.
    let mut node = shutter(&params, 4800);
    assert!(
        run(node.as_mut(), &quiet, None, &[])
            .iter()
            .all(|s| *s == 0.0)
    );
    // A loud key from elsewhere opens it, and the quiet input passes.
    let key = vec![0.5f32; 4800];
    let mut node = shutter(&params, 4800);
    assert_eq!(
        *run(node.as_mut(), &quiet, Some(&key), &[]).last().unwrap(),
        0.001
    );

    // With the SC EQ a band pass at 1 kHz, a loud 100 Hz key leaves it
    // shut and a loud 1 kHz key opens it.
    let mut band = params.to_vec();
    band.extend([
        (SC_EQ, 1.0),
        (SC_TYPE, 1.0),
        (SC_FREQ, 1_000.0),
        (SC_RES, 5.0),
    ]);
    let low = tone(100.0, 0.5, 0, 4800);
    let mid = tone(1_000.0, 0.5, 0, 4800);
    let mut node = shutter(&band, 4800);
    assert!(
        run(node.as_mut(), &quiet, Some(&low), &[])
            .iter()
            .all(|s| *s == 0.0)
    );
    let mut node = shutter(&band, 4800);
    assert!(
        run(node.as_mut(), &quiet, Some(&mid), &[])
            .iter()
            .any(|s| *s != 0.0)
    );
}

// Spec: gate R8
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let input: Vec<f32> = noise(16_384).iter().map(|s| s * 0.05).collect();
    let mut renders = Vec::new();
    for block in [1usize, 64, 512, 4096] {
        let params = [(LOOKAHEAD, 2.0), (THRESHOLD, -30.0), (FLOOR, 0.1)];
        let mut node = shutter(&params, block);
        let mut out = Vec::new();
        for (n, chunk) in input.chunks(block).enumerate() {
            let events = if n == 0 {
                vec![set(0, HOLD, 5.0), set(0, RELEASE, 20.0), set(0, FLOOR, 0.3)]
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

// Spec: gate R9
#[test]
fn the_display_reads_the_threshold_and_return_and_the_published_gain_reduction() {
    let device_type = gate_type();
    let values = device(&device_type, &[(THRESHOLD, -24.0), (RETURN, 8.0)]);
    let shown = |data: Option<&[f32]>| {
        bl_devices::display(
            bl_devices::gate::GATE,
            values.id,
            &device_type.description,
            &values.params,
            data,
        )
        .expect("Shutter has a display")
    };
    let from_values = shown(None);
    let texts: Vec<_> = from_values
        .readings
        .iter()
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(texts, ["-24.0 dB", "8.0 dB"]);
    let live = shown(Some(&[-12.0, -20.0, -32.0]));
    assert!(
        live.readings
            .iter()
            .any(|r| r.name == "Gain Reduction" && r.value == -12.0)
    );

    // A closed gate with a -12 dB floor publishes -12 dB; an open one 0 dB.
    let mut node = shutter(&[(FLOOR, db_to_gain(-12.0)), (ATTACK, 0.01)], 4096);
    let mut reader = node.display().expect("a display");
    run(node.as_mut(), &vec![0.0; 4096], None, &[]);
    assert!((reader.read().values[0] + 12.0).abs() < 0.01);
    run(node.as_mut(), &vec![0.5; 4096], None, &[]);
    assert!(reader.read().values[0].abs() < 0.01);
}
