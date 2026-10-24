//! Linked-track editing in the shell (WP-6.23), driven headless through
//! `Shell::run`: Link Tracks and Unlink Track(s), selecting on a linked
//! track, and the edits that reach every track of a group, each one undo
//! step that undo and redo restore exactly. The behaviour is in the
//! linked-track editing spec.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, ContentHash, Document, Editor, TakeLaneId, TrackId,
};
use bl_shell::audio::AudioHost;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
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

/// The factory default set with a third audio track: `tracks` are its
/// 1 MIDI, 2 MIDI, 3 Audio, 4 Audio and the new 5 Audio.
struct Song {
    shell: Shell,
    tracks: Vec<TrackId>,
    /// The folder the set lives in, where Consolidate writes its files;
    /// removed when dropped.
    folder: PathBuf,
}

impl Drop for Song {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn song() -> Song {
    let mut editor = factory_default_set();
    editor.clear_history().unwrap();
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let folder = std::env::temp_dir().join(format!(
        "bl-linked-tracks-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&folder).unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": folder.join("Linked.blset") }))
        .unwrap();
    let index = shell.editor().document().top_level().len();
    let id: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": id, "kind": "audio", "name": "5 Audio", "index": index }),
        )
        .unwrap();
    let tracks = shell.editor().document().top_level().collect();
    Song {
        shell,
        tracks,
        folder,
    }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    /// Links the audio tracks 2 to 4.
    fn linked(mut self) -> Self {
        self.shell
            .run("track.link", json!({ "tracks": &self.tracks[2..] }))
            .unwrap();
        self
    }

    fn main_clip(&mut self, track: usize, start: i64, bars: i64) -> ClipId {
        let id: ClipId = self.shell.new_id();
        let clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Audio(AudioData::new(ContentHash([track as u8; 32]))),
        );
        let track = self.tracks[track];
        self.shell
            .run("clip.create", json!({ "track": track, "clip": clip }))
            .unwrap();
        id
    }

    fn lane(&mut self, track: usize, name: &str) -> TakeLaneId {
        let lane: TakeLaneId = self.shell.new_id();
        let track = self.tracks[track];
        self.shell
            .run(
                "track.add_take_lane",
                json!({ "track": track, "lane": { "id": lane, "name": name, "clips": [] } }),
            )
            .unwrap();
        lane
    }

    fn lane_clip(&mut self, lane: TakeLaneId, file: u8, start: i64, bars: i64) -> ClipId {
        let id: ClipId = self.shell.new_id();
        let clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Audio(AudioData::new(ContentHash([file; 32]))),
        );
        self.shell
            .run(
                "take_lane.create_clip",
                json!({ "lane": lane, "clip": clip }),
            )
            .unwrap();
        id
    }

    /// The main-lane clips of a track as `(start bar, end bar)`.
    fn main(&self, track: usize) -> Vec<(i64, i64)> {
        let mut clips: Vec<(i64, i64)> = self
            .doc()
            .track(self.tracks[track])
            .unwrap()
            .clips()
            .map(|c| (c.start.get() / BAR, c.end().get() / BAR))
            .collect();
        clips.sort_unstable();
        clips
    }

    /// The main-lane clips of the three audio tracks.
    fn mains(&self) -> Vec<Vec<(i64, i64)>> {
        (2..5).map(|t| self.main(t)).collect()
    }

    /// Runs `id`, checks it is one undo step that undo and redo restore
    /// exactly, and leaves it done.
    fn one_step(&mut self, id: &str, args: Value) -> Value {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self
            .shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        assert!(after.check_invariants().is_ok(), "{id}");
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }

    fn selected_clips(&mut self) -> Vec<ClipId> {
        let state = self.shell.run("arrangement.state", json!({})).unwrap();
        let mut clips: Vec<ClipId> =
            serde_json::from_value(state["selected_clips"].clone()).unwrap();
        clips.sort_unstable();
        clips
    }
}

fn sorted(mut clips: Vec<ClipId>) -> Vec<ClipId> {
    clips.sort_unstable();
    clips
}

