//! MIDI Map Mode and Key Map Mode in the window, headless (WP-7.3; the map
//! modes spec, R1 to R10): the switches, the overlays, selecting then
//! learning, the Mapping Browser, and the Status Bar's mode chooser.

use crate::common;
use bl_midi_io::{Direction, TestBackend};
use bl_model::TrackId;
use bl_ui::keymap::Platform;
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use bl_ui::menus::{ItemState, item_state};
use common::{Harness, Node, Window, command};
use egui::accesskit::{Action, ActionRequest, Role, TreeId};
use egui::{Event, Id, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::{Value, json};

fn rect_of(node: &Node) -> Rect {
    let b = node.bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

/// A harness on the Session View, with a MIDI port "Pads" whose Remote switch
/// is on.
fn session() -> (Harness, TestBackend) {
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Pads");
    h.shell.use_midi_backend(Box::new(test.clone()));
    h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Pads", "remote": true, "track": false }),
        )
        .unwrap();
    h.settle();
    (h, test)
}

fn tracks(h: &Harness) -> Vec<TrackId> {
    h.shell.editor().document().top_level().collect()
}

fn tree(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

/// The node labelled `label` inside the column or region labelled `within`.
fn inside<'a>(nodes: &'a [Node], within: &str, label: &str) -> &'a Node {
    let region = Harness::node(nodes, within);
    nodes
        .iter()
        .find(|n| n.label == label && Harness::descends_from(nodes, n.id, region.id))
        .unwrap_or_else(|| panic!("{within}: no {label}"))
}

/// The fader of the track named `column`.
fn fader(h: &mut Harness, column: &str) -> Rect {
    let nodes = tree(h);
    rect_of(inside(&nodes, column, "Track Volume"))
}

/// The nodes under `node`, depth first in the order the tree lists them.
fn in_order<'a>(nodes: &'a [Node], node: &Node) -> Vec<&'a Node> {
    let mut found = Vec::new();
    for id in &node.children {
        if let Some(child) = nodes.iter().find(|n| n.id == *id) {
            found.push(child);
            found.extend(in_order(nodes, child));
        }
    }
    found
}

fn volume(h: &Harness, track: TrackId) -> f64 {
    h.shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .mixer()
        .volume
}

fn state(h: &mut Harness) -> Value {
    h.shell.run("remote.state", json!({})).unwrap()
}

fn mappings(h: &mut Harness) -> Vec<Value> {
    h.shell.run("remote.mappings", json!({})).unwrap()["mappings"]
        .as_array()
        .unwrap()
        .clone()
}

fn volume_of(track: TrackId) -> Value {
    json!({ "type": "mixer", "track": track, "param": "volume" })
}

/// Sends a message from the port and lets the window take it.
fn send(h: &mut Harness, test: &TestBackend, bytes: &[u8]) {
    assert!(test.send("Pads", h.shell.midi().now(), bytes));
    h.shell.poll();
    h.settle();
}

fn switch(h: &mut Harness, label: &str) -> Option<bool> {
    let nodes = tree(h);
    Harness::node(&nodes, label).toggled
}

