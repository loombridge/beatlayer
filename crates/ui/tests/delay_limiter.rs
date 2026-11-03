//! Delay's LFO Controls toggle and Limiter's Maximize labels (WP-8.33),
//! headless: the title bar's expand toggle shows the rest of Delay's LFO, and
//! Maximize turns Ceiling and Gain into Threshold and Output in the body, in
//! the accessibility tree and in `device.parameters`.
//!
//! Spec: delay R13; limiter R12.

use crate::common;
use bl_devices::delay::{DELAY, LFO_DELAY, delay_type};
use bl_devices::limiter::{CEILING, GAIN, MAXIMIZE, OUTPUT, THRESHOLD, limiter_type};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::body_rect;
use bl_ui::device_view::{control_id, layout};
use common::{Harness, Window};
use egui::accesskit::Action;
use egui::{Rect, pos2};
use serde_json::json;

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

/// Where a parameter's control is drawn, if the panel draws one.
fn control(h: &mut Harness, device: DeviceId, param: bl_model::ParamId) -> Option<Rect> {
    let frame = frame_of(h, device);
    let state = h.shell.device_view_state();
    let panel = state
        .devices
        .iter()
        .find(|d| d.id == device)?
        .panel
        .clone()?;
    let expanded = state.devices.iter().find(|d| d.id == device)?.expanded;
    bl_ui::device_panel::place_shown(&panel, expanded, body_rect(frame, TITLE_BAR_HEIGHT))
        .0
        .into_iter()
        .find(|(p, _, _)| *p == param)
        .map(|c| c.2)
}

fn labelled(h: &mut Harness, label: &str) -> bool {
    Harness::nodes(&h.settle())
        .iter()
        .any(|n| n.label == label || n.label.starts_with(&format!("{label}:")))
}

// Spec: delay R13
#[test]
fn the_lfo_controls_toggle_shows_the_rest_of_the_lfo_and_is_no_undo_step() {
    let mut h = harness();
    let delay = add(&mut h, &delay_type());
    assert_eq!(DELAY, "bl.device.reprise");
    // The two amounts are in the body; Rate, Wave and Morph are not.
    assert!(control(&mut h, delay, LFO_DELAY).is_some());
    for label in ["Rate Mode", "Wave", "Morph"] {
        assert!(!labelled(&mut h, label), "{label} is hidden");
    }
    let steps = h.shell.editor().undo_history().count();
    h.accesskit(control_id(delay, "expand"), Action::Click);
    for label in ["Rate Mode", "Wave", "Morph", "LFO Time", "LFO Synced"] {
        assert!(labelled(&mut h, label), "{label} is shown");
    }
    h.accesskit(control_id(delay, "expand"), Action::Click);
    assert!(!labelled(&mut h, "Wave"));
    assert_eq!(h.shell.editor().undo_history().count(), steps);
}

// Spec: limiter R12
#[test]
fn maximize_relabels_ceiling_and_gain_in_the_body_the_tree_and_the_parameters() {
    let mut h = harness();
    let limiter = add(&mut h, &limiter_type());
    let shown = |h: &mut Harness| -> Vec<(String, bool)> {
        let parameters = h
            .shell
            .run("device.parameters", json!({ "device": limiter }))
            .unwrap();
        parameters["params"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["name"].as_str().unwrap().to_owned(),
                    p["shown"].as_bool().unwrap(),
                )
            })
            .collect()
    };
    let flag = |list: &[(String, bool)], name: &str| list.iter().find(|p| p.0 == name).unwrap().1;

    // Off: Ceiling and Gain are drawn, named, and shown.
    assert!(labelled(&mut h, "Ceiling") && labelled(&mut h, "Gain"));
    assert!(!labelled(&mut h, "Threshold") && !labelled(&mut h, "Output"));
    let list = shown(&mut h);
    assert!(flag(&list, "Ceiling") && flag(&list, "Gain"));
    assert!(!flag(&list, "Threshold") && !flag(&list, "Output"));
    let ceiling_at = control(&mut h, limiter, CEILING).unwrap();

    // Turning Maximize on, by its toggle, is one undo step named for it.
    let steps = h.shell.editor().undo_history().count();
    let toggle = control(&mut h, limiter, MAXIMIZE).unwrap();
    h.click(toggle.center());
    h.settle();
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);
    assert_eq!(
        h.shell.editor().undo_history().next().unwrap(),
        "Change \"Maximize\""
    );

    // On: Threshold sits where Ceiling did and Output where Gain did.
    assert!(labelled(&mut h, "Threshold") && labelled(&mut h, "Output"));
    assert!(!labelled(&mut h, "Ceiling") && !labelled(&mut h, "Gain"));
    assert_eq!(control(&mut h, limiter, THRESHOLD), Some(ceiling_at));
    assert!(control(&mut h, limiter, CEILING).is_none());
    assert!(control(&mut h, limiter, OUTPUT).is_some() && control(&mut h, limiter, GAIN).is_none());
    let list = shown(&mut h);
    assert!(!flag(&list, "Ceiling") && !flag(&list, "Gain"));
    assert!(flag(&list, "Threshold") && flag(&list, "Output"));
    // Every parameter is still listed: Ceiling and Gain keep their values.
    assert_eq!(list.len(), 11);
}
