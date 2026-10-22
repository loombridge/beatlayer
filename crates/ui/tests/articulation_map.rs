//! The Articulation Map's list in the Device View (WP-6.47; the articulation
//! map spec, R15), headless: the list shows every articulation, and a click
//! on a row's number makes that articulation current, as one undo step named
//! Change "Articulation".

mod common;

use bl_devices::articulation_map::{ARTICULATION, ARTICULATION_MAP};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_ui::device_panel::place_shown;
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Rect, pos2};
use serde_json::json;

/// A harness with Beatlayer's own devices, and the engine off so the display
/// data a test sets stays.
fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.frame(vec![]);
    h
}

/// The Articulation Map, on the selected track.
fn add_map(h: &mut Harness) -> DeviceId {
    let types = bl_devices::device_types();
    let t = types
        .iter()
        .find(|t| {
            matches!(&t.kind, bl_model::DeviceKind::Native { device_type } if device_type == ARTICULATION_MAP)
        })
        .unwrap();
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    h.settle();
    added.device
}

/// The device's frame in the Device View.
fn frame_of(h: &mut Harness, device: DeviceId) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1
}

/// The Articulation control's value.
fn value(h: &Harness, device: DeviceId) -> f64 {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&ARTICULATION)
        .copied()
        .unwrap_or(1.0)
}

// Spec: articulation-map R15. The list shows every articulation, and a click
// on a row's number makes that articulation current.
#[test]
fn a_click_on_a_rows_number_makes_that_articulation_current() {
    let mut h = harness();
    let device = add_map(&mut h);
    h.shell
        .run(
            "articulation.add",
            json!({ "device": device, "name": "Staccato" }),
        )
        .unwrap();
    h.settle();
    assert_eq!(value(&h, device), 1.0, "the first articulation is current");

    // The list's cell, and the second row's number in it.
    let frame = frame_of(&mut h, device);
    let panel = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    let body = bl_ui::device_panel::body_rect(frame, bl_shell::device_commands::TITLE_BAR_HEIGHT);
    let cells = place_shown(&panel, false, body).1;
    let (_, cell, _) = cells
        .into_iter()
        .find(|(name, _, _)| name == bl_devices::articulation_map::LIST_CELL)
        .expect("the list is drawn");
    let number = pos2(cell.left() + 6.0, cell.top() + 3.0 + 15.0);

    h.click(number);
    h.settle();
    assert_eq!(value(&h, device), 2.0, "the second articulation is current");
    // The click is one undo step, named for the control.
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(value(&h, device), 1.0, "the click is one undo step");
}

// Spec: articulation-map R15. Each row's Learn button lights while it is
// armed, and it goes out when the learn is over.
#[test]
fn a_rows_learn_button_lights_while_it_is_armed() {
    let mut h = harness();
    let device = add_map(&mut h);
    h.shell
        .run(
            "articulation.add",
            json!({ "device": device, "name": "Staccato" }),
        )
        .unwrap();
    h.settle();
    h.shell
        .run(
            "articulation.learn",
            json!({ "device": device, "index": 1 }),
        )
        .unwrap();
    h.settle();
    let panel = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    assert_eq!(panel.learning, Some(1), "the second row is learning");
    // Clicking the lit button again puts it out.
    h.shell
        .run(
            "articulation.learn",
            json!({ "device": device, "index": 1 }),
        )
        .unwrap();
    h.settle();
    let panel = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    assert_eq!(panel.learning, None, "the learn is over");
}
