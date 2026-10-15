//! The Clip View's note editing, driven headless through `Shell::run` and
//! `Shell::gesture`, as agents and the Clip View's mouse drive it (WP-2.3;
//! D17). Every edit is one undo step, and undo and redo restore the set
//! exactly. The behaviour is in the piano roll spec.

use std::path::{Path, PathBuf};

use bl_commands::Permission;
use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, Document, Editor, MidiData, Note, TrackId};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::note_commands::{self as ids, ClipViewState, NoteEditResult};
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();
const SIXTEENTH: i64 = Ticks::QUARTER.get() / 4;

/// Opens a set held in memory.
struct Preloaded(Option<Editor>);

impl SetStore for Preloaded {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.0.take().ok_or("opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _: &Path, _: &Editor) -> Result<Vec<String>, String> {
        Err("not saved".to_owned())
    }

    fn save_as(&mut self, _: &Path, _: &Editor) -> Result<(PathBuf, Vec<String>), String> {
        Err("not saved".to_owned())
    }

    fn close(&mut self) {}
}

/// A song with a MIDI clip on the first MIDI track, selected, as a click on
/// it or its Arrangement selection leaves it.
struct Song {
    shell: Shell,
    clip: ClipId,
    track: TrackId,
}

fn song(notes: Vec<Note>) -> Song {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let track = tracks[0];
    let id: ClipId = editor.new_id();
    let mut clip = Clip::new(
        id,
        Ticks::ZERO,
        Ticks(4 * BAR),
        ClipData::Midi(MidiData::new(notes)),
    );
    clip.name = "Notes".to_owned();
    editor.apply(Command::CreateClip { track, clip }).unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Notes.blset" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [id] }))
        .unwrap();
    Song {
        shell,
        clip: id,
        track,
    }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn notes(&self) -> Vec<Note> {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => notes_in_order(midi),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    fn edit(&mut self, id: &str, args: Value) -> NoteEditResult {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }

    fn view(&mut self, id: &str, args: Value) -> ClipViewState {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }
    /// Runs an edit, checks it's exactly one undo step, and that undo and
    /// redo restore the set exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value) -> NoteEditResult {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self.edit(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert!(result.undo_step, "{id}");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        assert!(self.doc().check_invariants().is_ok());
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }
}

#[test]
fn drawing_a_note_is_one_undo_step() {
    // Spec: piano-roll R8. A drawn note is one grid step long and sounds at
    // 100.
    let mut song = song(Vec::new());
    let result = song.one_step(
        ids::ADD_NOTE,
        json!({ "key": 60, "start": 0, "length": SIXTEENTH }),
    );
    assert_eq!(song.notes(), [song_note(60, 0, SIXTEENTH)]);
    assert_eq!(result.notes, [0]);
    assert_eq!(result.selected, [0], "the new note is selected");
    assert_eq!(song.doc().clip_track(song.clip), Some(song.track));
    // The clip was left out: the Clip View shows the Arrangement's selected
    // clip.
    assert_eq!(
        song.view(ids::SELECT_NOTES, json!({ "notes": [0] })).clip,
        Some(song.clip)
    );
}

#[test]
fn moving_notes_in_time_and_pitch_is_one_step() {
    // Spec: piano-roll R7, R10. A drag right runs it with semitones 0, a
    // drag up runs it with by 0.
    let mut song = song(vec![
        song_note(48, 0, SIXTEENTH),
        song_note(60, BAR, QUARTER),
    ]);
    let result = song.one_step(
        ids::MOVE_NOTES,
        json!({ "notes": [0], "by": BAR, "semitones": 0 }),
    );
    assert_eq!(song.notes()[0].start, Ticks(BAR), "in time");
    assert_eq!(song.notes()[0].key, 48, "pitch unchanged");
    assert_eq!(result.selected, [0], "the moved note stays selected");
    song.one_step(
        ids::MOVE_NOTES,
        json!({ "notes": [0], "by": 0, "semitones": 12 }),
    );
    assert_eq!(song.notes()[0].key, 60, "in pitch");
    assert_eq!(song.notes()[0].start, Ticks(BAR), "time unchanged");
    // Dragging the selection: no `notes` acts on what's selected.
    song.one_step(ids::MOVE_NOTES, json!({ "by": -SIXTEENTH }));
    assert_eq!(song.notes()[0].start, Ticks(BAR - SIXTEENTH));
}

#[test]
fn resizing_notes_moves_their_ends_and_keeps_a_tick() {
    // Spec: piano-roll R9. The starts don't move, and a note keeps at least
    // one tick.
    let mut song = song(vec![
        song_note(60, BAR, QUARTER),
        song_note(64, 2 * BAR, QUARTER),
    ]);
    let result = song.one_step(
        ids::RESIZE_NOTES,
        json!({ "notes": [0, 1], "by": -SIXTEENTH }),
    );
    assert_eq!(song.notes()[0].length, Ticks(QUARTER - SIXTEENTH));
    assert_eq!(song.notes()[1].length, Ticks(QUARTER - SIXTEENTH));
    assert_eq!(result.notes, [0, 1], "the resized notes stay selected");
    let before = song.doc().clone();
    let steps = song.steps();
    assert!(
        song.shell
            .run(ids::RESIZE_NOTES, json!({ "notes": [0], "by": -QUARTER }))
            .is_err(),
        "a shorter-than-a-tick note is refused"
    );
    assert_eq!(*song.doc(), before, "and changes nothing");
    assert_eq!(song.steps(), steps, "and records no step");
}

