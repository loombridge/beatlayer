//! Clips between the Session and the Arrangement, and the Session View's
//! Cut, Copy, Paste and Duplicate, driven headless through `Shell::run`, as
//! agents and keys drive them (WP-6.29b). Each is one undo step, and what
//! lands on a slot or a lane replaces what was there. The behaviour is in the
//! second window spec, R18 to R23, and the session view spec, R39 to R43.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, Editor, MidiData, SceneId,
    TrackId,
};
use bl_shell::audio::AudioHost;
use bl_shell::clip_drag::{ClipDragEvent, DragSource, DropSpot};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::session_view_commands::{self as ids, CopiedClips, SessionClipboardState};
use bl_shell::settings::Preferences;
use bl_shell::{Shell, WindowKind};
use serde_json::{Value, json};

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

/// The factory default set (tracks 0 and 1 MIDI, 2 and 3 audio, two returns,
/// eight scenes) with the clips a test names: Session clips by `(track,
/// scene, bars, name)` and Arrangement clips by `(track, start bar, bars,
/// name)`.
struct Song {
    shell: Shell,
    tracks: Vec<TrackId>,
    scenes: Vec<SceneId>,
}

fn clip(editor: &mut Editor, audio: bool, bars: i64, name: &str, start: i64) -> Clip {
    let id: ClipId = editor.new_id();
    let data = if audio {
        ClipData::Audio(AudioData::new(ContentHash([name.len() as u8; 32])))
    } else {
        ClipData::Midi(MidiData::new(std::iter::empty()))
    };
    let mut clip = Clip::new(id, Ticks(start * BAR), Ticks(bars * BAR), data);
    clip.name = name.to_owned();
    clip
}