// Spec: linked-track editing R2, R3, R5, R16. Link Tracks puts the audio and
// MIDI tracks into a group of their own, `into` adds to a group, Unlink
// Track(s) takes tracks out, and each is one undo step.
#[test]
fn link_tracks_and_unlink_tracks_are_one_undo_step_each() {
    let mut s = song();
    let [_, _, a, b, c] = s.tracks[..] else {
        panic!()
    };
    let result = s.one_step("track.link", json!({ "tracks": [a, b] }));
    assert_eq!(result["tracks"], json!([a, b]));
    assert_eq!(s.shell.editor().undo_history().next(), Some("Link Tracks"));
    assert_eq!(s.doc().linked_with(a), [a, b]);

    // Cmd: the track joins the group of the one named.
    s.one_step("track.link", json!({ "tracks": [c], "into": a }));
    assert_eq!(s.doc().linked_with(c), [a, b, c]);

    s.one_step("track.unlink", json!({ "tracks": [c] }));
    assert_eq!(
        s.shell.editor().undo_history().next(),
        Some("Unlink Tracks")
    );
    assert_eq!(s.doc().linked_with(a), [a, b]);
    // A group left with one track ends.
    s.one_step("track.unlink", json!({ "tracks": [b] }));
    assert!(s.doc().linked_with(a).is_empty());
    assert_eq!(s.doc().track(a).unwrap().link(), None);

    // Linking again takes a track out of the group it was in.
    s.shell
        .run("track.link", json!({ "tracks": [a, b, c] }))
        .unwrap();
    let midi = s.tracks[0];
    s.shell
        .run("track.link", json!({ "tracks": [c, midi] }))
        .unwrap();
    assert_eq!(s.doc().linked_with(a), [a, b]);
    assert_eq!(s.doc().linked_with(midi), [midi, c]);
}

// Spec: linked-track editing R2. Linking needs two audio or MIDI tracks.
#[test]
fn link_tracks_is_unavailable_with_fewer_than_two_audio_or_midi_tracks() {
    let mut s = song();
    let [_, _, a, ..] = s.tracks[..] else {
        panic!()
    };
    let main = s.doc().main_id();
    let steps = s.steps();
    for tracks in [vec![a], vec![a, main], vec![]] {
        assert!(
            !s.shell
                .can_invoke("track.link", &json!({ "tracks": tracks }))
        );
        let error = s
            .shell
            .run("track.link", json!({ "tracks": tracks }))
            .unwrap_err();
        assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");
    }
    assert!(
        s.shell
            .can_invoke("track.link", &json!({ "tracks": [a, s.tracks[3]] }))
    );
    assert!(
        !s.shell
            .can_invoke("track.unlink", &json!({ "tracks": [a] }))
    );
    assert_eq!(s.steps(), steps);
}

// Spec: linked-track editing R4. Link Tracks on a group track links the
// audio and MIDI tracks inside it, at any depth; Unlink Track(s) there
// takes them out.
#[test]
fn a_group_tracks_menu_links_the_tracks_inside_it_at_any_depth() {
    let mut s = song();
    let [_, _, a, b, c] = s.tracks[..] else {
        panic!()
    };
    let inner = s
        .shell
        .run("track.group", json!({ "tracks": [a, b] }))
        .unwrap();
    let inner = serde_json::from_value::<TrackId>(inner["track"].clone()).unwrap();
    let outer = s
        .shell
        .run("track.group", json!({ "tracks": [inner, c] }))
        .unwrap();
    let outer = serde_json::from_value::<TrackId>(outer["track"].clone()).unwrap();

    s.one_step("track.link", json!({ "tracks": [outer] }));
    assert_eq!(s.doc().linked_with(c).len(), 3);
    assert!(
        s.doc().track(outer).unwrap().link().is_none(),
        "the group has no indicator"
    );
    s.one_step("track.unlink", json!({ "tracks": [outer] }));
    assert!(s.doc().linked_with(a).is_empty());
}

// Spec: linked-track editing R5. A duplicated track isn't linked; a deleted
// one leaves its group, and undoing the deletion puts it back.
#[test]
fn a_duplicate_is_unlinked_and_a_deleted_track_leaves_its_group() {
    let mut s = song().linked();
    let [_, _, a, b, c] = s.tracks[..] else {
        panic!()
    };
    let copies = s
        .shell
        .run("track.duplicate", json!({ "tracks": [a] }))
        .unwrap();
    let copy: TrackId = serde_json::from_value(copies["tracks"][0].clone()).unwrap();
    assert!(s.doc().linked_with(copy).is_empty());
    assert_eq!(s.doc().linked_with(a), [a, b, c]);

    let before = s.doc().clone();
    s.shell.run("track.delete", json!({ "track": b })).unwrap();
    assert_eq!(s.doc().linked_with(a), [a, c]);
    s.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*s.doc(), before);
}

