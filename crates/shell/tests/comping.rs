//! Comping in the document, driven headless through `Shell::run` (WP-6.21;
//! D17): inserting and duplicating take lanes, copying a take's material to
//! the main lane, replacing it with the next or previous take, moving a comp
//! edge, and the freeze's rules for take lanes. Every command is one undo
//! step, and undo and redo restore the set exactly. The behaviour is in the
//! comping spec, R4, R5 and R17 to R21, and the freeze and bounce spec, R30.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, ContentHash, Document, Editor, MidiData, Note, TakeLaneId,
    TrackId, crossfades,
};
use bl_shell::audio::AudioHost;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{GesturePhase, Invocation, Shell, ShellError};
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

/// The factory default set: tracks 0 and 1 are MIDI, 2 and 3 audio.
struct Song {
    shell: Shell,
    tracks: Vec<TrackId>,
}

fn song() -> Song {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Comp.blset" }))
        .unwrap();
    Song { shell, tracks }
}

fn take_media(n: u8) -> ContentHash {
    ContentHash([n; 32])
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    /// An audio clip of file `file` on `track`'s main lane.
    fn main_clip(&mut self, track: usize, file: u8, start: i64, bars: i64) -> ClipId {
        let id: ClipId = self.shell.new_id();
        let clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Audio(AudioData::new(take_media(file))),
        );
        let track = self.tracks[track];
        self.shell
            .run("clip.create", json!({ "track": track, "clip": clip }))
            .unwrap();
        id
    }

    /// A take lane with one clip playing file `file` from bar `start` for
    /// `bars` bars (its Start marker `marker` seconds in).
    fn lane(&mut self, track: usize, file: u8, start: i64, bars: i64, marker: f64) -> TakeLaneId {
        let track = self.tracks[track];
        let name = format!(
            "Take {}",
            self.doc().track(track).unwrap().take_lanes().len() + 1
        );
        let lane = self.empty_lane(track, &name);
        let clip = Clip::new(
            self.shell.new_id(),
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Audio(AudioData {
                start: marker,
                ..AudioData::new(take_media(file))
            }),
        );
        self.shell
            .run(
                "take_lane.create_clip",
                json!({ "lane": lane, "clip": clip }),
            )
            .unwrap();
        lane
    }

    fn empty_lane(&mut self, track: TrackId, name: &str) -> TakeLaneId {
        let lane: TakeLaneId = self.shell.new_id();
        self.shell
            .run(
                "track.add_take_lane",
                json!({ "track": track, "lane": { "id": lane, "name": name, "clips": [] } }),
            )
            .unwrap();
        lane
    }

    /// The track's main-lane clips as `(start bar, end bar)`, in order.
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

    fn refused(&mut self, id: &str, args: Value) -> ShellError {
        let before = self.doc().clone();
        let error = self.shell.run(id, args).unwrap_err();
        assert_eq!(*self.doc(), before, "a refused {id} changes nothing");
        error
    }
}

// Spec: comping R4. Insert Take Lane adds a lane named one past the highest
// `Take N` to each audio or MIDI track, directly after the selected lane.
#[test]
fn insert_take_lane_names_and_places_each_new_lane() {
    let mut s = song();
    let first = s.lane(2, 1, 0, 2, 0.0);
    let _second = s.lane(2, 1, 0, 2, 0.5);
    let result = s.one_step(
        "take_lane.insert",
        json!({ "tracks": [s.tracks[2], s.tracks[0]], "after": [first] }),
    );
    let made: Vec<TakeLaneId> = serde_json::from_value(result["lanes"].clone()).unwrap();
    assert_eq!(made.len(), 2);
    let audio = s.doc().track(s.tracks[2]).unwrap();
    let names: Vec<&str> = audio.take_lanes().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        ["Take 1", "Take 3", "Take 2"],
        "directly after the selected lane"
    );
    let ids: Vec<TakeLaneId> = audio.take_lanes().map(|l| l.id).collect();
    assert_eq!(ids[1], made[0]);
    let midi = s.doc().track(s.tracks[0]).unwrap();
    assert_eq!(
        midi.take_lanes().next().unwrap().name,
        "Take 1",
        "a MIDI track takes lanes too"
    );
    assert_eq!(
        s.shell.editor().undo_history().next(),
        Some("Insert Take Lane")
    );

    // A group, a return or Main has none.
    let main = s.doc().main_id();
    s.refused("take_lane.insert", json!({ "tracks": [main] }));
}

