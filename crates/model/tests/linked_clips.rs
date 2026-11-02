//! Linked clips in the model (WP-6.44): clips that share their contents,
//! and what keeps them equal. Spec: linked-clips R2 to R9, R13 and R15.

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, Editor, MidiData, Note,
    SceneId, TrackId, TrackKind,
};
use serde_json::json;

const BAR: Ticks = Ticks::WHOLE;

fn track(editor: &mut Editor, kind: TrackKind, index: usize) -> TrackId {
    let id: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind,
            name: String::new(),
            parent: None,
            index,
        })
        .unwrap();
    id
}

fn notes(keys: &[u8]) -> ClipData {
    ClipData::Midi(MidiData::new(keys.iter().enumerate().map(|(at, key)| {
        Note::new(*key, Ticks(at as i64 * 240), Ticks(240), 100)
    })))
}

fn midi_clip(editor: &mut Editor, track: TrackId, start: Ticks, keys: &[u8]) -> ClipId {
    let id: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(id, start, BAR * 2, notes(keys)),
        })
        .unwrap();
    id
}

fn slot_clip(editor: &mut Editor, track: TrackId, scene: SceneId, keys: &[u8]) -> ClipId {
    let id: ClipId = editor.new_id();
    editor
        .apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(id, Ticks::ZERO, BAR * 2, notes(keys)),
        })
        .unwrap();
    id
}

fn keys(doc: &Document, clip: ClipId) -> Vec<u8> {
    let ClipData::Midi(midi) = &doc.clip(clip).unwrap().data else {
        panic!("not a MIDI clip");
    };
    let mut keys: Vec<u8> = midi.notes.iter().map(|note| note.key).collect();
    keys.sort_unstable();
    keys
}

fn link(editor: &mut Editor, clips: &[ClipId]) {
    editor
        .apply(Command::LinkClips {
            clips: clips.to_vec(),
        })
        .unwrap();
}

/// Replaces the notes of `clip` with `keys`, as a note edit does.
fn set_notes(editor: &mut Editor, clip: ClipId, keys: &[u8]) {
    let mut data = editor.document().clip(clip).unwrap().data.clone();
    let ClipData::Midi(midi) = &mut data else {
        panic!("not a MIDI clip");
    };
    *midi = MidiData {
        notes: MidiData::new(
            keys.iter()
                .enumerate()
                .map(|(at, key)| Note::new(*key, Ticks(at as i64 * 240), Ticks(240), 100)),
        )
        .notes,
        ..midi.clone()
    };
    editor.apply(Command::SetClipData { clip, data }).unwrap();
}

/// Two MIDI tracks and a slot: three linked clips holding keys 60 and 64.
fn three() -> (Editor, [ClipId; 3]) {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Midi, 0);
    let b = track(&mut editor, TrackKind::Midi, 1);
    let scene = editor.document().scenes().next().unwrap().id;
    let first = midi_clip(&mut editor, a, Ticks::ZERO, &[60, 64]);
    let second = midi_clip(&mut editor, b, BAR * 4, &[60, 64]);
    let third = slot_clip(&mut editor, a, scene, &[60, 64]);
    link(&mut editor, &[first, second, third]);
    (editor, [first, second, third])
}

// Spec: linked-clips R2, R4
#[test]
fn a_note_edit_on_one_of_three_linked_clips_reaches_all_three_in_one_undo_step() {
    let (mut editor, clips) = three();
    let steps = editor.undo_history().len();
    set_notes(&mut editor, clips[1], &[60, 64, 67]);
    for clip in clips {
        assert_eq!(keys(editor.document(), clip), [60, 64, 67]);
    }
    assert_eq!(editor.undo_history().len(), steps + 1);
    editor.undo().unwrap();
    for clip in clips {
        assert_eq!(keys(editor.document(), clip), [60, 64]);
    }
    editor.redo().unwrap();
    for clip in clips {
        assert_eq!(keys(editor.document(), clip), [60, 64, 67]);
    }
    editor.document().check_invariants().unwrap();
}

