//! Granulator III's panel (WP-8.27; the Granulator III spec R25): the
//! Capture button, the source chooser and Capture Length sit with the
//! display, and every control and cell lies within the body at 50 %, 100 %
//! and 200 % zoom with nothing overlapping.

use crate::common;
use bl_devices::granulator_iii::{CAPTURE_LENGTH, granulator_iii_type};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place_with_pages};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Rect, pos2, vec2};
use serde_json::json;

fn harness() -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track: TrackId = h.shell.editor().document().top_level().nth(1).unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    h.settle();
    let t = granulator_iii_type();
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role, "sidechain": t.sidechain } }),
            )
            .unwrap(),
    )
    .unwrap();
    h.settle();
    (h, added.device)
}

/// The body's controls, cells and pages' buttons, placed as they are drawn.
fn placed(
    h: &mut Harness,
    device: DeviceId,
) -> (
    Rect,
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
    let body = body_rect(frame, TITLE_BAR_HEIGHT);
    let (controls, cells, pages) = place_with_pages(&panel, false, body);
    (body, controls, cells, pages)
}

// Spec: granulator iii R25, R27
#[test]
fn the_capture_button_and_length_lie_within_the_body_at_three_zooms_with_nothing_overlapping() {
    let (mut h, device) = harness();
    for percent in [50, 100, 200] {
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        h.settle();
        let nodes = Harness::nodes(&h.settle());
        assert!(
            nodes.iter().any(|n| n.label == "Capture"),
            "the Capture button at {percent}%"
        );
        for page in ["Grain", "Envelope and Filter", "MPE"] {
            h.shell
                .invoke(
                    "device.set_page",
                    json!({ "device": device, "node": bl_devices::granulator_iii::PAGES, "page": page }),
                )
                .unwrap();
            let (body, controls, cells, pages) = placed(&mut h, device);
            if page == "Grain" {
                assert!(
                    controls.iter().any(|(param, ..)| *param == CAPTURE_LENGTH),
                    "Capture Length at {percent}%"
                );
            }
            let mut rects: Vec<Rect> = pages
                .iter()
                .flat_map(|p| p.buttons.iter())
                .map(|r| Rect::from_min_size(pos2(r.x, r.y), vec2(r.width, r.height)))
                .collect();
            rects.extend(controls.iter().map(|(_, _, r)| *r));
            rects.extend(cells.iter().map(|(_, r, _)| *r));
            for (i, a) in rects.iter().enumerate() {
                assert!(
                    body.expand(0.5).contains_rect(*a),
                    "{percent}% {page}: {a:?} outside {body:?}"
                );
                for b in &rects[i + 1..] {
                    assert!(
                        !a.shrink(0.01).intersects(b.shrink(0.01)),
                        "{percent}% {page}: {a:?} {b:?}"
                    );
                }
            }
        }
    }
}
