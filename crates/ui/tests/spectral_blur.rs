//! Spectral Blur in the generic UI (WP-8.49), headless: an edge of the range
//! on the display is dragged to a new frequency as one undo step.
//!
//! Spec: spectral-blur R11.

use crate::common;
use bl_devices::spectral_blur::{HIGH, LOW, spectral_blur_type};
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_display::handle_center;
use bl_ui::device_panel::{body_rect, place_with_pages};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn harness() -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    let track: TrackId = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    h.settle();
    let t = spectral_blur_type();
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    h.settle();
    (h, added.device)
}

fn value(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    let device = h.shell.editor().document().device(device).unwrap();
    device.params.get(&param).copied().unwrap_or_else(|| {
        spectral_blur_type()
            .description
            .param(param)
            .unwrap()
            .default
    })
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn drag(h: &mut Harness, from: Pos2, by: egui::Vec2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for i in 1..=10 {
        h.frame(vec![Event::PointerMoved(from + by * (i as f32 / 10.0))]);
    }
    h.frame(vec![press(from + by, false)]);
    h.settle();
}

/// Where the edge named `name` is drawn now, and the display's cell.
fn edge(h: &mut Harness, device: DeviceId, name: &str) -> (Pos2, Rect) {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    let frame = layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1;
    let panel = h
        .shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .unwrap();
    let (_, cells, _) = place_with_pages(&panel, false, body_rect(frame, TITLE_BAR_HEIGHT));
    let cell = cells
        .iter()
        .find(|c| c.0 == "Range")
        .expect("the display")
        .1;
    let display = h.shell.device_display(device).expect("a display");
    let marker = display.markers.iter().find(|m| m.name == name).unwrap();
    (
        handle_center(cell, &display, marker, &panel).expect("a handle"),
        cell,
    )
}

// Spec: spectral-blur R11
#[test]
fn dragging_an_edge_of_the_range_sets_low_or_high_as_one_undo_step() {
    let (mut h, device) = harness();
    let before = h.shell.editor().undo_history().len();
    let (at, cell) = edge(&mut h, device, "Low");
    // Towards the right, a log axis: Low rises from its default.
    drag(&mut h, at, vec2(cell.width() * 0.1, 0.0));
    assert!(value(&h, device, LOW) > 250.0, "{}", value(&h, device, LOW));
    assert_eq!(value(&h, device, HIGH), 5_000.0, "the other edge stays");
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    h.shell.invoke("edit.undo", json!({})).unwrap();
    h.settle();
    assert_eq!(value(&h, device, LOW), 200.0);
    let (at, cell) = edge(&mut h, device, "High");
    drag(&mut h, at, vec2(-cell.width() * 0.1, 0.0));
    assert!(value(&h, device, HIGH) < 4_500.0);
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
}
