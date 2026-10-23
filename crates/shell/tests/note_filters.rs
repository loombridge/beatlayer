//! Find and Select Notes (WP-6.4), driven headless through `Shell::run`, as
//! agents and the filter bar drive it. The behaviour is in the
//! find-and-select-notes spec.

use std::path::{Path, PathBuf};

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, Editor, MidiData, Note};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::note_commands::{self as ids, ClipViewState};
use bl_shell::settings::Preferences;
use bl_shell_spec::note_filters::{NoteFilter, NoteFiltersResult};
use serde_json::{Value, json};

const BEAT: i64 = Ticks::QUARTER.get();
const BAR: i64 = Ticks::WHOLE.get();

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

/// A song with a four-bar MIDI clip holding `notes`, selected, and a second
/// one holding `other`.
struct Song {
    shell: Shell,
    clip: ClipId,
    other: ClipId,
}

fn song(notes: Vec<Note>) -> Song {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let make = |editor: &mut Editor, notes: Vec<Note>| {
        let id: ClipId = editor.new_id();
        let clip = Clip::new(
            id,
            Ticks::ZERO,
            Ticks(4 * BAR),
            ClipData::Midi(MidiData::new(notes)),
        );
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        id
    };
    let clip = make(&mut editor, notes);
    let other = make(&mut editor, vec![note(60, 0)]);
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
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    Song { shell, clip, other }
}

/// A one-beat note at `beat` beats.
fn note(key: u8, beat: i64) -> Note {
    Note::new(key, Ticks(beat * BEAT), Ticks(BEAT), 100)
}

impl Song {
    fn filters(&mut self, args: Value) -> NoteFiltersResult {
        serde_json::from_value(
            self.shell
                .run(ids::NOTE_FILTERS, args)
                .unwrap_or_else(|e| panic!("note_filters failed: {e}")),
        )
        .unwrap()
    }

    fn state(&mut self) -> ClipViewState {
        let state: bl_shell::commands::ViewState =
            serde_json::from_value(self.shell.run("view.state", json!({})).unwrap()).unwrap();
        state.clip_view
    }

