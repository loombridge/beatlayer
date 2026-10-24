//! The displays of Recombine, Strum, Time Warp and Velocity Shaper, headless
//! (WP-6.13). Spec: midi-transformations R43, midi-tools R22; L-CLIP-007.
//! Each display's gesture is one `midi_tool.set` gesture: one undo step
//! named after the tool, and Esc puts the notes back.

use crate::common;

use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, MidiData, Note};
use bl_shell::files::factory_default_set;
use bl_shell::midi_tool_commands::MidiToolState;
use bl_ui::arrangement::benchmark;
use common::{Harness, Node, Window};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect};
use serde_json::{Value, json};

const SIXTEENTH: i64 = Ticks::SIXTEENTH.get();

/// A MIDI clip of these notes, `(key, start, length)` each, in the
/// Arrangement, open in the Clip View with every note selected and the tool
/// chosen and its panel showing.
fn song(tool: &str, notes: &[(u8, i64, i64)]) -> (Harness, ClipId) {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let clip: ClipId = editor.new_id();
    let made = notes
        .iter()
        .map(|(key, start, length)| Note::new(*key, Ticks(*start), Ticks(*length), 100));
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * Ticks::WHOLE.get()),
                ClipData::Midi(MidiData::new(made)),
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
    let places: Vec<usize> = (0..notes.len()).collect();
    h.shell
        .run("clip_view.select_notes", json!({ "notes": places }))
        .unwrap();
    h.shell
        .run("midi_tool.choose", json!({ "tool": tool }))
        .unwrap();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
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

fn notes(h: &Harness, clip: ClipId) -> Vec<(u8, i64, i64)> {
    match &h.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => bl_shell::clip_view::notes_in_order(midi)
            .iter()
            .map(|note| (note.key, note.start.get(), note.length.get()))
            .collect(),
        ClipData::Audio(_) => panic!("the clip is MIDI"),
    }
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

/// A tool's settings as `midi_tool.state` reads them back.
fn settings(h: &mut Harness, tool: &str) -> serde_json::Map<String, Value> {
    let state: MidiToolState =
        serde_json::from_value(h.shell.run("midi_tool.state", json!({})).unwrap()).unwrap();
    state
        .tools
        .into_iter()
        .find(|t| t.tool == tool)
        .unwrap()
        .values
}

fn pointer(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
}

fn press(h: &mut Harness, at: Pos2) {
    pointer(h, at);
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

/// Moves the pointer from `from` to `to` in eight frames, as a drag does.
fn drag_to(h: &mut Harness, from: Pos2, to: Pos2) {
    for step in 1..=8 {
        pointer(h, from + (to - from) * (step as f32 / 8.0));
    }
}

/// A whole drag from one point to another.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    press(h, from);
    drag_to(h, from, to);
    release(h, to);
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

fn named(h: &Harness, name: &str) -> bool {
    h.shell.editor().undo_history().next() == Some(name)
}

fn int(value: &Value) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| panic!("a whole number: {value}"))
}

#[test]
fn strums_breakpoints_set_its_values_in_one_gesture_and_esc_puts_the_notes_back() {
    // Spec: midi-transformations R29, R43. Dragging Strum Low's breakpoint
    // up sets Strum Low as one `midi_tool.set` gesture.
    // A long chord, late in the clip, so a strum of a grid step moves it
    // freely either way.
    let at = 2 * Ticks::WHOLE.get();
    let long = Ticks::WHOLE.get();
    let chord = [(60, at, long), (64, at, long), (67, at, long)];
    let (mut h, clip) = song("strum", &chord);
    let all = nodes(&mut h);
    let low = centre(Harness::node(&all, "Strum Low Breakpoint"));
    let before = steps(&h);
    drag(&mut h, low, low + egui::vec2(0.0, -18.0));
    let values = settings(&mut h, "strum");
    assert!(int(&values["strum_low"]) > 0, "up is later: {values:?}");
    assert_eq!(values["strum_high"], json!(0));
    assert_eq!(steps(&h), before + 1, "one undo step");
    assert!(named(&h, "Strum"));
    let lowest = notes(&h, clip).into_iter().find(|n| n.0 == 60).unwrap();
    assert!(lowest.1 > at, "the lowest note starts later");

    // The high breakpoint sets Strum High.
    let all = nodes(&mut h);
    let high = centre(Harness::node(&all, "Strum High Breakpoint"));
    drag(&mut h, high, high + egui::vec2(0.0, 18.0));
    assert!(int(&settings(&mut h, "strum")["strum_high"]) < 0);
    assert_eq!(steps(&h), before + 2);

    // Esc during the drag puts the notes back.
    let held = notes(&h, clip);
    let all = nodes(&mut h);
    let low = centre(Harness::node(&all, "Strum Low Breakpoint"));
    press(&mut h, low);
    drag_to(&mut h, low, low + egui::vec2(0.0, 10.0));
    assert_ne!(notes(&h, clip), held, "the drag is under way");
    escape(&mut h);
    release(&mut h, low + egui::vec2(0.0, 10.0));
    assert_eq!(notes(&h, clip), held, "Esc put the notes back");
    assert_eq!(steps(&h), before + 2);
}

