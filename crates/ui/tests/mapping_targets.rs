//! What the map modes map across the views, in the window, headless (WP-7.3b;
//! the map modes spec, R11 to R22): the relative Session mapping strip, New,
//! the crossfader's fields, the step buttons, the Clip View's controls and
//! the scrub control, each shown in the right modes and driven by a mapping.

use crate::common;
use bl_midi_io::{Direction, TestBackend};
use bl_model::SceneId;
use bl_ui::layout::mixer::{SLOT_ROW, Sections, SessionLayout};
use bl_ui::layout::{DetailViews, Element, REFERENCE, ShellLayout};
use common::{Harness, Node, Window};
use egui::accesskit::Role;
use egui::{Key, Modifiers, Rect, pos2};
use serde_json::{Value, json};

fn rect_of(node: &Node) -> Rect {
    let b = node.bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

/// A manually paced harness on the Session View, with a MIDI port "Pads"
/// whose Remote switch is on.
fn session() -> (Harness, TestBackend) {
    let mut h = Harness::manual(Window::Main);
    h.settle();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Pads");
    h.shell.use_midi_backend(Box::new(test.clone()));
    h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Pads", "remote": true, "track": false }),
        )
        .unwrap();
    h.shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        )
        .unwrap();
    h.settle();
    (h, test)
}

