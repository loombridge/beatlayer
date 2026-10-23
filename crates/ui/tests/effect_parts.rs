//! The device frame's shared parts for the effects (WP-5.42), headless: the
//! expand toggle in the title bar of a device that has an expanded part,
//! and a device's own items in its Show Options menu. The devices are the
//! framework's test devices.
//!
//! Spec: device-chains R34 and R35.

use crate::common;
use bl_devices::testing::{self, EFFECTS_INVERT};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::{DeviceAdded, frame_width};
use bl_ui::device_view::{control_id, layout, title_rect};
use common::{Harness, Window};
use egui::accesskit::{Action, Role};
use egui::{Event, PointerButton, Rect, pos2};
use serde_json::json;

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(testing::device_types());
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    h
}

fn add(h: &mut Harness, t: &bl_engine::DeviceType) -> DeviceId {
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

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn expanded(h: &Harness, device: DeviceId) -> bool {
    h.shell
        .device_view_state()
        .devices
        .iter()
        .any(|d| d.id == device && d.expanded)
}

// Spec: device-chains R34
#[test]
fn only_a_device_with_an_expanded_part_has_the_toggle_and_it_shows_the_part() {
    let mut h = harness();
    let scope = add(&mut h, &testing::scope_type());
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes.iter().all(|n| n.label != "Expand"),
        "a device without an expanded part has no toggle"
    );

    let effects = add(&mut h, &testing::effects_type());
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        nodes.iter().filter(|n| n.label == "Expand").count(),
        1,
        "only the device with one"
    );
    let toggle = Harness::node(&nodes, "Expand");
    assert_eq!((toggle.role, toggle.value.as_str()), (Role::Button, "off"));
    assert!(
        nodes.iter().all(|n| n.label != "Phase"),
        "the expanded controls are hidden"
    );
    let steps = h.shell.editor().undo_history().count();
    let narrow = frame_width(&h.shell.device_view_state().devices[1]);

    // Clicking it runs `device.expand`: the part shows and the frame widens.
    h.accesskit(control_id(effects, "expand"), Action::Click);
    let nodes = Harness::nodes(&h.settle());
    assert!(expanded(&h, effects) && !expanded(&h, scope));
    assert_eq!(Harness::node(&nodes, "Expand").value, "on");
    assert!(nodes.iter().any(|n| n.label.starts_with("Phase")));
    assert!(frame_width(&h.shell.device_view_state().devices[1]) > narrow);
    let wide = frame_of(&mut h, effects);
    assert_eq!(
        wide.width(),
        frame_width(&h.shell.device_view_state().devices[1])
    );
    // The toggle sits in the title bar, right of the activator.
    let toggle = Harness::node(&nodes, "Expand").bounds.unwrap();
    let activator = Harness::node(&nodes, "Device Activator").bounds.unwrap();
    assert!(toggle.x0 > activator.x1 && toggle.x1 < f64::from(wide.left() + 40.0));

    // Again, and the part hides. Neither is an undo step.
    h.accesskit(control_id(effects, "expand"), Action::Click);
    let nodes = Harness::nodes(&h.settle());
    assert!(!expanded(&h, effects));
    assert!(nodes.iter().all(|n| n.label != "Phase"));
    assert_eq!(h.shell.editor().undo_history().count(), steps);
}

// Spec: device-chains R35
#[test]
fn the_device_menu_ends_with_the_devices_own_items_ticked_while_on() {
    let mut h = harness();
    let effects = add(&mut h, &testing::effects_type());
    let value = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .device(effects)
            .unwrap()
            .params
            .get(&EFFECTS_INVERT)
            .copied()
    };
    let open = |h: &mut Harness| {
        let frame = frame_of(h, effects);
        let at = title_rect(frame, false).center();
        h.frame(vec![Event::PointerMoved(at)]);
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.frame(vec![press(true)]);
        h.frame(vec![press(false)]);
        Harness::nodes(&h.settle())
    };

    // Off: listed after Save as Default Preset, with no tick.
    let nodes = open(&mut h);
    let place = |nodes: &[common::Node], label: &str| {
        nodes
            .iter()
            .position(|n| n.label.trim_start_matches("✔ ") == label)
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
    };
    assert!(
        place(&nodes, "Invert LFO") > place(&nodes, "Save as Default Preset"),
        "the device's own items follow the general ones"
    );
    let item = &nodes[place(&nodes, "Invert LFO")];
    assert!(!item.disabled && !item.label.starts_with('✔'));

    // Choosing it turns it on, as one undo step; the menu then ticks it.
    let steps = h.shell.editor().undo_history().count();
    let spot = item.bounds.unwrap();
    h.click(pos2(
        ((spot.x0 + spot.x1) / 2.0) as f32,
        ((spot.y0 + spot.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(value(&h), Some(1.0));
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);
    assert_eq!(last_step(&h), "Change parameter");
    let nodes = open(&mut h);
    let item = &nodes[place(&nodes, "Invert LFO")];
    assert!(item.label.starts_with('✔'), "ticked while on");
}
