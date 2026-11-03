//! Meld's panel (WP-8.20; the meld spec, R63 to R69 and R73), headless: the
//! Matrix, MIDI and MPE tabs' grids for the engine shown, a touch that adds
//! a row, Copy to the other engine and X, the Expanded View, the displays,
//! the accessibility tree and the layout at every zoom.

use crate::common;
use bl_devices::meld::{FILTER_FREQ, PAN, param};
use bl_model::{DeviceId, ParamId};
use bl_shell::device_commands::{DeviceFrame, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place_shown, place_with_pages};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn window() -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let t = bl_devices::meld::meld_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    h.shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    h.settle();
    (h, device)
}

fn frame_state(h: &Harness, device: DeviceId) -> DeviceFrame {
    h.shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
}

/// The device's body and what the layout places in it.
fn body(h: &mut Harness, device: DeviceId) -> (Rect, DeviceFrame) {
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
    let shown = frame_state(h, device);
    let mut body = body_rect(frame, TITLE_BAR_HEIGHT);
    body.min.x += bl_shell::device_commands::sidechain_width(&shown);
    (body, shown)
}

fn page(h: &mut Harness, device: DeviceId, node: &str, page: &str) {
    h.shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": node, "page": page }),
        )
        .unwrap();
    h.settle();
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn centre(rect: Rect) -> Pos2 {
    rect.center()
}

fn click(h: &mut Harness, at: Pos2) {
    let event = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![event(true)]);
    h.frame(vec![event(false)]);
    h.settle();
}

fn drag(h: &mut Harness, from: Pos2, by: egui::Vec2) {
    let press = |at: Pos2, pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for i in 1..=10 {
        h.frame(vec![Event::PointerMoved(from + by * (i as f32 / 10.0))]);
    }
    h.frame(vec![press(from + by, false)]);
    h.settle();
}

/// Where the node called `label` is drawn.
fn node_rect(h: &mut Harness, label: &str) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, label)
        .bounds
        .unwrap_or_else(|| panic!("{label} has no place"));
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn routes(h: &Harness, device: DeviceId) -> Vec<(String, String, f64)> {
    let device = h.shell.editor().document().device(device).unwrap().clone();
    h.shell
        .device_matrix(&device)
        .unwrap()
        .slots
        .into_iter()
        .filter_map(|slot| {
            Some((
                slot.source?.to_owned(),
                slot.destination?.to_owned(),
                slot.amount,
            ))
        })
        .collect()
}

fn rows(h: &Harness, device: DeviceId) -> Vec<String> {
    frame_state(h, device)
        .panel
        .unwrap()
        .matrix_grid
        .unwrap()
        .rows
        .into_iter()
        .map(|row| row.destination)
        .collect()
}

fn add_route(h: &mut Harness, device: DeviceId, source: &str, destination: &str, amount: f64) {
    h.shell
        .run(
            "device.set_route",
            json!({ "device": device, "source": source, "destination": destination,
                    "amount": amount }),
        )
        .unwrap();
    h.settle();
}

// Spec: meld R63
#[test]
fn a_drag_in_the_matrix_tabs_grid_sets_an_amount_in_one_step_and_the_tab_shows_the_engine_shown() {
    let (mut h, device) = window();
    page(&mut h, device, "A Tabs", "Matrix");
    h.shell
        .run(
            "device.add_destination",
            json!({ "device": device, "destination": "A Filter Frequency" }),
        )
        .unwrap();
    let cell = node_rect(&mut h, "A LFO 1 to A Filter Frequency");
    let before = steps(&h);
    drag(&mut h, centre(cell), vec2(0.0, -25.0));
    let taken = routes(&h, device);
    assert_eq!(taken.len(), 1);
    assert_eq!(
        (taken[0].0.as_str(), taken[0].1.as_str()),
        ("A LFO 1", "A Filter Frequency")
    );
    assert!((30.0..=50.0).contains(&taken[0].2), "{}", taken[0].2);
    assert_eq!(
        steps(&h),
        before + 1,
        "the slot and the amount are one step"
    );
    assert_eq!(last_step(&h), "Change Modulation Route");
    // B's tab doesn't show A's destination row, and A's columns are A's.
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes
            .iter()
            .any(|n| n.label == "B LFO 1 to A Filter Frequency")
    );
    page(&mut h, device, "Engine", "B");
    page(&mut h, device, "B Tabs", "Matrix");
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::Row && n.label == "A Filter Frequency"),
        "engine B's grid shows B's rows"
    );
    // The MIDI tab has the shared sources as columns, the same row.
    page(&mut h, device, "Engine", "A");
    page(&mut h, device, "A Tabs", "MIDI");
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Velocity to A Filter Frequency"),
        "the row shows on the MIDI grid"
    );
    page(&mut h, device, "A Tabs", "MPE");
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "MPE Slide to A Filter Frequency")
    );
}

