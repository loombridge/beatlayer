//! Several MIDI clips in one note editor, driven headless through
//! `Shell::run` (WP-6.6; D17): the clips shown, the foreground clip, edits
//! across clips, Duplicate Loop, and Focus Mode. The behaviour is in the
//! multi-clip editing spec.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, Document, Editor, MidiData, Note, SceneId, TrackId,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::note_commands::{ClipViewState, NoteEditResult};
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

fn note(key: u8, start: i64) -> Note {
    Note::new(key, Ticks(start), Ticks(SIXTEENTH), 100)
}

/// A set of clips to open, built before the shell opens it.
struct Build {
    editor: Editor,
    tracks: Vec<TrackId>,
    scenes: Vec<SceneId>,
}

fn build() -> Build {
    let editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let scenes: Vec<SceneId> = editor.document().scenes().map(|scene| scene.id).collect();
    Build {
        editor,
        tracks,
        scenes,
    }
}

impl Build {
    fn midi(&mut self, length: i64, notes: Vec<Note>, looping: bool) -> Clip {
        let id: ClipId = self.editor.new_id();
        let mut data = MidiData::new(notes);
        data.looping = looping;
        Clip::new(id, Ticks::ZERO, Ticks(length), ClipData::Midi(data))
    }

    /// An Arrangement clip on track `track`, starting at `start`.
    fn arranged(&mut self, track: usize, start: i64, length: i64, notes: Vec<Note>) -> ClipId {
        let mut clip = self.midi(length, notes, false);
        clip.start = Ticks(start);
        let id = clip.id;
        let track = self.tracks[track];
        self.editor
            .apply(Command::CreateClip { track, clip })
            .unwrap();
        id
    }

    /// A Session clip in the slot of `track` and `scene`.
    fn slot(
        &mut self,
        track: usize,
        scene: usize,
        length: i64,
        notes: Vec<Note>,
        looping: bool,
    ) -> ClipId {
        let clip = self.midi(length, notes, looping);
        let id = clip.id;
        self.editor
            .apply(Command::CreateSlotClip {
                track: self.tracks[track],
                scene: self.scenes[scene],
                clip,
            })
            .unwrap();
        id
    }

    fn open(mut self) -> Song {
        self.editor.clear_history().unwrap();
        let mut shell = Shell::new(
            Preferences::default(),
            None,
            AudioHost::null(Default::default()),
            Box::new(Preloaded(Some(self.editor))),
        );
        shell
            .run("set.open", json!({ "path": "Multi.blset" }))
            .unwrap();
        shell.run("view.toggle_clip_view", json!({})).unwrap();
        Song {
            shell,
            tracks: self.tracks,
            scenes: self.scenes,
        }
    }
}

