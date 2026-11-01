//! Roar's shaper display in the Device View (WP-8.38; the roar spec, R12 and
//! R19), headless: a vertical drag on the display changes the shown stage's
//! Shaper Amount as one undo step, and the stage's own view is the one that
//! moves.

use crate::common;
use bl_devices::roar::{AMOUNT, FB_NOTE, MIDI_FB_NOTE, ROAR, stage_param};
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_ui::device_panel::place_shown;
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

/// A harness with Beatlayer's own devices, and the engine off so the display
/// data a test sets stays.
fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.frame(vec![]);
    h
}

fn add_roar(h: &mut Harness) -> DeviceId {
    let types = bl_devices::device_types();
    let t = types
        .iter()
        .find(|t| matches!(&t.kind, bl_model::DeviceKind::Native { device_type } if device_type == ROAR))
        .unwrap();
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role, "midi_sidechain": true });
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    h.settle();
    added.device
}

/// The shaper display's cell in the Device View.
fn cell_of(h: &mut Harness, device: DeviceId) -> Rect {
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
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    let body = bl_ui::device_panel::body_rect(frame, bl_shell::device_commands::TITLE_BAR_HEIGHT);
    place_shown(&panel, false, body)
        .1
        .into_iter()
        .find(|(name, _, _)| name == "Shaper")
        .expect("the shaper display is drawn")
        .1
}

fn value(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
        .unwrap_or(0.0)
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Presses at `from` and drags 25 points up in five moves.
fn drag_up(h: &mut Harness, from: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for step in 1..=5 {
        h.frame(vec![Event::PointerMoved(
            from - vec2(0.0, 5.0 * step as f32),
        )]);
    }
    h.frame(vec![press(from - vec2(0.0, 25.0), false)]);
    h.settle();
}

// Spec: roar R12
#[test]
fn a_vertical_drag_on_the_shaper_display_changes_the_shown_stages_amount_in_one_undo_step() {
    let mut h = harness();
    let roar = add_roar(&mut h);
    let cell = cell_of(&mut h, roar);
    let steps = h.shell.editor().undo_history().len();
    drag_up(&mut h, cell.center());
    let first = value(&h, roar, stage_param(0, AMOUNT));
    assert!(
        first > 10.0 && first < 30.0,
        "25 points is a fifth of the range: {first}"
    );
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change \"Stage 1 Amount\"")
    );
    // The view the display shows is the stage that moves.
    h.shell
        .run(
            "device.set_display_view",
            json!({ "device": roar, "view": "Stage 2" }),
        )
        .unwrap();
    h.settle();
    drag_up(&mut h, cell.center());
    assert!(value(&h, roar, stage_param(1, AMOUNT)) > 10.0);
    assert_eq!(
        value(&h, roar, stage_param(0, AMOUNT)),
        first,
        "stage 1 stays"
    );
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change \"Stage 2 Amount\"")
    );
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        value(&h, roar, stage_param(1, AMOUNT)),
        0.0,
        "and it undoes"
    );
}

// Spec: roar R12, device-framework R38
#[test]
fn a_click_on_a_view_button_over_the_draggable_display_chooses_its_stage() {
    let mut h = harness();
    let roar = add_roar(&mut h);
    let cell = cell_of(&mut h, roar);
    // The second of three view buttons along the cell's top-right corner,
    // left of the 6-point meter.
    let right = cell.right() - 12.0;
    let at = pos2(right - 2.0 * 26.0 + 12.0, cell.top() + 9.0);
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![press(at, true)]);
    h.frame(vec![press(at, false)]);
    h.settle();
    let view = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == roar)
        .unwrap()
        .display_view
        .clone();
    assert_eq!(
        view.as_deref(),
        Some("Stage 2"),
        "the button takes the click"
    );
    assert_eq!(
        value(&h, roar, stage_param(0, AMOUNT)),
        0.0,
        "and the drag doesn't"
    );
}

