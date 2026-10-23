//! The Clip View's note editing, headless (WP-2.3): clicking, dragging, and
//! the marquee reach the same note commands agents run, and each edit is one
//! undo step however many frames its drag spans. Spec: piano-roll.

mod common;

use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, Document, MidiData, Note, TrackId};
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::factory_default_set;
use bl_shell::note_commands::NoteField;
use bl_ui::arrangement::benchmark;
use bl_ui::clip_view::{
    KEY_HEIGHT, KEY_WIDTH, grid_rect, lane_rect, note_rect, shown_keys, strip_rect, tick_at,
    x_of_tick, y_of_key, y_of_value,
};
use common::{Harness, Node, Window, command};
use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, TouchPhase, pos2, vec2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// The modifier that bypasses snapping while dragging: Cmd on macOS, Alt
/// elsewhere.
fn bypass_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::ALT
    }
}

struct Notes {
    h: Harness,
    clip: ClipId,
}

/// A song with a sixteen-bar MIDI clip on the first MIDI track, selected
/// and open in the Clip View, drawn at the reference size.
fn set(notes: Vec<Note>) -> Notes {
    set_with(Ticks::ZERO, notes)
}

/// As [`set`], with the clip's Start marker — where in its contents it
/// starts playing — at `marker`.
fn set_with(marker: Ticks, notes: Vec<Note>) -> Notes {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let id: ClipId = editor.new_id();
    let midi = MidiData {
        start: marker,
        ..MidiData::new(notes)
    };
    editor
        .apply(Command::CreateClip {
            track: tracks[0],
            clip: Clip::new(id, Ticks::ZERO, Ticks(16 * BAR), ClipData::Midi(midi)),
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
    h.settle();
    Notes { h, clip: id }
}

impl Notes {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn length(&self) -> Ticks {
        self.doc().clip(self.clip).unwrap().length
    }

    fn notes(&self) -> Vec<Note> {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => notes_in_order(midi),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    /// The clip's Start marker: where in its contents the clip starts
    /// playing, which the editor draws at the clip's start.
    fn marker(&self) -> Ticks {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => midi.start,
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    /// The clip's loop brace and switch, as the document holds them.
    fn loop_brace(&self) -> (Ticks, Option<Ticks>, bool) {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => (midi.loop_start, midi.loop_end, midi.looping),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    /// The editing grid's step under the Clip View: one grid step.
    fn step(&self) -> Ticks {
        self.h.shell.arrangement().grid_ticks(self.doc())
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    /// The Clip View's rectangle, from its accessible node.
    fn view(&mut self) -> Rect {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let node = Harness::node(&nodes, "Clip View");
        let bounds = node.bounds.expect("the Clip View has bounds");
        Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        )
    }

    /// The note area's rectangle.
    fn grid(&mut self) -> Rect {
        grid_rect(self.view())
    }

    /// The lane's rectangle under the note area.
    fn lane(&mut self) -> Rect {
        lane_rect(self.view())
    }

    /// A point in the lane at `tick`'s x — a note's stem when the tick is
    /// its start — and the height of `value` of `field`.
    fn lane_at(&mut self, tick: Ticks, field: NoteField, value: f64) -> Pos2 {
        let grid = self.grid();
        let lane = self.lane();
        pos2(
            x_of_tick(grid, self.marker(), self.length(), tick) + 1.0,
            y_of_value(lane, field, value),
        )
    }

    /// A point on a note's row while folded (F), where only the rows that
    /// hold notes show.
    fn at_folded(&mut self, key: u8, tick: Ticks) -> Pos2 {
        let keys = shown_keys(true, &self.notes());
        let row = keys
            .iter()
            .position(|shown| *shown == key)
            .expect("a shown row");
        let grid = self.grid();
        let first = self.h.main.clip_view().first_row();
        let y = grid.top() + (row as f32 - first) * KEY_HEIGHT + KEY_HEIGHT / 2.0;
        pos2(x_of_tick(grid, self.marker(), self.length(), tick), y)
    }

    /// The marker strip's rectangle, from the Clip View's accessible node.
    fn strip(&mut self) -> Rect {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let node = Harness::node(&nodes, "Clip View");
        let bounds = node.bounds.expect("the Clip View has bounds");
        let view = Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        );
        strip_rect(view)
    }

    /// A point in the marker strip at `tick`: in the strip's upper part for
    /// a marker, its lower part for the loop brace.
    fn at_strip(&mut self, tick: Ticks, marker: bool) -> Pos2 {
        let strip = self.strip();
        let grid = self.grid();
        let x = x_of_tick(grid, self.marker(), self.length(), tick);
        let y = if marker {
            strip.top() + 4.0
        } else {
            strip.bottom() - 3.0
        };
        pos2(x, y)
    }

    /// A point in the note area: `tick` contents ticks into the clip, in the
    /// middle of the row of `key`.
    fn at(&mut self, key: u8, tick: Ticks) -> Pos2 {
        let grid = self.grid();
        let start = self.marker();
        let top = self.h.main.clip_view().top();
        pos2(
            x_of_tick(grid, start, self.length(), tick),
            y_of_key(grid, top, key) + KEY_HEIGHT / 2.0,
        )
    }

    /// The contents tick under a point, as the view reads it.
    fn tick(&mut self, at: Pos2) -> Ticks {
        let grid = self.grid();
        let start = self.marker();
        tick_at(grid, start, self.length(), at.x)
    }

    /// A click with modifiers held.
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

    /// A press, a drag through eight frames, and a release, with modifiers
    /// held throughout.
    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        self.drag_frames(from, to, modifiers);
        self.release(to, modifiers);
    }

    /// The press and drag frames of a drag, without its release.
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

    /// The insert marker, in ticks from the clip's contents.
    fn insert(&mut self) -> Option<Ticks> {
        self.h
            .shell
            .clip_view()
            .insert_marker(self.doc(), self.h.shell.arrangement())
    }

    /// The time selection, in ticks from the clip's contents.
    fn time(&mut self) -> Option<(Ticks, Ticks)> {
        self.h
            .shell
            .clip_view()
            .time_selection(self.doc(), self.h.shell.arrangement())
    }

    /// The selected notes, by their place in the clip's notes in order.
    fn selected(&mut self) -> Vec<u32> {
        self.h
            .shell
            .clip_view()
            .selected_notes(self.doc(), self.h.shell.arrangement())
            .to_vec()
    }

    /// A point on the key `key` in the piano ruler.
    fn key_at(&mut self, key: u8) -> Pos2 {
        let grid = self.grid();
        let top = self.h.main.clip_view().top();
        pos2(
            grid.left() - KEY_WIDTH / 2.0,
            y_of_key(grid, top, key) + KEY_HEIGHT / 2.0,
        )
    }

    /// Checks the newest step is exactly one, labelled `label`, and that
    /// undo and redo restore the set exactly.
    fn one_step(&mut self, before: &Document, label: &str) {
        let after = self.doc().clone();
        assert_ne!(after, *before, "the gesture changed the set");
        assert_eq!(
            self.h
                .shell
                .editor()
                .undo_history()
                .next()
                .map(str::to_owned),
            Some(label.to_owned())
        );
        self.h.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), *before, "one undo restores the set exactly");
        assert!(self.doc().check_invariants().is_ok());
        self.h.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redo restores the edit exactly");
    }
}

#[test]
fn drawing_needs_draw_mode_and_makes_a_note_one_grid_step_long() {
    // Spec: piano-roll R2, R8. Draw Mode is B (S-GRID-001).
    let mut n = set(Vec::new());
    let step = n.step();
    // With Draw Mode off, clicking empty grid adds nothing.
    let at = n.at(60, step + step / 8);
    n.click(at, Modifiers::NONE);
    assert!(
        n.notes().is_empty(),
        "a click without Draw Mode draws nothing"
    );
    assert_eq!(n.steps(), 0);
    n.h.key(egui::Key::B, Modifiers::NONE);
    n.h.settle();
    assert!(n.h.shell.clip_view().draw_mode(), "B switches Draw Mode on");
    // A click a little after a grid line lands on the line, one step long.
    let before = n.doc().clone();
    let at = n.at(60, step + step / 8);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.notes(), [Note::new(60, step, step, 100)]);
    n.one_step(&before, "Add note");
    // Clicking the note removes it, also one step.
    let before = n.doc().clone();
    let at = n.at(60, step + step / 2);
    n.click(at, Modifiers::NONE);
    assert!(n.notes().is_empty(), "drawing over a note removes it");
    n.one_step(&before, "Delete notes");
}

