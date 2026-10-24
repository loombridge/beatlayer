//! The Clip View's MPE tab, headless (WP-6.18): the tab and its keys, the
//! lanes, toggles and triangle, the Pitch envelopes over the notes, and the
//! gestures that edit breakpoints, each reaching the command agents run as
//! one undo step. Spec: mpe R22 to R34.

use crate::common;
use bl_core::time::Ticks;
use bl_model::{
    Breakpoint, Clip, ClipData, ClipId, Command, Document, Expression, MidiData, MpeLane, Note,
    TrackId,
};
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::factory_default_set;
use bl_shell::note_commands::ClipTab;
use bl_ui::arrangement::benchmark;
use bl_ui::clip_view::{KEY_HEIGHT, tick_at, x_of_tick, y_of_key};
use bl_ui::mpe_tab::{MpeLayout, layout};
use bl_ui::note_filters::body_rect;
use common::{Harness, Window};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();
const QUARTER: i64 = Ticks::QUARTER.get();

/// The Option (Alt) key with Shift, for the tab keys.
fn option_shift() -> Modifiers {
    Modifiers::ALT | Modifiers::SHIFT
}

/// Option+Tab on macOS, Ctrl+Tab on Windows and Linux (S-CLIPMODE-002).
fn next_tab() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    }
}

/// Cmd on macOS, Ctrl elsewhere.
fn command_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

/// The modifier that bypasses snapping: Cmd on macOS, Alt elsewhere.
fn bypass_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::ALT
    }
}

fn point(at: i64, value: f64) -> Breakpoint {
    Breakpoint {
        at: Ticks(at),
        value,
        bend: 0.0,
    }
}

struct Env {
    h: Harness,
    clip: ClipId,
    track: TrackId,
}

/// A song with a sixteen-bar MIDI clip on the first MIDI track holding
/// `notes`, selected and open in the Clip View on its MPE tab.
fn set(mut notes: Vec<Note>) -> Env {
    // A high note with no envelopes, so the editor opens with room above
    // the notes for their Pitch to be drawn and grabbed.
    notes.push(Note::new(72, Ticks(12 * BAR), Ticks(QUARTER), 100));
    let mut editor = factory_default_set();
    let track: TrackId = editor.document().top_level().next().unwrap();
    let id: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks(16 * BAR),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [id] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("view.clip_tab", json!({ "tab": "mpe" }))
        .unwrap();
    h.settle();
    Env { h, clip: id, track }
}

/// A one-bar note carrying a Slide and a Pitch envelope.
fn shaped(key: u8, start: i64) -> Note {
    Note {
        pitch: vec![point(0, 0.0), point(BAR / 2, 6.0), point(BAR, 0.0)],
        slide: vec![point(0, 10.0), point(BAR, 110.0)],
        ..Note::new(key, Ticks(start), Ticks(BAR), 100)
    }
}

