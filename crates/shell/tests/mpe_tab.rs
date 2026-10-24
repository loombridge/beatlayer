//! The Clip View's MPE tab, driven headless through `Shell::run` and
//! `Shell::gesture` as agents and the tab's mouse drive it (WP-6.18; D17):
//! the tab, its lanes and grid, and the commands that add, move, delete,
//! bend, scale, draw and clear a note's Pitch, Slide and Pressure
//! breakpoints. The behaviour is in the MPE spec, R22 to R32.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    Breakpoint, Clip, ClipData, ClipId, Command, Document, Editor, MidiData, Note, TrackId,
};
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::note_commands::{self as ids, ClipTab, ClipViewState, NoteEditResult};
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

struct Song {
    shell: Shell,
    clip: ClipId,
    track: TrackId,
}

fn point(at: i64, value: f64) -> Breakpoint {
    Breakpoint {
        at: Ticks(at),
        value,
        bend: 0.0,
    }
}

fn note(key: u8, start: i64, length: i64) -> Note {
    Note::new(key, Ticks(start), Ticks(length), 100)
}

/// A song with a MIDI clip on the first MIDI track, selected, the Clip View
/// open on the MPE tab.
fn song(notes: Vec<Note>) -> Song {
    let mut editor = factory_default_set();
    let track: TrackId = editor.document().top_level().next().unwrap();
    let id: ClipId = editor.new_id();
    let clip = Clip::new(
        id,
        Ticks::ZERO,
        Ticks(4 * BAR),
        ClipData::Midi(MidiData::new(notes)),
    );
    editor.apply(Command::CreateClip { track, clip }).unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Mpe.blset" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [id] }))
        .unwrap();
    shell.run("view.toggle_clip_view", json!({})).unwrap();
    shell.run("view.clip_tab", json!({ "tab": "mpe" })).unwrap();
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

    /// Runs an edit, checks it's exactly one undo step named `name`, and that
    /// undo and redo restore the set exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value, name: &str) -> NoteEditResult {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self.edit(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert!(result.undo_step, "{id}");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        assert_eq!(self.shell.editor().undo_history().next(), Some(name));
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }

    /// Runs `id` and expects it to be refused with the set untouched.
    fn refused(&mut self, id: &str, args: Value) {
        let before = self.doc().clone();
        assert!(self.shell.run(id, args.clone()).is_err(), "{id} {args}");
        assert_eq!(*self.doc(), before, "{id} {args} changed the set");
    }

    /// One step of a drag; `Null` arguments end it with no last command.
    fn gesture(&mut self, phase: GesturePhase, id: &str, args: Value) {
        let invocation = Invocation::new(id, args.clone());
        self.shell
            .gesture(phase, (!args.is_null()).then_some(&invocation))
            .unwrap_or_else(|e| panic!("{id} failed: {e}"));
    }
}

/// A note with the envelopes the editing tests start from.
fn shaped() -> Note {
    Note {
        pitch: vec![
            point(0, 0.0),
            point(100, 4.0),
            point(200, 8.0),
            point(400, 0.0),
        ],
        slide: vec![point(0, 10.0), point(400, 100.0)],
        ..note(60, 0, 400)
    }
}