// Spec: comping R5. Duplicate copies a lane directly after it, with its name
// and a copy of each clip under a new ID.
#[test]
fn duplicate_copies_each_lane_after_it_with_new_ids() {
    let mut s = song();
    let a = s.lane(2, 1, 0, 2, 0.0);
    let b = s.lane(2, 2, 4, 1, 0.0);
    let result = s.one_step("take_lane.duplicate", json!({ "lanes": [a, b] }));
    let copies: Vec<TakeLaneId> = serde_json::from_value(result["lanes"].clone()).unwrap();
    let track = s.doc().track(s.tracks[2]).unwrap();
    let order: Vec<TakeLaneId> = track.take_lanes().map(|l| l.id).collect();
    assert_eq!(order, [a, copies[0], b, copies[1]]);
    for (original, copy) in [(a, copies[0]), (b, copies[1])] {
        let (original, copy) = (
            track.take_lane(original).unwrap(),
            track.take_lane(copy).unwrap(),
        );
        assert_eq!(original.name, copy.name);
        assert_eq!(original.clips.len(), copy.clips.len());
        assert_ne!(original.clips[0].id, copy.clips[0].id);
        assert_eq!(original.clips[0].data, copy.clips[0].data);
        assert_eq!(
            (original.clips[0].start, original.clips[0].length),
            (copy.clips[0].start, copy.clips[0].length)
        );
    }
    assert_eq!(
        s.shell.editor().undo_history().next(),
        Some("Duplicate Take Lane(s)")
    );
}

// Spec: comping R17, R18. Copy Selection to Main Lane cuts the lane's clips
// at the range's edges, replaces what the copies cover, leaves the rest of
// the main lane, and the copies are independent of the take.
#[test]
fn copy_to_the_main_lane_cuts_replaces_and_stays_independent() {
    let mut s = song();
    let original = s.main_clip(2, 9, 0, 8);
    let lane = s.lane(2, 1, 0, 8, 0.0);
    let result = s.one_step(
        "take_lane.copy_to_main_lane",
        json!({ "lanes": [lane], "start": BAR, "end": 3 * BAR }),
    );
    let created: Vec<ClipId> = serde_json::from_value(result["created"].clone()).unwrap();
    assert_eq!(created.len(), 1);
    // The original clip was split around the copy.
    assert_eq!(s.main(2), [(0, 1), (1, 3), (3, 8)]);
    assert!(s.doc().clip(original).is_some());
    let copy = s.doc().clip(created[0]).unwrap().clone();
    let ClipData::Audio(audio) = &copy.data else {
        panic!()
    };
    assert_eq!(audio.media, take_media(1));
    assert!(
        audio.start > 0.0,
        "the copy plays on from the take's own place"
    );
    assert!(s.doc().clip_lane(created[0]).is_none(), "on the main lane");
    assert_eq!(
        s.shell.editor().undo_history().next(),
        Some("Copy Selection to Main Lane")
    );

    // Independent: deleting the copy leaves the take as it was.
    let take_clip = s.doc().take_lane(lane).unwrap().clips[0].clone();
    s.shell
        .run("clip.delete", json!({ "clip": created[0] }))
        .unwrap();
    assert_eq!(s.doc().take_lane(lane).unwrap().clips[0], take_clip);

    // A stretch the lane has no clip in leaves the main lane as it was.
    let before = s.doc().clone();
    let none = s
        .shell
        .run(
            "take_lane.copy_to_main_lane",
            json!({ "lanes": [lane], "start": 9 * BAR, "end": 10 * BAR }),
        )
        .unwrap();
    assert_eq!(none["created"], json!([]));
    assert_eq!(*s.doc(), before);
}