// Spec: linked-track editing R7. Selecting clips on a linked track selects,
// on the others, the clips that start and end at the same places.
#[test]
fn selecting_clips_selects_the_matching_clips_of_the_group() {
    let mut s = song().linked();
    let on_a = s.main_clip(2, 0, 2);
    let on_b = s.main_clip(3, 0, 2);
    let on_c = s.main_clip(4, 0, 2);
    let longer = s.main_clip(3, 4, 3);
    let steps = s.steps();
    s.shell
        .run("arrangement.select_clips", json!({ "clips": [on_a] }))
        .unwrap();
    assert_eq!(s.selected_clips(), sorted(vec![on_a, on_b, on_c]));
    s.shell
        .run("arrangement.select_clips", json!({ "clips": [longer] }))
        .unwrap();
    assert_eq!(
        s.selected_clips(),
        [longer],
        "a clip that ends elsewhere doesn't match"
    );
    s.shell
        .run(
            "arrangement.select_clips",
            json!({ "clips": [on_a], "linked": false }),
        )
        .unwrap();
    assert_eq!(s.selected_clips(), [on_a]);
    assert_eq!(s.steps(), steps, "selecting is never an undo step");
}

// Spec: linked-track editing R7. Selecting time on a linked track selects the
// same time on the group's tracks, on the same lane: a take lane at the same
// place among their take lanes.
#[test]
fn selecting_time_selects_the_same_lane_of_every_track() {
    let mut s = song().linked();
    let [_, _, a, b, c] = s.tracks[..] else {
        panic!()
    };
    let a1 = s.lane(2, "A1");
    let a2 = s.lane(2, "A2");
    let b1 = s.lane(3, "B1");
    let c1 = s.lane(4, "C1");
    let c2 = s.lane(4, "C2");
    let state = s
        .shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": BAR, "tracks": [a] }),
        )
        .unwrap();
    assert_eq!(
        sorted_tracks(&state["selected_tracks"]),
        sorted_tracks(&json!([a, b, c]))
    );
    let lanes = |state: &Value| {
        let mut lanes: Vec<TakeLaneId> = state["time_selection_lanes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|l| serde_json::from_value(l["lane"].clone()).ok())
            .collect();
        lanes.sort_unstable();
        lanes
    };
    let state = s
        .shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": BAR, "lanes": [{ "type": "take", "track": a, "lane": a2 }] }),
        )
        .unwrap();
    // A track with no lane at that place is left out.
    let mut want = vec![a2, c2];
    want.sort_unstable();
    assert_eq!(lanes(&state), want);
    let state = s
        .shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": BAR, "lanes": [{ "type": "take", "track": a, "lane": a1 }] }),
        )
        .unwrap();
    let mut want = vec![a1, b1, c1];
    want.sort_unstable();
    assert_eq!(lanes(&state), want);
    assert!(!lanes(&state).contains(&c2));
    let state = s
        .shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": BAR, "tracks": [a], "linked": false }),
        )
        .unwrap();
    assert_eq!(state["selected_tracks"], json!([a]));
}

fn sorted_tracks(value: &Value) -> Vec<TrackId> {
    let mut tracks: Vec<TrackId> = serde_json::from_value(value.clone()).unwrap();
    tracks.sort_unstable();
    tracks
}

// Spec: linked-track editing R8, R15. Moving, resizing and splitting reach
// the matching clips of the group in one undo step, and `linked: false`
// limits an edit to the clips it names.
#[test]
fn moving_resizing_and_splitting_reach_the_group() {
    let mut s = song().linked();
    let on_a = s.main_clip(2, 0, 4);
    let _on_b = s.main_clip(3, 0, 4);
    let _on_c = s.main_clip(4, 0, 4);
    s.main_clip(3, 8, 2);

    s.one_step(
        "arrangement.move_clips",
        json!({ "clips": [on_a], "by": BAR }),
    );
    assert_eq!([2, 3, 4].map(|t| s.main(t)[0]), [(1, 5); 3]);
    s.one_step(
        "arrangement.resize_clips",
        json!({ "clips": [on_a], "edge": "end", "by": BAR }),
    );
    assert_eq!([2, 3, 4].map(|t| s.main(t)[0]), [(1, 6); 3]);
    s.one_step(
        "arrangement.split",
        json!({ "clips": [on_a], "at": [3 * BAR] }),
    );
    assert_eq!([2, 3, 4].map(|t| s.main(t).len()), [2, 3, 2]);
    assert_eq!(s.main(3), [(1, 3), (3, 6), (8, 10)]);

    s.one_step(
        "arrangement.move_clips",
        json!({ "clips": [on_a], "by": 2 * BAR, "linked": false }),
    );
    assert_eq!(s.main(2)[0], (3, 5));
    assert_eq!(s.main(3)[0], (1, 3), "the other tracks keep theirs");
}

