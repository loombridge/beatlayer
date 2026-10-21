//! IMP-15: a switched-off device costs nothing.
//!
//! - A switched-off device isn't run, and the changes to its parameters
//!   still reach it (the device-chains spec, R6).
//!
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::atomic::Ordering::Relaxed;

use bl_commands::Registry as Commands;
use bl_engine::testing;
use bl_model::{Editor, TrackKind, registry};
use common::{Rig, dc, device, track};
use serde_json::{Value, json};

/// Runs a document command by ID, as an agent does, checking its arguments
/// against the registered schema first.
fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

// Spec: device-chains R6, IMP-15.
#[test]
fn a_switched_off_device_is_not_run_and_takes_its_parameter_change() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let counter = device(&mut rig.editor, a, testing::COUNTER, &[]);
    let level = dc(&mut rig.editor, a, 0.5);
    rig.sync();
    rig.block(64);
    assert_eq!(rig.probe.counted.load(Relaxed), 64, "the counter counts");

    for device in [counter, level] {
        run(
            &mut rig.editor,
            "device.set_enabled",
            json!({ "device": device, "enabled": false }),
        );
    }
    rig.sync();
    rig.block(64);
    assert_eq!(rig.last()[0], 0.0, "off: no DC");
    let counted = rig.probe.counted.load(Relaxed);
    rig.block(64);
    rig.block(64);
    assert_eq!(
        rig.probe.counted.load(Relaxed),
        counted,
        "off: the device isn't run"
    );

    // A change while it's off still reaches it, without a recompile.
    run(
        &mut rig.editor,
        "device.set_param",
        json!({ "device": level, "param": testing::VALUE, "value": 0.75 }),
    );
    assert!(!rig.sync().program);
    rig.block(64);

    for device in [counter, level] {
        run(
            &mut rig.editor,
            "device.set_enabled",
            json!({ "device": device, "enabled": true }),
        );
    }
    rig.sync();
    rig.block(64);
    assert_eq!(
        rig.last()[0],
        0.75,
        "back on, with the change made while off"
    );
    assert!(
        rig.probe.counted.load(Relaxed) > counted,
        "counting on again"
    );
}