/// The Options menu's item of this row, as it reads and works now.
fn options_item(h: &Harness, row: &str) -> ItemState {
    let bar = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap_or_else(|| panic!("{row} is in the menus"));
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

fn click_at(h: &mut Harness, at: Pos2) {
    h.click(at);
    h.settle();
}

// Spec: map-modes R1, R2. Cmd-M, the MIDI switch and the Options item each
// turn MIDI Map Mode on and off and tick; the same for Key Map Mode; one mode
// is on at a time.
#[test]
fn the_switches_the_keys_and_the_options_items_turn_the_modes_on_and_off() {
    let (mut h, _test) = session();
    let layout = ShellLayout::compute(REFERENCE, true, true);
    let (midi, key) = ("MIDI Map Mode", "Key Map Mode");
    assert_eq!(switch(&mut h, midi), Some(false));
    assert_eq!(switch(&mut h, key), Some(false));
    assert_eq!(options_item(&h, "M-OPT-002").checked, Some(false));

    // The switch.
    click_at(&mut h, layout.rect(Element::MidiMap).center());
    assert_eq!(switch(&mut h, midi), Some(true));
    assert_eq!(options_item(&h, "M-OPT-002").checked, Some(true));
    assert_eq!(options_item(&h, "M-OPT-003").checked, Some(false));
    click_at(&mut h, layout.rect(Element::MidiMap).center());
    assert_eq!(switch(&mut h, midi), Some(false));

    // Cmd-M, and the Options item.
    h.key(Key::M, command());
    assert_eq!(state(&mut h)["mode"], "midi");
    h.key(Key::M, command());
    assert_eq!(state(&mut h)["mode"], "off");
    let item = options_item(&h, "M-OPT-002");
    let invocation = item.invocation.expect("Edit MIDI Map runs a command");
    h.shell
        .run(&invocation.id, invocation.args.clone())
        .unwrap();
    assert_eq!(switch(&mut h, midi), Some(true));
    assert_eq!(options_item(&h, "M-OPT-002").checked, Some(true));
    h.shell.run(&invocation.id, invocation.args).unwrap();
    assert_eq!(switch(&mut h, midi), Some(false));

    // Key Map Mode, the same three ways, and turning one on turns the other
    // off.
    click_at(&mut h, layout.rect(Element::KeyMap).center());
    assert_eq!(switch(&mut h, key), Some(true));
    assert_eq!(options_item(&h, "M-OPT-003").checked, Some(true));
    h.key(Key::M, command());
    assert_eq!(switch(&mut h, midi), Some(true));
    assert_eq!(switch(&mut h, key), Some(false), "MIDI turned Key off");
    h.key(Key::K, command());
    assert_eq!(switch(&mut h, key), Some(true));
    assert_eq!(switch(&mut h, midi), Some(false), "Key turned MIDI off");
    h.key(Key::K, command());
    assert_eq!(state(&mut h)["mode"], "off");
    let item = options_item(&h, "M-OPT-003");
    let invocation = item.invocation.expect("Edit Key Map runs a command");
    h.shell.run(&invocation.id, invocation.args).unwrap();
    assert_eq!(state(&mut h)["mode"], "key");
}

// Spec: map-modes R10 (accessibility R1). The switches are toggle buttons.
#[test]
fn the_switches_are_toggle_buttons_in_the_accessibility_tree() {
    let (mut h, _test) = session();
    let nodes = tree(&mut h);
    for label in ["MIDI Map Mode", "Key Map Mode"] {
        let node = Harness::node(&nodes, label);
        assert_eq!(node.toggled, Some(false), "{label}");
        assert!(!node.disabled, "{label} is live");
    }
}

// Spec: map-modes R2. A map mode turns a rack's Map mode off, and a rack's
// Map mode turns a map mode off.
#[test]
fn a_map_mode_and_a_racks_map_mode_exclude_each_other() {
    let (mut h, _test) = session();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks(&h)[2]] }),
        )
        .unwrap();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "audio_effect" } },
                "name": "Rack",
                "role": "audio_effect",
            } }),
        )
        .unwrap();
    let rack = added["device"].clone();
    h.settle();
    h.shell
        .run("rack.map_mode", json!({ "rack": rack, "on": true }))
        .unwrap();
    assert!(h.shell.devices().map_mode().is_some());
    h.key(Key::M, command());
    assert_eq!(state(&mut h)["mode"], "midi");
    assert!(h.shell.devices().map_mode().is_none(), "MIDI turned it off");
    h.shell
        .run("rack.map_mode", json!({ "rack": rack, "on": true }))
        .unwrap();
    assert_eq!(
        state(&mut h)["mode"],
        "off",
        "the rack's Map mode turned MIDI off"
    );
    assert!(h.shell.devices().map_mode().is_some());
}