// Spec: mpe R32. A breakpoint made on a segment, in one undo step.
#[test]
fn a_breakpoint_is_added_on_a_notes_envelope_in_one_step() {
    let mut song = song(vec![shaped(), note(64, QUARTER, 400)]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    song.one_step(
        ids::ADD_EXPRESSION_BREAKPOINT,
        json!({ "envelope": "pitch", "at": 150, "value": 6.5 }),
        "Add Expression Breakpoint",
    );
    let notes = song.notes();
    assert_eq!(
        notes[0].pitch,
        [
            point(0, 0.0),
            point(100, 4.0),
            point(150, 6.5),
            point(200, 8.0),
            point(400, 0.0)
        ]
    );
    assert!(!notes[1].has_expression(), "only the selected note changed");
    // A breakpoint already there takes the value and keeps its bend.
    song.one_step(
        ids::ADD_EXPRESSION_BREAKPOINT,
        json!({ "envelope": "pitch", "at": 100, "value": -3.0, "bend": 0.25 }),
        "Add Expression Breakpoint",
    );
    let pitch = &song.notes()[0].pitch;
    assert_eq!((pitch[1].value, pitch[1].bend), (-3.0, 0.25));
    // Spec: mpe R26. Out of range, or past the note's end, changes nothing.
    for bad in [
        json!({ "envelope": "pitch", "at": 10, "value": 48.5 }),
        json!({ "envelope": "slide", "at": 10, "value": -1.0 }),
        json!({ "envelope": "slide", "at": 401, "value": 1.0 }),
        json!({ "envelope": "slide", "at": -1, "value": 1.0 }),
        json!({ "envelope": "slide", "at": 10, "value": 1.0, "bend": 2.0 }),
    ] {
        song.refused(ids::ADD_EXPRESSION_BREAKPOINT, bad);
    }
}

// Spec: mpe R26, I-DRAG-050. A drag moves every selected breakpoint, a
// neighbour passed is removed, and the drag is one transaction that Esc puts
// back.
#[test]
fn a_dragged_breakpoint_moves_with_the_selection_and_removes_a_neighbour_it_passes() {
    let mut song = song(vec![shaped()]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    song.view(
        ids::SELECT_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "pitch", "breakpoints": [{ "note": 0, "at": 100 }] }),
    );
    // Selected, the breakpoint is the default of the move.
    song.one_step(
        ids::MOVE_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "pitch", "time": 150, "value": 2.0 }),
        "Move Expression Breakpoints",
    );
    assert_eq!(
        song.notes()[0].pitch,
        [point(0, 0.0), point(250, 6.0), point(400, 0.0)],
        "the breakpoint at 200 was passed"
    );
    // Held inside the note and the range.
    song.edit(
        ids::MOVE_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "slide", "breakpoints": [
            { "note": 0, "at": 0 }, { "note": 0, "at": 400 }
        ], "time": 90000, "value": 90000.0 }),
    );
    assert_eq!(
        song.notes()[0].slide,
        [point(0, 37.0), point(400, 127.0)],
        "the group moves together and stops where one end would leave the range"
    );
    // A drag: one step, measured from where it began, and Esc puts it back.
    let start = song.doc().clone();
    let steps = song.steps();
    let args = |time: i64| {
        json!({ "envelope": "pitch", "breakpoints": [{ "note": 0, "at": 250 }],
            "time": time, "value": 0.0 })
    };
    for time in [10, 20, -40] {
        song.gesture(
            GesturePhase::Update,
            ids::MOVE_EXPRESSION_BREAKPOINTS,
            args(time),
        );
    }
    assert_eq!(
        song.notes()[0].pitch[1].at,
        Ticks(210),
        "from where it began"
    );
    song.gesture(
        GesturePhase::Cancel,
        ids::MOVE_EXPRESSION_BREAKPOINTS,
        Value::Null,
    );
    assert_eq!(*song.doc(), start, "Esc puts the clip back");
    for time in [10, 20] {
        song.gesture(
            GesturePhase::Update,
            ids::MOVE_EXPRESSION_BREAKPOINTS,
            args(time),
        );
    }
    song.gesture(
        GesturePhase::Commit,
        ids::MOVE_EXPRESSION_BREAKPOINTS,
        args(30),
    );
    assert_eq!(song.steps(), steps + 1, "a drag is one undo step");
    assert_eq!(song.notes()[0].pitch[1].at, Ticks(280));
}

// Spec: mpe R26. A segment inside the time selection gets breakpoints at its
// edges and moves whole.
#[test]
fn a_segment_in_the_time_selection_moves_whole() {
    let mut song = song(vec![Note {
        pitch: vec![point(0, 0.0), point(400, 8.0)],
        ..note(60, 0, 400)
    }]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    song.one_step(
        ids::MOVE_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "pitch", "breakpoints": [{ "note": 0, "at": 0 }],
            "value": 3.0, "fill": { "start": 100, "end": 300 } }),
        "Move Expression Breakpoints",
    );
    assert_eq!(
        song.notes()[0].pitch,
        [
            point(0, 0.0),
            point(100, 5.0),
            point(300, 9.0),
            point(400, 8.0)
        ]
    );
}

