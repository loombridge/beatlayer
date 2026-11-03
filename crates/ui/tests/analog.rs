//! Analog's panel in the Device View (WP-8.15; the Analog spec, R22, R26 and
//! R27), headless: the shell's sections switch the display's controls and
//! drawings without an undo step, every section lays out with nothing
//! overlapping at 50, 100 and 200 % zoom, and Quick Routing's four buttons run
//! their command.

use crate::common;
use bl_devices::analog::{self, QUICK_ROUTINGS, SECTIONS};
use bl_model::device_layout::PlacedPages;
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place_with_pages};
use bl_ui::device_view::layout;
use bl_ui::main_window::UiEvent;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Rect, pos2};
use serde_json::json;

fn harness() -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track: TrackId = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    h.settle();
    let t = bl_devices::analog_type();
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

fn show(h: &mut Harness, device: DeviceId, section: &str) {
    h.shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": analog::PAGES, "page": section }),
        )
        .unwrap();
    h.settle();
}

/// The device's controls, cells and section buttons placed in its body.
fn placed_all(
    h: &mut Harness,
    device: DeviceId,
) -> (
    bl_ui::device_panel::ControlRects,
    bl_ui::device_panel::CellRects,
    Vec<PlacedPages>,
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

// Spec: analog R27
#[test]
fn every_section_is_laid_out_with_nothing_overlapping_at_50_100_and_200_percent() {
    let (mut h, device) = harness();
    for percent in [50, 100, 200] {
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        h.settle();
        for section in SECTIONS {
            show(&mut h, device, section);
            let (controls, cells, pages) = placed_all(&mut h, device);
            let mut rects: Vec<Rect> = pages
                .iter()
                .flat_map(|p| p.buttons.iter())
                .map(|r| Rect::from_min_size(pos2(r.x, r.y), egui::vec2(r.width, r.height)))
                .collect();
            assert_eq!(rects.len(), SECTIONS.len(), "a button for each section");
            rects.extend(controls.iter().map(|(_, _, r)| *r));
            rects.extend(cells.iter().map(|(_, r, _)| *r));
            for (i, a) in rects.iter().enumerate() {
                for b in &rects[i + 1..] {
                    assert!(
                        !a.shrink(0.01).intersects(b.shrink(0.01)),
                        "{percent}% {section}: {a:?} {b:?}"
                    );
                }
            }
        }
    }
}

// Spec: analog R26, R27
#[test]
fn a_section_button_switches_the_display_and_the_cells_are_drawn() {
    let (mut h, device) = harness();
    let steps = h.shell.editor().undo_history().len();
    let dirty = h.shell.is_dirty();
    let cells = |h: &mut Harness| -> Vec<String> {
        Harness::nodes(&h.settle())
            .iter()
            .filter(|n| n.role == Role::Image)
            .map(|n| n.label.clone())
            .collect()
    };
    let (_, _, pages) = placed_all(&mut h, device);
    let button = |n: usize| {
        let r = pages[0].buttons[n];
        pos2(r.x + r.width / 2.0, r.y + r.height / 2.0)
    };
    assert!(
        !cells(&mut h).iter().any(|c| c.contains("Response")),
        "Oscillator 1 draws no filter"
    );
    h.click(button(3));
    h.settle();
    let shown = cells(&mut h);
    for name in ["Filter 1 Response", "Filter 1 Envelope"] {
        assert!(shown.iter().any(|c| c == name), "{name} in {shown:?}");
    }
    h.click(button(8));
    h.settle();
    assert!(cells(&mut h).iter().any(|c| c == "LFO 2 Wave"));
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");
    assert_eq!(h.shell.is_dirty(), dirty, "the set is unchanged");
}

// Spec: analog R22
#[test]
fn quick_routings_four_buttons_each_run_their_command() {
    let (mut h, device) = harness();
    show(&mut h, device, "Global");
    // The device is wider than the window: scroll its chain to the right end.
    h.shell
        .run("device_view.scroll", json!({ "x": 2_000.0 }))
        .unwrap();
    h.settle();
    for (button, routing) in
        QUICK_ROUTINGS
            .into_iter()
            .zip(["parallel", "split", "filter_1_only", "serial"])
    {
        let nodes = Harness::nodes(&h.settle());
        let b = nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label == button)
            .unwrap_or_else(|| panic!("a {button} button"))
            .bounds
            .unwrap();
        h.take_events();
        assert!(
            b.x1 < f64::from(h.size.x),
            "{button} is in the window: {b:?}"
        );
        h.click(pos2(
            ((b.x0 + b.x1) / 2.0) as f32,
            ((b.y0 + b.y1) / 2.0) as f32,
        ));
        h.settle();
        let wanted = bl_shell::Invocation::new(
            "analog.quick_routing",
            json!({ "device": device, "routing": routing }),
        );
        assert!(
            h.take_events().contains(&UiEvent::Invoke(wanted)),
            "{button} runs the command"
        );
    }
}