#[test]
fn dragging_moves_notes_by_grid_steps_and_semitones() {
    // Spec: piano-roll R7, R10. The dragged note snaps in time and moves by
    // whole semitones.
    let mut n = set(vec![Note::new(60, Ticks::ZERO, n_step(), 100)]);
    assert_eq!(n.notes().len(), 1);
    let step = n.step();
    let before = n.doc().clone();
    let from = n.at(60, step / 2);
    let to = n.at(62, 2 * step + step / 2);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(
        n.notes(),
        [Note::new(62, 2 * step, step, 100)],
        "two grid steps later, two semitones up"
    );
    n.one_step(&before, "Move notes");
}

#[test]
fn a_drag_with_cmd_lands_off_the_grid() {
    // Spec: piano-roll R10. Cmd (Alt on Windows) bypasses snapping.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let from = n.at(60, step / 2);
    let to = n.at(60, 2 * step + step / 2 + step / 5);
    let wanted = n.tick(to) - n.tick(from);
    n.drag(from, to, bypass_key());
    let start = n.notes()[0].start;
    assert!(
        (start - wanted).get().abs() <= 2,
        "the note lands where the pointer is, {start:?} vs {wanted:?}"
    );
    assert_ne!(start.get() % step.get(), 0, "and not on a grid line");
}

#[test]
fn resizing_a_note_respects_the_grid_and_keeps_one_step() {
    // Spec: piano-roll R9. Dragging a note's end snaps to the grid, and a
    // note keeps at least one grid step while snapping.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let before = n.doc().clone();
    let from = n.at(60, step);
    let to = n.at(60, 3 * step + step / 4);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.notes()[0].length, 3 * step, "the end snapped");
    assert_eq!(n.notes()[0].start, Ticks::ZERO, "the start stayed");
    n.one_step(&before, "Resize notes");
    // Dragging the end back over the start keeps one grid step.
    let from = n.at(60, 3 * step);
    let to = n.at(60, step / 8);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(
        n.notes()[0].length,
        step,
        "shortening past a step stops at one"
    );
}

#[test]
fn a_marquee_selects_exactly_the_notes_it_touches() {
    // Spec: piano-roll R5, R6. Selecting is never an undo step.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 100),
        Note::new(60, 8 * step, step, 100),
    ]);
    let steps = n.steps();
    let from = n.at(59, 2 * step);
    let to = n.at(66, 5 * step);
    n.drag(from, to, Modifiers::NONE);
    let selected =
        n.h.shell
            .clip_view()
            .selected_notes(n.doc(), n.h.shell.arrangement())
            .to_vec();
    assert_eq!(selected, [1], "the note the marquee ran over, and no other");
    assert_eq!(n.steps(), steps, "selecting is never an undo step");
    // A plain click selects one note alone; Cmd-click toggles it off.
    let at = n.at(64, 4 * step + step / 2);
    n.click(at, Modifiers::NONE);
    let selected =
        n.h.shell
            .clip_view()
            .selected_notes(n.doc(), n.h.shell.arrangement())
            .to_vec();
    assert_eq!(selected, [1], "a click selects the note under it");
    let second = n.at(60, step / 2);
    n.click(second, Modifiers::NONE);
    let selected =
        n.h.shell
            .clip_view()
            .selected_notes(n.doc(), n.h.shell.arrangement())
            .to_vec();
    assert_eq!(
        selected,
        [0],
        "a later click selects another, got {selected:?}"
    );
    // Cmd-click toggles a note out of the selection.
    n.click(second, command());
    let selected =
        n.h.shell
            .clip_view()
            .selected_notes(n.doc(), n.h.shell.arrangement())
            .to_vec();
    assert!(
        selected.is_empty(),
        "Cmd-click removes it, got {selected:?}"
    );
    assert_eq!(n.steps(), steps, "and none of it is an undo step");
}

#[test]
fn esc_cancels_a_drag_and_records_nothing() {
    // Spec: piano-roll R11. Esc puts the set and the selection back.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let before = n.doc().clone();
    let steps = n.steps();
    let from = n.at(60, step / 2);
    let to = n.at(62, 2 * step + step / 2);
    n.drag_frames(from, to, Modifiers::NONE);
    assert_ne!(*n.doc(), before, "the drag moved the note as it went");
    n.h.key(egui::Key::Escape, Modifiers::NONE);
    n.release(to, Modifiers::NONE);
    assert_eq!(*n.doc(), before, "Esc puts the set back");
    assert_eq!(n.steps(), steps, "and records no step");
}

#[test]
fn the_editor_reads_the_clips_start_marker() {
    // Spec: piano-roll R4. Contents tick `MidiData::start` — where the clip
    // starts playing — sits at the clip's start.
    let step = n_step();
    let mut n = set_with(4 * step, vec![Note::new(60, 4 * step, step, 100)]);
    let grid = n.grid();
    let note = &n.notes()[0];
    let top = n.h.main.clip_view().top();
    let rect = note_rect(grid, n.marker(), n.length(), top, note);
    assert!(
        (rect.left() - grid.left()).abs() < 1.0,
        "a note at the marker sits at the clip's start, {:?} vs {:?}",
        rect.left(),
        grid.left()
    );
    // Drawing just after the left edge lands on the marker's line.
    n.h.key(egui::Key::B, Modifiers::NONE);
    n.h.settle();
    let at = n.at(58, 4 * step + step / 8);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.notes()[0].start, 4 * step, "on the editor's first line");
    assert_eq!(n.notes()[0].key, 58);
}