// Spec: linked-clips R3
#[test]
fn a_clip_keeps_its_own_place_markers_and_name() {
    let (mut editor, clips) = three();
    editor
        .apply(Command::SetMidiClipRegion {
            clip: clips[0],
            start: Ticks(480),
            end: Some(BAR),
        })
        .unwrap();
    editor
        .apply(Command::RenameClip {
            clip: clips[0],
            name: "Verse".to_owned(),
        })
        .unwrap();
    set_notes(&mut editor, clips[2], &[48]);
    let doc = editor.document();
    let ClipData::Midi(own) = &doc.clip(clips[0]).unwrap().data else {
        panic!("not MIDI");
    };
    assert_eq!((own.start, own.end), (Ticks(480), Some(BAR)));
    assert_eq!(doc.clip(clips[0]).unwrap().name, "Verse");
    assert_eq!(doc.clip(clips[1]).unwrap().name, "");
    assert_eq!(doc.clip(clips[1]).unwrap().start, BAR * 4);
    assert_eq!(keys(doc, clips[0]), [48]);
}

// Spec: linked-clips R2
#[test]
fn only_clips_that_hold_the_same_contents_link() {
    let mut editor = Editor::new();
    let midi = track(&mut editor, TrackKind::Midi, 0);
    let audio = track(&mut editor, TrackKind::Audio, 1);
    let a = midi_clip(&mut editor, midi, Ticks::ZERO, &[60]);
    let b = midi_clip(&mut editor, midi, BAR * 2, &[61]);
    let sound: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: audio,
            clip: Clip::new(
                sound,
                Ticks::ZERO,
                BAR,
                ClipData::Audio(AudioData::new(ContentHash([1; 32]))),
            ),
        })
        .unwrap();
    for clips in [vec![a, b], vec![a, sound], vec![a]] {
        let before = editor.document().clone();
        assert!(editor.apply(Command::LinkClips { clips }).is_err());
        assert_eq!(*editor.document(), before);
    }
}

// Spec: linked-clips R5
#[test]
fn crop_leaves_the_others_notes_whole() {
    let (mut editor, clips) = three();
    editor
        .apply(Command::CropClip {
            clip: clips[0],
            media: None,
            from: 0.0,
            to: 240.0,
        })
        .unwrap();
    assert_eq!(keys(editor.document(), clips[0]), [60]);
    assert_eq!(keys(editor.document(), clips[1]), [60, 64]);
    assert_eq!(keys(editor.document(), clips[2]), [60, 64]);
    assert_eq!(editor.document().clip(clips[0]).unwrap().link, None);
    assert_eq!(editor.document().linked_clip_ids(clips[1]), [clips[2]]);
    editor.document().check_invariants().unwrap();
}

// Spec: linked-clips R6
#[test]
fn a_split_gives_two_clips_in_the_link() {
    let (mut editor, clips) = three();
    editor
        .apply(Command::InsertTime {
            at: BAR,
            length: BAR,
            clips: Vec::new(),
        })
        .unwrap();
    let doc = editor.document();
    let link = doc.clip(clips[0]).unwrap().link;
    assert!(link.is_some());
    // The first clip crossed the insert point and became two; the second
    // clip and the slot clip are as they were.
    assert_eq!(doc.link_members(link.unwrap()).len(), 4);
    doc.check_invariants().unwrap();
}