impl Env {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        let result = self
            .h
            .shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"));
        self.h.settle();
        result
    }

    fn notes(&self) -> Vec<Note> {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => notes_in_order(midi),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    fn length(&self) -> Ticks {
        self.doc().clip(self.clip).unwrap().length
    }

    fn view(&mut self) -> Rect {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let bounds = Harness::node(&nodes, "Clip View").bounds.unwrap();
        bl_ui::clip_view::editor_rect(Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        ))
    }

    /// The tab's layout for the lanes the shell says show.
    fn layout(&mut self) -> MpeLayout {
        let view = self.view();
        let state = self
            .h
            .shell
            .clip_view()
            .state(self.doc(), self.h.shell.arrangement());
        let lanes: Vec<(MpeLane, f32)> =
            state.mpe.lanes.iter().map(|l| (l.lane, l.height)).collect();
        layout(body_rect(view, false), &lanes, state.mpe.hidden)
    }

    fn selected(&self) -> Vec<u32> {
        self.h
            .shell
            .clip_view()
            .selected_notes(self.doc(), self.h.shell.arrangement())
            .to_vec()
    }

    /// A point in the note area on `key`'s row, `rows` semitones above its
    /// centre, at `tick` of the clip's contents.
    fn pitch_at(&mut self, key: u8, tick: i64, semitones: f64) -> Pos2 {
        let grid = self.layout().grid;
        let top = self.h.main.clip_view().top();
        pos2(
            x_of_tick(grid, Ticks::ZERO, self.length(), Ticks(tick)),
            y_of_key(grid, top, key) + KEY_HEIGHT / 2.0 - semitones as f32 * KEY_HEIGHT,
        )
    }

    /// A point in `lane` at `tick` of the clip's contents, `value` high.
    fn lane_at(&mut self, lane: MpeLane, tick: i64, value: f64) -> Pos2 {
        let layout = self.layout();
        let rect = layout.plot(layout.lane(lane).expect("the lane shows"));
        let (low, high) = match lane {
            MpeLane::Velocity | MpeLane::ReleaseVelocity => (1.0, 127.0),
            MpeLane::PitchBend => (-48.0, 48.0),
            _ => (0.0, 127.0),
        };
        let x = x_of_tick(layout.grid, Ticks::ZERO, self.length(), Ticks(tick));
        let y = rect.bottom() - ((value - low) / (high - low)) as f32 * rect.height();
        pos2(x, y)
    }

    fn node_centre(&mut self, label: &str) -> Pos2 {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let b = Harness::node(&nodes, label).bounds.unwrap();
        pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
    }

    fn click(&mut self, at: Pos2, modifiers: Modifiers) {
        let held = Event::ModifiersChanged(modifiers);
        self.h.frame(vec![held.clone(), Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.h.frame(vec![
                held.clone(),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers,
                },
            ]);
        }
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    fn drag_frames(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        let held = Event::ModifiersChanged(modifiers);
        self.h.frame(vec![held.clone(), Event::PointerMoved(from)]);
        self.h.frame(vec![
            held.clone(),
            Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers,
            },
        ]);
        for step in 1..=8 {
            let t = step as f32 / 8.0;
            self.h.frame(vec![
                held.clone(),
                Event::PointerMoved(from + (to - from) * t),
            ]);
        }
    }

    fn release(&mut self, at: Pos2, modifiers: Modifiers) {
        self.h.frame(vec![
            Event::ModifiersChanged(modifiers),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers,
            },
        ]);
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        self.drag_frames(from, to, modifiers);
        self.release(to, modifiers);
    }

    /// Checks the gesture just made is exactly one undo step called `label`
    /// and that undo and redo restore the set exactly.
    fn one_step(&mut self, before: &Document, steps: usize, label: &str) {
        assert_eq!(self.steps(), steps + 1, "{label} is one undo step");
        assert_eq!(self.h.shell.editor().undo_history().next(), Some(label));
        let after = self.doc().clone();
        self.run("edit.undo", json!({}));
        assert!(self.doc() == before, "undo restores the set exactly");
        self.run("edit.redo", json!({}));
        assert!(*self.doc() == after, "redo restores it exactly");
    }

    /// Selects a note.
    fn select(&mut self, places: &[u32]) {
        self.run(
            "clip_view.select_notes",
            json!({ "clip": self.clip, "notes": places }),
        );
    }
}