// Spec: mpe R26, R32. Deleting, and selecting breakpoints, which is view
// state.
#[test]
fn breakpoints_are_deleted_and_selected() {
    let mut song = song(vec![shaped()]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    let steps = song.steps();
    let state = song.view(
        ids::SELECT_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "pitch", "all": true }),
    );
    assert_eq!(state.mpe.envelope, Some(bl_model::Expression::Pitch));
    assert_eq!(state.mpe.breakpoints.len(), 4, "Cmd+A selects all of them");
    let state = song.view(
        ids::SELECT_EXPRESSION_BREAKPOINTS,
        json!({ "breakpoints": [{ "note": 0, "at": 100 }, { "note": 0, "at": 200 }], "mode": "toggle" }),
    );
    assert_eq!(state.mpe.breakpoints.len(), 2, "two toggled out");
    assert_eq!(song.steps(), steps, "selecting is no undo step");
    song.one_step(
        ids::DELETE_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "pitch" }),
        "Delete Expression Breakpoints",
    );
    assert_eq!(song.notes()[0].pitch, [point(100, 4.0), point(200, 8.0)]);
    song.refused(
        ids::DELETE_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "pitch", "breakpoints": [{ "note": 0, "at": 7 }] }),
    );
    song.refused(
        ids::SELECT_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "pitch", "breakpoints": [{ "note": 9, "at": 7 }] }),
    );
}

// Spec: mpe R28. Option-dragging a segment curves it; Option-double-click
// straightens it.
#[test]
fn a_segment_is_curved_and_straightened() {
    let mut song = song(vec![shaped()]);
    song.one_step(
        ids::SET_EXPRESSION_CURVE,
        json!({ "note": 0, "envelope": "pitch", "at": 100, "bend": 0.5 }),
        "Set Expression Curve",
    );
    assert_eq!(song.notes()[0].pitch[1].bend, 0.5);
    song.one_step(
        ids::SET_EXPRESSION_CURVE,
        json!({ "note": 0, "envelope": "pitch", "at": 100, "bend": 0.0 }),
        "Set Expression Curve",
    );
    song.refused(
        ids::SET_EXPRESSION_CURVE,
        json!({ "note": 0, "envelope": "pitch", "at": 101, "bend": 0.5 }),
    );
    song.refused(
        ids::SET_EXPRESSION_CURVE,
        json!({ "note": 0, "envelope": "pitch", "at": 100, "bend": 1.5 }),
    );
}

// Spec: mpe R29. Scaling multiplies from 0 for every selected note; Pitch
// doesn't scale.
#[test]
fn a_lane_scales_over_every_selected_notes_whole_length() {
    let mut song = song(vec![shaped(), note(64, QUARTER, 400)]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0, 1] }));
    song.one_step(
        ids::SCALE_EXPRESSION,
        json!({ "lane": "slide", "factor": 0.5 }),
        "Scale Expression",
    );
    assert_eq!(song.notes()[0].slide, [point(0, 5.0), point(400, 50.0)]);
    song.one_step(
        ids::SCALE_EXPRESSION,
        json!({ "lane": "velocity", "factor": 0.5 }),
        "Scale Expression",
    );
    assert!(song.notes().iter().all(|n| n.velocity == 50));
    song.one_step(
        ids::SCALE_EXPRESSION,
        json!({ "lane": "release_velocity", "factor": 3.0 }),
        "Scale Expression",
    );
    assert!(song.notes().iter().all(|n| n.release_velocity == 127));
    song.refused(
        ids::SCALE_EXPRESSION,
        json!({ "lane": "pitch_bend", "factor": 2.0 }),
    );
    song.refused(
        ids::SCALE_EXPRESSION,
        json!({ "lane": "slide", "factor": -1.0 }),
    );
}

