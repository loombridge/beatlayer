//! Take lanes in the document (WP-6.21, the comping spec R1 to R9, R14 and
//! R20): their clips are clips of their own, found by ID; the commands that
//! add, rename, delete, move and audition them are one undo step each; and
//! which take-lane material the main lane uses is one pure function.

use bl_core::time::{TempoMap, Ticks};
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, Document, Editor, MidiData, ModelError,
    NewTakeLane, Note, TakeLaneId, Track, TrackId, TrackKind, comp,
};
use proptest::prelude::*;

const BAR: i64 = Ticks::WHOLE.get();

fn song() -> (Editor, TrackId, TrackId) {
    let mut editor = Editor::new();
    let track = |editor: &mut Editor, kind, index| {
        let id: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: format!("{kind}"),
                parent: None,
                index,
            })
            .unwrap();
        id
    };
    let audio = track(&mut editor, TrackKind::Audio, 0);
    let midi = track(&mut editor, TrackKind::Midi, 1);
    (editor, audio, midi)
}

fn audio_clip(editor: &mut Editor, start: i64, bars: i64, marker: f64) -> Clip {
    Clip::new(
        editor.new_id(),
        Ticks(start),
        Ticks(bars * BAR),
        ClipData::Audio(AudioData {
            start: marker,
            ..AudioData::new(ContentHash([9; 32]))
        }),
    )
}

fn midi_clip(editor: &mut Editor, start: i64, notes: &[(u8, i64)]) -> Clip {
    Clip::new(
        editor.new_id(),
        Ticks(start),
        Ticks(BAR),
        ClipData::Midi(MidiData::new(
            notes
                .iter()
                .map(|&(key, at)| Note::new(key, Ticks(at), Ticks::QUARTER, 100)),
        )),
    )
}

fn lane(editor: &mut Editor, track: TrackId, name: &str, clips: Vec<Clip>) -> TakeLaneId {
    let id: TakeLaneId = editor.new_id();
    editor
        .apply(Command::AddTakeLane {
            track,
            lane: NewTakeLane {
                id: Some(id),
                name: name.to_owned(),
                clips: Vec::new(),
            },
            index: None,
        })
        .unwrap();
    for clip in clips {
        editor
            .apply(Command::CreateTakeClip { lane: id, clip })
            .unwrap();
    }
    id
}

fn lanes_of(doc: &Document, track: TrackId) -> Vec<TakeLaneId> {
    doc.track(track)
        .unwrap()
        .take_lanes()
        .map(|l| l.id)
        .collect()
}

fn names_of(doc: &Document, track: TrackId) -> Vec<String> {
    doc.track(track)
        .unwrap()
        .take_lanes()
        .map(|l| l.name.clone())
        .collect()
}