// Spec: comping R17. With lanes of one track listed, the last is copied from.
#[test]
fn the_last_listed_lane_of_a_track_is_copied_from() {
    let mut s = song();
    let a = s.lane(2, 1, 0, 4, 0.0);
    let b = s.lane(2, 2, 0, 4, 0.0);
    let result = s.one_step(
        "take_lane.copy_to_main_lane",
        json!({ "lanes": [a, b], "start": 0, "end": 4 * BAR }),
    );
    let created: Vec<ClipId> = serde_json::from_value(result["created"].clone()).unwrap();
    let ClipData::Audio(audio) = &s.doc().clip(created[0]).unwrap().data else {
        panic!()
    };
    assert_eq!(audio.media, take_media(2));
}

// Spec: comping R19. Replace with Next or Previous Take counts from the
// selected lane and skips lanes with no clip in the range.
#[test]
fn replace_with_a_take_counts_from_the_selected_lane_and_skips_empty_ones() {
    let mut s = song();
    let first = s.lane(2, 1, 0, 8, 0.0);
    let _empty = s.lane(2, 2, 20, 1, 0.0);
    let _third = s.lane(2, 3, 0, 8, 0.0);
    let audio = s.tracks[2];
    let args = |direction: &str, lane: Option<TakeLaneId>| {
        let mut args = json!({ "track": audio, "start": 2 * BAR, "end": 4 * BAR,
                               "direction": direction });
        if let Some(lane) = lane {
            args["lane"] = json!(lane);
        }
        args
    };
    // From the first lane, the next take with a clip there is the third.
    let result = s.one_step("take_lane.replace_with_take", args("next", Some(first)));
    let created: Vec<ClipId> = serde_json::from_value(result["created"].clone()).unwrap();
    let ClipData::Audio(audio) = &s.doc().clip(created[0]).unwrap().data else {
        panic!()
    };
    assert_eq!(audio.media, take_media(3));
    assert_eq!(
        s.shell.editor().undo_history().next(),
        Some("Replace with Next Take")
    );
    // Nothing above the first lane: that part is left as it was.
    let before = s.doc().clone();
    let result = s
        .shell
        .run("take_lane.replace_with_take", args("previous", Some(first)))
        .unwrap();
    assert_eq!(result["created"], json!([]));
    assert_eq!(*s.doc(), before);

    // From the main lane the comp's clip counts from the lane it uses: the
    // third, so previous goes to the first.
    let (_, ok) = (
        (),
        s.shell
            .run("take_lane.replace_with_take", args("previous", None)),
    );
    let created: Vec<ClipId> = serde_json::from_value(ok.unwrap()["created"].clone()).unwrap();
    let ClipData::Audio(audio) = &s.doc().clip(created[0]).unwrap().data else {
        panic!()
    };
    assert_eq!(audio.media, take_media(1));
    assert_eq!(
        s.shell.editor().undo_history().next(),
        Some("Replace with Previous Take")
    );
}

// Spec: comping R19. A part of the main lane that uses no take counts from
// before the first lane, so Next takes the first lane with a clip there.
#[test]
fn from_the_main_lane_a_part_that_uses_no_take_takes_the_first_lane() {
    let mut s = song();
    s.main_clip(2, 9, 0, 4);
    let _first = s.lane(2, 1, 0, 8, 0.0);
    let _second = s.lane(2, 2, 0, 8, 0.0);
    let result = s.one_step(
        "take_lane.replace_with_take",
        json!({ "track": s.tracks[2], "start": 0, "end": 2 * BAR, "direction": "next" }),
    );
    let created: Vec<ClipId> = serde_json::from_value(result["created"].clone()).unwrap();
    let ClipData::Audio(audio) = &s.doc().clip(created[0]).unwrap().data else {
        panic!()
    };
    assert_eq!(audio.media, take_media(1));
    // Previous from before the first lane has nothing.
    let before = s.doc().clone();
    let result = s
        .shell
        .run(
            "take_lane.replace_with_take",
            json!({ "track": s.tracks[2], "start": 5 * BAR, "end": 6 * BAR, "direction": "previous" }),
        )
        .unwrap();
    assert_eq!(result["created"], json!([]));
    assert_eq!(*s.doc(), before);
}

