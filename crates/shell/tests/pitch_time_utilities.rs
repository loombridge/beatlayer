//! The Pitch and Time Utilities, driven headless through `Shell::run` and
//! `Shell::gesture`, as agents and the panel's controls drive them (WP-6.16;
//! the pitch and time utilities spec, R1 to R15).

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    Chain, Clip, ClipData, ClipId, Command, Document, Editor, MidiData, Note, RackKind, Scale,
    ScaleName, TrackId,
};
use bl_shell::audio::AudioHost;
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::midi_tool_commands::{self as tool_ids, MidiToolState};
use bl_shell::note_commands::{self as ids, NoteEditResult, UtilitiesState, duration_length};
use bl_shell::settings::Preferences;
use bl_shell::{GesturePhase, Invocation, Shell};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();
const QUARTER: i64 = Ticks::QUARTER.get();

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

fn note(key: u8, start: i64, length: i64) -> Note {
    Note::new(key, Ticks(start), Ticks(length), 100)
}

/// A song with a MIDI clip of four bars on the first MIDI track, selected as
/// a click on it leaves it.
struct Song {
    shell: Shell,
    clip: ClipId,
}

fn new_song(notes: Vec<Note>) -> Song {
    song_on(MidiData::new(notes), Scale::default(), |_, _| {})
}

fn song_in(notes: Vec<Note>, scale: Scale) -> Song {
    song_on(MidiData::new(notes), scale, |_, _| {})
}

/// A song whose first MIDI track `setup` may fit out before the set opens.
fn song_on(midi: MidiData, scale: Scale, setup: impl FnOnce(&mut Editor, TrackId)) -> Song {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    setup(&mut editor, track);
    let clip = editor.new_id();
    let mut value = Clip::new(clip, Ticks::ZERO, Ticks(4 * BAR), ClipData::Midi(midi));
    value.scale = scale;
    editor
        .apply(Command::CreateClip { track, clip: value })
        .unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Utilities.blset" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    Song { shell, clip }
}

fn major() -> Scale {
    Scale::new(0, ScaleName::Major)
}

