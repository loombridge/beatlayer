//! WP-2.19b's acceptance in the engine: devices' sidechains (the routing
//! spec, R22 to R27).
//!
//! The device under test is the test sidechain probe, which multiplies its
//! input by what its detector hears while External is on. Every track is a
//! DC level, so Main's output says exactly what the detector heard. The
//! sidechain's commands run by ID with JSON arguments, as an agent runs
//! them (D17), and every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use bl_commands::Registry as Commands;
use bl_engine::routing::Graph;
use bl_engine::testing::KEY;
use bl_model::{Command, Device, DeviceId, DeviceKind, Editor, TrackId, TrackKind, registry};
use common::{Rig, dc, track};
use serde_json::{Value, json};

fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// Frames to render after a change, so a 20 ms glide has finished before
/// the last block.
const SETTLE: usize = 1280;

fn settled(rig: &mut Rig) -> f32 {
    rig.render(SETTLE, 256);
    let last = rig.last();
    assert!(last.iter().all(|&s| s == last[0]), "a steady level");
    last[0]
}

/// Puts the sidechain probe at the end of `track`'s chain.
fn key(editor: &mut Editor, track: TrackId) -> DeviceId {
    let id = editor.new_id();
    let device = Device::new(
        id,
        "Test Sidechain",
        DeviceKind::Native {
            device_type: KEY.to_owned(),
        },
    )
    .with_sidechain();
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

fn source(editor: &mut Editor, device: DeviceId, from: TrackId, tap: &str) {
    run(
        editor,
        "device.set_sidechain_input",
        json!({ "device": device, "input": { "type": "track", "track": from, "tap": tap } }),
    );
}

fn set(rig: &mut Rig, id: &str, device: DeviceId, field: &str, value: Value) -> bool {
    run(
        &mut rig.editor,
        id,
        json!({ "device": device, field: value }),
    );
    rig.sync().program
}

// Spec: routing R22, R23, R24
#[test]
fn a_device_hears_its_source_through_gain_and_mix_and_listen_plays_it() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let s = track(ed, TrackKind::Audio, None);
    let t = track(ed, TrackKind::Audio, None);
    dc(ed, s, 0.25);
    dc(ed, t, 0.5);
    let gate = key(ed, t);
    rig.sync();
    assert_eq!(
        settled(&mut rig),
        0.75,
        "External off: the device listens to itself"
    );

    // Choosing a source, Post FX as the window picks, is a new program; with
    // External off, nothing changes.
    source(&mut rig.editor, gate, s, "post_fx");
    assert!(rig.sync().program);
    assert_eq!(settled(&mut rig), 0.75);

    // External on: the detector hears S (0.25), so T is 0.125. S still
    // plays on Main.
    assert!(!set(
        &mut rig,
        "device.set_sidechain_enabled",
        gate,
        "enabled",
        json!(true)
    ));
    assert_eq!(settled(&mut rig), 0.25 + 0.125);
    // SC Gain ×4: it hears 1.
    assert!(!set(
        &mut rig,
        "device.set_sidechain_gain",
        gate,
        "gain",
        json!(4.0)
    ));
    assert_eq!(settled(&mut rig), 0.25 + 0.5);
    // Mix 50 %: it hears 4 · 0.5 · 0.25 + 0.5 · 0.5 = 0.75.
    assert!(!set(
        &mut rig,
        "device.set_sidechain_mix",
        gate,
        "mix",
        json!(0.5)
    ));
    assert_eq!(settled(&mut rig), 0.25 + 0.375);
    // Listen: T plays what the detector hears.
    assert!(!set(
        &mut rig,
        "device.set_sidechain_listen",
        gate,
        "listen",
        json!(true)
    ));
    assert_eq!(settled(&mut rig), 0.25 + 0.75);

    // The taps: S at half volume plays 0.125 on Main. Post FX is before
    // its mixer; Post Mixer after it; Pre FX before its DC device, silence.
    run(
        &mut rig.editor,
        "mixer.set_volume",
        json!({ "track": s, "volume": 0.5 }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig), 0.125 + 0.75);
    source(&mut rig.editor, gate, s, "post_mixer");
    rig.sync();
    assert_eq!(settled(&mut rig), 0.125 + (4.0 * 0.5 * 0.125 + 0.25));
    source(&mut rig.editor, gate, s, "pre_fx");
    rig.sync();
    assert_eq!(settled(&mut rig), 0.125 + 0.25);

    // External off again: the device listens to itself, Listen or not.
    set(
        &mut rig,
        "device.set_sidechain_enabled",
        gate,
        "enabled",
        json!(false),
    );
    assert_eq!(settled(&mut rig), 0.125 + 0.5);
}

