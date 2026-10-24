//! Linked tracks in the document (WP-6.23, the linked-track editing spec R1
//! to R5, R8, R11, R15 and R16): the groups and how they are saved, and the
//! edits the command layer makes to every track of a group in one undo step.

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, Editor, LinkGroupId,
    NewTakeLane, TakeLaneId, TrackId, TrackKind, registry,
};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

fn track(editor: &mut Editor, kind: TrackKind) -> TrackId {
    let id: TrackId = editor.new_id();
    let index = editor.document().top_level().len();
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
}

fn link(editor: &mut Editor, tracks: &[TrackId]) {
    let group: LinkGroupId = editor.new_id();
    editor
        .apply(Command::SetTrackLinks {
            tracks: tracks.to_vec(),
            group: Some(group),
        })
        .unwrap();
}

/// Three linked audio tracks and a fourth that isn't.
fn song() -> (Editor, [TrackId; 4]) {
    let mut editor = Editor::new();
    let tracks = [(); 4].map(|()| track(&mut editor, TrackKind::Audio));
    link(&mut editor, &tracks[..3]);
    (editor, tracks)
}

fn audio_clip(editor: &mut Editor, track: TrackId, start: i64, bars: i64) -> ClipId {
    let id: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks(start),
                Ticks(bars * BAR),
                ClipData::Audio(AudioData::new(ContentHash([9; 32]))),
            ),
        })
        .unwrap();
    id
}

fn take_lane(editor: &mut Editor, track: TrackId, name: &str) -> TakeLaneId {
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
    id
}

fn run(editor: &mut Editor, id: &str, args: serde_json::Value) -> bl_model::ChangeSet {
    registry::run(editor, id, args).unwrap()
}

fn start(doc: &Document, clip: ClipId) -> i64 {
    doc.clip(clip).unwrap().start.get()
}

// Spec: linked-track editing R1, R16. Link groups are saved with the set,
// and a set saved without them opens with none.
#[test]
fn link_groups_are_saved_and_older_sets_open_unlinked() {
    let (editor, [a, b, c, d]) = song();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let loaded: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(&loaded, editor.document());
    assert_eq!(loaded.linked_with(b), [a, b, c]);
    assert!(loaded.linked_with(d).is_empty());

    let plain = Editor::new();
    let text = serde_json::to_string(plain.document()).unwrap();
    assert!(!text.contains("\"link\""), "an unlinked set writes none");
}

// Spec: linked-track editing R1, R2. A track is in one group at most; only
// audio and MIDI tracks are linked.
#[test]
fn linking_takes_a_track_out_of_its_group_and_refuses_other_kinds() {
    let (mut editor, [a, b, c, d]) = song();
    let midi = track(&mut editor, TrackKind::Midi);
    link(&mut editor, &[c, d, midi]);
    assert_eq!(editor.document().linked_with(c), [c, d, midi]);
    assert_eq!(editor.document().linked_with(a), [a, b]);
    let group = track(&mut editor, TrackKind::Group);
    let before = editor.document().clone();
    let id: LinkGroupId = editor.new_id();
    let refused = editor.apply(Command::SetTrackLinks {
        tracks: vec![a, group],
        group: Some(id),
    });
    assert!(refused.is_err());
    assert_eq!(editor.document(), &before);
}

// Spec: linked-track editing R5. A group left with one track ends; a deleted
// track leaves its group and undo puts it back.
#[test]
fn a_group_left_with_one_track_ends_and_a_deleted_track_leaves_its_group() {
    let (mut editor, [a, b, c, _]) = song();
    editor
        .apply(Command::SetTrackLinks {
            tracks: vec![b],
            group: None,
        })
        .unwrap();
    assert_eq!(editor.undo_history().next(), Some("Unlink Tracks"));
    assert_eq!(editor.document().linked_with(a), [a, c]);

    let before = editor.document().clone();
    editor.apply(Command::DeleteTrack { track: c }).unwrap();
    assert!(
        editor.document().linked_with(a).is_empty(),
        "a group of one ends"
    );
    assert_eq!(editor.document().track(a).unwrap().link(), None);
    editor.undo().unwrap();
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.document().linked_with(a), [a, c]);
}

// Spec: linked-track editing R8, R15. Arming reaches the group, Arm
// Exclusive arms the group and disarms the rest, and `linked: false`
// limits an edit to what it names.
#[test]
fn arming_reaches_the_group() {
    let (mut editor, [a, b, c, d]) = song();
    let armed =
        |editor: &Editor| [a, b, c, d].map(|t| editor.document().track(t).unwrap().mixer().arm);
    let changes = run(
        &mut editor,
        "mixer.set_arm",
        json!({ "track": a, "arm": true }),
    );
    assert_eq!(armed(&editor), [true, true, true, false]);
    assert!(
        changes.tracks.contains_key(&c),
        "the result lists every track changed"
    );
    editor.undo().unwrap();
    assert_eq!(armed(&editor), [false; 4]);

    run(
        &mut editor,
        "mixer.set_arm",
        json!({ "track": d, "arm": true }),
    );
    run(&mut editor, "mixer.arm_exclusive", json!({ "track": b }));
    assert_eq!(armed(&editor), [true, true, true, false]);
    let steps = editor.undo_history().count();
    run(
        &mut editor,
        "mixer.set_arm",
        json!({ "track": a, "arm": false, "linked": false }),
    );
    assert_eq!(armed(&editor), [false, true, true, false]);
    assert_eq!(editor.undo_history().count(), steps + 1);
}

