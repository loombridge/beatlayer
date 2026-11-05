//! Operator's shell in the generic UI (WP-8.12), headless: each section's
//! button switches the display, and every section is laid out with nothing
//! overlapping at 50, 100 and 200 per cent.
//!
//! Spec: operator R18.

use crate::common;
use bl_devices::operator::{OSC_FEEDBACK, OSC_WAVE, PITCH_DEST_B, operator_type, osc, target_dest};
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place_with_pages};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Rect, pos2};
use serde_json::json;

const NODE: &str = "Sections";
const SECTIONS: [&str; 8] = [
    "Osc A", "Osc B", "Osc C", "Osc D", "LFO", "Filter", "Pitch", "Global",
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
fn a_click_on_a_row_switches_the_display_to_that_rows_fields() {
    let (mut h, device, _) = harness();
    let steps = h.shell.editor().undo_history().len();
    assert_eq!(shown(&h, device), "Osc A");
    // A click in the margin of a row's controls, where no control takes it.
    let click = |h: &mut Harness, index: usize| {
        let pages = placed_all(h, device).2;
        let r = pages.iter().find(|p| p.name == NODE).unwrap().buttons[index];
        h.click(pos2(r.x + 1.0, r.y + 1.0));
        h.settle();
    };
    let drawn =
        |h: &mut Harness, label: &str| Harness::nodes(&h.settle()).iter().any(|n| n.label == label);
    // Osc A's row is shown: its fields, not the global table's.
    assert!(drawn(&mut h, &name_of(osc(0, OSC_FEEDBACK))));
    assert!(!drawn(&mut h, &name_of(target_dest(0, false))));
    // The rows' own controls show whichever row does.
    assert!(drawn(&mut h, "Tone"));
    click(&mut h, 7);
    assert_eq!(shown(&h, device), "Global");
    assert!(drawn(&mut h, &name_of(target_dest(0, false))));
    assert!(!drawn(&mut h, &name_of(osc(0, OSC_FEEDBACK))));
    click(&mut h, 2);
    assert_eq!(shown(&h, device), "Osc C");
    assert!(drawn(&mut h, &name_of(osc(2, OSC_FEEDBACK))));
    assert!(!drawn(&mut h, &name_of(osc(0, OSC_FEEDBACK))));
    click(&mut h, 6);
    assert_eq!(shown(&h, device), "Pitch");
    assert!(drawn(&mut h, &name_of(PITCH_DEST_B)));
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");
}

// Spec: operator R18
#[test]
fn every_row_is_laid_out_with_nothing_overlapping_at_50_100_and_200_percent() {
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
            let (_, cells, pages) = placed_all(&mut h, device);
            // The controls that show now, as the panel draws them: those a
            // condition leaves out take no place.
            let panel = h
                .shell
                .device_view_state()
                .devices
                .into_iter()
                .find(|d| d.id == device)
                .unwrap()
                .panel
                .unwrap();
            let (controls, _, _) = {
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
                let body = body_rect(frame, TITLE_BAR_HEIGHT);
                let laid = panel.layout(false).place_for(
                    bl_model::device_layout::Size::new(body.width(), body.height()),
                    0,
                );
                let shown: Vec<_> = laid
                    .controls
                    .iter()
                    .filter(|c| c.shown.is_none_or(|s| s.holds(panel.read(s.param))))
                    .map(|c| {
                        Rect::from_min_size(
                            body.min + egui::vec2(c.rect.x, c.rect.y),
                            egui::vec2(c.rect.width, c.rect.height),
                        )
                    })
                    .collect();
                (shown, (), ())
            };
            assert_eq!(
                pages.iter().map(|p| p.buttons.len()).sum::<usize>(),
                SECTIONS.len(),
                "a region for each row"
            );
            let mut rects = controls;
            // The global page's cell holds its controls, so it is no part of
            // the check.
            rects.extend(
                cells
                    .iter()
                    .filter(|(name, _, _)| name != "Global")
                    .map(|(_, r, _)| *r),
            );
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

// ---- Part 2 (WP-8.13): drawing partials, the menu and the display ----

/// The display cell's rect on the page shown.
fn display_cell(h: &mut Harness, device: DeviceId) -> Rect {
    let cells = placed_all(h, device).1;
    cells
        .iter()
        .find(|(name, _, _)| name == "Display")
        .map(|(_, rect, _)| *rect)
        .expect("the page has a display cell")
}

fn drag(h: &mut Harness, points: &[egui::Pos2]) {
    use egui::{Event, Modifiers, PointerButton};
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.frame(vec![Event::PointerMoved(points[0])]);
    h.frame(vec![button(points[0], true)]);
    for p in &points[1..] {
        h.frame(vec![Event::PointerMoved(*p)]);
    }
    h.frame(vec![button(*points.last().unwrap(), false)]);
    h.settle();
}

fn drawn_partials(h: &Harness, device: DeviceId) -> Option<Vec<f32>> {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .partials
        .first()
        .map(|p| p.levels.clone())
}

// Spec: operator R27 (dragging across the bars sets each partial as one undo step)
#[test]
fn dragging_across_the_partials_sets_each_one_to_the_height_as_one_undo_step() {
    let (mut h, device, _) = harness();
    h.shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": NODE, "page": "Osc A" }),
        )
        .unwrap();
    h.shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": osc(0, OSC_WAVE), "value": 22 }),
        )
        .unwrap();
    h.shell
        .run(
            "device.set_display_view",
            json!({ "device": device, "view": "Osc A Oscillator" }),
        )
        .unwrap();
    h.settle();
    let steps = h.shell.editor().undo_history().len();
    let cell = display_cell(&mut h, device);
    // 16 partials across the cell: the pointer crosses the second to the
    // fifth at a height of 70 %.
    let at = |partial: usize, height: f32| {
        pos2(
            cell.left() + cell.width() * (partial as f32 - 0.5) / 16.0,
            cell.bottom() - cell.height() * height,
        )
    };
    drag(&mut h, &[at(2, 0.7), at(3, 0.7), at(4, 0.7), at(5, 0.7)]);
    let levels = drawn_partials(&h, device).expect("a drawn wave");
    assert_eq!(levels.len(), 16);
    for (k, level) in levels.iter().enumerate().take(5).skip(1) {
        assert!((level - 0.7).abs() < 0.05, "partial {}: {level}", k + 1);
    }
    assert_eq!(levels[0], 1.0, "the first partial is as it was");
    assert_eq!(levels[8], 0.0);
    // One undo step, named for the gesture.
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change Partials")
    );
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(drawn_partials(&h, device), None);
    // Even Only restricts a drag to the even partials, and with Normalize
    // off the heights stay as drawn.
    h.shell
        .run(
            "operator.set_harmonics",
            json!({ "device": device, "even_only": true, "normalize": false }),
        )
        .unwrap();
    h.settle();
    drag(&mut h, &[at(2, 0.4), at(3, 0.4), at(4, 0.4), at(5, 0.4)]);
    let levels = drawn_partials(&h, device).expect("a drawn wave");
    assert!((levels[1] - 0.4).abs() < 0.05 && (levels[3] - 0.4).abs() < 0.05);
    assert_eq!(
        (levels[2], levels[4]),
        (0.0, 0.0),
        "the odd partials are left"
    );
}

