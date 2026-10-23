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
use bl_shell::note_commands::{
    self as ids, ClipViewState, NoteEditResult, QuantizeSettings, QuantizeTarget,
};
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

    /// The Quantize settings a settings command reports.
    fn settings(&mut self, id: &str, args: Value) -> QuantizeSettings {
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
fn quantising_at_100_percent_puts_starts_on_the_grid() {
    // Spec: piano-roll (WP-2.3 part 3; the quantise rule). A note a little
    // after a grid line and one a little before the next land on those
    // lines, and each quantise is one undo step.
    let mut song = song(vec![
        song_note(60, SIXTEENTH + 30, QUARTER),
        song_note(64, 2 * SIXTEENTH - 30, QUARTER),
    ]);
    let result = song.one_step(
        ids::QUANTIZE_NOTES,
        json!({ "notes": [0, 1], "grid": SIXTEENTH, "amount": 100 }),
    );
    assert_eq!(song.notes()[0].start, Ticks(SIXTEENTH));
    assert_eq!(song.notes()[1].start, Ticks(2 * SIXTEENTH));
    assert_eq!(song.notes()[0].key, 60, "quantising moves no pitch");
    assert_eq!(song.notes()[0].length, Ticks(QUARTER), "no lengths move");
    assert_eq!(result.selected, [0, 1], "the quantised notes stay selected");
}

#[test]
fn quantising_at_50_percent_halves_the_distance_to_the_grid() {
    // Spec: piano-roll (WP-2.3 part 3). Each note keeps its start only when
    // the amount is 0; 50 percent moves it half way to the line.
    let mut song = song(vec![song_note(60, QUARTER + 200, QUARTER)]);
    song.one_step(
        ids::QUANTIZE_NOTES,
        json!({ "notes": [0], "grid": QUARTER, "amount": 50 }),
    );
    assert_eq!(
        song.notes()[0].start,
        Ticks(QUARTER + 100),
        "half of the 200-tick distance"
    );
    // The settings steer the next quantise when its arguments leave them
    // out, and 0 percent changes nothing at all.
    let state = song.settings(
        ids::QUANTIZE_SETTINGS,
        json!({ "grid": QUARTER, "target": "note_start", "amount": 0 }),
    );
    assert_eq!(state.amount, 0);
    let before = song.doc().clone();
    let steps = song.steps();
    let result = song.edit(ids::QUANTIZE_NOTES, json!({ "notes": [0] }));
    assert!(!result.undo_step, "0 percent records no step");
    assert_eq!(*song.doc(), before);
    assert_eq!(song.steps(), steps);
}

#[test]
fn quantising_ends_leaves_starts_alone() {
    // Spec: piano-roll (WP-2.3 part 3). With `note_end`, each note's end
    // moves to the line and its start stays exactly where it was.
    let mut notes = song(vec![song_note(60, SIXTEENTH + 30, QUARTER - 60)]);
    notes.one_step(
        ids::QUANTIZE_NOTES,
        json!({ "notes": [0], "grid": SIXTEENTH, "target": "note_end", "amount": 100 }),
    );
    assert_eq!(
        notes.notes()[0].start,
        Ticks(SIXTEENTH + 30),
        "the start stays"
    );
    assert_eq!(
        notes.notes()[0].start + notes.notes()[0].length,
        Ticks(5 * SIXTEENTH),
        "the end sits on the line"
    );
    // An end whose nearest line is behind its start would leave a note of no
    // length: refused, changing nothing.
    let mut short = song(vec![song_note(60, 10, 20)]);
    let before = short.doc().clone();
    let steps = short.steps();
    assert!(
        short
            .shell
            .run(
                ids::QUANTIZE_NOTES,
                json!({ "notes": [0], "grid": SIXTEENTH, "target": "note_end" }),
            )
            .is_err(),
        "a shorter-than-a-tick note is refused"
    );
    assert_eq!(*short.doc(), before);
    assert_eq!(short.steps(), steps);
}

#[test]
fn the_quantise_settings_are_view_state() {
    // Spec: piano-roll (WP-2.3 part 3). Like Draw Mode, the settings change
    // nothing in the set and are no undo step; fields left out keep their
    // values.
    let mut song = song(Vec::new());
    let steps = song.steps();
    let state = song.settings(ids::QUANTIZE_SETTINGS, json!({}));
    assert_eq!(state.grid, None, "a new view follows the editor's grid");
    assert_eq!(state.target, QuantizeTarget::NoteStart);
    assert_eq!(state.amount, 100);
    let state = song.settings(
        ids::QUANTIZE_SETTINGS,
        json!({ "grid": QUARTER, "amount": 50 }),
    );
    assert_eq!(state.grid, Some(Ticks(QUARTER)));
    assert_eq!(state.target, QuantizeTarget::NoteStart, "the target stays");
    assert_eq!(state.amount, 50);
    assert_eq!(song.steps(), steps, "the settings are no undo step");
    assert!(!song.shell.is_dirty(), "and don't dirty the set");
    assert!(
        song.shell
            .run(ids::QUANTIZE_NOTES, json!({ "grid": 0 }))
            .is_err(),
        "a grid of no ticks is refused"
    );
    assert!(
        song.shell
            .run(ids::QUANTIZE_NOTES, json!({ "amount": 101 }))
            .is_err(),
        "an amount past 100 percent is refused"
    );
}

#[test]
fn quantising_to_bars_counts_them_from_1_1_1() {
    // Spec: piano-roll (WP-2.3 part 3). A grid of whole bars snaps to bar
    // lines; a grid of several bars counts blocks of them from 1.1.1, as
    // the editing grid's bar spacings do.
    let mut song = song(vec![
        song_note(60, SIXTEENTH, QUARTER),
        song_note(64, 4 * BAR + SIXTEENTH, QUARTER),
    ]);
    song.one_step(
        ids::QUANTIZE_NOTES,
        json!({ "notes": [0], "grid": BAR, "amount": 100 }),
    );
    assert_eq!(song.notes()[0].start, Ticks::ZERO, "bar 1");
    song.one_step(
        ids::QUANTIZE_NOTES,
        json!({ "notes": [1], "grid": 4 * BAR, "amount": 100 }),
    );
    assert_eq!(
        song.notes()[1].start,
        Ticks(4 * BAR),
        "the nearest line of the 4-bar grid"
    );
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

#[test]
fn the_insert_marker_and_the_time_selection_are_view_state() {
    // Spec: piano-roll R23, R24, R31. The insert marker and the time
    // selection belong to the view: they change nothing in the set and are
    // never an undo step, and the state reports them.
    let mut song = song(vec![song_note(60, 0, QUARTER)]);
    let steps = song.steps();
    let state = song.view(
        "clip_view.set_insert_marker",
        json!({ "clip": song.clip, "beat": 2 * QUARTER }),
    );
    assert_eq!(state.insert_marker, Some(Ticks(2 * QUARTER)));
    assert_eq!(state.time_selection, None);
    assert_eq!(song.steps(), steps, "the marker is never an undo step");
    assert!(!song.shell.is_dirty(), "and doesn't dirty the set");
    // The marker stays inside the clip's span.
    let state = song.view(
        "clip_view.set_insert_marker",
        json!({ "clip": song.clip, "beat": 100 * BAR }),
    );
    assert_eq!(
        state.insert_marker,
        Some(Ticks(4 * BAR)),
        "the clip's span ends it"
    );
    // A time selection is drawn across the note area and the lanes, and
    // moving the marker takes it away.
    let state = song.view(
        "clip_view.select_time",
        json!({ "clip": song.clip, "start": QUARTER, "end": 2 * QUARTER }),
    );
    assert_eq!(
        state.time_selection,
        Some((Ticks(QUARTER), Ticks(2 * QUARTER)))
    );
    assert_eq!(
        state.insert_marker,
        Some(Ticks(4 * BAR)),
        "the marker stays"
    );
    let state = song.view(
        "clip_view.set_insert_marker",
        json!({ "clip": song.clip, "beat": QUARTER }),
    );
    assert_eq!(state.time_selection, None, "the marker clears it");
    assert_eq!(song.steps(), steps, "none of it is an undo step");
}

#[test]
fn the_keyboard_selects_the_notes_the_spec_says() {
    // Spec: piano-roll R25, R26. The nearest note, the previous and next
    // note, the same pitch's, every note, and a key's notes.
    let mut song = song(vec![
        song_note(60, 0, QUARTER),
        song_note(64, QUARTER, QUARTER),
        song_note(60, 2 * QUARTER, QUARTER),
    ]);
    song.view(
        "clip_view.set_insert_marker",
        json!({ "clip": song.clip, "beat": QUARTER / 2 }),
    );
    // The nearest note's start, and of notes that start together the lowest
    // pitch.
    let state = song.view(ids::SELECT_NOTES, json!({ "select": "nearest" }));
    assert_eq!(state.selected, [0], "the nearest note's start");
    // The previous and next note in the clip's order.
    let state = song.view(ids::SELECT_NOTES, json!({ "select": "next" }));
    assert_eq!(state.selected, [1]);
    let state = song.view(ids::SELECT_NOTES, json!({ "select": "previous" }));
    assert_eq!(state.selected, [0]);
    // With Shift too the note is added to the selection; with nothing
    // selected that note is the nearest one (S-NOTE-043, S-A11Y-007).
    let state = song.view(ids::SELECT_NOTES, json!({ "notes": [] }));
    assert!(state.selected.is_empty());
    let state = song.view(
        ids::SELECT_NOTES,
        json!({ "select": "nearest", "add": true }),
    );
    assert_eq!(state.selected, [0], "the nearest note is added");
    // The previous and next note at the same pitch.
    let state = song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    assert_eq!(state.selected, [0]);
    let state = song.view(ids::SELECT_NOTES, json!({ "select": "same_pitch_next" }));
    assert_eq!(state.selected, [2], "the next note at 60");
    let state = song.view(
        ids::SELECT_NOTES,
        json!({ "select": "same_pitch_previous" }),
    );
    assert_eq!(state.selected, [0], "and the previous one");
    // Every note in the clip's span, and a key's notes.
    let state = song.view(ids::SELECT_NOTES, json!({ "select": "all" }));
    assert_eq!(state.selected, [0, 1, 2]);
    let state = song.view(ids::SELECT_NOTES, json!({ "select": { "key": 60 } }));
    assert_eq!(state.selected, [0, 2], "the notes at that pitch");
    // Shift-clicking a key adds its notes, or takes them out when they are
    // all selected.
    let state = song.view(
        ids::SELECT_NOTES,
        json!({ "select": { "key": 64 }, "add": true }),
    );
    assert_eq!(state.selected, [0, 1, 2], "the other key's notes are added");
    let state = song.view(
        ids::SELECT_NOTES,
        json!({ "select": { "key": 60 }, "add": true }),
    );
    assert_eq!(state.selected, [1], "and the first key's are taken out");
    // The time selection follows the notes.
    let state = song.view(ids::SELECT_NOTES, json!({ "select": { "key": 60 } }));
    assert_eq!(
        state.time_selection,
        Some((Ticks(0), Ticks(2 * QUARTER + QUARTER))),
        "from the earliest start to the latest end"
    );
}

#[test]
fn the_note_clipboard_copies_cuts_pastes_and_duplicates() {
    // Spec: piano-roll R28. The clipboard keeps the pitches, the values and
    // the places in time, and it's apart from the clips' clipboard.
    let mut notes = song(vec![
        song_note(60, 0, QUARTER),
        song_note(64, QUARTER, QUARTER),
    ]);
    // Copy the selected notes.
    notes.view(ids::SELECT_NOTES, json!({ "notes": [0, 1] }));
    let copied = notes
        .shell
        .run(ids::COPY, json!({ "clip": notes.clip }))
        .unwrap();
    assert_eq!(copied["notes"], 2, "two notes on the clipboard");
    assert_eq!(notes.steps(), 0, "copying is never an undo step");
    // Paste them at the insert marker: the pitches and the spacing hold.
    notes.view(
        "clip_view.set_insert_marker",
        json!({ "clip": notes.clip, "beat": 2 * QUARTER }),
    );
    let result = notes.one_step(ids::PASTE, json!({ "clip": notes.clip }));
    assert_eq!(result.selected.len(), 2, "the pasted notes are selected");
    assert_eq!(
        notes.notes(),
        [
            song_note(60, 0, QUARTER),
            song_note(64, QUARTER, QUARTER),
            song_note(60, 2 * QUARTER, QUARTER),
            song_note(64, 3 * QUARTER, QUARTER),
        ],
        "the copies keep their pitches and their spacing"
    );
    // Cut copies the notes and deletes them.
    notes.view(ids::SELECT_NOTES, json!({ "notes": [0, 1] }));
    notes.one_step(ids::CUT, json!({ "clip": notes.clip }));
    assert_eq!(notes.notes().len(), 2, "the cut notes are gone");
    // Delete removes the selected notes.
    notes.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    notes.one_step(ids::DELETE_NOTES, json!({ "clip": notes.clip }));
    assert_eq!(notes.notes().len(), 1);
    // Duplicate lays a copy straight after the time selection, and another
    // one after that.
    let mut copies = song(vec![song_note(60, 0, QUARTER)]);
    copies.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    copies.one_step(ids::DUPLICATE, json!({ "clip": copies.clip }));
    assert_eq!(
        copies.notes(),
        [song_note(60, 0, QUARTER), song_note(60, QUARTER, QUARTER)],
        "the copy lies straight after"
    );
    copies.one_step(ids::DUPLICATE, json!({ "clip": copies.clip }));
    assert_eq!(
        copies.notes(),
        [
            song_note(60, 0, QUARTER),
            song_note(60, QUARTER, QUARTER),
            song_note(60, 2 * QUARTER, QUARTER),
        ],
        "and another one after that"
    );
}
#[test]
fn copying_a_note_by_dragging_leaves_the_original() {
    // Spec: piano-roll R29. Option-dragging a note copies it: the notes
    // stay where they are and the copies land where they would have gone.
    let mut song = song(vec![song_note(60, 0, QUARTER)]);
    let result = song.one_step(
        ids::MOVE_NOTES,
        json!({ "notes": [0], "by": QUARTER, "copy": true }),
    );
    assert_eq!(
        song.notes(),
        [song_note(60, 0, QUARTER), song_note(60, QUARTER, QUARTER)],
        "the original stays and the copy lands where the note would have"
    );
    assert_eq!(result.selected, [1], "the copy is selected");
}

// Spec: note-editing R20. The value commands are in the edit class and the
// lane commands are read.
#[test]
fn the_note_value_commands_are_registered_with_their_classes() {
    let mut registry = bl_commands::Registry::new();
    bl_shell::register(&mut registry).unwrap();
    let class = |id: &str| registry.get(id).unwrap().permission;
    for id in [ids::ADJUST_VALUE, ids::RANDOMIZE, ids::DEACTIVATE] {
        assert!(registry.contains(id), "{id}");
        assert_eq!(class(id), Some(Permission::Edit), "{id}");
    }
    assert_eq!(
        class(bl_shell::commands::VIEW_LANES),
        Some(Permission::Read)
    );
}

// Spec: note-editing R15, R5. A change by an amount holds each note within
// its range, so dragging past an end brings every selected note to it.
#[test]
fn adjusting_values_is_one_step_and_holds_each_note_in_range() {
    let mut song = song(vec![
        song_note(48, 0, SIXTEENTH),
        song_note(60, BAR, QUARTER),
        song_note(64, 2 * BAR, QUARTER),
    ]);
    song.one_step(
        ids::SET_VALUE,
        json!({ "notes": [0], "field": "velocity", "value": 20 }),
    );
    song.view(ids::SELECT_NOTES, json!({ "notes": [0, 1] }));
    let result = song.one_step(ids::ADJUST_VALUE, json!({ "field": "velocity", "by": 10 }));
    let velocities =
        |song: &Song| -> Vec<u8> { song.notes().iter().map(|note| note.velocity).collect() };
    assert_eq!(
        velocities(&song),
        [30, 100 + 10, 100],
        "{:?}",
        velocities(&song)
    );
    assert_eq!(result.selected, [0, 1], "the notes stay selected");
    // Past the end, each stops at it.
    song.one_step(ids::ADJUST_VALUE, json!({ "field": "velocity", "by": 100 }));
    assert_eq!(velocities(&song), [127, 127, 100]);
    song.one_step(
        ids::ADJUST_VALUE,
        json!({ "field": "velocity", "by": -300 }),
    );
    assert_eq!(velocities(&song), [1, 1, 100]);
    // A change that moves no note is no step.
    let steps = song.steps();
    let result = song.edit(ids::ADJUST_VALUE, json!({ "field": "velocity", "by": -5 }));
    assert!(!result.undo_step);
    assert_eq!(song.steps(), steps);
    // Chance moves by fractions, and ten steps of 10 % reach 100 % exactly.
    song.one_step(ids::SET_VALUE, json!({ "field": "chance", "value": 0.0 }));
    for _ in 0..10 {
        song.edit(ids::ADJUST_VALUE, json!({ "field": "chance", "by": 0.1 }));
    }
    let chances: Vec<f64> = song.notes().iter().map(|note| note.probability).collect();
    assert_eq!(chances, [1.0, 1.0, 1.0]);
    // A fraction of a whole-number field, and a value that isn't a number,
    // change nothing.
    let before = song.doc().clone();
    for args in [
        json!({ "field": "velocity", "by": 1.5 }),
        json!({ "field": "velocity", "by": 1, "nope": 1 }),
        json!({ "field": "velocity", "notes": [9], "by": 1 }),
    ] {
        assert!(
            song.shell.run(ids::ADJUST_VALUE, args.clone()).is_err(),
            "{args}"
        );
        assert_eq!(*song.doc(), before, "{args}");
    }
}

// Spec: note-editing R7, R8. A note's deviation stays within the room its
// velocity leaves; a value outside -127 to 127 is still refused.
#[test]
fn a_deviation_is_held_to_the_room_its_velocity_leaves() {
    let mut song = song(vec![
        song_note(48, 0, SIXTEENTH),
        song_note(60, BAR, QUARTER),
    ]);
    song.one_step(
        ids::SET_VALUE,
        json!({ "notes": [0], "field": "velocity", "value": 50 }),
    );
    song.view(ids::SELECT_NOTES, json!({ "notes": [0, 1] }));
    // A note at 50 reaches +77; a note at 100 reaches +27.
    song.one_step(
        ids::SET_VALUE,
        json!({ "field": "velocity_deviation", "value": 127 }),
    );
    let deviations = |song: &Song| -> Vec<i8> {
        song.notes()
            .iter()
            .map(|note| note.velocity_deviation)
            .collect()
    };
    assert_eq!(deviations(&song), [77, 27]);
    // And below: at least 1 less than the velocity.
    song.one_step(
        ids::SET_VALUE,
        json!({ "field": "velocity_deviation", "value": -127 }),
    );
    assert_eq!(deviations(&song), [-49, -99]);
    // Adjusting by the key's step is held the same way.
    song.one_step(
        ids::ADJUST_VALUE,
        json!({ "field": "velocity_deviation", "by": 200 }),
    );
    assert_eq!(deviations(&song), [77, 27]);
    // A later change of velocity leaves the deviation as it is.
    song.one_step(
        ids::SET_VALUE,
        json!({ "notes": [1], "field": "velocity", "value": 120 }),
    );
    assert_eq!(deviations(&song), [77, 27]);
    // A ramp is held the same way.
    song.one_step(
        ids::RAMP_VALUE,
        json!({ "field": "velocity_deviation", "from": 100, "to": 100 }),
    );
    assert_eq!(deviations(&song), [77, 7]);
    let before = song.doc().clone();
    assert!(
        song.shell
            .run(
                ids::SET_VALUE,
                json!({ "field": "velocity_deviation", "value": 128 })
            )
            .is_err()
    );
    assert_eq!(*song.doc(), before);
}

// Spec: note-editing R11. A ramp spreads by start time, and notes that start
// together take the same value.
#[test]
fn a_ramp_spreads_by_start_time_with_chords_equal() {
    let mut song = song(vec![
        song_note(48, 0, SIXTEENTH),
        song_note(55, 0, SIXTEENTH),
        song_note(60, BAR, QUARTER),
        song_note(64, 3 * BAR, QUARTER),
        song_note(67, 3 * BAR, QUARTER),
    ]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0, 1, 2, 3, 4] }));
    song.one_step(
        ids::RAMP_VALUE,
        json!({ "field": "velocity", "from": 20, "to": 120 }),
    );
    let velocities: Vec<u8> = song.notes().iter().map(|note| note.velocity).collect();
    // Starts 0, 0, 1 bar, 3 bars, 3 bars: a third of the way at the bar.
    assert_eq!(velocities, [20, 20, 53, 120, 120]);
    // Every selected note starting together: all take the start value.
    song.view(ids::SELECT_NOTES, json!({ "notes": [0, 1] }));
    song.one_step(
        ids::RAMP_VALUE,
        json!({ "field": "velocity", "from": 70, "to": 90 }),
    );
    let velocities: Vec<u8> = song.notes().iter().map(|note| note.velocity).collect();
    assert_eq!(&velocities[..2], [70, 70]);
}