fn song(slots: &[(usize, usize, i64, &str)], lane: &[(usize, i64, i64, &str)]) -> Song {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let scenes: Vec<SceneId> = editor.document().scenes().map(|s| s.id).collect();
    for (track, scene, bars, name) in slots {
        let clip = clip(&mut editor, *track >= 2, *bars, name, 0);
        editor
            .apply(Command::CreateSlotClip {
                track: tracks[*track],
                scene: scenes[*scene],
                clip,
            })
            .unwrap();
    }
    for (track, start, bars, name) in lane {
        let clip = clip(&mut editor, *track >= 2, *bars, name, *start);
        editor
            .apply(Command::CreateClip {
                track: tracks[*track],
                clip,
            })
            .unwrap();
    }
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Clips.blset" }))
        .unwrap();
    Song {
        shell,
        tracks,
        scenes,
    }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    /// The named clip in a slot, by track and scene numbers from 0.
    fn slot(&self, track: usize, scene: usize) -> Option<String> {
        self.doc()
            .slot(self.tracks[track], self.scenes[scene])
            .map(|clip| clip.name.clone())
    }

    /// A track's Arrangement clips: `(start bar, bars, name)`, by start.
    fn lane(&self, track: usize) -> Vec<(i64, i64, String)> {
        let mut clips: Vec<_> = self
            .doc()
            .track(self.tracks[track])
            .unwrap()
            .clips()
            .map(|c| (c.start.get() / BAR, c.length.get() / BAR, c.name.clone()))
            .collect();
        clips.sort();
        clips
    }

    /// The Session clip's ID, by track and scene numbers from 0.
    fn slot_clip(&self, track: usize, scene: usize) -> ClipId {
        self.doc()
            .slot(self.tracks[track], self.scenes[scene])
            .unwrap()
            .id
    }

    /// The Arrangement clip's ID, by track number and name.
    fn named(&self, track: usize, name: &str) -> ClipId {
        self.doc()
            .track(self.tracks[track])
            .unwrap()
            .clips()
            .find(|c| c.name == name)
            .unwrap()
            .id
    }

    fn select_slots(&mut self, slots: &[(usize, usize)]) {
        for (n, (track, scene)) in slots.iter().enumerate() {
            let mode = if n == 0 { "replace" } else { "add" };
            self.run(
                ids::SELECT,
                json!({ "track": self.tracks[*track], "scene": self.scenes[*scene], "mode": mode }),
            );
        }
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    /// Runs a command, checks it's exactly one undo step named `label`, and
    /// that undo and redo restore the set exactly, then leaves it done.
    fn one_step(&mut self, id: &str, args: Value, label: &str) -> Value {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self.run(id, args);
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        assert_eq!(
            self.shell.editor().undo_history().next(),
            Some(label),
            "{id}'s step"
        );
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        assert!(self.doc().check_invariants().is_ok());
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }
}

fn bars(at: i64) -> Value {
    json!(at * BAR)
}

// Spec: second-window R20, R23. One Session clip copied into the Arrangement
// starts at the position on the track, keeps what it holds, leaves its slot as
// it was, and is one undo step.
#[test]
fn a_session_clip_is_copied_to_the_arrangement_at_the_position() {
    let mut song = song(&[(0, 1, 2, "Riff")], &[]);
    let source = song.doc().slot(song.tracks[0], song.scenes[1]).cloned();
    let clip = song.slot_clip(0, 1);
    song.one_step(
        ids::COPY_TO_ARRANGEMENT,
        json!({ "clips": [clip], "track": song.tracks[0], "at": bars(2) }),
        "Copy Clips to Arrangement",
    );
    assert_eq!(song.lane(0), [(2, 2, "Riff".to_owned())]);
    assert_eq!(
        song.doc().slot(song.tracks[0], song.scenes[1]).cloned(),
        source,
        "the slot stays as it was"
    );
}

// Spec: second-window R20. Without arguments it copies the selected slots to
// the selected track at the insert marker.
#[test]
fn without_arguments_it_copies_the_selection_to_the_selected_track_at_the_insert_marker() {
    let mut song = song(&[(0, 0, 1, "Riff")], &[]);
    song.select_slots(&[(0, 0)]);
    song.run(
        "arrangement.select_tracks",
        json!({ "tracks": [song.tracks[1]] }),
    );
    song.run("arrangement.set_insert_marker", json!({ "at": bars(3) }));
    song.run(ids::COPY_TO_ARRANGEMENT, json!({}));
    assert_eq!(song.lane(1), [(3, 1, "Riff".to_owned())]);
}

// Spec: second-window R20. Clips of three tracks land on the tracks as far
// down as they were across the grid, each starting at the position.
#[test]
fn clips_of_three_tracks_keep_their_distance_across_the_tracks() {
    let mut song = song(&[(0, 0, 1, "A"), (1, 0, 2, "B"), (2, 1, 1, "C")], &[]);
    let clips: Vec<ClipId> = [(0, 0), (1, 0), (2, 1)]
        .iter()
        .map(|(t, s)| song.slot_clip(*t, *s))
        .collect();
    song.one_step(
        ids::COPY_TO_ARRANGEMENT,
        json!({ "clips": clips, "track": song.tracks[0], "at": bars(1) }),
        "Copy Clips to Arrangement",
    );
    assert_eq!(song.lane(0), [(1, 1, "A".to_owned())]);
    assert_eq!(song.lane(1), [(1, 2, "B".to_owned())]);
    // The third is an audio clip, two tracks down.
    assert_eq!(song.lane(2), [(1, 1, "C".to_owned())]);
}

// Spec: second-window R20. One track's clips from several scenes follow one
// another in scene order, each as long as it is.
#[test]
fn one_tracks_clips_from_three_scenes_land_end_to_end() {
    let mut song = song(&[(0, 0, 1, "A"), (0, 2, 2, "B"), (0, 3, 1, "C")], &[]);
    let clips: Vec<ClipId> = [0, 2, 3].iter().map(|s| song.slot_clip(0, *s)).collect();
    song.one_step(
        ids::COPY_TO_ARRANGEMENT,
        json!({ "clips": clips, "track": song.tracks[0], "at": bars(2) }),
        "Copy Clips to Arrangement",
    );
    assert_eq!(
        song.lane(0),
        [
            (2, 1, "A".to_owned()),
            (3, 2, "B".to_owned()),
            (5, 1, "C".to_owned())
        ]
    );
}

// Spec: second-window R20. Clips that would land on a track of the other
// kind, or past the last track, aren't placed, and the Status Bar counts them.
#[test]
fn clips_for_a_track_of_the_wrong_kind_are_skipped_and_counted() {
    let mut song = song(&[(0, 0, 1, "A"), (1, 0, 1, "B"), (2, 0, 1, "C")], &[]);
    let clips: Vec<ClipId> = [(0, 0), (1, 0), (2, 0)]
        .iter()
        .map(|(t, s)| song.slot_clip(*t, *s))
        .collect();
    // Starting on audio track 2: A is MIDI on an audio track, B is MIDI on
    // the last audio track, and C falls past the last track.
    let result: CopiedClips = serde_json::from_value(song.run(
        ids::COPY_TO_ARRANGEMENT,
        json!({ "clips": clips, "track": song.tracks[2], "at": bars(0) }),
    ))
    .unwrap();
    assert_eq!(result.skipped, 3);
    assert!(song.lane(2).is_empty() && song.lane(3).is_empty());
    assert_eq!(
        song.shell.message().map(|m| m.text.as_str()),
        Some(
            "3 clips weren't placed: they would land on tracks of another kind or past the last \
             track"
        )
    );
    // Starting on MIDI track 1: A is MIDI and goes on a MIDI track; B and C
    // follow: B on audio track 2 (skipped), C's audio on the last (placed).
    let result: CopiedClips = serde_json::from_value(song.run(
        ids::COPY_TO_ARRANGEMENT,
        json!({ "clips": clips, "track": song.tracks[1], "at": bars(0) }),
    ))
    .unwrap();
    assert_eq!(result.skipped, 1, "{result:?}");
    assert_eq!(song.lane(1), [(0, 1, "A".to_owned())]);
    assert_eq!(song.lane(3), [(0, 1, "C".to_owned())]);
}

// Spec: second-window R20, arrangement-editing R6. The copies replace what
// they cover.
#[test]
fn the_copies_replace_what_they_cover() {
    let mut song = song(&[(0, 0, 2, "New")], &[(0, 0, 4, "Old")]);
    let clip = song.slot_clip(0, 0);
    song.run(
        ids::COPY_TO_ARRANGEMENT,
        json!({ "clips": [clip], "track": song.tracks[0], "at": bars(1) }),
    );
    assert_eq!(
        song.lane(0),
        [
            (0, 1, "Old".to_owned()),
            (1, 2, "New".to_owned()),
            (3, 1, "Old".to_owned())
        ]
    );
}

// Spec: second-window R21, R23. One track's Arrangement clips go down the
// scenes in the order they play, and the Arrangement stays as it was.
#[test]
fn arrangement_clips_of_one_track_go_down_the_scenes_in_the_order_they_play() {
    let mut song = song(
        &[],
        &[(0, 4, 1, "Third"), (0, 0, 1, "First"), (0, 2, 1, "Second")],
    );
    let before: Vec<_> = song.lane(0);
    let clips: Vec<ClipId> = ["Third", "First", "Second"]
        .iter()
        .map(|n| song.named(0, n))
        .collect();
    song.one_step(
        ids::COPY_TO_SESSION,
        json!({ "clips": clips, "track": song.tracks[0], "scene": song.scenes[1] }),
        "Copy Clips to Session",
    );
    assert_eq!(song.slot(0, 1).as_deref(), Some("First"));
    assert_eq!(song.slot(0, 2).as_deref(), Some("Second"));
    assert_eq!(song.slot(0, 3).as_deref(), Some("Third"));
    assert_eq!(song.lane(0), before, "the Arrangement stays as it was");
}

// Spec: second-window R21. The clips of two tracks go across the tracks, one
// clip a slot, each track's from the same scene.
#[test]
fn arrangement_clips_of_two_tracks_go_across_the_tracks() {
    let mut song = song(&[], &[(0, 0, 1, "A1"), (0, 1, 1, "A2"), (1, 0, 1, "B1")]);
    let clips: Vec<ClipId> = [(0, "A1"), (0, "A2"), (1, "B1")]
        .iter()
        .map(|(t, n)| song.named(*t, n))
        .collect();
    song.run(
        ids::COPY_TO_SESSION,
        json!({ "clips": clips, "track": song.tracks[0], "scene": song.scenes[0] }),
    );
    assert_eq!(song.slot(0, 0).as_deref(), Some("A1"));
    assert_eq!(song.slot(0, 1).as_deref(), Some("A2"));
    assert_eq!(song.slot(1, 0).as_deref(), Some("B1"));
    assert_eq!(song.slot(1, 1), None);
}

// Spec: second-window R21. Scenes are added at the end when the clips run past
// the last one, and a full slot's clip is replaced.
#[test]
fn scenes_are_added_at_the_end_and_full_slots_are_replaced() {
    let mut song = song(
        &[(0, 6, 1, "Old")],
        &[(0, 0, 1, "A"), (0, 1, 1, "B"), (0, 2, 1, "C")],
    );
    let clips: Vec<ClipId> = ["A", "B", "C"].iter().map(|n| song.named(0, n)).collect();
    let scenes = song.scenes.len();
    song.run(
        ids::COPY_TO_SESSION,
        json!({ "clips": clips, "track": song.tracks[0], "scene": song.scenes[6] }),
    );
    let after: Vec<SceneId> = song.doc().scenes().map(|s| s.id).collect();
    assert_eq!(after.len(), scenes + 1, "scene 9 is added");
    let slot = |scene: usize| {
        song.doc()
            .slot(song.tracks[0], after[scene])
            .map(|c| c.name.clone())
    };
    assert_eq!(
        slot(6).as_deref(),
        Some("A"),
        "the full slot's clip is replaced"
    );
    assert_eq!(slot(7).as_deref(), Some("B"));
    assert_eq!(slot(8).as_deref(), Some("C"));
}

// Spec: second-window R21. A clip for a track of the other kind takes no slot.
#[test]
fn arrangement_clips_for_the_wrong_kind_of_track_take_no_slot() {
    let mut song = song(&[], &[(0, 0, 1, "Midi"), (2, 0, 1, "Audio")]);
    let clips = [song.named(0, "Midi"), song.named(2, "Audio")];
    // From audio track 2: the MIDI clip (first track) lands on audio, and the
    // audio clip two tracks down is past the last track.
    let result: CopiedClips = serde_json::from_value(song.run(
        ids::COPY_TO_SESSION,
        json!({ "clips": clips, "track": song.tracks[2], "scene": song.scenes[0] }),
    ))
    .unwrap();
    assert_eq!(result.skipped, 2);
    assert_eq!(song.slot(2, 0), None);
}

// Spec: second-window R22, session-view R39. Session clips copied and pasted
// in the Arrangement land at the insert marker on the selected track.
#[test]
fn session_clips_pasted_in_the_arrangement_land_at_the_insert_marker_on_the_selected_track() {
    let mut song = song(&[(0, 0, 1, "A"), (0, 1, 2, "B")], &[]);
    song.select_slots(&[(0, 0), (0, 1)]);
    let state: SessionClipboardState =
        serde_json::from_value(song.run(ids::COPY, json!({}))).unwrap();
    assert_eq!(state.clips, 2);
    song.run(
        "arrangement.select_tracks",
        json!({ "tracks": [song.tracks[1]] }),
    );
    song.run("arrangement.set_insert_marker", json!({ "at": bars(4) }));
    let before = song.doc().clone();
    song.one_step("arrangement.paste", json!({}), "Copy Clips to Arrangement");
    assert_eq!(
        song.lane(1),
        [(4, 1, "A".to_owned()), (5, 2, "B".to_owned())]
    );
    assert_eq!(song.slot(0, 0).as_deref(), Some("A"));
    assert!(before.slot(song.tracks[0], song.scenes[0]).is_some());
}

// Spec: second-window R22. Arrangement clips copied and pasted in the Session
// View go down the scenes from the selected slot.
#[test]
fn arrangement_clips_pasted_in_the_session_view_start_at_the_selected_slot() {
    let mut song = song(&[], &[(0, 0, 1, "A"), (0, 2, 1, "B")]);
    let clips = [song.named(0, "A"), song.named(0, "B")];
    song.run("arrangement.select_clips", json!({ "clips": clips }));
    song.run("arrangement.copy", json!({}));
    song.select_slots(&[(1, 3)]);
    song.one_step(ids::PASTE, json!({}), "Copy Clips to Session");
    assert_eq!(song.slot(1, 3).as_deref(), Some("A"));
    assert_eq!(song.slot(1, 4).as_deref(), Some("B"));
}

// Spec: session-view R39, R40. Copy and Paste of slot clips keep their places
// across the tracks and down the scenes from the selected slot.
#[test]
fn slot_clips_are_copied_and_pasted_keeping_their_places() {
    let mut song = song(&[(0, 0, 1, "A"), (1, 1, 1, "B")], &[]);
    song.select_slots(&[(0, 0), (1, 1)]);
    song.run(ids::COPY, json!({}));
    song.select_slots(&[(0, 4)]);
    song.one_step(ids::PASTE, json!({}), "Paste");
    assert_eq!(song.slot(0, 4).as_deref(), Some("A"));
    assert_eq!(song.slot(1, 5).as_deref(), Some("B"));
    assert_eq!(song.slot(0, 0).as_deref(), Some("A"), "the source stays");
}

// Spec: session-view R39. Cut copies the selected slot clips and deletes
// them, in one undo step; the clipboard keeps them.
#[test]
fn cut_removes_the_clips_and_paste_puts_them_back() {
    let mut song = song(&[(0, 0, 1, "A")], &[]);
    song.select_slots(&[(0, 0)]);
    song.one_step(ids::CUT, json!({}), "Cut");
    assert_eq!(song.slot(0, 0), None);
    song.select_slots(&[(1, 2)]);
    song.run(ids::PASTE, json!({}));
    assert_eq!(song.slot(1, 2).as_deref(), Some("A"));
}

// Spec: session-view R40. A pasted clip for a track of the other kind isn't
// placed.
#[test]
fn a_paste_skips_clips_for_tracks_of_the_wrong_kind() {
    let mut song = song(&[(0, 0, 1, "A")], &[]);
    song.select_slots(&[(0, 0)]);
    song.run(ids::COPY, json!({}));
    song.select_slots(&[(2, 0)]);
    let result: CopiedClips = serde_json::from_value(song.run(ids::PASTE, json!({}))).unwrap();
    assert_eq!(result.skipped, 1);
    assert_eq!(song.slot(2, 0), None);
}

// Spec: session-view R43. Duplicate puts copies straight below the
// selection's rows, replacing what's there, and adds scenes at the end.
#[test]
fn duplicate_puts_copies_below_the_selection() {
    let mut song = song(&[(0, 6, 1, "A"), (1, 7, 1, "B")], &[]);
    song.select_slots(&[(0, 6), (1, 7)]);
    let scenes = song.scenes.len();
    song.one_step(ids::DUPLICATE, json!({}), "Duplicate");
    let after: Vec<SceneId> = song.doc().scenes().map(|s| s.id).collect();
    assert_eq!(after.len(), scenes + 2);
    let slot = |track: usize, scene: usize| {
        song.doc()
            .slot(song.tracks[track], after[scene])
            .map(|c| c.name.clone())
    };
    assert_eq!(slot(0, 8).as_deref(), Some("A"));
    assert_eq!(slot(1, 9).as_deref(), Some("B"));
}

// Spec: session-view R41. Scenes are copied with their clips, name and
// tempo and pasted as new scenes after the selected scene; Cut removes them.
#[test]
fn scenes_are_copied_cut_and_pasted() {
    let mut song = song(&[(0, 1, 1, "A"), (2, 1, 1, "C")], &[]);
    song.run(
        "scene.rename",
        json!({ "scene": song.scenes[1], "name": "Verse" }),
    );
    song.run(
        "scene.set_tempo",
        json!({ "scene": song.scenes[1], "tempo": 90.0 }),
    );
    song.run(ids::SELECT, json!({ "scene": song.scenes[1] }));
    let state: SessionClipboardState =
        serde_json::from_value(song.run(ids::COPY, json!({}))).unwrap();
    assert_eq!((state.scenes, state.clips), (1, 2));
    song.run(ids::SELECT, json!({ "scene": song.scenes[3] }));
    song.one_step(ids::PASTE, json!({}), "Paste");
    let scenes: Vec<_> = song.doc().scenes().cloned().collect();
    assert_eq!(scenes.len(), 9);
    let pasted = &scenes[4];
    assert_eq!(pasted.name, "Verse");
    assert_eq!(pasted.tempo, scenes[1].tempo);
    assert!(song.doc().slot(song.tracks[0], pasted.id).is_some());
    assert!(song.doc().slot(song.tracks[2], pasted.id).is_some());
    // Cut takes the scene out with its clips.
    song.run(ids::SELECT, json!({ "scene": song.scenes[1] }));
    song.one_step(ids::CUT, json!({}), "Cut");
    assert_eq!(song.doc().scenes().count(), 8);
}

// Spec: session-view R42 (S-EDIT-014). With `all_tracks`, Copy takes every
// track's clip in the selected slot's scene and Paste starts at the first
// track.
#[test]
fn shift_applies_to_the_clips_of_every_track_in_the_scene() {
    let mut song = song(&[(0, 0, 1, "A"), (1, 0, 1, "B"), (2, 0, 1, "C")], &[]);
    song.select_slots(&[(1, 0)]);
    let state: SessionClipboardState =
        serde_json::from_value(song.run(ids::COPY, json!({ "all_tracks": true }))).unwrap();
    assert_eq!(state.clips, 3);
    song.select_slots(&[(1, 5)]);
    song.run(ids::PASTE, json!({ "all_tracks": true }));
    assert_eq!(song.slot(0, 5).as_deref(), Some("A"));
    assert_eq!(song.slot(1, 5).as_deref(), Some("B"));
    assert_eq!(song.slot(2, 5).as_deref(), Some("C"));
}

// Spec: second-window R18, R19, R23. A drag of clips between the views keeps
// agents waiting until it ends, drops where the pointer was last over a place
// in the other view, and Esc leaves everything as it was.
#[test]
fn a_clip_drag_waits_for_agents_and_drops_where_the_pointer_was() {
    let mut song = song(&[(0, 0, 1, "A")], &[]);
    let clip = song.slot_clip(0, 0);
    let track = song.tracks[0];
    assert!(!song.shell.gesture_under_way());
    song.shell
        .clip_drag_event(
            WindowKind::Main,
            ClipDragEvent::Begin(DragSource::Slots(vec![clip])),
        )
        .unwrap();
    assert!(song.shell.gesture_under_way());
    // Over a place in the other view, then off it: nothing is dropped.
    let spot = DropSpot::Arrangement {
        track,
        at: Ticks(2 * BAR),
    };
    let before = song.doc().clone();
    song.shell
        .clip_drag_event(WindowKind::Second, ClipDragEvent::Over(Some(spot)))
        .unwrap();
    song.shell
        .clip_drag_event(WindowKind::Second, ClipDragEvent::Over(None))
        .unwrap();
    song.shell
        .clip_drag_event(WindowKind::Main, ClipDragEvent::End { drop: true })
        .unwrap();
    assert!(!song.shell.gesture_under_way());
    assert_eq!(*song.doc(), before, "a drop on nothing changes nothing");
    // Esc after hovering a place changes nothing either.
    song.shell
        .clip_drag_event(
            WindowKind::Main,
            ClipDragEvent::Begin(DragSource::Slots(vec![clip])),
        )
        .unwrap();
    song.shell
        .clip_drag_event(WindowKind::Second, ClipDragEvent::Over(Some(spot)))
        .unwrap();
    song.shell
        .clip_drag_event(WindowKind::Main, ClipDragEvent::End { drop: false })
        .unwrap();
    assert_eq!(*song.doc(), before);
    // Dropped over the place: one undo step.
    let steps = song.steps();
    song.shell
        .clip_drag_event(
            WindowKind::Main,
            ClipDragEvent::Begin(DragSource::Slots(vec![clip])),
        )
        .unwrap();
    song.shell
        .clip_drag_event(WindowKind::Second, ClipDragEvent::Over(Some(spot)))
        .unwrap();
    song.shell
        .clip_drag_event(WindowKind::Main, ClipDragEvent::End { drop: true })
        .unwrap();
    assert_eq!(song.lane(0), [(2, 1, "A".to_owned())]);
    assert_eq!(song.steps(), steps + 1);
}

// Spec: second-window R23. The new commands read back through the query
// tool's tracks (the readback table), and Copy and Paste are in the Session
// context with the Edit keys.
#[test]
fn the_commands_are_registered_with_their_classes_and_keys() {
    let shell = Shell::headless();
    let registry = shell.registry();
    let class = |id: &str| registry.get(id).unwrap().permission.unwrap();
    use bl_commands::Permission;
    assert_eq!(class(ids::CUT), Permission::Destructive);
    assert_eq!(class(ids::COPY), Permission::Read);
    for id in [
        ids::PASTE,
        ids::DUPLICATE,
        ids::COPY_TO_ARRANGEMENT,
        ids::COPY_TO_SESSION,
    ] {
        assert_eq!(class(id), Permission::Edit, "{id}");
    }
    let keys = |id: &str| -> Vec<String> {
        registry
            .get(id)
            .unwrap()
            .bindings
            .iter()
            .map(|b| b.shortcut.to_string())
            .collect()
    };
    assert_eq!(keys(ids::CUT), ["S-EDIT-001", "S-SES-029"]);
    assert_eq!(keys(ids::COPY), ["S-EDIT-002", "S-SES-030"]);
    assert_eq!(keys(ids::PASTE), ["S-EDIT-003", "S-SES-031"]);
    assert_eq!(keys(ids::DUPLICATE), ["S-EDIT-004", "S-SES-032"]);
    assert!(registry.problems().is_empty(), "{:?}", registry.problems());
}

/// Makes a track at `index` in the open set, with the next ID the set issues.
fn create_track(shell: &mut Shell, kind: &str, index: usize) -> TrackId {
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": kind, "name": "T", "parent": null, "index": index }),
        )
        .unwrap();
    track
}

