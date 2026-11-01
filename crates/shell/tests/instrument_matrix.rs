//! An instrument's modulation matrix in the shell (WP-8.4; the instrument
//! modulation spec, R8 to R11): `device.set_route` and `device.clear_route`,
//! their undo steps, their refusals, and the readback of the matrix. The
//! device is the framework's test instrument, with four free slots, one
//! chosen and one fixed.

use bl_devices::testing::{
    self, INSTRUMENT, INSTRUMENT_CHOSEN_AMOUNT, INSTRUMENT_FIXED_AMOUNT, INSTRUMENT_FREE_AMOUNTS,
};
use bl_model::{DeviceId, ModRoute, ParamId};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

fn shell() -> (Shell, DeviceId) {
    let mut shell = Shell::headless();
    shell.set_device_types(testing::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    let t = testing::instrument_type();
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    assert!(
        matches!(&t.kind, bl_model::DeviceKind::Native { device_type } if device_type == INSTRUMENT)
    );
    (shell, added.device)
}

fn routes(shell: &Shell, device: DeviceId) -> Vec<ModRoute> {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .routes
        .clone()
}

fn amount(shell: &Shell, device: DeviceId, param: ParamId) -> f64 {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
        .unwrap_or(0.0)
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn set(shell: &mut Shell, device: DeviceId, args: Value) -> Result<Value, String> {
    let mut args = args;
    args["device"] = json!(device);
    shell
        .invoke("device.set_route", args)
        .map_err(|e| e.to_string())
}

fn route(slot: u8, source: &str, destination: &str) -> ModRoute {
    ModRoute {
        slot,
        source: source.to_owned(),
        destination: destination.to_owned(),
    }
}

// Spec: instrument modulation R8, R9
#[test]
fn setting_and_clearing_a_route_are_one_undo_step_each_and_undo_restores_the_amount() {
    let (mut shell, device) = shell();
    let before = shell.editor().undo_history().count();
    set(
        &mut shell,
        device,
        json!({ "source": "Velocity", "destination": "Note Level", "amount": -40.0 }),
    )
    .unwrap();
    assert_eq!(routes(&shell, device), [route(0, "Velocity", "Note Level")]);
    assert_eq!(amount(&shell, device, INSTRUMENT_FREE_AMOUNTS[0]), -40.0);
    assert_eq!(shell.editor().undo_history().count(), before + 1);
    assert_eq!(last_step(&shell), "Change Modulation Route");

    // Setting the same pair again finds its slot and changes only the amount.
    set(
        &mut shell,
        device,
        json!({ "source": "velocity", "destination": "note level", "amount": 25.0 }),
    )
    .unwrap();
    assert_eq!(routes(&shell, device).len(), 1);
    assert_eq!(amount(&shell, device, INSTRUMENT_FREE_AMOUNTS[0]), 25.0);
    // A different pair takes the first empty free slot.
    set(
        &mut shell,
        device,
        json!({ "source": "Key", "destination": "Fine Tune", "amount": 10.0 }),
    )
    .unwrap();
    assert_eq!(
        routes(&shell, device),
        [
            route(0, "Velocity", "Note Level"),
            route(1, "Key", "Fine Tune")
        ]
    );

    // Clearing is one step that empties the slot and its amount; undo brings
    // back the source, the destination and the amount together.
    shell
        .invoke("device.clear_route", json!({ "device": device, "slot": 0 }))
        .unwrap();
    assert_eq!(routes(&shell, device), [route(1, "Key", "Fine Tune")]);
    assert_eq!(amount(&shell, device, INSTRUMENT_FREE_AMOUNTS[0]), 0.0);
    assert_eq!(last_step(&shell), "Clear Modulation Route");
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(routes(&shell, device).len(), 2);
    assert_eq!(amount(&shell, device, INSTRUMENT_FREE_AMOUNTS[0]), 25.0);
    // Clearing by the pair names.
    shell
        .invoke(
            "device.clear_route",
            json!({ "device": device, "source": "Key", "destination": "Fine Tune" }),
        )
        .unwrap();
    assert_eq!(routes(&shell, device), [route(0, "Velocity", "Note Level")]);
    // Undoing the set restores the route to its first amount.
    shell.invoke("edit.undo", json!({})).unwrap();
    shell.invoke("edit.undo", json!({})).unwrap();
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(amount(&shell, device, INSTRUMENT_FREE_AMOUNTS[0]), -40.0);
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(routes(&shell, device).is_empty());
    assert_eq!(amount(&shell, device, INSTRUMENT_FREE_AMOUNTS[0]), 0.0);
}

// Spec: instrument modulation R8, R9
#[test]
fn a_route_is_refused_for_unknown_names_taken_slots_and_a_full_matrix() {
    let (mut shell, device) = shell();
    let refused = |result: Result<Value, String>, text: &str| {
        let error = result.expect_err(text);
        assert!(error.contains(text), "{error}");
    };
    refused(
        set(
            &mut shell,
            device,
            json!({ "source": "Nope", "destination": "Note Level" }),
        ),
        "no modulation source",
    );
    refused(
        set(
            &mut shell,
            device,
            json!({ "source": "Velocity", "destination": "Nope" }),
        ),
        "no modulation destination",
    );
    // A pair the device doesn't allow together.
    refused(
        set(
            &mut shell,
            device,
            json!({ "source": "Key", "destination": "Master" }),
        ),
        "doesn't route",
    );
    // The chosen slot (4) and the fixed slot (5) take no route.
    for slot in [4, 5] {
        refused(
            set(
                &mut shell,
                device,
                json!({ "slot": slot, "source": "Velocity", "destination": "Note Level" }),
            ),
            "only free slots",
        );
    }
    refused(
        set(
            &mut shell,
            device,
            json!({ "slot": 9, "source": "Velocity", "destination": "Note Level" }),
        ),
        "no slot 9",
    );
    // Nothing was changed by a refusal.
    assert!(routes(&shell, device).is_empty());
    assert!(!last_step(&shell).contains("Route"));
    // Four routes fill the free slots; a fifth is refused.
    for (source, destination) in [
        ("Velocity", "Note Level"),
        ("Velocity", "Fine Tune"),
        ("Key", "Note Level"),
        ("Pressure", "Master"),
    ] {
        set(
            &mut shell,
            device,
            json!({ "source": source, "destination": destination }),
        )
        .unwrap();
    }
    refused(
        set(
            &mut shell,
            device,
            json!({ "source": "Mod Wheel", "destination": "Master" }),
        ),
        "every free slot",
    );
    // An empty slot, and a pair no slot routes, can't be cleared.
    shell
        .invoke("device.clear_route", json!({ "device": device, "slot": 3 }))
        .unwrap();
    let error = shell
        .invoke("device.clear_route", json!({ "device": device, "slot": 3 }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("holds no route"), "{error}");
    let error = shell
        .invoke(
            "device.clear_route",
            json!({ "device": device, "source": "Mod Wheel", "destination": "Master" }),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("no slot routes"), "{error}");
}

// Spec: instrument modulation R11
#[test]
fn the_matrix_reads_back_every_slot_with_its_amount() {
    let (mut shell, device) = shell();
    set(
        &mut shell,
        device,
        json!({ "source": "Velocity", "destination": "Note Level", "amount": 50.0 }),
    )
    .unwrap();
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": INSTRUMENT_CHOSEN_AMOUNT, "value": -20.0 }),
        )
        .unwrap();
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": testing::INSTRUMENT_CHOSEN_SOURCE, "value": 2.0 }),
        )
        .unwrap();
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": testing::INSTRUMENT_CHOSEN_DEST, "value": 3.0 }),
        )
        .unwrap();
    let matrix = shell
        .device_matrix(shell.editor().document().device(device).unwrap())
        .expect("the test instrument has a matrix");
    let state = serde_json::to_value(&matrix).unwrap();
    assert_eq!(state["sources"].as_array().unwrap().len(), 5);
    assert_eq!(state["destinations"][2]["kind"], "multiplicative");
    assert_eq!(state["destinations"][1]["rate"], "audio");
    let slots = state["slots"].as_array().unwrap();
    assert_eq!(slots.len(), 6);
    assert_eq!(slots[0]["kind"], "free");
    assert_eq!(slots[0]["source"], "Velocity");
    assert_eq!(slots[0]["destination"], "Note Level");
    assert_eq!(slots[0]["amount"], 50.0);
    assert_eq!(slots[0]["amount_param"], json!(INSTRUMENT_FREE_AMOUNTS[0]));
    assert_eq!(slots[1]["source"], Value::Null);
    // The chosen slot reads its choosers: the second source after Off, the
    // third destination.
    assert_eq!(slots[4]["kind"], "chosen");
    assert_eq!(slots[4]["source"], "Key");
    assert_eq!(slots[4]["destination"], "Master");
    assert_eq!(slots[4]["amount"], -20.0);
    // The fixed slot is Pressure into Fine Tune.
    assert_eq!(slots[5]["kind"], "fixed");
    assert_eq!(slots[5]["source"], "Pressure");
    assert_eq!(slots[5]["destination"], "Fine Tune");
    assert_eq!(slots[5]["amount_param"], json!(INSTRUMENT_FIXED_AMOUNT));
}