// Spec: comping R1, R2. A take lane holds clips of its track's kind, each
// found by ID like a main-lane clip, and the clip commands act on it there.
#[test]
fn a_take_lane_clip_is_a_clip_found_by_id_that_the_clip_commands_edit() {
    let (mut editor, audio, midi) = song();
    let clip = audio_clip(&mut editor, 0, 2, 0.0);
    let id = clip.id;
    let take = lane(&mut editor, audio, "Take 1", vec![clip]);
    let doc = editor.document();
    assert_eq!(doc.clip_track(id), Some(audio));
    assert_eq!(doc.clip_lane(id), Some(take));
    assert_eq!(doc.take_lane_track(take), Some(audio));
    assert_eq!(
        doc.track(audio).unwrap().clips().len(),
        0,
        "not on the main lane"
    );

    editor
        .apply(Command::SetClipLength {
            clip: id,
            length: Ticks(BAR),
        })
        .unwrap();
    editor
        .apply(Command::RenameClip {
            clip: id,
            name: "Verse".to_owned(),
        })
        .unwrap();
    editor
        .apply(Command::SetClipMute {
            clip: id,
            mute: true,
        })
        .unwrap();
    let held = editor.document().clip(id).unwrap();
    assert_eq!(
        (held.length, held.name.as_str(), held.mute),
        (Ticks(BAR), "Verse", true)
    );
    editor.document().check_invariants().unwrap();

    // Moving it within its track keeps it on its lane; moving it to another
    // track puts it on that track's main lane.
    editor
        .apply(Command::MoveClip {
            clip: id,
            track: audio,
            start: Ticks(BAR),
        })
        .unwrap();
    assert_eq!(editor.document().clip_lane(id), Some(take));
    assert_eq!(editor.document().clip(id).unwrap().start, Ticks(BAR));
    let other: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: other,
            kind: TrackKind::Audio,
            name: "Other".to_owned(),
            parent: None,
            index: 2,
        })
        .unwrap();
    editor
        .apply(Command::MoveClip {
            clip: id,
            track: other,
            start: Ticks::ZERO,
        })
        .unwrap();
    assert_eq!(editor.document().clip_lane(id), None);
    assert_eq!(editor.document().clip_track(id), Some(other));
    assert!(editor.document().take_lane(take).unwrap().clips.is_empty());

    // Deleting a lane's clip removes it from the lookups.
    let second = audio_clip(&mut editor, 0, 1, 0.0);
    let second_id = second.id;
    editor
        .apply(Command::CreateTakeClip {
            lane: take,
            clip: second,
        })
        .unwrap();
    editor
        .apply(Command::DeleteClip { clip: second_id })
        .unwrap();
    assert!(editor.document().clip(second_id).is_none());
    editor.document().check_invariants().unwrap();

    // A MIDI track takes MIDI clips on a lane, and refuses audio ones.
    let notes = midi_clip(&mut editor, 0, &[(60, 0)]);
    let on_midi = lane(&mut editor, midi, "Take 1", vec![notes]);
    assert_eq!(lanes_of(editor.document(), midi), [on_midi]);
    let wrong = audio_clip(&mut editor, 0, 1, 0.0);
    let error = editor
        .apply(Command::CreateTakeClip {
            lane: on_midi,
            clip: wrong,
        })
        .unwrap_err();
    assert!(error.to_string().contains("audio"), "{error}");
}

// Spec: comping R3, R9. Take lanes are saved with their IDs, names, order and
// clips; deleting the track deletes them, and undo brings them back with
// their IDs.
#[test]
fn take_lanes_are_saved_and_go_with_their_track_through_delete_and_undo() {
    let (mut editor, audio, _) = song();
    let clip = audio_clip(&mut editor, 0, 1, 0.0);
    let id = clip.id;
    let first = lane(&mut editor, audio, "Take 1", vec![clip]);
    let second = lane(&mut editor, audio, "Take 2", vec![]);
    editor
        .apply(Command::SetAudition {
            lane: second,
            on: true,
        })
        .unwrap();
    let before = editor.document().clone();
    let saved = serde_json::to_string(&before).unwrap();
    let loaded: Document = serde_json::from_str(&saved).unwrap();
    assert_eq!(loaded, before);
    loaded.check_invariants().unwrap();
    assert_eq!(loaded.track(audio).unwrap().audition(), Some(second));
    assert_eq!(lanes_of(&loaded, audio), [first, second]);

    editor.apply(Command::DeleteTrack { track: audio }).unwrap();
    assert!(editor.document().clip(id).is_none());
    assert!(editor.document().take_lane(first).is_none());
    assert!(editor.document().take_lane_track(second).is_none());
    editor.undo().unwrap();
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.document().clip_lane(id), Some(first));
}