// Spec: note-editing R13. Randomize draws evenly around each note's value,
// the same seed repeats, and an amount of 0 changes nothing.
#[test]
fn randomizing_repeats_with_a_seed_and_an_amount_of_zero_changes_nothing() {
    let notes: Vec<Note> = (0..16)
        .map(|i| song_note(48 + i as u8, i * SIXTEENTH, SIXTEENTH))
        .collect();
    let mut song = song(notes);
    let before = song.doc().clone();
    let steps = song.steps();
    // Nothing selected: every note in the clip's span, and an amount of 0
    // moves none.
    let result = song.edit(
        ids::RANDOMIZE,
        json!({ "field": "velocity", "amount": 0, "seed": 7 }),
    );
    assert!(!result.undo_step);
    assert_eq!(*song.doc(), before);
    assert_eq!(song.steps(), steps);
    // One step, every note within the amount and the range, and whole.
    song.one_step(
        ids::RANDOMIZE,
        json!({ "field": "velocity", "amount": 20, "seed": 7 }),
    );
    let first: Vec<u8> = song.notes().iter().map(|note| note.velocity).collect();
    assert!(first.iter().all(|v| (80..=120).contains(v)), "{first:?}");
    assert!(first.iter().any(|v| *v != 100), "{first:?}");
    // The same seed on the same notes gives the same values.
    song.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*song.doc(), before);
    song.edit(
        ids::RANDOMIZE,
        json!({ "field": "velocity", "amount": 20, "seed": 7 }),
    );
    let again: Vec<u8> = song.notes().iter().map(|note| note.velocity).collect();
    assert_eq!(again, first);
    // A larger amount moves each note further the same way.
    song.shell.run("edit.undo", json!({})).unwrap();
    song.edit(
        ids::RANDOMIZE,
        json!({ "field": "velocity", "amount": 10, "seed": 7 }),
    );
    let half: Vec<u8> = song.notes().iter().map(|note| note.velocity).collect();
    for (half, full) in half.iter().zip(&first) {
        let (half, full) = (i32::from(*half) - 100, i32::from(*full) - 100);
        assert!(half.abs() <= full.abs() + 1, "{half} {full}");
        assert!(half * full >= 0, "{half} {full}: the same direction");
    }
    // Chance draws in its own units, held to 0 to 1.
    song.shell.run("edit.undo", json!({})).unwrap();
    song.one_step(
        ids::RANDOMIZE,
        json!({ "field": "chance", "amount": 0.5, "seed": 3 }),
    );
    let chances: Vec<f64> = song.notes().iter().map(|note| note.probability).collect();
    assert!(
        chances.iter().all(|c| (0.5..=1.0).contains(c)),
        "{chances:?}"
    );
    // Only the selected notes move when some are selected, and each call
    // without a seed draws afresh.
    song.view(ids::SELECT_NOTES, json!({ "notes": [0, 1, 2] }));
    let mid = song.doc().clone();
    song.edit(
        ids::RANDOMIZE,
        json!({ "field": "release_velocity", "amount": 30 }),
    );
    let one: Vec<u8> = song.notes().iter().map(|n| n.release_velocity).collect();
    assert!(one[3..].iter().all(|v| *v == 64), "{one:?}");
    song.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*song.doc(), mid);
    song.edit(
        ids::RANDOMIZE,
        json!({ "field": "release_velocity", "amount": 30 }),
    );
    let two: Vec<u8> = song.notes().iter().map(|n| n.release_velocity).collect();
    assert_ne!(one, two, "an unseeded call draws afresh");
    // Out of range or the wrong field changes nothing.
    let before = song.doc().clone();
    for args in [
        json!({ "field": "velocity", "amount": 128 }),
        json!({ "field": "velocity", "amount": -1 }),
        json!({ "field": "chance", "amount": 1.5 }),
        json!({ "field": "velocity_deviation", "amount": 5 }),
    ] {
        assert!(
            song.shell.run(ids::RANDOMIZE, args.clone()).is_err(),
            "{args}"
        );
        assert_eq!(*song.doc(), before, "{args}");
    }
}