fn tree(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

fn has(nodes: &[Node], label: &str) -> bool {
    nodes.iter().any(|n| n.label == label)
}

fn scenes(h: &Harness) -> Vec<SceneId> {
    h.shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect()
}

fn map(h: &mut Harness, source: Value, target: Value) {
    h.shell
        .run("remote.map", json!({ "source": source, "target": target }))
        .unwrap();
}

fn mode(h: &mut Harness, mode: &str) {
    h.shell
        .run("remote.map_mode", json!({ "mode": mode }))
        .unwrap();
    h.settle();
}

fn note(key: u8) -> Value {
    json!({ "type": "note", "channel": 1, "key": key })
}

fn strip(control: &str) -> Value {
    json!({ "type": "strip", "control": control })
}

/// Sends a note on and off from the port and lets the window take them.
fn tap(h: &mut Harness, test: &TestBackend, key: u8) {
    for status in [0x90, 0x80] {
        assert!(test.send("Pads", h.shell.midi().now(), &[status, key, 100]));
        h.shell.poll();
    }
    h.settle();
}

// Spec: map-modes R14, R3. In each mode the strip shows one slot row tall
// above the status row, with a Launch button in each track's column and the
// five controls in the Main column; leaving the mode takes it away.
#[test]
fn the_strip_shows_above_the_status_row_in_both_modes_and_is_gone_after() {
    let (mut h, _test) = session();
    let controls = [
        "Scene Up",
        "Scene Down",
        "Scene Number",
        "Launch Scene",
        "Cancel Launch",
    ];
    let before = tree(&mut h);
    assert!(controls.iter().all(|label| !has(&before, label)));
    for which in ["midi", "key"] {
        mode(&mut h, which);
        let nodes = tree(&mut h);
        let status = rect_of(Harness::node(&nodes, "Stop All Clips"));
        for label in controls {
            let rect = rect_of(Harness::node(&nodes, label));
            assert_eq!(
                rect.height().round(),
                SLOT_ROW - 2.0,
                "{label} is a slot row tall"
            );
            assert!(
                (rect.bottom() - status.top()).abs() <= 2.0,
                "{label} sits above the status row in {which}"
            );
        }
        let launches: Vec<&Node> = nodes
            .iter()
            .filter(|n| n.label.ends_with(" at the Highlighted Scene"))
            .collect();
        assert!(launches.len() >= 2, "a Launch button for each track");
        mode(&mut h, "off");
        let after = tree(&mut h);
        assert!(controls.iter().all(|label| !has(&after, label)), "{which}");
    }
}

// Spec: map-modes R14, R15. Each strip control takes a mapping, and through
// the remote input Scene Down moves the highlight, Launch Scene launches it
// and Select Next Scene on Launch moves it on, a track's Launch button
// presses its slot at the highlight, and Cancel Launch drops a queued scene
// launch and leaves what plays.
#[test]
fn the_strips_controls_map_and_act_through_the_remote_input() {
    let (mut h, test) = session();
    let scenes = scenes(&h);
    // The first MIDI track takes the clips.
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .unwrap()
        .id();
    for scene in &scenes[..2] {
        h.shell
            .run(
                "session.insert_empty_midi_clip",
                json!({ "track": midi, "scene": scene }),
            )
            .unwrap();
    }
    // Each control maps by a click and a note.
    mode(&mut h, "midi");
    for (key, (label, control)) in (36_u8..).zip([
        ("Scene Up", "scene_up"),
        ("Scene Down", "scene_down"),
        ("Launch Scene", "launch_scene"),
        ("Cancel Launch", "cancel_launch"),
    ]) {
        let nodes = tree(&mut h);
        h.click(rect_of(Harness::node(&nodes, label)).center());
        h.settle();
        assert_eq!(
            h.shell.run("remote.state", json!({})).unwrap()["selected"],
            strip(control),
            "a click selects {label}"
        );
        assert!(test.send("Pads", h.shell.midi().now(), &[0x90, key, 100]));
        h.shell.poll();
        h.settle();
    }
    let nodes = tree(&mut h);
    let launch = nodes
        .iter()
        .find(|n| n.label.starts_with("Launch ") && n.label.ends_with(" at the Highlighted Scene"))
        .map(|n| n.label.clone())
        .expect("a track's Launch button");
    let launch_rect = rect_of(Harness::node(&nodes, &launch));
    h.click(launch_rect.center());
    h.settle();
    h.shell
        .run(
            "remote.select",
            json!({ "target": { "type": "track_launch", "track": midi } }),
        )
        .unwrap();
    assert!(test.send("Pads", h.shell.midi().now(), &[0x90, 40, 100]));
    h.shell.poll();
    h.settle();
    let mapped = h.shell.run("remote.mappings", json!({})).unwrap();
    assert_eq!(mapped["mappings"].as_array().unwrap().len(), 5);
    mode(&mut h, "off");

    // The notes act: Scene Down highlights the first scene, then the second;
    // Scene Up goes back.
    tap(&mut h, &test, 37);
    assert_eq!(h.shell.highlighted_scene(), Some(scenes[0]));
    tap(&mut h, &test, 37);
    assert_eq!(h.shell.highlighted_scene(), Some(scenes[1]));
    tap(&mut h, &test, 36);
    assert_eq!(h.shell.highlighted_scene(), Some(scenes[0]));

    // Launch Scene launches the highlighted scene, and the highlight moves
    // on.
    let playing = |h: &Harness, scene: SceneId| {
        h.shell.session_states().iter().any(|slot| {
            slot.track == midi && slot.scene == scene && slot.state == bl_engine::SlotState::Playing
        })
    };
    tap(&mut h, &test, 38);
    h.wait("the highlighted scene to launch", |h| playing(h, scenes[0]));
    assert_eq!(
        h.shell.highlighted_scene(),
        Some(scenes[1]),
        "Select Next Scene on Launch moved the highlight on"
    );

    // The track's Launch button presses the slot at the highlight.
    tap(&mut h, &test, 40);
    h.wait("the slot at the highlight to launch", |h| {
        playing(h, scenes[1])
    });

    // Cancel Launch drops a launch that is queued and leaves what plays.
    h.shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "8_bars" }),
        )
        .unwrap();
    h.shell
        .run("session.launch_scene", json!({ "scene": scenes[0] }))
        .unwrap();
    h.wait("a launch to queue", |h| {
        h.shell
            .session_states()
            .iter()
            .any(|slot| slot.state == bl_engine::SlotState::Queued)
    });
    tap(&mut h, &test, 39);
    h.wait("the queued launch to drop", |h| {
        h.shell
            .session_states()
            .iter()
            .all(|slot| slot.state != bl_engine::SlotState::Queued)
    });
    assert!(playing(&h, scenes[1]), "the clip that played plays on");
}

