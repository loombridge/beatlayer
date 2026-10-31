//! Roar's shaper display in the Device View (WP-8.38; the roar spec, R12 and
//! R19), headless: a vertical drag on the display changes the shown stage's
//! Shaper Amount as one undo step, and the stage's own view is the one that
//! moves.

use crate::common;
use bl_devices::roar::{AMOUNT, ROAR, stage_param};
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_ui::device_panel::place_shown;
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
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

fn add_roar(h: &mut Harness) -> DeviceId {
    let types = bl_devices::device_types();
    let t = types
        .iter()
        .find(|t| matches!(&t.kind, bl_model::DeviceKind::Native { device_type } if device_type == ROAR))
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

/// The shaper display's cell in the Device View.
fn cell_of(h: &mut Harness, device: DeviceId) -> Rect {
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
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    let body = bl_ui::device_panel::body_rect(frame, bl_shell::device_commands::TITLE_BAR_HEIGHT);
    place_shown(&panel, false, body)
        .1
        .into_iter()
        .find(|(name, _, _)| name == "Shaper")
        .expect("the shaper display is drawn")
        .1
}

fn value(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
        .unwrap_or(0.0)
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Presses at `from` and drags 25 points up in five moves.
fn drag_up(h: &mut Harness, from: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for step in 1..=5 {
        h.frame(vec![Event::PointerMoved(
            from - vec2(0.0, 5.0 * step as f32),
        )]);
    }
    h.frame(vec![press(from - vec2(0.0, 25.0), false)]);
    h.settle();
}

// Spec: roar R12
#[test]
fn a_vertical_drag_on_the_shaper_display_changes_the_shown_stages_amount_in_one_undo_step() {
    let mut h = harness();
    let roar = add_roar(&mut h);
    let cell = cell_of(&mut h, roar);
    let steps = h.shell.editor().undo_history().len();
    drag_up(&mut h, cell.center());
    let first = value(&h, roar, stage_param(0, AMOUNT));
    assert!(
        first > 10.0 && first < 30.0,
        "25 points is a fifth of the range: {first}"
    );
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change \"Stage 1 Amount\"")
    );
    // The view the display shows is the stage that moves.
    h.shell
        .run(
            "device.set_display_view",
            json!({ "device": roar, "view": "Stage 2" }),
        )
        .unwrap();
    h.settle();
    drag_up(&mut h, cell.center());
    assert!(value(&h, roar, stage_param(1, AMOUNT)) > 10.0);
    assert_eq!(
        value(&h, roar, stage_param(0, AMOUNT)),
        first,
        "stage 1 stays"
    );
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change \"Stage 2 Amount\"")
    );
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        value(&h, roar, stage_param(1, AMOUNT)),
        0.0,
        "and it undoes"
    );
}