// Spec: note-editing R18. Deactivating toggles, keeps each note's place and
// values, and is one step.
#[test]
fn deactivating_notes_toggles_and_is_one_step() {
    let mut song = song(vec![
        song_note(48, 0, SIXTEENTH),
        song_note(60, BAR, QUARTER),
    ]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0, 1] }));
    assert_eq!(
        song.shell.dynamic_label(ids::DEACTIVATE).as_deref(),
        Some("Deactivate Note(s)")
    );
    let result = song.one_step(ids::DEACTIVATE, json!({}));
    assert_eq!(result.selected, [0, 1]);
    assert!(song.notes().iter().all(|note| note.mute));
    assert_eq!(song.notes()[0].start, Ticks(0), "the note keeps its place");
    assert_eq!(song.notes()[0].velocity, 100, "and its values");
    assert_eq!(
        song.shell.dynamic_label(ids::DEACTIVATE).as_deref(),
        Some("Activate Note(s)"),
        "every selected note is off"
    );
    // All off: the command activates them again.
    song.one_step(ids::DEACTIVATE, json!({}));
    assert!(song.notes().iter().all(|note| !note.mute));
    // With only some off, it deactivates the rest.
    song.one_step(ids::DEACTIVATE, json!({ "notes": [0] }));
    song.view(ids::SELECT_NOTES, json!({ "notes": [0, 1] }));
    assert_eq!(
        song.shell.dynamic_label(ids::DEACTIVATE).as_deref(),
        Some("Deactivate Note(s)")
    );
    song.one_step(ids::DEACTIVATE, json!({}));
    assert!(song.notes().iter().all(|note| note.mute));
    // `active` says which way.
    song.one_step(ids::DEACTIVATE, json!({ "active": true }));
    assert!(song.notes().iter().all(|note| !note.mute));
    let steps = song.steps();
    let result = song.edit(ids::DEACTIVATE, json!({ "active": true }));
    assert!(!result.undo_step, "already active");
    assert_eq!(song.steps(), steps);
}