// Spec: map-modes R16. With forty scenes the highlighted scene sits at the
// slot area's centre after a mapped Scene Down, and a click on a scene
// doesn't move the grid.
#[test]
fn a_mapped_scene_down_centres_the_highlighted_scene_in_forty_scenes() {
    let (mut h, test) = session();
    for _ in scenes(&h).len()..40 {
        h.shell.run("session.insert_scene", json!({})).unwrap();
    }
    assert_eq!(scenes(&h).len(), 40);
    map(&mut h, note(36), strip("scene_down"));
    h.settle();
    let slots_height = {
        let main = common::layout(REFERENCE, true, true).main_view;
        SessionLayout::compute(main, Some(Sections::DEFAULT))
            .slots
            .height()
    };
    let scroll = |h: &mut Harness| -> f32 {
        h.shell.run("session.state", json!({})).unwrap()["scroll_y"]
            .as_f64()
            .unwrap() as f32
    };
    for step in 1..=20_usize {
        tap(&mut h, &test, 36);
        let expected =
            ((step as f32 - 0.5) * SLOT_ROW - slots_height / 2.0).clamp(0.0, 39.0 * SLOT_ROW);
        assert!(
            (scroll(&mut h) - expected).abs() < 0.5,
            "step {step}: {} against {expected}",
            scroll(&mut h)
        );
    }
    // With a map mode on the strip takes a slot row from the area, and the
    // next request centres within what is left.
    mode(&mut h, "midi");
    let with_strip = {
        let main = common::layout(REFERENCE, true, true).main_view;
        SessionLayout::compute(main, Some(Sections::DEFAULT))
            .with_strip()
            .slots
            .height()
    };
    assert_eq!(with_strip, slots_height - SLOT_ROW);
    mode(&mut h, "off");
}

// Spec: map-modes R20. New shows before Session Record in both modes and
// nowhere else; mapped to a key it stops the armed tracks' clips and
// highlights or inserts a scene.
#[test]
fn the_new_button_shows_only_in_the_map_modes_and_a_mapped_key_runs_it() {
    let (mut h, _test) = session();
    assert!(!has(&tree(&mut h), "New"), "hidden outside the map modes");
    let shown = ShellLayout::compute_with_detail(
        REFERENCE,
        true,
        true,
        None,
        None,
        DetailViews::default().with_new_button(true),
    );
    assert!(shown.rect(Element::New).width() > 0.0);
    assert!(shown.rect(Element::New).right() <= shown.rect(Element::SessionRecord).left());
    for which in ["midi", "key"] {
        mode(&mut h, which);
        let nodes = tree(&mut h);
        let new = rect_of(Harness::node(&nodes, "New"));
        let record = rect_of(Harness::node(&nodes, "Session Record"));
        assert!(
            new.right() <= record.left(),
            "{which}: before Session Record"
        );
        mode(&mut h, "off");
    }

    // Mapped to a key, it stops the armed track's clip and highlights the
    // first scene with a free slot.
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .unwrap()
        .id();
    let scenes = scenes(&h);
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": midi, "scene": scenes[0] }),
        )
        .unwrap();
    h.shell
        .run("mixer.set_arm", json!({ "track": midi, "arm": true }))
        .unwrap();
    h.shell
        .run(
            "remote.map",
            json!({ "source": { "type": "key", "key": "N" },
                    "target": { "type": "bar", "control": "new" } }),
        )
        .unwrap();
    h.shell
        .run(
            "session.launch",
            json!({ "track": midi, "scene": scenes[0] }),
        )
        .unwrap();
    h.shell
        .run("session.highlight_scene", json!({ "number": 1 }))
        .unwrap();
    h.key(Key::N, Modifiers::NONE);
    assert_eq!(h.shell.highlighted_scene(), Some(scenes[1]));
}