// Spec: map-modes R3, R4, R5, R8, R10. A click on a tinted fader selects it
// without moving it; a controller then maps it, the Mapping Browser lists it
// and the overlay reads its source.
#[test]
fn a_click_selects_a_tinted_fader_and_a_controller_maps_it() {
    let (mut h, test) = session();
    let t = tracks(&h);
    let at = fader(&mut h, "3 Audio");
    let before = volume(&h, t[2]);
    h.key(Key::M, command());

    // The overlay tints the fader in the MIDI colour.
    let output = h.settle();
    let tint = h.theme.colors.midi_map.0.gamma_multiply(0.3);
    assert!(
        common::painted_rects(&output)
            .iter()
            .any(|(rect, fill)| rect.contains(at.center()) && *fill == tint),
        "the fader is tinted"
    );
    assert_eq!(
        Harness::node(&Harness::nodes(&output), "3 Audio").label,
        "3 Audio"
    );

    // A click selects it instead of moving it.
    click_at(&mut h, pos2(at.center().x, at.bottom() - 2.0));
    assert_eq!(state(&mut h)["selected"]["type"], "mixer");
    assert_eq!(volume(&h, t[2]), before, "a click doesn't move it");
    let nodes = tree(&mut h);
    let node = inside(&nodes, "3 Audio", "Track Volume");
    assert!(
        node.description.contains("MIDI Map Mode: not mapped"),
        "{}",
        node.description
    );

    // CC 30 maps it; the Mapping Browser lists it and the overlay reads it.
    send(&mut h, &test, &[0xB0, 30, 64]);
    let listed = mappings(&mut h);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["target"], volume_of(t[2]));
    assert_eq!(volume(&h, t[2]), before, "no mapping acts in a map mode");
    let nodes = tree(&mut h);
    let node = inside(&nodes, "3 Audio", "Track Volume");
    assert!(
        node.description.contains("mapped to 1 · CC 30"),
        "{}",
        node.description
    );
    let table = Harness::node(&nodes, "Mapping Browser");
    assert_eq!(table.role, Role::Table);
    let headers: Vec<&str> = in_order(&nodes, table)
        .into_iter()
        .filter(|n| n.role == Role::ColumnHeader)
        .map(|n| n.label.as_str())
        .collect();
    assert_eq!(headers, ["Source", "Path", "Name", "Min", "Max"]);
    let row = nodes
        .iter()
        .find(|n| n.role == Role::Row && Harness::descends_from(&nodes, n.id, table.id))
        .expect("a row");
    assert!(row.label.starts_with("1 · CC 30:"), "{}", row.label);
    assert_eq!(row.selected, Some(true), "the mapped control is selected");
}

// Spec: map-modes R6. Holding C3 and sending B2 and D3 on a selected slot
// maps a range with root C3.
#[test]
fn a_slot_takes_a_range_around_its_root() {
    let (mut h, test) = session();
    let t = tracks(&h);
    h.key(Key::M, command());
    let nodes = tree(&mut h);
    let slot = nodes
        .iter()
        .find(|n| n.role == Role::ListItem && n.label.starts_with("3 Audio: "))
        .expect("a slot of the audio track");
    click_at(&mut h, rect_of(slot).center());
    assert_eq!(state(&mut h)["selected"]["type"], "slot");
    assert_eq!(state(&mut h)["selected"]["track"], json!(t[2]));

    send(&mut h, &test, &[0x90, 60, 100]);
    send(&mut h, &test, &[0x90, 59, 100]);
    send(&mut h, &test, &[0x90, 62, 100]);
    let listed = mappings(&mut h);
    assert_eq!(
        listed[0]["source"],
        json!({ "type": "note_range", "channel": 1, "low": 59, "high": 62, "root": 60 })
    );
}

