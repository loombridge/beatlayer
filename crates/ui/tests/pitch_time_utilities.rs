//! The Clip View's Pitch and Time Utilities panel, headless (WP-6.16): its
//! controls in order, a drag on Transpose, Interval Size or Stretch as one
//! undo step that Esc puts back, and the buttons running their commands.
//! Spec: pitch-and-time-utilities R1, R3, R4, R7, R8; L-CLIP-006.

use crate::common;

use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, MidiData, Note};
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use common::{Harness, Node, Window};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect};
use serde_json::json;

const QUARTER: i64 = Ticks::QUARTER.get();

/// A MIDI clip of three notes in the Arrangement, open in the Clip View with
/// its three notes selected, showing the Pitch and Time Utilities.
fn set() -> (Harness, ClipId) {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let clip: ClipId = editor.new_id();
    let notes = [60u8, 62, 64]
        .into_iter()
        .enumerate()
        .map(|(at, key)| Note::new(key, Ticks(at as i64 * QUARTER), Ticks(QUARTER), 100));
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * Ticks::WHOLE.get()),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("clip_view.select_notes", json!({ "notes": [0, 1, 2] }))
        .unwrap();
    h.settle();
    (h, clip)
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

fn centre(node: &Node) -> Pos2 {
    let b = node.bounds.expect("the control has bounds");
    Rect::from_min_max(
        Pos2::new(b.x0 as f32, b.y0 as f32),
        Pos2::new(b.x1 as f32, b.y1 as f32),
    )
    .center()
}

fn notes(h: &Harness, clip: ClipId) -> Vec<Note> {
    match &h.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => bl_shell::clip_view::notes_in_order(midi),
        ClipData::Audio(_) => panic!("the clip is MIDI"),
    }
}

