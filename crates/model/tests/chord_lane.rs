//! The chord lane in the model (WP-6.48): its regions, each edit one undo
//! step, the tracks' switch and what a set file saves. Spec: chord-lane R2
//! to R5, R10.

use bl_core::time::Ticks;
use bl_model::{
    ChordRegion, ChordRegionId, Command, Document, EditError, Editor, ModelError, Scale, ScaleName,
    TrackId, TrackKind,
};

const BAR: Ticks = Ticks::WHOLE;

/// A song with a MIDI track and an audio track.
fn song() -> (Editor, TrackId, TrackId) {
    let mut editor = Editor::new();
    let [midi, audio] = [TrackKind::Midi, TrackKind::Audio].map(|kind| {
        let id: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        id
    });
    (editor, midi, audio)
}

fn add(editor: &mut Editor, start: Ticks, end: Ticks) -> Result<ChordRegionId, EditError> {
    let id: ChordRegionId = editor.new_id();
    editor.apply(Command::AddChordRegion {
        region: ChordRegion::new(id, start, end),
    })?;
    Ok(id)
}

#[test]
fn a_region_is_added_moved_set_and_deleted_one_undo_step_each() {
    // Spec: chord-lane R2, R3, R4
    let (mut editor, ..) = song();
    let steps = |editor: &Editor| editor.undo_history().len();
    let before = steps(&editor);
    let region = add(&mut editor, BAR, BAR * 2).unwrap();
    assert_eq!(steps(&editor), before + 1);
    editor
        .apply(Command::MoveChordRegion {
            region,
            start: BAR * 2,
            end: BAR * 4,
        })
        .unwrap();
    editor
        .apply(Command::SetChordRegion {
            region,
            enabled: Some(true),
            root: Some(2),
            name: Some("dorian".to_owned()),
            transpose: Some(-2),
        })
        .unwrap();
    assert_eq!(steps(&editor), before + 3);
    let held = *editor.document().chord_region(region).unwrap();
    assert_eq!((held.start, held.end), (BAR * 2, BAR * 4));
    assert_eq!(held.scale, Scale::new(2, ScaleName::Dorian));
    assert_eq!(held.transpose, -2);
    // A set leaves what it isn't given as it was.
    editor
        .apply(Command::SetChordRegion {
            region,
            enabled: None,
            root: None,
            name: None,
            transpose: Some(5),
        })
        .unwrap();
    let held = *editor.document().chord_region(region).unwrap();
    assert_eq!(held.scale, Scale::new(2, ScaleName::Dorian));
    assert_eq!(held.transpose, 5);
    editor.apply(Command::DeleteChordRegion { region }).unwrap();
    assert!(editor.document().chord_lane().is_empty());
    editor.undo().unwrap();
    assert_eq!(editor.document().chord_lane().len(), 1);
    editor.undo().unwrap();
    editor.undo().unwrap();
    editor.undo().unwrap();
    assert_eq!(editor.document().chord_lane()[0].start, BAR);
    editor.undo().unwrap();
    assert!(editor.document().chord_lane().is_empty());
}

#[test]
fn regions_never_overlap_and_a_refused_edit_changes_nothing() {
    // Spec: chord-lane R2, R3
    let (mut editor, ..) = song();
    let first = add(&mut editor, BAR, BAR * 2).unwrap();
    let second = add(&mut editor, BAR * 2, BAR * 3).unwrap();
    let before = editor.document().clone();
    for (start, end) in [
        (BAR * 2 - Ticks(1), BAR * 3),
        (ZERO, BAR * 4),
        (BAR, BAR * 2),
    ] {
        let id: ChordRegionId = editor.new_id();
        let refused = editor.apply(Command::AddChordRegion {
            region: ChordRegion::new(id, start, end),
        });
        assert!(
            matches!(
                refused,
                Err(EditError::Command(ModelError::ChangeInUse { .. }))
            ),
            "{start:?}..{end:?}: {refused:?}"
        );
    }
    let refused = editor.apply(Command::MoveChordRegion {
        region: first,
        start: BAR,
        end: BAR * 2 + Ticks(1),
    });
    assert!(refused.is_err());
    // Moving a region over its own old place is fine, and so is touching.
    editor
        .apply(Command::MoveChordRegion {
            region: second,
            start: BAR * 2 + Ticks(1),
            end: BAR * 3,
        })
        .unwrap();
    assert_ne!(editor.document(), &before);
    let lane: Vec<_> = editor
        .document()
        .chord_lane()
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(lane, [first, second]);
}

