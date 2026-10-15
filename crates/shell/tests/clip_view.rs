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
fn moving_a_note_names_it_where_it_lands() {
    // Spec: piano-roll R3, R7, R13. The result and the selection name the
    // moved note by its new place, not the first place.
    let mut song = song(vec![
        song_note(60, 0, SIXTEENTH),
        song_note(64, BAR, QUARTER),
    ]);
    let result = song.one_step(
        ids::MOVE_NOTES,
        json!({ "notes": [1], "by": BAR, "semitones": 0 }),
    );
    assert_eq!(
        song.notes()[1].start,
        Ticks(2 * BAR),
        "the later note moved"
    );
    assert_eq!(result.notes, [1], "reported where it landed");
    assert_eq!(result.selected, [1], "and selected there");
    // The next command with no `notes` acts on that selection, so it moves
    // the moved note, not the one at place 0.
    song.one_step(ids::MOVE_NOTES, json!({ "semitones": 12 }));
    assert_eq!(song.notes()[1].key, 76, "the moved note, up a semitone");
    assert_eq!(song.notes()[0].key, 60, "place 0 is untouched");
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
    for id in [
        bl_shell::commands::VIEW_FOLD_NOTES,
        bl_shell::commands::VIEW_LANE,
    ] {
        assert!(registry.contains(id), "{id}");
    }
    let class = |id: &str| registry.get(id).unwrap().permission;
    assert_eq!(class(ids::ADD_NOTE), Some(Permission::Edit));
    assert_eq!(class(ids::MOVE_NOTES), Some(Permission::Edit));
    assert_eq!(class(ids::RESIZE_NOTES), Some(Permission::Edit));
    assert_eq!(class(ids::SET_VALUE), Some(Permission::Edit));
    assert_eq!(class(ids::RAMP_VALUE), Some(Permission::Edit));
    assert_eq!(class(ids::DELETE_NOTES), Some(Permission::Destructive));
    assert_eq!(class(ids::SELECT_NOTES), Some(Permission::Read));
    assert_eq!(
        class(bl_shell::commands::VIEW_FOLD_NOTES),
        Some(Permission::Read)
    );
    assert_eq!(class(bl_shell::commands::VIEW_LANE), Some(Permission::Read));
}

#[test]
fn setting_a_notes_values_is_one_step() {
    // Spec: piano-roll R14. Dragging a stem in the lane sets that note's
    // value, and the agent API runs the same command.
    let mut song = song(vec![
        song_note(48, 0, SIXTEENTH),
        song_note(60, BAR, QUARTER),
        song_note(64, 2 * BAR, QUARTER),
    ]);
    let result = song.one_step(
        ids::SET_VALUE,
        json!({ "notes": [0], "field": "velocity", "value": 64 }),
    );
    assert_eq!(song.notes()[0].velocity, 64);
    assert_eq!(
        song.notes()[1..]
            .iter()
            .map(|note| note.velocity)
            .collect::<Vec<_>>(),
        [100, 100],
        "nothing else changed"
    );
    assert_eq!(result.notes, [0], "the note stays selected");
    // With no `notes`, the command acts on the selection, as the lane's
    // stem drag after a click does.
    song.one_step(ids::SET_VALUE, json!({ "field": "chance", "value": 0.25 }));
    assert_eq!(song.notes()[0].probability, 0.25);
    assert_eq!(
        song.notes()[1].probability,
        1.0,
        "the other notes keep their chance"
    );
    // Release velocity and velocity deviation take whole numbers.
    song.one_step(
        ids::SET_VALUE,
        json!({ "notes": [1], "field": "release_velocity", "value": 32 }),
    );
    song.one_step(
        ids::SET_VALUE,
        json!({ "notes": [1], "field": "velocity_deviation", "value": -10 }),
    );
    assert_eq!(song.notes()[1].release_velocity, 32);
    assert_eq!(song.notes()[1].velocity_deviation, -10);
}

#[test]
fn a_ramp_sets_evenly_spaced_values() {
    // Spec: piano-roll R14. Dragging across several notes ramps their
    // values, from the first note to the last.
    let mut song = song(vec![
        song_note(48, 0, SIXTEENTH),
        song_note(60, BAR, QUARTER),
        song_note(64, 2 * BAR, QUARTER),
    ]);
    let result = song.one_step(
        ids::RAMP_VALUE,
        json!({ "notes": [0, 1, 2], "field": "velocity", "from": 20, "to": 120 }),
    );
    assert_eq!(
        song.notes()
            .iter()
            .map(|note| note.velocity)
            .collect::<Vec<_>>(),
        [20, 70, 120],
        "evenly spaced from the first note to the last"
    );
    assert_eq!(result.notes, [0, 1, 2], "all of them stay selected");
    // Chance is a fraction, so its ramp is too.
    let values = song.one_step(
        ids::RAMP_VALUE,
        json!({ "field": "chance", "from": 0.0, "to": 1.0 }),
    );
    assert_eq!(values.notes, [0, 1, 2]);
    let chances: Vec<f64> = song.notes().iter().map(|note| note.probability).collect();
    assert_eq!(chances, [0.0, 0.5, 1.0]);
    // One note takes the ramp's first value.
    song.one_step(
        ids::RAMP_VALUE,
        json!({ "notes": [1], "field": "velocity", "from": 33, "to": 99 }),
    );
    assert_eq!(song.notes()[1].velocity, 33);
}