#[test]
fn the_wheel_scrolls_the_key_track() {
    // Spec: piano-roll R12. The note editor shows a window of the key
    // track; the wheel moves it, three rows to a notch.
    let mut n = set(vec![Note::new(60, Ticks::ZERO, n_step(), 100)]);
    let before = n.h.main.clip_view().top();
    let at = n.at(60, Ticks::ZERO);
    n.h.frame(vec![
        Event::PointerMoved(at),
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, KEY_HEIGHT * 3.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        },
    ]);
    // egui smooths the wheel's delta over the frames after the event.
    for _ in 0..30 {
        n.h.frame(Vec::new());
    }
    let after = n.h.main.clip_view().top();
    assert!(
        (after - before - 3.0).abs() < 0.01,
        "three rows with one wheel notch, moved {}",
        after - before
    );
}

#[test]
fn the_arrow_keys_transpose_the_selection_a_semitone_or_an_octave() {
    // Spec: piano-roll (WP-2.3 part 3). S-A11Y-008 and S-A11Y-009: with
    // notes selected, Up and Down transpose by a semitone, Shift by an
    // octave, each running `note.move` as one undo step.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    // A click selects the note and gives the Clip View the keyboard.
    let at = n.at(60, step / 2);
    n.click(at, Modifiers::NONE);
    let before = n.doc().clone();
    n.h.key(egui::Key::ArrowUp, Modifiers::NONE);
    assert_eq!(n.notes()[0].key, 61, "a semitone up");
    n.one_step(&before, "Move notes");
    n.h.key(egui::Key::ArrowUp, Modifiers::SHIFT);
    assert_eq!(n.notes()[0].key, 73, "an octave with Shift");
    n.h.key(egui::Key::ArrowDown, Modifiers::SHIFT);
    assert_eq!(n.notes()[0].key, 61, "and down again");
    n.h.key(egui::Key::ArrowDown, Modifiers::NONE);
    assert_eq!(n.notes()[0].key, 60);
}

#[test]
fn transposing_past_the_top_key_is_refused_and_changes_nothing() {
    // Spec: piano-roll (WP-2.3 part 3). A note at key 127 can't go higher:
    // the edit is refused, the set is unchanged, and no step is recorded.
    let step = n_step();
    let mut n = set(vec![Note::new(127, Ticks::ZERO, step, 100)]);
    let at = n.at(127, step / 2);
    n.click(at, Modifiers::NONE);
    let before = n.doc().clone();
    let steps = n.steps();
    n.h.key(egui::Key::ArrowUp, Modifiers::SHIFT);
    assert_eq!(*n.doc(), before, "nothing moved");
    assert_eq!(n.steps(), steps, "and no step was recorded");
}

#[test]
fn cmd_u_quantises_the_selected_notes() {
    // Spec: piano-roll (WP-2.3 part 3). Cmd-U (S-NOTE-011) runs
    // `note.quantize` with the view's settings — the editing grid, note
    // starts, 100 percent.
    let step = n_step();
    let mut n = set(vec![Note::new(60, step / 3, step, 100)]);
    let at = n.at(60, step / 3 + step / 2);
    n.click(at, Modifiers::NONE);
    let before = n.doc().clone();
    n.h.key(egui::Key::U, command());
    assert_eq!(
        n.notes()[0].start,
        Ticks::ZERO,
        "the note lands on the grid line before it"
    );
    n.one_step(&before, "Quantize notes");
}

#[test]
fn dragging_the_loop_brace_edits_the_clip_in_one_step() {
    // Spec: piano-roll (WP-2.3 part 3). The brace's start drags along the
    // strip's lower edge, snapping to the editing grid, as one undo step.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    assert_eq!(
        n.loop_brace(),
        (Ticks::ZERO, None, false),
        "a new clip's brace is the whole clip, unlooped"
    );
    let before = n.doc().clone();
    let from = n.at_strip(Ticks::ZERO, false);
    let to = n.at_strip(4 * step, false);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(
        n.loop_brace(),
        (4 * step, None, false),
        "the brace's start moved on the grid"
    );
    n.one_step(&before, "Change clip loop");
}

#[test]
fn dragging_the_start_marker_edits_the_clips_region_in_one_step() {
    // Spec: piano-roll (WP-2.3 part 3). The Start marker drags in the
    // strip's upper part: its new place becomes the contents tick the clip
    // starts playing from (R4), as one undo step.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let before = n.doc().clone();
    let from = n.at_strip(Ticks::ZERO, true);
    let to = n.at_strip(4 * step, true);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.marker(), 4 * step, "the Start marker moved");
    n.one_step(&before, "Change clip region");
}

#[test]
fn dragging_a_stem_sets_that_notes_velocity_and_nothing_else() {
    // Spec: piano-roll R14. A drag on a note's stem in the lane sets that
    // note's velocity — the lane's default value — and the whole drag is
    // one undo step.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 40),
    ]);
    let before = n.doc().clone();
    // Press on the first note's stem at its own velocity and draw down to
    // 20: only that note changes.
    let from = n.lane_at(Ticks::ZERO, NoteField::Velocity, 100.0);
    let to = n.lane_at(Ticks::ZERO, NoteField::Velocity, 20.0);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.notes()[0].velocity, 20);
    assert_eq!(n.notes()[1].velocity, 40, "the other note is untouched");
    n.one_step(&before, "Set note values");
    // The same drag in the chance lane sets chances.
    n.h.shell
        .run(bl_shell::commands::VIEW_LANE, json!({ "lane": "chance" }))
        .unwrap();
    let before = n.doc().clone();
    let from = n.lane_at(Ticks::ZERO, NoteField::Chance, 1.0);
    let to = n.lane_at(Ticks::ZERO, NoteField::Chance, 0.25);
    n.drag(from, to, Modifiers::NONE);
    let chance = n.notes()[0].probability;
    assert!(
        (chance - 0.25).abs() < 0.05,
        "the chance follows the pointer, {chance}"
    );
    assert_eq!(n.notes()[1].probability, 1.0, "and nothing else");
    n.one_step(&before, "Set note values");
}

#[test]
fn dragging_across_notes_draws_a_ramp() {
    // Spec: piano-roll R14. A drag across several notes ramps their values
    // evenly, from where it began to the pointer, as one undo step.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 20),
        Note::new(62, 4 * step, step, 20),
        Note::new(64, 8 * step, step, 20),
    ]);
    let before = n.doc().clone();
    let from = n.lane_at(Ticks::ZERO, NoteField::Velocity, 20.0);
    let to = n.lane_at(8 * step, NoteField::Velocity, 120.0);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(
        n.notes()
            .iter()
            .map(|note| note.velocity)
            .collect::<Vec<_>>(),
        [20, 70, 120],
        "evenly spaced from the first note to the last"
    );
    n.one_step(&before, "Ramp note values");
}

