//! Echo in the generic UI (WP-8.37), headless: the three tabs are view
//! state and no undo step, and a handle on the tunnel or the filter display
//! is dragged as one undo step.
//!
//! Spec: echo R1, R8, R24.

use crate::common;
use bl_devices::echo::{self, TABS, echo_type, line_param};
use bl_model::{DeviceId, DisplayHandle, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_display::handle_center;
use bl_ui::device_panel::{body_rect, place_with_pages};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::accesskit::Role;
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
    let t = echo_type();
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

fn set(h: &mut Harness, device: DeviceId, param: ParamId, value: f64) {
    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        )
        .unwrap();
    h.settle();
}

fn value(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    let device = h.shell.editor().document().device(device).unwrap();
    device
        .params
        .get(&param)
        .copied()
        .unwrap_or_else(|| echo_type().description.param(param).unwrap().default)
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

/// The body's placement: the controls, the cells and the tab buttons.
fn placed(
    h: &mut Harness,
    device: DeviceId,
) -> (
    bl_ui::device_panel::ControlRects,
    bl_ui::device_panel::CellRects,
    Vec<bl_model::device_layout::PlacedPages>,
) {
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
    place_with_pages(&panel, false, body_rect(frame, TITLE_BAR_HEIGHT))
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

/// Where the handle named `name` is drawn now.
fn handle(h: &mut Harness, device: DeviceId, name: &str) -> (Pos2, DisplayHandle, Rect) {
    let (_, cells, _) = placed(h, device);
    let cell = cells.iter().find(|c| c.0 == "Echo").expect("the display").1;
    let display = h.shell.device_display(device).expect("a display");
    let marker = display
        .markers
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no {name}"));
    let panel = h
        .shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .unwrap();
    (
        handle_center(cell, &display, marker, &panel).expect("a handle"),
        marker.handle.unwrap(),
        cell,
    )
}

fn shown(h: &Harness, device: DeviceId) -> String {
    let panel = h
        .shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .unwrap();
    panel.pages.into_iter().find(|p| p.node == TABS).unwrap().shown
}

// Spec: echo R1
#[test]
fn the_tabs_switch_the_controls_shown_and_are_no_undo_step() {
    let (mut h, device) = harness();
    let before = steps(&h);
    let dirty = h.shell.is_dirty();
    assert_eq!(shown(&h, device), "Echo");
    let drawn = |h: &mut Harness, names: &[&str]| {
        let nodes = Harness::nodes(&h.settle());
        names.iter().map(|n| nodes.iter().any(|node| node.label == *n)).collect::<Vec<_>>()
    };
    // The Echo tab holds the filters and the tunnel, the left section and the
    // global controls are always there.
    assert_eq!(drawn(&mut h, &["HP Freq", "Waveform", "Gate", "Feedback", "Dry/Wet"]), [true, false, false, true, true]);
    for (tab, controls) in [("Modulation", ["Waveform", "HP Freq"]), ("Character", ["Gate", "Waveform"])] {
        let (_, _, pages) = placed(&mut h, device);
        let node = pages.iter().find(|p| p.name == TABS).unwrap();
        let index = node.names.iter().position(|n| n == tab).unwrap();
        let r = node.buttons[index];
        h.click(pos2(r.x + r.width / 2.0, r.y + r.height / 2.0));
        h.settle();
        assert_eq!(shown(&h, device), tab);
        assert_eq!(drawn(&mut h, &controls), [true, false], "{tab}");
        assert_eq!(drawn(&mut h, &["Feedback", "Dry/Wet"]), [true, true]);
    }
    assert_eq!(steps(&h), before, "no undo step");
    assert_eq!(h.shell.is_dirty(), dirty, "the set is unchanged");
}

// Spec: echo R8
#[test]
fn dragging_a_tunnel_handle_sideways_sets_that_lines_time_as_one_undo_step() {
    let (mut h, device) = harness();
    // Apart, so the two handles don't sit on each other.
    set(&mut h, device, line_param(0, echo::NOTES), 5.0);
    set(&mut h, device, line_param(1, echo::NOTES), 1.0);
    let before = steps(&h);
    let (at, handle, cell) = handle(&mut h, device, "Left Time");
    assert_eq!(handle.x, line_param(0, echo::NOTES));
    let nodes = Harness::nodes(&h.settle());
    let node = Harness::node(&nodes, "Left Time");
    assert_eq!(node.role, Role::Slider);
    assert!(node.value.contains("ms"), "{}", node.value);
    // Back a fifth of the cell: from 1/2 note (5 of 10) to about 3.
    drag(&mut h, at, vec2(-cell.width() * 0.2, 0.0));
    let notes = value(&h, device, line_param(0, echo::NOTES));
    assert!((2.0..=4.0).contains(&notes) && notes != 5.0, "{notes}");
    assert_eq!(steps(&h), before + 1, "one undo step");
    assert_eq!(
        h.shell.editor().undo_history().next().unwrap(),
        "Change \"L Notes\""
    );
    assert_eq!(value(&h, device, line_param(1, echo::NOTES)), 1.0, "the other line");
    h.shell.invoke("edit.undo", json!({})).unwrap();
    h.settle();
    assert_eq!(value(&h, device, line_param(0, echo::NOTES)), 5.0);
}

// Spec: echo R8
#[test]
fn the_filter_views_handles_set_the_cutoff_sideways_and_the_resonance_up() {
    let (mut h, device) = harness();
    set(&mut h, device, echo::FILTER, 1.0);
    set(&mut h, device, echo::HP_FREQ, 400.0);
    h.shell
        .invoke("device.set_display_view", json!({ "device": device, "view": "Filter" }))
        .unwrap();
    h.settle();
    let before = steps(&h);
    let (at, handle, cell) = handle(&mut h, device, "HP");
    assert_eq!((handle.x, handle.y), (echo::HP_FREQ, Some(echo::HP_RES)));
    drag(&mut h, at, vec2(cell.width() * 0.2, -cell.height() * 0.3));
    assert!(value(&h, device, echo::HP_FREQ) > 800.0, "sideways: the cutoff");
    assert!(value(&h, device, echo::HP_RES) > 20.0, "up: the resonance");
    assert_eq!(steps(&h), before + 1, "both in one undo step");
}

// Spec: echo R24
#[test]
fn a_handle_takes_the_keyboard_and_steps_with_the_arrows() {
    let (mut h, device) = harness();
    set(&mut h, device, line_param(1, echo::NOTES), 1.0);
    let (at, _, _) = handle(&mut h, device, "Left Time");
    let notes = value(&h, device, line_param(0, echo::NOTES));
    let before = steps(&h);
    h.click(at);
    h.settle();
    h.key(egui::Key::ArrowRight, Modifiers::NONE);
    h.settle();
    assert_eq!(value(&h, device, line_param(0, echo::NOTES)), notes + 1.0);
    assert_eq!(steps(&h), before + 1);
    h.key(egui::Key::ArrowLeft, Modifiers::NONE);
    h.settle();
    assert_eq!(value(&h, device, line_param(0, echo::NOTES)), notes);
}
