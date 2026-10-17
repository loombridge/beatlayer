//! A device's A and B states in the shell (WP-5.12; the device compare spec,
//! R3 to R9 and R12): switching and copying, their undo steps and labels,
//! what they do to automation, and what a copy and a preset keep. The
//! device is the framework's test utility, whose Gain is linear and 1 by
//! default.

use bl_devices::testing::{self, UTILITY_GAIN};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::{DeviceAdded, DeviceParameters};
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

const SWITCH: &str = "device.compare_switch";
const COPY: &str = "device.compare_copy";

fn shell() -> (Shell, TrackId) {
    let mut shell = Shell::headless();
    shell.set_device_types(testing::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    (shell, audio)
}

fn add(shell: &mut Shell) -> DeviceId {
    let t = testing::utility_type();
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn set_gain(shell: &mut Shell, device: DeviceId, value: f64) {
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": UTILITY_GAIN, "value": value }),
        )
        .unwrap();
}

/// The Gain in the active state and in the other one, and which is active.
fn gain(shell: &mut Shell, device: DeviceId) -> (f64, f64, String) {
    let reply = shell
        .run("device.parameters", json!({ "device": device }))
        .unwrap();
    let parameters: DeviceParameters = serde_json::from_value(reply).unwrap();
    let gain = &parameters.params[0];
    (
        gain.value,
        gain.other.expect("a native device has two states"),
        parameters.active.expect("a native device has two states"),
    )
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

// Spec: device-compare R2, R3, R12
#[test]
fn switching_plays_the_other_states_values_in_one_undo_step() {
    let (mut shell, _) = shell();
    let device = add(&mut shell);
    set_gain(&mut shell, device, 0.25);
    assert_eq!(gain(&mut shell, device), (0.25, 1.0, "A".to_owned()));
    assert_eq!(
        shell.dynamic_label(SWITCH).as_deref(),
        Some("Compare: Switch to B")
    );

    // The key runs the command on the selected device.
    shell.invoke(SWITCH, json!({})).unwrap();
    assert_eq!(gain(&mut shell, device), (1.0, 0.25, "B".to_owned()));
    assert_eq!(last_step(&shell), "Compare: Switch to B");
    assert_eq!(
        shell.dynamic_label(SWITCH).as_deref(),
        Some("Compare: Switch to A")
    );
    // A change goes to B only.
    set_gain(&mut shell, device, 0.5);
    assert_eq!(gain(&mut shell, device), (0.5, 0.25, "B".to_owned()));

    shell.invoke(SWITCH, json!({ "device": device })).unwrap();
    assert_eq!(gain(&mut shell, device), (0.25, 0.5, "A".to_owned()));
    assert_eq!(last_step(&shell), "Compare: Switch to A");

    // Undo brings back both states and which was active (R12).
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(gain(&mut shell, device), (0.5, 0.25, "B".to_owned()));
    shell.invoke("edit.redo", json!({})).unwrap();
    assert_eq!(gain(&mut shell, device), (0.25, 0.5, "A".to_owned()));
}

// Spec: device-compare R4, R12
#[test]
fn copy_overwrites_the_other_state_and_reverses_with_b_active() {
    let (mut shell, _) = shell();
    let device = add(&mut shell);
    set_gain(&mut shell, device, 0.25);
    assert_eq!(
        shell.dynamic_label(COPY).as_deref(),
        Some("Compare: Copy A to B")
    );
    shell.invoke(COPY, json!({})).unwrap();
    assert_eq!(last_step(&shell), "Compare: Copy A to B");
    // What plays is the same, and B now holds it too.
    assert_eq!(gain(&mut shell, device), (0.25, 0.25, "A".to_owned()));

    shell.invoke(SWITCH, json!({})).unwrap();
    set_gain(&mut shell, device, 0.75);
    assert_eq!(
        shell.dynamic_label(COPY).as_deref(),
        Some("Compare: Copy B to A")
    );
    shell.invoke(COPY, json!({})).unwrap();
    assert_eq!(last_step(&shell), "Compare: Copy B to A");
    assert_eq!(gain(&mut shell, device), (0.75, 0.75, "B".to_owned()));

    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(gain(&mut shell, device), (0.75, 0.25, "B".to_owned()));
}

// Spec: device-compare R5
#[test]
fn switching_keeps_the_preset_name() {
    let (mut shell, _) = shell();
    let device = add(&mut shell);
    shell
        .invoke("device.rename", json!({ "name": "Trim" }))
        .unwrap();
    let title = |shell: &Shell| shell.device_view_state().devices[0].title.clone();
    assert_eq!(title(&shell), "Trim");
    shell.invoke(SWITCH, json!({ "device": device })).unwrap();
    let frame = &shell.device_view_state().devices[0];
    assert_eq!((frame.title.as_str(), frame.b_active), ("Trim", true));
    assert_eq!(frame.title_text(), "Trim (B)");
    shell.invoke(SWITCH, json!({ "device": device })).unwrap();
    assert_eq!(shell.device_view_state().devices[0].title_text(), "Trim");
}

fn gain_target(device: DeviceId) -> Value {
    json!({ "type": "device", "device": device, "param": UTILITY_GAIN.0 })
}

fn envelope_on(shell: &Shell, track: TrackId, device: DeviceId) -> bool {
    let target = bl_model::AutomationTarget::Device {
        device,
        param: UTILITY_GAIN,
    };
    shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .automation_for(&target)
        .unwrap()
        .enabled
}

// Spec: device-compare R7
#[test]
fn a_switch_overrides_the_devices_envelopes_and_re_enable_returns_them() {
    let (mut shell, track) = shell();
    let device = add(&mut shell);
    shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": track, "target": gain_target(device), "at": 0, "value": 0.5 }),
        )
        .unwrap();
    assert!(envelope_on(&shell, track, device));

    // The two states hold the same values, and the switch still overrides.
    shell.invoke(SWITCH, json!({})).unwrap();
    assert!(!envelope_on(&shell, track, device));
    shell.invoke("automation.re_enable", json!({})).unwrap();
    assert!(envelope_on(&shell, track, device));
    // The next switch overrides it again.
    shell.invoke(SWITCH, json!({})).unwrap();
    assert!(!envelope_on(&shell, track, device));
    // One undo step: the envelope is back on with the first state.
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(envelope_on(&shell, track, device));
}

// Spec: device-compare R7
#[test]
fn copy_overrides_nothing() {
    let (mut shell, track) = shell();
    let device = add(&mut shell);
    shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": track, "target": gain_target(device), "at": 0, "value": 0.5 }),
        )
        .unwrap();
    // Move another parameter, so the states differ and the copy acts.
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": 3, "value": 1.0 }),
        )
        .unwrap();
    shell.invoke(COPY, json!({})).unwrap();
    assert_eq!(last_step(&shell), "Compare: Copy A to B");
    assert!(envelope_on(&shell, track, device));
}