#[test]
fn folding_shows_only_the_rows_that_hold_notes() {
    // Spec: piano-roll R15. Fold (F) hides every row without a note, so the
    // notes sit on neighbouring rows; folding is view state and isn't an
    // undo step.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 100),
    ]);
    assert_eq!(
        shown_keys(true, &n.notes()),
        [64, 60],
        "only the rows that hold notes"
    );
    assert_eq!(
        shown_keys(false, &n.notes()).len(),
        128,
        "unfolded, every row shows"
    );
    let steps = n.steps();
    n.h.shell.set_focused_area(bl_shell::Area::Detail);
    n.h.key(egui::Key::F, Modifiers::NONE);
    n.h.settle();
    assert!(
        n.h.shell.clip_view().is_folded(n.clip),
        "F folds the editor"
    );
    assert_eq!(n.steps(), steps, "folding isn't an undo step");
    assert!(!n.h.shell.is_dirty(), "and doesn't dirty the set");
    // The notes sit on neighbouring rows now: dragging the lower one up a
    // single row reaches the higher one's key, four semitones away.
    let before = n.doc().clone();
    let from = n.at_folded(60, step / 2);
    let to = n.at_folded(64, step / 2);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.notes()[0].key, 64, "one row up is four semitones");
    assert_eq!(n.notes()[0].start, Ticks::ZERO, "and the note stayed put");
    n.one_step(&before, "Move notes");
}

/// The default editing grid's step, for a test that builds its clip with
/// notes of one step.
fn n_step() -> Ticks {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(factory_default_set());
    h.settle();
    h.shell
        .arrangement()
        .grid_ticks(h.shell.editor().document())
}

/// The modifier that moves the insert marker to a note edge: Option on
/// macOS, Ctrl elsewhere (S-A11Y-006).
fn edge_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    }
}

/// The modifier that adds to the selection while jumping between notes:
/// Option with Shift on macOS, Ctrl with Shift elsewhere (S-NOTE-043).
fn add_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT | Modifiers::SHIFT
    } else {
        Modifiers::CTRL | Modifiers::SHIFT
    }
}

/// A right-click at `at`, which opens the editor's context menu.
fn right_click(n: &mut Notes, at: Pos2) -> Vec<Node> {
    n.h.frame(vec![Event::PointerMoved(at)]);
    n.h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    n.h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    let output = n.h.settle();
    Harness::nodes(&output)
}

/// The menu item `label`, which reads with the keys it shows after it.
fn menu_item(nodes: &[Node], label: &str) -> Node {
    nodes
        .iter()
        .find(|n| {
            n.role == egui::accesskit::Role::Button
                && (n.label == label
                    || n.label
                        .strip_prefix(label)
                        .and_then(|keys| keys.strip_prefix(' '))
                        .is_some_and(|keys| !keys.contains(' ')))
        })
        .unwrap_or_else(|| panic!("no menu item {label:?}"))
        .clone()
}

#[test]
fn a_click_places_the_insert_marker_and_the_arrows_move_it() {
    // Spec: piano-roll R23. A click on empty grid places the insert marker
    // on the nearest grid line, and Left, Right, Option-Left, Home and End
    // move it.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 100),
    ]);
    let at = n.at(62, 2 * step + step / 8);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.insert(), Some(2 * step), "on the nearest grid line");
    // Right moves it a grid step, Left back.
    n.h.key(egui::Key::ArrowRight, Modifiers::NONE);
    assert_eq!(n.insert(), Some(3 * step));
    n.h.key(egui::Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(n.insert(), Some(2 * step));
    // Option-Left moves it to the previous note edge, Option-Right to the
    // next.
    n.h.key(egui::Key::ArrowLeft, edge_key());
    assert_eq!(n.insert(), Some(step), "the first note's end");
    n.h.key(egui::Key::ArrowRight, edge_key());
    assert_eq!(n.insert(), Some(4 * step), "the second note's start");
    // Home and End move it to the clip's first and last tick.
    n.h.key(egui::Key::Home, Modifiers::NONE);
    assert_eq!(n.insert(), Some(Ticks::ZERO));
    n.h.key(egui::Key::End, Modifiers::NONE);
    assert_eq!(n.insert(), Some(n.length()));
}

#[test]
fn the_note_editors_home_leaves_the_transport_alone() {
    // Spec: piano-roll R23. The note editor reads Home itself, so the
    // transport's Home, which moves the song's insert marker, doesn't run
    // there as well.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    n.h.shell.start_audio();
    assert!(n.h.shell.audio().engine_running(), "the engine runs");
    // The song's insert marker is somewhere else.
    n.h.shell
        .run("transport.locate", json!({ "position": 4 * step }))
        .unwrap();
    n.h.wait("the song's insert marker to move", |h| {
        h.shell.arrangement().insert_marker() == 4 * step
    });
    // A click on empty grid places the editor's insert marker.
    let empty = n.at(62, 2 * step);
    n.click(empty, Modifiers::NONE);
    assert_eq!(n.insert(), Some(2 * step));
    // Home moves the editor's insert marker, and leaves the song's alone.
    n.h.key(egui::Key::Home, Modifiers::NONE);
    assert_eq!(n.insert(), Some(Ticks::ZERO), "the editor's insert marker");
    n.h.settle();
    assert_eq!(
        n.h.shell.arrangement().insert_marker(),
        4 * step,
        "the song's insert marker stays"
    );
}

#[test]
fn the_marquee_and_the_keys_make_the_time_selection() {
    // Spec: piano-roll R24. A marquee's time span is the time selection,
    // Shift with the arrows selects time from the insert marker, and Enter
    // swaps between the notes in it and the time.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 100),
    ]);
    // A marquee over the second note: its time span is the time selection.
    let from = n.at(63, 3 * step);
    let to = n.at(65, 5 * step);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.time(), Some((3 * step, 5 * step)), "the marquee's span");
    // Esc takes the selection and the time away.
    n.h.key(egui::Key::Escape, Modifiers::NONE);
    assert!(n.selected().is_empty());
    assert_eq!(n.time(), None);
    // Shift with the arrows selects time from the insert marker a step at a
    // time.
    n.h.key(egui::Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(n.time(), Some((Ticks::ZERO, step)), "one step");
    n.h.key(egui::Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(n.time(), Some((Ticks::ZERO, 2 * step)), "two steps");
    // Option-Shift with the arrows moves the time selection's end to a note
    // edge.
    n.h.key(egui::Key::ArrowRight, Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(
        n.time(),
        Some((Ticks::ZERO, 4 * step)),
        "the next note's start"
    );
    // Enter selects the notes that start inside the time, keeping it.
    n.h.key(egui::Key::Enter, Modifiers::NONE);
    assert_eq!(n.selected(), [0], "the note that starts inside it");
    assert_eq!(n.time(), Some((Ticks::ZERO, 4 * step)), "the time stays");
    // Enter again deselects the notes and keeps the time.
    n.h.key(egui::Key::Enter, Modifiers::NONE);
    assert!(n.selected().is_empty());
    assert_eq!(n.time(), Some((Ticks::ZERO, 4 * step)), "the time stays");
}

#[test]
fn the_keyboard_selects_the_notes() {
    // Spec: piano-roll R25. Option-Down and Option-Up select the note
    // nearest the insert marker and then the previous or next note, and
    // with Shift the note is added to the selection.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 100),
        Note::new(60, 8 * step, step, 100),
    ]);
    // A click places the insert marker between the notes.
    let at = n.at(62, step);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.insert(), Some(step));
    // Option-Down selects the nearest note.
    n.h.key(egui::Key::ArrowDown, Modifiers::ALT);
    assert_eq!(n.selected(), [0], "the nearest note's start");
    // Option-Down again has no earlier note to go to, and Option-Up the
    // next one.
    n.h.key(egui::Key::ArrowDown, Modifiers::ALT);
    assert_eq!(n.selected(), [0], "and stays, with no earlier note");
    n.h.key(egui::Key::ArrowUp, Modifiers::ALT);
    assert_eq!(n.selected(), [1], "and the next one");
    // Option-Shift with the arrows adds the note to the selection.
    n.h.key(egui::Key::ArrowDown, add_key());
    assert_eq!(n.selected(), [0, 1], "the previous note is added");
    // Option-Left and Option-Right select the previous or next note of the
    // same pitch.
    let first = n.at(60, step / 2);
    n.click(first, Modifiers::NONE);
    n.h.key(egui::Key::ArrowRight, Modifiers::ALT);
    assert_eq!(n.selected(), [2], "the next note at that pitch");
    n.h.key(egui::Key::ArrowLeft, Modifiers::ALT);
    assert_eq!(n.selected(), [0], "and the previous one");
    // Esc deselects; with nothing selected, Option-Shift-Down adds the note
    // nearest the insert marker (S-NOTE-043, S-A11Y-007).
    n.h.key(egui::Key::Escape, Modifiers::NONE);
    assert!(n.selected().is_empty());
    n.h.key(egui::Key::ArrowDown, add_key());
    assert_eq!(n.selected(), [0], "the nearest note is added");
}

