//! The MIDI Note Editor's view, clip markers and time commands, driven
//! headless through `Shell::run`, as agents, keys and the mouse drive them
//! (WP-6.54; the piano roll spec, R32 to R42).

use std::path::{Path, PathBuf};

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_commands::Permission;
use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, Document, Editor, MidiData, Note, SceneId, TrackId,
};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::note_commands::ClipViewState;
use bl_shell::settings::Preferences;
use bl_shell::testing;
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

/// A song with a MIDI clip four bars long on the first MIDI track, selected
/// in the Arrangement, on a null device paced by hand.
struct Song {
    shell: Shell,
    control: NullControl,
    clip: ClipId,
    track: TrackId,
}

fn song(notes: Vec<Note>) -> Song {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let id: ClipId = editor.new_id();
    let clip = Clip::new(
        id,
        Ticks::ZERO,
        Ticks(4 * BAR),
        ClipData::Midi(MidiData::new(notes)),
    );
    editor.apply(Command::CreateClip { track, clip }).unwrap();
    editor.clear_history().unwrap();
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            pacing: Pacing::Manual,
            ..NullOptions::default()
        },
        &[("null", "Null device")],
    );
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(Preloaded(Some(editor))),
    );
    shell.use_test_clock(control.clock().expect("manual pacing"));
    shell
        .run("set.open", json!({ "path": "Notes.blset" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [id] }))
        .unwrap();
    Song {
        shell,
        control,
        clip: id,
        track,
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

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    fn view(&mut self, id: &str, args: Value) -> ClipViewState {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }

    fn show(&mut self, args: Value) -> ClipViewState {
        self.view("clip_view.note_view", args)
    }

    fn select_time(&mut self, start: i64, end: i64) {
        self.view(
            "clip_view.select_time",
            json!({ "clip": self.clip, "start": start, "end": end }),
        );
    }

    fn insert_marker(&mut self, at: Option<i64>) {
        self.view(
            "clip_view.set_insert_marker",
            json!({ "clip": self.clip, "beat": at }),
        );
    }

    /// Runs an edit, checks it's exactly one undo step, and that undo and
    /// redo restore the set exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value) -> Value {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self
            .shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"));
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }

    /// Starts the song and lets `millis` of test time pass.
    fn play(&mut self, millis: u64) {
        self.shell.start_audio();
        assert!(self.shell.audio().engine_running());
        self.shell.run("transport.play", json!({})).unwrap();
        testing::advance(&self.control, &mut self.shell, millis);
    }
}

fn spread() -> Song {
    song(vec![
        note(50, 0, QUARTER),
        note(60, BAR, QUARTER),
        note(70, 2 * BAR, QUARTER),
    ])
}

// Spec: piano-roll R32, R42.
#[test]
fn z_then_x_twice_goes_to_the_selection_back_and_to_the_whole_clip() {
    let mut song = spread();
    let steps = song.steps();
    let whole = (Ticks::ZERO, Ticks(4 * BAR));
    let state = song.show(json!({}));
    assert_eq!(state.span, Some(whole), "the whole clip shows at first");
    assert_eq!(state.rows, None, "and the editor's own rows");
    // Z zooms to the time selection and the selected notes' rows.
    song.view("clip_view.select_notes", json!({ "notes": [1] }));
    song.select_time(BAR, 2 * BAR);
    let state = song.show(json!({ "show": "selection" }));
    assert_eq!(state.span, Some((Ticks(BAR), Ticks(2 * BAR))));
    assert_eq!(
        state.rows,
        Some((66.0, 70.0)),
        "key 60 with a row clear above and below, and four rows at least"
    );
    // X goes back, and with nothing to go back to shows the whole clip.
    let state = song.show(json!({ "show": "back" }));
    assert_eq!((state.span, state.rows), (Some(whole), None));
    let state = song.show(json!({ "show": "back" }));
    assert_eq!((state.span, state.rows), (Some(whole), None));
    // The same through view.state's clip_view.
    song.show(json!({ "show": "selection" }));
    let reported = song.shell.run("view.state", json!({})).unwrap();
    assert_eq!(
        reported["clip_view"]["span"],
        json!([BAR, 2 * BAR]),
        "{reported}"
    );
    assert_eq!(reported["clip_view"]["rows"], json!([66.0, 70.0]));
    assert_eq!(song.steps(), steps, "none of it is an undo step");
    assert!(!song.shell.is_dirty(), "and none of it dirties the set");
}

