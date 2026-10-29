//! The time-signature lane, headless (WP-6.61): its place under the
//! beat-time ruler, the markers in it and their gestures, the hatched short
//! bar and its menu, the Create menu's item and the Control Bar's fields.
//! Spec: time-editing R29 to R34.

use crate::common;
use bl_core::time::{Ticks, TimeSignature};
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::locator_commands as ids;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::keymap::Platform;
use bl_ui::layout::{ArrangementLayout, SIGNATURE_LANE};
use bl_ui::menus::item_state;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

fn sig(numerator: u8, denominator: u8) -> TimeSignature {
    TimeSignature::new(numerator, denominator).unwrap()
}

fn state(h: &mut Harness) -> ArrangementState {
    serde_json::from_value(h.shell.run("arrangement.state", json!({})).unwrap()).unwrap()
}

/// The Arrangement's layout in a harness that draws Live's title bar, as the
/// set has it now.
fn layout(h: &Harness) -> ArrangementLayout {
    let shell = bl_ui::layout::ShellLayout::compute(
        h.size - egui::vec2(0.0, bl_ui::layout::TITLE_BAR),
        true,
        true,
    )
    .translate(egui::vec2(0.0, bl_ui::layout::TITLE_BAR));
    let doc = h.shell.editor().document();
    let pinned = doc.returns().len() + 1;
    let layout =
        ArrangementLayout::compute(shell.main_view, h.shell.arrangement().overview(), pinned);
    if doc.settings().time_signatures.has_changes() {
        layout.with_signature_lane()
    } else {
        layout
    }
}

/// A harness with a marker of `signature` at `at`.
fn with_marker(at: Ticks, signature: TimeSignature) -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell
        .run(
            "song.set_time_signature_change",
            json!({ "at": at, "signature": signature }),
        )
        .unwrap();
    h.settle();
    h
}