// Spec: mpe R22; AccessKit. The MPE tab is the third tab and clicking it,
// Option+Shift+3 and Option+Tab show it.
#[test]
fn the_tab_opens_by_click_by_key_and_by_option_tab() {
    let mut e = set(vec![shaped(60, 0)]);
    e.run("view.clip_tab", json!({ "tab": "notes" }));
    let output = e.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "MPE").toggled, Some(false));
    assert!(!Harness::node(&nodes, "MPE").disabled, "the tab is on");
    let mpe = e.node_centre("MPE");
    e.click(mpe, Modifiers::NONE);
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Mpe);
    assert_eq!(
        Harness::node(&Harness::nodes(&e.h.settle()), "MPE").toggled,
        Some(true)
    );
    e.h.shell.set_focused_area(bl_shell::Area::Detail);
    e.run("view.clip_tab", json!({ "tab": "notes" }));
    e.h.key(Key::Num3, option_shift());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Mpe);
    // Option+Tab goes round the three tabs.
    e.h.key(Key::Tab, next_tab());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Notes);
    e.h.key(Key::Tab, next_tab());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Envelopes);
    e.h.key(Key::Tab, next_tab());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Mpe);
    // View ▸ Expression Editor runs it while a MIDI clip shows.
    use bl_shell::menus::{MenuPlatform, menu_bar};
    use bl_ui::keymap::{Keymap, Platform};
    e.run("view.clip_tab", json!({ "tab": "notes" }));
    let keymap = Keymap::live12();
    let bar = menu_bar(MenuPlatform::current());
    let view = bar.iter().find(|m| m.title == "View").unwrap();
    let item = view
        .walk()
        .find(|i| i.label == "Expression Editor")
        .expect("the menu item");
    let state = bl_ui::menus::item_state(item, &e.h.shell, &keymap, Platform::current());
    assert!(state.enabled, "enabled while a MIDI clip shows");
    let invocation = state.invocation.unwrap();
    e.h.shell.invoke(&invocation.id, invocation.args).unwrap();
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Mpe);
    // With no clip showing it is disabled.
    e.run("arrangement.select_clips", json!({ "clips": [] }));
    let state = bl_ui::menus::item_state(item, &e.h.shell, &keymap, Platform::current());
    assert!(!state.enabled);
}

// Spec: mpe R23, R34. The toggles and the triangle are switches in the tree;
// each toggle, the triangle (and with Option) and a split drag change only
// view state.
#[test]
fn the_toggles_the_triangle_and_the_splits_change_only_the_view() {
    let mut e = set(vec![shaped(60, 0)]);
    let steps = e.steps();
    let output = e.h.settle();
    let nodes = Harness::nodes(&output);
    for (name, on) in [
        ("Slide", true),
        ("Pressure", true),
        ("Velocity", false),
        ("Release Velocity", false),
        ("Expression Lanes", true),
    ] {
        assert_eq!(Harness::node(&nodes, name).toggled, Some(on), "{name}");
    }
    let lanes = |e: &mut Env| {
        e.layout()
            .lanes
            .iter()
            .map(|(lane, _)| *lane)
            .collect::<Vec<_>>()
    };
    assert_eq!(lanes(&mut e), [MpeLane::Slide, MpeLane::Pressure]);
    let velocity = e.node_centre("Velocity");
    e.click(velocity, Modifiers::NONE);
    assert_eq!(
        lanes(&mut e),
        [MpeLane::Slide, MpeLane::Pressure, MpeLane::Velocity]
    );
    let slide = e.node_centre("Slide");
    e.click(slide, Modifiers::NONE);
    assert_eq!(lanes(&mut e), [MpeLane::Pressure, MpeLane::Velocity]);
    // The triangle hides the lanes and their toggles, and shows them again.
    let triangle = e.node_centre("Expression Lanes");
    e.click(triangle, Modifiers::NONE);
    assert!(lanes(&mut e).is_empty());
    let nodes = Harness::nodes(&e.h.settle());
    assert!(nodes.iter().all(|n| n.label != "Slide"), "the toggles hide");
    let triangle = e.node_centre("Expression Lanes");
    e.click(triangle, Modifiers::NONE);
    assert_eq!(lanes(&mut e), [MpeLane::Pressure, MpeLane::Velocity]);
    // With Option it shows all four.
    let triangle = e.node_centre("Expression Lanes");
    e.click(triangle, Modifiers::ALT);
    assert_eq!(
        lanes(&mut e),
        [
            MpeLane::Slide,
            MpeLane::Pressure,
            MpeLane::Velocity,
            MpeLane::ReleaseVelocity
        ]
    );
    assert_eq!(e.steps(), steps, "none of it is an undo step");
    assert!(!e.h.shell.is_dirty());
}

// Spec: mpe R24. Pitch is drawn over each note, and hidden while Fold is on.
#[test]
fn pitch_is_drawn_over_the_notes_and_hidden_under_fold() {
    let mut e = set(vec![shaped(60, 0), shaped(64, 2 * BAR)]);
    e.h.settle();
    assert_eq!(e.h.main.clip_view().mpe_pitch_drawn(), 3);
    e.run("view.fold_notes", json!({ "folded": true }));
    assert_eq!(e.h.main.clip_view().mpe_pitch_drawn(), 0);
    e.run("view.fold_notes", json!({ "folded": false }));
    assert_eq!(e.h.main.clip_view().mpe_pitch_drawn(), 3);
}