// Spec: comping R20, R21. Moving the boundary between two comp parts moves
// both edges, up to what the growing part's take reaches.
#[test]
fn a_comp_edge_moves_within_what_the_take_reaches() {
    let mut s = song();
    let a = s.lane(2, 1, 0, 6, 0.0);
    let b = s.lane(2, 2, 2, 6, 0.0);
    for (lane, start, end) in [(a, 0, 4), (b, 4, 8)] {
        s.shell
            .run(
                "take_lane.copy_to_main_lane",
                json!({ "lanes": [lane], "start": start * BAR, "end": end * BAR }),
            )
            .unwrap();
    }
    assert_eq!(s.main(2), [(0, 4), (4, 8)]);
    let sources = s
        .shell
        .run("take_lane.comp_sources", json!({ "track": s.tracks[2] }))
        .unwrap();
    assert_eq!(sources["stretches"].as_array().unwrap().len(), 2);
    assert_eq!(sources["unused_lanes"], json!([]));

    // Later: the first part grows as far as its take (to bar 6).
    s.one_step(
        "take_lane.move_comp_edge",
        json!({ "track": s.tracks[2], "from": 4 * BAR, "to": 5 * BAR }),
    );
    assert_eq!(s.main(2), [(0, 5), (5, 8)]);
    assert_eq!(
        s.shell.editor().undo_history().next(),
        Some("Move Comp Edge")
    );
    let error = s.refused(
        "take_lane.move_comp_edge",
        json!({ "track": s.tracks[2], "from": 5 * BAR, "to": 7 * BAR }),
    );
    assert!(error.to_string().contains("reach"), "{error}");
    // Earlier: the second part grows back to where its take starts (bar 2).
    s.one_step(
        "take_lane.move_comp_edge",
        json!({ "track": s.tracks[2], "from": 5 * BAR, "to": 3 * BAR }),
    );
    assert_eq!(s.main(2), [(0, 3), (3, 8)]);
    s.refused(
        "take_lane.move_comp_edge",
        json!({ "track": s.tracks[2], "from": 3 * BAR, "to": BAR }),
    );
    // Both parts keep a tick at least, and a boundary must exist.
    s.refused(
        "take_lane.move_comp_edge",
        json!({ "track": s.tracks[2], "from": 3 * BAR, "to": 0 }),
    );
    s.refused(
        "take_lane.move_comp_edge",
        json!({ "track": s.tracks[2], "from": 2 * BAR, "to": 3 * BAR }),
    );
}

// Spec: comping R20. MIDI comps match by notes in the song.
#[test]
fn a_midi_comp_matches_by_its_notes() {
    let mut s = song();
    let track = s.tracks[0];
    let id: ClipId = s.shell.new_id();
    let notes = (0..4).map(|n| {
        Note::new(
            60 + n,
            Ticks(i64::from(n) * Ticks::QUARTER.get()),
            Ticks::QUARTER,
            100,
        )
    });
    let clip = Clip::new(
        id,
        Ticks::ZERO,
        Ticks(BAR),
        ClipData::Midi(MidiData::new(notes)),
    );
    let lane = s.empty_lane(track, "Take 1");
    s.shell
        .run(
            "take_lane.create_clip",
            json!({ "lane": lane, "clip": clip }),
        )
        .unwrap();
    s.shell
        .run(
            "take_lane.copy_to_main_lane",
            json!({ "lanes": [lane], "start": BAR / 4, "end": BAR / 2 }),
        )
        .unwrap();
    let sources = s
        .shell
        .run("take_lane.comp_sources", json!({ "track": track }))
        .unwrap();
    assert_eq!(sources["stretches"].as_array().unwrap().len(), 1);
    let copy = s.doc().track(track).unwrap().clips().next().unwrap().id;
    s.shell
        .run("clip.rename", json!({ "clip": copy, "name": "x" }))
        .unwrap();
    let sources = s
        .shell
        .run("take_lane.comp_sources", json!({ "track": track }))
        .unwrap();
    assert_eq!(
        sources["stretches"].as_array().unwrap().len(),
        1,
        "a name isn't contents"
    );
}

