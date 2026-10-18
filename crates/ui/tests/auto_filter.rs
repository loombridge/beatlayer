//! Sweep, the native Auto Filter, in the Device View (WP-5.20; the
//! auto-filter spec, R12 and R13), headless: its own items in the device's
//! menu, no expand toggle, and its display cell drawn from the newest data.

mod common;

use bl_devices::auto_filter::{AUTO_FILTER, HI_QUALITY, MONO_SIDECHAIN};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_ui::device_view::{layout, title_rect};
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, PointerButton, Rect, pos2};
use serde_json::json;

/// A harness with Beatlayer's own devices, and the engine off so the display
/// data a test sets stays (the engine would publish its own).
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

fn add_sweep(h: &mut Harness) -> DeviceId {
    let types = bl_devices::device_types();
    let t = types
        .iter()
        .find(|t| matches!(&t.kind, bl_model::DeviceKind::Native { device_type } if device_type == AUTO_FILTER))
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

fn value(h: &Harness, device: DeviceId, param: bl_model::ParamId) -> Option<f64> {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
}

// Spec: auto-filter R12
#[test]
fn the_devices_menu_ends_with_mono_sidechain_ticked_and_hi_quality() {
    let mut h = harness();
    let sweep = add_sweep(&mut h);
    let open = |h: &mut Harness| {
        let frame = frame_of(h, sweep);
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
    let nodes = open(&mut h);
    let place = |nodes: &[common::Node], label: &str| {
        nodes
            .iter()
            .position(|n| n.label.trim_start_matches("✔ ") == label)
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
    };
    let save = place(&nodes, "Save as Default Preset");
    let (mono, quality) = (place(&nodes, "Mono Sidechain"), place(&nodes, "Hi-Quality"));
    assert!(
        save < mono && mono < quality,
        "after Save as Default Preset, in order"
    );
    assert!(
        nodes[mono].label.starts_with('✔'),
        "Mono Sidechain is ticked in a new device"
    );
    assert!(!nodes[quality].label.starts_with('✔'));

    // Choosing Hi-Quality turns it on, as one undo step.
    let steps = h.shell.editor().undo_history().count();
    let spot = nodes[quality].bounds.unwrap();
    h.click(pos2(
        ((spot.x0 + spot.x1) / 2.0) as f32,
        ((spot.y0 + spot.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(value(&h, sweep, HI_QUALITY), Some(1.0));
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);
    let nodes = open(&mut h);
    assert!(nodes[place(&nodes, "Hi-Quality")].label.starts_with('✔'));
    assert!(value(&h, sweep, MONO_SIDECHAIN).is_none_or(|v| v == 1.0));
}

// Spec: auto-filter R13
#[test]
fn the_filter_cell_has_no_expand_toggle_and_draws_the_curves_and_readings() {
    let mut h = harness();
    let sweep = add_sweep(&mut h);
    assert!(
        h.shell
            .device_view_state()
            .devices
            .iter()
            .all(|d| d.id != sweep || !d.expandable),
        "Sweep has no expanded part"
    );
    h.shell
        .set_device_display_data(sweep, &[2_000.0, 500.0, 1.0]);
    let nodes = Harness::nodes(&h.frame(vec![]));
    let node = Harness::node(&nodes, "Filter");
    assert_eq!(node.role, Role::Image);
    assert!(node.value.contains("Left Cutoff 2000 Hz"), "{}", node.value);
    assert!(node.value.contains("Clip Clipping"), "{}", node.value);
}