// Spec: linked-track editing R8. Consolidate makes the same range on every
// linked track.
#[test]
fn consolidating_reaches_the_group() {
    let mut s = song().linked();
    for track in 2..5 {
        s.main_clip(track, 0, 1);
        s.main_clip(track, 1, 1);
    }
    let a = s.tracks[2];
    s.one_step(
        "arrangement.consolidate",
        json!({ "tracks": [a], "start": 0, "end": 2 * BAR }),
    );
    assert_eq!(s.mains(), vec![vec![(0, 2)]; 3]);
}

// Spec: linked-track editing R9. Delete, Duplicate, Cut and Copy on clips or
// time selected on a linked track act on the group's tracks as well.
#[test]
fn delete_duplicate_cut_and_copy_act_on_all_of_the_selection() {
    let mut s = song().linked();
    let on_a = s.main_clip(2, 0, 2);
    s.main_clip(3, 0, 2);
    s.main_clip(4, 0, 2);
    s.shell
        .run("arrangement.select_clips", json!({ "clips": [on_a] }))
        .unwrap();
    s.one_step("arrangement.duplicate", json!({}));
    assert_eq!(s.mains(), vec![vec![(0, 2), (2, 4)]; 3]);
    s.shell.run("edit.undo", json!({})).unwrap();

    s.shell
        .run("arrangement.select_clips", json!({ "clips": [on_a] }))
        .unwrap();
    s.one_step("arrangement.delete", json!({}));
    assert_eq!(s.mains(), vec![vec![]; 3]);
    s.shell.run("edit.undo", json!({})).unwrap();

    // A named clip is the selection's stand-in: it reaches the group too.
    s.one_step("arrangement.cut", json!({ "clips": [on_a] }));
    assert_eq!(s.mains(), vec![vec![]; 3]);
    s.shell.run("edit.undo", json!({})).unwrap();
    let copied = s
        .shell
        .run("arrangement.copy", json!({ "clips": [on_a] }))
        .unwrap();
    assert_eq!(
        (&copied["clips"], &copied["tracks"]),
        (&json!(3), &json!(3))
    );
    s.one_step(
        "arrangement.delete",
        json!({ "clips": [on_a], "linked": false }),
    );
    assert_eq!(s.main(3), [(0, 2)]);
}

// Spec: linked-track editing R11. Inserting, renaming, deleting and
// auditioning take lanes reach the lane at the same place on each track.
#[test]
fn take_lane_edits_reach_the_group() {
    let mut s = song().linked();
    let [_, _, a, b, c] = s.tracks[..] else {
        panic!()
    };
    let result = s.one_step("take_lane.insert", json!({ "tracks": [a] }));
    assert_eq!(result["lanes"].as_array().unwrap().len(), 3);
    for track in [a, b, c] {
        assert_eq!(s.doc().track(track).unwrap().take_lanes().len(), 1);
    }
    s.one_step(
        "take_lane.insert",
        json!({ "tracks": [b], "linked": false }),
    );
    assert_eq!(s.doc().track(b).unwrap().take_lanes().len(), 2);
    assert_eq!(s.doc().track(c).unwrap().take_lanes().len(), 1);

    let lane_of =
        |s: &Song, t: TrackId, n: usize| s.doc().track(t).unwrap().take_lanes().nth(n).unwrap().id;
    let (la, lb) = (lane_of(&s, a, 0), lane_of(&s, b, 0));
    s.one_step("take_lane.rename", json!({ "lane": la, "name": "Snare" }));
    assert_eq!(s.doc().take_lane(lb).unwrap().name, "Snare");
    s.one_step("take_lane.audition", json!({ "lanes": [la] }));
    assert_eq!(
        s.doc().track(c).unwrap().audition(),
        Some(lane_of(&s, c, 0))
    );
    // Duplicating acts on the lanes named only.
    s.one_step("take_lane.duplicate", json!({ "lanes": [la] }));
    assert_eq!(s.doc().track(a).unwrap().take_lanes().len(), 2);
    assert_eq!(s.doc().track(b).unwrap().take_lanes().len(), 2);
    assert_eq!(s.doc().track(c).unwrap().take_lanes().len(), 1);
    s.one_step("take_lane.delete", json!({ "lanes": [la] }));
    assert_eq!(s.doc().track(c).unwrap().take_lanes().len(), 0);
    assert_eq!(s.doc().track(b).unwrap().take_lanes().len(), 1);
}

