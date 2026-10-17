//! The Curve Mod (WP-5.10; the modulation spec, R23): it plays its
//! breakpoints and their curves, 1-Shot plays once per trigger, Manual
//! outputs the point of the shape it is given, and a new shape reaches it as
//! node data. Every block runs as a real-time section under the allocation
//! trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Frames, TempoMap};
use bl_devices::modulator::{MANUAL, RATE_HZ, TRIGGER, TRIGGER_MODE, curve_mod_type};
use bl_engine::modbus::Shape;
use bl_engine::{Collector, Node, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::modulator::{self, POLARITY, ShapePoint, shape_value};
use common::{control, device, node, set};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::modulator::register(&mut registry);
    registry
}

fn playing() -> TransportInfo {
    TransportInfo {
        playing: true,
        position: Frames(0),
    }
}

/// A Curve Mod playing `shape`, slot 1 giving its output itself, with
/// `params` over that.
fn curve(shape: &[ShapePoint], params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![(modulator::amount(0), 100.0), (POLARITY, 1.0)];
    all.extend_from_slice(params);
    let mut device = device(&curve_mod_type(), &all);
    device.shape = Some(shape.to_vec());
    node(&registry(), &device, 4_800)
}

const P: fn(f64, f64, f64) -> ShapePoint = |x, y, bend| ShapePoint { x, y, bend };

// Spec: modulation R23
#[test]
fn the_shape_plays_its_breakpoints_and_curves() {
    // A rise to the middle, a curved fall, and a floor.
    let shape = [
        P(0.0, 0.0, 0.0),
        P(0.5, 1.0, 0.6),
        P(0.9, 0.25, 0.0),
        P(1.0, 0.25, 0.0),
    ];
    let mut node = curve(&shape, &[(RATE_HZ, 10.0)]);
    // 10 Hz is 150 grid steps to the cycle.
    let out = control(
        node.as_mut(),
        4_800 * 2,
        500,
        &[],
        &TempoMap::default(),
        playing(),
    );
    let out = out.contribution(0);
    for step in 0..300usize {
        let x = (step % 150) as f64 / 150.0;
        let got = f64::from(out[step * 32]);
        let want = shape_value(&shape, x);
        assert!((got - want).abs() < 1e-5, "step {step}: {got} isn't {want}");
    }
    // The curve bows: at a quarter of the way down the fall it isn't on the
    // straight line.
    let straight = 1.0 + (0.25 - 1.0) * 0.25;
    assert!((shape_value(&shape, 0.5 + 0.4 * 0.25) - straight).abs() > 0.01);
}

// Spec: modulation R23
#[test]
fn one_shot_plays_once_per_trigger() {
    let shape = [P(0.0, 0.0, 0.0), P(1.0, 1.0, 0.0)];
    let mut node = curve(&shape, &[(RATE_HZ, 10.0), (TRIGGER_MODE, 1.0)]);
    let events = [
        set(1_000, TRIGGER, 1.0),
        set(8_000, TRIGGER, 0.0),
        set(10_000, TRIGGER, 1.0),
    ];
    let out = control(
        node.as_mut(),
        16_000,
        700,
        &events,
        &TempoMap::default(),
        playing(),
    );
    let out = out.contribution(0);
    assert!(
        out[..900].iter().all(|&v| v == 0.0),
        "rests at the start before it is triggered"
    );
    assert!(
        out[1_000 + 2_400] > 0.4 && out[1_000 + 2_400] < 0.6,
        "half way after half a cycle"
    );
    assert!(
        out[1_000 + 4_800 + 200..9_900].iter().all(|&v| v == 1.0),
        "rests at the end, not looping"
    );
    assert!(
        out[10_000 + 2_400] > 0.4 && out[10_000 + 2_400] < 0.6,
        "plays again at the next trigger"
    );
    assert!(out[15_900] == 1.0);
}

// Spec: modulation R23
#[test]
fn manual_mode_outputs_the_picked_point() {
    let shape = [P(0.0, 0.0, 0.0), P(0.5, 1.0, 0.0), P(1.0, 0.0, 0.0)];
    let mut node = curve(&shape, &[(TRIGGER_MODE, 2.0), (MANUAL, 25.0)]);
    let events = [set(2_000, MANUAL, 75.0), set(4_000, MANUAL, 50.0)];
    let out = control(
        node.as_mut(),
        6_000,
        400,
        &events,
        &TempoMap::default(),
        playing(),
    );
    let out = out.contribution(0);
    // The control glides for 20 ms, so look well after each move.
    assert!((out[1_500] - 0.5).abs() < 1e-5, "25 % of the triangle");
    assert!((out[3_900] - 0.5).abs() < 1e-5, "75 % of it");
    assert!((out[5_900] - 1.0).abs() < 1e-5, "50 % of it");
}

// Spec: modulation R23
#[test]
fn a_new_shape_reaches_the_device_as_node_data() {
    let collector = Collector::new();
    let mut node = curve(&[P(0.0, 1.0, 0.0)], &[(TRIGGER_MODE, 2.0), (MANUAL, 50.0)]);
    let out = control(
        node.as_mut(),
        256,
        256,
        &[],
        &TempoMap::default(),
        playing(),
    );
    assert_eq!(out.contribution(0)[200], 1.0);
    node.receive_data(collector.handle().data(Shape(vec![P(0.0, 0.25, 0.0)])));
    let out = control(
        node.as_mut(),
        256,
        256,
        &[],
        &TempoMap::default(),
        playing(),
    );
    assert_eq!(out.contribution(0)[200], 0.25);
}