// Spec: map-modes R4, R7, R9. In Key Map Mode the next key maps, Esc maps
// nothing and clears the selection, and Delete unmaps; a continuous control
// can't be selected and a click on it does nothing.
#[test]
fn key_map_mode_maps_the_next_key_and_esc_and_delete_work_on_it() {
    let (mut h, _test) = session();
    let t = tracks(&h);
    let solo = json!({ "type": "solo", "track": t[2] });
    h.key(Key::K, command());

    // The overlays are red, and a continuous control isn't tinted.
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let switch = rect_of(inside(&nodes, "3 Audio", "Solo"));
    let tint = h.theme.colors.key_map.0.gamma_multiply(0.3);
    let painted = common::painted_rects(&output);
    assert!(
        painted
            .iter()
            .any(|(rect, fill)| rect.contains(switch.center()) && *fill == tint),
        "Solo is tinted red"
    );
    let slider = rect_of(inside(&nodes, "3 Audio", "Track Volume"));
    assert!(
        !painted
            .iter()
            .any(|(rect, fill)| rect.contains(slider.center()) && *fill == tint),
        "the fader keeps its look"
    );

    // A click on a continuous control does nothing.
    let at = fader(&mut h, "3 Audio");
    let before = volume(&h, t[2]);
    click_at(&mut h, pos2(at.center().x, at.bottom() - 2.0));
    assert!(state(&mut h)["selected"].is_null());
    assert_eq!(volume(&h, t[2]), before);

    // Esc with a control selected clears it and maps nothing.
    h.shell
        .run("remote.select", json!({ "target": solo }))
        .unwrap();
    h.key(Key::Escape, Modifiers::NONE);
    assert!(state(&mut h)["selected"].is_null());
    assert!(mappings(&mut h).is_empty());

    // The next key maps, whether or not the computer MIDI keyboard is on.
    h.shell
        .run("midi.toggle_computer_keyboard", json!({ "on": true }))
        .unwrap();
    h.shell
        .run("remote.select", json!({ "target": solo }))
        .unwrap();
    h.key(Key::A, Modifiers::NONE);
    let listed = mappings(&mut h);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["source"], json!({ "type": "key", "key": "A" }));
    assert_eq!(listed[0]["target"], solo);
    let nodes = tree(&mut h);
    let node = inside(&nodes, "3 Audio", "Solo");
    assert!(
        node.description.contains("mapped to A"),
        "{}",
        node.description
    );

    // Delete unmaps it, as one undo step.
    let steps = h.shell.editor().undo_history().count();
    h.key(Key::Delete, Modifiers::NONE);
    assert!(mappings(&mut h).is_empty());
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(mappings(&mut h).len(), 1, "undo brings it back");
}

// Spec: map-modes R8, R9. Invert Range and Delete are one undo step each; a
// row selects its control.
#[test]
fn the_mapping_browser_inverts_a_range_and_deletes_a_mapping() {
    let (mut h, _test) = session();
    let t = tracks(&h);
    h.shell
        .run(
            "remote.map",
            json!({ "source": { "type": "cc", "channel": 1, "controller": 20 },
                    "target": volume_of(t[2]) }),
        )
        .unwrap();
    h.key(Key::M, command());
    let nodes = tree(&mut h);
    let row = nodes
        .iter()
        .find(|n| n.role == Role::Row && n.label.starts_with("1 · CC 20:"))
        .expect("the mapping's row");
    let at = rect_of(row).center();

    // A click on the row selects its control.
    click_at(&mut h, pos2(rect_of(row).left() + 8.0, at.y));
    assert_eq!(state(&mut h)["selected"], volume_of(t[2]));

    // Invert Range, from the row's menu.
    let before = mappings(&mut h)[0].clone();
    let steps = h.shell.editor().undo_history().count();
    h.frame(vec![Event::PointerMoved(pos2(
        rect_of(row).left() + 8.0,
        at.y,
    ))]);
    h.frame(vec![
        Event::PointerButton {
            pos: pos2(rect_of(row).left() + 8.0, at.y),
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        Event::PointerButton {
            pos: pos2(rect_of(row).left() + 8.0, at.y),
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]);
    let nodes = tree(&mut h);
    let item = Harness::node(&nodes, "Invert Range");
    click_at(&mut h, rect_of(item).center());
    let after = mappings(&mut h)[0].clone();
    assert_eq!(after["min"], before["max"]);
    assert_eq!(after["max"], before["min"]);
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);

    // Delete deletes the selected row's mapping.
    h.key(Key::Delete, Modifiers::NONE);
    assert!(mappings(&mut h).is_empty());
    assert_eq!(h.shell.editor().undo_history().count(), steps + 2);
}

// Spec: map-modes R8. The Mapping Browser takes the browser's place and the
// browser comes back when the mode ends; a hidden browser stays hidden until
// Cmd-Option-B.
#[test]
fn the_mapping_browser_replaces_the_browser_and_hides_with_it() {
    let (mut h, _test) = session();
    let has = |nodes: &[Node], label: &str| nodes.iter().any(|n| n.label == label);
    let browser = |nodes: &[Node]| {
        nodes
            .iter()
            .any(|n| n.label == "Browser" || n.label.starts_with("Browser:"))
    };
    let shown = tree(&mut h);
    assert!(browser(&shown) && !has(&shown, "Mapping Browser"));

    h.key(Key::M, command());
    let mapping = tree(&mut h);
    assert!(has(&mapping, "Mapping Browser"));
    assert!(!browser(&mapping), "it took the browser's place");
    h.key(Key::M, command());
    let back = tree(&mut h);
    assert!(browser(&back) && !has(&back, "Mapping Browser"));

    // Hidden, the browser stays hidden in the mode, until Cmd-Option-B.
    h.key(Key::B, command() | Modifiers::ALT);
    assert!(!browser(&tree(&mut h)));
    h.key(Key::M, command());
    assert!(!has(&tree(&mut h), "Mapping Browser"));
    h.key(Key::B, command() | Modifiers::ALT);
    assert!(has(&tree(&mut h), "Mapping Browser"));
    h.key(Key::M, command());
    let after = tree(&mut h);
    assert!(browser(&after), "leaving the mode puts the browser back");
}

// Spec: map-modes R10. After a learn of repeated 127s the Status Bar's
// chooser shows the guess, the message area says it is a guess, and choosing
// another mode changes the mapping.
#[test]
fn the_status_bars_chooser_shows_the_guess_and_changes_the_mode() {
    let (mut h, test) = session();
    let t = tracks(&h);
    h.key(Key::M, command());
    h.shell
        .run("remote.select", json!({ "target": volume_of(t[2]) }))
        .unwrap();
    assert!(
        !Harness::nodes(&h.settle())
            .iter()
            .any(|n| n.label == "Mapping Mode"),
        "no chooser before a controller is mapped"
    );
    send(&mut h, &test, &[0xB0, 30, 127]);
    send(&mut h, &test, &[0xB0, 30, 127]);
    let nodes = tree(&mut h);
    let chooser = Harness::node(&nodes, "Mapping Mode");
    assert_eq!(chooser.value, "Twos Complement Linear");
    let message = Harness::node(&nodes, "Status");
    assert!(message.value.contains("guessed"), "{}", message.value);

    h.accesskit(Id::new(("chooser", "mapping-mode")), Action::Click);
    let nodes = tree(&mut h);
    let item = nodes
        .iter()
        .find(|n| n.label == "Absolute" && n.role == Role::Button)
        .expect("the chooser lists Absolute");
    h.frame(vec![Event::AccessKitActionRequest(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: item.id,
        data: None,
    })]);
    h.settle();
    assert_eq!(mappings(&mut h)[0]["mode"]["type"], "absolute");
}