#[test]
fn shift_clicking_and_the_piano_ruler_select_the_notes() {
    // Spec: piano-roll R26. Shift-clicking a note adds it to the selection
    // or takes it out, and clicking a key in the piano ruler selects the
    // notes at that pitch, which Shift-clicking adds or takes out.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 100),
    ]);
    // Shift-clicking a note adds it to the selection.
    let first = n.at(60, step / 2);
    let second = n.at(64, 4 * step + step / 2);
    n.click(first, Modifiers::NONE);
    assert_eq!(n.selected(), [0]);
    n.click(second, Modifiers::SHIFT);
    assert_eq!(n.selected(), [0, 1], "Shift-click adds it");
    n.click(second, Modifiers::SHIFT);
    assert_eq!(n.selected(), [0], "and takes it out again");
    // Clicking a key in the piano ruler selects the notes at that pitch.
    let key = n.key_at(60);
    n.click(key, Modifiers::NONE);
    assert_eq!(n.selected(), [0], "the notes at that pitch");
    let other = n.key_at(64);
    n.click(other, Modifiers::SHIFT);
    assert_eq!(n.selected(), [0, 1], "Shift-click adds the other key's");
    n.click(other, Modifiers::SHIFT);
    assert_eq!(n.selected(), [0], "and takes them out again");
}