// Spec: mpe R24. A Drum Rack's track shows the Pitch Bend lane first, and
// nothing is drawn over the pads.
#[test]
fn a_drum_rack_track_shows_its_pitch_bend_lane_and_draws_nothing_over_pads() {
    let mut e = set(vec![shaped(36, 0)]);
    e.h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [e.track] }))
        .unwrap();
    e.h.shell.set_focused_area(bl_shell::Area::Detail);
    e.h.shell
        .invoke(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack",
                "role": "instrument",
            } }),
        )
        .unwrap();
    e.run("arrangement.select_clips", json!({ "clips": [e.clip] }));
    let layout = e.layout();
    assert_eq!(layout.lanes[0].0, MpeLane::PitchBend);
    assert_eq!(e.h.main.clip_view().mpe_pitch_drawn(), 0);
    let output = e.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Pitch Bend lane").value, "-48 to 48");
    // The lane draws and edits the Pitch envelopes: a click on its segment
    // adds a breakpoint to the selected note.
    e.select(&[0]);
    let at = e.lane_at(MpeLane::PitchBend, BAR / 4, 3.0);
    let steps = e.steps();
    e.click(at, Modifiers::NONE);
    assert_eq!(e.steps(), steps + 1);
    assert_eq!(e.notes()[0].pitch.len(), 4);
}

// Spec: mpe R26, R32. A click on a segment of a selected note's envelope
// adds a breakpoint on it, and a click on a breakpoint deletes it, each one
// undo step of its command.
#[test]
fn clicking_a_segment_adds_a_breakpoint_and_clicking_one_deletes_it() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    // In the Slide lane, on the segment between 10 and 110.
    let before = e.doc().clone();
    let steps = e.steps();
    let on = e.lane_at(MpeLane::Slide, BAR / 2, 60.0);
    e.click(on, Modifiers::NONE);
    let slide = e.notes()[0].slide.clone();
    assert_eq!(slide.len(), 3, "{slide:?}");
    assert!((slide[1].value - 60.0).abs() < 2.5, "{slide:?}");
    e.one_step(&before, steps, "Add Expression Breakpoint");
    // On the Pitch envelope over the note.
    let before = e.doc().clone();
    let steps = e.steps();
    let on = e.pitch_at(60, BAR / 4, 3.0);
    e.click(on, Modifiers::NONE);
    assert_eq!(e.notes()[0].pitch.len(), 4);
    e.one_step(&before, steps, "Add Expression Breakpoint");
    // Clicking the breakpoint at 6 semitones deletes it.
    let before = e.doc().clone();
    let steps = e.steps();
    let on = e.pitch_at(60, BAR / 2, 6.0);
    e.click(on, Modifiers::NONE);
    assert!(e.notes()[0].pitch.iter().all(|p| p.at != Ticks(BAR / 2)));
    e.one_step(&before, steps, "Delete Expression Breakpoints");
}

// Spec: mpe R26, R32. Dragging a breakpoint moves it in time and value as
// one undo step, and Esc puts the clip back.
#[test]
fn dragging_a_breakpoint_moves_it_and_esc_puts_the_clip_back() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    let before = e.doc().clone();
    let steps = e.steps();
    let from = e.pitch_at(60, BAR / 2, 6.0);
    let to = e.pitch_at(60, BAR / 2 + QUARTER, 9.0);
    e.drag(from, to, Modifiers::NONE);
    let moved = e.notes()[0].pitch.clone();
    assert_eq!(moved.len(), 3);
    assert!(moved[1].at > Ticks(BAR / 2), "{moved:?}");
    assert!((moved[1].value - 9.0).abs() < 0.6, "{moved:?}");
    e.one_step(&before, steps, "Move Expression Breakpoints");
    // Esc during a drag.
    e.run("edit.undo", json!({}));
    e.drag_frames(from, to, Modifiers::NONE);
    assert!(*e.doc() != before, "the clip follows the drag");
    e.h.key(Key::Escape, Modifiers::NONE);
    assert!(*e.doc() == before, "Esc puts the clip back");
    e.release(to, Modifiers::NONE);
    assert!(*e.doc() == before);
    assert_eq!(e.steps(), steps);
}