/// The x of `at` in the lane's coordinates.
fn x_of(h: &Harness, layout: &ArrangementLayout, at: Ticks) -> f32 {
    let horizontal = h
        .shell
        .arrangement()
        .horizontal(h.shell.editor().document());
    layout.scrub.left() + horizontal.x(at) as f32
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn signatures(h: &Harness) -> Vec<(Ticks, TimeSignature)> {
    h.shell
        .editor()
        .document()
        .settings()
        .time_signatures
        .changes()
        .map(|c| (c.at, c.signature))
        .collect()
}

/// A right-click at `pos`.
fn right_click(h: &mut Harness, pos: Pos2) {
    h.frame(vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    h.frame(vec![Event::PointerButton {
        pos,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

/// The state of the menu item `row`.
fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let menus = menu_bar(MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap();
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

#[test]
fn the_lane_shows_with_the_first_change_and_the_rows_below_move_down() {
    // Spec: time-editing R29; layout L-ARR-004.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let without = layout(&h);
    assert!(without.signature_lane.is_none(), "no lane without changes");
    let nodes = Harness::nodes(&h.settle());
    assert!(!nodes.iter().any(|n| n.label == "4/4"));
    // Create > Insert Time Signature Change at bar 9 makes the lane appear.
    h.shell
        .run("arrangement.set_insert_marker", json!({ "at": 8 * BAR }))
        .unwrap();
    h.shell.invoke(ids::INSERT_SIGNATURE, json!({})).unwrap();
    let nodes = Harness::nodes(&h.settle());
    let with = layout(&h);
    let lane = with.signature_lane.expect("the lane shows");
    assert_eq!(lane.height(), SIGNATURE_LANE);
    // The loop brace, the scrub area and the track lanes shift down by the
    // lane's height, and the lane sits between the bar numbers and the
    // brace, inside the ruler.
    for (moved, was) in [
        (with.beat_ruler.bottom(), without.beat_ruler.bottom()),
        (with.scrub.top(), without.scrub.top()),
        (with.lanes.top(), without.lanes.top()),
    ] {
        assert_eq!(moved - was, SIGNATURE_LANE);
    }
    assert_eq!(with.beat_ruler.top(), without.beat_ruler.top());
    assert!(lane.top() > with.beat_ruler.top() && lane.bottom() < with.beat_ruler.bottom());
    assert_eq!(with.lanes.bottom(), without.lanes.bottom());
    // The starting signature is a marker at bar 1 too: both are buttons
    // named for their signatures, their value their position.
    let markers: Vec<_> = nodes.iter().filter(|n| n.label == "4/4").collect();
    assert_eq!(markers.len(), 2, "{markers:?}");
    assert!(markers.iter().all(|n| n.role == Role::Button));
    let values: Vec<&str> = markers.iter().map(|n| n.value.as_str()).collect();
    assert!(
        values.contains(&"1.1.1") && values.contains(&"9.1.1"),
        "{values:?}"
    );
    // Undo removes it again.
    h.shell.run("edit.undo", json!({})).unwrap();
    h.settle();
    assert!(layout(&h).signature_lane.is_none());
}

#[test]
fn clicking_a_marker_selects_it_and_cmd_r_types_a_signature() {
    // Spec: time-editing R31.
    let mut h = with_marker(Ticks(4 * BAR), sig(3, 4));
    let layout = layout(&h);
    let lane = layout.signature_lane.unwrap();
    h.click(pos2(
        x_of(&h, &layout, Ticks(4 * BAR)) + 6.0,
        lane.center().y,
    ));
    h.settle();
    assert_eq!(state(&mut h).selected_time_signature, Some(Ticks(4 * BAR)));
    let before = steps(&h);
    h.key(Key::R, Modifiers::COMMAND);
    h.settle();
    // The field opens with the old signature in it: select it, type over it.
    h.frame(vec![Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }]);
    h.frame(vec![Event::Text("7/8".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(steps(&h), before + 1);
    assert_eq!(signatures(&h)[1], (Ticks(4 * BAR), sig(7, 8)));
    // A signature outside the ranges is refused and changes nothing.
    h.key(Key::R, Modifiers::COMMAND);
    h.settle();
    h.frame(vec![Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }]);
    h.frame(vec![Event::Text("3/5".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(steps(&h), before + 1);
    assert_eq!(signatures(&h)[1].1, sig(7, 8));
}

#[test]
fn dragging_a_marker_moves_it_in_one_undo_step_and_the_first_stays() {
    // Spec: time-editing R32.
    let mut h = with_marker(Ticks(4 * BAR), sig(3, 4));
    let layout = layout(&h);
    let lane = layout.signature_lane.unwrap();
    let (x, y) = (x_of(&h, &layout, Ticks(4 * BAR)) + 6.0, lane.center().y);
    let before = steps(&h);
    h.frame(vec![
        Event::PointerMoved(pos2(x, y)),
        Event::PointerButton {
            pos: pos2(x, y),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    for step in 1..=6 {
        h.frame(vec![Event::PointerMoved(pos2(x + step as f32 * 20.0, y))]);
    }
    h.frame(vec![Event::PointerButton {
        pos: pos2(x + 120.0, y),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    assert_eq!(steps(&h), before + 1, "the whole drag is one step");
    let moved = signatures(&h)[1].0;
    assert!(moved > Ticks(4 * BAR), "{moved:?}");
    // It stays selected where it landed, and Left moves it one grid step.
    assert_eq!(state(&mut h).selected_time_signature, Some(moved));
    h.key(Key::ArrowLeft, Modifiers::NONE);
    assert!(signatures(&h)[1].0 < moved);
    assert_eq!(steps(&h), before + 2);
    // The marker at 1.1.1 can't be dragged.
    let first = layout.scrub.left() + 6.0;
    h.frame(vec![
        Event::PointerMoved(pos2(first, y)),
        Event::PointerButton {
            pos: pos2(first, y),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    for step in 1..=4 {
        h.frame(vec![Event::PointerMoved(pos2(
            first + step as f32 * 20.0,
            y,
        ))]);
    }
    h.frame(vec![Event::PointerButton {
        pos: pos2(first + 80.0, y),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    assert_eq!(signatures(&h)[0].0, Ticks::ZERO);
    assert_eq!(steps(&h), before + 2);
    // Delete deletes the selected marker.
    h.shell
        .run(ids::SELECT_SIGNATURE, json!({ "at": signatures(&h)[1].0 }))
        .unwrap();
    h.settle();
    h.key(Key::Delete, Modifiers::NONE);
    assert_eq!(signatures(&h).len(), 1);
}

#[test]
fn a_short_bar_is_hatched_and_its_menu_deletes_or_completes_it() {
    // Spec: time-editing R34 (C-TIME-024). 3/4 from a beat and a half into
    // bar 2 leaves bar 2 two beats short.
    let change = Ticks(BAR + 2 * Ticks::QUARTER.get());
    let mut h = with_marker(change, sig(3, 4));
    let layout = layout(&h);
    let lane = layout.signature_lane.unwrap();
    let before = steps(&h);
    // Right-clicking inside the short bar, away from the markers.
    let (from, to) = (x_of(&h, &layout, Ticks(BAR)), x_of(&h, &layout, change));
    assert!(to - from > 10.0, "{from} to {to}");
    right_click(&mut h, pos2((from + to) / 2.0, lane.center().y));
    let nodes = Harness::nodes(&h.settle());
    let delete = nodes
        .iter()
        .find(|n| n.label == "Delete Fragmentary Bar Time")
        .expect("the short bar's menu deletes its time");
    assert!(!delete.disabled);
    let complete = nodes
        .iter()
        .find(|n| n.label == "Complete Fragmentary Bar")
        .expect("and completes the bar");
    let bounds = complete.bounds.unwrap();
    h.click(pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(steps(&h), before + 1);
    assert_eq!(signatures(&h)[1].0, Ticks(2 * BAR), "on a bar line now");
}

#[test]
fn the_create_menus_item_reads_insert_with_the_arrangement_focused() {
    // Spec: time-editing R30 (M-CREATE-017).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let layout = layout(&h);
    // Focus the Arrangement with a click in its lanes.
    h.click(pos2(layout.lanes.left() + 1.0, layout.lanes.center().y));
    h.settle();
    let item = menu_item(&h, "M-CREATE-017");
    assert_eq!(item.label, "Insert Time Signature Change");
    assert!(item.enabled);
    // Elsewhere it reads Add Time Signature Change and is off.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    let item = menu_item(&h, "M-CREATE-017");
    assert_eq!(item.label, "Add Time Signature Change");
    assert!(!item.enabled);
}

#[test]
fn the_scrub_areas_menu_inserts_a_marker_at_the_insert_marker() {
    // Spec: time-editing R30 (C-TIME-022).
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell
        .run("arrangement.set_insert_marker", json!({ "at": 3 * BAR }))
        .unwrap();
    h.settle();
    let layout = layout(&h);
    right_click(
        &mut h,
        pos2(layout.scrub.left() + 200.0, layout.scrub.center().y),
    );
    let nodes = Harness::nodes(&h.settle());
    let item = nodes
        .iter()
        .find(|n| n.label == "Insert Time Signature Change")
        .expect("the scrub area's menu has it");
    assert!(!item.disabled);
    let bounds = item.bounds.unwrap();
    h.click(pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(signatures(&h)[1].0, Ticks(3 * BAR));
    assert_eq!(state(&mut h).selected_time_signature, Some(Ticks(3 * BAR)));
}

#[test]
fn the_control_bars_fields_show_an_led_and_set_the_marker_in_force() {
    // Spec: time-editing R33; control-bar-transport R7.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    let numerator = Harness::node(&nodes, "Time Signature Numerator");
    assert!(
        !numerator.description.contains("time-signature changes"),
        "no LED without changes: {}",
        numerator.description
    );
    h.shell
        .run(
            "song.set_time_signature_change",
            json!({ "at": 4 * BAR, "signature": sig(3, 4) }),
        )
        .unwrap();
    h.shell
        .run("arrangement.set_insert_marker", json!({ "at": 5 * BAR }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let numerator = Harness::node(&nodes, "Time Signature Numerator");
    assert!(
        numerator.description.contains("time-signature changes"),
        "{}",
        numerator.description
    );
    assert!(!numerator.description.contains("can't be set"));
}