#[test]
fn a_value_out_of_range_or_fractional_changes_nothing() {
    // Spec: piano-roll R14 (the invalid-argument rows).
    let mut song = song(vec![song_note(60, 0, QUARTER), song_note(64, BAR, QUARTER)]);
    let before = song.doc().clone();
    let steps = song.steps();
    for args in [
        json!({ "notes": [0], "field": "velocity", "value": 0 }),
        json!({ "notes": [0], "field": "velocity", "value": 128 }),
        json!({ "notes": [0], "field": "velocity", "value": 99.5 }),
        json!({ "notes": [0], "field": "release_velocity", "value": 0 }),
        json!({ "notes": [0], "field": "chance", "value": 1.5 }),
        json!({ "notes": [0], "field": "chance", "value": -0.1 }),
        json!({ "notes": [0], "field": "velocity_deviation", "value": 128 }),
        json!({ "notes": [0, 1], "field": "velocity", "from": 20, "to": 128 }),
        json!({ "notes": [0], "field": "velocity", "value": 60, "nope": 1 }),
    ] {
        let id = if args.get("from").is_some() {
            ids::RAMP_VALUE
        } else {
            ids::SET_VALUE
        };
        assert!(
            song.shell.run(id, args.clone()).is_err(),
            "{args} is refused"
        );
        assert_eq!(*song.doc(), before, "{args} changes nothing");
        assert_eq!(song.steps(), steps, "{args} records no step");
    }
}

#[test]
fn the_lane_and_fold_are_view_switches() {
    // Spec: piano-roll R14, R15. The lane's value and the fold are view
    // state: no undo step, and the set isn't dirtied.
    let mut song = song(vec![song_note(60, 0, QUARTER)]);
    let steps = song.steps();
    let state = song.view(bl_shell::commands::VIEW_LANE, json!({ "lane": "chance" }));
    assert_eq!(state.lane, bl_shell::note_commands::NoteField::Chance);
    assert_eq!(song.shell.clip_view().lane().name(), "chance");
    // Fold (F, View ▸ Fold to Notes) folds the clip the editor shows.
    let state = song.view(bl_shell::commands::VIEW_FOLD_NOTES, json!({}));
    assert!(state.folded);
    assert!(song.shell.clip_view().is_folded(song.clip));
    // `folded` says which way, and left out it switches.
    let state = song.view(
        bl_shell::commands::VIEW_FOLD_NOTES,
        json!({ "folded": true }),
    );
    assert!(state.folded, "asked for folded, stays folded");
    let state = song.view(bl_shell::commands::VIEW_FOLD_NOTES, json!({}));
    assert!(!state.folded, "and switches back");
    assert_eq!(state.lane, bl_shell::note_commands::NoteField::Chance);
    assert_eq!(song.steps(), steps, "neither is an undo step");
    assert!(!song.shell.is_dirty(), "and neither dirties the set");
}

#[test]
fn a_folded_editor_survives_a_set_round_trip() {
    // Spec: piano-roll R15. Folding is saved with the set beside the song,
    // like the floating devices' windows, and a new set starts unfolded.
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: tracks[0],
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * BAR),
                ClipData::Midi(MidiData::new([song_note(60, 0, QUARTER)])),
            ),
        })
        .unwrap();
    let dir = std::env::temp_dir().join(format!(
        "bl-shell-folds-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Folds.blset");
    let mut set = bl_shell::files::ProjectFiles::new();
    set.save(&path, &editor).unwrap();

    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    shell
        .run("set.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    assert!(
        shell.editor().document().clip(clip).is_some(),
        "the clip is there"
    );
    shell
        .run(
            bl_shell::commands::VIEW_FOLD_NOTES,
            json!({ "clip": clip, "folded": true }),
        )
        .unwrap();
    assert!(shell.clip_view().is_folded(clip));
    assert!(!shell.is_dirty(), "folding doesn't dirty the set");
    shell.run("set.save", json!({})).unwrap();
    shell.run("set.new", json!({})).unwrap();
    assert!(
        shell.clip_view().folds().is_empty(),
        "a new set starts unfolded"
    );
    shell
        .run("set.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    assert!(
        shell.clip_view().is_folded(clip),
        "the reopened set shows the folded editor"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const QUARTER: i64 = Ticks::QUARTER.get();

/// A note for building songs, outside `Song` so a song's notes can be
/// written in its own construction.
fn song_note(key: u8, start: i64, length: i64) -> Note {
    Note::new(key, Ticks(start), Ticks(length), 100)
}
