//! The Clip View's note editing, headless (WP-2.3): clicking, dragging, and
//! the marquee reach the same note commands agents run, and each edit is one
//! undo step however many frames its drag spans. Spec: piano-roll.

mod common;

use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, Document, MidiData, Note, TrackId};
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use bl_ui::clip_view::{KEY_HEIGHT, grid_rect, note_rect, tick_at, x_of_tick, y_of_key};
use common::{Harness, Window, command};
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

    /// The editing grid's step under the Clip View: one grid step.
    fn step(&self) -> Ticks {
        self.h.shell.arrangement().grid_ticks(self.doc())
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    /// The note area's rectangle, from the Clip View's accessible node.
    fn grid(&mut self) -> Rect {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let node = Harness::node(&nodes, "Clip View");
        let bounds = node.bounds.expect("the Clip View has bounds");
        let view = Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        );
        grid_rect(view)
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
    let note = n.notes()[0];
    let top = n.h.main.clip_view().top();
    let rect = note_rect(grid, n.marker(), n.length(), top, &note);
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