// Spec: session-view R41; set-exchange R6. A copied scene pastes into another
// set by the column each clip was in, not by the source set's track IDs.
#[test]
fn scenes_pasted_into_another_set_land_by_column() {
    let (mut shell, _control) = bl_shell::testing::rig();
    // Set A: an audio track, then a MIDI track at column 1.
    let midi = create_track(&mut shell, "midi", 0);
    create_track(&mut shell, "audio", 0);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": midi, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(midi, scene).unwrap().id;
    shell
        .run("clip.rename", json!({ "clip": clip, "name": "M" }))
        .unwrap();
    shell.run(ids::SELECT, json!({ "scene": scene })).unwrap();
    shell.run(ids::COPY, json!({})).unwrap();

    // Set B: its audio track has the ID A's MIDI track had, and its MIDI
    // track, in the same column, another.
    shell.run("set.new", json!({})).unwrap();
    let a = create_track(&mut shell, "audio", 0);
    let m = create_track(&mut shell, "midi", 1);
    assert_eq!(a, midi, "the IDs of the two sets collide");
    let first = shell.editor().document().scenes().next().unwrap().id;
    shell.run(ids::SELECT, json!({ "scene": first })).unwrap();
    shell.run(ids::PASTE, json!({})).unwrap();

    let doc = shell.editor().document();
    let pasted = doc.scenes().nth(1).unwrap().id;
    assert_eq!(
        doc.slot(m, pasted).map(|clip| clip.name.as_str()),
        Some("M")
    );
    assert!(doc.slot(a, pasted).is_none());
}