// Spec: map-modes R19, R3. The crossfader's three fields show over it in
// both modes and not outside them, and Key Map Mode leaves the slider
// untinted but tints its fields.
#[test]
fn the_crossfaders_fields_show_in_both_modes_and_key_map_mode_leaves_the_slider() {
    let (mut h, _test) = session();
    let fields = ["Crossfader Left", "Crossfader Centre", "Crossfader Right"];
    // The Crossfader section is hidden by default (L-MIX-006).
    h.shell.run("mixer_view.crossfader", json!({})).unwrap();
    let outside = tree(&mut h);
    assert!(has(&outside, "Crossfader"));
    assert!(fields.iter().all(|label| !has(&outside, label)));
    for which in ["midi", "key"] {
        mode(&mut h, which);
        let output = h.settle();
        let nodes = Harness::nodes(&output);
        let rects: Vec<Rect> = fields
            .iter()
            .map(|label| rect_of(Harness::node(&nodes, label)))
            .collect();
        let slider = rect_of(Harness::node(&nodes, "Crossfader"));
        assert!(rects[0].left() <= slider.left() + 1.0);
        assert!(rects[2].right() >= slider.right() - 1.0);
        assert!(rects[0].right() <= rects[1].left() && rects[1].right() <= rects[2].left());
        let colour = if which == "key" {
            h.theme.colors.key_map.0
        } else {
            h.theme.colors.midi_map.0
        }
        .gamma_multiply(0.3);
        let painted = common::painted_rects(&output);
        let tinted = |rect: Rect| {
            painted
                .iter()
                .any(|(r, fill)| r.contains(rect.center()) && *fill == colour && r.width() < 20.0)
        };
        assert!(tinted(rects[1]), "{which}: the fields are tinted");
        mode(&mut h, "off");
    }
}

// Spec: map-modes R11, R12. Key Map Mode leaves the continuous controls
// untinted and tints the rest.
#[test]
fn key_map_mode_leaves_continuous_controls_untinted() {
    let (mut h, _test) = session();
    let tempo = Element::Tempo;
    let metronome = Element::Metronome;
    let layout = common::layout(REFERENCE, true, true);
    mode(&mut h, "key");
    let output = h.settle();
    let painted = common::painted_rects(&output);
    let tint = h.theme.colors.key_map.0.gamma_multiply(0.3);
    let tinted = |element: Element| {
        let rect = layout.rect(element);
        painted
            .iter()
            .any(|(r, fill)| *fill == tint && r.contains(rect.center()))
    };
    assert!(tinted(metronome), "the Metronome switch is tinted");
    assert!(!tinted(tempo), "the Tempo field keeps its look");
    mode(&mut h, "midi");
    let output = h.settle();
    let painted = common::painted_rects(&output);
    let tint = h.theme.colors.midi_map.0.gamma_multiply(0.3);
    let rect = layout.rect(tempo);
    assert!(
        painted
            .iter()
            .any(|(r, fill)| *fill == tint && r.contains(rect.center())),
        "MIDI Map Mode tints the Tempo field"
    );
}

// Spec: map-modes R11. The Control Bar's mappable controls take a click that
// selects them; the map switches and the Computer MIDI Keyboard aren't
// mappable.
#[test]
fn a_click_on_a_control_bar_control_selects_it_and_the_switches_are_not_mappable() {
    let (mut h, _test) = session();
    let layout = common::layout(REFERENCE, true, true);
    mode(&mut h, "midi");
    h.click(layout.rect(Element::Metronome).center());
    h.settle();
    assert_eq!(
        h.shell.run("remote.state", json!({})).unwrap()["selected"],
        json!({ "type": "bar", "control": "metronome" })
    );
    assert!(
        !h.shell.editor().document().settings().metronome.enabled,
        "the click didn't switch it"
    );
    let output = h.settle();
    let painted = common::painted_rects(&output);
    let tint = h.theme.colors.midi_map.0.gamma_multiply(0.3);
    let rect = layout.rect(Element::ComputerKeyboard);
    assert!(
        !painted
            .iter()
            .any(|(r, fill)| *fill == tint && r.contains(rect.center())),
        "the Computer MIDI Keyboard isn't tinted"
    );
}