// Spec: device-compare R8
#[test]
fn a_copy_takes_the_active_state_in_both() {
    let (mut shell, _) = shell();
    let device = add(&mut shell);
    set_gain(&mut shell, device, 0.25);
    shell.invoke(SWITCH, json!({})).unwrap();
    set_gain(&mut shell, device, 0.5);
    // B is active, holding 0.5; A holds 0.25.
    let duplicate: DeviceAdded =
        serde_json::from_value(shell.invoke("device.duplicate", json!({})).unwrap()).unwrap();
    assert_eq!(
        gain(&mut shell, duplicate.device),
        (0.5, 0.5, "A".to_owned()),
        "both states take the active one, with A active"
    );
    // The original keeps both states and its active one.
    assert_eq!(gain(&mut shell, device), (0.5, 0.25, "B".to_owned()));
}

// Spec: device-compare R9
#[test]
fn a_preset_saves_the_active_state_and_loads_into_both() {
    let library = std::env::temp_dir().join(format!("bl-compare-presets-{}", std::process::id()));
    std::fs::remove_dir_all(&library).ok();
    std::fs::create_dir_all(&library).unwrap();
    let (mut shell, _) = shell();
    shell.set_user_library(Some(library.clone()));
    let a = add(&mut shell);
    set_gain(&mut shell, a, 0.25);
    shell.invoke(SWITCH, json!({})).unwrap();
    set_gain(&mut shell, a, 0.5);
    // B is active: the preset holds B's values.
    let saved: Value = shell.invoke("device.save_preset", json!({})).unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();

    // It loads into both states of another device, with A active.
    let b = add(&mut shell);
    set_gain(&mut shell, b, 0.125);
    shell.invoke(SWITCH, json!({})).unwrap();
    shell
        .invoke("device.load_preset", json!({ "device": b, "path": path }))
        .unwrap();
    assert_eq!(gain(&mut shell, b), (0.5, 0.5, "A".to_owned()));
    std::fs::remove_dir_all(&library).ok();
}

// Spec: device-compare R6
#[test]
fn a_device_without_states_refuses_the_compare_commands() {
    let (mut shell, track) = shell();
    let plugin: DeviceId = shell.new_id();
    shell
        .run(
            "device.insert",
            json!({ "track": track, "index": 0, "device": {
                "id": plugin, "name": "Reverb", "enabled": true, "params": [],
                "kind": { "plugin": { "format": "clap", "plugin_id": "org.example.reverb" } },
            } }),
        )
        .unwrap();
    shell
        .invoke("device.select", json!({ "device": plugin }))
        .unwrap();
    for id in [SWITCH, COPY] {
        assert!(!shell.can_invoke(id, &json!({})), "{id} on a plug-in");
        let error = shell.run(id, json!({ "device": plugin })).unwrap_err();
        assert!(error.to_string().contains("A and B"), "{error}");
    }
    // With a native device selected they're on again.
    add(&mut shell);
    for id in [SWITCH, COPY] {
        assert!(shell.can_invoke(id, &json!({})), "{id} on a native device");
    }
}
