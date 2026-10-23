//! The Macro Controls and Macro Variations in the Device View (WP-5.4 and
//! WP-5.5; the macros spec, R3, R9, R11, R20, R25 and R29), headless: a macro
//! is a knob with its name and value, the mappings list and Map mode run the
//! commands an agent runs, and the variations view stores, launches and edits
//! them.

use crate::common;
use bl_model::{DeviceId, RackView, TrackId};
use bl_shell::Area;
use bl_shell::device_commands::DeviceAdded;
use bl_ui::rack_view::{self, RackLayout};
use common::{Harness, Window, command};
use egui::accesskit::Role;
use egui::{Key, Modifiers, Pos2, Rect, pos2};
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

/// The Device View's rect, which holds the racks.
fn view(h: &mut Harness) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn layout_of(h: &mut Harness, rack: DeviceId) -> RackLayout {
    let rect = view(h);
    rack_view::layouts(&h.shell, rect)
        .into_iter()
        .find(|(id, _)| *id == rack)
        .unwrap_or_else(|| panic!("no layout for {rack}"))
        .1
}

fn selector_at(h: &mut Harness, rack: DeviceId, view: RackView) -> Pos2 {
    layout_of(h, rack)
        .selectors
        .iter()
        .find(|(v, _)| *v == view)
        .unwrap_or_else(|| panic!("no {view:?} selector"))
        .1
        .center()
}

fn variations_of(h: &mut Harness, rack: DeviceId) -> Vec<Value> {
    let state: Value = h
        .shell
        .invoke("rack.macros", json!({ "rack": rack }))
        .unwrap();
    state["variations"].as_array().cloned().unwrap_or_default()
}

// Spec: macros R25
#[test]
fn the_variations_switch_shows_the_variations_view() {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let (rack, _) = set(&mut h);
    // A new rack hides the Macro Variations view.
    assert!(!h.shell.devices().views_of(rack).variations);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "Show/Hide Macro Variations").value,
        "hidden"
    );
    // Clicking the selector shows the view, which draws its New button.
    let at = selector_at(&mut h, rack, RackView::Variations);
    h.click(at);
    assert!(h.shell.devices().views_of(rack).variations);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "Show/Hide Macro Variations").value,
        "shown"
    );
    assert_eq!(Harness::node(&nodes, "Macro Variations").role, Role::Group);
    assert_eq!(
        Harness::node(&nodes, "New Macro Variation").role,
        Role::Button
    );
    // Storing a variation draws its row with Overwrite and Launch.
    h.shell
        .invoke("rack.store_variation", json!({ "rack": rack }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Variation 1").role, Role::Button);
    assert_eq!(
        Harness::node(&nodes, "Overwrite Macro Variation").role,
        Role::Button
    );
    assert_eq!(
        Harness::node(&nodes, "Launch Macro Variation").role,
        Role::Button
    );
    assert_eq!(variations_of(&mut h, rack).len(), 1);
}

// Spec: macros R29
#[test]
fn variation_rows_take_the_edit_keys() {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let (rack, _) = set(&mut h);
    h.shell
        .invoke(
            "rack.show_variations",
            json!({ "rack": rack, "shown": true }),
        )
        .unwrap();
    h.shell
        .invoke("rack.store_variation", json!({ "rack": rack }))
        .unwrap();
    h.settle();
    // Clicking a row selects its variation.
    let row = layout_of(&mut h, rack).variations.unwrap().rows[0].row;
    h.click(row.center());
    assert_eq!(h.shell.devices().variation_selected(), Some((rack, 0)));
    h.shell.set_focused_area(Area::Detail);

    // Cmd+D duplicates the selected variation, not the device.
    h.key(Key::D, command());
    assert_eq!(variations_of(&mut h, rack).len(), 2);
    assert_eq!(variations_of(&mut h, rack)[1]["name"], "Variation 1");

    // Delete removes the selected one.
    h.key(Key::Delete, Modifiers::NONE);
    let left = variations_of(&mut h, rack);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["name"], "Variation 1");

    // Cmd+R renames it: the UI starts typing in its row.
    h.key(Key::R, command());
    h.settle();
    assert!(h.main.is_typing(), "Rename types the variation's name");
}
