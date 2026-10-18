//! The Strike Mod (WP-5.11; the modulation spec, R26): each note plays its
//! shape from the start, a sustain breakpoint holds the shape while a key is
//! held and lets it go on when the key ends, Loop starts it again past the
//! sustain point, Velocity scales it and Echo repeats it. The notes pass
//! through untouched. Every block runs as a real-time section under the
//! allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Frames, TempoMap};
use bl_devices::modulator::RATE_HZ;
use bl_devices::strike_mod::{ECHO, ECHO_TIME, LOOP, VELOCITY, strike_mod_type};
use bl_engine::modbus::Shape;
use bl_engine::{Collector, Node, NoteEvent, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::modulator::{self, POLARITY, ShapePoint, with_sustain};
use common::{Signal, device, node, note_off, note_on, signal};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::strike_mod::register(&mut registry);
    registry
}

fn playing(at: usize) -> TransportInfo {
    TransportInfo {
        playing: true,
        position: Frames(at as i64),
    }
}

/// A Strike Mod with the shape a new one has (a rise to the top by a tenth
/// of the cycle, which is its sustain breakpoint, and a fall to the end),
/// playing a cycle in 4,800 frames, slot 1 giving its output itself;
/// `params` over those.
fn strike(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![
        (modulator::amount(0), 100.0),
        (POLARITY, 1.0),
        (RATE_HZ, 10.0),
    ];
    all.extend_from_slice(params);
    node(&registry(), &device(&strike_mod_type(), &all), 512)
}

fn run(node: &mut dyn Node, frames: usize, block: usize, notes: &[NoteEvent]) -> Signal {
    signal(
        node,
        frames,
        block,
        &[],
        None,
        notes,
        &[],
        &TempoMap::default(),
        playing,
    )
}

fn rises(level: &[f32], threshold: f32) -> usize {
    level
        .windows(2)
        .filter(|w| w[0] < threshold && w[1] >= threshold)
        .count()
}

// Spec: modulation R26
#[test]
fn the_sustain_breakpoint_holds_while_the_note_is_held() {
    let on = note_on(1_000, 60, 1.0, None);
    let notes = [on, note_off(15_000, &on, 0.5)];
    let mut node = strike(&[]);
    let out = run(node.as_mut(), 26_000, 512, &notes);
    let level = out.rows.contribution(0);
    assert!(
        level[..1_000].iter().all(|&v| v == 0.0),
        "rests at the start"
    );
    assert!(
        level[1_800] > 0.9,
        "up by the sustain breakpoint: {}",
        level[1_800]
    );
    // Held at the top for as long as the key is, far past a cycle.
    assert!(
        level[2_000..15_000].iter().all(|&v| v > 0.99),
        "held at the sustain breakpoint"
    );
    // The key's end lets the shape go on to its end: a fall over nine tenths
    // of a cycle, 4,320 frames.
    assert!(
        (level[17_000] - 0.54).abs() < 0.06,
        "falling: {}",
        level[17_000]
    );
    assert!(level[19_800] < 0.01, "to the end: {}", level[19_800]);
    assert!(level[26_000 - 1] < 0.01, "and there it rests");
    assert_eq!(out.notes, notes.to_vec(), "the notes pass through");
}

// Spec: modulation R26
#[test]
fn loop_repeats_past_the_sustain_point() {
    let on = note_on(1_000, 60, 1.0, None);
    let mut node = strike(&[(LOOP, 1.0)]);
    let out = run(node.as_mut(), 25_000, 512, &[on]);
    let level = out.rows.contribution(0);
    // Starts at 1,000, 5,800, 10,600, 15,400 and 20,200.
    assert_eq!(rises(level, 0.9), 5);
    assert!(level[5_700] < 0.1, "down at each end: {}", level[5_700]);

    // Without Loop the same key holds the top.
    let mut node = strike(&[]);
    let held = run(node.as_mut(), 25_000, 512, &[on]);
    assert_eq!(rises(held.rows.contribution(0), 0.9), 1);
}

