//! The scales in the model (WP-6.7): every clip carries a scale, the
//! commands that set it, and what a set file saves. Spec: scales R3 to R8.

use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, EditError, Editor, MidiData, ModelError, Scale, ScaleName,
    TrackId, TrackKind,
};

/// A MIDI track with a clip on it.
fn song() -> (Editor, TrackId, ClipId) {
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Keys".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::default()),
            ),
        })
        .unwrap();
    (editor, track, clip)
}

/// An audio clip on an audio track.
fn audio_song() -> (Editor, TrackId, ClipId) {
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: "Audio".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Audio(bl_model::AudioData::new(bl_model::ContentHash([7; 32]))),
            ),
        })
        .unwrap();
    (editor, track, clip)
}

#[test]
fn every_kind_of_clip_carries_a_scale() {
    // Spec: scales R3
    let (mut editor, _track, clip) = song();
    assert_eq!(
        editor.document().clip(clip).unwrap().scale,
        Scale::default(),
        "a new clip is off, at C, in Major"
    );
    let audio_track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: audio_track,
            kind: TrackKind::Audio,
            name: "Audio".to_owned(),
            parent: None,
            index: 1,
        })
        .unwrap();
    let audio: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: audio_track,
            clip: Clip::new(
                audio,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Audio(bl_model::AudioData::new(bl_model::ContentHash([9; 32]))),
            ),
        })
        .unwrap();
    assert_eq!(
        editor.document().clip(audio).unwrap().scale,
        Scale::default(),
        "an audio clip carries a scale too"
    );
    let (editor, _, _) = audio_song();
    assert_eq!(editor.document().clip_count(), 1);
}

#[test]
fn a_clip_set_scale_is_one_undo_step() {
    // Spec: scales R5
    let (mut editor, track, first) = song();
    let mut clips = vec![first];
    for _ in 0..2 {
        let clip: ClipId = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(MidiData::default()),
                ),
            })
            .unwrap();
        clips.push(clip);
    }
    let steps = editor.undo_history().len();
    editor
        .apply(Command::SetClipScale {
            clips: clips.clone(),
            enabled: Some(true),
            root: Some(2),
            name: Some("dorian".to_owned()),
        })
        .unwrap();
    assert_eq!(editor.undo_history().len(), steps + 1, "one undo step");
    assert_eq!(editor.undo_history().next(), Some("Change Clip Scale"));
    for clip in &clips {
        assert_eq!(
            editor.document().clip(*clip).unwrap().scale,
            Scale::new(2, ScaleName::Dorian)
        );
    }
    editor.undo().unwrap();
    for clip in &clips {
        assert_eq!(
            editor.document().clip(*clip).unwrap().scale,
            Scale::default()
        );
    }
}

#[test]
fn a_clip_set_scale_leaves_the_values_it_is_not_given() {
    // Spec: scales R5, R6
    let (mut editor, _, clip) = song();
    editor
        .apply(Command::SetClipScale {
            clips: vec![clip],
            enabled: Some(true),
            root: Some(5),
            name: Some("Minor".to_owned()),
        })
        .unwrap();
    editor
        .apply(Command::SetClipScale {
            clips: vec![clip],
            root: Some(7),
            name: None,
            enabled: None,
        })
        .unwrap();
    let scale = editor.document().clip(clip).unwrap().scale;
    assert!(scale.enabled, "the switch stays on");
    assert_eq!(scale.root, 7);
    assert_eq!(scale.name, ScaleName::Minor, "the name stays");
}

#[test]
fn a_clip_set_scale_refuses_a_bad_name_or_root() {
    // Spec: scales R5
    let (mut editor, _, clip) = song();
    let before = editor.document().clone();
    let error = editor
        .apply(Command::SetClipScale {
            clips: vec![clip],
            enabled: None,
            root: None,
            name: Some("Bogus".to_owned()),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        EditError::Command(ModelError::InvalidInput(_))
    ));
    let error = editor
        .apply(Command::SetClipScale {
            clips: vec![clip],
            enabled: None,
            root: Some(12),
            name: None,
        })
        .unwrap_err();
    assert!(matches!(
        error,
        EditError::Command(ModelError::InvalidValue {
            what: "root note",
            ..
        })
    ));
    assert_eq!(editor.document(), &before, "nothing changed");
}