const ZERO: Ticks = Ticks::ZERO;

#[test]
fn a_region_that_breaks_a_rule_is_refused() {
    // Spec: chord-lane R2
    let (mut editor, ..) = song();
    assert!(
        add(&mut editor, BAR * 2, BAR).is_err(),
        "ends before it starts"
    );
    assert!(add(&mut editor, BAR, BAR).is_err(), "is empty");
    assert!(
        add(&mut editor, Ticks(-1), BAR).is_err(),
        "starts before 1.1.1"
    );
    let region = add(&mut editor, BAR, BAR * 2).unwrap();
    for transpose in [25, -25, 300] {
        let refused = editor.apply(Command::SetChordRegion {
            region,
            enabled: None,
            root: None,
            name: None,
            transpose: Some(transpose),
        });
        assert!(refused.is_err(), "{transpose}");
    }
    for transpose in [24, -24] {
        editor
            .apply(Command::SetChordRegion {
                region,
                enabled: None,
                root: None,
                name: None,
                transpose: Some(transpose),
            })
            .unwrap();
    }
    let bad_root = editor.apply(Command::SetChordRegion {
        region,
        enabled: None,
        root: Some(12),
        name: None,
        transpose: None,
    });
    assert!(bad_root.is_err());
    let missing: ChordRegionId = editor.new_id();
    assert!(
        editor
            .apply(Command::DeleteChordRegion { region: missing })
            .is_err()
    );
}

#[test]
fn only_a_midi_track_follows_the_lane() {
    // Spec: chord-lane R5
    let (mut editor, midi, audio) = song();
    assert!(!editor.document().track(midi).unwrap().follow_chord_lane());
    assert!(
        editor
            .apply(Command::SetFollowChordLane {
                track: audio,
                follow: true
            })
            .is_err()
    );
    let before = editor.undo_history().len();
    editor
        .apply(Command::SetFollowChordLane {
            track: midi,
            follow: true,
        })
        .unwrap();
    assert_eq!(editor.undo_history().len(), before + 1);
    assert!(editor.document().track(midi).unwrap().follow_chord_lane());
    editor.undo().unwrap();
    assert!(!editor.document().track(midi).unwrap().follow_chord_lane());
}

#[test]
fn the_lane_and_the_switch_are_saved_and_a_set_from_before_loads_with_neither() {
    // Spec: chord-lane R10
    let (mut editor, midi, _) = song();
    let saved = serde_json::to_value(editor.document()).unwrap();
    assert!(saved["settings"].get("chord_lane").is_none());
    assert!(!saved.to_string().contains("follow_chord_lane"));
    let region = add(&mut editor, BAR, BAR * 2).unwrap();
    editor
        .apply(Command::SetChordRegion {
            region,
            enabled: Some(true),
            root: Some(7),
            name: Some("minor".to_owned()),
            transpose: Some(3),
        })
        .unwrap();
    editor
        .apply(Command::SetFollowChordLane {
            track: midi,
            follow: true,
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    assert_eq!(saved["settings"]["chord_lane"][0]["transpose"], 3);
    let loaded: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(&loaded, editor.document());
    assert!(loaded.track(midi).unwrap().follow_chord_lane());
    assert_eq!(
        loaded.chord_lane()[0].scale,
        Scale::new(7, ScaleName::Minor)
    );
}

#[test]
fn a_saved_lane_with_overlapping_regions_or_a_shared_id_does_not_load() {
    // Spec: chord-lane R2, R10
    let (mut editor, ..) = song();
    add(&mut editor, BAR, BAR * 2).unwrap();
    add(&mut editor, BAR * 3, BAR * 4).unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let mut overlapping = saved.clone();
    overlapping["settings"]["chord_lane"][1]["start"] = serde_json::json!(BAR.0 + 1);
    assert!(serde_json::from_value::<Document>(overlapping).is_err());
    let mut shared = saved.clone();
    shared["settings"]["chord_lane"][1]["id"] = shared["settings"]["chord_lane"][0]["id"].clone();
    assert!(serde_json::from_value::<Document>(shared).is_err());
    // A lane written out of order loads in order, which playback relies on.
    let mut reversed = saved;
    reversed["settings"]["chord_lane"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let loaded = serde_json::from_value::<Document>(reversed).unwrap();
    assert!(loaded.chord_lane()[0].start < loaded.chord_lane()[1].start);
}
