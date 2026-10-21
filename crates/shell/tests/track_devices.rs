//! Deactivate All Devices through the shell (IMP-25; the device chains spec,
//! R39): one command that switches off every top-level device on the selected
//! tracks as one undo step, and turns them back on when they are all off
//! already. It reaches no rack's contents.

use bl_engine::testing;
use bl_model::{ChainId, DeviceId, TrackId};
use bl_shell::Shell;
use bl_shell::device_commands::DeviceAdded;
use bl_shell_spec::tracks::{DevicesEnabled, SET_DEVICES_ENABLED};
use serde_json::{Value, json};

struct Set {
    shell: Shell,
    audio: TrackId,
}

fn set() -> Set {
    let mut shell = Shell::headless();
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    Set { shell, audio }
}

fn effect(name: &str) -> Value {
    let t = testing::gain_type();
    json!({ "kind": t.kind, "name": name, "role": t.role })
}

fn rack_of(kind: &str) -> Value {
    json!({ "kind": { "rack": { "kind": kind } }, "name": "Rack", "role": "audio_effect" })
}

fn add(shell: &mut Shell, args: Value) -> DeviceAdded {
    serde_json::from_value(shell.invoke("device.add", args).unwrap()).unwrap()
}

fn enabled(shell: &Shell, device: DeviceId) -> bool {
    shell.editor().document().device(device).unwrap().enabled
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn run(shell: &mut Shell, args: Value) -> DevicesEnabled {
    serde_json::from_value(shell.invoke(SET_DEVICES_ENABLED, args).unwrap()).unwrap()
}

// Spec: device-chains R39
#[test]
fn deactivate_all_devices_switches_off_every_top_level_device_in_one_step() {
    let Set { mut shell, audio } = set();
    let a = add(&mut shell, json!({ "device": effect("A") })).device;
    let b = add(&mut shell, json!({ "device": effect("B") })).device;
    let before = steps(&shell);
    assert!(enabled(&shell, a) && enabled(&shell, b));

    // The menu item, by its command ID, with `enabled` left out: it switches
    // them off and reads back through each device's `enabled`.
    let result = run(&mut shell, json!({}));
    assert_eq!(result.tracks, vec![audio]);
    assert!(!result.enabled);
    assert!(!enabled(&shell, a) && !enabled(&shell, b));
    assert_eq!(steps(&shell), before + 1, "one undo step");
    assert_eq!(last_step(&shell), "Deactivate All Devices");

    // Every device is off, so it reads Activate All Devices and turns them
    // back on, again in one step.
    let result = run(&mut shell, json!({}));
    assert!(result.enabled);
    assert!(enabled(&shell, a) && enabled(&shell, b));
    assert_eq!(steps(&shell), before + 2, "one undo step");
    assert_eq!(last_step(&shell), "Activate All Devices");

    // Undone in one step, back to off.
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(!enabled(&shell, a) && !enabled(&shell, b));
    assert_eq!(steps(&shell), before + 1);
}

// Spec: device-chains R39
#[test]
fn deactivate_all_devices_reaches_no_racks_contents() {
    let Set { mut shell, audio } = set();
    let rack = add(&mut shell, json!({ "device": rack_of("audio_effect") })).device;
    let chain: ChainId = shell.new_id();
    shell
        .invoke(
            "chain.add",
            json!({ "rack": rack, "index": 0, "chain": { "id": chain, "name": "", "devices": [] } }),
        )
        .unwrap();
    let inner = add(
        &mut shell,
        json!({ "device": effect("Inner"), "chain": chain }),
    )
    .device;

    run(&mut shell, json!({}));
    assert!(!enabled(&shell, rack), "the rack is a top-level device");
    assert!(enabled(&shell, inner), "a chain's device stays as it is");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(audio)
            .unwrap()
            .devices()
            .len(),
        1,
        "the track holds only the rack"
    );
}
