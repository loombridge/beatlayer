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
    KEY_HEIGHT, KEY_WIDTH, note_rect, shown_keys, strip_rect, tick_at, x_of_tick, y_of_key,
    y_of_value,
};
use bl_ui::note_filters::{BAR_HEIGHT, bar_rect, body_rect, magnifier_rect};
use bl_ui::note_lanes::{NoteLayout, note_layout};
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
        // The editor sits beside the panel groups (L-CLIP-003).
        bl_ui::clip_view::editor_rect(Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        ))
    }

    /// Where the note area, the lanes and the footer sit, as the editor
    /// lays them out for the lanes shown now.
    fn layout(&mut self) -> NoteLayout {
        let view = self.view();
        let shown = self.h.shell.clip_view().lanes().shown();
        let bar = self.h.shell.clip_view().note_filters().visible;
        note_layout(
            body_rect(view, bar),
            &self.h.main.clip_view().lane_heights(&shown),
        )
    }

    /// The note area's rectangle.
    fn grid(&mut self) -> Rect {
        self.layout().grid
    }

    /// The Velocity lane's rectangle under the note area.
    fn lane(&mut self) -> Rect {
        self.lane_of(NoteField::Velocity)
    }

    /// The rectangle of a lane that shows.
    fn lane_of(&mut self, field: NoteField) -> Rect {
        self.layout().lane(field).expect("the lane shows")
    }

    /// A point in the lane at `tick`'s x — a note's stem when the tick is
    /// its start — and the height of `value` of `field`.
    fn lane_at(&mut self, tick: Ticks, field: NoteField, value: f64) -> Pos2 {
        let grid = self.grid();
        let lane = self.lane_of(field);
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
        let view = bl_ui::clip_view::editor_rect(Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        ));
        let bar = self.h.shell.clip_view().note_filters().visible;
        strip_rect(body_rect(view, bar))
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
    // Spec: note-editing R5 (replacing piano-roll R14, R15). A drag on a note's stem in the lane sets that
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
    n.one_step(&before, "Adjust note values");
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
    n.one_step(&before, "Adjust note values");
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
    // are drawn disabled and name their packages. Group Notes needs two
    // notes and Ungroup Notes a grouped one.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let at = n.at(60, step / 2);
    let nodes = right_click(&mut n, at);
    // The note the menu opened on is selected alone.
    assert_eq!(n.selected(), [0]);
    for label in [
        "Cut",
        "Copy",
        "Paste",
        "Duplicate",
        "Delete",
        "Invert Selection",
        "Deactivate Note(s)",
    ] {
        let node = menu_item(&nodes, label);
        assert!(!node.disabled, "{label} works");
    }
    for label in ["Group Notes", "Ungroup Notes"] {
        assert!(menu_item(&nodes, label).disabled, "{label} has no use here");
    }
    let node = menu_item(&nodes, "Highlight Scales");
    assert!(node.disabled, "Highlight Scales waits");
    assert!(
        node.description.contains("WP-6.7b"),
        "Highlight Scales names WP-6.7b: {:?}",
        node.description
    );
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
    // C-KEYS: Deactivate Note(s), which works, and the note spelling, which
    // waits for its package, drawn disabled and naming it.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let at = n.key_at(60);
    let nodes = right_click(&mut n, at);
    assert!(!menu_item(&nodes, "Deactivate Note(s)").disabled);
    let node = menu_item(&nodes, "Note spelling");
    assert!(node.disabled, "Note spelling waits");
    assert!(
        node.description.contains("WP-6.7b"),
        "Note spelling names WP-6.7b: {:?}",
        node.description
    );
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

// -- The expression lanes and the footer (WP-6.1) ---------------------------

use bl_shell::Area;
use bl_ui::note_lanes::{footer_rects, shows_corner};
use egui::Key;

impl Notes {
    /// The rectangle of a footer control for the active lane.
    fn footer(&mut self, name: &str) -> Rect {
        let layout = self.layout();
        let lane = self.h.shell.clip_view().lane();
        footer_rects(layout.footer, lane)
            .into_iter()
            .find(|(named, _)| *named == name)
            .unwrap_or_else(|| panic!("no footer control {name:?}"))
            .1
    }

    /// The lanes shown, from the top.
    fn shown(&self) -> Vec<NoteField> {
        self.h.shell.clip_view().lanes().shown()
    }

    /// Gives the note editor the keyboard.
    fn focus(&mut self) {
        self.h.shell.set_focused_area(Area::Detail);
        self.h.settle();
    }

    fn select(&mut self, places: &[u32]) {
        self.h
            .shell
            .run(
                "clip_view.select_notes",
                json!({ "clip": self.clip, "notes": places }),
            )
            .unwrap();
        self.h.settle();
    }

    /// A double-click: two presses and releases in quick succession.
    fn double_click(&mut self, at: Pos2, modifiers: Modifiers) {
        self.h.frame(vec![Event::PointerMoved(at)]);
        for _ in 0..2 {
            for pressed in [true, false] {
                self.h.frame(vec![Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers,
                }]);
            }
        }
        self.h.settle();
    }

    /// Clicks the middle of a node found by its label and role.
    fn click_node(&mut self, nodes: &[Node], role: egui::accesskit::Role, label: &str) {
        let node = nodes
            .iter()
            .find(|node| node.role == role && node.label == label)
            .unwrap_or_else(|| panic!("no {role:?} {label:?}"));
        let b = node.bounds.expect("bounds");
        let at = pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
        self.h.click(at);
        self.h.settle();
    }

    fn velocities(&self) -> Vec<u8> {
        self.notes().iter().map(|note| note.velocity).collect()
    }
}

/// A set of `count` notes one grid step apart, all at velocity `velocity`.
fn run_of_notes(count: i64, velocity: u8) -> Notes {
    let step = n_step();
    set((0..count)
        .map(|i| Note::new(60 + i as u8, i * step, step, velocity))
        .collect())
}

#[test]
fn the_lane_selector_the_switch_and_swap_lane_show_hide_and_swap_lanes() {
    // Spec: note-editing R1, R2, R3. A new editor shows the Velocity lane
    // alone; the Lane Selector shows and hides each lane; the switch hides
    // them all and shows them as they were; Swap Lane trades a lane for the
    // next hidden one.
    use NoteField::{Chance, ReleaseVelocity, Velocity};
    let mut n = run_of_notes(2, 100);
    let steps = n.steps();
    assert_eq!(n.shown(), [Velocity]);
    // The Lane Selector lists the three lanes; choosing Chance shows it and
    // makes it the active lane.
    let selector = n.footer("Lane Selector").center();
    n.h.click(selector);
    let nodes = Harness::nodes(&n.h.settle());
    n.click_node(&nodes, egui::accesskit::Role::CheckBox, "Chance");
    assert_eq!(n.shown(), [Velocity, Chance]);
    assert_eq!(n.h.shell.clip_view().lane(), Chance, "the active lane");
    // The footer follows the active lane.
    let randomize = n.footer("Randomize");
    assert!(randomize.is_positive());
    assert!(n.layout().lane(Chance).is_some());
    // Swap Lane, in the lane header's menu, hides the Velocity lane and shows
    // the first hidden lane after it.
    let velocity = n.lane_of(Velocity);
    let header = pos2(velocity.left() + 4.0, velocity.center().y);
    let nodes = right_click(&mut n, header);
    let swap = menu_item(&nodes, "Swap Lane");
    assert!(!swap.disabled);
    let b = swap.bounds.unwrap();
    n.h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    n.h.settle();
    assert_eq!(n.shown(), [Chance, ReleaseVelocity]);
    // The switch hides every lane, and shows them again.
    let switch = n.footer("Show/Hide All Expression Editors").center();
    n.h.click(switch);
    n.h.settle();
    assert!(n.shown().is_empty());
    assert!(n.h.shell.clip_view().lanes().all_hidden());
    let grid_without_lanes = n.grid();
    n.h.click(switch);
    n.h.settle();
    assert_eq!(n.shown(), [Chance, ReleaseVelocity], "as they were");
    assert!(
        n.grid().bottom() < grid_without_lanes.bottom(),
        "the lanes take the note area's room"
    );
    assert_eq!(n.steps(), steps, "none of it is an undo step");
    assert!(!n.h.shell.is_dirty(), "and none of it dirties the set");
}