// Spec: freeze-and-bounce R30. A frozen track's take lane can't be
// auditioned and nothing is copied from one to the main lane, but lanes can
// still be inserted, renamed and deleted and their clips edited.
#[test]
fn a_frozen_track_refuses_the_audition_and_the_copy_but_not_the_lanes_edits() {
    let mut s = song();
    let main = s.main_clip(2, 9, 0, 4);
    let lane = s.lane(2, 1, 0, 4, 0.0);
    let unfrozen = s.lane(3, 2, 0, 4, 0.0);
    let on_lane = s.doc().take_lane(lane).unwrap().clips[0].id;
    s.shell
        .run(
            "track.set_frozen",
            json!({ "frozen": [[s.tracks[2], { "clips": [], "tails": [], "output": null }]] }),
        )
        .unwrap();
    assert!(s.doc().track(s.tracks[2]).unwrap().is_frozen());

    let error = s.refused(
        "take_lane.set_audition",
        json!({ "lane": lane, "on": true }),
    );
    assert!(error.to_string().contains("frozen"), "{error}");
    let error = s.refused(
        "take_lane.copy_to_main_lane",
        json!({ "lanes": [lane], "start": 0, "end": BAR }),
    );
    assert!(error.to_string().contains("frozen"), "{error}");
    // A frozen track among several is refused too, wherever it's listed.
    let error = s.refused(
        "take_lane.copy_to_main_lane",
        json!({ "lanes": [unfrozen, lane], "start": 0, "end": BAR }),
    );
    assert!(error.to_string().contains("frozen"), "{error}");
    s.refused(
        "take_lane.replace_with_take",
        json!({ "track": s.tracks[2], "start": 0, "end": BAR, "direction": "next" }),
    );
    // The main lane's clip settings stay locked; the lane's are free.
    s.refused("clip.set_gain", json!({ "clip": main, "gain": 0.5 }));
    s.shell
        .run("clip.set_gain", json!({ "clip": on_lane, "gain": 0.5 }))
        .unwrap();
    s.shell
        .run("take_lane.rename", json!({ "lane": lane, "name": "Verse" }))
        .unwrap();
    s.shell
        .run("take_lane.insert", json!({ "tracks": [s.tracks[2]] }))
        .unwrap();
    s.shell
        .run("take_lane.delete", json!({ "lanes": [lane] }))
        .unwrap();
}

