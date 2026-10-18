//! Mapping a modulator in the UI (WP-5.10), headless: the Map button lights
//! and tints every parameter it can map, a click on one maps it as one undo
//! step, Esc puts the button out, Unmap removes the mapping, and the Curve
//! Mod's display adds, moves, curves and deletes breakpoints. Spec:
//! modulation R15, R19, R23.

mod common;

use bl_devices::testing::{self, UTILITY_GAIN};
use bl_model::DeviceId;
use bl_model::modulator::{ModTarget, ShapePoint, default_shape};
use bl_shell::device_commands::DeviceAdded;
use bl_ui::device_panel::{body_rect, place};
use bl_ui::device_view::layout;
use common::{Harness, Node, Window};
use egui::{Event, FullOutput, Key, Modifiers, PointerButton, Pos2, Rect, Shape, pos2, vec2};
use serde_json::json;

fn rect_of_node(node: &Node) -> Rect {
    let b = node.bounds.expect("a node with bounds");
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

struct Rig {
    h: Harness,
    wave: DeviceId,
    utility: DeviceId,
}

fn add(h: &mut Harness, kind: &bl_model::DeviceKind, name: &str) -> DeviceId {
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": kind, "name": name, "role": "audio_effect" } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

/// A track holding a test utility and a LFO, shown in the Device View.
fn rig() -> Rig {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.frame(vec![]);
    let mut types = testing::device_types();
    types.extend(bl_devices::device_types());
    h.shell.set_device_types(types);
    h.settle();
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let utility_type = testing::utility_type();
    let utility = add(&mut h, &utility_type.kind, &utility_type.name);
    let wave_type = bl_devices::modulator::wave_mod_type();
    let wave = add(&mut h, &wave_type.kind, &wave_type.name);
    h.settle();
    Rig { h, wave, utility }
}

fn node_rect(h: &mut Harness, label: &str) -> Rect {
    let nodes = Harness::nodes(&h.frame(vec![]));
    rect_of_node(Harness::node(&nodes, label))
}

fn mappings(h: &Harness, device: DeviceId) -> Vec<bl_model::modulator::Mapping> {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .mappings
        .clone()
}

/// The rectangle of the utility's gain knob.
fn gain_knob(rig: &mut Rig) -> Rect {
    let nodes = Harness::nodes(&rig.h.settle());
    let view = rect_of_node(Harness::node(&nodes, "Device View"));
    let frame = layout(&rig.h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == rig.utility)
        .unwrap()
        .1;
    let panel = rig
        .h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == rig.utility)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    place(
        &panel,
        body_rect(frame, bl_shell::device_commands::TITLE_BAR_HEIGHT),
    )
    .0
    .into_iter()
    .find(|(p, _, _)| *p == UTILITY_GAIN)
    .unwrap()
    .2
}

fn shapes(output: &FullOutput) -> Vec<Shape> {
    fn walk(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Whether anything inside `rect` was filled with the mapping tint.
fn tinted(h: &Harness, output: &FullOutput, rect: Rect) -> bool {
    let tint = h.theme.colors.mapping.0.gamma_multiply(0.3);
    shapes(output).into_iter().any(
        |s| matches!(s, Shape::Rect(r) if r.fill == tint && rect.contains_rect(r.rect.shrink(0.5))),
    )
}

// Spec: modulation R15
#[test]
fn map_arms_tints_and_maps_the_clicked_parameter_in_one_undo_step() {
    let mut rig = rig();
    let knob = gain_knob(&mut rig);
    let before = rig.h.frame(vec![]);
    assert!(
        !tinted(&rig.h, &before, knob),
        "nothing is tinted before Map is lit"
    );
    let map = node_rect(&mut rig.h, "Map 1");
    rig.h.click(map.center());
    assert_eq!(
        rig.h.shell.armed_map().map(|a| (a.device, a.slot)),
        Some((rig.wave, 0)),
        "the button is lit"
    );
    let lit = rig.h.frame(vec![]);
    assert!(
        tinted(&rig.h, &lit, knob),
        "another device's parameter is tinted"
    );
    // The modulator's own parameters aren't targets.
    let wave_frame = rig.h.shell.device_view_state();
    assert!(wave_frame.armed_map.is_some());
    // A click on the knob maps it.
    rig.h.click(knob.center());
    assert_eq!(
        mappings(&rig.h, rig.wave)[0].target,
        ModTarget::Device {
            device: rig.utility,
            param: UTILITY_GAIN
        }
    );
    assert!(
        rig.h.shell.armed_map().is_none(),
        "mapping puts the button out"
    );
    assert_eq!(
        rig.h.shell.editor().undo_history().next(),
        Some("Map Modulator"),
        "one undo step"
    );
    // The knob keeps its value: a click while lit didn't also turn it.
    let after = rig.h.frame(vec![]);
    assert!(
        !tinted(&rig.h, &after, knob),
        "the tint goes with the button"
    );
    rig.h.shell.run("edit.undo", json!({})).unwrap();
    assert!(mappings(&rig.h, rig.wave).is_empty());
    // The slot shows the target it maps.
    rig.h.shell.run("edit.redo", json!({})).unwrap();
    let state = rig.h.shell.device_view_state();
    let modulator = state
        .devices
        .iter()
        .find(|d| d.id == rig.wave)
        .and_then(|d| d.modulator.clone())
        .unwrap();
    assert!(
        modulator.mappings[0].label.ends_with("/ Gain"),
        "{}",
        modulator.mappings[0].label
    );
}

// Spec: modulation R15
#[test]
fn esc_disarms_map() {
    let mut rig = rig();
    let map = node_rect(&mut rig.h, "Map 1");
    rig.h.click(map.center());
    assert!(rig.h.shell.armed_map().is_some());
    let undo = rig.h.shell.editor().undo_history().count();
    rig.h.key(Key::Escape, Modifiers::NONE);
    assert!(rig.h.shell.armed_map().is_none(), "Esc puts it out");
    assert_eq!(
        rig.h.shell.editor().undo_history().count(),
        undo,
        "and changes nothing"
    );
    assert!(mappings(&rig.h, rig.wave).is_empty());
    // The button again does the same.
    rig.h.click(map.center());
    assert!(rig.h.shell.armed_map().is_some());
    rig.h.click(map.center());
    assert!(rig.h.shell.armed_map().is_none());
}

// Spec: modulation R15
#[test]
fn unmap_removes_the_mapping() {
    let mut rig = rig();
    rig.h
        .shell
        .run(
            "modulator.map",
            json!({ "device": rig.wave, "slot": 0,
                    "target": { "type": "device", "device": rig.utility, "param": UTILITY_GAIN.0 } }),
        )
        .unwrap();
    rig.h.settle();
    let unmap = node_rect(&mut rig.h, "Unmap 1");
    rig.h.click(unmap.center());
    assert!(mappings(&rig.h, rig.wave).is_empty());
    assert_eq!(
        rig.h.shell.editor().undo_history().next(),
        Some("Unmap Modulator")
    );
}

/// The Shaper's shape display and its device.
fn curve_rig() -> (Rig, DeviceId, Rect) {
    let mut rig = rig();
    let curve_type = bl_devices::modulator::curve_mod_type();
    let curve = add(&mut rig.h, &curve_type.kind, &curve_type.name);
    let area = node_rect(&mut rig.h, "Curve Shape");
    (rig, curve, area)
}

fn shape(h: &Harness, device: DeviceId) -> Vec<ShapePoint> {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .shape
        .clone()
        .unwrap_or_else(default_shape)
}

fn drag(h: &mut Harness, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.frame(vec![
        Event::ModifiersChanged(modifiers),
        Event::PointerMoved(from),
    ]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers,
    }]);
    for i in 1..=4 {
        let at = from + (to - from) * (i as f32 / 4.0);
        h.frame(vec![Event::PointerMoved(at)]);
    }
    h.frame(vec![Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers,
    }]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
}

// Spec: modulation R23
#[test]
fn shaper_display_adds_moves_curves_and_deletes_breakpoints() {
    let (mut rig, curve, area) = curve_rig();
    assert_eq!(
        shape(&rig.h, curve).len(),
        3,
        "a new Shaper plays the triangle"
    );
    // The plot area sits above the cell's bottom controls.
    let plot = Rect::from_min_max(area.min, pos2(area.max.x, area.max.y));
    let at = |x: f32, y: f32| {
        pos2(
            plot.left() + plot.width() * x,
            plot.bottom() - plot.height() * y,
        )
    };
    // A click adds a breakpoint, one undo step.
    rig.h.click(at(0.25, 0.8));
    let added = shape(&rig.h, curve);
    assert_eq!(added.len(), 4);
    assert!(
        added
            .iter()
            .any(|p| (p.x - 0.25).abs() < 0.05 && (p.y - 0.8).abs() < 0.1)
    );
    assert_eq!(
        rig.h.shell.editor().undo_history().next(),
        Some("Edit Shape")
    );
    // Dragging a breakpoint moves it, as one step.
    let steps = rig.h.shell.editor().undo_history().count();
    let index = added
        .iter()
        .position(|p| (p.x - 0.25).abs() < 0.05)
        .unwrap();
    let from = at(added[index].x as f32, added[index].y as f32);
    drag(&mut rig.h, from, at(0.35, 0.3), Modifiers::NONE);
    let moved = shape(&rig.h, curve);
    assert!(
        (moved[index].x - 0.35).abs() < 0.06 && (moved[index].y - 0.3).abs() < 0.12,
        "{:?}",
        moved[index]
    );
    assert_eq!(
        rig.h.shell.editor().undo_history().count(),
        steps + 1,
        "one undo step for the drag"
    );
    // Alt-dragging a segment curves it.
    let mid = at(0.15, 0.2);
    drag(&mut rig.h, mid, mid + vec2(0.0, -30.0), Modifiers::ALT);
    assert!(
        shape(&rig.h, curve).iter().any(|p| p.bend != 0.0),
        "a segment curved"
    );
    // Shift-clicking a breakpoint deletes it.
    let now = shape(&rig.h, curve);
    let victim = now[index];
    let at_point = at(victim.x as f32, victim.y as f32);
    rig.h.frame(vec![
        Event::ModifiersChanged(Modifiers::SHIFT),
        Event::PointerMoved(at_point),
    ]);
    for pressed in [true, false] {
        rig.h.frame(vec![Event::PointerButton {
            pos: at_point,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::SHIFT,
        }]);
    }
    rig.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    assert_eq!(shape(&rig.h, curve).len(), now.len() - 1);
    // Clear empties it; a preset draws a new shape.
    let clear = node_rect(&mut rig.h, "Clear");
    rig.h.click(clear.center());
    assert!(shape(&rig.h, curve).is_empty());
    let preset = node_rect(&mut rig.h, "Shape preset 1");
    rig.h.click(preset.center());
    assert_eq!(
        shape(&rig.h, curve),
        bl_model::modulator::shape_preset(1).unwrap()
    );
}