// Spec: map-modes R5. While no input has its Remote switch on, the Status
// Bar says so when a control is selected.
#[test]
fn the_status_bar_says_when_no_input_has_remote_on() {
    let (mut h, _test) = session();
    let t = tracks(&h);
    h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Pads", "remote": false, "track": true }),
        )
        .unwrap();
    h.key(Key::M, command());
    h.shell
        .run("remote.select", json!({ "target": volume_of(t[2]) }))
        .unwrap();
    let nodes = tree(&mut h);
    let message = Harness::node(&nodes, "Status");
    assert!(message.value.contains("Remote"), "{}", message.value);
}

// Spec: map-modes R9. Dragging a Min or Max box is one undo step, in the
// control's own units.
#[test]
fn a_min_box_edits_the_range_as_one_undo_step() {
    let (mut h, _test) = session();
    let t = tracks(&h);
    h.shell
        .run(
            "remote.map",
            json!({ "source": { "type": "cc", "channel": 1, "controller": 20 },
                    "target": { "type": "mixer", "track": t[2], "param": "pan" } }),
        )
        .unwrap();
    h.key(Key::M, command());
    let nodes = tree(&mut h);
    let min = nodes
        .iter()
        .find(|n| n.label.starts_with("Min: "))
        .expect("a Min box");
    let from = rect_of(min).center();
    let steps = h.shell.editor().undo_history().count();
    assert_eq!(mappings(&mut h)[0]["min"], -1.0);

    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=6 {
        h.frame(vec![Event::PointerMoved(
            from + egui::vec2(step as f32 * 4.0, 0.0),
        )]);
    }
    h.frame(vec![Event::PointerButton {
        pos: from + egui::vec2(24.0, 0.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    let min = mappings(&mut h)[0]["min"].as_f64().unwrap();
    assert!(min > -1.0, "the box moved Min: {min}");
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(mappings(&mut h)[0]["min"], -1.0);
}