// Spec: mpe R30. Draw Mode replaces the stretch with steps, in one
// transaction for the drag.
#[test]
fn drawing_replaces_the_stretch_dragged_with_steps() {
    let mut song = song(vec![shaped()]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    let steps = [
        json!({ "at": 100, "length": 50, "value": 20.0 }),
        json!({ "at": 150, "length": 50, "value": 30.0 }),
    ];
    song.one_step(
        ids::DRAW_EXPRESSION,
        json!({ "envelope": "slide", "steps": steps }),
        "Draw Expression",
    );
    assert_eq!(
        song.notes()[0].slide,
        [
            point(0, 10.0),
            point(100, 20.0),
            point(149, 20.0),
            point(150, 30.0),
            point(200, 30.0),
            point(400, 100.0)
        ]
    );
    // Pitch is drawn in key rows: each note takes the row less its own key.
    song.one_step(
        ids::DRAW_EXPRESSION,
        json!({ "envelope": "pitch", "absolute": true,
            "steps": [{ "at": 0, "length": 400, "value": 67.0 }] }),
        "Draw Expression",
    );
    assert_eq!(song.notes()[0].pitch, [point(0, 7.0), point(400, 7.0)]);
    song.refused(
        ids::DRAW_EXPRESSION,
        json!({ "envelope": "slide", "steps": [] }),
    );
    song.refused(
        ids::DRAW_EXPRESSION,
        json!({ "envelope": "slide", "steps": [{ "at": 0, "length": 0, "value": 1.0 }] }),
    );
    // One drag, one step.
    let start = song.steps();
    for to in [50, 100, 150] {
        song.gesture(
            GesturePhase::Update,
            ids::DRAW_EXPRESSION,
            json!({ "envelope": "pressure",
                "steps": [{ "at": 0, "length": to, "value": f64::from(to as i32 / 2) }] }),
        );
    }
    song.gesture(GesturePhase::Commit, ids::DRAW_EXPRESSION, Value::Null);
    assert_eq!(song.steps(), start + 1);
}

// Spec: mpe R31. Clear All Envelopes clears the three envelopes of the
// selected notes in one undo step; Z brings the Pitch rows into view.
#[test]
fn clear_all_envelopes_and_the_zoom() {
    let mut song = song(vec![
        shaped(),
        Note {
            pressure: vec![point(0, 9.0)],
            ..note(72, QUARTER, 400)
        },
    ]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    song.one_step(ids::CLEAR_EXPRESSIONS, json!({}), "Clear All Envelopes");
    let notes = song.notes();
    assert!(!notes[0].has_expression());
    assert!(
        notes[1].has_expression(),
        "the unselected note keeps its envelopes"
    );
    song.refused(ids::CLEAR_EXPRESSIONS, json!({ "notes": [5] }));
    // Z: the rows the Pitch envelopes reach inside the time selection.
    let mut song = self::song(vec![shaped()]);
    song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    song.view(
        "clip_view.select_time",
        json!({ "clip": song.clip, "start": 0.0, "end": 100.0 }),
    );
    song.view(ids::MPE_ZOOM, json!({}));
    let zoom = song.shell.clip_view().mpe_zoom().unwrap();
    assert_eq!((zoom.low, zoom.high), (60.0, 64.0));
}

// Spec: mpe R22. The tab is for a MIDI clip: it shows, cycles with the other
// two, and an audio clip has none.
#[test]
fn the_mpe_tab_opens_for_a_midi_clip_and_cycles_with_the_other_tabs() {
    let mut song = song(vec![note(60, 0, QUARTER)]);
    assert_eq!(song.view("view.clip_tab", json!({})).tab, ClipTab::Notes);
    assert_eq!(
        song.view("view.clip_tab", json!({})).tab,
        ClipTab::Envelopes
    );
    assert_eq!(song.view("view.clip_tab", json!({})).tab, ClipTab::Mpe);
    assert_eq!(song.view("view.clip_tab", json!({})).tab, ClipTab::Notes);
    let steps = song.steps();
    song.view("view.clip_tab", json!({ "tab": "mpe" }));
    assert_eq!(song.steps(), steps, "choosing a tab is no undo step");
    // The editor's contexts: the note editor still, and the envelope's once
    // one has the keyboard.
    song.view(ids::SELECT_NOTES, json!({ "notes": [0] }));
    song.shell.run("focus.clip_view", json!({})).unwrap();
    assert!(
        song.shell
            .can_invoke("view.clip_tab", &json!({ "tab": "mpe" }))
    );
    let contexts = song.shell.active_contexts();
    assert!(contexts.contains(&bl_commands::Context::NoteEditor));
    assert!(!contexts.contains(&bl_commands::Context::Envelope));
    song.view(
        ids::SELECT_EXPRESSION_BREAKPOINTS,
        json!({ "envelope": "slide", "breakpoints": [] }),
    );
    assert!(
        song.shell
            .active_contexts()
            .contains(&bl_commands::Context::Envelope)
    );
    // With no MIDI clip showing the menu item is disabled.
    song.shell
        .run("arrangement.select_clips", json!({ "clips": [] }))
        .unwrap();
    assert!(
        !song
            .shell
            .can_invoke("view.clip_tab", &json!({ "tab": "mpe" }))
    );
}

// Spec: mpe R23. The lanes: a new set shows Slide and Pressure; toggles, the
// triangle and its Option click, heights, the swap; none an undo step.
#[test]
fn the_lanes_toggle_hide_swap_and_resize_as_view_state() {
    let mut song = song(vec![note(60, 0, QUARTER)]);
    let steps = song.steps();
    let state = song.view("view.mpe_lanes", json!({}));
    let lanes = |state: &ClipViewState| state.mpe.lanes.iter().map(|l| l.lane).collect::<Vec<_>>();
    use bl_model::MpeLane::{PitchBend, Pressure, ReleaseVelocity, Slide, Velocity};
    assert_eq!(lanes(&state), [Slide, Pressure]);
    let state = song.view("view.mpe_lanes", json!({ "lane": "velocity" }));
    assert_eq!(lanes(&state), [Slide, Pressure, Velocity]);
    let state = song.view("view.mpe_lanes", json!({ "lane": "slide", "show": false }));
    assert_eq!(lanes(&state), [Pressure, Velocity]);
    let state = song.view(
        "view.mpe_lanes",
        json!({ "lane": "pressure", "swap": true }),
    );
    assert_eq!(
        lanes(&state),
        [Velocity, ReleaseVelocity],
        "Release Velocity is the first after it that was off"
    );
    let state = song.view(
        "view.mpe_lanes",
        json!({ "lane": "velocity", "height": 90.0 }),
    );
    assert_eq!(state.mpe.lanes[0].height, 90.0);
    // The triangle hides the lanes and the toggles, and shows them again.
    let state = song.view("view.mpe_lanes", json!({ "visible": false }));
    assert!(state.mpe.hidden && state.mpe.lanes.is_empty());
    let state = song.view("view.mpe_lanes", json!({ "visible": true }));
    assert_eq!(lanes(&state), [Velocity, ReleaseVelocity]);
    // Option shows all four.
    let state = song.view("view.mpe_lanes", json!({ "all": true }));
    assert_eq!(lanes(&state), [Slide, Pressure, Velocity, ReleaseVelocity]);
    for bad in [
        json!({ "show": true }),
        json!({ "lane": "slide", "height": 2.0 }),
        json!({ "lane": "pitch_bend", "show": true }),
    ] {
        assert!(song.shell.run("view.mpe_lanes", bad).is_err());
    }
    assert!(
        !lanes(&state).contains(&PitchBend),
        "no Drum Rack, no Pitch Bend lane"
    );
    assert_eq!(song.steps(), steps, "none of it is an undo step");
}

// Spec: mpe R27. The tab's own grid is off in a new clip and per clip.
#[test]
fn the_tabs_grid_is_its_own_and_per_clip() {
    let mut song = song(vec![note(60, 0, QUARTER)]);
    let arrangement_snap = song.shell.arrangement().grid().snap;
    assert!(!song.view("view.mpe_grid", json!({ "on": false })).mpe.grid);
    assert!(
        song.view("view.mpe_grid", json!({})).mpe.grid,
        "it switches"
    );
    assert_eq!(song.shell.arrangement().grid().snap, arrangement_snap);
    assert!(!song.view("view.mpe_grid", json!({})).mpe.grid);
    song.view("view.mpe_grid", json!({ "clip": song.clip, "on": true }));
    assert!(song.shell.clip_view().mpe().grid_on(song.clip));
    // Cmd+4 only works in the tab.
    assert!(
        song.shell.can_invoke("view.mpe_grid", &json!({})) || {
            song.shell.run("focus.clip_view", json!({})).unwrap();
            song.shell.can_invoke("view.mpe_grid", &json!({}))
        }
    );
    song.view("view.clip_tab", json!({ "tab": "notes" }));
    assert!(!song.shell.can_invoke("view.mpe_grid", &json!({})));
    let _ = song.track;
}

// Spec: mpe R24. On a track whose instrument is a Drum Rack, the Pitch Bend
// lane comes first among the lanes; the toggles don't switch it.
#[test]
fn a_drum_rack_track_has_the_pitch_bend_lane_first() {
    use bl_model::MpeLane::{PitchBend, Pressure, Slide};
    let mut song = song(vec![note(36, 0, QUARTER)]);
    let lanes = |song: &mut Song| {
        song.view("view.mpe_lanes", json!({}))
            .mpe
            .lanes
            .iter()
            .map(|l| l.lane)
            .collect::<Vec<_>>()
    };
    assert_eq!(lanes(&mut song), [Slide, Pressure]);
    song.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [song.track] }),
        )
        .unwrap();
    song.shell.set_focused_area(bl_shell::Area::Detail);
    song.shell
        .invoke(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack",
                "role": "instrument",
            } }),
        )
        .unwrap();
    song.shell
        .run("arrangement.select_clips", json!({ "clips": [song.clip] }))
        .unwrap();
    assert_eq!(lanes(&mut song), [PitchBend, Slide, Pressure]);
    // The triangle hides it with the others.
    song.view("view.mpe_lanes", json!({ "visible": false }));
    assert!(lanes(&mut song).is_empty());
}