// Spec: piano-roll R32.
#[test]
fn w_and_h_fit_the_span_and_the_lowest_to_the_highest_note() {
    let mut song = spread();
    song.select_time(BAR, 2 * BAR);
    song.show(json!({ "show": "selection" }));
    // W fits the clip's span to the width and leaves the rows.
    let state = song.show(json!({ "show": "whole" }));
    assert_eq!(state.span, Some((Ticks::ZERO, Ticks(4 * BAR))));
    assert!(state.rows.is_some());
    // H fits the key rows from the lowest note (50) to the highest (70),
    // a row clear above and below.
    let state = song.show(json!({ "show": "height" }));
    assert_eq!(state.rows, Some((56.0, 79.0)));
    // With no notes there is nothing to fit.
    let mut empty = song_without_notes();
    assert!(
        empty
            .shell
            .run("clip_view.note_view", json!({ "show": "height" }))
            .is_err()
    );
    let state = empty.show(json!({ "show": "whole" }));
    assert_eq!(state.span, Some((Ticks::ZERO, Ticks(4 * BAR))));
}

fn song_without_notes() -> Song {
    song(Vec::new())
}

// Spec: piano-roll R32.
#[test]
fn plus_and_minus_zoom_about_the_time_selection_or_the_insert_marker() {
    let mut song = spread();
    // About the insert marker with no time selection: the marker holds
    // still.
    song.insert_marker(Some(BAR));
    let state = song.show(json!({ "show": "in" }));
    assert_eq!(
        state.span,
        Some((Ticks(BAR / 2), Ticks(5 * BAR / 2))),
        "the span halves about the marker"
    );
    let state = song.show(json!({ "show": "out" }));
    assert_eq!(
        state.span,
        Some((Ticks::ZERO, Ticks(4 * BAR))),
        "and out again shows the whole clip"
    );
    // About the middle of the time selection.
    song.select_time(2 * BAR, 3 * BAR);
    let state = song.show(json!({ "show": "in" }));
    assert_eq!(
        state.span,
        Some((Ticks(5 * BAR / 4), Ticks(13 * BAR / 4))),
        "halved about 2.5 bars"
    );
    // Each zoom is one X back.
    let state = song.show(json!({ "show": "back" }));
    assert_eq!(state.span, Some((Ticks::ZERO, Ticks(4 * BAR))));
}

// Spec: piano-roll R33, R34, R35.
#[test]
fn scrolling_is_no_zoom_and_pauses_follow_until_playback_starts() {
    let mut song = spread();
    song.select_time(BAR, 2 * BAR);
    song.show(json!({ "show": "selection" }));
    // Scrolling by hand: a span of ticks, and an octave of rows.
    let state = song.show(json!({
        "show": "scroll",
        "start": 2 * BAR, "end": 3 * BAR, "top": 54.0, "bottom": 58.0,
    }));
    assert_eq!(state.span, Some((Ticks(2 * BAR), Ticks(3 * BAR))));
    assert_eq!(state.rows, Some((54.0, 58.0)));
    assert!(state.follow_paused, "a scroll by hand pauses Follow");
    // A scroll keeps the width inside the clip.
    let state = song.show(json!({ "show": "scroll", "start": 9 * BAR, "end": 10 * BAR }));
    assert_eq!(state.span, Some((Ticks(3 * BAR), Ticks(4 * BAR))));
    // Scrolling isn't a zoom X goes back through: the view before Z shows.
    let state = song.show(json!({ "show": "back" }));
    assert_eq!(state.span, Some((Ticks::ZERO, Ticks(4 * BAR))));
    assert_eq!(state.rows, None);
    // Follow turning the page doesn't pause it.
    song.show(json!({}));
    let state = song.show(json!({ "show": "page", "start": BAR, "end": 2 * BAR }));
    assert_eq!(state.span, Some((Ticks(BAR), Ticks(2 * BAR))));
    assert!(!state.follow_paused);
    // A bad span is refused, and so is asking for nothing.
    for bad in [
        json!({ "show": "scroll", "start": BAR, "end": BAR }),
        json!({ "show": "scroll", "top": 5.0 }),
        json!({ "show": "span" }),
    ] {
        assert!(song.shell.run("clip_view.note_view", bad).is_err());
    }
}