// Spec: linked-track editing R12. The comp commands act on the same range
// and lane of every track, each copying its own lane's material.
#[test]
fn the_comp_commands_reach_the_group() {
    let mut s = song().linked();
    let lanes: Vec<TakeLaneId> = (2..5).map(|t| s.lane(t, "Take 1")).collect();
    for (n, lane) in lanes.iter().enumerate() {
        s.lane_clip(*lane, 10 + n as u8, 0, 8);
    }
    s.one_step(
        "take_lane.copy_to_main_lane",
        json!({ "lanes": [lanes[0]], "start": BAR, "end": 3 * BAR }),
    );
    assert_eq!(s.mains(), vec![vec![(1, 3)]; 3]);
    let files: Vec<ContentHash> = (2..5)
        .map(|t| {
            match &s
                .doc()
                .track(s.tracks[t])
                .unwrap()
                .clips()
                .next()
                .unwrap()
                .data
            {
                ClipData::Audio(audio) => audio.media,
                ClipData::Midi(_) => unreachable!(),
            }
        })
        .collect();
    assert_eq!(
        files,
        [10, 11, 12].map(|n| ContentHash([n; 32])),
        "each from its own lane"
    );

    // A lane at the same place on each: the second lane of every track.
    let seconds: Vec<TakeLaneId> = (2..5).map(|t| s.lane(t, "Take 2")).collect();
    for (n, lane) in seconds.iter().enumerate() {
        s.lane_clip(*lane, 20 + n as u8, 0, 8);
    }
    s.one_step(
        "take_lane.replace_with_take",
        json!({ "track": s.tracks[2], "lane": lanes[0], "start": BAR, "end": 3 * BAR, "direction": "next" }),
    );
    let used = |s: &mut Song, t: usize| -> Vec<TakeLaneId> {
        let result = s
            .shell
            .run("take_lane.comp_sources", json!({ "track": s.tracks[t] }))
            .unwrap();
        result["stretches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|st| serde_json::from_value(st["lane"].clone()).unwrap())
            .collect()
    };
    assert_eq!(used(&mut s, 2), [seconds[0]]);
    assert_eq!(used(&mut s, 3), [seconds[1]]);
    assert_eq!(used(&mut s, 4), [seconds[2]]);

    // The comp edge moves on every track that has one there.
    s.shell
        .run(
            "take_lane.copy_to_main_lane",
            json!({ "lanes": [lanes[0]], "start": 3 * BAR, "end": 5 * BAR }),
        )
        .unwrap();
    s.one_step(
        "take_lane.move_comp_edge",
        json!({ "track": s.tracks[2], "from": 3 * BAR, "to": 4 * BAR }),
    );
    assert_eq!(s.mains(), vec![vec![(1, 4), (4, 5)]; 3]);
}

// Spec: linked-track editing R13, R14. The …Time commands act on every track
// whether linked or not, and the edits R14 lists act on what they name.
#[test]
fn other_edits_act_on_what_they_name() {
    let mut s = song().linked();
    let on_a = s.main_clip(2, 0, 2);
    let on_b = s.main_clip(3, 0, 2);
    s.one_step("clip.set_gain", json!({ "clip": on_a, "gain": 0.5 }));
    let gain = |s: &Song, clip| match &s.doc().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.gain,
        ClipData::Midi(_) => unreachable!(),
    };
    assert!(gain(&s, on_a) < 1.0 && (gain(&s, on_b) - 1.0).abs() < f64::EPSILON);
    s.one_step(
        "arrangement.insert_silence",
        json!({ "start": 0, "end": BAR }),
    );
    assert_eq!(s.main(2), s.main(3));
}