/// A harness showing an empty MIDI clip of the first MIDI track in a Session
/// slot, selected, with the Clip View open on its Launch Properties.
fn session_clip() -> Harness {
    let (mut h, _test) = session();
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .unwrap()
        .id();
    let scene = scenes(&h)[0];
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": midi, "scene": scene }),
        )
        .unwrap();
    h.shell
        .run("session.select", json!({ "track": midi, "scene": scene }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("clip_view.show_properties", json!({ "panel": "launch" }))
        .unwrap();
    h.settle();
    h
}

// Spec: map-modes R21, R22, R11; launch quantisation R35. The Launch
// Properties panel's controls are tinted in both modes, the scrub control
// shows between the nudge buttons in MIDI Map Mode alone, and the step
// buttons show in the note editor before the grid chooser in both.
#[test]
fn the_clip_views_controls_tint_the_scrub_shows_in_midi_map_mode_only_and_the_step_buttons_show() {
    let mut h = session_clip();
    let before = tree(&mut h);
    for label in ["Scrub", "Step Back", "Step Forward"] {
        assert!(
            !has(&before, label),
            "{label} is hidden outside the map modes"
        );
    }
    for which in ["midi", "key"] {
        mode(&mut h, which);
        let output = h.settle();
        let nodes = Harness::nodes(&output);
        let back = rect_of(Harness::node(&nodes, "Nudge Backward"));
        let forward = rect_of(Harness::node(&nodes, "Nudge Forward"));
        assert_eq!(has(&nodes, "Scrub"), which == "midi", "Scrub in {which}");
        if which == "midi" {
            let scrub = rect_of(Harness::node(&nodes, "Scrub"));
            assert!(back.right() <= scrub.left() && scrub.right() <= forward.left());
        }
        // The step buttons sit before the grid chooser.
        let grid = rect_of(Harness::node(&nodes, "Grid"));
        for label in ["Step Back", "Step Forward"] {
            assert!(
                rect_of(Harness::node(&nodes, label)).right() <= grid.left(),
                "{label}"
            );
        }
        // The Launch Mode chooser and the nudge buttons are tinted; the
        // Velocity field is a continuous control, red mapping leaves it.
        let colour = if which == "key" {
            h.theme.colors.key_map.0
        } else {
            h.theme.colors.midi_map.0
        }
        .gamma_multiply(0.3);
        let painted = common::painted_rects(&output);
        let tinted = |rect: Rect| {
            painted
                .iter()
                .any(|(r, fill)| *fill == colour && r.contains(rect.center()))
        };
        assert!(
            tinted(back) && tinted(forward),
            "{which}: the nudge buttons"
        );
        let mode_chooser = rect_of(Harness::node(&nodes, "Launch Mode"));
        assert!(tinted(mode_chooser), "{which}: Launch Mode");
        // The panel's field reads a percentage; the editor's lane has the
        // same name.
        let velocity = rect_of(
            nodes
                .iter()
                .find(|n| n.label == "Velocity" && n.value.ends_with('%'))
                .expect("the Velocity field"),
        );
        assert_eq!(tinted(velocity), which == "midi", "{which}: Velocity");
        mode(&mut h, "off");
    }
    let after = tree(&mut h);
    assert!(!has(&after, "Scrub") && !has(&after, "Step Back"));
}

// Spec: macros R34. A radio-button macro is tinted red in Key Map Mode and
// takes a key, which then steps it to its next button; a knob macro keeps its
// look in Key Map Mode.
#[test]
fn a_radio_button_macro_takes_a_key_and_a_knob_macro_does_not() {
    let (mut h, _test) = session();
    let audio = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let rack = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "audio_effect" } },
                "name": "Rack",
                "role": "audio_effect",
            } }),
        )
        .unwrap()["device"]
        .clone();
    h.shell
        .run(
            "rack.set_macro_style",
            json!({ "rack": rack, "macro_number": 1, "style": "radio_buttons" }),
        )
        .unwrap();
    h.settle();
    let radio = json!({ "type": "device", "device": rack, "param": bl_model::macro_param(1) });
    let knob = json!({ "type": "device", "device": rack, "param": bl_model::macro_param(2) });
    mode(&mut h, "key");
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let painted = common::painted_rects(&output);
    let tint = h.theme.colors.key_map.0.gamma_multiply(0.3);
    let tinted = |label: &str| {
        let group = nodes
            .iter()
            .find(|n| n.label == label && n.role == Role::RadioGroup)?;
        let button = nodes.iter().find(|n| {
            n.role == Role::RadioButton && Harness::descends_from(&nodes, n.id, group.id)
        })?;
        let rect = rect_of(button);
        Some(
            painted
                .iter()
                .any(|(r, fill)| *fill == tint && r.contains(rect.center())),
        )
    };
    assert_eq!(
        tinted("Macro 1"),
        Some(true),
        "the radio-button macro is tinted red"
    );
    // Key Map Mode takes the radio macro by `remote.select`, and not the knob.
    h.shell
        .run("remote.select", json!({ "target": radio }))
        .unwrap();
    assert!(
        h.shell
            .run("remote.select", json!({ "target": knob }))
            .is_err(),
        "a knob macro is continuous: MIDI only"
    );
    h.key(Key::R, Modifiers::NONE);
    let mapped = h.shell.run("remote.mappings", json!({})).unwrap();
    assert_eq!(
        mapped["mappings"][0]["source"],
        json!({ "type": "key", "key": "R" })
    );
    assert_eq!(mapped["mappings"][0]["kind"]["type"], "radio");
    mode(&mut h, "off");
    let value = |h: &Harness| {
        let rack: bl_model::DeviceId = serde_json::from_value(rack.clone()).unwrap();
        h.shell
            .editor()
            .document()
            .device(rack)
            .unwrap()
            .params
            .get(&bl_model::macro_param(1))
            .copied()
            .unwrap_or(0.0)
    };
    let first = value(&h);
    h.key(Key::R, Modifiers::NONE);
    assert!(
        value(&h) > first,
        "the key steps the macro to its next button"
    );
}