// Spec: comping R2, R27. The Arrangement's edits by clip ID act on a take
// lane's clip within its lane: a resize clears only that lane.
#[test]
fn resizing_a_take_lane_clip_clears_only_its_own_lane() {
    let mut s = song();
    let lane = s.lane(2, 1, 0, 2, 0.0);
    let id = s.doc().take_lane(lane).unwrap().clips[0].id;
    // A second clip on the lane, and a clip on the main lane under it.
    let second = Clip::new(
        s.shell.new_id(),
        Ticks(3 * BAR),
        Ticks(BAR),
        ClipData::Audio(AudioData::new(take_media(2))),
    );
    s.shell
        .run(
            "take_lane.create_clip",
            json!({ "lane": lane, "clip": second }),
        )
        .unwrap();
    s.main_clip(2, 9, 0, 6);
    s.one_step(
        "arrangement.resize_clips",
        json!({ "clips": [id], "edge": "end", "by": 2 * BAR }),
    );
    let held = s.doc().take_lane(lane).unwrap();
    let mut spans: Vec<(i64, i64)> = held
        .clips
        .iter()
        .map(|c| (c.start.get() / BAR, c.end().get() / BAR))
        .collect();
    spans.sort_unstable();
    assert_eq!(spans, [(0, 4)], "the second clip on the lane was covered");
    assert_eq!(s.main(2), [(0, 6)], "the main lane was left alone");

    // A split of a lane clip makes its other part on the lane.
    s.one_step(
        "arrangement.split",
        json!({ "clips": [id], "at": [2 * BAR] }),
    );
    assert_eq!(s.doc().take_lane(lane).unwrap().clips.len(), 2);
    assert_eq!(s.main(2), [(0, 6)]);

    // A move to another track puts it on that track's main lane; by time it
    // stays on its lane.
    s.one_step(
        "arrangement.move_clips",
        json!({ "clips": [id], "by": BAR }),
    );
    assert_eq!(s.doc().clip_lane(id), Some(lane));
    s.one_step(
        "arrangement.move_clips",
        json!({ "clips": [id], "track": s.tracks[3] }),
    );
    assert_eq!(s.doc().clip_lane(id), None);
    assert_eq!(s.doc().clip_track(id), Some(s.tracks[3]));
}

// Spec: comping R9. Duplicate Track Without Clips copies no take lanes, as it
// copies no clips.
#[test]
fn duplicate_track_without_clips_copies_no_take_lanes() {
    let mut s = song();
    s.main_clip(2, 9, 0, 2);
    s.lane(2, 1, 0, 2, 0.0);
    let result = s
        .shell
        .run(
            "track.duplicate_without_clips",
            json!({ "tracks": [s.tracks[2]] }),
        )
        .unwrap();
    let copies: Vec<TrackId> = serde_json::from_value(result["tracks"].clone()).unwrap();
    assert_eq!(copies.len(), 1);
    let copy = s.doc().track(copies[0]).unwrap();
    assert_eq!((copy.take_lanes().len(), copy.clips().len()), (0, 0));
    assert_eq!(s.doc().track(s.tracks[2]).unwrap().take_lanes().len(), 1);
}

impl Song {
    /// The main-lane crossfades of `track`, as the bars of their boundaries.
    fn boundaries(&self, track: usize) -> Vec<i64> {
        crossfades(self.doc().track(self.tracks[track]).unwrap().clips())
            .iter()
            .map(|c| c.boundary.get() / BAR)
            .collect()
    }

    /// A three-part comp on track `track`: bars 0 to 2 from take 1, 2 to 4
    /// from take 2, and 4 to 6 from take 3, each take reaching 1 bar further
    /// on both sides.
    fn three_part_comp(&mut self, track: usize) -> [TakeLaneId; 3] {
        let lanes = [
            self.lane(track, 1, 0, 8, 5.0),
            self.lane(track, 2, 0, 8, 5.0),
            self.lane(track, 3, 0, 8, 5.0),
        ];
        for (lane, (start, end)) in lanes.iter().zip([(0, 2), (2, 4), (4, 6)]) {
            self.shell
                .run(
                    "take_lane.copy_to_main_lane",
                    json!({ "lanes": [lane], "start": start * BAR, "end": end * BAR }),
                )
                .unwrap();
        }
        lanes
    }
}

// Spec: comping R22. With Create Fades on Clip Edges on, the parts of a comp
// are crossfaded where they meet, in the copy's own undo step.
#[test]
fn comp_parts_are_crossfaded_where_they_meet() {
    let mut s = song();
    let lanes = s.three_part_comp(2);
    assert_eq!(s.main(2), [(0, 2), (2, 4), (4, 6)]);
    assert_eq!(s.boundaries(2), [2, 4]);
    assert_eq!(
        s.doc().check_invariants(),
        Ok(()),
        "the crossfades keep the set valid"
    );

    // Copying a middle stretch from another take crossfades its new edges
    // too, still as one undo step.
    let steps = s.steps();
    s.shell
        .run(
            "take_lane.copy_to_main_lane",
            json!({ "lanes": [lanes[0]], "start": 3 * BAR, "end": 7 * BAR / 2 }),
        )
        .unwrap();
    assert_eq!(s.steps(), steps + 1);
    assert_eq!(s.boundaries(2).len(), 4);

    // With the setting off, a copy leaves its edges as they are.
    let mut s = song();
    s.shell
        .run(
            "settings.set_create_fades_on_clip_edges",
            json!({ "on": false }),
        )
        .unwrap();
    s.three_part_comp(2);
    assert_eq!(s.boundaries(2), Vec::<i64>::new());
}