// Spec: linked-clips R7
#[test]
fn a_clip_made_by_a_command_is_independent() {
    let (mut editor, clips) = three();
    let track = editor.document().clip_track(clips[0]).unwrap();
    let copy: ClipId = editor.new_id();
    let mut made = editor.document().clip(clips[0]).unwrap().clone();
    made.id = copy;
    made.start = BAR * 8;
    assert!(made.link.is_some());
    editor
        .apply(Command::CreateClip { track, clip: made })
        .unwrap();
    assert_eq!(editor.document().clip(copy).unwrap().link, None);
    set_notes(&mut editor, copy, &[1]);
    assert_eq!(keys(editor.document(), clips[0]), [60, 64]);

    let scene = editor.document().scenes().next().unwrap().id;
    let tracks: Vec<TrackId> = editor
        .document()
        .tracks()
        .filter(|track| track.slot(scene).is_some())
        .map(|track| track.id())
        .collect();
    let (new_scene, new_clip): (SceneId, ClipId) = (editor.new_id(), editor.new_id());
    editor
        .apply(Command::DuplicateScene {
            scene,
            id: new_scene,
            clips: tracks.into_iter().map(|t| (t, new_clip)).collect(),
        })
        .unwrap();
    assert_eq!(editor.document().clip(new_clip).unwrap().link, None);
}

// Spec: linked-clips R8, R9
#[test]
fn unlinking_and_deleting_down_to_one_clip_end_the_link() {
    let (mut editor, clips) = three();
    editor
        .apply(Command::UnlinkClips {
            clips: vec![clips[0]],
        })
        .unwrap();
    set_notes(&mut editor, clips[0], &[1]);
    assert_eq!(keys(editor.document(), clips[1]), [60, 64]);
    editor
        .apply(Command::DeleteClip { clip: clips[1] })
        .unwrap();
    let doc = editor.document();
    assert_eq!(doc.clip(clips[2]).unwrap().link, None);
    assert!(doc.linked_clip_ids(clips[2]).is_empty());
    doc.check_invariants().unwrap();
}

// Spec: linked-clips R9
#[test]
fn moving_a_clip_keeps_it_in_its_link() {
    let (mut editor, clips) = three();
    let other = editor.document().clip_track(clips[1]).unwrap();
    editor
        .apply(Command::MoveClip {
            clip: clips[0],
            track: other,
            start: BAR * 10,
        })
        .unwrap();
    assert_eq!(editor.document().linked_clip_ids(clips[0]).len(), 2);
}

// Spec: linked-clips R13
#[test]
fn a_set_saves_each_links_contents_once_and_loads_the_links_back() {
    let (editor, clips) = three();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let text = saved.to_string();
    // Key 64's note appears once in the set, in the link's contents.
    assert_eq!(text.matches("\"key\":64").count(), 1, "{text}");
    assert_eq!(saved["links"].as_array().unwrap().len(), 1);
    let loaded: Document = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(&loaded, editor.document());
    for clip in clips {
        assert_eq!(
            loaded.clip(clip).unwrap().link,
            editor.document().clip(clip).unwrap().link
        );
        assert_eq!(keys(&loaded, clip), [60, 64]);
    }

    // A clip naming a link the set doesn't hold doesn't load.
    let mut broken = saved.clone();
    broken["links"] = json!([]);
    assert!(serde_json::from_value::<Document>(broken).is_err());
    // A link whose clips aren't of one kind doesn't load.
    let mut mixed = saved;
    mixed["links"][0]["contents"]["data"] = json!({ "audio": { "media": "00".repeat(32) } });
    assert!(serde_json::from_value::<Document>(mixed).is_err());
}

// Spec: linked-clips R4, R13
#[test]
fn a_reverse_mirrors_each_linked_clips_own_markers() {
    let mut editor = Editor::new();
    let audio = track(&mut editor, TrackKind::Audio, 0);
    let mut ids = Vec::new();
    for (start, region) in [(Ticks::ZERO, 0.0), (BAR * 4, 1.0)] {
        let id: ClipId = editor.new_id();
        let mut data = AudioData::new(ContentHash([3; 32]));
        data.start = region;
        data.end = Some(region + 2.0);
        editor
            .apply(Command::CreateClip {
                track: audio,
                clip: Clip::new(id, start, BAR, ClipData::Audio(data)),
            })
            .unwrap();
        ids.push(id);
    }
    link(&mut editor, &ids);
    editor
        .apply(Command::ReverseClip {
            clip: ids[0],
            media: Some(ContentHash([4; 32])),
            seconds: 10.0,
        })
        .unwrap();
    let region = |clip| match &editor.document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => (audio.media, audio.start, audio.end),
        ClipData::Midi(_) => unreachable!(),
    };
    assert_eq!(region(ids[0]), (ContentHash([4; 32]), 8.0, None));
    assert_eq!(region(ids[1]), (ContentHash([4; 32]), 7.0, Some(9.0)));
}