// Spec: comping R4, R6. A new lane is named one number past the highest
// `Take N` the track has; a name can't be empty.
#[test]
fn lanes_are_named_by_the_highest_take_number_and_never_empty() {
    let (mut editor, audio, _) = song();
    assert_eq!(
        editor.document().track(audio).unwrap().next_take_name(),
        "Take 1"
    );
    let first = lane(&mut editor, audio, "Take 4", vec![]);
    lane(&mut editor, audio, "Verse", vec![]);
    lane(&mut editor, audio, "Take 2", vec![]);
    assert_eq!(
        editor.document().track(audio).unwrap().next_take_name(),
        "Take 5"
    );
    editor
        .apply(Command::RenameTakeLane {
            lane: first,
            name: "Chorus".to_owned(),
        })
        .unwrap();
    assert_eq!(
        editor.document().track(audio).unwrap().next_take_name(),
        "Take 3"
    );
    let refused = editor
        .apply(Command::RenameTakeLane {
            lane: first,
            name: String::new(),
        })
        .unwrap_err();
    assert!(refused.to_string().contains("empty"), "{refused}");
    // One undo step per rename.
    let steps = editor.undo_history().len();
    editor
        .apply(Command::RenameTakeLane {
            lane: first,
            name: "Bridge".to_owned(),
        })
        .unwrap();
    assert_eq!(editor.undo_history().len(), steps + 1);
    assert_eq!(editor.undo_history().next(), Some("Rename Take Lane"));
}

// Spec: comping R7, R8, R14. Lanes are deleted with their clips, all at once
// or the unused ones; moved keeping their order; and at most one is
// auditioned.
#[test]
fn lanes_are_deleted_moved_and_auditioned_one_undo_step_each() {
    let (mut editor, audio, _) = song();
    let a = lane(&mut editor, audio, "Take 1", vec![]);
    let b = lane(&mut editor, audio, "Take 2", vec![]);
    let c = lane(&mut editor, audio, "Take 3", vec![]);
    let d = lane(&mut editor, audio, "Take 4", vec![]);

    // Move: the lanes keep their order among themselves, and a group goes to
    // a place counted among the lanes that stay.
    editor
        .apply(Command::MoveTakeLanes {
            lanes: vec![d, b],
            index: 0,
        })
        .unwrap();
    assert_eq!(lanes_of(editor.document(), audio), [b, d, a, c]);
    assert_eq!(editor.undo_history().next(), Some("Move Take Lane(s)"));
    editor
        .apply(Command::MoveTakeLanes {
            lanes: vec![b],
            index: 99,
        })
        .unwrap();
    assert_eq!(
        lanes_of(editor.document(), audio),
        [d, a, c, b],
        "past the last is the last"
    );

    // Audition: one at a time; deleting the auditioned lane ends it.
    editor
        .apply(Command::SetAudition { lane: a, on: true })
        .unwrap();
    editor
        .apply(Command::SetAudition { lane: c, on: true })
        .unwrap();
    assert_eq!(editor.document().track(audio).unwrap().audition(), Some(c));
    editor
        .apply(Command::SetAudition { lane: a, on: false })
        .unwrap();
    assert_eq!(
        editor.document().track(audio).unwrap().audition(),
        Some(c),
        "switching off a lane that isn't auditioned changes nothing"
    );
    assert_eq!(editor.undo_history().next(), Some("Audition Take Lane"));
    editor
        .apply(Command::DeleteTakeLanes { lanes: vec![c, d] })
        .unwrap();
    assert_eq!(lanes_of(editor.document(), audio), [a, b]);
    assert_eq!(editor.document().track(audio).unwrap().audition(), None);
    assert_eq!(editor.undo_history().next(), Some("Delete Take Lane(s)"));
    editor.undo().unwrap();
    assert_eq!(editor.document().track(audio).unwrap().audition(), Some(c));

    editor
        .apply(Command::DeleteAllTakeLanes {
            tracks: vec![audio],
        })
        .unwrap();
    assert!(lanes_of(editor.document(), audio).is_empty());
    assert_eq!(editor.undo_history().next(), Some("Delete All Take Lanes"));
}