// Spec: map-modes R23, R12. A MIDI Tool's apply button, Reset button and Auto
// switch are tinted in both modes, a number parameter in MIDI Map Mode only,
// and a click selects the control by the tool's and the control's names.
#[test]
fn a_midi_tools_controls_are_tinted_and_a_click_selects_one_by_name() {
    let mut h = session_clip();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
    h.shell
        .run("midi_tool.choose", json!({ "tool": "velocity_shaper" }))
        .unwrap();
    h.settle();
    for which in ["midi", "key"] {
        mode(&mut h, which);
        let output = h.settle();
        let nodes = Harness::nodes(&output);
        let colour = if which == "key" {
            h.theme.colors.key_map.0
        } else {
            h.theme.colors.midi_map.0
        }
        .gamma_multiply(0.3);
        let painted = common::painted_rects(&output);
        let tinted = |label: &str| {
            let rect = rect_of(Harness::node(&nodes, label));
            painted
                .iter()
                .any(|(r, fill)| *fill == colour && r.contains(rect.center()))
        };
        assert!(tinted("Transform"), "{which}: the apply button");
        assert!(tinted("Reset"), "{which}: Reset");
        assert!(tinted("Velocity Shaper Auto"), "{which}: Auto");
        assert_eq!(
            tinted("Maximum Velocity"),
            which == "midi",
            "{which}: a number parameter"
        );
        mode(&mut h, "off");
    }
    mode(&mut h, "midi");
    let nodes = tree(&mut h);
    h.click(rect_of(Harness::node(&nodes, "Transform")).center());
    h.settle();
    assert_eq!(
        h.shell.run("remote.state", json!({})).unwrap()["selected"],
        json!({ "type": "midi_tool", "tool": "velocity_shaper", "control": { "type": "apply" } })
    );
}