#[test]
fn selecting_notes_is_never_an_undo_step() {
    // Spec: piano-roll R5. Replace and toggle, and the selection is never a
    // step.
    let mut song = song(vec![song_note(60, 0, QUARTER), song_note(64, BAR, QUARTER)]);
    let steps = song.steps();
    let state = song.view(
        ids::SELECT_NOTES,
        json!({ "notes": [1], "mode": "replace" }),
    );
    assert_eq!(state.selected, [1]);
    let state = song.view(ids::SELECT_NOTES, json!({ "notes": [0], "mode": "toggle" }));
    assert_eq!(state.selected, [1, 0]);
    let state = song.view(ids::SELECT_NOTES, json!({ "notes": [1], "mode": "toggle" }));
    assert_eq!(state.selected, [0], "toggle removes one");
    let state = song.view(ids::SELECT_NOTES, json!({ "notes": [] }));
    assert!(state.selected.is_empty(), "replace with none clears");
    assert_eq!(song.steps(), steps, "selecting is never an undo step");
    assert!(!song.shell.is_dirty(), "and doesn't dirty the set");
}

#[test]
fn notes_are_named_by_their_place_in_order() {
    // Spec: piano-roll R3. Place 0 is the earliest start, and of notes that
    // start together the lowest pitch.
    let mut song = song(vec![
        song_note(72, BAR, QUARTER),
        song_note(64, 0, QUARTER),
        song_note(60, 0, QUARTER),
    ]);
    song.one_step(ids::DELETE_NOTES, json!({ "notes": [0] }));
    assert_eq!(song.notes().len(), 2);
    assert!(
        song.notes().iter().all(|note| note.key != 60),
        "the lowest first"
    );
    song.one_step(ids::DELETE_NOTES, json!({ "notes": [0] }));
    assert_eq!(song.notes()[0].key, 72, "the earliest then");
}

#[test]
fn a_move_that_would_leave_the_keys_or_the_contents_changes_nothing() {
    // Spec: piano-roll R7 (the Refused rows).
    let mut song = song(vec![song_note(0, 0, QUARTER), song_note(127, BAR, QUARTER)]);
    let before = song.doc().clone();
    let steps = song.steps();
    for args in [
        json!({ "notes": [0], "semitones": -1 }),
        json!({ "notes": [1], "semitones": 1 }),
        json!({ "notes": [0], "by": -SIXTEENTH }),
    ] {
        assert!(
            song.shell.run(ids::MOVE_NOTES, args).is_err(),
            "refused, changing nothing"
        );
        assert_eq!(*song.doc(), before);
        assert_eq!(song.steps(), steps);
    }
}

#[test]
fn the_clip_view_and_draw_mode_are_view_switches() {
    // Spec: piano-roll R1, R2. Both are view state: no undo step, and the
    // result says what they show.
    let mut song = song(Vec::new());
    let steps = song.steps();
    let state = song.view("view.toggle_clip_view", json!({}));
    assert!(state.visible);
    assert_eq!(state.clip, Some(song.clip), "the selected MIDI clip shows");
    assert!(song.shell.clip_view().visible());
    assert!(!song.shell.is_dirty(), "the set doesn't change");
    let state = song.view("view.draw_mode", json!({}));
    assert!(state.draw_mode);
    assert!(song.shell.clip_view().draw_mode());
    assert_eq!(song.steps(), steps, "neither is an undo step");
    assert!(!song.view("view.toggle_clip_view", json!({})).visible);
}

#[test]
fn note_edits_are_registered_with_their_classes() {
    // Spec: piano-roll R13.
    let mut registry = bl_commands::Registry::new();
    bl_shell::register(&mut registry).unwrap();
    for id in ids::IDS {
        assert!(registry.contains(id), "{id}");
    }
    let class = |id: &str| registry.get(id).unwrap().permission;
    assert_eq!(class(ids::ADD_NOTE), Some(Permission::Edit));
    assert_eq!(class(ids::MOVE_NOTES), Some(Permission::Edit));
    assert_eq!(class(ids::RESIZE_NOTES), Some(Permission::Edit));
    assert_eq!(class(ids::DELETE_NOTES), Some(Permission::Destructive));
    assert_eq!(class(ids::SELECT_NOTES), Some(Permission::Read));
}

const QUARTER: i64 = Ticks::QUARTER.get();

/// A note for building songs, outside `Song` so a song's notes can be
/// written in its own construction.
fn song_note(key: u8, start: i64, length: i64) -> Note {
    Note::new(key, Ticks(start), Ticks(length), 100)
}
