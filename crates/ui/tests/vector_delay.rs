//! Vector Delay in the generic UI (WP-8.52), headless: a particle on the
//! display is dragged to a new starting place as one undo step, and takes
//! the keyboard and a screen reader.
//!
//! Spec: vector-delay R8, R13.

use crate::common;
use bl_devices::vector_delay::{tap_x, tap_y, vector_delay_type};
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place_with_pages};
use bl_ui::device_view::layout;
use bl_ui::particles::particle_center;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2};
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
    let t = vector_delay_type();
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
        vector_delay_type()
            .description
            .param(param)
            .unwrap()
            .default
    })
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

/// The display cell's rectangle, as the body places it.
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
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .unwrap();
    let (_, cells, _) = place_with_pages(&panel, false, body_rect(frame, TITLE_BAR_HEIGHT));
    cells
        .iter()
        .find(|c| c.0 == "Particle Field")
        .expect("the display")
        .1
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

/// Where the particle named `name` is drawn now, and the display's cell.
fn particle(h: &mut Harness, device: DeviceId, name: &str) -> (Pos2, Rect) {
    let cell = cell_of(h, device);
    let display = h.shell.device_display(device).expect("a display");
    let marker = display
        .markers
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no {name}"));
    (particle_center(cell, marker), cell)
}

fn default(param: ParamId) -> f64 {
    vector_delay_type()
        .description
        .param(param)
        .unwrap()
        .default
}

// Spec: vector-delay R8
#[test]
fn dragging_a_particle_sets_its_starting_place_as_one_undo_step() {
    let (mut h, device) = harness();
    let before = steps(&h);
    let (at, cell) = particle(&mut h, device, "Tap 1");
    // Read back through device.display too: four particles, each a slider a
    // screen reader names with its time and level.
    assert_eq!(h.shell.device_display(device).unwrap().markers.len(), 4);
    let nodes = Harness::nodes(&h.settle());
    let node = Harness::node(&nodes, "Tap 1");
    assert_eq!(node.role, Role::Slider);
    assert!(
        node.value.contains("ms") && node.value.contains("dB"),
        "{}",
        node.value
    );
    // Drop it four fifths across and nine tenths up.
    let to = pos2(
        cell.left() + cell.width() * 0.8,
        cell.bottom() - cell.height() * 0.9,
    );
    drag(&mut h, at, to - at);
    let (x, y) = (value(&h, device, tap_x(0)), value(&h, device, tap_y(0)));
    assert!((x - 0.8).abs() < 0.02 && (y - 0.9).abs() < 0.02, "{x}, {y}");
    assert_eq!(steps(&h), before + 1, "one undo step for both");
    assert_eq!(
        h.shell.editor().undo_history().next().unwrap(),
        "Move \"Tap 1\""
    );
    assert_eq!(
        value(&h, device, tap_x(1)),
        default(tap_x(1)),
        "the others stay"
    );
    h.shell.invoke("edit.undo", json!({})).unwrap();
    h.settle();
    assert_eq!(value(&h, device, tap_x(0)), default(tap_x(0)));
    assert_eq!(value(&h, device, tap_y(0)), default(tap_y(0)));
}

// Spec: vector-delay R13
#[test]
fn a_particle_takes_the_keyboard_and_its_arrow_keys_move_its_starting_place() {
    let (mut h, device) = harness();
    let (at, _) = particle(&mut h, device, "Tap 2");
    let (x, y) = (value(&h, device, tap_x(1)), value(&h, device, tap_y(1)));
    let before = steps(&h);
    h.click(at);
    h.settle();
    h.key(egui::Key::ArrowRight, Modifiers::NONE);
    h.settle();
    assert!((value(&h, device, tap_x(1)) - (x + 0.01)).abs() < 1e-9);
    assert_eq!(steps(&h), before + 1);
    h.key(egui::Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert!((value(&h, device, tap_y(1)) - (y - 0.01)).abs() < 1e-9);
    h.key(egui::Key::ArrowLeft, Modifiers::NONE);
    h.settle();
    assert!((value(&h, device, tap_x(1)) - x).abs() < 1e-9);
    assert_eq!(steps(&h), before + 3);
    // Every particle is reachable: each is a slider with its tap's name.
    let nodes = Harness::nodes(&h.settle());
    for n in 1..=4 {
        assert_eq!(
            Harness::node(&nodes, &format!("Tap {n}")).role,
            Role::Slider,
            "Tap {n}"
        );
    }
}