// Spec: meld R63
#[test]
fn touching_a_control_while_a_grid_tab_shows_adds_its_row_and_an_unrouted_row_leaves() {
    let (mut h, device) = window();
    page(&mut h, device, "A Tabs", "Matrix");
    assert!(rows(&h, device).is_empty());
    let knob = |h: &mut Harness, id: ParamId| {
        let (body, shown) = body(h, device);
        let (controls, ..) = place_with_pages(shown.panel.as_ref().unwrap(), shown.expanded, body);
        controls
            .into_iter()
            .find(|(p, ..)| *p == id)
            .map(|(.., rect)| rect)
            .unwrap_or_else(|| panic!("no control {id:?}"))
    };
    let before = steps(&h);
    let at = centre(knob(&mut h, param(0, FILTER_FREQ)));
    click(&mut h, at);
    assert_eq!(rows(&h, device), ["A Filter Frequency"]);
    assert_eq!(steps(&h), before, "a row is view state, no undo step");
    // Another touch replaces the unrouted row.
    let at = centre(knob(&mut h, param(0, PAN)));
    click(&mut h, at);
    assert_eq!(rows(&h, device), ["A Pan"]);
    // A routed row stays when another is added.
    add_route(&mut h, device, "A LFO 2", "A Pan", 30.0);
    let at = centre(knob(&mut h, param(0, FILTER_FREQ)));
    click(&mut h, at);
    assert_eq!(rows(&h, device), ["A Filter Frequency", "A Pan"]);
    // With the Settings tab shown, a touch adds nothing.
    page(&mut h, device, "A Tabs", "Settings");
    h.shell
        .run(
            "device.remove_destination",
            json!({ "device": device, "destination": "A Filter Frequency" }),
        )
        .unwrap();
    let at = centre(knob(&mut h, param(0, FILTER_FREQ)));
    click(&mut h, at);
    assert_eq!(rows(&h, device), ["A Pan"]);
}

// Spec: meld R63
#[test]
fn adding_a_row_on_one_engines_grid_keeps_the_other_engines_unrouted_rows() {
    let (mut h, device) = window();
    page(&mut h, device, "A Tabs", "Matrix");
    for destination in ["A Filter Frequency", "B Pan"] {
        h.shell
            .run(
                "device.add_destination",
                json!({ "device": device, "destination": destination }),
            )
            .unwrap();
    }
    assert_eq!(rows(&h, device), ["A Filter Frequency", "B Pan"]);
    // A second row on A replaces A's unrouted row and leaves B's.
    h.shell
        .run(
            "device.add_destination",
            json!({ "device": device, "destination": "A Tone" }),
        )
        .unwrap();
    assert_eq!(rows(&h, device), ["A Tone", "B Pan"]);
}

// Spec: meld R64
#[test]
fn copy_to_the_other_engine_and_x_run_their_commands_as_one_step_each() {
    let (mut h, device) = window();
    page(&mut h, device, "A Tabs", "Matrix");
    add_route(&mut h, device, "A LFO 1", "A Filter Frequency", 40.0);
    add_route(&mut h, device, "Velocity", "A Volume", 30.0);
    let before = steps(&h);
    let copy = node_rect(&mut h, "Copy to B");
    click(&mut h, centre(copy));
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Copy Modulation");
    let after = routes(&h, device);
    assert!(
        after.contains(&("B LFO 1".to_owned(), "B Filter Frequency".to_owned(), 40.0)),
        "{after:?}"
    );
    assert!(after.contains(&("Velocity".to_owned(), "B Volume".to_owned(), 30.0)));
    // X clears the engine shown's routes, B's untouched.
    let clear = node_rect(&mut h, "Clear A Modulation");
    click(&mut h, centre(clear));
    assert_eq!(last_step(&h), "Clear Modulation");
    let left = routes(&h, device);
    assert_eq!(left.len(), 2);
    assert!(
        left.iter()
            .all(|(_, destination, _)| destination.starts_with("B "))
    );
    // Engine B's tab has Copy to A.
    page(&mut h, device, "Engine", "B");
    page(&mut h, device, "B Tabs", "Matrix");
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Button && n.label == "Copy to A")
    );
}

// Spec: meld R66
#[test]
fn the_expanded_view_shows_every_route_of_both_engines_and_widens_only_the_view() {
    let (mut h, device) = window();
    add_route(&mut h, device, "A LFO 1", "A Filter Frequency", 40.0);
    add_route(&mut h, device, "B LFO 2", "B Pan", -20.0);
    add_route(&mut h, device, "Velocity", "Volume", 10.0);
    let width = |h: &mut Harness| {
        let (body, _) = body(h, device);
        body.width()
    };
    let docked = width(&mut h);
    assert!(frame_state(&h, device).expandable);
    let steps_before = steps(&h);
    h.shell
        .run("device.expand", json!({ "device": device }))
        .unwrap();
    h.settle();
    let wider = width(&mut h);
    assert!(wider > docked + 100.0, "the part sits right of the body");
    let nodes = Harness::nodes(&h.settle());
    for label in [
        "A LFO 1 to A Filter Frequency",
        "B LFO 2 to B Pan",
        "Velocity to Volume",
    ] {
        assert!(nodes.iter().any(|n| n.label == label), "{label}");
    }
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Table && n.label == "Expanded View")
    );
    h.shell
        .run("device.expand", json!({ "device": device }))
        .unwrap();
    h.settle();
    assert_eq!(
        width(&mut h),
        docked,
        "hidden, the device is as wide as before"
    );
    assert_eq!(steps(&h), steps_before, "view state, no undo step");
}

