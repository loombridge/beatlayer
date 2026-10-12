//! The device framework in the shell (WP-2.5; the device framework spec,
//! R7, R13, and R14): a described device's frame carries its parameters
//! and layout for the generic UI, `device.parameters` reports them, and a
//! preset loads clamped to them. The devices are the framework's test
//! devices.

use bl_devices::testing::{self, UTILITY_GAIN, UTILITY_POLARITY};
use bl_model::param::db_to_gain;
use bl_model::{ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, DeviceParameters, DeviceViewState, frame_width};
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

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

fn add(shell: &mut Shell, device_type: &bl_engine::DeviceType) -> DeviceAdded {
    let device =
        json!({ "kind": device_type.kind, "name": device_type.name, "role": device_type.role });
    serde_json::from_value(
        shell
            .invoke("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

fn state(shell: &Shell) -> DeviceViewState {
    shell.device_view_state()
}

// Spec: device-framework R13
#[test]
fn a_described_devices_frame_carries_its_panel_and_docked_width() {
    let (mut shell, _) = shell();
    let utility = add(&mut shell, &testing::utility_type());
    let scope = add(&mut shell, &testing::scope_type());
    shell
        .invoke(
            "device.set_param",
            json!({ "device": utility.device, "param": UTILITY_GAIN, "value": 0.25 }),
        )
        .unwrap();
    let frames = state(&shell).devices;
    let panel = frames[0].panel.as_ref().unwrap();
    assert_eq!(panel.description, testing::utility_type().description);
    assert_eq!(
        panel.values,
        [0.25, 0.0, 0.0, 0.0],
        "set values, then defaults"
    );
    let scope_type = testing::scope_type();
    assert_eq!(
        frame_width(&frames[1]),
        scope_type.description.layout.docked_size().width,
        "a frame is its layout's docked width"
    );
    assert_eq!(scope.device, frames[1].id);
    // A device whose type isn't described keeps the plain frame.
    let gain = bl_engine::testing::gain_type();
    add(&mut shell, &gain);
    let frames = state(&shell).devices;
    assert!(frames[2].panel.is_none());
    assert_eq!(frame_width(&frames[2]), 120.0);
}

// Spec: device-framework R14
#[test]
fn agents_read_every_parameter_described() {
    let (mut shell, _) = shell();
    let drive = add(&mut shell, &testing::drive_type());
    let reply = shell
        .run("device.parameters", json!({ "device": drive.device }))
        .unwrap();
    let parameters: DeviceParameters = serde_json::from_value(reply.clone()).unwrap();
    let names: Vec<&str> = parameters
        .params
        .iter()
        .map(|p| p.descriptor.name.as_str())
        .collect();
    assert_eq!(names, ["Drive", "Output", "Oversampling"]);
    assert!(parameters.params.iter().all(|p| p.descriptor.automatable));
    assert_eq!(parameters.params[1].text, "0.00 dB");
    assert_eq!(parameters.params[2].text, "4x");
    // Flattened, as agents read it.
    assert_eq!(reply["params"][0]["kind"]["gain"]["max_db"], json!(36.0));
    assert_eq!(reply["params"][0]["value"], json!(1.0));
    // With no device named, the selected one; a device that isn't
    // described has none.
    let gain = bl_engine::testing::gain_type();
    add(&mut shell, &gain);
    let none: DeviceParameters =
        serde_json::from_value(shell.invoke("device.parameters", json!({})).unwrap()).unwrap();
    assert!(none.params.is_empty());
}

// Spec: device-framework R7
#[test]
fn a_preset_loads_clamped_to_the_devices_parameters() {
    let library = std::env::temp_dir().join(format!("bl-framework-presets-{}", std::process::id()));
    std::fs::remove_dir_all(&library).ok();
    std::fs::create_dir_all(&library).unwrap();
    let (mut shell, _) = shell();
    shell.set_user_library(Some(library.clone()));
    let a = add(&mut shell, &testing::utility_type());
    for (param, value) in [(UTILITY_GAIN, 0.5), (UTILITY_POLARITY, 1.0)] {
        shell
            .invoke(
                "device.set_param",
                json!({ "device": a.device, "param": param, "value": value }),
            )
            .unwrap();
    }
    let saved: Value = shell.invoke("device.save_preset", json!({})).unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();

    // Round trip: a fresh device takes the saved values.
    let b = add(&mut shell, &testing::utility_type());
    shell
        .invoke(
            "device.load_preset",
            json!({ "device": b.device, "path": path }),
        )
        .unwrap();
    let params = shell
        .editor()
        .document()
        .device(b.device)
        .unwrap()
        .params
        .clone();
    assert_eq!(params.get(&UTILITY_GAIN), Some(&0.5));
    assert_eq!(params.get(&UTILITY_POLARITY), Some(&1.0));

    // A preset with values out of range, and one for a parameter the
    // device doesn't have, loads clamped and without it.
    let text = std::fs::read_to_string(&path).unwrap();
    let mut file: Value = serde_json::from_str(&text).unwrap();
    file["params"] = json!([[0, 100.0], [3, 7.0], [42, 1.0]]);
    let odd = library.join("Odd.blpreset");
    std::fs::write(&odd, file.to_string()).unwrap();
    shell
        .invoke(
            "device.load_preset",
            json!({ "device": b.device, "path": odd }),
        )
        .unwrap();
    let params = shell
        .editor()
        .document()
        .device(b.device)
        .unwrap()
        .params
        .clone();
    assert!((params[&UTILITY_GAIN] - db_to_gain(12.0)).abs() < 1e-12);
    assert_eq!(params.get(&UTILITY_POLARITY), Some(&1.0));
    assert_eq!(params.get(&ParamId(42)), None);
    std::fs::remove_dir_all(&library).ok();
}