// Spec: mpe R26. Dragging on an envelope's background selects the
// breakpoints it encloses; Cmd+A selects them all, and a drag then moves
// them together.
#[test]
fn a_marquee_selects_breakpoints_and_cmd_a_selects_all_of_them() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    let from = e.lane_at(MpeLane::Slide, BAR / 8, 120.0);
    let to = e.lane_at(MpeLane::Slide, BAR + QUARTER, 90.0);
    e.drag(from, to, Modifiers::NONE);
    let state =
        e.h.shell
            .clip_view()
            .state(e.doc(), e.h.shell.arrangement());
    assert_eq!(state.mpe.envelope, Some(Expression::Slide));
    assert_eq!(
        state.mpe.breakpoints.len(),
        1,
        "only the one inside: {state:?}"
    );
    // Click the envelope's background, then Cmd+A.
    let background = e.lane_at(MpeLane::Slide, BAR / 3, 120.0);
    e.click(background, Modifiers::NONE);
    e.h.shell.set_focused_area(bl_shell::Area::Detail);
    e.h.key(Key::A, command_key());
    let state =
        e.h.shell
            .clip_view()
            .state(e.doc(), e.h.shell.arrangement());
    assert_eq!(state.mpe.breakpoints.len(), 2, "Cmd+A selects both");
    // Dragging one moves both by the same change. The lane is made taller
    // so the drag is longer than a click.
    e.run(
        "view.mpe_lanes",
        json!({ "lane": "slide", "height": 200.0 }),
    );
    let before = e.doc().clone();
    let steps = e.steps();
    let grab = e.lane_at(MpeLane::Slide, BAR, 110.0);
    let to = e.lane_at(MpeLane::Slide, BAR, 125.0);
    e.drag(grab, to, Modifiers::NONE);
    let slide = e.notes()[0].slide.clone();
    assert!((slide[0].value - 25.0).abs() < 3.0, "{slide:?}");
    assert!((slide[1].value - 125.0).abs() < 3.0, "{slide:?}");
    e.one_step(&before, steps, "Move Expression Breakpoints");
}

// Spec: mpe R28. Option-dragging a segment curves it.
#[test]
fn option_dragging_a_segment_curves_it() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    let before = e.doc().clone();
    let steps = e.steps();
    let from = e.lane_at(MpeLane::Slide, BAR / 2, 60.0);
    e.drag(from, from + vec2(0.0, -12.0), Modifiers::ALT);
    assert!(e.notes()[0].slide[0].bend > 0.1, "{:?}", e.notes()[0].slide);
    e.one_step(&before, steps, "Set Expression Curve");
}

// Spec: mpe R29. Dragging an unselected segment up or down scales the value
// over the note's whole length.
#[test]
fn dragging_a_segment_scales_the_lane() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    let before = e.doc().clone();
    let steps = e.steps();
    let from = e.lane_at(MpeLane::Slide, BAR / 2, 60.0);
    e.drag(from, from + vec2(0.0, 10.0), Modifiers::NONE);
    let slide = e.notes()[0].slide.clone();
    assert!(
        slide[1].value < 110.0 && slide[0].value < 10.0 + 0.001,
        "{slide:?}"
    );
    e.one_step(&before, steps, "Scale Expression");
}

// Spec: mpe R30. Draw Mode draws a Slide envelope across the lane in one
// undo step.
#[test]
fn draw_mode_draws_steps_in_a_lane() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    e.run("view.draw_mode", json!({}));
    let before = e.doc().clone();
    let steps = e.steps();
    let from = e.lane_at(MpeLane::Slide, BAR / 4, 100.0);
    let to = e.lane_at(MpeLane::Slide, BAR / 2, 30.0);
    e.drag(from, to, Modifiers::NONE);
    let slide = e.notes()[0].slide.clone();
    assert!(slide.len() > 4, "steps were drawn: {slide:?}");
    assert!(slide.iter().any(|p| p.value > 90.0));
    e.one_step(&before, steps, "Draw Expression");
}