fn keys(h: &Harness, clip: ClipId) -> Vec<u8> {
    notes(h, clip).iter().map(|note| note.key).collect()
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn last_step(h: &Harness) -> Option<&str> {
    h.shell.editor().undo_history().next()
}

fn press(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
}

fn release(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

/// Moves the pointer up by `points` in eight frames, as a drag does.
fn drag_up(h: &mut Harness, from: Pos2, points: f32) {
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        h.frame(vec![Event::PointerMoved(
            from + egui::vec2(0.0, -points * t),
        )]);
    }
}

fn escape(h: &mut Harness) {
    h.frame(vec![Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
}

#[test]
fn the_panel_holds_its_controls_and_fit_to_scale_needs_scale_mode() {
    // Spec: pitch-and-time-utilities R1, R5. The new Clip View shows this
    // panel, with its controls named for assistive technology.
    let (mut h, _) = set();
    let all = nodes(&mut h);
    for name in [
        "Transpose",
        "Fit to Scale",
        "Invert",
        "Interval Size",
        "Add Interval",
        "Stretch Factor",
        "Double",
        "Halve",
        "Duration",
        "Set Length",
        "Humanize Amount",
        "Humanize",
        "Reverse",
        "Legato",
    ] {
        Harness::node(&all, name);
    }
    assert!(Harness::node(&all, "Fit to Scale").disabled);
    assert_eq!(Harness::node(&all, "Duration").value, "Grid");
    // Transpose shows the notes' range, by name.
    assert_eq!(Harness::node(&all, "Transpose").value, "C3 - E3");
    h.shell
        .run(
            "clip.set_scale",
            json!({ "enabled": true, "root": 0, "name": "major" }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert!(!Harness::node(&all, "Fit to Scale").disabled);
    assert_eq!(Harness::node(&all, "Interval Size").value, "0 sd");
}

#[test]
fn a_transpose_drag_is_one_undo_step_and_esc_puts_the_notes_back() {
    // Spec: pitch-and-time-utilities R3, R4. The frames of the drag are
    // `note.transpose` gestures.
    let (mut h, clip) = set();
    let all = nodes(&mut h);
    let field = centre(Harness::node(&all, "Transpose"));
    let (before, original) = (steps(&h), keys(&h, clip));
    press(&mut h, field);
    drag_up(&mut h, field, 24.0);
    release(&mut h, field + egui::vec2(0.0, -24.0));
    assert_ne!(keys(&h, clip), original, "the drag moved the notes");
    assert_eq!(steps(&h), before + 1, "however many frames, one undo step");
    assert_eq!(last_step(&h), Some("Transpose"));
    // The field then shows the new range.
    let all = nodes(&mut h);
    let moved = keys(&h, clip);
    assert_ne!(Harness::node(&all, "Transpose").value, "C3 - E3");
    let field = centre(Harness::node(&all, "Transpose"));
    press(&mut h, field);
    drag_up(&mut h, field, 24.0);
    assert_ne!(keys(&h, clip), moved, "the drag is under way");
    escape(&mut h);
    release(&mut h, field + egui::vec2(0.0, -24.0));
    assert_eq!(keys(&h, clip), moved, "Esc put the notes back");
    assert_eq!(steps(&h), before + 1, "and recorded nothing");
}

#[test]
fn an_interval_size_drag_adds_copies_in_one_step_and_sets_the_value() {
    // Spec: pitch-and-time-utilities R7. The copies are of the notes selected
    // when the drag began; Esc puts the notes back.
    let (mut h, clip) = set();
    let all = nodes(&mut h);
    let field = centre(Harness::node(&all, "Interval Size"));
    let before = steps(&h);
    press(&mut h, field);
    drag_up(&mut h, field, 24.0);
    assert_eq!(
        notes(&h, clip).len(),
        6,
        "one set of copies, not one a frame"
    );
    escape(&mut h);
    release(&mut h, field + egui::vec2(0.0, -24.0));
    assert_eq!(notes(&h, clip).len(), 3, "Esc took the copies away");
    assert_eq!(steps(&h), before);
    let all = nodes(&mut h);
    let field = centre(Harness::node(&all, "Interval Size"));
    press(&mut h, field);
    drag_up(&mut h, field, 24.0);
    release(&mut h, field + egui::vec2(0.0, -24.0));
    assert_eq!(notes(&h, clip).len(), 6);
    assert_eq!(steps(&h), before + 1, "however many frames, one undo step");
    assert_eq!(last_step(&h), Some("Interval"));
    let state = bl_shell::clip_view::utilities_state(
        h.shell.editor().document(),
        h.shell.clip_view(),
        h.shell.arrangement(),
    );
    assert!(state.interval > 0, "the field kept its value");
}

#[test]
fn typing_into_interval_size_adds_the_interval_and_the_field_goes_back_to_zero() {
    // Spec: pitch-and-time-utilities R7. Observed in Live: Enter applies.
    let (mut h, clip) = set();
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Interval Size")));
    let before = steps(&h);
    h.frame(vec![Event::Text("7".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(notes(&h, clip).len(), 6);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), Some("Interval"));
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Interval Size").value, "0 st");
}

#[test]
fn the_stretch_knob_stretches_as_it_is_dragged_and_goes_back_to_one() {
    // Spec: pitch-and-time-utilities R8. One step, "Stretch"; Esc puts the
    // notes back; the knob reads ×1 again afterwards.
    let (mut h, clip) = set();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Stretch Factor").value, "\u{d7}1.00");
    let knob = centre(Harness::node(&all, "Stretch Factor"));
    let (before, original) = (steps(&h), notes(&h, clip));
    press(&mut h, knob);
    drag_up(&mut h, knob, 30.0);
    assert_ne!(notes(&h, clip), original, "stretched while dragged");
    release(&mut h, knob + egui::vec2(0.0, -30.0));
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), Some("Stretch"));
    let all = nodes(&mut h);
    assert_eq!(
        Harness::node(&all, "Stretch Factor").value,
        "\u{d7}1.00",
        "back at ×1"
    );
    let stretched = notes(&h, clip);
    let knob = centre(Harness::node(&all, "Stretch Factor"));
    press(&mut h, knob);
    drag_up(&mut h, knob, 30.0);
    escape(&mut h);
    release(&mut h, knob + egui::vec2(0.0, -30.0));
    assert_eq!(notes(&h, clip), stretched, "Esc put the notes back");
    assert_eq!(steps(&h), before + 1);
}

#[test]
fn the_buttons_run_their_commands_one_undo_step_each() {
    // Spec: pitch-and-time-utilities R3, R9, R12, R13. ×2 and ÷2 are named
    // Stretch Time by Double and Shrink Time by Half.
    let (mut h, clip) = set();
    for (button, step) in [
        ("Double", "Stretch Time by Double"),
        ("Halve", "Shrink Time by Half"),
        ("Invert", "Invert"),
        ("Reverse", "Reverse"),
        ("Legato", "Legato"),
    ] {
        let before = steps(&h);
        let all = nodes(&mut h);
        h.click(centre(Harness::node(&all, button)));
        assert_eq!(steps(&h), before + 1, "{button}");
        assert_eq!(last_step(&h), Some(step), "{button}");
    }
    assert_eq!(notes(&h, clip).len(), 3);
}

#[test]
fn the_info_view_shows_a_controls_registered_description() {
    // Spec: pitch-and-time-utilities R1; info-view R4. While the pointer is
    // over a control the Info View shows its name and the description of the
    // command it runs.
    let (mut h, _) = set();
    let all = nodes(&mut h);
    let invert = centre(Harness::node(&all, "Invert"));
    h.frame(vec![Event::PointerMoved(invert)]);
    h.settle();
    h.frame(vec![Event::PointerMoved(invert)]);
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Invert");
    assert!(
        content.body.starts_with("Turns notes upside down"),
        "{}",
        content.body
    );
}