// Spec: routing R25
#[test]
fn deleting_the_source_leaves_no_input_and_the_device_its_own_input() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let s = track(ed, TrackKind::Audio, None);
    let t = track(ed, TrackKind::Audio, None);
    dc(ed, s, 0.25);
    dc(ed, t, 0.5);
    let gate = key(ed, t);
    source(ed, gate, s, "post_fx");
    run(
        ed,
        "device.set_sidechain_enabled",
        json!({ "device": gate, "enabled": true }),
    );
    run(
        ed,
        "device.set_sidechain_mix",
        json!({ "device": gate, "mix": 0.5 }),
    );
    rig.sync();
    // It hears 0.5 · 0.25 + 0.5 · 0.5.
    assert_eq!(settled(&mut rig), 0.25 + 0.5 * 0.375);
    rig.apply(Command::DeleteTrack { track: s });
    // No Input: it hears only its own half, External still on.
    assert_eq!(settled(&mut rig), 0.5 * 0.25);
    let sidechain = rig
        .editor
        .document()
        .device(gate)
        .unwrap()
        .sidechain
        .clone()
        .unwrap();
    assert!(sidechain.enabled);
    // Deleting the device takes its sidechain with it.
    rig.editor.undo().unwrap();
    rig.sync();
    rig.apply(Command::RemoveDevice { device: gate });
    assert_eq!(settled(&mut rig), 0.25 + 0.5);
}

// Spec: routing R26
#[test]
fn a_sidechain_that_closes_a_loop_is_silent_and_its_own_track_before_its_devices_is_heard() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let s = track(ed, TrackKind::Audio, None);
    let t = track(ed, TrackKind::Audio, None);
    dc(ed, s, 0.25);
    dc(ed, t, 0.5);
    let gate = key(ed, t);
    run(
        ed,
        "device.set_sidechain_enabled",
        json!({ "device": gate, "enabled": true }),
    );
    // T's output into S's Track In, S monitoring: S hears T. T's sidechain
    // listening to S would close a loop, so it's silent: T is 0.
    run(
        ed,
        "routing.set_output",
        json!({ "track": t, "output": { "type": "track", "track": s } }),
    );
    run(
        ed,
        "routing.set_monitor",
        json!({ "track": s, "monitor": "in" }),
    );
    source(ed, gate, s, "post_fx");
    let graph = Graph::of(rig.editor.document());
    assert_eq!(graph.broken, vec![(s, t)]);
    rig.sync();
    assert_eq!(settled(&mut rig), 0.25);

    // Its own track: after its devices it would hear itself, so it's
    // silent; before them (where T's input, routed from another track,
    // arrives) it's heard.
    let ed = &mut rig.editor;
    run(
        ed,
        "routing.set_output",
        json!({ "track": t, "output": { "type": "main" } }),
    );
    run(
        ed,
        "routing.set_monitor",
        json!({ "track": s, "monitor": "auto" }),
    );
    source(ed, gate, t, "post_fx");
    assert_eq!(Graph::of(rig.editor.document()).broken, vec![(t, t)]);
    rig.sync();
    assert_eq!(settled(&mut rig), 0.25);
    let ed = &mut rig.editor;
    let u = track(ed, TrackKind::Audio, None);
    dc(ed, u, 0.5);
    run(
        ed,
        "routing.set_output",
        json!({ "track": u, "output": { "type": "track", "track": t } }),
    );
    run(
        ed,
        "routing.set_monitor",
        json!({ "track": t, "monitor": "in" }),
    );
    source(ed, gate, t, "pre_fx");
    assert!(Graph::of(rig.editor.document()).broken.is_empty());
    rig.sync();
    // T's chain: U's 0.5 arrives, DC adds 0.5, and the gate multiplies the
    // 1 by what it heard before the chain, U's 0.5.
    assert_eq!(settled(&mut rig), 0.25 + 0.5);
}

// Spec: routing R27
#[test]
fn soloing_leaves_a_sidechain_listening() {
    let mut rig = Rig::new(256);
    let ed = &mut rig.editor;
    let s = track(ed, TrackKind::Audio, None);
    let t = track(ed, TrackKind::Audio, None);
    dc(ed, s, 0.25);
    dc(ed, t, 0.5);
    let gate = key(ed, t);
    source(ed, gate, s, "post_mixer");
    run(
        ed,
        "device.set_sidechain_enabled",
        json!({ "device": gate, "enabled": true }),
    );
    run(ed, "mixer.set_solo", json!({ "track": t, "solo": true }));
    rig.sync();
    assert_eq!(
        settled(&mut rig),
        0.125,
        "S is silent on Main but still heard by the gate"
    );
}