/// Two audio clips of one file on two tracks, linked, with Warp on.
fn warped_pair() -> (Editor, [ClipId; 2]) {
    let mut editor = Editor::new();
    let mut clips = Vec::new();
    for index in 0..2 {
        let audio = track(&mut editor, TrackKind::Audio, index);
        let id: ClipId = editor.new_id();
        let mut data = AudioData::new(ContentHash([5; 32]));
        data.end = Some(8.0);
        editor
            .apply(Command::CreateClip {
                track: audio,
                clip: Clip::new(id, BAR * 4 * index as i64, BAR * 2, ClipData::Audio(data)),
            })
            .unwrap();
        clips.push(id);
    }
    link(&mut editor, &clips);
    (editor, [clips[0], clips[1]])
}

fn audio_of(editor: &Editor, clip: ClipId) -> AudioData {
    match &editor.document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!(),
    }
}

// Spec: linked-clips R3, R4
#[test]
fn a_warp_marker_moved_on_one_linked_audio_clip_moves_on_both() {
    let (mut editor, [a, b]) = warped_pair();
    editor
        .apply(Command::SetClipRegion {
            clip: b,
            start: 1.0,
            end: Some(5.0),
            ticks: false,
        })
        .unwrap();
    editor
        .apply(Command::SetClipLoop {
            clip: b,
            start: 2.0,
            end: Some(3.0),
            ticks: false,
        })
        .unwrap();
    // Warp goes on in both, then a marker is put in and moved through one.
    editor
        .apply(Command::SetClipWarp {
            clip: a,
            warp: true,
        })
        .unwrap();
    assert!(audio_of(&editor, b).warp);
    editor
        .apply(Command::InsertWarpMarker {
            clips: vec![a],
            beat: Some(Ticks(960)),
            time: None,
            pin_neighbours: false,
            transients: None,
        })
        .unwrap();
    assert_eq!(audio_of(&editor, a).warp_markers.len(), 2);
    assert_eq!(
        audio_of(&editor, b).warp_markers,
        audio_of(&editor, a).warp_markers
    );
    let before = audio_of(&editor, b).warp_markers.clone();
    editor
        .apply(Command::MoveWarpMarkers {
            clips: vec![a],
            markers: vec![1],
            by: Ticks(240),
        })
        .unwrap();
    assert_ne!(audio_of(&editor, b).warp_markers, before);
    assert_eq!(
        audio_of(&editor, b).warp_markers,
        audio_of(&editor, a).warp_markers
    );
    editor.document().check_invariants().unwrap();
    editor.undo().unwrap();
    assert_eq!(audio_of(&editor, b).warp_markers, before);
}

// Spec: linked-clips R3, R4
#[test]
fn new_contents_with_warp_off_take_each_linked_clips_markers_back_to_seconds() {
    let (mut editor, [a, b]) = warped_pair();
    editor
        .apply(Command::SetClipWarp {
            clip: a,
            warp: true,
        })
        .unwrap();
    assert!(audio_of(&editor, b).warp);
    // Unwarped contents with no warp map: the other clip's markers leave
    // the map they were in, not the new one.
    let mut data = AudioData::new(ContentHash([5; 32]));
    data.end = Some(8.0);
    editor
        .apply(Command::SetClipData {
            clip: a,
            data: ClipData::Audio(data),
        })
        .unwrap();
    let audio = audio_of(&editor, b);
    assert!(!audio.warp);
    assert_eq!(audio.end, Some(8.0));
    editor.document().check_invariants().unwrap();
}