// Spec: linked-track editing R8. Moving and resizing reach the clips that
// start and end where the named one does, by the same amount.
#[test]
fn moving_and_resizing_reach_the_matching_clips() {
    let (mut editor, [a, b, _, d]) = song();
    let on_a = audio_clip(&mut editor, a, 0, 2);
    let on_b = audio_clip(&mut editor, b, 0, 2);
    let shorter = audio_clip(&mut editor, b, 4 * BAR, 1);
    let on_d = audio_clip(&mut editor, d, 0, 2);
    let before = editor.document().clone();

    run(
        &mut editor,
        "clip.move",
        json!({ "clip": on_a, "track": a, "start": BAR }),
    );
    let doc = editor.document();
    assert_eq!((start(doc, on_a), start(doc, on_b)), (BAR, BAR));
    assert_eq!(start(doc, on_d), 0, "an unlinked track keeps its clip");
    assert_eq!(start(doc, shorter), 4 * BAR);
    editor.undo().unwrap();
    assert_eq!(
        editor.document(),
        &before,
        "one undo step restores every track"
    );

    run(
        &mut editor,
        "clip.set_length",
        json!({ "clip": on_b, "length": BAR }),
    );
    assert_eq!(editor.document().clip(on_a).unwrap().length.get(), BAR);
    run(
        &mut editor,
        "clip.set_length",
        json!({ "clip": on_a, "length": 2 * BAR, "linked": false }),
    );
    assert_eq!(editor.document().clip(on_b).unwrap().length.get(), BAR);
}

// Spec: linked-track editing R8. Only fades that start at the same place are
// adjusted together: a fade-in matches a fade-in, a fade-out a fade-out.
#[test]
fn fades_follow_where_they_start() {
    let (mut editor, [a, b, ..]) = song();
    let on_a = audio_clip(&mut editor, a, 0, 4);
    let same = audio_clip(&mut editor, b, 0, 4);
    let ends_elsewhere = audio_clip(&mut editor, a, 8 * BAR, 2);
    let starts_elsewhere = audio_clip(&mut editor, b, 7 * BAR, 3);
    let fades = |doc: &Document, clip| {
        let ClipData::Audio(audio) = &doc.clip(clip).unwrap().data else {
            unreachable!()
        };
        (audio.fade_in, audio.fade_out)
    };
    run(
        &mut editor,
        "clip.set_fades",
        json!({ "clip": on_a, "fade_in": 0.5, "fade_out": 0.25 }),
    );
    assert_eq!(fades(editor.document(), same), (0.5, 0.25));
    assert_eq!(fades(editor.document(), ends_elsewhere), (0.0, 0.0));

    // Starts differ, ends are the same: only the fade-out is shared.
    run(
        &mut editor,
        "clip.set_fades",
        json!({ "clip": ends_elsewhere, "fade_in": 0.1, "fade_out": 0.2 }),
    );
    assert_eq!(fades(editor.document(), starts_elsewhere), (0.0, 0.2));
    assert_eq!(fades(editor.document(), on_a), (0.5, 0.25));
}

// Spec: linked-track editing R11. Renaming, deleting and auditioning take
// lanes reach the lane at the same place on each linked track, and a track
// with no lane there is left alone.
#[test]
fn take_lane_edits_reach_the_lane_at_the_same_place() {
    let (mut editor, [a, b, c, d]) = song();
    let a1 = take_lane(&mut editor, a, "A1");
    let a2 = take_lane(&mut editor, a, "A2");
    let b1 = take_lane(&mut editor, b, "B1");
    let c1 = take_lane(&mut editor, c, "C1");
    let c2 = take_lane(&mut editor, c, "C2");
    let d1 = take_lane(&mut editor, d, "D1");
    let before = editor.document().clone();

    run(
        &mut editor,
        "take_lane.rename",
        json!({ "lane": a1, "name": "Take" }),
    );
    let doc = editor.document();
    assert_eq!(
        [a1, b1, c1, d1].map(|l| doc.take_lane(l).unwrap().name.clone()),
        ["Take", "Take", "Take", "D1"]
    );
    editor.undo().unwrap();
    assert_eq!(editor.document(), &before);

    run(
        &mut editor,
        "take_lane.rename",
        json!({ "lane": a2, "name": "Two" }),
    );
    assert_eq!(editor.document().take_lane(c2).unwrap().name, "Two");
    assert_eq!(editor.document().take_lane(b1).unwrap().name, "B1");

    run(
        &mut editor,
        "take_lane.set_audition",
        json!({ "lane": a1, "on": true }),
    );
    let doc = editor.document();
    assert_eq!(
        [a, b, c, d].map(|t| doc.track(t).unwrap().audition()),
        [Some(a1), Some(b1), Some(c1), None]
    );

    run(&mut editor, "take_lane.delete", json!({ "lanes": [a2] }));
    let doc = editor.document();
    assert!(doc.take_lane(a2).is_none() && doc.take_lane(c2).is_none());
    assert!(doc.take_lane(c1).is_some() && doc.take_lane(d1).is_some());
    editor.undo().unwrap();
    assert!(editor.document().take_lane(c2).is_some());

    run(
        &mut editor,
        "take_lane.delete",
        json!({ "lanes": [a1], "linked": false }),
    );
    assert!(editor.document().take_lane(b1).is_some());
}