#[test]
fn the_arrows_move_and_lengthen_the_selection() {
    // Spec: piano-roll R27. Left and Right move the selection a grid step,
    // and Shift with them lengthens or shortens it.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let at = n.at(60, step / 2);
    n.click(at, Modifiers::NONE);
    let before = n.doc().clone();
    n.h.key(egui::Key::ArrowRight, Modifiers::NONE);
    assert_eq!(n.notes()[0].start, step, "a grid step later");
    n.one_step(&before, "Move notes");
    // Shift with the arrows lengthens the note.
    let before = n.doc().clone();
    n.h.key(egui::Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(n.notes()[0].length, 2 * step, "a grid step longer");
    n.one_step(&before, "Resize notes");
    // A move that would start before the clip's contents is refused.
    n.h.key(egui::Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(n.notes()[0].start, Ticks::ZERO, "it moves to the contents");
    let before = n.doc().clone();
    n.h.key(egui::Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(*n.doc(), before, "and stops there");
}

#[test]
fn option_dragging_a_note_copies_it() {
    // Spec: piano-roll R29. Option-dragging a note copies it: the note
    // stays where it is and the copy lands where it would have gone.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let from = n.at(60, step / 2);
    let to = n.at(60, 4 * step + step / 2);
    let copy = if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    };
    n.drag(from, to, copy);
    assert_eq!(
        n.notes(),
        [
            Note::new(60, Ticks::ZERO, step, 100),
            Note::new(60, 4 * step, step, 100),
        ],
        "the note stays and the copy lands where it would have"
    );
}

#[test]
fn the_context_menu_lists_the_items_in_order() {
    // Spec: piano-roll R30. Right-clicking a note opens the editor's
    // context menu: the items whose commands exist run them, and the rest
    // are drawn disabled and name their packages.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let at = n.at(60, step / 2);
    let nodes = right_click(&mut n, at);
    // The note the menu opened on is selected alone.
    assert_eq!(n.selected(), [0]);
    for label in ["Cut", "Copy", "Paste", "Duplicate", "Delete"] {
        let node = menu_item(&nodes, label);
        assert!(!node.disabled, "{label} works");
    }
    for (label, package) in [
        ("Invert Selection", "WP-6.4"),
        ("Deactivate Note(s)", "WP-6.1"),
        ("Group Notes", "WP-6.2"),
        ("Ungroup Notes", "WP-6.2"),
        ("Highlight Scales", "WP-6.7b"),
    ] {
        let node = menu_item(&nodes, label);
        assert!(node.disabled, "{label} waits");
        assert!(
            node.description.contains(package),
            "{label} names {package}: {:?}",
            node.description
        );
    }
    // Spec: note-editing R35, R39, R40, R41. WP-6.3's items run their
    // commands; Split Note(s) shows only while a note would be split.
    for label in [
        "Chop Note(s) on Grid",
        "Join Notes",
        "Fit to Time Range",
        "Split Clip",
    ] {
        assert!(!menu_item(&nodes, label).disabled, "{label} works");
    }
    assert!(
        !nodes
            .iter()
            .any(|node| node.label.starts_with("Split Note(s)")),
        "no note would split"
    );
}

#[test]
fn split_notes_shows_in_the_menu_while_the_insert_marker_crosses_a_note() {
    // Spec: note-editing R35. C-NOTE-010 runs `note.split`, and shows only
    // while a note would be split.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, 4 * step, 100)]);
    let elsewhere = n.at(61, step);
    let nodes = right_click(&mut n, elsewhere);
    assert!(
        !nodes
            .iter()
            .any(|node| node.label.starts_with("Split Note(s)")),
        "nothing crosses the marker yet"
    );
    n.h.key(egui::Key::Escape, Modifiers::NONE);
    let at = n.at(62, 2 * step);
    n.click(at, Modifiers::NONE);
    let nodes = right_click(&mut n, elsewhere);
    let item = menu_item(&nodes, "Split Note(s)");
    assert!(!item.disabled);
}

#[test]
fn the_clipboard_keys_act_on_the_notes() {
    // Spec: piano-roll R28. The Edit menu's keys (S-EDIT-001 to S-EDIT-005)
    // copy, cut, paste, duplicate and delete the selected notes, and the
    // note clipboard is apart from the clips' clipboard.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    // A click on empty grid places the insert marker.
    let empty = n.at(62, 4 * step);
    n.click(empty, Modifiers::NONE);
    assert_eq!(n.insert(), Some(4 * step));
    // A click on the note selects it.
    let at = n.at(60, step / 2);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.selected(), [0]);
    // Copy puts the note on the note clipboard, and Paste puts it at the
    // insert marker.
    n.h.key(egui::Key::C, command());
    n.h.key(egui::Key::V, command());
    assert_eq!(
        n.notes(),
        [
            Note::new(60, Ticks::ZERO, step, 100),
            Note::new(60, 4 * step, step, 100),
        ],
        "the copy is at the insert marker"
    );
    // Duplicate lays another copy straight after the time selection, and
    // Delete removes the selected notes.
    n.h.key(egui::Key::D, command());
    assert_eq!(n.notes().len(), 3, "and another one");
    n.h.key(egui::Key::Delete, Modifiers::NONE);
    assert_eq!(n.notes().len(), 2, "the selected note is deleted");
    // Cut copies the notes and deletes them.
    let at = n.at(60, step / 2);
    n.click(at, Modifiers::NONE);
    n.h.key(egui::Key::X, command());
    assert_eq!(n.notes().len(), 1, "the cut note is gone");
}

#[test]
fn the_piano_rulers_menu_lists_its_items_and_their_packages() {
    // Spec: piano-roll R30. Right-clicking a key in the piano ruler opens
    // C-KEYS: Deactivate Note(s) and the note spelling, both waiting for
    // their packages, drawn disabled and naming them.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let at = n.key_at(60);
    let nodes = right_click(&mut n, at);
    for (label, package) in [
        ("Deactivate Note(s)", "WP-6.1"),
        ("Note spelling", "WP-6.7b"),
    ] {
        let node = menu_item(&nodes, label);
        assert!(node.disabled, "{label} waits");
        assert!(
            node.description.contains(package),
            "{label} names {package}: {:?}",
            node.description
        );
    }
}

#[test]
fn the_insert_marker_and_the_time_selection_reach_assistive_technology() {
    // Spec: piano-roll R23, R24. The insert marker's place and the time
    // selection reach assistive technology, as the Sample Editor's markers
    // do: an item naming it, with its place in the clip's contents.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    // A click on empty grid places the insert marker, and Shift with the
    // arrows makes the time selection.
    let empty = n.at(62, 2 * step);
    n.click(empty, Modifiers::NONE);
    n.h.key(egui::Key::ArrowRight, Modifiers::SHIFT);
    n.h.key(egui::Key::ArrowRight, Modifiers::SHIFT);
    let output = n.h.settle();
    let nodes = Harness::nodes(&output);
    let marker = Harness::node(&nodes, "Insert Marker");
    assert_eq!(marker.role, egui::accesskit::Role::ListItem);
    assert_eq!(marker.value, format!("tick {}", (2 * step).get()));
    let time = Harness::node(&nodes, "Time Selection");
    assert_eq!(time.role, egui::accesskit::Role::ListItem);
    assert_eq!(
        time.value,
        format!("tick {} to tick {}", (2 * step).get(), (4 * step).get())
    );
    // Esc takes the time selection away and leaves the insert marker.
    n.h.key(egui::Key::Escape, Modifiers::NONE);
    let output = n.h.settle();
    let nodes = Harness::nodes(&output);
    assert!(nodes.iter().all(|node| node.label != "Time Selection"));
    assert!(nodes.iter().any(|node| node.label == "Insert Marker"));
}

// Spec: view-options R1, piano-roll R44.
#[test]
fn the_middle_button_pans_the_note_area() {
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let grid = n.grid();
    let over = grid.center();
    let note = n.notes()[0].clone();
    // The note's rectangle as the editor draws it, from the key at the
    // grid's top edge.
    let before = note_rect(
        grid,
        n.marker(),
        n.length(),
        n.h.main.clip_view().top(),
        &note,
    );
    // A middle-button drag of 100 points right and 50 down pans the note
    // area with the pointer: the note follows it 50 points down the grid.
    n.h.middle_drag(over, over + vec2(100.0, 50.0));
    let after = note_rect(
        grid,
        n.marker(),
        n.length(),
        n.h.main.clip_view().top(),
        &note,
    );
    assert!(
        (after.top() - before.top() - 50.0).abs() < 0.01,
        "the note moved 50 points down with the pointer: {} to {}",
        before.top(),
        after.top()
    );
    // Nothing else changed: no selection, no note, no undo step.
    assert!(n.selected().is_empty(), "no note is selected");
    assert_eq!(n.notes().len(), 1, "the note is where it was");
    assert_eq!(n.steps(), 0, "not an undo step");
}

// Spec: piano-roll R43.
#[test]
fn the_notes_are_paler_the_softer_their_velocity() {
    let step = n_step();
    let mut muted = Note::new(67, Ticks::ZERO, step, 127);
    muted.mute = true;
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 127),
        Note::new(62, Ticks::ZERO, step, 64),
        Note::new(64, Ticks::ZERO, step, 1),
        muted,
    ]);
    let grid = n.grid();
    let start = n.marker();
    let length = n.length();
    let track = n.doc().clip_track(n.clip).expect("the clip's track");
    let full = bl_ui::arrangement::scene::track_colour(&n.h.theme, track);
    let palest = bl_ui::theme::note_colour(full, 1);
    let middle = bl_ui::theme::note_colour(full, 64);
    let output = n.h.settle();
    let top = n.h.main.clip_view().top();
    let rects = common::painted_rects(&output);
    let fill_of = |note: &Note| {
        let rect = note_rect(grid, start, length, top, note);
        rects
            .iter()
            .find(|(r, _)| *r == rect)
            .map(|(_, colour)| *colour)
            .unwrap_or_else(|| panic!("the note at key {} is painted", note.key))
    };
    let notes = n.notes();
    assert_eq!(
        fill_of(&notes[0]),
        full,
        "velocity 127 is the clip's colour"
    );
    assert_eq!(fill_of(&notes[1]), middle, "velocity 64 is between");
    assert_eq!(fill_of(&notes[2]), palest, "velocity 1 is the palest");
    assert_eq!(
        fill_of(&notes[3]),
        n.h.theme.colors.clip_muted.0,
        "a deactivated note is grey"
    );
    // The middle is between the two ends, in every channel.
    let (p, m, f) = (palest.to_array(), middle.to_array(), full.to_array());
    for channel in 0..4 {
        assert!(
            f[channel] <= m[channel] && m[channel] <= p[channel],
            "velocity 64 is between the palest and the full colour"
        );
    }
}

/// The modifier that copies while dragging, and with E chops: Option on
/// macOS, Ctrl elsewhere (S-NOTE-004).
fn chop_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