/// A Drum Rack with pads on keys 36, 38 and 42.
fn pads(editor: &mut Editor, track: TrackId) {
    let rack = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: bl_model::Device::rack(rack, RackKind::Drum),
        })
        .unwrap();
    for key in [36u8, 38, 42] {
        let chain = editor.new_id();
        editor
            .apply(Command::AddChain {
                rack,
                index: 0,
                chain: Chain::for_pad(chain, "Pad", key),
            })
            .unwrap();
    }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn midi(&self) -> MidiData {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => midi.clone(),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn notes(&self) -> Vec<Note> {
        notes_in_order(&self.midi())
    }

    fn keys(&self) -> Vec<u8> {
        self.notes().iter().map(|note| note.key).collect()
    }

    fn starts(&self) -> Vec<i64> {
        self.notes().iter().map(|note| note.start.get()).collect()
    }

    fn lengths(&self) -> Vec<i64> {
        self.notes().iter().map(|note| note.length.get()).collect()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    fn last_step(&self) -> Option<String> {
        self.shell.editor().undo_history().next().map(str::to_owned)
    }

    fn grid(&self) -> i64 {
        self.shell.arrangement().grid_ticks(self.doc()).get()
    }

    fn edit(&mut self, id: &str, args: Value) -> NoteEditResult {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }

    /// Runs an edit that changes the notes: it is exactly one undo step named
    /// `name`, and undo and redo restore the set exactly.
    fn one_step(&mut self, id: &str, args: Value, name: &str) -> NoteEditResult {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self.edit(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert!(result.undo_step, "{id}");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        assert_eq!(self.last_step().as_deref(), Some(name));
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }

    /// Runs an edit that would change nothing: it records no step.
    fn no_step(&mut self, id: &str, args: Value) {
        let (before, steps) = (self.doc().clone(), self.steps());
        let result = self.edit(id, args);
        assert!(!result.undo_step, "{id} recorded a step");
        assert_eq!((self.doc().clone(), self.steps()), (before, steps), "{id}");
    }

    fn refused(&mut self, id: &str, args: Value) -> String {
        let (before, steps) = (self.doc().clone(), self.steps());
        let message = self
            .shell
            .run(id, args)
            .expect_err(&format!("{id} should be refused"))
            .to_string();
        assert_eq!((self.doc().clone(), self.steps()), (before, steps), "{id}");
        message
    }

    fn select_notes(&mut self, places: &[u32]) {
        self.shell
            .run("clip_view.select_notes", json!({ "notes": places }))
            .unwrap();
    }

    fn select_time(&mut self, start: i64, end: i64) {
        self.shell
            .run(
                "clip_view.select_time",
                json!({ "start": start, "end": end }),
            )
            .unwrap();
    }

    fn time(&self) -> Option<(i64, i64)> {
        self.shell
            .clip_view()
            .time_selection(self.doc(), self.shell.arrangement())
            .map(|(from, to)| (from.get(), to.get()))
    }

    fn utilities(&mut self, args: Value) -> UtilitiesState {
        serde_json::from_value(
            self.shell
                .run(ids::UTILITIES, args)
                .unwrap_or_else(|e| panic!("{} failed: {e}", ids::UTILITIES)),
        )
        .unwrap()
    }

    fn frame(&mut self, id: &str, args: Value) {
        self.shell
            .gesture(GesturePhase::Update, Some(&Invocation::new(id, args)))
            .unwrap();
    }
}

fn three() -> Vec<Note> {
    vec![
        note(60, 0, QUARTER),
        note(64, QUARTER, QUARTER),
        note(67, 2 * QUARTER, QUARTER),
    ]
}

#[test]
fn r2_the_selection_comes_first_then_the_time_selection_then_every_note() {
    // Spec: pitch-and-time-utilities R2, R3. The notes a utility acts on are
    // the selected ones; with none, those that start in the time selection;
    // with neither, a button acts on every note, and Transpose on none.
    let mut song = new_song(vec![
        note(60, 0, QUARTER),
        note(61, QUARTER, QUARTER),
        note(62, 2 * QUARTER, QUARTER),
        note(63, 3 * QUARTER, QUARTER),
    ]);
    let message = song.refused(ids::TRANSPOSE, json!({ "by": 1 }));
    assert!(message.contains("no notes are selected"), "{message}");
    song.select_notes(&[3]);
    song.select_time(0, 3 * QUARTER);
    let result = song.one_step(ids::TRANSPOSE, json!({ "by": 12 }), "Transpose");
    assert_eq!(song.keys(), [60, 61, 62, 75], "the selection, not the time");
    assert_eq!(result.selected, [3], "the notes it acted on stay selected");
    song.select_notes(&[]);
    song.one_step(ids::TRANSPOSE, json!({ "by": 12 }), "Transpose");
    assert_eq!(
        song.keys(),
        [72, 73, 74, 75],
        "the notes in the time selection"
    );
    // Notes named in the arguments stand in for both.
    song.one_step(
        ids::TRANSPOSE,
        json!({ "notes": [0], "by": -12 }),
        "Transpose",
    );
    assert_eq!(song.keys(), [60, 73, 74, 75]);
}

#[test]
fn r4_transpose_counts_steps_of_the_scale_or_semitones() {
    // Spec: pitch-and-time-utilities R2, R4. A step is a degree of the clip's
    // scale in Scale Mode, a semitone otherwise; a note off the scale keeps
    // its distance above the scale note below it.
    let mut song = song_in(
        vec![
            note(60, 0, QUARTER),
            note(64, QUARTER, QUARTER),
            note(66, 2 * QUARTER, QUARTER),
            note(72, 3 * QUARTER, QUARTER),
        ],
        major(),
    );
    song.select_notes(&[0, 1, 2]);
    let result = song.one_step(ids::TRANSPOSE, json!({ "by": 2 }), "Transpose");
    assert_eq!(song.keys(), [64, 67, 70, 72]);
    assert_eq!(result.selected, [0, 1, 2]);
    song.one_step(
        ids::TRANSPOSE,
        json!({ "by": 1, "in": "semitones" }),
        "Transpose",
    );
    assert_eq!(song.keys(), [65, 68, 71, 72]);
    // It would move a note off the keys: refused, and nothing changes.
    let message = song.refused(ids::TRANSPOSE, json!({ "by": 100, "in": "semitones" }));
    assert!(message.contains("0 to 127"), "{message}");
    song.refused(ids::TRANSPOSE, json!({ "by": -66, "in": "semitones" }));
    // Moving nothing records no step.
    song.no_step(ids::TRANSPOSE, json!({ "by": 0 }));
}

#[test]
fn r4_on_a_drum_rack_a_step_is_the_next_key_a_pad_receives() {
    // Spec: pitch-and-time-utilities R2; drum-racks R1.
    let mut song = song_on(
        MidiData::new(vec![note(36, 0, QUARTER), note(38, QUARTER, QUARTER)]),
        Scale::default(),
        pads,
    );
    song.select_notes(&[0, 1]);
    song.refused(ids::TRANSPOSE, json!({ "by": 2 }));
    song.one_step(
        ids::TRANSPOSE,
        json!({ "notes": [0], "by": 2 }),
        "Transpose",
    );
    assert_eq!(song.keys(), [42, 38], "36 is two pads up at 42");
}

#[test]
fn r5_fit_to_scale_moves_notes_to_the_nearest_key_and_ties_go_lower() {
    // Spec: pitch-and-time-utilities R5. Observed in Live: every note
    // half-way between two scale notes went down.
    let keys = [60u8, 61, 63, 66, 68, 70, 71];
    let notes = keys
        .iter()
        .enumerate()
        .map(|(at, key)| note(*key, at as i64 * QUARTER, QUARTER))
        .collect();
    let mut song = song_in(notes, major());
    let result = song.one_step(ids::FIT_TO_SCALE, json!({}), "Fit to Scale");
    assert_eq!(song.keys(), [60, 60, 62, 65, 67, 69, 71]);
    assert_eq!(result.notes, [1, 2, 3, 4, 5], "the notes it moved");
    // Everything is on the scale now: nothing moves, no step.
    song.no_step(ids::FIT_TO_SCALE, json!({}));
    // Off, the command is refused and says why.
    let mut plain = song_in(vec![note(61, 0, QUARTER)], Scale::default());
    let message = plain.refused(ids::FIT_TO_SCALE, json!({}));
    assert!(message.contains("Scale Mode"), "{message}");
    assert!(!plain.utilities(json!({})).fit_to_scale);
}

#[test]
fn r6_invert_swaps_the_highest_and_the_lowest_and_flips_the_rest() {
    // Spec: pitch-and-time-utilities R6.
    let mut song = new_song(three());
    song.one_step(ids::INVERT, json!({}), "Invert");
    assert_eq!(song.keys(), [67, 63, 60]);
    // One note, or notes at one key, change nothing.
    let mut one = song_in(vec![note(60, 0, QUARTER)], Scale::default());
    one.no_step(ids::INVERT, json!({}));
    // With a scale, by its degrees; a note off the scale still mirrors by
    // semitones.
    let mut scaled = song_in(
        vec![
            note(60, 0, QUARTER),
            note(62, QUARTER, QUARTER),
            note(66, 2 * QUARTER, QUARTER),
            note(67, 3 * QUARTER, QUARTER),
        ],
        major(),
    );
    scaled.one_step(ids::INVERT, json!({}), "Invert");
    assert_eq!(scaled.keys(), [67, 65, 61, 60]);
    // When the highest note is off the scale, every note moves by semitones.
    let mut off = song_in(
        vec![
            note(60, 0, QUARTER),
            note(62, QUARTER, QUARTER),
            note(66, 2 * QUARTER, QUARTER),
        ],
        major(),
    );
    off.one_step(ids::INVERT, json!({}), "Invert");
    assert_eq!(off.keys(), [66, 64, 60]);
    // The selected notes only: the range is theirs.
    let mut part = song_in(three(), Scale::default());
    part.select_notes(&[0, 1]);
    part.one_step(ids::INVERT, json!({}), "Invert");
    assert_eq!(part.keys(), [64, 60, 67]);
}

#[test]
fn r7_add_interval_adds_selected_copies_and_leaves_out_those_off_the_keys() {
    // Spec: pitch-and-time-utilities R7. The copies keep their notes' starts,
    // lengths and values, and are the notes selected afterwards.
    let mut song = song_in(
        vec![
            Note::new(60, Ticks::ZERO, Ticks(QUARTER), 90),
            Note::new(64, Ticks(QUARTER), Ticks(QUARTER / 2), 50),
        ],
        major(),
    );
    let result = song.one_step(ids::ADD_INTERVAL, json!({ "by": 2 }), "Interval");
    assert_eq!(song.keys(), [60, 64, 64, 67], "two scale steps up");
    assert_eq!(result.notes, [1, 3]);
    assert_eq!(result.selected, [1, 3], "the copies are selected");
    let notes = song.notes();
    assert_eq!(
        (
            notes[3].start.get(),
            notes[3].length.get(),
            notes[3].velocity
        ),
        (QUARTER, QUARTER / 2, 50)
    );
    // With no interval named, the panel's Interval Size; with notes selected
    // only theirs are copied.
    song.select_notes(&[0]);
    song.utilities(json!({ "interval": 3 }));
    song.one_step(ids::ADD_INTERVAL, json!({}), "Interval");
    assert_eq!(song.keys(), [60, 64, 65, 64, 67], "60 and three steps up");
    // A copy past key 127 is left out.
    let mut high = new_song(vec![note(126, 0, QUARTER), note(60, QUARTER, QUARTER)]);
    let result = high.one_step(
        ids::ADD_INTERVAL,
        json!({ "by": 3, "in": "semitones" }),
        "Interval",
    );
    assert_eq!(
        high.keys(),
        [126, 60, 63],
        "only the note that fits is copied"
    );
    assert_eq!(result.selected.len(), 1);
    // Nothing added, no step.
    high.no_step(
        ids::ADD_INTERVAL,
        json!({ "notes": [0], "by": 12, "in": "semitones" }),
    );
    let message = high.refused(ids::ADD_INTERVAL, json!({ "by": 37 }));
    assert!(message.contains("-36 to 36"), "{message}");
}

#[test]
fn r7_on_a_drum_rack_a_copy_on_a_key_no_pad_receives_is_left_out() {
    // Spec: pitch-and-time-utilities R7; drum-racks R1.
    let mut song = song_on(
        MidiData::new(vec![note(36, 0, QUARTER), note(38, QUARTER, QUARTER)]),
        Scale::default(),
        pads,
    );
    // By pads: 36 goes to 42, and 38 has none two pads on.
    song.one_step(ids::ADD_INTERVAL, json!({ "by": 2 }), "Interval");
    assert_eq!(song.keys(), [36, 42, 38]);
    // By semitones: 37 and 39 are no pad's key.
    song.no_step(ids::ADD_INTERVAL, json!({ "by": 1, "in": "semitones" }));
}

#[test]
fn r8_r9_stretch_scales_around_the_time_selections_start_and_it_follows() {
    // Spec: pitch-and-time-utilities R3, R8. Each note's distance from the
    // time selection's start and its length are multiplied by the factor, and
    // the time selection stretches with them.
    let mut song = new_song(vec![
        note(60, QUARTER, QUARTER),
        note(64, 3 * QUARTER, QUARTER),
        note(67, 8 * QUARTER, QUARTER),
    ]);
    song.select_notes(&[0, 1]);
    song.select_time(QUARTER, 4 * QUARTER);
    let result = song.one_step(ids::STRETCH, json!({ "factor": 3.0 }), "Stretch");
    assert_eq!(song.starts(), [QUARTER, 7 * QUARTER, 8 * QUARTER]);
    assert_eq!(song.lengths(), [3 * QUARTER, 3 * QUARTER, QUARTER]);
    assert_eq!(song.time(), Some((QUARTER, 10 * QUARTER)));
    assert_eq!(result.selected, [0, 1]);
    // The undo step is named for the buttons' factors.
    song.one_step(
        ids::STRETCH,
        json!({ "factor": 2.0 }),
        "Stretch Time by Double",
    );
    song.one_step(
        ids::STRETCH,
        json!({ "factor": 0.5 }),
        "Shrink Time by Half",
    );
    // Around a point named, in place of the time selection's start.
    song.select_time(0, 12 * QUARTER);
    song.one_step(
        ids::STRETCH,
        json!({ "factor": 0.5, "point": { "from": QUARTER, "to": QUARTER } }),
        "Shrink Time by Half",
    );
    assert_eq!(song.starts()[0], QUARTER, "the point stays where it is");
    // Stretched notes that would start before the contents refuse the edit.
    let message = song.refused(
        ids::STRETCH,
        json!({ "factor": 2.0, "point": { "from": 5 * QUARTER, "to": 5 * QUARTER } }),
    );
    assert!(message.contains("before the clip's contents"), "{message}");
    // With no time selection, around the first selected note.
    let mut bare = song_in(
        vec![note(60, QUARTER, QUARTER), note(62, 2 * QUARTER, QUARTER)],
        Scale::default(),
    );
    bare.select_notes(&[0, 1]);
    bare.one_step(
        ids::STRETCH,
        json!({ "factor": 2.0 }),
        "Stretch Time by Double",
    );
    assert_eq!(bare.starts(), [QUARTER, 3 * QUARTER]);
    assert_eq!(
        bare.time(),
        Some((QUARTER, 5 * QUARTER)),
        "the time selection follows the notes"
    );
}

#[test]
fn r8_stretch_takes_a_range_or_a_factor_and_a_factor_from_a_tenth_to_ten() {
    // Spec: pitch-and-time-utilities R8, R15; the maintainer's answer on
    // `note.stretch`: exactly one of `to` and `factor`.
    let mut song = new_song(three());
    let both = song.refused(
        ids::STRETCH,
        json!({ "to": { "start": 0, "end": QUARTER }, "factor": 2.0 }),
    );
    assert!(both.contains("`to` or `factor`"), "{both}");
    let neither = song.refused(ids::STRETCH, json!({}));
    assert!(neither.contains("`to` or `factor`"), "{neither}");
    for factor in [0.09, 10.5] {
        let message = song.refused(ids::STRETCH, json!({ "factor": factor }));
        assert!(message.contains("0.1 to 10"), "{message}");
    }
    song.one_step(ids::STRETCH, json!({ "factor": 10.0 }), "Stretch");
    assert_eq!(song.starts(), [0, 10 * QUARTER, 20 * QUARTER]);
    // A `from` stands in for the time selection: the notes that start in it,
    // around its start, and the loop and markers stay.
    song.select_notes(&[]);
    song.shell.run("clip_view.select_time", json!({})).unwrap();
    let before = song.notes();
    song.one_step(
        ids::STRETCH,
        json!({ "factor": 2.0, "from": { "start": 10 * QUARTER, "end": 25 * QUARTER } }),
        "Stretch Time by Double",
    );
    let after = song.notes();
    assert_eq!(after[0], before[0], "the first note starts before `from`");
    assert_eq!(
        after[1].start,
        Ticks(10 * QUARTER) + (before[1].start - Ticks(10 * QUARTER)) * 2
    );
    song.shell.run("edit.undo", json!({})).unwrap();
    // By a range, the step keeps its name.
    song.select_time(0, 20 * QUARTER);
    song.one_step(
        ids::STRETCH,
        json!({ "to": { "start": 0, "end": 10 * QUARTER } }),
        "Stretch Notes",
    );
}

#[test]
fn r9_double_and_halve_with_nothing_selected_stretch_the_markers_and_loop_too() {
    // Spec: pitch-and-time-utilities R9. Every note, the Start and End
    // markers and the loop brace, around the contents' start; the clip's own
    // length stays.
    let mut midi = MidiData::new(vec![note(60, 0, QUARTER), note(72, BAR, QUARTER)]);
    midi.looping = true;
    midi.loop_start = Ticks(BAR / 2);
    midi.loop_end = Some(Ticks(BAR));
    midi.start = Ticks(QUARTER);
    midi.end = Some(Ticks(2 * BAR));
    let mut song = song_on(midi, Scale::default(), |_, _| {});
    let length = song.doc().clip(song.clip).unwrap().length;
    song.one_step(
        ids::STRETCH,
        json!({ "factor": 2.0, "markers": true }),
        "Stretch Time by Double",
    );
    assert_eq!(song.starts(), [0, 2 * BAR]);
    assert_eq!(song.lengths(), [2 * QUARTER, 2 * QUARTER]);
    let midi = song.midi();
    assert_eq!(
        (midi.loop_start, midi.loop_end, midi.start, midi.end),
        (
            Ticks(BAR),
            Some(Ticks(2 * BAR)),
            Ticks(2 * QUARTER),
            Some(Ticks(4 * BAR))
        ),
    );
    assert_eq!(song.doc().clip(song.clip).unwrap().length, length);
    song.one_step(
        ids::STRETCH,
        json!({ "factor": 0.5, "markers": true }),
        "Shrink Time by Half",
    );
    assert_eq!(song.starts(), [0, BAR]);
    assert_eq!(song.midi().loop_end, Some(Ticks(BAR)));
    // Stretching by the knob, with no `markers`, leaves them; so does a
    // selection.
    song.one_step(
        ids::STRETCH,
        json!({ "factor": 2.0 }),
        "Stretch Time by Double",
    );
    assert_eq!(song.midi().loop_end, Some(Ticks(BAR)));
    song.select_notes(&[0]);
    song.one_step(
        ids::STRETCH,
        json!({ "factor": 0.5, "markers": true }),
        "Shrink Time by Half",
    );
    assert_eq!(
        song.midi().end,
        Some(Ticks(2 * BAR)),
        "a selection leaves the markers"
    );
}

#[test]
fn r10_set_length_takes_a_length_a_division_or_the_time_selections_end() {
    // Spec: pitch-and-time-utilities R10.
    let mut song = new_song(vec![
        note(60, 0, QUARTER),
        note(62, QUARTER, QUARTER / 2),
        note(64, 2 * QUARTER, QUARTER),
    ]);
    song.one_step(
        ids::SET_LENGTH,
        json!({ "length": 2 * QUARTER }),
        "Set Length",
    );
    assert_eq!(
        song.lengths(),
        [2 * QUARTER; 3],
        "every note, with none selected"
    );
    // Grid, and a division, as the chooser gives them.
    let grid = Ticks(song.grid());
    let by_grid = duration_length("grid", grid).unwrap();
    song.one_step(ids::SET_LENGTH, json!({ "length": by_grid }), "Set Length");
    assert_eq!(song.lengths(), [song.grid(); 3]);
    assert_eq!(duration_length("1/4", grid), Some(Ticks(QUARTER)));
    assert_eq!(duration_length("1/3", grid), Some(Ticks(BAR / 3)));
    assert_eq!(duration_length("1/96", grid), Some(Ticks(BAR / 96)));
    assert_eq!(duration_length("time_range", grid), None);
    // The time selection's end: for the notes that start before it.
    song.select_notes(&[0, 1, 2]);
    song.select_time(QUARTER / 2, 2 * QUARTER);
    song.one_step(
        ids::SET_LENGTH,
        json!({ "to_time_range": true }),
        "Set Length",
    );
    assert_eq!(song.lengths()[..2], [2 * QUARTER, QUARTER]);
    assert_eq!(
        song.lengths()[2],
        song.grid(),
        "starting at its end, it keeps its length"
    );
    // One of the two, and a time selection for the second.
    let both = song.refused(
        ids::SET_LENGTH,
        json!({ "length": QUARTER, "to_time_range": true }),
    );
    assert!(both.contains("`length` or `to_time_range`"), "{both}");
    song.refused(ids::SET_LENGTH, json!({}));
    song.shell.run("clip_view.select_time", json!({})).unwrap();
    let message = song.refused(ids::SET_LENGTH, json!({ "to_time_range": true }));
    assert!(message.contains("no time selection"), "{message}");
    // Notes already that long: no step.
    song.edit(ids::SET_LENGTH, json!({ "length": QUARTER }));
    song.no_step(ids::SET_LENGTH, json!({ "length": QUARTER }));
}

#[test]
fn r11_humanize_moves_starts_within_a_quarter_of_the_grid_from_a_seed() {
    // Spec: pitch-and-time-utilities R11, R20 (midi-tools).
    let notes: Vec<Note> = (0..16)
        .map(|at| note(60, BAR + at * QUARTER / 4, QUARTER / 8))
        .collect();
    let mut song = new_song(notes.clone());
    let reach = song.grid() / 4;
    song.utilities(json!({ "humanize_amount": 100 }));
    song.one_step(ids::HUMANIZE, json!({ "seed": 7 }), "Humanize");
    let first = song.notes();
    for (made, was) in first.iter().zip(&notes) {
        assert!((made.start - was.start).abs().get() <= reach, "{made:?}");
        assert_eq!(made.length, was.length, "lengths stay");
    }
    assert_ne!(first, notes, "some move");
    // The same seed on the same notes gives the same notes; another doesn't.
    let mut again = new_song(notes.clone());
    again.utilities(json!({ "humanize_amount": 100 }));
    again.edit(ids::HUMANIZE, json!({ "seed": 7 }));
    assert_eq!(again.notes(), first);
    let mut other = new_song(notes.clone());
    other.edit(ids::HUMANIZE, json!({ "seed": 8, "amount": 100 }));
    assert_ne!(other.notes(), first);
    // A smaller amount reaches less.
    let mut small = new_song(notes.clone());
    small.edit(ids::HUMANIZE, json!({ "seed": 7, "amount": 10 }));
    for (made, was) in small.notes().iter().zip(&notes) {
        assert!((made.start - was.start).abs().get() <= reach / 10 + 1);
    }
    // No amount, no change, no step; and a note never starts before 0.
    small.no_step(ids::HUMANIZE, json!({ "amount": 0 }));
    let mut early = new_song(vec![note(60, 0, QUARTER)]);
    for seed in 0..50 {
        early.edit(ids::HUMANIZE, json!({ "seed": seed, "amount": 100 }));
        assert!(early.notes()[0].start >= Ticks::ZERO);
    }
    let message = early.refused(ids::HUMANIZE, json!({ "amount": 101 }));
    assert!(message.contains("0 to 100"), "{message}");
    // The button's seed is new each time.
    let mut drawn = new_song(notes.clone());
    drawn.utilities(json!({ "humanize_amount": 100 }));
    drawn.edit(ids::HUMANIZE, json!({}));
    let one = drawn.notes();
    drawn.shell.run("edit.undo", json!({})).unwrap();
    drawn.edit(ids::HUMANIZE, json!({}));
    assert_ne!(drawn.notes(), one);
}

#[test]
fn r12_reverse_mirrors_around_the_time_selection_or_the_loop() {
    // Spec: pitch-and-time-utilities R12.
    let mut song = new_song(three());
    song.select_notes(&[0, 1, 2]);
    song.select_time(0, 4 * QUARTER);
    song.one_step(ids::REVERSE, json!({}), "Reverse");
    assert_eq!(song.keys(), [67, 64, 60]);
    assert_eq!(song.starts(), [QUARTER, 2 * QUARTER, 3 * QUARTER]);
    assert_eq!(song.lengths(), [QUARTER; 3], "lengths stay");
    // With nothing selected and no time selection: the loop brace while the
    // loop is on.
    let mut midi = MidiData::new(vec![note(60, 0, QUARTER), note(62, QUARTER, QUARTER)]);
    midi.looping = true;
    midi.loop_end = Some(Ticks(2 * QUARTER));
    let mut looped = song_on(midi.clone(), Scale::default(), |_, _| {});
    looped.one_step(ids::REVERSE, json!({}), "Reverse");
    assert_eq!(looped.keys(), [62, 60]);
    // Off, the Start marker to the End marker.
    midi.looping = false;
    midi.start = Ticks::ZERO;
    midi.end = Some(Ticks(4 * QUARTER));
    let mut marked = song_on(midi, Scale::default(), |_, _| {});
    marked.one_step(ids::REVERSE, json!({}), "Reverse");
    assert_eq!(marked.starts(), [2 * QUARTER, 3 * QUARTER]);
    // A note that would start before the contents: refused, nothing changes.
    let mut over = song_on(
        {
            let mut midi = MidiData::new(three());
            midi.looping = true;
            midi.loop_end = Some(Ticks(2 * QUARTER));
            midi
        },
        Scale::default(),
        |_, _| {},
    );
    let message = over.refused(ids::REVERSE, json!({}));
    assert!(message.contains("before the clip's contents"), "{message}");
}

#[test]
fn r13_legato_reaches_the_next_start_and_the_last_notes_the_end() {
    // Spec: pitch-and-time-utilities R13.
    let sixteenth = QUARTER / 4;
    let mut song = new_song(vec![
        note(60, 0, sixteenth),
        note(62, QUARTER, sixteenth),
        note(64, 3 * QUARTER, sixteenth),
        note(65, 3 * QUARTER, sixteenth),
    ]);
    song.one_step(ids::LEGATO, json!({}), "Legato");
    // The clip is four bars, sixteen quarters, long.
    assert_eq!(
        song.lengths(),
        [QUARTER, 2 * QUARTER, 13 * QUARTER, 13 * QUARTER],
        "a chord's notes go to the same end"
    );
    song.no_step(ids::LEGATO, json!({}));
    // With a loop on: its end; a note starting after it keeps its length.
    let mut midi = MidiData::new(vec![
        note(60, 0, sixteenth),
        note(62, 3 * QUARTER, sixteenth),
        note(64, 9 * QUARTER, sixteenth),
    ]);
    midi.looping = true;
    midi.loop_end = Some(Ticks(8 * QUARTER));
    let mut looped = song_on(midi, Scale::default(), |_, _| {});
    looped.one_step(ids::LEGATO, json!({}), "Legato");
    assert_eq!(looped.lengths(), [3 * QUARTER, 6 * QUARTER, sixteenth]);
    // Only the selected notes count as the next ones.
    let mut part = song_in(three(), Scale::default());
    part.select_notes(&[0, 2]);
    part.one_step(ids::LEGATO, json!({}), "Legato");
    assert_eq!(part.lengths()[0], 2 * QUARTER);
}

#[test]
fn r14_the_panels_values_are_view_state_that_the_command_reads_and_sets() {
    // Spec: pitch-and-time-utilities R14. No undo step, no dirty set.
    let mut song = new_song(vec![note(120, 0, QUARTER), note(60, QUARTER, QUARTER)]);
    let steps = song.steps();
    let state = song.utilities(json!({}));
    assert_eq!(
        (
            state.interval,
            state.humanize_amount,
            state.duration.as_str()
        ),
        (0, 10, "grid")
    );
    assert!((state.stretch - 0.5).abs() < 1e-9 && (state.stretch_factor - 1.0).abs() < 1e-9);
    assert_eq!((state.lowest, state.highest), (Some(60), Some(120)));
    assert_eq!(
        state.interval_range,
        (-36, 7),
        "the copies must fit the keyboard"
    );
    assert!(!state.fit_to_scale && state.scale.is_none() && !state.time_range);
    let state = song.utilities(json!({
        "interval": 7, "stretch": 1.0, "duration": "1/8", "humanize_amount": 50
    }));
    assert_eq!(
        (
            state.interval,
            state.humanize_amount,
            state.duration.as_str()
        ),
        (7, 50, "1/8")
    );
    assert!((state.stretch_factor - 10.0).abs() < 1e-9);
    assert_eq!(song.utilities(json!({})).interval, 7, "it stays");
    for bad in [
        json!({ "interval": 37 }),
        json!({ "interval": -37 }),
        json!({ "stretch": 1.5 }),
        json!({ "duration": "1/7" }),
        json!({ "humanize_amount": 101 }),
    ] {
        assert!(
            song.shell.run(ids::UTILITIES, bad.clone()).is_err(),
            "{bad}"
        );
    }
    assert_eq!(song.steps(), steps);
    assert!(!song.shell.is_dirty());
    // A scale and a time selection show.
    let mut scaled = song_in(three(), major());
    scaled.select_time(0, QUARTER);
    let state = scaled.utilities(json!({}));
    assert_eq!(state.scale.as_deref(), Some("C Major"));
    assert!(state.fit_to_scale && state.time_range);
}

#[test]
fn r15_every_utility_is_registered_with_its_class_and_a_description() {
    // Spec: pitch-and-time-utilities R15.
    let song = new_song(Vec::new());
    for id in [
        ids::TRANSPOSE,
        ids::FIT_TO_SCALE,
        ids::INVERT,
        ids::ADD_INTERVAL,
        ids::STRETCH,
        ids::SET_LENGTH,
        ids::HUMANIZE,
        ids::REVERSE,
        ids::LEGATO,
        ids::UTILITIES,
    ] {
        let spec = song
            .shell
            .registry()
            .get(id)
            .unwrap_or_else(|| panic!("{id}"));
        assert!(spec.description.len() > 40, "{id}");
        let wanted = if id == ids::UTILITIES {
            bl_commands::Permission::Read
        } else {
            bl_commands::Permission::Edit
        };
        assert_eq!(spec.permission, Some(wanted), "{id}");
    }
}

#[test]
fn r21_a_utility_closes_the_open_midi_tool_application() {
    // Spec: midi-tools R21, R32; pitch-and-time-utilities R3.
    let mut song = new_song(three());
    let state = |song: &mut Song| -> MidiToolState {
        serde_json::from_value(song.shell.run(tool_ids::STATE, json!({})).unwrap()).unwrap()
    };
    song.select_notes(&[0, 1, 2]);
    song.shell
        .run(
            tool_ids::SET,
            json!({ "tool": "test_transpose", "values": { "degrees": 1 } }),
        )
        .unwrap();
    assert!(state(&mut song).application.is_some());
    song.edit(ids::REVERSE, json!({ "notes": [0, 1, 2] }));
    assert!(state(&mut song).application.is_none(), "Reverse closed it");
}

#[test]
fn r3_a_drag_is_one_step_and_escape_puts_the_notes_back() {
    // Spec: pitch-and-time-utilities R3, R4, R7, R8. Each frame runs the
    // command from where the drag began.
    let mut song = new_song(three());
    song.select_notes(&[0, 1, 2]);
    let (original, steps) = (song.notes(), song.steps());
    for by in 1..=3 {
        song.frame(ids::TRANSPOSE, json!({ "by": by }));
    }
    assert_eq!(song.keys(), [63, 67, 70], "the last frame, from the start");
    song.shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(song.notes(), original, "Esc puts the notes back");
    assert_eq!((song.steps(), song.shell.gesture_open()), (steps, false));
    for by in 1..=3 {
        song.frame(ids::TRANSPOSE, json!({ "by": by }));
    }
    song.shell
        .gesture(
            GesturePhase::Commit,
            Some(&Invocation::new(ids::TRANSPOSE, json!({ "by": 5 }))),
        )
        .unwrap();
    assert_eq!(song.keys(), [65, 69, 72]);
    assert_eq!(song.steps(), steps + 1, "however many frames, one step");
    assert_eq!(song.last_step().as_deref(), Some("Transpose"));
    // Interval Size: the copies are of the notes selected when the drag began,
    // at the value the field shows, and the drag leaves one set of them.
    let mut copies = new_song(three());
    copies.select_notes(&[0, 1]);
    let steps = copies.steps();
    for by in 1..=4 {
        copies.frame(ids::ADD_INTERVAL, json!({ "by": by }));
    }
    assert_eq!(copies.keys(), [60, 64, 64, 68, 67], "two copies, at +4");
    copies.shell.gesture(GesturePhase::Commit, None).unwrap();
    assert_eq!(copies.steps(), steps + 1);
    assert_eq!(copies.last_step().as_deref(), Some("Interval"));
    // Stretch: the factor from the drag's start, and Esc puts back the notes
    // and the time selection.
    let mut stretched = new_song(three());
    stretched.select_notes(&[0, 1, 2]);
    stretched.select_time(0, 3 * QUARTER);
    let original = stretched.notes();
    stretched.frame(ids::STRETCH, json!({ "factor": 2.0 }));
    stretched.frame(ids::STRETCH, json!({ "factor": 3.0 }));
    assert_eq!(stretched.starts(), [0, 3 * QUARTER, 6 * QUARTER]);
    assert_eq!(stretched.time(), Some((0, 9 * QUARTER)));
    stretched.shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(stretched.notes(), original);
    assert_eq!(stretched.time(), Some((0, 3 * QUARTER)));
}