#[test]
fn dragging_a_marker_moves_every_selected_note_by_the_same_amount() {
    // Spec: note-editing R5. Dragging a selected note's marker changes every
    // selected note by the same amount, each held within its range; one undo
    // step, and Esc puts the values back.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(62, 4 * step, step, 60),
        Note::new(64, 8 * step, step, 40),
    ]);
    n.select(&[0, 1]);
    let before = n.doc().clone();
    let from = n.lane_at(4 * step, NoteField::Velocity, 60.0);
    let to = n.lane_at(4 * step, NoteField::Velocity, 30.0);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.velocities(), [70, 30, 40], "both by -30, the third alone");
    n.one_step(&before, "Adjust note values");
    assert_eq!(n.selected(), [0, 1], "the selection holds");
    // Dragging past an end brings them all to it.
    let before = n.doc().clone();
    let lane = n.lane();
    let from = n.lane_at(4 * step, NoteField::Velocity, 30.0);
    let to = pos2(from.x, lane.bottom() + 40.0);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.velocities(), [1, 1, 40], "both stop at the end");
    n.one_step(&before, "Adjust note values");
    // Esc puts the values back and records nothing.
    let before = n.doc().clone();
    let steps = n.steps();
    let from = n.lane_at(8 * step, NoteField::Velocity, 40.0);
    let to = n.lane_at(8 * step, NoteField::Velocity, 120.0);
    n.drag_frames(from, to, Modifiers::NONE);
    assert_eq!(n.velocities()[2], 120, "the drag shows as it goes");
    n.h.key(Key::Escape, Modifiers::NONE);
    n.release(to, Modifiers::NONE);
    assert_eq!(*n.doc(), before);
    assert_eq!(n.steps(), steps);
    // A marker that isn't selected moves alone.
    let from = n.lane_at(8 * step, NoteField::Velocity, 40.0);
    let to = n.lane_at(8 * step, NoteField::Velocity, 70.0);
    n.drag(from, to, Modifiers::NONE);
    assert_eq!(n.velocities(), [1, 1, 70]);
}

#[test]
fn clicking_a_marker_selects_its_note_and_shift_adds_and_removes() {
    // Spec: note-editing R5, R3. A click on a marker selects its note and
    // makes its lane the active lane.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(62, 4 * step, step, 60),
    ]);
    let at = n.lane_at(4 * step, NoteField::Velocity, 60.0);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.selected(), [1]);
    let other = n.lane_at(Ticks::ZERO, NoteField::Velocity, 100.0);
    n.click(other, Modifiers::SHIFT);
    assert_eq!(n.selected(), [1, 0], "Shift adds");
    n.click(other, Modifiers::SHIFT);
    assert_eq!(n.selected(), [1], "and removes");
    // A press in the Chance lane makes it the active lane.
    n.h.shell
        .run("view.lanes", json!({ "lane": "chance" }))
        .unwrap();
    n.h.shell
        .run("view.lane", json!({ "lane": "velocity" }))
        .unwrap();
    let chance = n.lane_at(4 * step, NoteField::Chance, 1.0);
    n.click(chance, Modifiers::NONE);
    assert_eq!(n.h.shell.clip_view().lane(), NoteField::Chance);
}

#[test]
fn drawing_in_a_lane_with_draw_mode_sets_values_per_grid_step_or_per_note() {
    // Spec: note-editing R6. With Draw Mode on, dragging across a lane gives
    // each note whose start lies in a step the pointer passes the value at
    // the pointer's height there; without the grid each note takes the
    // height at its own start; only the selected notes when some are.
    let step = n_step();
    let mut n = run_of_notes(8, 100);
    n.h.shell.run("view.draw_mode", json!({})).unwrap();
    n.h.settle();
    let before = n.doc().clone();
    // A descending line across the first four notes, on the grid.
    let from = n.lane_at(Ticks::ZERO, NoteField::Velocity, 120.0);
    let to = n.lane_at(3 * step, NoteField::Velocity, 20.0);
    n.drag(from, to, Modifiers::NONE);
    let drawn = n.velocities();
    assert!(
        drawn[..4].windows(2).all(|pair| pair[0] > pair[1]),
        "the values follow the pointer down: {drawn:?}"
    );
    assert!(
        drawn[4..].iter().all(|v| *v == 100),
        "the rest are untouched"
    );
    assert!(drawn[0] > 90 && drawn[3] < 40, "{drawn:?}");
    n.one_step(&before, "Set note values");
    // Without the grid (Cmd held, Alt on Windows) each note takes the height
    // at its own start: the first note's, at the press, is the pressed
    // height.
    let from = n.lane_at(Ticks::ZERO, NoteField::Velocity, 120.0);
    let to = n.lane_at(3 * step, NoteField::Velocity, 20.0);
    n.drag(from, to, bypass_key());
    let off_grid = n.velocities();
    assert!(
        (i32::from(off_grid[0]) - 120).abs() <= 3 && (i32::from(off_grid[3]) - 20).abs() <= 3,
        "{off_grid:?}"
    );
    // With notes selected, only those are drawn on.
    let mut n = run_of_notes(8, 100);
    n.h.shell.run("view.draw_mode", json!({})).unwrap();
    n.h.settle();
    n.select(&[1]);
    let from = n.lane_at(Ticks::ZERO, NoteField::Velocity, 50.0);
    let to = n.lane_at(3 * step, NoteField::Velocity, 50.0);
    n.drag(from, to, Modifiers::NONE);
    let only = n.velocities();
    assert_eq!(only[0], 100);
    assert!((i32::from(only[1]) - 50).abs() <= 3, "{only:?}");
    assert!(only[2..].iter().all(|v| *v == 100), "{only:?}");
}