// Spec: piano-roll R35.
#[test]
fn follow_pauses_on_an_edit_and_resumes_with_playback_follow_or_a_click() {
    let mut song = spread();
    let paused = |song: &mut Song| {
        let state = song.shell.run("view.state", json!({})).unwrap();
        state["clip_view"]["follow_paused"] == json!(true)
    };
    let edit = |song: &mut Song, key: u8| {
        song.shell
            .run(
                "note.add",
                json!({ "clip": song.clip, "key": key, "start": 0, "length": QUARTER }),
            )
            .unwrap();
    };
    assert!(!paused(&mut song));
    // An edit in the editor pauses Follow; a click in the scrub area
    // resumes it.
    edit(&mut song, 61);
    assert!(paused(&mut song), "an edit pauses Follow");
    song.show(json!({}));
    assert!(!paused(&mut song), "a click in the scrub area resumes it");
    // Follow again resumes it.
    edit(&mut song, 62);
    assert!(paused(&mut song));
    song.shell.run("arrangement.follow", json!({})).unwrap();
    assert!(!paused(&mut song), "Follow again resumes it");
    // Starting playback resumes it too.
    edit(&mut song, 63);
    assert!(paused(&mut song));
    song.play(100);
    assert!(!paused(&mut song), "playback starting resumes it");
}

// Spec: piano-roll R37.
#[test]
fn the_clip_markers_take_the_time_selection_the_insert_marker_or_the_play_position() {
    let mut song = spread();
    // A time selection: the Start marker and the loop's start take its
    // start, the loop's end and the End marker its end; each is one undo
    // step.
    song.select_time(BAR, 2 * BAR);
    song.one_step_marker("start", |midi| midi.start == Ticks(BAR));
    song.one_step_marker("end", |midi| midi.end == Some(Ticks(2 * BAR)));
    song.one_step_marker("loop_start", |midi| midi.loop_start == Ticks(BAR));
    song.one_step_marker("loop_end", |midi| midi.loop_end == Some(Ticks(2 * BAR)));
    // Only an insert marker: every marker takes it.
    song.insert_marker(Some(3 * BAR / 2));
    song.one_step_marker("start", |midi| midi.start == Ticks(3 * BAR / 2));
    // An order R21 forbids is refused, and records nothing.
    song.insert_marker(Some(0));
    let before = song.doc().clone();
    let steps = song.steps();
    let error = song
        .shell
        .run("clip_view.set_marker", json!({ "marker": "end" }))
        .unwrap_err();
    assert!(!error.to_string().is_empty());
    assert_eq!((*song.doc() == before, song.steps()), (true, steps));
    // Neither a selection nor a marker, and not playing: nothing to take.
    song.insert_marker(None);
    assert!(
        song.shell
            .run("clip_view.set_marker", json!({ "marker": "start" }))
            .is_err()
    );
}

impl Song {
    /// Sets a marker, checking it is one undo step that undo and redo
    /// restore.
    fn one_step_marker(&mut self, which: &str, holds: impl Fn(&MidiData) -> bool) {
        let before = self.doc().clone();
        let steps = self.steps();
        self.shell
            .run("clip_view.set_marker", json!({ "marker": which }))
            .unwrap();
        assert!(holds(&self.midi()));
        assert_eq!(self.steps(), steps + 1, "{which} is one undo step");
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before);
        self.shell.run("edit.redo", json!({})).unwrap();
        assert!(holds(&self.midi()));
    }
}

// Spec: piano-roll R37.
#[test]
fn while_the_clip_plays_a_marker_takes_the_play_position() {
    let mut song = spread();
    song.play(500);
    let before = song.shell.arrangement().transport().position;
    assert!(before > Ticks::ZERO, "the song is playing");
    song.shell
        .run("clip_view.set_marker", json!({ "marker": "loop_end" }))
        .unwrap();
    let after = song.shell.arrangement().transport().position;
    let end = song.midi().loop_end.expect("the loop's end was set");
    assert!(
        before <= end && end <= after,
        "{before:?} <= {end:?} <= {after:?}"
    );
}