impl Notes {
    /// Presses a key and holds it, with modifiers, until `let_go`.
    fn hold(&mut self, key: egui::Key, modifiers: Modifiers) {
        self.h.frame(vec![
            Event::ModifiersChanged(modifiers),
            Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        ]);
    }

    /// Lets go of a key held with [`Notes::hold`], and of every modifier.
    fn let_go(&mut self, key: egui::Key, modifiers: Modifiers) {
        self.h.frame(vec![
            Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            },
            Event::ModifiersChanged(Modifiers::NONE),
        ]);
        self.h.settle();
    }

    /// A point in the stretch markers' band, `tick` along.
    fn at_band(&mut self, tick: Ticks) -> Pos2 {
        let strip = self.strip();
        let grid = self.grid();
        pos2(
            x_of_tick(grid, self.marker(), self.length(), tick),
            strip.top() + 7.5,
        )
    }

    /// Selects every note, which puts the time selection on their span.
    fn select_all(&mut self) {
        self.h
            .shell
            .run(
                "clip_view.select_notes",
                json!({ "clip": self.clip, "notes": [], "select": "all" }),
            )
            .unwrap();
        self.h.settle();
    }

    fn starts(&self) -> Vec<(u8, i64, i64)> {
        self.notes()
            .iter()
            .map(|n| (n.key, n.start.get(), n.length.get()))
            .collect()
    }
}

#[test]
fn e_click_splits_a_note_where_clicked_and_cmd_snaps_it() {
    // Spec: note-editing R33; S-NOTE-006. E held and a click splits the
    // note at the pointer, or at the nearest grid line with Cmd held. One
    // undo step.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, 4 * step, 100)]);
    n.hold(egui::Key::E, Modifiers::NONE);
    let before = n.doc().clone();
    let at = n.at(60, step + step / 3);
    let tick = n.tick(at);
    n.click(at, Modifiers::NONE);
    assert_eq!(
        n.starts(),
        [
            (60, 0, tick.get()),
            (60, tick.get(), 4 * step.get() - tick.get())
        ],
        "split where the pointer was"
    );
    n.one_step(&before, "Split Notes");
    assert!(n.selected().is_empty(), "the click selected nothing");
    // With Cmd held the split snaps to the nearest grid line.
    n.h.shell.run("edit.undo", json!({})).unwrap();
    n.h.settle();
    let at = n.at(60, step + step / 3);
    n.click(at, command());
    assert_eq!(
        n.starts(),
        [(60, 0, step.get()), (60, step.get(), 3 * step.get())]
    );
    n.let_go(egui::Key::E, Modifiers::NONE);
    // Without E, the same click only selects.
    let steps = n.steps();
    let at = n.at(60, 2 * step);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.steps(), steps);
}

#[test]
fn an_e_drag_splits_each_note_it_crosses_and_esc_puts_them_back() {
    // Spec: note-editing R33; I-DRAG-026. The line dragged across two
    // notes splits each where it crosses it: one undo step.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, step, 6 * step, 100),
        Note::new(64, step, 6 * step, 100),
    ]);
    n.hold(egui::Key::E, Modifiers::NONE);
    let before = n.doc().clone();
    // From above the upper note, down and right, to below the lower one.
    let from = n.at(66, 2 * step);
    let to = n.at(58, 6 * step);
    let (from_tick, to_tick) = (n.tick(from), n.tick(to));
    n.drag(from, to, Modifiers::NONE);
    let notes = n.notes();
    assert_eq!(notes.len(), 4, "each note split in two");
    let cut = |key: u8| {
        notes
            .iter()
            .filter(|note| note.key == key)
            .map(|note| note.start.get())
            .max()
            .unwrap()
    };
    // The line runs 8 rows from row 66 to row 58: row 64 is a quarter of
    // the way down it, and row 60 three quarters.
    for (key, fraction) in [(64, 0.25), (60, 0.75)] {
        let expected = from_tick.get() as f64 + (to_tick - from_tick).get() as f64 * fraction;
        assert!(
            (cut(key) as f64 - expected).abs() < step.get() as f64 / 4.0,
            "key {key} cut at {} expected near {expected}",
            cut(key)
        );
    }
    n.one_step(&before, "Split Notes");
    // Esc puts the notes back.
    let steps = n.steps();
    let now = n.doc().clone();
    let again = n.at(66, 3 * step);
    n.drag_frames(again, to, Modifiers::NONE);
    assert_ne!(*n.doc(), now);
    n.h.key(egui::Key::Escape, Modifiers::NONE);
    n.release(to, Modifiers::NONE);
    assert_eq!(*n.doc(), now, "Esc puts the set back");
    assert_eq!(n.steps(), steps);
    n.let_go(egui::Key::E, Modifiers::NONE);
}

#[test]
fn an_e_drag_inside_a_note_moves_the_split_with_the_pointer() {
    // Spec: note-editing R33. Dragging sideways inside a note moves the
    // split until the button is released.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, 8 * step, 100)]);
    n.hold(egui::Key::E, Modifiers::NONE);
    let before = n.doc().clone();
    let from = n.at(60, step);
    let to = n.at(60, 5 * step);
    let end = n.tick(to);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.notes().len(), 2);
    assert_eq!(n.notes()[1].start, end, "the split followed the pointer");
    n.one_step(&before, "Split Notes");
    n.let_go(egui::Key::E, Modifiers::NONE);
}

#[test]
fn cmd_e_splits_at_the_insert_marker_with_no_notes_selected() {
    // Spec: note-editing R35; S-NOTE-003. One undo step.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, 4 * step, 100)]);
    let at = n.at(61, 2 * step);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.insert(), Some(2 * step));
    let before = n.doc().clone();
    n.h.key(egui::Key::E, command());
    assert_eq!(
        n.starts(),
        [
            (60, 0, 2 * step.get()),
            (60, 2 * step.get(), 2 * step.get())
        ]
    );
    n.one_step(&before, "Split Notes");
}

#[test]
fn cmd_e_chops_selected_notes_and_cmd_with_the_arrows_re_chops_in_one_step() {
    // Spec: note-editing R36; S-NOTE-003, S-NOTE-047. A note four grid
    // steps long chops into four parts; Up makes five, Down four again,
    // Shift-Up eight; letting go of Cmd ends it, and one Undo undoes all.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, 4 * step, 100)]);
    let at = n.at(60, step / 2);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.selected(), [0]);
    let before = n.doc().clone();
    let steps = n.steps();
    n.hold(egui::Key::E, command());
    assert_eq!(n.notes().len(), 4, "chopped on the grid");
    let press = |n: &mut Notes, key: egui::Key, shift: bool| {
        let modifiers = if shift {
            command() | Modifiers::SHIFT
        } else {
            command()
        };
        n.h.frame(vec![
            Event::ModifiersChanged(modifiers),
            Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        ]);
        n.h.frame(vec![
            Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            },
            Event::ModifiersChanged(command()),
        ]);
    };
    press(&mut n, egui::Key::ArrowUp, false);
    assert_eq!(n.notes().len(), 5, "Cmd-Up: one part more");
    press(&mut n, egui::Key::ArrowDown, false);
    assert_eq!(n.notes().len(), 4, "Cmd-Down: one fewer");
    press(&mut n, egui::Key::ArrowUp, true);
    assert_eq!(n.notes().len(), 8, "Cmd-Shift-Up: the next power of two");
    assert_eq!(n.steps(), steps, "still under way: no step yet");
    n.let_go(egui::Key::E, command());
    assert_eq!(n.steps(), steps + 1, "releasing Cmd ends the one step");
    n.one_step(&before, "Chop Notes");
    // Without Cmd held, Up and Down transpose as before.
    n.h.key(egui::Key::ArrowUp, Modifiers::NONE);
    assert_eq!(n.notes().len(), 8);
    assert_eq!(n.notes()[0].key, 61);
}