/// The Device View's rectangle.
fn view_of(h: &mut Harness) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn frame_of(h: &mut Harness, device: DeviceId) -> Rect {
    let view = view_of(h);
    layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Where in `nodes` the node named `label` is, in the order the UI
/// announced them.
fn position(nodes: &[common::Node], label: &str) -> usize {
    nodes
        .iter()
        .position(|n| n.label == label)
        .unwrap_or_else(|| panic!("{label} is on screen"))
}

// Spec: routing R38, roar R28
#[test]
fn roars_strip_has_one_toggle_and_two_columns_the_audio_one_first() {
    use bl_ui::sidechain::{midi_rows, strip_buttons};
    let mut h = harness();
    let roar = add_roar(&mut h);
    let hidden = frame_of(&mut h, roar);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Show/Hide Sidechain").value, "off");

    // The one toggle shows both columns, which changes only the view.
    let steps = h.shell.editor().undo_history().len();
    h.click(strip_buttons(hidden)[0].center());
    h.settle();
    assert_eq!(h.shell.editor().undo_history().len(), steps);
    let shown = frame_of(&mut h, roar);
    assert_eq!(
        shown.width() - hidden.width(),
        2.0 * bl_shell::device_commands::SIDECHAIN_SECTION_WIDTH,
        "two columns, each a section wide"
    );
    let state = h.shell.device_view_state();
    let frame = state.devices.iter().find(|d| d.id == roar).unwrap();
    assert!(frame.sidechain.as_ref().unwrap().shown);
    assert!(frame.midi_sidechain.as_ref().unwrap().shown);

    // Audio first, then MIDI: External, then MIDI > FB Note, the MIDI
    // source and its tapping point.
    let nodes = Harness::nodes(&h.settle());
    let left = |label: &str| nodes[position(&nodes, label)].bounds.unwrap().x0;
    let audio = ["External", "Sidechain Source"].map(left);
    let midi = [
        "MIDI > FB Note",
        "MIDI Sidechain Source",
        "MIDI Sidechain Tapping Point",
    ]
    .map(left);
    assert!(
        audio.iter().all(|a| midi.iter().all(|m| a < m)),
        "the audio column is left of the MIDI one: {audio:?} {midi:?}"
    );

    // MIDI > FB Note is the device's own switch: one undo step.
    let [switch, _, _, tap] = midi_rows(shown);
    h.click(switch.center());
    h.settle();
    assert_eq!(value(&h, roar, MIDI_FB_NOTE), 1.0);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);

    // The tapping point, Post FX in a new device, from its chooser.
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "MIDI Sidechain Tapping Point").value,
        "Post FX"
    );
    h.click(tap.center());
    let nodes = Harness::nodes(&h.settle());
    let entry = nodes
        .iter()
        .find(|n| n.label == "Pre FX" && n.bounds.is_some_and(|b| b.y0 as f32 > tap.top()))
        .expect("the list is open");
    let b = entry.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    let state = h.shell.device_view_state();
    let frame = state.devices.iter().find(|d| d.id == roar).unwrap();
    assert_eq!(
        frame.midi_sidechain.as_ref().unwrap().tap.as_deref(),
        Some("Pre FX")
    );
    assert_eq!(last_step(&h), "Change \"Routing\"");
}

// Spec: roar R26
#[test]
fn up_and_down_step_the_note_by_a_semitone_and_shift_by_an_octave_each_one_undo_step() {
    let mut h = harness();
    let roar = add_roar(&mut h);
    let view = view_of(&mut h);
    let frame = layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == roar)
        .unwrap()
        .1;
    let panel = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == roar)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    let body = bl_ui::device_panel::body_rect(frame, bl_shell::device_commands::TITLE_BAR_HEIGHT);
    let note = place_shown(&panel, false, body)
        .0
        .into_iter()
        .find(|(param, _, _)| *param == FB_NOTE)
        .expect("the Note control is drawn")
        .2;
    // The Note control sits past the Device View's right edge at this
    // window size, so the keyboard reaches it as a screen reader does.
    assert!(note.right() > view.right() || note.contains(note.center()));
    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": roar, "param": FB_NOTE, "value": 60.0 }),
        )
        .unwrap();
    h.settle();
    h.accesskit(
        bl_ui::device_panel::param_control_id(roar, FB_NOTE),
        egui::accesskit::Action::Focus,
    );
    h.settle();
    let start = value(&h, roar, FB_NOTE);
    let steps = h.shell.editor().undo_history().len();
    for (key, shift, change) in [
        (egui::Key::ArrowUp, Modifiers::NONE, 1.0),
        (egui::Key::ArrowDown, Modifiers::NONE, -1.0),
        (egui::Key::ArrowUp, Modifiers::SHIFT, 12.0),
        (egui::Key::ArrowDown, Modifiers::SHIFT, -12.0),
    ] {
        let before = value(&h, roar, FB_NOTE);
        let undo = h.shell.editor().undo_history().len();
        h.key(key, shift);
        h.settle();
        assert_eq!(
            value(&h, roar, FB_NOTE),
            before + change,
            "{key:?} {shift:?}"
        );
        assert_eq!(
            h.shell.editor().undo_history().len(),
            undo + 1,
            "one undo step"
        );
        assert_eq!(last_step(&h), "Change \"Feedback Note\"");
    }
    assert_eq!(value(&h, roar, FB_NOTE), start);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 4);
}

// Spec: roar R31
#[test]
fn each_source_has_a_display_on_its_page() {
    let mut h = harness();
    let roar = add_roar(&mut h);
    let displays = |nodes: &[common::Node], name: &str| {
        nodes
            .iter()
            .filter(|n| n.label == name && n.role == egui::accesskit::Role::Image)
            .count()
    };
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(displays(&nodes, "LFO 1"), 0, "a page of its own");
    for (page, cell) in [
        ("LFO 1", "LFO 1"),
        ("LFO 2", "LFO 2"),
        ("Env", "Env"),
        ("Noise", "Noise"),
    ] {
        h.shell
            .run(
                "device.set_page",
                json!({ "device": roar, "node": bl_devices::roar::PAGES, "page": page }),
            )
            .unwrap();
        let nodes = Harness::nodes(&h.settle());
        assert_eq!(displays(&nodes, cell), 1, "{page}'s display is drawn");
    }
    // The first page has the shaper's display again.
    h.shell
        .run(
            "device.set_page",
            json!({ "device": roar, "node": bl_devices::roar::PAGES, "page": "Roar" }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert!(nodes.iter().any(|n| n.label == "Shaper"));
}