#[test]
fn option_draws_a_straight_line_and_with_shift_a_level_one() {
    // Spec: note-editing R6. Option-dragging draws a straight line from
    // where the drag began to the pointer, and with Shift too a level line
    // at the starting value.
    let step = n_step();
    let mut n = run_of_notes(8, 100);
    n.h.shell.run("view.draw_mode", json!({})).unwrap();
    n.h.settle();
    let line = if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        // Alt is also the key that bypasses the grid on Windows.
        Modifiers::ALT
    };
    let start = n.lane_at(Ticks::ZERO, NoteField::Velocity, 20.0);
    let end = n.lane_at(7 * step, NoteField::Velocity, 100.0);
    let wobble = n.lane_at(3 * step, NoteField::Velocity, 127.0);
    // A drag that wanders: the line ignores where it went.
    n.drag_frames(start, wobble, line);
    n.h.frame(vec![Event::PointerMoved(end)]);
    n.release(end, line);
    let drawn = n.velocities();
    assert!(
        drawn.windows(2).all(|pair| pair[0] <= pair[1] + 1),
        "a straight line up: {drawn:?}"
    );
    assert!((i32::from(drawn[0]) - 20).abs() <= 6, "{drawn:?}");
    assert!((i32::from(drawn[7]) - 100).abs() <= 6, "{drawn:?}");
    // With Shift too the line is level at the starting value.
    let level = line | Modifiers::SHIFT;
    n.drag(start, end, level);
    let flat = n.velocities();
    assert!(
        flat.iter().all(|v| (i32::from(*v) - 20).abs() <= 6),
        "{flat:?}"
    );
}

#[test]
fn cmd_dragging_a_velocity_marker_sets_its_deviation_and_a_double_click_resets_it() {
    // Spec: note-editing R7. Cmd-dragging up or down from a marker sets its
    // note's deviation, or every selected note's, within the room the
    // velocity leaves; a double-click on a marker with a deviation resets it.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 60),
        Note::new(62, 4 * step, step, 100),
    ]);
    let cmd = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    };
    let before = n.doc().clone();
    let from = n.lane_at(Ticks::ZERO, NoteField::Velocity, 60.0);
    let to = n.lane_at(Ticks::ZERO, NoteField::Velocity, 100.0);
    n.drag(from, to, cmd);
    assert_eq!(
        n.notes()[0].velocity_deviation,
        40,
        "up to where the pointer is"
    );
    assert_eq!(n.notes()[0].velocity, 60, "the velocity stays");
    assert_eq!(n.notes()[1].velocity_deviation, 0);
    n.one_step(&before, "Set note values");
    // Past the room a velocity of 100 leaves: held to +27.
    n.select(&[1]);
    let from = n.lane_at(4 * step, NoteField::Velocity, 100.0);
    let to = n.lane_at(4 * step, NoteField::Velocity, 127.0);
    n.drag(from, to, cmd);
    assert_eq!(n.notes()[1].velocity_deviation, 27);
    // Double-clicking a marker with a range resets it.
    let before = n.doc().clone();
    let marker = n.lane_at(Ticks::ZERO, NoteField::Velocity, 60.0);
    n.double_click(marker, Modifiers::NONE);
    assert_eq!(n.notes()[0].velocity_deviation, 0);
    n.one_step(&before, "Set note values");
}

#[test]
fn a_typed_velocity_sets_the_selected_notes_and_zero_is_a_digit_only_while_typing() {
    // Spec: note-editing R14, R18. A digit from 1 to 9 starts a velocity,
    // Enter sets it; a number outside 1 to 127 changes nothing; Esc drops it;
    // 0 deactivates the notes unless a number is being typed.
    let mut n = run_of_notes(3, 100);
    n.focus();
    n.select(&[0, 1]);
    let before = n.doc().clone();
    n.h.key(Key::Num5, Modifiers::NONE);
    assert!(n.h.main.clip_view().is_typing());
    n.h.key(Key::Num0, Modifiers::NONE);
    assert!(
        n.notes().iter().all(|note| !note.mute),
        "0 is a digit while a number is being typed"
    );
    n.h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(n.velocities(), [50, 50, 100]);
    assert!(!n.h.main.clip_view().is_typing());
    n.one_step(&before, "Set note values");
    // Out of range changes nothing.
    let before = n.doc().clone();
    for key in [Key::Num2, Key::Num0, Key::Num0, Key::Enter] {
        n.h.key(key, Modifiers::NONE);
    }
    assert_eq!(*n.doc(), before, "200 is refused");
    // Esc drops it and keeps the selection.
    n.h.key(Key::Num7, Modifiers::NONE);
    assert!(n.h.main.clip_view().is_typing());
    n.h.key(Key::Escape, Modifiers::NONE);
    assert!(!n.h.main.clip_view().is_typing());
    assert_eq!(n.selected(), [0, 1], "the notes stay selected");
    assert_eq!(*n.doc(), before);
    // 0 on its own deactivates the selected notes, and again activates them.
    n.h.key(Key::Num0, Modifiers::NONE);
    assert!(n.notes()[0].mute && n.notes()[1].mute && !n.notes()[2].mute);
    n.h.key(Key::Num0, Modifiers::NONE);
    assert!(n.notes().iter().all(|note| !note.mute));
}

#[test]
fn the_value_keys_change_the_selected_notes() {
    // Spec: note-editing R15, R16. Cmd with Up and Down changes the
    // velocity by 10, Cmd-Option the chance by 10 %, and Cmd-Shift the
    // deviation by 10; after a marker is clicked its lane has the keys.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 100),
    ]);
    n.focus();
    n.select(&[0]);
    let before = n.doc().clone();
    n.h.key(Key::ArrowDown, command());
    assert_eq!(n.velocities(), [90, 100]);
    n.one_step(&before, "Adjust note values");
    n.h.key(Key::ArrowUp, command());
    assert_eq!(n.velocities(), [100, 100]);
    n.h.key(Key::ArrowUp, command());
    assert_eq!(n.velocities(), [110, 100]);
    // Chance, by 10 %.
    n.h.key(Key::ArrowDown, command() | Modifiers::ALT);
    assert!((n.notes()[0].probability - 0.9).abs() < 1e-9);
    if cfg!(target_os = "macos") {
        // Deviation, within the room: velocity 110 leaves +17.
        n.h.key(Key::ArrowUp, command() | Modifiers::SHIFT);
        assert_eq!(n.notes()[0].velocity_deviation, 10);
    }
    // After a marker in the Chance lane is clicked, plain Up and Down change
    // the chance by 10 %, and Shift by 1 %, until the note area is clicked.
    n.h.shell
        .run("view.lanes", json!({ "lane": "chance" }))
        .unwrap();
    n.h.settle();
    let marker = n.lane_at(4 * step, NoteField::Chance, 1.0);
    n.click(marker, Modifiers::NONE);
    assert_eq!(n.selected(), [1]);
    n.h.key(Key::ArrowDown, Modifiers::NONE);
    assert!((n.notes()[1].probability - 0.9).abs() < 1e-9);
    n.h.key(Key::ArrowDown, Modifiers::SHIFT);
    assert!((n.notes()[1].probability - 0.89).abs() < 1e-9);
    assert_eq!(n.notes()[1].key, 64, "the notes aren't transposed");
    // In the Velocity lane Cmd-Shift steps the velocity by 1.
    n.h.shell
        .run("view.lane", json!({ "lane": "velocity" }))
        .unwrap();
    n.h.settle();
    let marker = n.lane_at(4 * step, NoteField::Velocity, 100.0);
    n.click(marker, Modifiers::NONE);
    n.h.key(Key::ArrowDown, command() | Modifiers::SHIFT);
    assert_eq!(n.velocities()[1], 99);
    // Clicking the note area gives the keys back: Up transposes.
    let at = n.at(64, step / 2 + 4 * step);
    n.click(at, Modifiers::NONE);
    n.h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(n.notes()[1].key, 65, "a semitone up again");
}