#[test]
fn dragging_strums_curve_sets_tension() {
    // Spec: midi-transformations R29, R43. With Strum Low above Strum High,
    // dragging the curve's middle sets Tension.
    let chord = [
        (60, 0, 4 * SIXTEENTH),
        (64, 0, 4 * SIXTEENTH),
        (67, 0, 4 * SIXTEENTH),
    ];
    let (mut h, _) = song("strum", &chord);
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "strum", "values": { "strum_low": 100 } }),
        )
        .unwrap();
    h.settle();
    let before = steps(&h);
    let all = nodes(&mut h);
    let middle = centre(Harness::node(&all, "Tension Curve"));
    drag(&mut h, middle, middle + egui::vec2(0.0, -10.0));
    // The curve runs from the high Strum Low down to Strum High: dragging
    // its middle up lifts it, which is a negative Tension (the gaps between
    // the notes widest at the end), and dragging it down a positive one.
    let tension = |h: &mut Harness| int(&settings(h, "strum")["tension"]);
    assert!(tension(&mut h) < 0);
    assert_eq!(steps(&h) - before, 1, "one undo step");
    let all = nodes(&mut h);
    let middle = centre(Harness::node(&all, "Tension Curve"));
    drag(&mut h, middle, middle + egui::vec2(0.0, 30.0));
    assert!(tension(&mut h) > 0);
}

#[test]
fn recombines_columns_rotate_by_dragging_and_the_step_buttons_by_one() {
    // Spec: midi-transformations R23, R43. Dragging across the columns sets
    // Rotation Steps; the Rotate Step buttons lower and raise it.
    let four = [
        (60, 0, SIXTEENTH),
        (62, SIXTEENTH, SIXTEENTH),
        (64, 2 * SIXTEENTH, SIXTEENTH),
        (65, 3 * SIXTEENTH, SIXTEENTH),
    ];
    let (mut h, clip) = song("recombine", &four);
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "recombine", "values": { "dimension": "pitch" } }),
        )
        .unwrap();
    h.settle();
    let keys = |h: &Harness| notes(h, clip).iter().map(|n| n.0).collect::<Vec<_>>();
    let all = nodes(&mut h);
    let columns = Harness::node(&all, "Recombine Columns");
    let bounds = columns.bounds.unwrap();
    let from = centre(columns);
    let one_column = (bounds.x1 - bounds.x0) as f32 / 4.0;
    let before = steps(&h);
    drag(&mut h, from, from + egui::vec2(one_column, 0.0));
    assert_eq!(settings(&mut h, "recombine")["rotation_steps"], json!(1));
    assert_eq!(keys(&h), [65, 60, 62, 64], "each pitch a place later");
    assert_eq!(steps(&h), before + 1, "one undo step");
    assert!(named(&h, "Recombine"));

    // Rotate Step Up and Down are one `midi_tool.set` each.
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Rotate Step Up")));
    assert_eq!(settings(&mut h, "recombine")["rotation_steps"], json!(2));
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Rotate Step Down")));
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Rotate Step Down")));
    assert_eq!(settings(&mut h, "recombine")["rotation_steps"], json!(0));
    assert_eq!(keys(&h), [60, 62, 64, 65]);
    // Rotate on Grid is for Position alone.
    assert!(
        !nodes(&mut h).iter().any(|n| n.label == "Rotate on Grid"),
        "not offered with Pitch"
    );
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "recombine", "values": { "dimension": "position" } }),
        )
        .unwrap();
    Harness::node(&nodes(&mut h), "Rotate on Grid");
}