// Spec: comping R8. Lanes of different tracks can't be moved together.
#[test]
fn lanes_of_two_tracks_are_not_moved_together() {
    let (mut editor, audio, midi) = song();
    let a = lane(&mut editor, audio, "Take 1", vec![]);
    let notes = midi_clip(&mut editor, 0, &[(60, 0)]);
    let m = lane(&mut editor, midi, "Take 1", vec![notes]);
    let error = editor
        .apply(Command::MoveTakeLanes {
            lanes: vec![a, m],
            index: 0,
        })
        .unwrap_err();
    assert!(matches!(
        error,
        bl_model::EditError::Command(ModelError::TakeLanesNotOnOneTrack)
    ));
}

fn tempo() -> TempoMap {
    TempoMap::default()
}

fn track_of(editor: &Editor, id: TrackId) -> &Track {
    editor.document().track(id).unwrap()
}

// Spec: comping R20. A main-lane clip uses a take-lane clip over the stretch
// where they play the same thing: a copy cut from a take matches it, and a
// change to the copy's gain, its envelopes or its notes breaks the match.
#[test]
fn a_copy_cut_from_a_take_uses_it_until_it_is_changed() {
    let (mut editor, audio, midi) = song();
    let take = audio_clip(&mut editor, 0, 4, 0.0);
    let take_id = take.id;
    let first = lane(&mut editor, audio, "Take 1", vec![take.clone()]);
    let second = lane(&mut editor, audio, "Take 2", vec![]);

    // A part of the take from bar 1 to bar 3, as Copy to Main Lane makes it.
    let part = Clip {
        id: editor.new_id(),
        ..take.part(Ticks(BAR), Ticks(3 * BAR), &tempo())
    };
    let part_id = part.id;
    editor
        .apply(Command::CreateClip {
            track: audio,
            clip: part,
        })
        .unwrap();
    let sources = comp::sources(track_of(&editor, audio), &tempo());
    assert_eq!(sources.len(), 1);
    assert_eq!(
        (sources[0].clip, sources[0].lane, sources[0].source),
        (part_id, first, take_id)
    );
    assert_eq!(
        (sources[0].start, sources[0].end),
        (Ticks(BAR), Ticks(3 * BAR))
    );
    assert_eq!(
        comp::unused_lanes(track_of(&editor, audio), &tempo()),
        [second]
    );

    // Trimming the copy, changing its fades or its loop switch keeps the match.
    editor
        .apply(Command::SetClipLength {
            clip: part_id,
            length: Ticks(BAR),
        })
        .unwrap();
    let sources = comp::sources(track_of(&editor, audio), &tempo());
    assert_eq!(
        (sources[0].start, sources[0].end),
        (Ticks(BAR), Ticks(2 * BAR))
    );
    editor
        .apply(Command::SetClipFades {
            clip: part_id,
            fade_in: 0.01,
            fade_out: 0.01,
            fade_in_curve: Some(0.5),
            fade_out_curve: None,
        })
        .unwrap();
    assert_eq!(comp::sources(track_of(&editor, audio), &tempo()).len(), 1);

    // A gain change on the part never matches; deactivating it uses nothing.
    editor
        .apply(Command::SetClipGain {
            clip: part_id,
            gain: 0.5,
        })
        .unwrap();
    assert!(comp::sources(track_of(&editor, audio), &tempo()).is_empty());
    editor
        .apply(Command::SetClipGain {
            clip: part_id,
            gain: 1.0,
        })
        .unwrap();
    assert_eq!(comp::sources(track_of(&editor, audio), &tempo()).len(), 1);
    editor
        .apply(Command::SetClipMute {
            clip: part_id,
            mute: true,
        })
        .unwrap();
    assert!(comp::sources(track_of(&editor, audio), &tempo()).is_empty());

    // MIDI: the same notes at the same places in the song.
    let notes = midi_clip(&mut editor, BAR, &[(60, 0), (64, Ticks::QUARTER.get())]);
    let m = lane(&mut editor, midi, "Take 1", vec![notes.clone()]);
    let copy = Clip {
        id: editor.new_id(),
        ..notes.part(Ticks(BAR), Ticks(2 * BAR), &tempo())
    };
    let copy_id = copy.id;
    editor
        .apply(Command::CreateClip {
            track: midi,
            clip: copy,
        })
        .unwrap();
    let sources = comp::sources(track_of(&editor, midi), &tempo());
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].lane, m);
    editor
        .apply(Command::SetClipData {
            clip: copy_id,
            data: ClipData::Midi(MidiData::new([Note::new(
                61,
                Ticks::ZERO,
                Ticks::QUARTER,
                100,
            )])),
        })
        .unwrap();
    assert!(comp::sources(track_of(&editor, midi), &tempo()).is_empty());
    assert_eq!(comp::unused_lanes(track_of(&editor, midi), &tempo()), [m]);
}