#[test]
fn cmd_dragging_a_note_changes_its_velocity_and_along_the_grid_moves_it() {
    // Spec: note-editing R17, piano-roll R10. Cmd-dragging a note up or down
    // raises or lowers its velocity by one for each point; along the note
    // area it still moves off the grid.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 8 * step, step, 50),
    ]);
    let before = n.doc().clone();
    let from = n.at(64, 8 * step + step / 2);
    n.drag(from, from + vec2(0.0, -20.0), bypass_key());
    assert_eq!(n.velocities(), [100, 70], "20 points up");
    assert_eq!(n.notes()[1].key, 64, "the pitch holds");
    assert_eq!(n.notes()[1].start, 8 * step, "and so does the start");
    n.one_step(&before, "Adjust note values");
    let to = from + vec2(0.0, 10.0);
    n.drag(from, to, bypass_key());
    assert_eq!(n.velocities()[1], 60, "down lowers it");
}

#[test]
fn deactivating_notes_from_the_menus_and_the_piano_ruler() {
    // Spec: note-editing R18, R19. The editor's and the piano ruler's menus
    // deactivate the selected notes, and read Activate Note(s) when they are
    // all off; right-clicking a key selects its notes first.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, 4 * step, step, 100),
    ]);
    let key = n.key_at(64);
    let nodes = right_click(&mut n, key);
    assert_eq!(n.selected(), [1], "the key's notes are selected first");
    let item = menu_item(&nodes, "Deactivate Note(s)");
    assert!(!item.disabled);
    let before = n.doc().clone();
    n.click_node(&nodes, egui::accesskit::Role::Button, &item.label);
    assert!(!n.notes()[0].mute && n.notes()[1].mute);
    n.one_step(&before, "Deactivate notes");
    // The editor's menu now reads Activate Note(s) for that note.
    let at = n.at(64, 4 * step + step / 2);
    let nodes = right_click(&mut n, at);
    let item = menu_item(&nodes, "Activate Note(s)");
    assert!(!item.disabled);
    n.click_node(&nodes, egui::accesskit::Role::Button, &item.label);
    assert!(n.notes().iter().all(|note| !note.mute));
}

#[test]
fn the_footer_has_the_active_lanes_controls() {
    // Spec: note-editing R10 to R13. Velocity: Randomize with its amount,
    // Ramp's start and end, Deviation; Chance: Randomize with its amount, and
    // Play All, Play One and Ungroup, which wait for notes to group.
    let mut n = run_of_notes(3, 100);
    let nodes = Harness::nodes(&n.h.settle());
    for label in [
        "Randomize",
        "Randomize Amount",
        "Ramp Start",
        "Ramp End",
        "Velocity Deviation",
    ] {
        assert!(nodes.iter().any(|node| node.label == label), "{label}");
    }
    n.h.shell
        .run("view.lane", json!({ "lane": "chance" }))
        .unwrap();
    let nodes = Harness::nodes(&n.h.settle());
    for label in ["Play All", "Play One", "Ungroup"] {
        let node = nodes
            .iter()
            .find(|node| node.label == label)
            .unwrap_or_else(|| panic!("{label}"));
        assert!(node.disabled, "{label} waits for notes to group");
    }
    assert!(!nodes.iter().any(|node| node.label == "Ramp Start"));
}

#[test]
fn the_footers_randomize_button_and_amount_randomise_in_one_step() {
    // Spec: note-editing R13. The Randomize button applies the amount once
    // to the selected notes, or to every note when none are; moving the
    // amount with notes selected randomises them at once from where they
    // were; each is one undo step, and with no notes selected moving the
    // amount changes nothing.
    let mut n = run_of_notes(8, 100);
    n.h.shell
        .run("view.lanes", json!({ "lane": "velocity", "amount": 30 }))
        .unwrap();
    n.h.settle();
    let before = n.doc().clone();
    let button = n.footer("Randomize").center();
    n.h.click(button);
    n.h.settle();
    let first = n.velocities();
    assert!(first.iter().all(|v| (70..=127).contains(v)), "{first:?}");
    assert!(first.iter().any(|v| *v != 100), "{first:?}");
    n.one_step(&before, "Randomize note values");
    // With notes selected, dragging the amount up randomises them, and Esc
    // puts it back.
    n.select(&[0, 1, 2]);
    let amount = n.footer("Randomize Amount");
    let from = amount.center();
    let before = n.doc().clone();
    let was = n.velocities();
    n.drag_frames(from, from + vec2(0.0, -30.0), Modifiers::NONE);
    let during = n.velocities();
    assert!(during[..3] != was[..3], "{during:?}");
    assert!(during[3..] == was[3..], "the others hold: {during:?}");
    n.h.key(Key::Escape, Modifiers::NONE);
    n.release(from + vec2(0.0, -30.0), Modifiers::NONE);
    assert_eq!(*n.doc(), before, "Esc puts the values back");
    // The drag, finished, is one step and the amount stays.
    n.drag(from, from + vec2(0.0, -30.0), Modifiers::NONE);
    n.one_step(&before, "Randomize note values");
    assert_eq!(n.h.shell.clip_view().randomize_amounts().velocity, 40);
    // With nothing selected, moving the amount changes nothing.
    n.select(&[]);
    let before = n.doc().clone();
    n.drag(from, from + vec2(0.0, -30.0), Modifiers::NONE);
    assert_eq!(*n.doc(), before);
    assert_eq!(n.h.shell.clip_view().randomize_amounts().velocity, 50);
}

#[test]
fn typing_into_the_footers_fields_sets_the_ramp_the_deviation_and_the_amount() {
    // Spec: note-editing R10 to R13. A value field shows the selected notes'
    // value and takes a typed value on Enter.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(62, 4 * step, step, 100),
        Note::new(64, 8 * step, step, 100),
    ]);
    n.select(&[0, 1, 2]);
    let type_into = |n: &mut Notes, name: &str, keys: &[Key]| {
        let at = n.footer(name).center();
        n.h.click(at);
        n.h.settle();
        for key in keys {
            n.h.key(*key, Modifiers::NONE);
        }
        n.h.key(Key::Enter, Modifiers::NONE);
    };
    // Ramp Start, then Ramp End: the notes between are on the line.
    let before = n.doc().clone();
    type_into(&mut n, "Ramp Start", &[Key::Num2, Key::Num0]);
    assert_eq!(
        n.velocities(),
        [20, 60, 100],
        "from 20 to the last note's 100"
    );
    n.one_step(&before, "Ramp note values");
    type_into(&mut n, "Ramp End", &[Key::Num1, Key::Num2, Key::Num0]);
    assert_eq!(n.velocities(), [20, 70, 120]);
    // Deviation takes a negative number.
    type_into(
        &mut n,
        "Velocity Deviation",
        &[Key::Minus, Key::Num1, Key::Num0],
    );
    assert!(n.notes().iter().all(|note| note.velocity_deviation == -10));
    // An out-of-range value changes nothing.
    let before = n.doc().clone();
    type_into(&mut n, "Ramp End", &[Key::Num2, Key::Num0, Key::Num0]);
    assert_eq!(*n.doc(), before);
    // The amount, typed, sets it and randomises the selected notes.
    type_into(&mut n, "Randomize Amount", &[Key::Num1, Key::Num5]);
    assert_eq!(n.h.shell.clip_view().randomize_amounts().velocity, 15);
    let velocities = n.velocities();
    assert!(
        velocities
            .iter()
            .zip([20u8, 70, 120])
            .all(|(now, was)| now.abs_diff(was) <= 15),
        "{velocities:?}"
    );
}