    fn notes(&self) -> Vec<Note> {
        match &self.shell.editor().document().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => notes_in_order(midi),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    /// The keys of the selected notes, in the clip's order.
    fn selected_keys(&self, result: &NoteFiltersResult) -> Vec<u8> {
        let notes = self.notes();
        result
            .selected
            .iter()
            .map(|p| notes[*p as usize].key)
            .collect()
    }

    /// The starts, in beats, of the selected notes.
    fn selected_beats(&self, result: &NoteFiltersResult) -> Vec<i64> {
        let notes = self.notes();
        result
            .selected
            .iter()
            .map(|p| notes[*p as usize].start.get() / BEAT)
            .collect()
    }

    /// Sets one value of the note at `place`.
    fn set(&mut self, place: u32, field: &str, value: f64) {
        self.shell
            .run(
                "note.set_value",
                json!({ "notes": [place], "field": field, "value": value }),
            )
            .unwrap();
    }
}

/// Eight one-beat notes on beats 0 to 7, keys 60 to 67.
fn eight() -> Song {
    song((0..8).map(|beat| note(60 + beat as u8, beat)).collect())
}

#[test]
fn pitch_lets_through_a_pitch_class_in_every_octave() {
    // Spec: find-and-select-notes R5, R4. C in any octave; none on lets
    // none through; Invert holds back what it would let through.
    let mut song = song(vec![note(48, 0), note(60, 1), note(61, 2), note(72, 3)]);
    let result = song.filters(json!({ "pitch": { "classes": [0] } }));
    assert_eq!(song.selected_keys(&result), [48, 60, 72]);
    assert!(result.filters.pitch.on);
    let result = song.filters(json!({ "pitch": { "invert": true } }));
    assert_eq!(song.selected_keys(&result), [61]);
    let result = song.filters(json!({ "pitch": { "classes": [], "invert": false } }));
    assert!(
        result.selected.is_empty(),
        "no pitch class on lets none through"
    );
}

#[test]
fn time_selects_a_window_that_may_repeat() {
    // Spec: find-and-select-notes R6. Start counts from 1; Repeat 0 is one
    // window; a new editor's Repeat is one bar.
    let mut song = eight();
    let result = song.filters(json!({ "time": { "start": 2, "length": 2, "repeat": 0 } }));
    assert_eq!(song.selected_beats(&result), [1, 2], "one window");
    let result = song.filters(json!({ "time": { "repeat": 4 } }));
    assert_eq!(song.selected_beats(&result), [1, 2, 5, 6], "repeating");
    let result = song.filters(json!({ "time": { "start": 1, "length": 1, "repeat": null } }));
    assert_eq!(song.selected_beats(&result), [0, 4], "one bar by default");
    assert_eq!(result.filters.time.repeat, None);
    let result = song.filters(json!({ "time": { "start": 1.5, "length": 1.5, "repeat": 0 } }));
    assert_eq!(song.selected_beats(&result), [1], "from half a beat to two");
}

#[test]
fn chance_and_velocity_select_their_bounds_and_move_each_other() {
    // Spec: find-and-select-notes R7.
    let mut song = eight();
    for (place, chance) in [0.0, 0.25, 0.5, 0.75].into_iter().enumerate() {
        song.set(place as u32, "chance", chance);
    }
    for place in 0..8 {
        song.set(place, "velocity", f64::from(15 * (place + 1)));
    }
    let result = song.filters(json!({ "chance": { "min": 25, "max": 75 } }));
    assert_eq!(
        song.selected_beats(&result),
        [1, 2, 3],
        "bounds are inclusive"
    );
    let result = song.filters(json!({ "chance": { "min": 90 } }));
    assert_eq!(result.filters.chance.max, 90.0, "Min past Max takes Max");
    assert!(
        result.selected.is_empty(),
        "no note has a chance of exactly 90"
    );
    let result = song.filters(json!({ "chance": { "max": 100 } }));
    assert_eq!(song.selected_beats(&result), [4, 5, 6, 7]);
    let result = song.filters(json!({ "velocity": { "min": 60, "max": 100 } }));
    assert_eq!(
        song.selected_beats(&result),
        [4, 5],
        "velocities 60 to 100 are on beats 3 to 5, and beat 3 has a chance below 90"
    );
    let result = song.filters(json!({ "velocity": { "max": 10 } }));
    assert_eq!(result.filters.velocity.min, 10, "Max past Min takes Min");
}

#[test]
fn condition_needs_every_switch_that_is_on() {
    // Spec: find-and-select-notes R8, note-editing R7, R18.
    let mut song = eight();
    song.shell
        .run("note.deactivate", json!({ "notes": [0], "active": false }))
        .unwrap();
    song.set(1, "chance", 0.5);
    song.set(2, "velocity_deviation", 10.0);
    song.set(3, "chance", 0.5);
    song.set(3, "velocity_deviation", 10.0);
    let result = song.filters(json!({ "condition": { "active": true } }));
    assert_eq!(song.selected_beats(&result), [1, 2, 3, 4, 5, 6, 7]);
    let result = song.filters(json!({ "condition": { "active": false, "chance": true } }));
    assert_eq!(song.selected_beats(&result), [1, 3]);
    let result =
        song.filters(json!({ "condition": { "chance": false, "velocity_deviation": true } }));
    assert_eq!(song.selected_beats(&result), [2, 3]);
    let result = song.filters(json!({ "condition": { "chance": true } }));
    assert_eq!(song.selected_beats(&result), [3], "two together");
    let result = song.filters(json!({
        "condition": { "chance": false, "velocity_deviation": false, "invert": false }
    }));
    assert_eq!(result.selected.len(), 8, "with none on, every note passes");
}

#[test]
fn count_counts_notes_or_grid_steps() {
    // Spec: find-and-select-notes R9. A chord counts once; with Quantized
    // the grid's steps count, empty ones included.
    let mut song = song(vec![note(60, 0), note(64, 0), note(67, 2), note(72, 3)]);
    let result = song.filters(json!({ "count": { "every": 2 } }));
    assert_eq!(
        song.selected_beats(&result),
        [0, 0, 3],
        "the chord is note 0, the note on beat 2 is 1, the one on 3 is 2"
    );
    let result = song.filters(json!({ "count": { "offset": 1 } }));
    assert_eq!(song.selected_beats(&result), [2]);
    song.shell
        .run("arrangement.set_grid", json!({ "fixed": "quarter" }))
        .unwrap();
    let result = song.filters(json!({ "count": { "every": 2, "offset": 0, "quantized": true } }));
    assert_eq!(
        song.selected_beats(&result),
        [0, 0, 2],
        "steps 0 and 2; the empty step 1 counts"
    );
    let result = song.filters(json!({ "count": { "every": 3, "offset": 2, "quantized": true } }));
    assert_eq!(
        song.selected_beats(&result),
        [2],
        "step 2 is the first counted"
    );
}

#[test]
fn duration_selects_lengths_in_beats() {
    // Spec: find-and-select-notes R10.
    let mut song = song(vec![
        Note::new(60, Ticks::ZERO, Ticks(BEAT / 2), 100),
        Note::new(62, Ticks(BEAT), Ticks(BEAT), 100),
        Note::new(64, Ticks(2 * BEAT), Ticks(2 * BEAT), 100),
    ]);
    let result = song.filters(json!({ "duration": { "min": 1, "max": 1.5 } }));
    assert_eq!(song.selected_keys(&result), [62]);
    let result = song.filters(json!({ "duration": { "min": 0.5, "max": 2 } }));
    assert_eq!(song.selected_keys(&result), [60, 62, 64]);
    let result = song.filters(json!({ "duration": { "invert": true, "min": 0.75 } }));
    assert_eq!(song.selected_keys(&result), [60]);
    assert_eq!(result.filters.duration.max, Some(2.0));
    assert!(
        song.shell
            .run(ids::NOTE_FILTERS, json!({ "duration": { "max": 17 } }))
            .is_err(),
        "a clip of four bars is sixteen beats long"
    );
}

#[test]
fn scale_reads_the_clip_scale_or_its_own() {
    // Spec: find-and-select-notes R11. With Use Clip Scale on the filter
    // works whether or not the clip's Scale Mode is on.
    let keys = [60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71];
    let mut song = song(
        keys.iter()
            .enumerate()
            .map(|(i, k)| note(*k, i as i64))
            .collect(),
    );
    song.shell
        .run(
            "clip.set_scale",
            json!({ "clips": [song.clip], "enabled": false, "root": 2, "name": "dorian" }),
        )
        .unwrap();
    // D Dorian: D E F G A B C.
    let result = song.filters(json!({ "scale": {} }));
    assert_eq!(song.selected_keys(&result), [60, 62, 64, 65, 67, 69, 71]);
    song.shell
        .run(
            "clip.set_scale",
            json!({ "clips": [song.clip], "enabled": true }),
        )
        .unwrap();
    let result = song.filters(json!({ "scale": { "invert": false } }));
    assert_eq!(song.selected_keys(&result), [60, 62, 64, 65, 67, 69, 71]);
    let result = song.filters(json!({
        "scale": { "use_clip_scale": false, "root": 0, "name": "minor_pentatonic" }
    }));
    assert_eq!(song.selected_keys(&result), [60, 63, 65, 67, 70]);
    let result = song.filters(json!({ "scale": { "invert": true } }));
    assert_eq!(song.selected_keys(&result), [61, 62, 64, 66, 68, 69, 71]);
}

#[test]
fn invert_holds_back_exactly_what_each_filter_lets_through() {
    // Spec: find-and-select-notes R4. For each filter, the selection with
    // Invert on and the selection with it off split the notes in the clip's
    // span between them.
    let mut song = eight();
    song.set(1, "chance", 0.5);
    song.set(2, "velocity", 40.0);
    song.set(3, "velocity_deviation", 10.0);
    song.shell
        .run("note.deactivate", json!({ "notes": [4], "active": false }))
        .unwrap();
    for args in [
        json!({ "pitch": { "classes": [0, 4, 7] } }),
        json!({ "time": { "start": 2, "length": 3, "repeat": 0 } }),
        json!({ "chance": { "min": 0, "max": 60 } }),
        json!({ "condition": { "active": true } }),
        json!({ "count": { "every": 3, "offset": 1 } }),
        json!({ "duration": { "min": 0, "max": 0.5 } }),
        json!({ "scale": { "use_clip_scale": false, "root": 0, "name": "major" } }),
        json!({ "velocity": { "min": 50 } }),
    ] {
        let filter = args.as_object().unwrap().keys().next().unwrap().clone();
        let direct = song.filters(args.clone());
        let mut flip = args.clone();
        flip[&filter]["invert"] = json!(true);
        let flipped = song.filters(flip);
        let mut both: Vec<u32> = direct
            .selected
            .iter()
            .chain(&flipped.selected)
            .copied()
            .collect();
        both.sort_unstable();
        assert_eq!(both, (0..8).collect::<Vec<u32>>(), "{filter}");
        assert!(
            direct
                .selected
                .iter()
                .all(|p| !flipped.selected.contains(p)),
            "{filter}: nothing passes both ways"
        );
        song.filters(json!({ "off": NoteFilter::ALL }));
    }
}

#[test]
fn filters_combine_and_each_keeps_its_own_invert() {
    // Spec: find-and-select-notes R3, R4.
    let mut song = eight();
    for place in (1..8).step_by(2) {
        song.set(place, "velocity", 30.0);
    }
    song.filters(json!({ "velocity": { "min": 50 } }));
    // Keys 60, 62, 64 and 66 have velocity 100; of them C, D and E pass the
    // pitch classes.
    let result = song.filters(json!({ "pitch": { "classes": [0, 2, 4, 5, 7] } }));
    assert_eq!(song.selected_keys(&result), [60, 62, 64]);
    let result = song.filters(json!({ "velocity": { "invert": true } }));
    assert_eq!(
        song.selected_keys(&result),
        [65, 67],
        "F and G of the quiet notes"
    );
}

#[test]
fn choosing_a_filter_keeps_the_selection_and_off_keeps_the_settings() {
    // Spec: find-and-select-notes R2, R3. A click switches every filter
    // off and Select switches one back on.
    let mut song = eight();
    let result = song.filters(json!({ "pitch": { "classes": [0] } }));
    assert_eq!(song.selected_keys(&result), [60]);
    let result = song.filters(json!({ "shown": "scale" }));
    assert_eq!(result.filters.shown, NoteFilter::Scale);
    assert!(result.filters.pitch.on, "Pitch stays on");
    assert_eq!(song.selected_keys(&result), [60], "and the selection stays");
    let all = NoteFilter::ALL.to_vec();
    let result = song.filters(json!({ "off": all }));
    assert!(!result.filters.any_on());
    assert_eq!(result.filters.pitch.classes, [0], "the settings stay");
    song.shell
        .run(ids::SELECT_NOTES, json!({ "notes": [] }))
        .unwrap();
    let result = song.filters(json!({ "select": ["pitch"] }));
    assert!(result.filters.pitch.on);
    assert_eq!(song.selected_keys(&result), [60], "Select selects again");
}

#[test]
fn the_time_selection_follows_the_filters_and_nothing_is_an_undo_step() {
    // Spec: find-and-select-notes R3, piano-roll R24.
    let mut song = eight();
    let steps = song.shell.editor().undo_history().len();
    song.filters(json!({ "pitch": { "classes": [4, 7] } }));
    let state = song.state();
    assert_eq!(state.selected, [4, 7]);
    assert_eq!(
        state.time_selection,
        Some((Ticks(4 * BEAT), Ticks(8 * BEAT))),
        "from the earliest start to the latest end"
    );
    song.filters(json!({ "pitch": { "classes": [] } }));
    assert_eq!(
        song.state().time_selection,
        None,
        "an empty selection has none"
    );
    assert_eq!(song.shell.editor().undo_history().len(), steps);
    assert!(!song.shell.is_dirty());
}

#[test]
fn another_clip_switches_every_filter_off() {
    // Spec: find-and-select-notes R12. The settings stay.
    let mut song = eight();
    song.filters(json!({ "pitch": { "classes": [0] }, "velocity": { "min": 5 }, "visible": true }));
    song.shell
        .run("arrangement.select_clips", json!({ "clips": [song.other] }))
        .unwrap();
    let state = song.state();
    assert!(!state.filters.any_on());
    assert_eq!(state.filters.pitch.classes, [0]);
    assert!(state.filters.visible, "the bar stays shown");
    song.shell
        .run("arrangement.select_clips", json!({ "clips": [song.clip] }))
        .unwrap();
    assert!(!song.state().filters.any_on(), "and stay off");
}

#[test]
fn the_bar_is_view_state_and_agents_read_the_filters_back() {
    // Spec: find-and-select-notes R1, R14.
    let mut song = eight();
    let result = song.filters(json!({ "toggle": true }));
    assert!(result.filters.visible);
    let result = song.filters(json!({ "visible": true, "toggle": true }));
    assert!(result.filters.visible, "`visible` wins");
    let result = song.filters(json!({ "visible": false }));
    assert!(!result.filters.visible);
    let set = song.filters(json!({
        "visible": true,
        "shown": "count",
        "count": { "every": 4, "offset": 1 },
    }));
    assert_eq!(song.state().filters, set.filters);
    assert_eq!(set.filters.count.every, 4);
    assert_eq!(
        song.filters(json!({})).filters,
        set.filters,
        "{{}} only reads"
    );
}

#[test]
fn a_control_out_of_range_changes_nothing() {
    // Spec: find-and-select-notes R5 to R11.
    let mut song = eight();
    let before = song.state();
    for args in [
        json!({ "pitch": { "classes": [12] } }),
        json!({ "time": { "start": 0 } }),
        json!({ "time": { "length": 0 } }),
        json!({ "time": { "repeat": -1 } }),
        json!({ "chance": { "max": 101 } }),
        json!({ "chance": { "min": 60, "max": 40 } }),
        json!({ "count": { "every": 17 } }),
        json!({ "count": { "every": 4, "offset": 4 } }),
        json!({ "velocity": { "min": 0 } }),
        json!({ "velocity": { "max": 128 } }),
        json!({ "scale": { "root": 12 } }),
    ] {
        assert!(
            song.shell.run(ids::NOTE_FILTERS, args.clone()).is_err(),
            "{args}"
        );
        assert_eq!(song.state(), before, "{args}");
    }
}

#[test]
fn invert_selection_swaps_the_selection_and_the_time_selection_follows() {
    // Spec: find-and-select-notes R13.
    let mut song = eight();
    let steps = song.shell.editor().undo_history().len();
    song.shell
        .run(ids::SELECT_NOTES, json!({ "notes": [0, 1, 2] }))
        .unwrap();
    let state: ClipViewState =
        serde_json::from_value(song.shell.run(ids::INVERT_SELECTION, json!({})).unwrap()).unwrap();
    assert_eq!(state.selected, [3, 4, 5, 6, 7]);
    assert_eq!(
        state.time_selection,
        Some((Ticks(3 * BEAT), Ticks(8 * BEAT)))
    );
    let state: ClipViewState =
        serde_json::from_value(song.shell.run(ids::INVERT_SELECTION, json!({})).unwrap()).unwrap();
    assert_eq!(state.selected, [0, 1, 2], "and back");
    assert_eq!(state.time_selection, Some((Ticks::ZERO, Ticks(3 * BEAT))));
    song.shell
        .run(ids::SELECT_NOTES, json!({ "notes": [] }))
        .unwrap();
    let state: ClipViewState =
        serde_json::from_value(song.shell.run(ids::INVERT_SELECTION, json!({})).unwrap()).unwrap();
    assert_eq!(
        state.selected.len(),
        8,
        "nothing selected selects every note"
    );
    assert_eq!(song.shell.editor().undo_history().len(), steps);
    assert!(!song.shell.is_dirty());
}

#[test]
fn invert_selection_needs_a_clip_that_holds_notes() {
    // Spec: find-and-select-notes R13, the menu's enabled state.
    let mut song = song(Vec::new());
    let active = [Context::NoteEditor];
    assert!(
        !song
            .shell
            .can_invoke_in(ids::INVERT_SELECTION, &json!({}), &active)
    );
    assert!(song.shell.run(ids::INVERT_SELECTION, json!({})).is_err());
    let full = eight();
    assert!(
        full.shell
            .can_invoke_in(ids::INVERT_SELECTION, &json!({}), &active)
    );
    assert!(
        !full
            .shell
            .can_invoke_in(ids::INVERT_SELECTION, &json!({}), &[Context::Global]),
        "only with the note editor focused"
    );
}