// Spec: mpe R28. Cmd with the grid off snaps a Pitch breakpoint to the
// nearest semitone.
#[test]
fn cmd_with_the_grid_off_snaps_pitch_to_a_semitone() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    let from = e.pitch_at(60, BAR / 2, 6.0);
    let to = e.pitch_at(60, BAR / 2, 8.4);
    e.drag(from, to, bypass_key());
    let value = e.notes()[0].pitch[1].value;
    assert_eq!(value, 8.0, "snapped to the semitone");
}

// Spec: mpe R27. Cmd+4 switches the tab's grid alone; with it on a moved
// breakpoint lands on a grid line.
#[test]
fn the_tabs_grid_snaps_a_moved_breakpoint() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    e.h.shell.set_focused_area(bl_shell::Area::Detail);
    let snap = e.h.shell.arrangement().grid().snap;
    e.h.key(Key::Num4, command_key());
    assert!(e.h.shell.clip_view().mpe().grid_on(e.clip));
    assert_eq!(e.h.shell.arrangement().grid().snap, snap, "only the tab's");
    let step = e.h.shell.arrangement().grid_ticks(e.doc()).get();
    let from = e.pitch_at(60, BAR / 2, 6.0);
    let to = e.pitch_at(60, BAR / 2 + step + step / 3, 6.0);
    e.drag(from, to, Modifiers::NONE);
    let at = e.notes()[0].pitch[1].at.get();
    assert_eq!(at % step, 0, "on a grid line: {at}");
    e.h.key(Key::Num4, command_key());
    assert!(!e.h.shell.clip_view().mpe().grid_on(e.clip));
}

// Spec: mpe R26. Edit Value in a breakpoint's menu types its value.
#[test]
fn edit_value_types_a_breakpoints_value() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    let at = e.lane_at(MpeLane::Slide, 0, 10.0);
    e.h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        e.h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    e.h.settle();
    let item = e.node_centre("Edit Value");
    e.click(item, Modifiers::NONE);
    let steps = e.steps();
    for key in [Key::Num5, Key::Num0, Key::Enter] {
        e.h.key(key, Modifiers::NONE);
    }
    assert_eq!(e.notes()[0].slide[0].value, 50.0);
    assert_eq!(e.steps(), steps + 1);
}

// Spec: mpe R31. Clear All Envelopes, in the note area's context menu.
#[test]
fn clear_all_envelopes_is_in_the_context_menu() {
    let mut e = set(vec![shaped(60, 0), shaped(64, 2 * BAR)]);
    e.select(&[0]);
    let at = e.lane_at(MpeLane::Slide, BAR / 3, 120.0);
    e.h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        e.h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    e.h.settle();
    let item = e.node_centre("Clear All Envelopes");
    let steps = e.steps();
    e.click(item, Modifiers::NONE);
    let notes = e.notes();
    assert!(!notes[0].has_expression() && notes[1].has_expression());
    assert_eq!(e.steps(), steps + 1);
}

// Spec: mpe R34. The lanes, the toggles and the selected notes'
// breakpoints are in the tree with names and values.
#[test]
fn the_lanes_and_a_notes_breakpoints_are_in_the_tree() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    let output = e.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Slide lane").value, "0 to 127");
    assert_eq!(Harness::node(&nodes, "Pressure lane").value, "0 to 127");
    let list = Harness::node(&nodes, "Slide of C3");
    assert_eq!(list.value, "2 breakpoints");
    let items: Vec<&common::Node> = nodes
        .iter()
        .filter(|n| n.label.starts_with("Slide breakpoint at tick"))
        .collect();
    assert_eq!(items.len(), 2);
    let mut values: Vec<&str> = items.iter().map(|n| n.value.as_str()).collect();
    values.sort_unstable();
    assert_eq!(values, ["10", "110"]);
    let pitch = Harness::node(&nodes, "Pitch of C3");
    assert_eq!(pitch.value, "3 breakpoints");
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Pitch breakpoint at tick 0" && n.value == "+0.00 st")
    );
    let _ = tick_at;
}