// Spec: note-editing R1 to R3, R13, R20. The lanes are view state: a new
// editor shows Velocity alone, and no change to them is a step.
#[test]
fn the_lanes_are_view_state_that_shows_hides_and_swaps() {
    use bl_shell::commands::{VIEW_LANE, VIEW_LANES};
    use bl_shell::note_commands::NoteField::{Chance, ReleaseVelocity, Velocity};
    let mut song = song(vec![song_note(60, 0, QUARTER)]);
    let steps = song.steps();
    let state = song.view(VIEW_LANES, json!({}));
    assert_eq!(state.lanes, [Velocity], "a new editor shows Velocity alone");
    assert_eq!(state.lane, Velocity);
    assert!(!state.lanes_hidden);
    assert_eq!(state.randomize, Default::default(), "every amount is 0");
    // The Lane Selector shows a lane and makes it the active one.
    let state = song.view(VIEW_LANES, json!({ "lane": "chance" }));
    assert_eq!(state.lanes, [Velocity, Chance], "in the lanes' order");
    assert_eq!(state.lane, Chance);
    let state = song.view(
        VIEW_LANES,
        json!({ "lane": "release_velocity", "show": true }),
    );
    assert_eq!(state.lanes, [Velocity, Chance, ReleaseVelocity]);
    // Choosing a shown lane hides it; the active lane moves on if it was.
    let state = song.view(
        VIEW_LANES,
        json!({ "lane": "release_velocity", "show": false }),
    );
    assert_eq!(state.lanes, [Velocity, Chance]);
    assert_eq!(
        state.lane, Velocity,
        "the first lane still shown takes over"
    );
    let state = song.view(VIEW_LANES, json!({ "lane": "chance" }));
    assert_eq!(state.lanes, [Velocity]);
    assert_eq!(
        state.lane, Velocity,
        "the active lane stays among those shown"
    );
    // `view.lane` shows the lane too, and velocity deviation is Velocity's.
    let state = song.view(VIEW_LANE, json!({ "lane": "release_velocity" }));
    assert_eq!(state.lanes, [Velocity, ReleaseVelocity]);
    assert_eq!(state.lane, ReleaseVelocity);
    let state = song.view(VIEW_LANE, json!({ "lane": "velocity_deviation" }));
    assert_eq!(state.lane, Velocity);
    // Swap Lane hides the lane and shows the first hidden lane after it,
    // going round to the first.
    let state = song.view(VIEW_LANES, json!({ "lane": "velocity", "swap": true }));
    assert_eq!(state.lanes, [Chance, ReleaseVelocity]);
    assert_eq!(state.lane, Chance, "the active lane's place is taken");
    let state = song.view(
        VIEW_LANES,
        json!({ "lane": "release_velocity", "swap": true }),
    );
    assert_eq!(state.lanes, [Velocity, Chance], "round to the first");
    // With every lane shown a swap changes nothing.
    song.view(
        VIEW_LANES,
        json!({ "lane": "release_velocity", "show": true }),
    );
    let state = song.view(VIEW_LANES, json!({ "lane": "chance", "swap": true }));
    assert_eq!(state.lanes, [Velocity, Chance, ReleaseVelocity]);
    // The switch hides every lane and shows them again as they were.
    let state = song.view(VIEW_LANES, json!({ "all": false }));
    assert!(state.lanes.is_empty() && state.lanes_hidden);
    let state = song.view(VIEW_LANES, json!({ "all": true }));
    assert_eq!(state.lanes, [Velocity, Chance, ReleaseVelocity]);
    // Randomize's amounts are the lane's own, held to its limit.
    let state = song.view(VIEW_LANES, json!({ "lane": "velocity", "amount": 40 }));
    assert_eq!(state.randomize.velocity, 40);
    assert_eq!(
        state.lanes.len(),
        3,
        "setting an amount doesn't hide the lane"
    );
    let state = song.view(VIEW_LANES, json!({ "lane": "chance", "amount": 25 }));
    assert_eq!(state.randomize.chance, 25);
    for args in [
        json!({ "lane": "chance", "amount": 101 }),
        json!({ "lane": "velocity", "amount": 128 }),
        json!({ "amount": 5 }),
        json!({ "show": true }),
        json!({ "nope": true }),
    ] {
        assert!(song.shell.run(VIEW_LANES, args.clone()).is_err(), "{args}");
    }
    assert_eq!(song.steps(), steps, "none of it is an undo step");
    assert!(!song.shell.is_dirty(), "and none of it dirties the set");
}

// Spec: note-editing R6. Drawing across a lane gives each note its own
// value in one step.
#[test]
fn each_note_takes_its_own_value_in_one_step() {
    let mut song = song(vec![
        song_note(48, 0, SIXTEENTH),
        song_note(60, BAR, QUARTER),
        song_note(64, 2 * BAR, QUARTER),
    ]);
    song.one_step(
        ids::SET_VALUE,
        json!({ "notes": [2, 0], "field": "velocity", "values": [10, 120] }),
    );
    let velocities: Vec<u8> = song.notes().iter().map(|note| note.velocity).collect();
    assert_eq!(
        velocities,
        [120, 100, 10],
        "in the order the notes were named"
    );
    let before = song.doc().clone();
    for args in [
        json!({ "notes": [0, 1], "field": "velocity", "values": [10] }),
        json!({ "notes": [0], "field": "velocity", "values": [200] }),
        json!({ "notes": [0], "field": "velocity", "value": 10, "values": [10] }),
        json!({ "notes": [0], "field": "velocity" }),
    ] {
        assert!(
            song.shell.run(ids::SET_VALUE, args.clone()).is_err(),
            "{args}"
        );
        assert_eq!(*song.doc(), before, "{args}");
    }
}