// Spec: piano-roll R38.
#[test]
fn the_region_moves_by_the_region_command_and_cmd_click_sets_a_marker_in_one_step() {
    let mut song = spread();
    song.one_step(
        "clip.set_midi_region",
        json!({ "clip": song.clip, "start": BAR, "end": 3 * BAR }),
    );
    let midi = song.midi();
    assert_eq!((midi.start, midi.end), (Ticks(BAR), Some(Ticks(3 * BAR))));
    // The Start marker to a clicked place, in one step.
    let before = song.doc().clone();
    song.shell
        .run(
            "clip_view.set_marker",
            json!({ "marker": "start", "at": 3 * BAR / 2 }),
        )
        .unwrap();
    assert_eq!(song.midi().start, Ticks(3 * BAR / 2));
    song.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*song.doc(), before);
}

// Spec: piano-roll R39.
#[test]
fn duplicate_insert_and_delete_time_move_the_right_notes_and_leave_the_markers() {
    let notes = vec![
        note(60, 0, QUARTER),
        note(62, BAR / 2, BAR),
        note(64, BAR, QUARTER),
        note(65, BAR + QUARTER, 2 * QUARTER),
        note(67, 2 * BAR, QUARTER),
        note(69, 3 * BAR, QUARTER),
    ];
    let mut song = song(notes);
    song.one_step(
        "clip.set_midi_region",
        json!({ "clip": song.clip, "start": 0, "end": 3 * BAR }),
    );
    song.one_step(
        "clip.set_midi_loop",
        json!({ "clip": song.clip, "start": 0, "end": 2 * BAR }),
    );
    let markers = |midi: &MidiData| (midi.start, midi.end, midi.loop_start, midi.loop_end);
    let kept = markers(&song.midi());
    // With no time selection each is refused, and nothing changes.
    let before = song.doc().clone();
    let steps = song.steps();
    for id in [
        "note.duplicate_time",
        "note.insert_time",
        "note.delete_time",
    ] {
        assert!(song.shell.run(id, json!({})).is_err(), "{id}");
    }
    assert_eq!((*song.doc() == before, song.steps()), (true, steps));
    song.select_time(BAR, 2 * BAR);
    let starts = |song: &Song| -> Vec<(u8, i64, i64)> {
        song.notes()
            .iter()
            .map(|n| (n.key, n.start.get(), n.length.get()))
            .collect()
    };
    // Duplicate Time: a copy of the notes that start inside, straight
    // after; the later notes move on by the span's length; the copy is
    // selected as time.
    let result = song.one_step("note.duplicate_time", json!({}));
    assert_eq!(
        starts(&song),
        [
            (60, 0, QUARTER),
            (62, BAR / 2, BAR),
            (64, BAR, QUARTER),
            (65, BAR + QUARTER, 2 * QUARTER),
            (64, 2 * BAR, QUARTER),
            (65, 2 * BAR + QUARTER, 2 * QUARTER),
            (67, 3 * BAR, QUARTER),
            (69, 4 * BAR, QUARTER),
        ]
    );
    assert_eq!(result["notes"], json!([4, 5]), "the copies");
    assert_eq!(markers(&song.midi()), kept, "the markers stay");
    let state = song.show(json!({}));
    assert_eq!(
        state.time_selection,
        Some((Ticks(2 * BAR), Ticks(3 * BAR))),
        "the copy becomes the time selection"
    );
    song.shell.run("edit.undo", json!({})).unwrap();
    song.select_time(BAR, 2 * BAR);
    // Insert Time: empty time before the span's start.
    song.one_step("note.insert_time", json!({}));
    assert_eq!(
        starts(&song),
        [
            (60, 0, QUARTER),
            (62, BAR / 2, BAR),
            (64, 2 * BAR, QUARTER),
            (65, 2 * BAR + QUARTER, 2 * QUARTER),
            (67, 3 * BAR, QUARTER),
            (69, 4 * BAR, QUARTER),
        ]
    );
    assert_eq!(markers(&song.midi()), kept, "the markers stay");
    song.shell.run("edit.undo", json!({})).unwrap();
    // Delete Time: the notes that start inside go, a note that runs into
    // the span ends at its start, and the later notes close up.
    song.select_time(BAR, 2 * BAR);
    song.one_step("note.delete_time", json!({}));
    assert_eq!(
        starts(&song),
        [
            (60, 0, QUARTER),
            (62, BAR / 2, BAR / 2),
            (67, BAR, QUARTER),
            (69, 2 * BAR, QUARTER),
        ]
    );
    assert_eq!(markers(&song.midi()), kept, "the markers stay");
    let state: ClipViewState = serde_json::from_value(
        song.shell.run("view.state", json!({})).unwrap()["clip_view"].clone(),
    )
    .unwrap();
    assert_eq!(state.insert_marker, Some(Ticks(BAR)), "at the span's start");
    assert_eq!(state.time_selection, None, "with no time selected");
    // The span can be given, too.
    song.shell
        .run(
            "note.insert_time",
            json!({ "clip": song.clip, "start": 0, "end": QUARTER }),
        )
        .unwrap();
    assert_eq!(song.notes()[0].start, Ticks(QUARTER));
}