// Spec: meld R67, R73
#[test]
fn the_accessibility_tree_names_every_tab_cell_and_display() {
    let (mut h, device) = window();
    let nodes = Harness::nodes(&h.settle());
    for tab in [
        "Envelopes",
        "LFOs",
        "Matrix",
        "MIDI",
        "MPE",
        "Settings",
        "A",
        "B",
    ] {
        assert!(
            nodes.iter().any(|n| n.role == Role::Tab && n.label == tab),
            "tab {tab}"
        );
    }
    let value = |nodes: &[common::Node], name: &str| {
        nodes
            .iter()
            .find(|n| n.role == Role::Image && n.label == name)
            .unwrap_or_else(|| panic!("no display {name}"))
            .value
            .clone()
    };
    assert!(value(&nodes, "A Oscillator").starts_with("A Oscillator Basic Shapes"));
    assert!(value(&nodes, "A Filter").contains("SVF 12dB"));
    assert!(value(&nodes, "A Amp Envelope").contains("Attack 1 ms"));
    assert!(value(&nodes, "A Mod Envelope").contains("loop None"));
    page(&mut h, device, "A Tabs", "LFOs");
    let nodes = Harness::nodes(&h.settle());
    assert!(value(&nodes, "A LFO 1").starts_with("A LFO 1 Basic Shapes at 1.00 Hz"));
    assert!(value(&nodes, "A LFO 1 FX").starts_with("A LFO 1 FX Off, then Off"));
    assert!(value(&nodes, "A LFO 2").starts_with("A LFO 2 Sine at 6.00 Hz"));
    for tab in ["Matrix", "MIDI", "MPE"] {
        page(&mut h, device, "A Tabs", tab);
        let nodes = Harness::nodes(&h.settle());
        let name = format!(
            "A {}",
            bl_devices::meld::GRID_CELLS[["Matrix", "MIDI", "MPE"]
                .iter()
                .position(|t| *t == tab)
                .unwrap()]
        );
        assert!(
            nodes
                .iter()
                .any(|n| n.role == Role::Table && n.label == name),
            "{name}"
        );
    }
}

// Spec: meld R69
#[test]
fn at_every_zoom_the_controls_and_displays_lie_inside_the_body_with_nothing_overlapping() {
    for percent in [50, 100, 200] {
        let (mut h, device) = window();
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        for engine in ["A", "B"] {
            page(&mut h, device, "Engine", engine);
            for tab in ["Envelopes", "LFOs", "Matrix", "MIDI", "MPE", "Settings"] {
                page(&mut h, device, &format!("{engine} Tabs"), tab);
                for expanded in [false, true] {
                    let shown = frame_state(&h, device);
                    if shown.expanded != expanded {
                        h.shell
                            .run("device.expand", json!({ "device": device }))
                            .unwrap();
                        h.settle();
                    }
                    let (body, shown) = body(&mut h, device);
                    let panel = shown.panel.as_ref().unwrap();
                    let (controls, cells, pages) = place_with_pages(panel, shown.expanded, body);
                    let mut rects: Vec<(String, Rect)> = controls
                        .into_iter()
                        .map(|(param, _, rect)| (format!("control {}", param.0), rect))
                        .chain(cells.into_iter().map(|(name, rect, _)| (name, rect)))
                        .collect();
                    rects.extend(pages.iter().flat_map(|p| {
                        p.buttons.iter().map(|r| {
                            (
                                format!("tab {}", p.name),
                                Rect::from_min_size(pos2(r.x, r.y), vec2(r.width, r.height)),
                            )
                        })
                    }));
                    assert!(rects.len() >= 5, "{engine} {tab} at {percent}%");
                    let at = format!("{engine} {tab} (expanded {expanded}) at {percent}%");
                    for (name, rect) in &rects {
                        assert!(
                            body.expand(0.5).contains_rect(*rect),
                            "{name} lies outside the body on {at}: {rect:?} in {body:?}"
                        );
                    }
                    for (i, (a, ra)) in rects.iter().enumerate() {
                        for (b, rb) in &rects[i + 1..] {
                            let overlap = ra.intersect(*rb);
                            assert!(
                                overlap.width() <= 0.01 || overlap.height() <= 0.01,
                                "{a} overlaps {b} on {at}: {ra:?} {rb:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    let _ = place_shown;
}