// Spec: operator R28 (the title bar menu's items)
#[test]
fn the_title_bar_menu_has_operators_items_and_they_run_their_commands() {
    let (mut h, device, _) = harness();
    h.shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": NODE, "page": "Filter" }),
        )
        .unwrap();
    let open = |h: &mut Harness| {
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
        let at = bl_ui::device_view::title_rect(frame, false).center();
        h.frame(vec![egui::Event::PointerMoved(at)]);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.frame(vec![press(true)]);
        h.frame(vec![press(false)]);
        Harness::nodes(&h.settle())
    };
    let nodes = open(&mut h);
    for item in [
        "Copy Oscillator Settings ⏵",
        "Set All Envelope Levels to Maximum",
        "Set All Envelope Levels to Minimum",
        "Set All Envelope Levels to Middle",
        "Even Only",
        "Odd Only",
        "✔ Normalize",
        "Play By Key",
    ] {
        assert!(nodes.iter().any(|n| n.label == item), "{item}");
    }
    // Choosing Play By Key sets the cutoff and Freq < Key, as one undo step.
    let steps = h.shell.editor().undo_history().len();
    let play = Harness::node(&nodes, "Play By Key").bounds.unwrap();
    h.click(pos2(
        ((play.x0 + play.x1) / 2.0) as f32,
        ((play.y0 + play.y1) / 2.0) as f32,
    ));
    h.settle();
    let param = |id| {
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .params
            .get(&id)
            .copied()
    };
    assert_eq!(param(bl_devices::operator::FILTER_FREQ), Some(466.0));
    assert_eq!(param(bl_devices::operator::FREQ_KEY), Some(100.0));
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    // Set All Envelope Levels acts on the section shown: the filter's.
    let nodes = open(&mut h);
    let max = Harness::node(&nodes, "Set All Envelope Levels to Minimum")
        .bounds
        .unwrap();
    h.click(pos2(
        ((max.x0 + max.x1) / 2.0) as f32,
        ((max.y0 + max.y1) / 2.0) as f32,
    ));
    h.settle();
    let sustain = bl_devices::operator::shaped(
        bl_devices::operator::FILTER_ENV,
        bl_devices::operator::SHAPED_SUSTAIN,
    );
    assert_eq!(
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .params
            .get(&sustain)
            .copied(),
        Some(-100.0)
    );
}