// Spec: mpe R23, R27. The lanes and the tab's grid are saved with the set,
// beside the song, and a new set starts with a new set's.
#[test]
fn the_lanes_and_the_grid_survive_a_set_round_trip() {
    let mut editor = factory_default_set();
    let track: TrackId = editor.document().top_level().next().unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * BAR),
                ClipData::Midi(MidiData::new([note(60, 0, QUARTER)])),
            ),
        })
        .unwrap();
    let dir = std::env::temp_dir().join(format!(
        "bl-shell-mpe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Mpe.blset");
    bl_shell::files::ProjectFiles::new()
        .save(&path, &editor)
        .unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    let open = |shell: &mut Shell| {
        shell
            .run("set.open", json!({ "path": path.to_string_lossy() }))
            .unwrap()
    };
    open(&mut shell);
    shell
        .run("view.mpe_grid", json!({ "clip": clip, "on": true }))
        .unwrap();
    shell
        .run(
            "view.mpe_lanes",
            json!({ "lane": "velocity", "show": true, "height": 70.0 }),
        )
        .unwrap();
    shell
        .run("view.mpe_lanes", json!({ "lane": "slide", "show": false }))
        .unwrap();
    assert!(!shell.is_dirty(), "the tab's state doesn't dirty the set");
    shell.run("set.save", json!({})).unwrap();
    shell.run("set.new", json!({})).unwrap();
    assert!(
        shell.clip_view().mpe().is_default(),
        "a new set starts anew"
    );
    open(&mut shell);
    let mpe = shell.clip_view().mpe();
    assert!(mpe.grid_on(clip), "the grid is kept for its clip");
    assert_eq!(
        mpe.lanes(false),
        [bl_model::MpeLane::Pressure, bl_model::MpeLane::Velocity]
    );
    assert_eq!(mpe.height(bl_model::MpeLane::Velocity), 70.0);
    let _ = std::fs::remove_dir_all(&dir);
}