#[test]
fn time_warps_breakpoints_drag_to_set_time_and_speed_together() {
    // Spec: midi-transformations R31, R43. A breakpoint dragged in the
    // display sets its Time and Speed in one gesture; the selected
    // breakpoint's fields show them.
    let eight: Vec<(u8, i64, i64)> = (0..8)
        .map(|n| (60 + n as u8, n * SIXTEENTH, SIXTEENTH / 2))
        .collect();
    let (mut h, clip) = song("time_warp", &eight);
    let held = notes(&h, clip);
    let all = nodes(&mut h);
    let second = centre(Harness::node(&all, "Speed Curve Breakpoint 2"));
    let before = steps(&h);
    drag(&mut h, second, second + egui::vec2(-12.0, -10.0));
    let values = settings(&mut h, "time_warp");
    assert!(int(&values["breakpoint_2_time"]) < 100, "{values:?}");
    assert!(int(&values["breakpoint_2_speed"]) > 100, "{values:?}");
    assert_eq!(values["breakpoint_1_speed"], json!(100));
    assert_eq!(steps(&h), before + 1, "one undo step");
    assert!(named(&h, "Time Warp"));
    assert_ne!(notes(&h, clip), held, "the notes moved");

    // The fields show and set the selected breakpoint.
    let all = nodes(&mut h);
    assert_eq!(
        Harness::node(&all, "Breakpoint Time").value,
        format!("{} %", int(&values["breakpoint_2_time"]))
    );
    h.click(centre(Harness::node(&all, "Breakpoint 3")));
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Breakpoint Time").value, "50 %");
    assert_eq!(Harness::node(&all, "Breakpoint").toggled, Some(false));
    h.click(centre(Harness::node(&all, "Breakpoint")));
    assert_eq!(settings(&mut h, "time_warp")["breakpoint_3"], json!(true));
    let all = nodes(&mut h);
    let speed = centre(Harness::node(&all, "Breakpoint Speed"));
    drag(&mut h, speed, speed + egui::vec2(0.0, -20.0));
    assert!(int(&settings(&mut h, "time_warp")["breakpoint_3_speed"]) > 100);
}

#[test]
fn time_warp_keeps_one_breakpoint_on() {
    // Spec: midi-transformations R31.
    let eight: Vec<(u8, i64, i64)> = (0..8)
        .map(|n| (60 + n as u8, n * SIXTEENTH, SIXTEENTH / 2))
        .collect();
    let (mut h, _) = song("time_warp", &eight);
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "time_warp", "values": { "breakpoint_2": false } }),
        )
        .unwrap();
    h.settle();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Breakpoint").toggled, Some(true));
    h.click(centre(Harness::node(&all, "Breakpoint")));
    assert_eq!(
        settings(&mut h, "time_warp")["breakpoint_1"],
        json!(true),
        "the last one on stays on"
    );
}

#[test]
fn velocity_shapers_envelope_takes_clicks_to_add_and_drags_to_move() {
    // Spec: midi-transformations R34, R43.
    let four: Vec<(u8, i64, i64)> = (0..4).map(|n| (60, n * 4 * SIXTEENTH, SIXTEENTH)).collect();
    let (mut h, clip) = song("velocity_shaper", &four);
    let all = nodes(&mut h);
    let envelope = Harness::node(&all, "Velocity Shaper Envelope");
    let bounds = envelope.bounds.unwrap();
    let before = steps(&h);
    // A click in the middle adds a breakpoint there.
    let middle = Pos2::new(
        (bounds.x0 + bounds.x1) as f32 / 2.0,
        bounds.y0 as f32 + (bounds.y1 - bounds.y0) as f32 * 0.25,
    );
    h.click(middle);
    let points = settings(&mut h, "velocity_shaper")["envelope"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(points.len(), 3, "{points:?}");
    assert!((points[1]["x"].as_f64().unwrap() - 0.5).abs() < 0.1);
    assert!(points[1]["y"].as_f64().unwrap() > 0.5, "near the top");

    // Dragging a breakpoint down moves it, in one gesture.
    let all = nodes(&mut h);
    let handle = centre(Harness::node(&all, "Envelope Breakpoint 2"));
    let steps_before = steps(&h);
    drag(&mut h, handle, handle + egui::vec2(0.0, 12.0));
    let moved = settings(&mut h, "velocity_shaper")["envelope"]
        .as_array()
        .unwrap()
        .clone();
    assert!(moved[1]["y"].as_f64().unwrap() < points[1]["y"].as_f64().unwrap());
    assert_eq!(steps(&h), steps_before + 1, "one undo step");
    assert!(named(&h, "Velocity Shaper"));
    // The first and last stay at the ends of the range.
    let all = nodes(&mut h);
    let first = centre(Harness::node(&all, "Envelope Breakpoint 1"));
    drag(&mut h, first, first + egui::vec2(15.0, -10.0));
    let ends = settings(&mut h, "velocity_shaper")["envelope"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(ends[0]["x"], json!(0.0), "pinned to the start");
    assert!(ends[0]["y"].as_f64().unwrap() > 0.0);
    assert_eq!(ends.last().unwrap()["x"], json!(1.0));
    assert!(steps(&h) > before);
    // The velocities follow the envelope.
    let velocities: Vec<u8> = match &h.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => bl_shell::clip_view::notes_in_order(midi)
            .iter()
            .map(|note| note.velocity)
            .collect(),
        ClipData::Audio(_) => unreachable!(),
    };
    assert_ne!(velocities, [100; 4]);
}
