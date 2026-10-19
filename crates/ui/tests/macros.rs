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