// Spec: comping R21, R22. Moving a comp edge moves its crossfade with it, in
// one undo step.
#[test]
fn a_moved_comp_edge_takes_its_crossfade_along() {
    let mut s = song();
    s.three_part_comp(2);
    let before = crossfades(s.doc().track(s.tracks[2]).unwrap().clips());
    s.one_step(
        "take_lane.move_comp_edge",
        json!({ "track": s.tracks[2], "from": 2 * BAR, "to": 3 * BAR / 2 }),
    );
    let after = crossfades(s.doc().track(s.tracks[2]).unwrap().clips());
    assert_eq!(after.len(), 2);
    let moved = after.iter().find(|c| c.boundary == Ticks(3 * BAR / 2));
    let first = before
        .iter()
        .find(|c| c.boundary == Ticks(2 * BAR))
        .unwrap();
    let moved = moved.expect("the crossfade is at the new boundary");
    assert_eq!(
        (moved.before, moved.after, moved.curve),
        (first.before, first.after, first.curve),
        "it keeps its span and curve"
    );
}

// Spec: comping R31, R34. Draw Mode comping and a highlight's edge run as
// gestures: every frame is one undo step, and Esc puts the set back.
#[test]
fn comping_drags_are_one_undo_step_that_cancel_undoes() {
    let mut s = song();
    let [_, b, _] = s.three_part_comp(2);
    let track = s.tracks[2];
    let steps = s.steps();
    let before = s.doc().clone();

    // Dragging across take 2 and releasing copies it to the main lane.
    let copy = |end: i64| {
        Invocation::new(
            "take_lane.copy_to_main_lane",
            json!({ "lanes": [b], "start": BAR / 2, "end": end }),
        )
    };
    for end in [BAR, 3 * BAR / 2] {
        s.shell
            .gesture(GesturePhase::Update, Some(&copy(end)))
            .unwrap();
    }
    s.shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(*s.doc(), before, "Esc cancels the copy");
    assert_eq!(s.steps(), steps);

    for end in [BAR, 3 * BAR / 2] {
        s.shell
            .gesture(GesturePhase::Update, Some(&copy(end)))
            .unwrap();
    }
    s.shell
        .gesture(GesturePhase::Commit, Some(&copy(2 * BAR)))
        .unwrap();
    assert_eq!(s.steps(), steps + 1, "the drag is one undo step");
    assert_eq!(
        s.shell.editor().undo_history().next(),
        Some("Copy Selection to Main Lane")
    );
    s.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*s.doc(), before);

    // Dragging a comp edge is one step, and Esc leaves the boundary.
    let edge = |to: i64| {
        Invocation::new(
            "take_lane.move_comp_edge",
            json!({ "track": track, "from": 2 * BAR, "to": to }),
        )
    };
    for to in [BAR + BAR / 2, BAR + BAR / 4] {
        s.shell
            .gesture(GesturePhase::Update, Some(&edge(to)))
            .unwrap();
    }
    s.shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(*s.doc(), before);
    s.shell
        .gesture(GesturePhase::Commit, Some(&edge(3 * BAR / 2)))
        .unwrap();
    assert_eq!(s.steps(), steps + 1);
    let boundary = crossfades(s.doc().track(track).unwrap().clips())
        .iter()
        .map(|c| c.boundary)
        .min();
    assert_eq!(boundary, Some(Ticks(3 * BAR / 2)), "the crossfade went too");
}