// Spec: mpe R23, I-DRAG-055. Dragging the split line under a lane resizes
// it, and dragging the split above the lanes to the bottom hides them; both
// are view state.
#[test]
fn a_split_line_resizes_a_lane_and_dragging_the_lanes_away_hides_them() {
    let mut e = set(vec![shaped(60, 0)]);
    let steps = e.steps();
    let layout = e.layout();
    let slide = layout.lane(MpeLane::Slide).unwrap();
    let split = pos2(slide.center().x, slide.bottom());
    e.drag(split, split + vec2(0.0, 12.0), Modifiers::NONE);
    let state =
        e.h.shell
            .clip_view()
            .state(e.doc(), e.h.shell.arrangement());
    assert!(
        (state.mpe.lanes[0].height - (slide.height() + 12.0)).abs() < 3.0,
        "{:?}",
        state.mpe.lanes
    );
    // The split above the lanes, dragged to the bottom, hides them and
    // their toggles.
    let layout = e.layout();
    let top = pos2(layout.band.center().x, layout.band.top() - 2.0);
    e.drag(top, top + vec2(0.0, 400.0), Modifiers::NONE);
    assert!(e.layout().lanes.is_empty());
    assert!(e.layout().toggles.is_empty());
    assert_eq!(e.steps(), steps);
}

// Spec: mpe R26. Clicking near a segment, or Shift-clicking on it, selects
// the segment, and dragging a selected segment moves it in value.
#[test]
fn a_selected_segment_moves_in_value() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    e.run(
        "view.mpe_lanes",
        json!({ "lane": "slide", "height": 200.0 }),
    );
    let on = e.lane_at(MpeLane::Slide, BAR / 2, 60.0);
    e.click(on, Modifiers::SHIFT);
    let state =
        e.h.shell
            .clip_view()
            .state(e.doc(), e.h.shell.arrangement());
    assert_eq!(state.mpe.breakpoints.len(), 2, "both ends: {state:?}");
    assert_eq!(e.notes()[0].slide.len(), 2, "Shift-click adds nothing");
    let before = e.doc().clone();
    let steps = e.steps();
    let up = e.lane_at(MpeLane::Slide, BAR / 2, 80.0);
    e.drag(on, up, Modifiers::NONE);
    let slide = e.notes()[0].slide.clone();
    assert_eq!(slide[0].at, Ticks::ZERO, "moved in value only: {slide:?}");
    assert!((slide[0].value - 10.0).abs() > 5.0 && (slide[1].value - 110.0).abs() > 5.0);
    e.one_step(&before, steps, "Move Expression Breakpoints");
}

// Spec: mpe R28. Shift keeps a drag to the axis it starts along.
#[test]
fn shift_keeps_a_drag_to_its_axis() {
    let mut e = set(vec![shaped(60, 0)]);
    e.select(&[0]);
    let from = e.pitch_at(60, BAR / 2, 6.0);
    // Mostly sideways, a little up.
    let to = e.pitch_at(60, BAR / 2 + QUARTER, 7.0);
    e.drag(from, to, Modifiers::SHIFT);
    let moved = e.notes()[0].pitch[1];
    assert!(moved.at > Ticks(BAR / 2), "{moved:?}");
    assert_eq!(moved.value, 6.0, "the value is held: {moved:?}");
}

// Spec: mpe R25, R26. Dragging a Velocity marker sets the note's value
// (`note.set_value`), as one undo step.
#[test]
fn dragging_a_velocity_marker_sets_the_notes_velocity() {
    let mut e = set(vec![shaped(60, 0)]);
    e.run(
        "view.mpe_lanes",
        json!({ "lane": "velocity", "show": true }),
    );
    e.select(&[0]);
    let before = e.doc().clone();
    let steps = e.steps();
    let from = e.lane_at(MpeLane::Velocity, 0, 100.0) + vec2(1.5, 0.0);
    let to = e.lane_at(MpeLane::Velocity, 0, 60.0) + vec2(1.5, 0.0);
    e.drag(from, to, Modifiers::NONE);
    let velocity = e.notes()[0].velocity;
    assert!((55..=66).contains(&velocity), "{velocity}");
    e.one_step(&before, steps, "Set note values");
}