#[test]
fn a_new_clip_takes_the_set_s_current_scale() {
    // Spec: scales R7, R9
    let (mut editor, track, _) = song();
    editor
        .apply(Command::SetScale {
            enabled: Some(true),
            root: Some(9),
            name: Some("mixolydian".to_owned()),
        })
        .unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::default()),
            ),
        })
        .unwrap();
    assert_eq!(
        editor.document().clip(clip).unwrap().scale,
        Scale::new(9, ScaleName::Mixolydian)
    );
}

#[test]
fn a_clip_created_with_a_scale_keeps_it() {
    // Spec: scales R7, R13. The command reference lists a clip's scale as
    // a field a caller may name, so `clip.create` keeps the one it is
    // given rather than the set's current scale.
    let (mut editor, track, _) = song();
    editor
        .apply(Command::SetScale {
            enabled: Some(true),
            root: Some(9),
            name: Some("mixolydian".to_owned()),
        })
        .unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip {
                scale: Scale::new(4, ScaleName::Minor),
                ..Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(MidiData::default()),
                )
            },
        })
        .unwrap();
    assert_eq!(
        editor.document().clip(clip).unwrap().scale,
        Scale::new(4, ScaleName::Minor),
        "the scale the caller named is kept"
    );
}

#[test]
fn the_set_s_current_scale_is_saved_and_loaded_back() {
    // Spec: scales R8, R9
    let (mut editor, _, _) = song();
    assert_eq!(
        editor.document().settings().scale,
        Scale::default(),
        "a new set's scale is off, at C, in Major"
    );
    editor
        .apply(Command::SetScale {
            enabled: Some(true),
            root: Some(2),
            name: Some("dorian".to_owned()),
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    assert_eq!(
        saved["settings"]["scale"],
        serde_json::json!({ "enabled": true, "root": 2, "name": "dorian" })
    );
    let loaded: bl_model::Document = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.settings().scale, Scale::new(2, ScaleName::Dorian));
}

#[test]
fn a_set_saved_before_the_scales_loads_unchanged() {
    // Spec: scales R8
    let (editor, _, _) = song();
    let saved = serde_json::to_value(editor.document()).unwrap();
    assert!(
        saved["settings"].get("scale").is_none(),
        "the default scale isn't written"
    );
    let loaded: bl_model::Document = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.settings().scale, Scale::default());
}

#[test]
fn an_unknown_saved_scale_name_loads_as_major() {
    // Spec: scales R8
    let (editor, _, _) = song();
    let mut saved = serde_json::to_value(editor.document()).unwrap();
    saved["settings"]["scale"] = serde_json::json!({
        "enabled": true,
        "root": 5,
        "name": "a scale nobody knows"
    });
    let loaded: bl_model::Document = serde_json::from_value(saved).unwrap();
    assert_eq!(
        loaded.settings().scale,
        Scale {
            enabled: true,
            root: 5,
            name: ScaleName::Major
        }
    );
}

#[test]
fn the_current_scale_following_a_selection_is_no_undo_step() {
    // Spec: scales R12
    let (mut editor, _, clip) = song();
    editor
        .apply(Command::SetClipScale {
            clips: vec![clip],
            enabled: Some(true),
            root: Some(2),
            name: Some("dorian".to_owned()),
        })
        .unwrap();
    let steps = editor.undo_history().len();
    editor.follow_scale(Scale::new(2, ScaleName::Dorian));
    assert_eq!(editor.undo_history().len(), steps, "no undo step");
    assert_eq!(
        editor.document().settings().scale,
        Scale::new(2, ScaleName::Dorian)
    );
    // Undoing the clip's scale leaves the current scale as it is.
    editor.undo().unwrap();
    assert_eq!(
        editor.document().settings().scale,
        Scale::new(2, ScaleName::Dorian)
    );
}

#[test]
fn redoing_a_step_leaves_a_scale_followed_since_it_was_undone() {
    // Spec: scales R12
    let (mut editor, _, _) = song();
    editor.undo().unwrap();
    editor.follow_scale(Scale::new(2, ScaleName::Minor));
    editor.redo().unwrap();
    assert_eq!(
        editor.document().settings().scale,
        Scale::new(2, ScaleName::Minor)
    );
}