#[test]
fn the_ramps_ends_follow_the_selection() {
    // Spec: note-editing R11. With one note selected only the start is
    // available, and with every selected note starting together the end is
    // disabled.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(64, Ticks::ZERO, step, 100),
        Note::new(62, 4 * step, step, 100),
    ]);
    let disabled = |n: &mut Notes, label: &str| -> bool {
        let nodes = Harness::nodes(&n.h.settle());
        nodes
            .iter()
            .find(|node| node.label == label)
            .unwrap_or_else(|| panic!("{label}"))
            .disabled
    };
    assert!(disabled(&mut n, "Ramp Start"), "nothing selected");
    n.select(&[0]);
    assert!(!disabled(&mut n, "Ramp Start"));
    assert!(disabled(&mut n, "Ramp End"), "one note");
    n.select(&[0, 1]);
    assert!(disabled(&mut n, "Ramp End"), "a chord starts together");
    n.select(&[0, 2]);
    assert!(!disabled(&mut n, "Ramp End"));
}

#[test]
fn a_note_that_doesnt_always_play_shows_a_corner_triangle_on_a_tall_enough_row() {
    // Spec: note-editing R4. Notes below 100 % show the triangle while their
    // key row is at least 9 points tall.
    let mut note = Note::new(60, Ticks::ZERO, Ticks(10), 100);
    assert!(!shows_corner(&note, 12.0), "100 %");
    note.probability = 0.5;
    assert!(shows_corner(&note, 9.0));
    assert!(!shows_corner(&note, 8.0), "too short a row");
}

#[test]
fn the_lanes_dividers_wheel_and_a_lane_dragged_to_nothing_resize_and_hide() {
    // Spec: note-editing R2. Dragging the split resizes the lanes, Option
    // with the wheel makes them taller or shorter, and a lane dragged down
    // to nothing hides.
    use NoteField::{Chance, Velocity};
    let mut n = run_of_notes(2, 100);
    n.h.shell
        .run("view.lanes", json!({ "lane": "chance" }))
        .unwrap();
    n.h.settle();
    let tall = n.lane_of(Velocity).height();
    // Dragging the divider above the Chance lane up makes it taller and the
    // Velocity lane shorter.
    let divider = n.lane_of(Chance).left_top() + vec2(300.0, -1.5);
    n.drag(divider, divider + vec2(0.0, -10.0), Modifiers::NONE);
    assert!(n.lane_of(Velocity).height() < tall);
    assert!(n.lane_of(Chance).height() > tall);
    // Option with the wheel makes every lane taller.
    let lane = n.lane_of(Velocity);
    let at = pos2(lane.center().x, lane.center().y);
    let before = n.lane_of(Velocity).height();
    n.h.frame(vec![Event::PointerMoved(at)]);
    n.h.frame(vec![
        Event::ModifiersChanged(Modifiers::ALT),
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, 12.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::ALT,
        },
    ]);
    n.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    n.h.settle();
    assert!(n.lane_of(Velocity).height() > before);
    // Dragged to nothing, the Chance lane hides.
    let chance = n.lane_of(Chance);
    let top = chance.left_top() + vec2(300.0, -1.5);
    n.drag(
        top,
        top + vec2(0.0, chance.height() + 20.0),
        Modifiers::NONE,
    );
    assert_eq!(n.shown(), [Velocity], "the lane hides");
}

