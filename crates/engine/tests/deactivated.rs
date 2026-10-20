//! IMP-15: a deactivated device or track costs nothing.
//!
//! - A switched-off device is out of the schedule: it isn't run, and the
//!   changes to its parameters still reach it (the device-chains spec, R6).
//! - A track whose activator is off, and whose signal nothing reads before
//!   its mixer, runs no devices; with a pre-fader send to a return, it
//!   keeps running and the send still sounds (the mixer spec, R5).
//!
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::atomic::Ordering::Relaxed;

use bl_commands::Registry as Commands;
use bl_engine::testing;
use bl_model::{Command, Editor, TrackKind, registry};
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
fn a_switched_off_device_is_out_of_the_schedule_and_takes_its_parameter_change() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let counter = device(&mut rig.editor, a, testing::COUNTER, &[]);
    let level = dc(&mut rig.editor, a, 0.5);
    rig.sync();
    rig.block(64);
    assert_eq!(rig.probe.counted.load(Relaxed), 64, "the counter counts");

    for device in [counter, level] {
        rig.apply(Command::SetDeviceEnabled {
            device,
            enabled: false,
        });
    }
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
    let synced = rig.apply(Command::SetDeviceParam {
        device: level,
        param: testing::VALUE,
        value: 0.75,
    });
    assert!(!synced.program);
    rig.block(64);

    for device in [counter, level] {
        rig.apply(Command::SetDeviceEnabled {
            device,
            enabled: true,
        });
    }
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

// Spec: IMP-15, the mixer spec, R5.
#[test]
fn a_deactivated_track_with_nothing_reading_it_runs_no_devices() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    device(&mut rig.editor, a, testing::COUNTER, &[]);
    dc(&mut rig.editor, a, 0.5);
    rig.sync();
    rig.block(64);
    assert_eq!(rig.probe.counted.load(Relaxed), 64, "the counter counts");

    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": a, "mute": true }),
    );
    rig.sync();
    // The activator takes effect in the block it is sent for, so the
    // devices run in that block and not in the one after.
    rig.block(64);
    let counted = rig.probe.counted.load(Relaxed);
    rig.block(64);
    rig.block(64);
    assert_eq!(
        rig.probe.counted.load(Relaxed),
        counted,
        "the track's devices don't run"
    );
    assert!(rig.last().iter().all(|&s| s == 0.0), "the track is silent");

    // Back on: the devices run again.
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": a, "mute": false }),
    );
    rig.sync();
    rig.block(64);
    rig.block(64);
    assert!(
        rig.probe.counted.load(Relaxed) > counted,
        "the counter counts again"
    );
}

// Spec: IMP-15, the mixer spec, R5.
#[test]
fn a_deactivated_track_with_a_pre_fader_send_keeps_running_and_the_send_sounds() {
    let mut rig = Rig::new(64);
    let pre = track(&mut rig.editor, TrackKind::Return, None);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    device(&mut rig.editor, a, testing::COUNTER, &[]);
    dc(&mut rig.editor, a, 0.5);
    run(
        &mut rig.editor,
        "mixer.set_send_tap",
        json!({ "track": pre, "tap": "pre" }),
    );
    run(
        &mut rig.editor,
        "mixer.set_send",
        json!({ "track": a, "to": pre, "level": 1.0 }),
    );
    run(
        &mut rig.editor,
        "mixer.set_volume",
        json!({ "track": pre, "volume": 1.0 }),
    );
    rig.sync();
    rig.block(64);
    assert_eq!(rig.probe.counted.load(Relaxed), 64, "the counter counts");

    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": a, "mute": true }),
    );
    rig.sync();
    // The activator and the send glide over 20 ms (960 frames).
    for _ in 0..21 {
        rig.block(64);
    }
    assert!(
        rig.probe.counted.load(Relaxed) > 64,
        "the pre-fader send keeps the track running"
    );
    assert!(
        rig.last().iter().all(|&s| s == 0.5),
        "the pre-fader send still sounds, {}",
        rig.last()[0]
    );
}
