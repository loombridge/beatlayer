//! Outline View in Focus.

use crate::accessibility::helpers::*;
use crate::common::Harness;
use bl_ui::mixer::{Place, switch_id};
use egui::accesskit::Role;
use egui::{FullOutput, Key, Modifiers};
use serde_json::json;

/// Every rectangle drawn with the focus colour in a frame.
fn focus_strokes(output: &FullOutput, colour: egui::Color32) -> Vec<egui::Rect> {
    fn walk(shape: &egui::Shape, colour: egui::Color32, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.stroke.color == colour && rect.stroke.width > 0.0 => {
                out.push(rect.rect);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, colour, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, colour, &mut out);
    }
    out
}

// Spec: accessibility R7.
#[test]
fn outline_view_in_focus_outlines_the_focused_area_and_control_until_switched_off() {
    let (mut h, tracks, _) = set();
    let colour = h.theme.colors.focus.0;
    assert!(h.shell.outline_view_in_focus(), "on, as in Live 12.4.6");
    tab_navigation_on(&mut h);
    press(&mut h, Key::Num2, Modifiers::ALT);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    // The locator buttons and Automation Mode, the title bar, the five
    // In/Out controls, then the activator.
    for _ in 0..11 {
        press(&mut h, Key::Tab, Modifiers::NONE);
    }
    let output = h.settle();
    let strokes = focus_strokes(&output, colour);
    let drawn = |nodes: &[crate::common::Node], role: Role, label: &str| {
        let b = nodes
            .iter()
            .find(|n| n.role == role && n.label == label && n.bounds.is_some())
            .and_then(|n| n.bounds)
            .unwrap();
        egui::Rect::from_min_max(
            egui::pos2(b.x0 as f32, b.y0 as f32),
            egui::pos2(b.x1 as f32, b.y1 as f32),
        )
    };
    let around = |strokes: &[egui::Rect], area: egui::Rect| {
        strokes
            .iter()
            .any(|r| (r.min - area.min).length() < 1.5 && (r.max - area.max).length() < 1.5)
    };
    let nodes = Harness::nodes(&output);
    let main = drawn(&nodes, Role::Pane, "Arrangement View").shrink(1.0);
    assert!(
        around(&strokes, main),
        "a border round the focused view: {strokes:?}"
    );
    let activator = nodes
        .iter()
        .find(|n| {
            n.id == switch_id(
                Place::Header,
                bl_shell::mixer_controls::Switch::Activator,
                tracks[2],
            )
            .accesskit_id()
        })
        .and_then(|n| n.bounds)
        .unwrap();
    let activator = egui::Rect::from_min_max(
        egui::pos2(activator.x0 as f32, activator.y0 as f32),
        egui::pos2(activator.x1 as f32, activator.y1 as f32),
    );
    assert!(
        strokes
            .iter()
            .any(|r| r.contains_rect(activator) && r.width() < activator.width() + 6.0),
        "an outline round the focused control"
    );
    // The Control Bar has the focus: the border goes there.
    press(&mut h, Key::Num0, Modifiers::ALT);
    let output = h.settle();
    let bar = drawn(&Harness::nodes(&output), Role::Toolbar, "Control Bar").shrink(1.0);
    assert!(around(&focus_strokes(&output, colour), bar));
    assert!(!around(&focus_strokes(&output, colour), main));
    // Switched off, nothing shows where the focus is.
    h.shell
        .run("settings.set_outline_view_in_focus", json!({ "on": false }))
        .unwrap();
    let output = h.settle();
    assert!(focused_id(&h).is_some());
    assert!(
        focus_strokes(&output, colour).is_empty(),
        "nothing shows where the focus is"
    );
}