// Spec: instrument modulation R2
#[test]
fn a_device_without_a_matrix_refuses_the_route_commands() {
    let mut shell = Shell::headless();
    shell.set_device_types(testing::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    let t = testing::utility_type();
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    let error = set(
        &mut shell,
        added.device,
        json!({ "source": "Velocity", "destination": "Note Level" }),
    )
    .unwrap_err();
    assert!(error.contains("no modulation matrix"), "{error}");
}

// Spec: instrument modulation R10
#[test]
fn a_preset_brings_its_routes_and_one_without_leaves_none() {
    let (mut shell, device) = shell();
    set(
        &mut shell,
        device,
        json!({ "source": "Velocity", "destination": "Note Level", "amount": 30.0 }),
    )
    .unwrap();
    let with = bl_project::preset::Preset::of(shell.editor().document().device(device).unwrap());
    let mut bare = shell.editor().document().device(device).unwrap().clone();
    bare.routes.clear();
    let without = bl_project::preset::Preset::of(&bare);
    let folder = std::env::temp_dir().join(format!("bl-matrix-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let load = |shell: &mut Shell, preset: &bl_project::preset::Preset, name: &str| {
        let path = folder.join(format!("{name}.blpreset"));
        std::fs::write(&path, preset.encode()).unwrap();
        shell
            .invoke(
                "device.load_preset",
                json!({ "device": device, "path": path.to_string_lossy() }),
            )
            .unwrap();
    };
    // A preset with no routes empties the device's.
    load(&mut shell, &without, "without");
    assert!(routes(&shell, device).is_empty());
    // And one with routes brings them back.
    load(&mut shell, &with, "with");
    assert_eq!(routes(&shell, device), [route(0, "Velocity", "Note Level")]);
    assert_eq!(amount(&shell, device, INSTRUMENT_FREE_AMOUNTS[0]), 30.0);
    std::fs::remove_file(folder.join("without.blpreset")).unwrap();
    std::fs::remove_file(folder.join("with.blpreset")).unwrap();
    std::fs::remove_dir(&folder).unwrap();
}