#[test]
fn assistive_technology_reads_each_lane_marker_and_footer_control() {
    // Spec: note-editing R20. Each lane is a group named for its value, each
    // marker an item with its note's name and value, and each footer control
    // its name and value.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 77)]);
    let nodes = Harness::nodes(&n.h.settle());
    assert!(
        nodes
            .iter()
            .any(|node| node.role == egui::accesskit::Role::Group && node.label == "Velocity"),
        "the lane is a group named Velocity"
    );
    let marker = nodes
        .iter()
        .find(|node| node.role == egui::accesskit::Role::ListItem && node.value == "77")
        .expect("the marker reads its value");
    assert_eq!(marker.label, "C3", "and its note's name");
    let amount = nodes
        .iter()
        .find(|node| node.label == "Randomize Amount")
        .expect("the footer's amount");
    assert_eq!(amount.value, "0");
    assert_eq!(amount.numeric, Some(0.0));
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

/// Switches Preview on, arms the clip's track and turns the computer MIDI
/// keyboard on, as step recording needs, and lets a frame pass so the
/// input's copy is on.
fn start_step_recording(n: &mut Notes) {
    let track = n.doc().clip_track(n.clip).unwrap();
    n.h.shell
        .run("mixer.set_arm", json!({ "track": track, "arm": true }))
        .unwrap();
    n.h.shell
        .run("clip_view.set_preview", json!({ "on": true }))
        .unwrap();
    n.h.shell
        .run("midi.toggle_computer_keyboard", json!({ "on": true }))
        .unwrap();
    n.h.settle();
    n.h.shell.poll();
}

#[test]
fn the_preview_switch_above_the_piano_ruler_runs_set_preview() {
    // Spec: step-recording R1. L-CLIP-012: the switch sits above the piano
    // ruler, in the marker strip's left end.
    let mut n = set(vec![]);
    assert!(!n.h.shell.clip_view().preview(), "off in a new install");
    let strip = n.strip();
    let switch = pos2(strip.left() + KEY_WIDTH / 2.0, strip.center().y);
    n.click(switch, Modifiers::NONE);
    assert!(n.h.shell.clip_view().preview());
    assert_eq!(n.steps(), 0, "no undo step");
    n.click(switch, Modifiers::NONE);
    assert!(!n.h.shell.clip_view().preview());
}

#[test]
fn a_key_held_in_the_piano_ruler_plays_until_released_with_preview_on() {
    // Spec: step-recording R2. A click on a key plays it at velocity 100
    // while the button is held, and only with Preview on.
    let mut n = set(vec![]);
    let track = n.doc().clip_track(n.clip).unwrap();
    let mut queue = n.h.shell.midi().sender().attach(64);
    let heard = |queue: &mut rtrb::Consumer<bl_core::midi::MidiInputEvent>| {
        std::iter::from_fn(|| queue.pop().ok())
            .map(|e| {
                (
                    e.source,
                    e.message.status(),
                    e.message.data1(),
                    e.message.data2(),
                )
            })
            .collect::<Vec<_>>()
    };
    let source = bl_core::midi::MidiSource::Preview(track.0);
    let key = n.key_at(57);
    let press = |n: &mut Notes, pressed: bool| {
        n.h.frame(vec![Event::PointerButton {
            pos: key,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
        n.h.settle();
    };
    n.h.frame(vec![Event::PointerMoved(key)]);
    press(&mut n, true);
    press(&mut n, false);
    assert_eq!(heard(&mut queue), [], "Preview is off");

    n.h.shell
        .run("clip_view.set_preview", json!({ "on": true }))
        .unwrap();
    n.h.settle();
    press(&mut n, true);
    assert_eq!(heard(&mut queue), [(source, 0x90, 57, 100)]);
    press(&mut n, false);
    assert_eq!(heard(&mut queue), [(source, 0x80, 57, 64)]);
    assert_eq!(n.steps(), 0, "a preview changes nothing in the set");
}

#[test]
fn a_dragged_note_plays_each_pitch_it_reaches_and_ends_on_release() {
    // Spec: step-recording R2. A drag that moves notes to a new pitch plays
    // the grabbed note at each new pitch, ending the one before.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 90)]);
    let track = n.doc().clip_track(n.clip).unwrap();
    n.h.shell
        .run("clip_view.set_preview", json!({ "on": true }))
        .unwrap();
    n.h.settle();
    let mut queue = n.h.shell.midi().sender().attach(256);
    let from = n.at(60, step / 2);
    let to = n.at(63, step / 2);
    n.drag(from, to, Modifiers::NONE);
    let heard: Vec<_> = std::iter::from_fn(|| queue.pop().ok())
        .map(|e| {
            assert_eq!(e.source, bl_core::midi::MidiSource::Preview(track.0));
            (e.message.is_note_on(), e.message.data1())
        })
        .collect();
    assert_eq!(heard.first(), Some(&(true, 60)), "the grabbed pitch first");
    assert!(heard.contains(&(true, 63)), "and the pitch it reached");
    assert_eq!(heard.last(), Some(&(false, 63)), "ended on release");
    let on = heard.iter().filter(|(on, _)| *on).count();
    let off = heard.len() - on;
    assert_eq!(on, off, "every note-on has its note-off");
    assert_eq!(n.notes()[0].key, 63);
}

#[test]
fn left_and_right_step_while_step_recording_and_move_the_marker_otherwise() {
    // Spec: step-recording R3, R4, R6, R10. Right with a key held adds the
    // note at the insert marker and steps on; Left takes it off again.
    let step = n_step();
    let mut n = set(vec![]);
    let at = n.at(60, step / 4);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.insert(), Some(Ticks::ZERO));
    // Not step recording yet: the arrows only move the marker.
    n.h.key(egui::Key::ArrowRight, Modifiers::NONE);
    assert_eq!(n.insert(), Some(step));
    n.h.key(egui::Key::ArrowLeft, Modifiers::NONE);

    start_step_recording(&mut n);
    // A on the computer MIDI keyboard is C3.
    n.h.shell
        .run(
            "midi.computer_keyboard_key",
            json!({ "key": "A", "pressed": true }),
        )
        .unwrap();
    n.h.settle();
    n.h.key(egui::Key::ArrowRight, Modifiers::NONE);
    assert_eq!(n.notes().len(), 1);
    assert_eq!(n.notes()[0].start, Ticks::ZERO);
    assert_eq!(n.notes()[0].length, step);
    assert_eq!(n.insert(), Some(step));
    n.h.key(egui::Key::ArrowRight, Modifiers::NONE);
    assert_eq!(n.notes()[0].length, 2 * step);
    n.h.key(egui::Key::ArrowLeft, Modifiers::NONE);
    n.h.key(egui::Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(n.notes(), []);
    assert_eq!(n.insert(), Some(Ticks::ZERO));
    // Shift still selects time, as without step recording.
    n.h.key(egui::Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(n.time(), Some((Ticks::ZERO, step)));
}

#[test]
fn cmd_shift_b_arms_the_track_and_switches_the_computer_keyboard() {
    // Spec: step-recording R9, M-OPT-018. The key runs Step Input Mode in
    // the note editor, with the Clip View open on a MIDI clip.
    let mut n = set(vec![]);
    n.h.shell.set_focused_area(bl_shell::Area::Detail);
    let track = n.doc().clip_track(n.clip).unwrap();
    assert!(!n.h.shell.computer_keyboard_on());
    let chord = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND | Modifiers::SHIFT
    } else {
        Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT
    };
    n.h.key(egui::Key::B, chord);
    assert!(n.doc().track(track).unwrap().mixer().arm);
    assert!(n.h.shell.computer_keyboard_on());
    assert_eq!(n.steps(), 1, "arming is the one undo step");
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
    // A click in the editor gives it the keyboard, so Cmd-E (Ctrl-E) and
    // E reach it rather than the Arrangement's keys.
    let at = n.at(60, step / 2);
    n.click(at, Modifiers::NONE);
    n.select_all();
    let before = n.doc().clone();
    n.hold(egui::Key::E, chop_key());
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

// -- Probability groups (WP-6.2) --------------------------------------------

use bl_model::{GroupKind, NoteGroup};

/// A clip of `count` notes one grid step apart, with the Chance lane shown
/// and active.
fn chance_notes(count: i64) -> Notes {
    let step = n_step();
    let mut n = set((0..count)
        .map(|i| Note::new(60 + i as u8, 4 * i * step, step, 100))
        .collect());
    n.h.shell
        .run("view.lanes", json!({ "lane": "chance" }))
        .unwrap();
    n.h.settle();
    n
}

impl Notes {
    /// The clip's groups.
    fn groups(&self) -> Vec<NoteGroup> {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => midi.groups.clone(),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn grouped(&self) -> Vec<Option<u32>> {
        self.notes().iter().map(|note| note.group).collect()
    }
}

// Spec: note-editing R22, R23, S-NOTE-031, S-NOTE-032. Cmd-G groups the
// selected notes, Cmd-Shift-G ungroups them, each in one undo step.
#[test]
fn cmd_g_groups_the_selected_notes_and_cmd_shift_g_ungroups_them() {
    let mut n = run_of_notes(3, 100);
    n.focus();
    n.select(&[0, 1, 2]);
    let before = n.doc().clone();
    n.h.key(Key::G, command());
    n.h.settle();
    assert_eq!(n.grouped(), [Some(1); 3]);
    assert_eq!(n.groups()[0].kind, GroupKind::PlayAll);
    assert_eq!(n.selected(), [0, 1, 2], "the notes stay selected");
    n.one_step(&before, "Group Notes");
    let before = n.doc().clone();
    n.h.key(Key::G, command() | Modifiers::SHIFT);
    n.h.settle();
    assert_eq!(n.grouped(), [None; 3]);
    n.one_step(&before, "Ungroup Notes");
}

// Spec: note-editing R22, R23, R10. The Chance lane's footer groups the
// selected notes as Play All or Play One, and ungroups them.
#[test]
fn the_footer_buttons_group_and_ungroup() {
    let mut n = chance_notes(4);
    let nodes = Harness::nodes(&n.h.settle());
    for label in ["Play All", "Play One", "Ungroup"] {
        assert!(
            nodes
                .iter()
                .find(|node| node.label == label)
                .unwrap()
                .disabled,
            "{label} needs notes"
        );
    }
    n.select(&[0, 1, 2]);
    let nodes = Harness::nodes(&n.h.settle());
    assert!(
        !nodes
            .iter()
            .find(|node| node.label == "Play One")
            .unwrap()
            .disabled
    );
    assert!(
        nodes
            .iter()
            .find(|node| node.label == "Ungroup")
            .unwrap()
            .disabled,
        "no note is grouped yet"
    );
    let before = n.doc().clone();
    let button = n.footer("Play One").center();
    n.h.click(button);
    n.h.settle();
    assert_eq!(n.groups()[0].kind, GroupKind::PlayOne);
    assert_eq!(n.grouped(), [Some(1), Some(1), Some(1), None]);
    n.one_step(&before, "Group Notes");
    // Play All on the same notes changes the group's kind.
    let button = n.footer("Play All").center();
    n.h.click(button);
    n.h.settle();
    assert_eq!(n.groups().len(), 1);
    assert_eq!(n.groups()[0].kind, GroupKind::PlayAll);
    let before = n.doc().clone();
    let button = n.footer("Ungroup").center();
    n.h.click(button);
    n.h.settle();
    assert!(n.groups().is_empty());
    n.one_step(&before, "Ungroup Notes");
}

// Spec: note-editing R24, R25, R5. A group shows one marker in the Chance
// lane, at its first note; dragging it sets the group's chance, in one step.
#[test]
fn a_group_has_one_marker_in_the_chance_lane_and_dragging_it_sets_the_group() {
    let step = n_step();
    let mut n = chance_notes(4);
    n.h.shell
        .run(
            "note.group",
            json!({ "clip": n.clip, "notes": [0, 1, 2], "kind": "play_one" }),
        )
        .unwrap();
    n.h.settle();
    let nodes = Harness::nodes(&n.h.settle());
    // The Chance lane's markers read a percentage.
    let in_chance = |nodes: &[Node]| {
        nodes
            .iter()
            .filter(|node| {
                node.role == egui::accesskit::Role::ListItem && node.value.ends_with('%')
            })
            .count()
    };
    assert_eq!(
        in_chance(&nodes),
        2,
        "one marker for the group, one for the loner"
    );
    // A press where the group's other notes start grabs nothing.
    let before = n.doc().clone();
    let from = n.lane_at(Ticks::ZERO, NoteField::Chance, 1.0);
    let to = n.lane_at(Ticks::ZERO, NoteField::Chance, 0.5);
    n.drag(from, to, Modifiers::NONE);
    let chances: Vec<f64> = n.notes().iter().map(|note| note.probability).collect();
    assert!(
        chances[0] < 1.0 && chances[0] == chances[1] && chances[1] == chances[2],
        "{chances:?}"
    );
    assert_eq!(chances[3], 1.0, "the loner is its own");
    n.one_step(&before, "Adjust note values");
    let other = n.lane_at(8 * step, NoteField::Chance, 1.0);
    let selected = n.selected();
    n.click(other, Modifiers::NONE);
    assert_eq!(
        n.selected(),
        selected,
        "no marker there: the group's is at its first note"
    );
}

// Spec: note-editing R23, R24, C-PGROUP-001 to C-PGROUP-003. The group
// marker's menu changes the group's kind, ticked, and ends the group.
#[test]
fn the_group_marker_s_menu_changes_its_kind_and_ends_it() {
    // The menu's kinds are the buttons that can be ticked; the footer has
    // buttons of the same names.
    fn kind_item(nodes: &[Node], label: &str) -> Node {
        nodes
            .iter()
            .find(|node| node.label == label && node.toggled.is_some())
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
            .clone()
    }
    fn click(n: &mut Notes, node: &Node) {
        let b = node.bounds.expect("bounds");
        n.h.click(pos2(
            ((b.x0 + b.x1) / 2.0) as f32,
            ((b.y0 + b.y1) / 2.0) as f32,
        ));
        n.h.settle();
    }
    let mut n = chance_notes(3);
    n.h.shell
        .run(
            "note.group",
            json!({ "clip": n.clip, "kind": "play_all", "notes": [0, 1] }),
        )
        .unwrap();
    n.h.settle();
    let marker = n.lane_at(Ticks::ZERO, NoteField::Chance, 1.0);
    let nodes = right_click(&mut n, marker);
    assert_eq!(
        kind_item(&nodes, "Play All").toggled,
        Some(true),
        "the kind is ticked"
    );
    assert_eq!(kind_item(&nodes, "Play One").toggled, Some(false));
    let before = n.doc().clone();
    click(&mut n, &kind_item(&nodes, "Play One"));
    assert_eq!(n.groups().len(), 1);
    assert_eq!(n.groups()[0].kind, GroupKind::PlayOne);
    n.one_step(&before, "Change Group");
    let nodes = right_click(&mut n, marker);
    assert_eq!(kind_item(&nodes, "Play One").toggled, Some(true));
    let before = n.doc().clone();
    n.click_node(&nodes, egui::accesskit::Role::Button, "Ungroup Notes");
    assert!(n.groups().is_empty(), "the whole group ended");
    assert_eq!(n.grouped(), [None; 3]);
    n.one_step(&before, "Ungroup Notes");
    // The editor's own menu has the items too, when they apply: Ungroup
    // Notes ended the group, and the notes are still selected.
    let at = n.at(60, n_step() / 2);
    let nodes = right_click(&mut n, at);
    assert_eq!(n.selected(), [0, 1]);
    assert!(!menu_item(&nodes, "Group Notes").disabled);
    assert!(
        menu_item(&nodes, "Ungroup Notes").disabled,
        "none is grouped"
    );
}

// Spec: note-editing R4, R24. A grouped note wears the corner triangle
// whatever its chance, and hovering a note marks its whole group.
#[test]
fn a_grouped_note_wears_the_corner_triangle_and_hovering_marks_the_group() {
    let mut n = run_of_notes(3, 100);
    n.h.shell
        .run("note.group", json!({ "clip": n.clip, "notes": [0, 1] }))
        .unwrap();
    n.h.settle();
    let notes = n.notes();
    assert!(shows_corner(&notes[0], 12.0) && shows_corner(&notes[1], 12.0));
    assert!(!shows_corner(&notes[2], 12.0), "the loner always plays");
    assert!(
        bl_ui::note_lanes::is_marked(&notes, Some(0), 1),
        "the group's other note"
    );
    assert!(bl_ui::note_lanes::is_marked(&notes, Some(0), 0));
    assert!(!bl_ui::note_lanes::is_marked(&notes, Some(0), 2));
    assert!(!bl_ui::note_lanes::is_marked(&notes, Some(2), 1));
}

// Spec: note-editing R24. While notes are selected the Status Bar names
// their groups' kind, with an asterisk across kinds.
#[test]
fn the_status_bar_names_the_selected_notes_group_kind() {
    let mut n = run_of_notes(4, 100);
    assert_eq!(n.h.shell.note_group_status(), None);
    for (notes, kind) in [([0, 1], "play_all"), ([2, 3], "play_one")] {
        n.h.shell
            .run(
                "note.group",
                json!({ "clip": n.clip, "notes": notes, "kind": kind }),
            )
            .unwrap();
    }
    n.select(&[0]);
    assert_eq!(
        n.h.shell.note_group_status().as_deref(),
        Some("Probability group: Play All")
    );
    // The Status Bar paints it.
    let output = n.h.settle();
    fn painted(shape: &egui::Shape, text: &str) -> bool {
        match shape {
            egui::Shape::Text(t) => t.galley.job.text == text,
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| painted(shape, text)),
            _ => false,
        }
    }
    assert!(
        output
            .shapes
            .iter()
            .any(|clipped| painted(&clipped.shape, "Probability group: Play All")),
        "the Status Bar reads it"
    );
    n.select(&[2]);
    assert_eq!(
        n.h.shell.note_group_status().as_deref(),
        Some("Probability group: Play One")
    );
    n.select(&[0, 3]);
    assert_eq!(
        n.h.shell.note_group_status().as_deref(),
        Some("Probability group: Play All, Play One*")
    );
    n.select(&[]);
    assert_eq!(n.h.shell.note_group_status(), None);
}

/// The middle of the control labelled `label` in the filter bar.
fn control(n: &mut Notes, label: &str) -> Pos2 {
    let output = n.h.settle();
    let nodes = Harness::nodes(&output);
    let bounds = Harness::node(&nodes, label)
        .bounds
        .unwrap_or_else(|| panic!("{label} has bounds"));
    pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    )
}

/// The filters the shell holds.
fn filters(n: &Notes) -> bl_shell::note_filter_commands::NoteFilters {
    n.h.shell.clip_view().note_filters().clone()
}

#[test]
fn the_magnifier_and_cmd_shift_f_show_the_bar_above_the_ruler() {
    // Spec: find-and-select-notes R1, R15. The switch and the key show and
    // hide the bar between the toolbar and the ruler, and the keyboard stays
    // in the editor so the key works after a click on the switch.
    let step = n_step();
    let mut n = set(vec![Note::new(60, Ticks::ZERO, step, 100)]);
    let view = n.view();
    let ruler = n.strip();
    assert!(!filters(&n).visible);
    let switch = control(&mut n, "Find and Select Notes");
    assert_eq!(
        switch,
        magnifier_rect(view).center(),
        "the magnifier leads the toolbar"
    );
    n.h.click(switch);
    n.h.settle();
    assert!(filters(&n).visible, "the magnifier shows the bar");
    assert_eq!(
        n.strip().top(),
        ruler.top() + BAR_HEIGHT,
        "the ruler moves down by the bar's height"
    );
    assert_eq!(
        n.strip().top(),
        bar_rect(view).bottom(),
        "just under the bar"
    );
    let output = n.h.settle();
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Find and Select Notes");
    assert_eq!(node.toggled, Some(true), "read as a switch with its state");
    assert_eq!(Harness::node(&nodes, "Filter").value, "Pitch");
    n.h.key(egui::Key::F, command() | Modifiers::SHIFT);
    assert!(
        !filters(&n).visible,
        "Cmd-Shift-F hides it, focus in the editor"
    );
    n.h.key(egui::Key::F, command() | Modifiers::SHIFT);
    assert!(filters(&n).visible, "and shows it");
    assert_eq!(n.steps(), 0, "the bar is view state, no undo step");
}

#[test]
fn a_control_selects_live_and_a_click_switches_the_filters_off() {
    // Spec: find-and-select-notes R3, R5, R2. A pitch class selects its
    // notes as it's clicked, the chooser's dot follows, and a click in the
    // note area switches the filter off and deselects.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(62, 2 * step, step, 100),
        Note::new(72, 4 * step, step, 100),
        Note::new(64, 6 * step, step, 100),
    ]);
    n.h.shell
        .run("clip_view.note_filters", json!({ "visible": true }))
        .unwrap();
    n.h.settle();
    let steps = n.steps();
    let output = n.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Note Filters").description, "All off");
    let c = control(&mut n, "C");
    n.h.click(c);
    let output = n.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(
        Harness::node(&nodes, "Note Filters").description,
        "On: Pitch"
    );
    assert!(filters(&n).pitch.on);
    assert_eq!(filters(&n).pitch.classes, [0]);
    assert_eq!(n.selected(), [0, 2], "C in both octaves");
    let d = control(&mut n, "D");
    n.h.click(d);
    assert_eq!(n.selected(), [0, 1, 2], "and D too");
    let invert = control(&mut n, "Invert");
    n.h.click(invert);
    assert_eq!(n.selected(), [3], "inverted: only the E is left");
    // A click in the note area switches every filter off, keeping the
    // settings, and deselects before doing what a click does.
    let empty = n.at(64, 8 * step);
    n.click(empty, Modifiers::NONE);
    assert!(!filters(&n).any_on());
    assert_eq!(filters(&n).pitch.classes, [0, 2], "the settings stay");
    assert!(n.selected().is_empty());
    assert_eq!(n.steps(), steps, "none of it is an undo step");
    // Select switches the shown filter back on and selects again.
    let select = control(&mut n, "Select");
    n.h.click(select);
    assert!(filters(&n).pitch.on);
    assert_eq!(n.selected(), [3], "Invert was still on");
}

#[test]
fn a_shift_drag_sets_the_time_filters_window() {
    // Spec: find-and-select-notes R6, I-DRAG-042. With the bar showing,
    // Shift-dragging sideways sets Start to where the drag began and Length
    // to the span, on the grid, and selects live.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(60, step, step, 100),
        Note::new(60, 2 * step, step, 100),
        Note::new(60, 3 * step, step, 100),
    ]);
    n.h.shell
        .run(
            "clip_view.note_filters",
            json!({ "visible": true, "time": { "repeat": 0 } }),
        )
        .unwrap();
    n.h.settle();
    let from = n.at(62, step);
    let to = n.at(62, 3 * step);
    let steps = n.steps();
    n.drag(from, to, Modifiers::SHIFT);
    let time = filters(&n).time;
    let beats = step.get() as f64 / Ticks::QUARTER.get() as f64;
    assert!(time.on);
    assert_eq!(
        (time.start, time.length),
        (1.0 + beats, 2.0 * beats),
        "from where the drag began, as long as it ran"
    );
    assert_eq!(n.selected(), [1, 2], "the notes in the window");
    assert_eq!(n.steps(), steps);
}

#[test]
fn invert_selection_runs_from_the_key_and_the_context_menu() {
    // Spec: find-and-select-notes R13. Cmd-Shift-A swaps the selection.
    let step = n_step();
    let mut n = set(vec![
        Note::new(60, Ticks::ZERO, step, 100),
        Note::new(62, 2 * step, step, 100),
        Note::new(64, 4 * step, step, 100),
    ]);
    let at = n.at(62, 2 * step + step / 2);
    n.click(at, Modifiers::NONE);
    assert_eq!(n.selected(), [1]);
    n.h.key(egui::Key::A, command() | Modifiers::SHIFT);
    assert_eq!(n.selected(), [0, 2]);
    assert_eq!(
        n.time(),
        Some((Ticks::ZERO, 5 * step)),
        "the time selection follows"
    );
    let nodes = right_click(&mut n, at);
    let item = menu_item(&nodes, "Invert Selection");
    assert!(!item.disabled);
}