#[test]
fn the_chop_drags_count_parts_in_the_status_bar() {
    // Spec: note-editing R37; S-NOTE-004, S-NOTE-005. E with Option (Ctrl on
    // Windows) held, dragging up on a selected note: one part more for each
    // 8 points, and with Shift the powers of two; Esc puts the notes back.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, 8 * step, 100),
        Note::new(64, Ticks::ZERO, 8 * step, 100),
    ]);
    n.select_all();
    n.hold(egui::Key::E, chop_key());
    let before = n.doc().clone();
    let from = n.at(60, 4 * step);
    let up = |by: f32| pos2(from.x, from.y - by);
    n.drag_frames(from, up(25.0), chop_key());
    assert_eq!(n.notes().len(), 8, "4 parts of each note after 25 points");
    assert_eq!(n.h.shell.hint(), Some("Chop: 4 parts"));
    n.release(up(25.0), chop_key());
    assert_eq!(n.h.shell.hint(), None);
    n.one_step(&before, "Chop Notes");
    // Shift: powers of two.
    n.drag(from, up(25.0), chop_key() | Modifiers::SHIFT);
    assert_eq!(n.notes().len(), 64, "each of the 8 parts into 8");
    // Esc puts them back.
    let steps = n.steps();
    let now = n.doc().clone();
    n.select_all();
    n.drag_frames(from, up(25.0), chop_key());
    n.h.key(egui::Key::Escape, Modifiers::NONE);
    n.release(up(25.0), chop_key());
    assert_eq!(*n.doc(), now);
    assert_eq!(n.steps(), steps);
    n.let_go(egui::Key::E, chop_key());
}

#[test]
fn cmd_j_joins_cmd_option_j_fits_and_cmd_shift_e_splits_the_clip() {
    // Spec: note-editing R39, R40, R41; S-NOTE-007, S-NOTE-008, S-NOTE-036.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(60, 2 * step, step, 100),
    ]);
    // A click in the editor gives it the keyboard.
    let at = n.at(60, step / 2);
    n.click(at, Modifiers::NONE);
    n.select_all();
    let before = n.doc().clone();
    n.h.key(egui::Key::J, command());
    assert_eq!(n.starts(), [(60, 0, 3 * step.get())]);
    n.one_step(&before, "Join Notes");
    // Fit to Time Range: the selection is the notes' span; widen it.
    n.h.shell
        .run(
            "clip_view.select_time",
            json!({ "clip": n.clip, "start": 0, "end": 8 * step.get() }),
        )
        .unwrap();
    n.h.settle();
    let before = n.doc().clone();
    n.h.key(egui::Key::J, command() | Modifiers::ALT);

    assert_eq!(n.starts(), [(60, 0, 8 * step.get())]);
    n.one_step(&before, "Fit to Time Range");
    // Split Clip.
    n.h.shell
        .run(
            "clip_view.select_time",
            json!({ "clip": n.clip, "start": BAR, "end": 2 * BAR }),
        )
        .unwrap();
    n.h.settle();
    let clips = |n: &Notes| {
        n.doc()
            .clip_ids()
            .filter(|id| {
                n.doc()
                    .clip(*id)
                    .is_some_and(|c| matches!(c.data, ClipData::Midi(_)))
            })
            .count()
    };
    assert_eq!(clips(&n), 1);
    n.h.key(egui::Key::E, command() | Modifiers::SHIFT);
    assert_eq!(clips(&n), 3, "split at both edges of the time selection");
}

#[test]
fn dragging_a_stretch_marker_scales_the_notes_about_the_other_one() {
    // Spec: note-editing R42, R43, R45; L-CLIP-020, I-DRAG-040. Two or more
    // notes selected put a marker at each end of the time selection;
    // dragging one scales the notes about the other, as one undo step.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, step, step, 100),
        Note::new(64, 3 * step, 2 * step, 100),
    ]);
    n.select_all();
    assert_eq!(n.time(), Some((step, 5 * step)), "the markers' places");
    let before = n.doc().clone();
    let from = n.at_band(5 * step);
    let to = n.at_band(9 * step);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(
        n.time(),
        Some((step, 9 * step)),
        "the dragged end moved, snapped to the grid"
    );
    assert_eq!(
        n.starts(),
        [
            (60, step.get(), 2 * step.get()),
            (64, 5 * step.get(), 4 * step.get())
        ],
        "starts and lengths doubled about the first marker"
    );
    n.one_step(&before, "Stretch Notes");
    // Esc puts them back.
    let steps = n.steps();
    let now = n.doc().clone();
    n.select_all();
    let first = n.at_band(step);
    let past = n.at_band(Ticks::ZERO);
    n.drag_frames(first, past, Modifiers::NONE);
    n.h.key(egui::Key::Escape, Modifiers::NONE);
    n.release(past, Modifiers::NONE);
    assert_eq!(*n.doc(), now);
    assert_eq!(n.steps(), steps);
}

#[test]
fn a_stretch_marker_dragged_with_cmd_does_not_snap_and_the_pseudo_marker_keeps_the_ends() {
    // Spec: note-editing R43, R44.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, step, step, 100),
        Note::new(64, 3 * step, 2 * step, 100),
    ]);
    n.select_all();
    // The pseudo marker: press between the markers and drag.
    let before = n.doc().clone();
    let from = n.at_band(3 * step);
    let to = n.at_band(4 * step);
    n.drag(from, to, Modifiers::NONE);
    let notes = n.notes();
    assert_eq!(n.time(), Some((step, 5 * step)), "both markers stay");
    assert_eq!(notes[0].start, step, "the first note keeps its start");
    assert_eq!(
        notes[1].start + notes[1].length,
        5 * step,
        "the last keeps its end"
    );
    assert_eq!(
        notes[1].start,
        4 * step,
        "what lay after the point starts at its new place"
    );
    n.one_step(&before, "Stretch Notes");
    // Cmd bypasses snapping: the marker lands where the pointer is.
    n.h.shell.run("edit.undo", json!({})).unwrap();
    n.select_all();
    let from = n.at_band(5 * step);
    let to = pos2(n.at_band(7 * step).x + 3.0, from.y);
    let wanted = n.tick(to);
    n.drag(from, to, bypass_key());
    let (_, end) = n.time().unwrap();
    assert!((end - wanted).get().abs() <= 2, "{end:?} near {wanted:?}");
    assert_ne!(end.get() % step.get(), 0);
}