struct Song {
    shell: Shell,
    tracks: Vec<TrackId>,
    scenes: Vec<SceneId>,
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    fn notes(&self, clip: ClipId) -> Vec<Note> {
        match &self.doc().clip(clip).unwrap().data {
            ClipData::Midi(midi) => notes_in_order(midi),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn midi(&self, clip: ClipId) -> MidiData {
        match &self.doc().clip(clip).unwrap().data {
            ClipData::Midi(midi) => midi.clone(),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    fn edit(&mut self, id: &str, args: Value) -> NoteEditResult {
        serde_json::from_value(self.run(id, args)).unwrap()
    }

    fn view(&mut self, id: &str, args: Value) -> ClipViewState {
        serde_json::from_value(self.run(id, args)).unwrap()
    }

    fn state(&mut self) -> ClipViewState {
        let state: bl_shell::commands::ViewState =
            serde_json::from_value(self.run("view.state", json!({}))).unwrap();
        state.clip_view
    }

    /// Selects the Arrangement's clips.
    fn select_arranged(&mut self, clips: &[ClipId]) {
        self.run("arrangement.select_clips", json!({ "clips": clips }));
    }

    /// Selects the Session slots, the first by a click and the rest added
    /// by Cmd-clicks.
    fn select_slots(&mut self, slots: &[(usize, usize)]) {
        for (at, (track, scene)) in slots.iter().enumerate() {
            let mode = if at == 0 { "replace" } else { "add" };
            self.run(
                "session.select",
                json!({
                    "track": self.tracks[*track],
                    "scene": self.scenes[*scene],
                    "mode": mode,
                }),
            );
        }
    }
}

// Spec: multi-clip-editing R1, R2, R3, R5. Three looped Session clips of one,
// two and three bars show in track then scene order; the foreground clip is
// the one selected last; and on the shared axis they run six bars, with
// six, three and two passes.
#[test]
fn three_looped_session_clips_show_a_six_bar_span_with_their_passes() {
    let mut build = build();
    let one = build.slot(0, 0, BAR, vec![note(60, 0)], true);
    let two = build.slot(0, 1, 2 * BAR, vec![note(62, 0)], true);
    let three = build.slot(1, 0, 3 * BAR, vec![note(64, 0)], true);
    let mut song = build.open();
    song.select_slots(&[(1, 0), (0, 1), (0, 0)]);
    let state = song.state();
    assert_eq!(state.clips, [one, two, three]);
    assert_eq!(state.clip, Some(one), "the clip selected last");
    let layout = song
        .shell
        .clip_view()
        .layout(song.doc(), song.shell.arrangement());
    assert_eq!((layout.from, layout.to), (Ticks::ZERO, Ticks(6 * BAR)));
    assert_eq!(layout.windows(one).len(), 6);
    assert_eq!(layout.windows(two).len(), 3);
    assert_eq!(layout.windows(three).len(), 2);
    assert_eq!(layout.windows(two)[1].at, Ticks(2 * BAR));
}

// Spec: multi-clip-editing R5. The span is no longer than 64 times the
// longest loop.
#[test]
fn the_session_span_stops_at_64_times_the_longest_loop() {
    let mut build = build();
    let a = build.slot(0, 0, 97, vec![note(60, 0)], true);
    let b = build.slot(1, 0, 101, vec![note(62, 0)], true);
    let mut song = build.open();
    song.select_slots(&[(0, 0), (1, 0)]);
    let layout = song
        .shell
        .clip_view()
        .layout(song.doc(), song.shell.arrangement());
    assert_eq!(layout.to, Ticks(64 * 101));
    assert_eq!(layout.windows(a).len() as i64, 64 * 101 / 97);
    assert_eq!(layout.windows(b).len(), 64);
}

// Spec: multi-clip-editing R1. A Session selection holding an unlooped clip
// shows the clip selected last alone, and a ninth clip isn't shown.
#[test]
fn the_shown_clips_are_limited_as_the_spec_says() {
    let mut build = build();
    let looped = build.slot(0, 0, BAR, vec![note(60, 0)], true);
    let plain = build.slot(0, 1, BAR, vec![note(62, 0)], false);
    let mut song = build.open();
    song.select_slots(&[(0, 0), (0, 1)]);
    let state = song.state();
    assert_eq!(
        state.clips,
        [plain],
        "the unlooped clip, selected last, alone"
    );
    song.select_slots(&[(0, 0)]);
    assert_eq!(song.state().clips, [looped]);

    // Nine Arrangement clips on the two MIDI tracks: the first eight show.
    let mut build = self::build();
    let clips: Vec<ClipId> = (0..9)
        .map(|at| build.arranged(at % 2, (at as i64 / 2) * BAR, BAR, vec![note(60, 0)]))
        .collect();
    let mut song = build.open();
    song.select_arranged(&clips);
    let shown = song.state().clips;
    assert_eq!(shown.len(), 8);
    // Track then time: the first track's clips lead.
    let first_track = song.doc().clip_track(shown[0]).unwrap();
    assert_eq!(song.doc().clip_track(shown[1]), Some(first_track));
}

// Spec: multi-clip-editing R4. Two tracks' Arrangement clips show from the
// first start to the last end, each clip's notes where it plays them.
#[test]
fn two_tracks_arrangement_clips_show_from_the_first_start_to_the_last_end() {
    let mut build = build();
    let early = build.arranged(0, BAR, 2 * BAR, vec![note(60, 0)]);
    let late = build.arranged(1, 3 * BAR, 2 * BAR, vec![note(62, 0)]);
    let mut song = build.open();
    song.select_arranged(&[late, early]);
    let state = song.state();
    assert_eq!(state.clips, [early, late]);
    assert_eq!(state.clip, Some(early), "the clip selected last");
    let layout = song
        .shell
        .clip_view()
        .layout(song.doc(), song.shell.arrangement());
    assert_eq!((layout.from, layout.to), (Ticks(BAR), Ticks(5 * BAR)));
    let late_window = layout.windows(late)[0];
    assert_eq!(late_window.at, Ticks(3 * BAR));
    assert_eq!(
        (late_window.from, late_window.to),
        (Ticks::ZERO, Ticks(2 * BAR))
    );
}

// Spec: multi-clip-editing R3. Clicking a note or a loop bar makes its clip
// the foreground clip; the state reads it and the selected bars back.
#[test]
fn the_foreground_clip_and_the_loop_bars_are_view_state() {
    let mut build = build();
    let a = build.arranged(0, 0, BAR, vec![note(60, 0)]);
    let b = build.arranged(1, 0, BAR, vec![note(62, 0)]);
    let mut song = build.open();
    song.select_arranged(&[a, b]);
    let steps = song.steps();
    assert_eq!(song.state().clip, Some(b));
    assert_eq!(
        song.view("clip_view.foreground", json!({ "clip": a })).clip,
        Some(a)
    );
    let state = song.view("clip_view.select_loop_bars", json!({ "clips": [b] }));
    assert_eq!(state.loop_bars, [b]);
    assert_eq!(
        state.clip,
        Some(b),
        "a clicked bar's clip is the foreground clip"
    );
    let state = song.view(
        "clip_view.select_loop_bars",
        json!({ "clips": [a], "toggle": true }),
    );
    assert_eq!(state.loop_bars, [a, b]);
    assert_eq!(song.steps(), steps, "view state is no undo step");
    assert!(
        song.shell
            .run("clip_view.foreground", json!({ "clip": 99_999 }))
            .is_err()
    );
}

fn two_clips() -> (Song, ClipId, ClipId) {
    let mut build = build();
    let a = build.arranged(
        0,
        0,
        2 * BAR,
        vec![note(60, SIXTEENTH / 4), note(64, BAR + SIXTEENTH / 4)],
    );
    let b = build.arranged(
        1,
        0,
        2 * BAR,
        vec![note(67, SIXTEENTH / 4), note(71, 3 * SIXTEENTH)],
    );
    let mut song = build.open();
    song.select_arranged(&[a, b]);
    (song, a, b)
}

// Spec: multi-clip-editing R7, R18. Notes are named by clip; a quantise, a
// transpose and a delete across two clips are one undo step each and leave
// each note in its clip.
#[test]
fn edits_across_two_clips_are_one_undo_step_each_and_keep_each_note_in_its_clip() {
    let (mut song, a, b) = two_clips();
    let steps = song.steps();
    song.view(
        "clip_view.select_notes",
        json!({ "clips": [{ "clip": a, "notes": [0, 1] }, { "clip": b, "notes": [0] }] }),
    );
    let state = song.state();
    assert_eq!(state.clip_selections.len(), 2);
    assert_eq!(song.steps(), steps, "selecting is no undo step");

    let result = song.edit("note.quantize", json!({ "grid": SIXTEENTH, "amount": 100 }));
    assert_eq!(
        song.steps(),
        steps + 1,
        "a quantise across clips is one step"
    );
    assert_eq!(result.clips.len(), 2);
    assert_eq!(song.notes(a)[0].start, Ticks::ZERO);
    assert_eq!(song.notes(a)[1].start, Ticks(BAR));
    assert_eq!(song.notes(b)[0].start, Ticks::ZERO);
    assert_eq!(
        song.notes(b)[1].start,
        Ticks(3 * SIXTEENTH),
        "unselected, unmoved"
    );

    song.edit("note.move", json!({ "semitones": 2 }));
    assert_eq!(song.steps(), steps + 2);
    assert_eq!(
        song.notes(a).iter().map(|n| n.key).collect::<Vec<_>>(),
        [62, 66]
    );
    assert_eq!(
        song.notes(b).iter().map(|n| n.key).collect::<Vec<_>>(),
        [69, 71]
    );

    song.edit("note.delete", json!({}));
    assert_eq!(song.steps(), steps + 3);
    assert!(song.notes(a).is_empty());
    assert_eq!(song.notes(b).len(), 1);
    assert_eq!(
        song.notes(b)[0].key,
        71,
        "the unselected note stays in its clip"
    );

    // One undo undoes the delete in both clips.
    song.run("edit.undo", json!({}));
    assert_eq!(song.notes(a).len(), 2);
    assert_eq!(song.notes(b).len(), 2);
}

// Spec: multi-clip-editing R6, R18. A command that names its clips and notes
// edits only those; a clip named twice is refused.
#[test]
fn a_command_can_name_its_clips_and_notes() {
    let (mut song, a, b) = two_clips();
    song.edit(
        "note.adjust_value",
        json!({ "clips": [{ "clip": a, "notes": [1] }, { "clip": b, "notes": [0, 1] }],
                "field": "velocity", "by": -10 }),
    );
    assert_eq!(song.notes(a)[0].velocity, 100);
    assert_eq!(song.notes(a)[1].velocity, 90);
    assert_eq!(song.notes(b)[0].velocity, 90);
    assert_eq!(song.notes(b)[1].velocity, 90);
    assert!(
        song.shell
            .run(
                "note.delete",
                json!({ "clips": [{ "clip": a, "notes": [0] }, { "clip": a, "notes": [1] }] }),
            )
            .is_err()
    );
    assert!(
        song.shell
            .run(
                "note.delete",
                json!({ "clip": a, "clips": [{ "clip": b }] })
            )
            .is_err(),
        "`clips` takes the place of `clip` and `notes`"
    );
    assert_eq!(song.notes(a).len(), 2);
}

// Spec: multi-clip-editing R15, R13, R14. With Focus Mode on only the
// active clip changes, N switches it, and it acts only while several clips
// are shown.
#[test]
fn with_focus_mode_on_only_the_active_clip_changes() {
    let (mut song, a, b) = two_clips();
    song.view("clip_view.select_notes", json!({ "select": "all" }));
    assert_eq!(
        song.state().clip_selections.len(),
        2,
        "Select All reaches every clip shown"
    );
    let steps = song.steps();
    let state = song.view("clip_view.focus_mode", json!({}));
    assert!(state.focus_mode);
    assert_eq!(song.steps(), steps, "Focus Mode is no undo step");
    song.edit("note.move", json!({ "semitones": 1 }));
    // The foreground clip is b, the clip selected last.
    assert_eq!(
        song.notes(a).iter().map(|n| n.key).collect::<Vec<_>>(),
        [60, 64]
    );
    assert_eq!(
        song.notes(b).iter().map(|n| n.key).collect::<Vec<_>>(),
        [68, 72]
    );
    song.view("clip_view.foreground", json!({ "clip": a }));
    song.view("clip_view.select_notes", json!({ "select": "all" }));
    let state = song.state();
    assert_eq!(
        state
            .clip_selections
            .iter()
            .map(|c| c.clip)
            .collect::<Vec<_>>(),
        [a],
        "selecting in Focus Mode reaches the active clip alone"
    );
    // Only one loop bar can be selected; a selection of several made before
    // is set aside until Focus Mode is off.
    let state = song.view("clip_view.select_loop_bars", json!({ "clips": [a, b] }));
    assert_eq!(state.loop_bars, [b]);
    song.view("clip_view.focus_mode", json!({ "on": false }));
    song.view("clip_view.select_loop_bars", json!({ "clips": [a, b] }));
    assert_eq!(song.state().loop_bars, [a, b]);
    let state = song.view("clip_view.focus_mode", json!({ "on": true }));
    assert!(state.focus_mode);
    assert_eq!(state.loop_bars, [b], "the foreground clip's bar alone");
    let state = song.view("clip_view.focus_mode", json!({ "on": false }));
    assert!(!state.focus_mode);
    assert_eq!(state.loop_bars, [a, b]);
    // With one clip shown, Focus Mode does nothing.
    song.select_arranged(&[a]);
    song.view("clip_view.focus_mode", json!({ "on": true }));
    song.view("clip_view.select_notes", json!({ "select": "all" }));
    song.edit("note.move", json!({ "semitones": 1 }));
    assert_eq!(
        song.notes(a).iter().map(|n| n.key).collect::<Vec<_>>(),
        [61, 65]
    );
}

// Spec: multi-clip-editing R9. The Loop switch, the loop's place and length
// and the Signature change every clip they're given, in one undo step.
#[test]
fn loop_and_signature_changes_reach_every_clip_in_one_step() {
    let (mut song, a, b) = two_clips();
    let steps = song.steps();
    song.run(
        "clip.set_loop_enabled",
        json!({ "clips": [a, b], "enabled": true }),
    );
    assert_eq!(song.steps(), steps + 1);
    assert!(song.midi(a).looping && song.midi(b).looping);
    song.run(
        "clip.set_midi_loop",
        json!({ "clips": [a, b], "start": 0, "end": BAR }),
    );
    assert_eq!(song.steps(), steps + 2);
    assert_eq!(song.midi(a).loop_end, Some(Ticks(BAR)));
    assert_eq!(song.midi(b).loop_end, Some(Ticks(BAR)));
    song.run(
        "clip.set_signature",
        json!({ "clips": [a, b], "signature": { "numerator": 3, "denominator": 4 } }),
    );
    assert_eq!(song.steps(), steps + 3);
    assert_eq!(song.midi(a).signature, song.midi(b).signature);
    assert!(song.midi(a).signature.is_some());
    song.run("clip.set_mute", json!({ "clips": [a, b], "mute": true }));
    assert!(song.doc().clip(a).unwrap().mute && song.doc().clip(b).unwrap().mute);
    song.run("clip.rename", json!({ "clips": [a, b], "name": "Same" }));
    assert_eq!(song.doc().clip(b).unwrap().name, "Same");
    // One undo undoes the name in both.
    song.run("edit.undo", json!({}));
    assert_eq!(song.doc().clip(a).unwrap().name, "");
    assert_eq!(song.doc().clip(b).unwrap().name, "");
    // A refused clip leaves the others as they were.
    let steps = song.steps();
    assert!(
        song.shell
            .run(
                "clip.set_midi_loop",
                json!({ "clips": [a, b], "start": BAR, "end": BAR }),
            )
            .is_err()
    );
    assert_eq!(song.steps(), steps);
}

// Spec: multi-clip-editing R11. Duplicate Loop doubles each chosen clip's
// loop and copies its notes into the new half, in one step; a single clip's
// loop brace has it too.
#[test]
fn duplicate_loop_doubles_each_loop_and_its_notes_in_one_step() {
    let mut build = build();
    let a = build.arranged(
        0,
        0,
        4 * BAR,
        vec![note(60, 0), note(62, BAR / 2), note(64, 2 * BAR + 10)],
    );
    let b = build.arranged(1, 0, 4 * BAR, vec![note(67, 0)]);
    let mut song = build.open();
    song.run(
        "clip.set_midi_loop",
        json!({ "clips": [a, b], "start": 0, "end": BAR }),
    );
    song.select_arranged(&[a, b]);
    song.view("clip_view.select_loop_bars", json!({ "clips": [a, b] }));
    let steps = song.steps();
    song.run("clip.duplicate_loop", json!({}));
    assert_eq!(song.steps(), steps + 1, "one undo step for every clip");
    assert_eq!(
        song.notes(a)
            .iter()
            .map(|n| (n.key, n.start.get()))
            .collect::<Vec<_>>(),
        [
            (60, 0),
            (62, BAR / 2),
            (60, BAR),
            (62, BAR + BAR / 2),
            (64, 3 * BAR + 10)
        ],
        "the loop's notes are copied, and the note after the loop keeps its distance from the end"
    );
    assert_eq!(song.notes(b).len(), 2);
    assert_eq!(song.midi(a).loop_end, Some(Ticks(2 * BAR)));
    assert_eq!(song.midi(b).loop_end, Some(Ticks(2 * BAR)));
    song.run("edit.undo", json!({}));
    assert_eq!(song.notes(a).len(), 3);
    assert_eq!(song.midi(b).loop_end, Some(Ticks(BAR)));

    // A single clip's loop brace doubles too.
    song.select_arranged(&[b]);
    song.run("clip.duplicate_loop", json!({}));
    assert_eq!(song.midi(b).loop_end, Some(Ticks(2 * BAR)));
    assert_eq!(song.midi(a).loop_end, Some(Ticks(BAR)));
    // The clip grows when the doubled loop wouldn't fit.
    song.run("clip.duplicate_loop", json!({}));
    song.run("clip.duplicate_loop", json!({}));
    assert_eq!(song.midi(b).loop_end, Some(Ticks(8 * BAR)));
    assert_eq!(song.doc().clip(b).unwrap().length, Ticks(8 * BAR));
}

// Spec: multi-clip-editing R12. Notes cut or copied from several clips paste
// back into the same clips while the clips shown and the foreground clip are
// the same; otherwise they all go into the foreground clip.
#[test]
fn paste_goes_back_into_the_same_clips_or_else_the_foreground_clip() {
    let (mut song, a, b) = two_clips();
    song.view(
        "clip_view.select_notes",
        json!({ "clips": [{ "clip": a, "notes": [0] }, { "clip": b, "notes": [0] }] }),
    );
    let copied = song.run("note.copy", json!({}));
    assert_eq!(copied["notes"], 2);
    song.run(
        "clip_view.set_insert_marker",
        json!({ "beat": BAR + BAR / 2 }),
    );
    let steps = song.steps();
    let result = song.edit("note.paste", json!({}));
    assert_eq!(song.steps(), steps + 1, "one undo step for both clips");
    assert_eq!(result.clips.len(), 2);
    let pasted = |song: &Song, clip, key| {
        song.notes(clip)
            .iter()
            .find(|n| n.key == key && n.start == Ticks(BAR + BAR / 2))
            .is_some()
    };
    assert!(
        pasted(&song, a, 60) && pasted(&song, b, 67),
        "each into its own clip"
    );
    assert_eq!(song.notes(a).len(), 3);
    assert!(!pasted(&song, a, 67) && !pasted(&song, b, 60));

    // With another clip in the foreground, everything goes into it.
    song.view("clip_view.foreground", json!({ "clip": a }));
    song.run("clip_view.set_insert_marker", json!({ "beat": BAR }));
    song.edit("note.paste", json!({}));
    assert_eq!(song.notes(a).len(), 5);
    assert_eq!(song.notes(b).len(), 3, "the other clip is left alone");
}

// Spec: multi-clip-editing R12. Cut copies the notes of every clip and
// deletes them, as one undo step.
#[test]
fn cut_takes_the_notes_of_every_clip_in_one_step() {
    let (mut song, a, b) = two_clips();
    song.view(
        "clip_view.select_notes",
        json!({ "clips": [{ "clip": a, "notes": [1] }, { "clip": b, "notes": [1] }] }),
    );
    let steps = song.steps();
    song.edit("note.cut", json!({}));
    assert_eq!(song.steps(), steps + 1);
    assert_eq!(song.notes(a).len(), 1);
    assert_eq!(song.notes(b).len(), 1);
    song.run("clip_view.set_insert_marker", json!({ "beat": BAR }));
    song.edit("note.paste", json!({}));
    assert_eq!(song.notes(a).len(), 2);
    assert_eq!(song.notes(b).len(), 2);
}

// Spec: multi-clip-editing R18. `view.state` reads the clips shown, the
// foreground clip, the bars, Focus Mode and each clip's selected notes.
#[test]
fn view_state_reads_the_clips_back() {
    let (mut song, a, b) = two_clips();
    song.view(
        "clip_view.select_notes",
        json!({ "clips": [{ "clip": a, "notes": [1] }] }),
    );
    song.view("clip_view.select_loop_bars", json!({ "clips": [a] }));
    song.view("clip_view.focus_mode", json!({ "on": true }));
    let state = song.state();
    assert_eq!(state.clips, [a, b]);
    assert_eq!(state.clip, Some(a));
    assert_eq!(state.loop_bars, [a]);
    assert!(state.focus_mode);
    assert_eq!(state.clip_selections.len(), 1);
    assert_eq!(state.clip_selections[0].selected, [1]);
    assert_eq!(state.selected, [1], "the foreground clip's selection");
}

// Spec: multi-clip-editing R6. A note shown in any pass of its clip's loop is
// that one note: the clip holds it once, and every pass of the layout shows
// it.
#[test]
fn a_note_in_a_later_pass_is_the_clips_one_note() {
    let mut build = build();
    let a = build.arranged(0, 0, 4 * BAR, vec![note(60, 0)]);
    let b = build.arranged(1, 0, 4 * BAR, vec![note(62, 0)]);
    let mut song = build.open();
    song.run(
        "clip.set_midi_loop",
        json!({ "clips": [a, b], "start": 0, "end": BAR }),
    );
    song.run(
        "clip.set_loop_enabled",
        json!({ "clips": [a, b], "enabled": true }),
    );
    song.select_arranged(&[a, b]);
    let layout = song
        .shell
        .clip_view()
        .layout(song.doc(), song.shell.arrangement());
    let passes = layout.windows(a);
    assert_eq!(passes.len(), 4, "four passes of the one-bar loop");
    assert_eq!(passes[3].at, Ticks(3 * BAR));
    assert_eq!(song.notes(a).len(), 1);
    // Editing it by its place changes it in every pass.
    song.edit(
        "note.move",
        json!({ "clip": a, "notes": [0], "semitones": 3 }),
    );
    assert_eq!(song.notes(a)[0].key, 63);
    assert_eq!(song.notes(a).len(), 1);
}

// Spec: multi-clip-editing R7, R8. Notes selected in two clips can't share a
// probability group, and an edit reaches the notes selected in a clip that
// isn't the foreground clip.
#[test]
fn grouping_across_clips_is_refused_and_an_edit_reaches_a_background_clip() {
    let (mut song, a, b) = two_clips();
    song.view(
        "clip_view.select_notes",
        json!({ "clips": [{ "clip": a, "notes": [0] }, { "clip": b, "notes": [0] }] }),
    );
    let steps = song.steps();
    assert!(
        song.shell
            .run("note.group", json!({ "kind": "play_all" }))
            .is_err()
    );
    assert_eq!(song.steps(), steps, "nothing grouped");
    assert!(song.midi(a).groups.is_empty() && song.midi(b).groups.is_empty());

    // The foreground clip is b, the one selected last; a's notes alone are
    // selected.
    song.view(
        "clip_view.select_notes",
        json!({ "clips": [{ "clip": a, "notes": [0, 1] }] }),
    );
    song.edit("note.move", json!({ "semitones": 1 }));
    assert_eq!(
        song.notes(a).iter().map(|n| n.key).collect::<Vec<_>>(),
        [61, 65]
    );
    assert_eq!(
        song.notes(b).iter().map(|n| n.key).collect::<Vec<_>>(),
        [67, 71]
    );
    // Within one clip they group.
    song.edit(
        "note.group",
        json!({ "clip": a, "notes": [0, 1], "kind": "play_all" }),
    );
    assert_eq!(song.midi(a).groups.len(), 1);
}

// Spec: multi-clip-editing R11; note-editing R31. Duplicate Loop puts the
// copied notes in a group of their own.
#[test]
fn duplicate_loop_gives_the_copies_groups_of_their_own() {
    let mut build = build();
    let a = build.arranged(0, 0, 2 * BAR, vec![note(60, 0), note(64, BAR / 2)]);
    let mut song = build.open();
    song.run(
        "clip.set_midi_loop",
        json!({ "clips": [a], "start": 0, "end": BAR }),
    );
    song.select_arranged(&[a]);
    song.edit(
        "note.group",
        json!({ "clip": a, "notes": [0, 1], "kind": "play_one" }),
    );
    song.run("clip.duplicate_loop", json!({ "clips": [a] }));
    let midi = song.midi(a);
    assert_eq!(midi.groups.len(), 2, "{:?}", midi.groups);
    let notes = song.notes(a);
    assert_eq!(notes.len(), 4);
    assert_eq!(notes[0].group, notes[1].group);
    assert_eq!(notes[2].group, notes[3].group);
    assert_ne!(notes[0].group, notes[2].group);
}
