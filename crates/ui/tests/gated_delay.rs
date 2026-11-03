//! Gated Delay's step row (WP-8.51; the gated-delay spec, R9), headless: the
//! 16 steps are switches in the device's display cell, a click turns one
//! over as one undo step named for it, and an agent reads the row back.
//!
//! Spec: gated-delay R9.

use crate::common;
use bl_devices::gated_delay::{self, STEPS_SERIES, gated_delay_type, step};
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Rect, pos2};
use serde_json::json;

fn harness() -> (Harness, DeviceId) {
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
    let t = gated_delay_type();
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    h.settle();
    (h, added.device)
}

/// Where a step's switch is drawn.
fn switch(h: &mut Harness, device: DeviceId, param: ParamId) -> Rect {
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
    place(&panel, body_rect(frame, TITLE_BAR_HEIGHT))
        .0
        .into_iter()
        .find(|(p, _, _)| *p == param)
        .unwrap()
        .2
}

fn value(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    let device = h.shell.editor().document().device(device).unwrap();
    device
        .params
        .get(&param)
        .copied()
        .unwrap_or_else(|| gated_delay_type().description.param(param).unwrap().default)
}

fn row(h: &mut Harness, device: DeviceId) -> Vec<f64> {
    let display = h
        .shell
        .run("device.display", json!({ "device": device }))
        .unwrap();
    let series = display["series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == STEPS_SERIES)
        .expect("the step row")
        .clone();
    serde_json::from_value(series["values"].clone()).unwrap()
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_click_on_a_step_turns_it_over_as_one_undo_step_and_the_row_reads_back() {
    let (mut h, device) = harness();
    let mut want: Vec<f64> = (1..=gated_delay::STEP_COUNT)
        .map(|n| value(&h, device, step(n)))
        .collect();
    assert_eq!(row(&mut h, device), want, "the row reads the switches");
    // Step 2 is off: a click turns it on, and one undo turns it off.
    let two = switch(&mut h, device, step(2));
    h.click(two.center());
    h.settle();
    want[1] = 1.0;
    assert_eq!(value(&h, device, step(2)), 1.0);
    assert_eq!(last_step(&h), "Change \"Step 2\"");
    assert_eq!(row(&mut h, device), want);
    h.shell.run("edit.undo", json!({})).unwrap();
    h.settle();
    assert_eq!(value(&h, device, step(2)), 0.0, "one undo step");
    // Step 16, in the second row, is off too; step 4 is on and goes off.
    let four = switch(&mut h, device, step(4));
    h.click(four.center());
    h.settle();
    assert_eq!(value(&h, device, step(4)), 0.0);
    assert_eq!(last_step(&h), "Change \"Step 4\"");
    let sixteen = switch(&mut h, device, step(16));
    assert!(
        sixteen.top() > four.bottom(),
        "steps 9 to 16 are the second row"
    );
}