// Spec: piano-roll R39, R42.
#[test]
fn the_time_commands_are_registered_with_their_permission_classes() {
    let song = spread();
    let registry = song.shell.registry();
    for (id, class) in [
        ("note.duplicate_time", Permission::Edit),
        ("note.insert_time", Permission::Edit),
        ("note.delete_time", Permission::Destructive),
        ("clip_view.set_marker", Permission::Edit),
        ("clip_view.note_view", Permission::Read),
        ("arrangement.follow", Permission::Read),
        ("clip.set_midi_region", Permission::Edit),
        ("clip.set_midi_loop", Permission::Edit),
        ("clip.crop", Permission::Edit),
    ] {
        assert_eq!(registry.get(id).unwrap().permission, Some(class), "{id}");
    }
}

// Spec: piano-roll R40.
#[test]
fn crop_takes_the_note_editors_time_selection() {
    let mut song = song(vec![
        note(60, 0, QUARTER),
        note(62, BAR, QUARTER),
        note(64, 2 * BAR, 2 * BAR),
        note(65, 3 * BAR, QUARTER),
    ]);
    song.select_time(BAR, 3 * BAR);
    song.shell.run("clip.crop", json!({})).unwrap();
    // Only the notes that start inside the selection stay, counted from its
    // start, and the one that ran past its end is cut there.
    let kept: Vec<(i64, i64)> = song
        .notes()
        .iter()
        .map(|n| (n.start.get(), n.length.get()))
        .collect();
    assert_eq!(kept, [(0, QUARTER), (BAR, BAR)]);
    let clip = song.doc().clip(song.clip).unwrap();
    assert_eq!((clip.start, clip.length), (Ticks(BAR), Ticks(2 * BAR)));
}

// Spec: piano-roll R40.
#[test]
fn crop_a_session_clip_with_the_editors_time_selection_keeps_the_notes_inside_it() {
    let mut song = spread();
    let scene: SceneId = song.doc().scenes().next().unwrap().id;
    let slot: ClipId = song.shell.new_id();
    song.shell
        .run(
            "clip.create_in_slot",
            json!({
                "track": song.track,
                "scene": scene,
                "clip": {
                    "id": slot, "name": "", "start": 0, "length": 4 * BAR,
                    "offset": 0, "mute": false,
                    "data": { "midi": { "notes": [
                        { "key": 60, "velocity": 100, "start": 0, "length": QUARTER },
                        { "key": 62, "velocity": 100, "start": BAR, "length": QUARTER },
                        { "key": 64, "velocity": 100, "start": 2 * BAR, "length": QUARTER },
                        { "key": 65, "velocity": 100, "start": 3 * BAR, "length": QUARTER },
                    ] } },
                },
            }),
        )
        .unwrap();
    song.shell
        .run(
            "session.select",
            json!({ "track": song.track, "scene": scene }),
        )
        .unwrap();
    song.view(
        "clip_view.select_time",
        json!({ "clip": slot, "start": BAR, "end": 3 * BAR }),
    );
    song.shell.run("clip.crop", json!({})).unwrap();
    let ClipData::Midi(midi) = &song.doc().clip(slot).unwrap().data else {
        panic!("a MIDI clip");
    };
    let starts: Vec<i64> = notes_in_order(midi).iter().map(|n| n.start.get()).collect();
    assert_eq!(
        starts,
        [0, BAR],
        "the notes inside, counted from the selection"
    );
}