// Spec: comping R7, R20. Delete All Unused Take Lanes keeps the lanes the
// main lane uses.
#[test]
fn deleting_unused_lanes_keeps_those_the_main_lane_uses() {
    let (mut editor, audio, _) = song();
    let take = audio_clip(&mut editor, 0, 4, 0.0);
    let used = lane(&mut editor, audio, "Take 1", vec![take.clone()]);
    let other = audio_clip(&mut editor, 0, 4, 1.0);
    let unused = lane(&mut editor, audio, "Take 2", vec![other]);
    let part = Clip {
        id: editor.new_id(),
        ..take.part(Ticks(BAR), Ticks(2 * BAR), &tempo())
    };
    editor
        .apply(Command::CreateClip {
            track: audio,
            clip: part,
        })
        .unwrap();
    editor
        .apply(Command::DeleteUnusedTakeLanes {
            tracks: vec![audio],
        })
        .unwrap();
    assert_eq!(lanes_of(editor.document(), audio), [used]);
    assert!(editor.document().take_lane(unused).is_none());
    assert_eq!(editor.document().track(audio).unwrap().clips().len(), 1);
    assert_eq!(
        editor.undo_history().next(),
        Some("Delete All Unused Take Lanes")
    );
    assert_eq!(names_of(editor.document(), audio), ["Take 1"]);
}

proptest! {
    // Spec: comping R20. A comp copied from random takes always matches its
    // sources, wherever it is cut, and a gain change on a part never does.
    #[test]
    fn a_comp_copied_from_random_takes_matches_its_sources(
        takes in prop::collection::vec((0i64..8, 1i64..8, 0u8..4), 1..5),
        which in 0usize..5,
        from in 0i64..16,
        length in 1i64..16,
    ) {
        let (mut editor, audio, _) = song();
        let mut made: Vec<Clip> = Vec::new();
        for (n, (start, bars, marker)) in takes.iter().enumerate() {
            let clip = audio_clip(&mut editor, start * BAR, *bars, f64::from(*marker));
            lane(&mut editor, audio, &format!("Take {n}"), vec![clip.clone()]);
            made.push(clip);
        }
        let take = &made[which % made.len()];
        let (from, to) = (
            Ticks(from * BAR / 2).max(take.start),
            Ticks((from + length) * BAR / 2).min(take.end()),
        );
        prop_assume!(from < to);
        let copy = Clip { id: editor.new_id(), ..take.part(from, to, &tempo()) };
        editor.apply(Command::CreateClip { track: audio, clip: copy.clone() }).unwrap();
        let found = comp::sources(track_of(&editor, audio), &tempo());
        prop_assert!(
            found.iter().any(|s| s.clip == copy.id && s.start == copy.start && s.end == copy.end()),
            "no source covers the whole copy: {found:?}"
        );
        // The same part with another gain is never a match.
        editor.apply(Command::SetClipGain { clip: copy.id, gain: 0.5 }).unwrap();
        prop_assert!(comp::sources(track_of(&editor, audio), &tempo()).is_empty());
    }
}
