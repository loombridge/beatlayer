//! Operator's shell in the generic UI (WP-8.12), headless: each section's
//! button switches the display, and every section is laid out with nothing
//! overlapping at 50, 100 and 200 per cent.
//!
//! Spec: operator R18.

use crate::common;
use bl_devices::operator::{OSC_FINE, OSC_WAVE, PITCH_DEST_B, operator_type, osc};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place_with_pages};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Rect, pos2};
use serde_json::json;

const NODE: &str = "Sections";
const SECTIONS: [&str; 8] = [
    "Global", "Osc A", "Osc B", "Osc C", "Osc D", "LFO", "Filter", "Pitch",
];

fn harness() -> (Harness, DeviceId, TrackId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    let track: TrackId = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    h.settle();
    let t = operator_type();
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
    (h, added.device, track)
}

fn placed_all(
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

fn shown(h: &Harness, device: DeviceId) -> String {
    let frame = h
        .shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap();
    let pages = frame.panel.unwrap().pages;
    pages.into_iter().find(|p| p.node == NODE).unwrap().shown
}

fn name_of(param: bl_model::ParamId) -> String {
    operator_type()
        .description
        .param(param)
        .unwrap()
        .name
        .clone()
}

// Spec: operator R18
#[test]
fn a_section_button_switches_the_display_to_that_sections_controls() {
    let (mut h, device, _) = harness();
    let steps = h.shell.editor().undo_history().len();
    assert_eq!(shown(&h, device), "Global");
    let click = |h: &mut Harness, index: usize| {
        let pages = placed_all(h, device).2;
        let r = pages.iter().find(|p| p.name == NODE).unwrap().buttons[index];
        h.click(egui::Rect::from_min_size(pos2(r.x, r.y), egui::vec2(r.width, r.height)).center());
        h.settle();
    };
    let drawn =
        |h: &mut Harness, label: &str| Harness::nodes(&h.settle()).iter().any(|n| n.label == label);
    // Global: its own controls, not an oscillator's.
    assert!(drawn(&mut h, "Tone"));
    assert!(!drawn(&mut h, &name_of(osc(2, OSC_FINE))));
    // The sections' own controls show whichever section does.
    assert!(drawn(&mut h, &name_of(osc(2, OSC_WAVE))));
    click(&mut h, 3);
    assert_eq!(shown(&h, device), "Osc C");
    assert!(drawn(&mut h, &name_of(osc(2, OSC_FINE))));
    assert!(!drawn(&mut h, &name_of(osc(0, OSC_FINE))));
    assert!(!drawn(&mut h, "Tone"));
    click(&mut h, 7);
    assert_eq!(shown(&h, device), "Pitch");
    assert!(drawn(&mut h, &name_of(PITCH_DEST_B)));
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");
}

// Spec: operator R18
#[test]
fn every_section_is_laid_out_with_nothing_overlapping_at_50_100_and_200_percent() {
    let (mut h, device, _) = harness();
    for percent in [50, 100, 200] {
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        h.settle();
        for page in SECTIONS {
            h.shell
                .invoke(
                    "device.set_page",
                    json!({ "device": device, "node": NODE, "page": page }),
                )
                .unwrap();
            let (controls, cells, pages) = placed_all(&mut h, device);
            let mut rects: Vec<Rect> = pages
                .iter()
                .flat_map(|p| p.buttons.iter())
                .map(|r| Rect::from_min_size(pos2(r.x, r.y), egui::vec2(r.width, r.height)))
                .collect();
            assert_eq!(rects.len(), SECTIONS.len());
            rects.extend(controls.iter().map(|(_, _, r)| *r));
            rects.extend(cells.iter().map(|(_, r, _)| *r));
            for (i, a) in rects.iter().enumerate() {
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
