//! The Macro Controls in the Device View (WP-5.4; the macros spec, R3, R9,
//! R11 and R20), headless: a macro is a knob with its name and value, and
//! the mappings list and Map mode run the commands an agent runs.

mod common;

use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use common::{Harness, Window};
use egui::accesskit::Role;
use serde_json::{Value, json};

fn add(h: &mut Harness, args: Value) -> DeviceId {
    let added: DeviceAdded =
        serde_json::from_value(h.shell.run("device.add", args).unwrap()).unwrap();
    h.settle();
    added.device
}

/// An audio effect rack on the shown track with a chain holding a Utility.
fn set(h: &mut Harness) -> (DeviceId, DeviceId) {
    let audio: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio[2]] }))
        .unwrap();
    h.settle();
    let rack = add(
        h,
        json!({
            "device": {
                "kind": { "rack": { "kind": "audio_effect" } },
                "name": "Rack",
                "role": "audio_effect",
            },
        }),
    );
    let chain: bl_model::ChainId = h.shell.new_id();
    h.shell
        .invoke(
            "chain.add",
            json!({ "rack": rack, "index": 0, "chain": { "id": chain, "name": "Range", "devices": [] } }),
        )
        .unwrap();
    let utility = add(
        h,
        json!({
            "chain": chain,
            "device": {
                "kind": { "native": { "device_type": "beatlayer.utility" } },
                "name": "Utility",
                "role": "audio_effect",
            },
        }),
    );
    (rack, utility)
}

// Spec: macros R20
#[test]
fn a_macro_is_a_knob_with_its_name_and_value() {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let (rack, utility) = set(&mut h);
    h.shell
        .invoke(
            "rack.map_macro",
            json!({
                "rack": rack,
                "macro_number": 1,
                "target": { "type": "device", "device": utility, "param": 6 },
            }),
        )
        .unwrap();
    h.shell
        .invoke(
            "rack.rename_macro",
            json!({ "rack": rack, "macro_number": 1, "name": "Brightness" }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let node = Harness::node(&nodes, "Brightness");
    assert_eq!(node.role, Role::Slider);
    assert_eq!(node.value, "100 %");
}

// Spec: macros R23
#[test]
fn a_radio_button_macro_draws_its_steps_and_a_click_selects_one() {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let (rack, utility) = set(&mut h);
    h.shell
        .invoke(
            "rack.map_macro",
            json!({
                "rack": rack,
                "macro_number": 1,
                "target": { "type": "device", "device": utility, "param": 6 },
            }),
        )
        .unwrap();
    h.shell
        .invoke(
            "rack.set_macro_style",
            json!({ "rack": rack, "macro_number": 1, "style": "radio_buttons", "steps": 4 }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    // The buttons read as a radio group named for the macro.
    let group = nodes
        .iter()
        .find(|node| node.role == Role::RadioGroup && node.label == "Width")
        .unwrap_or_else(|| panic!("a radio group named for the macro: {nodes:?}"));
    assert_eq!(group.label, "Width");
    let mut buttons: Vec<&common::Node> = nodes
        .iter()
        .filter(|node| node.role == Role::RadioButton)
        .collect();
    // The buttons are in a column, top to bottom: the steps in order.
    buttons.sort_by(|a, b| {
        let key = |node: &common::Node| node.bounds.map_or(0.0, |b| b.y0);
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    assert_eq!(buttons.len(), 4, "one button per step: {buttons:?}");
    for button in &buttons {
        assert!(
            Harness::descends_from(&nodes, button.id, group.id),
            "the button is in the group"
        );
    }
    // The current step's button is lit.
    assert_eq!(
        buttons.iter().filter(|b| b.selected == Some(true)).count(),
        1
    );
    // A click on a button moves the macro to that step.
    let button = buttons[2];
    let bounds = button.bounds.expect("the button's bounds");
    let at = egui::pos2(
        (bounds.x0 + bounds.x1) as f32 / 2.0,
        (bounds.y0 + bounds.y1) as f32 / 2.0,
    );
    h.click(at);
    let state: Value = h
        .shell
        .invoke("rack.macros", json!({ "rack": rack }))
        .unwrap();
    let value = state["macros"][0]["value"].as_f64().expect("a value");
    assert!((value - 2.0 / 3.0).abs() < 1e-9, "the third step: {value}");
}

// Spec: macros R3, R9
#[test]
fn map_mode_and_the_mappings_list_run_their_commands() {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let (rack, utility) = set(&mut h);
    h.shell
        .invoke(
            "rack.map_macro",
            json!({
                "rack": rack,
                "macro_number": 1,
                "target": { "type": "device", "device": utility, "param": 6 },
            }),
        )
        .unwrap();
    h.shell
        .invoke("rack.map_mode", json!({ "rack": rack, "on": true }))
        .unwrap();
    assert_eq!(h.shell.devices().map_mode(), Some(rack));
    h.shell
        .invoke(
            "rack.set_mapping_range",
            json!({
                "rack": rack,
                "target": { "type": "device", "device": utility, "param": 6 },
                "min": 0.25,
                "max": 0.75,
            }),
        )
        .unwrap();
    let state: Value = h
        .shell
        .invoke("rack.macros", json!({ "rack": rack }))
        .unwrap();
    let mapping = &state["macros"][0]["mappings"][0];
    assert_eq!(mapping["min_position"], 0.25);
    assert_eq!(mapping["max_position"], 0.75);
    // Turning Map mode off puts the browser back.
    h.shell
        .invoke("rack.map_mode", json!({ "rack": rack, "on": false }))
        .unwrap();
    assert_eq!(h.shell.devices().map_mode(), None);
}
