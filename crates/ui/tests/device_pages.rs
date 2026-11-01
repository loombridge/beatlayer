//! Device pages in the generic UI (WP-8.1), headless: the tab row and the
//! shell's section buttons switch pages, each button carries a modulation
//! indicator, and the tabs are a screen reader's tab list with the arrow
//! keys. The device is the framework's test pages device.
//!
//! Spec: device-framework R41 to R44.

use crate::common;
use bl_devices::testing::{self, PAGES_SHELL, PAGES_TABS, PAGES_TITLE};
use bl_engine::{PlayedParam, target_node};
use bl_model::{AutomationTarget, DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, page_button_id, place_with_pages};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{FullOutput, Key, Modifiers, Pos2, Rect, Shape, pos2};
use serde_json::json;

fn harness() -> (Harness, DeviceId, TrackId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(testing::device_types());
    h.settle();
    let track: TrackId = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    h.settle();
    let t = testing::pages_type();
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

/// The device's controls, cells and pages nodes' buttons placed in its body.
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

fn placed(h: &mut Harness, device: DeviceId) -> Vec<bl_model::device_layout::PlacedPages> {
    placed_all(h, device).2
}

fn button(h: &mut Harness, device: DeviceId, node: &str, page: usize) -> Rect {
    let pages = placed(h, device);
    let r = pages.iter().find(|p| p.name == node).unwrap().buttons[page];
    Rect::from_min_size(pos2(r.x, r.y), egui::vec2(r.width, r.height))
}

fn shown(h: &Harness, device: DeviceId, node: &str) -> String {
    let frame = h
        .shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap();
    let pages = frame.panel.unwrap().pages;
    pages.into_iter().find(|p| p.node == node).unwrap().shown
}

// Spec: device-framework R41, R43
#[test]
fn a_tab_click_and_a_shell_section_click_switch_pages() {
    let (mut h, device, _) = harness();
    let steps = h.shell.editor().undo_history().len();
    let dirty = h.shell.is_dirty();
    assert_eq!(shown(&h, device, PAGES_TABS), "Osc");
    let filter = button(&mut h, device, PAGES_TABS, 1);
    h.click(filter.center());
    h.settle();
    assert_eq!(shown(&h, device, PAGES_TABS), "Filter");
    assert_eq!(
        shown(&h, device, PAGES_SHELL),
        "Env",
        "the other node stays"
    );
    let lfo = button(&mut h, device, PAGES_SHELL, 1);
    h.click(lfo.center());
    h.settle();
    assert_eq!(shown(&h, device, PAGES_SHELL), "LFO");
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");
    assert_eq!(h.shell.is_dirty(), dirty, "the set is unchanged");
    // The shown page's controls are drawn, a hidden one's are not.
    let nodes = Harness::nodes(&h.settle());
    for shown in ["P2", "P3", "P5", "P6", "P7", "P9"] {
        assert!(nodes.iter().any(|n| n.label == shown), "{shown} is drawn");
    }
    for hidden in ["P0", "P1", "P4", "P8", "P10"] {
        assert!(!nodes.iter().any(|n| n.label == hidden), "{hidden} is not");
    }
}

fn circles(output: &FullOutput) -> Vec<Pos2> {
    fn walk(shape: &Shape, out: &mut Vec<Pos2>) {
        match shape {
            Shape::Circle(c) if (c.radius - 2.5).abs() < 1e-3 => out.push(c.center),
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

// Spec: device-framework R44
#[test]
fn a_tab_lights_its_indicator_while_a_control_on_its_page_is_modulated() {
    let (mut h, device, track) = harness();
    // Stops the engine, so that what the test sets as modulated stays.
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.frame(vec![]);
    let corner = |rect: Rect| rect.right_top() + egui::vec2(-4.0, 4.0);
    let tabs: Vec<Rect> = (0..3)
        .map(|i| button(&mut h, device, PAGES_TABS, i))
        .collect();
    let sections: Vec<Rect> = (0..3)
        .map(|i| button(&mut h, device, PAGES_SHELL, i))
        .collect();
    let lit = |output: &FullOutput, rect: Rect| {
        circles(output)
            .into_iter()
            .any(|c| c.distance(corner(rect)) < 0.5)
    };
    let output = h.frame(vec![]);
    assert!(tabs.iter().chain(&sections).all(|r| !lit(&output, *r)));
    // P3 is on the Filter tab, hidden while Osc shows; P7 is the Out section's.
    let modulate = |h: &mut Harness, params: &[u32]| {
        h.shell.set_modulated(
            params
                .iter()
                .map(|&p| {
                    let target = AutomationTarget::Device {
                        device,
                        param: ParamId(p),
                    };
                    let (node, param) = target_node(track, &target).unwrap();
                    PlayedParam {
                        node,
                        param,
                        base: 0.0,
                        played: 50.0,
                    }
                })
                .collect(),
        );
    };
    modulate(&mut h, &[3, 7]);
    let output = h.frame(vec![]);
    assert!(lit(&output, tabs[1]), "Filter");
    assert!(!lit(&output, tabs[0]) && !lit(&output, tabs[2]));
    assert!(lit(&output, sections[2]), "Out");
    assert!(!lit(&output, sections[0]) && !lit(&output, sections[1]));
    modulate(&mut h, &[]);
    let output = h.frame(vec![]);
    assert!(tabs.iter().chain(&sections).all(|r| !lit(&output, *r)));
}

// Spec: device-framework R44
#[test]
fn the_tabs_are_a_tab_list_with_the_selected_tab_and_the_arrow_keys() {
    let (mut h, device, _) = harness();
    let nodes = Harness::nodes(&h.settle());
    let lists: Vec<&str> = nodes
        .iter()
        .filter(|n| n.role == Role::TabList)
        .map(|n| n.label.as_str())
        .collect();
    assert!(lists.contains(&PAGES_TABS) && lists.contains(&PAGES_SHELL));
    let selected = |nodes: &[common::Node], name: &str| {
        nodes
            .iter()
            .find(|n| n.role == Role::Tab && n.label == name)
            .unwrap_or_else(|| panic!("no tab {name}"))
            .selected
    };
    for name in ["Osc", "Env"] {
        assert_eq!(selected(&nodes, name), Some(true));
    }
    for name in ["Filter", "Amp", "LFO", "Out"] {
        assert_eq!(selected(&nodes, name), Some(false));
    }
    // Right moves the focus to the next tab, and Enter shows its page.
    h.ctx
        .memory_mut(|m| m.request_focus(page_button_id(device, PAGES_TABS, "Osc")));
    h.settle();
    let focus = |h: &Harness, page: &str| {
        h.ctx
            .memory(|m| m.has_focus(page_button_id(device, PAGES_TABS, page)))
    };
    assert!(focus(&h, "Osc"));
    h.key(Key::ArrowRight, Modifiers::NONE);
    h.settle();
    assert!(focus(&h, "Filter"));
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(shown(&h, device, PAGES_TABS), "Filter");
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(selected(&nodes, "Filter"), Some(true));
    assert_eq!(selected(&nodes, "Osc"), Some(false));
    h.key(Key::ArrowRight, Modifiers::NONE);
    h.key(Key::ArrowRight, Modifiers::NONE);
    h.settle();
    assert!(focus(&h, "Amp"), "it stops at the last");
}

// Spec: device-framework R41, R42
#[test]
fn every_page_is_laid_out_with_nothing_overlapping_at_50_100_and_200_percent() {
    let (mut h, device, _) = harness();
    for percent in [50, 100, 200] {
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        h.settle();
        for page in ["Osc", "Filter", "Amp"] {
            h.shell
                .invoke(
                    "device.set_page",
                    json!({ "device": device, "node": PAGES_TABS, "page": page }),
                )
                .unwrap();
            let (controls, cells, pages) = placed_all(&mut h, device);
            let mut rects: Vec<Rect> = pages
                .iter()
                .flat_map(|p| p.buttons.iter())
                .map(|r| Rect::from_min_size(pos2(r.x, r.y), egui::vec2(r.width, r.height)))
                .collect();
            assert_eq!(rects.len(), 6, "the title bar's tabs aren't in the body");
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

// Spec: device-framework R41, R44
#[test]
fn title_bar_tabs_sit_in_the_title_bar_and_switch_pages() {
    let (mut h, device, _) = harness();
    let nodes = Harness::nodes(&h.settle());
    let list = Harness::node(&nodes, PAGES_TITLE);
    assert_eq!(list.role, Role::TabList);
    let aux = nodes
        .iter()
        .find(|n| n.role == Role::Tab && n.label == "Aux")
        .unwrap();
    assert_eq!(aux.selected, Some(false));
    let b = aux.bounds.unwrap();
    let at = pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
    // The tab is above the body: in the title bar.
    let body_top = button(&mut h, device, PAGES_TABS, 0).top();
    assert!(
        at.y < body_top,
        "{at:?} is in the title bar, above the body at {body_top}"
    );
    h.click(at);
    h.settle();
    assert_eq!(shown(&h, device, PAGES_TITLE), "Aux");
    let nodes = Harness::nodes(&h.settle());
    let tab = |name: &str| {
        nodes
            .iter()
            .find(|n| n.role == Role::Tab && n.label == name)
            .unwrap()
            .selected
    };
    assert_eq!((tab("Aux"), tab("Main")), (Some(true), Some(false)));
}