// Spec: modulation R26
#[test]
fn velocity_and_echo_scale_the_output() {
    let on = note_on(0, 60, 0.5, None);
    let held = [on];
    let held_peak = |params: &[(ParamId, f64)]| {
        let mut node = strike(params);
        let out = run(node.as_mut(), 3_000, 512, &held);
        out.rows.contribution(0).iter().copied().fold(0.0, f32::max)
    };
    assert!(
        (held_peak(&[]) - 1.0).abs() < 0.02,
        "0 % leaves velocity out"
    );
    assert!((held_peak(&[(VELOCITY, 100.0)]) - 0.5).abs() < 0.02);
    assert!((held_peak(&[(VELOCITY, 50.0)]) - 0.75).abs() < 0.02);

    // Echo: a key held for a moment at the top, then 100 ms between echoes,
    // each half the last.
    let top = [
        note_on(0, 60, 1.0, None),
        note_off(1_000, &note_on(0, 60, 1.0, None), 0.5),
    ];
    let mut node = strike(&[(ECHO_TIME, 100.0), (ECHO, 50.0)]);
    let out = run(node.as_mut(), 4_800 * 3, 512, &top);
    let level = out.rows.contribution(0);
    for (echo, expected) in [1.0f32, 0.5, 0.25].into_iter().enumerate() {
        let window = &level[echo * 4_800..(echo + 1) * 4_800];
        let peak = window.iter().copied().fold(0.0, f32::max);
        assert!(
            (peak - expected).abs() < 0.03,
            "echo {echo}: {peak} for {expected}"
        );
    }
}

// Spec: modulation R26
#[test]
fn a_new_shape_with_a_sustain_breakpoint_reaches_the_device_as_node_data() {
    let collector = Collector::new();
    let p = |x, y, sustain| ShapePoint {
        x,
        y,
        bend: 0.0,
        sustain,
    };
    // A flat shape at 0.25; then one that rises to 0.75 and holds there.
    let mut node = strike(&[]);
    node.receive_data(collector.handle().data(Shape(vec![p(0.0, 0.25, false)])));
    let on = note_on(0, 60, 1.0, None);
    let out = run(node.as_mut(), 1_024, 512, &[on]);
    assert!((out.rows.contribution(0)[900] - 0.25).abs() < 1e-3);
    node.receive_data(collector.handle().data(Shape(vec![
        p(0.0, 0.0, false),
        p(0.05, 0.75, true),
        p(1.0, 0.0, false),
    ])));
    let again = note_on(0, 60, 1.0, None);
    let out = run(node.as_mut(), 2_048, 512, &[again]);
    assert!((out.rows.contribution(0)[2_000] - 0.75).abs() < 0.02);

    // Making a breakpoint the sustain breakpoint takes the flag from the
    // last one.
    let points = vec![p(0.0, 0.0, true), p(0.5, 1.0, false), p(1.0, 0.0, false)];
    let moved = with_sustain(&points, 1);
    assert_eq!(
        moved.iter().map(|p| p.sustain).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert!(
        with_sustain(&moved, 1).iter().all(|p| !p.sustain),
        "again takes it away"
    );
}

// Spec: modulation R26
#[test]
fn the_output_is_the_same_at_any_block_size() {
    let on = note_on(700, 60, 0.7, None);
    let notes = [on, note_off(9_000, &on, 0.5)];
    let params = [(VELOCITY, 80.0), (ECHO, 30.0)];
    let mut node = strike(&params);
    let reference = run(node.as_mut(), 20_000, 512, &notes);
    for block in [1, 32, 64] {
        let mut again = strike(&params);
        assert_eq!(
            run(again.as_mut(), 20_000, block, &notes).rows.0,
            reference.rows.0,
            "blocks of {block} frames"
        );
    }
}
