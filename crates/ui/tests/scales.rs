//! The Control Bar's scale controls (WP-6.7): the Scale Mode switch, the
//! Root Note chooser and the Scale Name chooser, live in place of pending,
//! running the commands that change a clip's scale or the set's current
//! scale. The behaviour is in the scales spec.

mod common;

use bl_model::{ClipId, Scale, ScaleName, TrackId};
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use bl_ui::main_window::element_id;
use common::{Harness, Node, Window};
use egui::{FullOutput, pos2};
use serde_json::json;

/// Where an element is drawn, in a harness without a drawn title bar.
fn rect(element: Element) -> egui::Rect {
    ShellLayout::compute(REFERENCE, true, true).rect(element)
}

/// A harness with a MIDI track and two clips on it, the first selected.
fn harness() -> (Harness, TrackId, ClipId, ClipId) {
    let mut h = Harness::new(Window::Main);
    let track: TrackId = h.shell.new_id();
    h.shell
        .run(
            "track.create",
            json!({
                "id": track,
                "kind": "midi",
                "name": "Keys",
                "parent": null,
                "index": 0,
            }),
        )
        .unwrap();
    let mut clips = Vec::new();
    for at in [0, 705_600] {
        let clip: ClipId = h.shell.new_id();
        h.shell
            .run(
                "clip.create",
                json!({
                    "track": track,
                    "clip": {
                        "id": clip,
                        "name": "",
                        "start": at,
                        "length": 705_600,
                        "offset": 0,
                        "mute": false,
                        "data": { "midi": { "notes": [] } },
                    },
                }),
            )
            .unwrap();
        clips.push(clip);
    }
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clips[0]] }))
        .unwrap();
    h.settle();
    (h, track, clips[0], clips[1])
}

/// The clip's own scale.
fn scale_of(h: &Harness, clip: ClipId) -> Scale {
    h.shell.editor().document().clip(clip).unwrap().scale
}

/// The node the Control Bar's element is, with what it reads.
fn node(output: &FullOutput, element: Element) -> Node {
    let nodes = Harness::nodes(output);
    nodes
        .iter()
        .find(|node| node.id == element_id(element).accesskit_id())
        .cloned()
        .unwrap_or_else(|| panic!("no node for {element:?}"))
}

/// Clicks the open menu's item labelled `label`.
fn choose(h: &mut Harness, output: &FullOutput, label: &str) {
    let nodes = Harness::nodes(output);
    let node = nodes
        .iter()
        .rfind(|node| {
            (node.label == label || node.label.starts_with(&format!("{label} ")))
                && node.bounds.is_some()
        })
        .unwrap_or_else(|| {
            let labels: Vec<_> = nodes.iter().map(|node| node.label.clone()).collect();
            panic!("no {label:?} in {labels:?}")
        });
    let bounds = node.bounds.unwrap();
    h.click(pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
}

#[test]
fn the_three_scale_controls_are_live_and_in_the_control_bar() {
    // Spec: scales R9, R14; layout.md L-CTRL-011.
    let (mut h, _track, _first, _second) = harness();
    let output = h.settle();
    for element in [Element::ScaleMode, Element::RootNote, Element::ScaleName] {
        let node = node(&output, element);
        assert!(!node.disabled, "{element:?} is disabled");
        assert!(node.bounds.is_some(), "{element:?} isn't drawn");
    }
    // The switch is a switch, and the choosers read their value.
    let switch = node(&output, Element::ScaleMode);
    assert_eq!(switch.toggled, Some(false), "the switch is off");
    let root = node(&output, Element::RootNote);
    assert_eq!(root.value, "C", "the root is C");
    let name = node(&output, Element::ScaleName);
    assert_eq!(name.value, "Major", "the scale is Major");
}

#[test]
fn the_scale_mode_switch_changes_the_selected_clips_scale() {
    // Spec: scales R12.
    let (mut h, _track, first, _second) = harness();
    let switch = rect(Element::ScaleMode).center();
    h.click(switch);
    h.settle();
    assert!(
        scale_of(&h, first).enabled,
        "the switch turned the clip's Scale Mode on"
    );
    h.click(switch);
    h.settle();
    assert!(!scale_of(&h, first).enabled, "and off again");
}

#[test]
fn the_root_note_chooser_lists_the_twelve_roots_and_sets_one() {
    // Spec: scales R9, R12.
    let (mut h, _track, first, _second) = harness();
    let output = h.click(rect(Element::RootNote).center());
    h.settle();
    assert!(egui::Popup::is_any_open(&h.ctx), "the chooser opened");
    let labels: Vec<String> = Harness::nodes(&output)
        .iter()
        .map(|node| node.label.clone())
        .collect();
    for root in [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ] {
        assert!(
            labels.iter().any(|label| label == root),
            "no {root} in {labels:?}"
        );
    }
    choose(&mut h, &output, "D");
    h.settle();
    assert_eq!(scale_of(&h, first).root, 2, "the root is D");
}

#[test]
fn the_scale_name_chooser_lists_the_scale_list_and_sets_one() {
    // Spec: scales R1, R9, R12.
    let (mut h, _track, first, _second) = harness();
    let output = h.click(rect(Element::ScaleName).center());
    h.settle();
    assert!(egui::Popup::is_any_open(&h.ctx), "the chooser opened");
    let labels: Vec<String> = Harness::nodes(&output)
        .iter()
        .map(|node| node.label.clone())
        .collect();
    for scale in ScaleName::ALL {
        assert!(
            labels.iter().any(|label| label == scale.label()),
            "no {} in {labels:?}",
            scale.label()
        );
    }
    choose(&mut h, &output, "Dorian");
    h.settle();
    assert_eq!(scale_of(&h, first).name, ScaleName::Dorian);
}

#[test]
fn a_mixed_selection_shows_an_asterisk_and_a_partly_filled_switch() {
    // Spec: scales R6, R14.
    let (mut h, _track, first, second) = harness();
    h.shell
        .run(
            "clip.set_scale",
            json!({ "clips": [first], "enabled": true, "root": 2, "name": "dorian" }),
        )
        .unwrap();
    h.shell
        .run(
            "clip.set_scale",
            json!({ "clips": [second], "enabled": false, "root": 0, "name": "major" }),
        )
        .unwrap();
    h.shell
        .run(
            "arrangement.select_clips",
            json!({ "clips": [first, second] }),
        )
        .unwrap();
    let output = h.settle();
    // The switch is partly filled: some of the clips have it on.
    let switch = node(&output, Element::ScaleMode);
    assert_eq!(switch.value, "mixed", "the switch reads mixed");
    // The choosers read as mixed.
    let root = node(&output, Element::RootNote);
    assert_eq!(root.value, "mixed", "the root reads mixed");
    let name = node(&output, Element::ScaleName);
    assert_eq!(name.value, "mixed", "the scale reads mixed");
}

#[test]
fn the_scale_controls_take_the_keyboard_focus() {
    // Spec: scales R14.
    let (mut h, _track, _first, _second) = harness();
    h.ctx
        .memory_mut(|memory| memory.request_focus(element_id(Element::ScaleMode)));
    h.shell.set_focused_area(bl_shell::Area::ControlBar);
    h.settle();
    assert_eq!(
        h.ctx.memory(|memory| memory.focused()),
        Some(element_id(Element::ScaleMode)),
        "the switch has the keyboard"
    );
}
