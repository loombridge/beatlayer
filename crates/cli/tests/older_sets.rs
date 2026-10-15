//! Sets saved before WP-1.4b (schema version 1) kept a split or trimmed
//! audio clip's place in its file as an offset in ticks. They load through
//! the project's migrations with the offset moved into the clip's Start
//! marker, as Live moves it, and play exactly what they played (the audio
//! clips spec, R16).

use std::sync::Arc;

use bl_core::id::IdAllocator;
use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_media::SampleData;
use bl_model::{AudioData, Clip, ClipData, Command, ContentHash, Document, Editor, TrackKind};
use bl_project::{LoadOptions, Media, Project, decode_json, encode_json};
use serde_json::{Value, json};

const MEDIA: ContentHash = ContentHash([3; 32]);

/// A file whose every frame says which it is.
fn media() -> MediaMap {
    let left: Vec<f32> = (0..200_000).map(|i| (i + 1) as f32 / 262_144.0).collect();
    let mut map = MediaMap::new();
    map.insert(
        MEDIA,
        Arc::new(SampleData::new(48_000, vec![left]).unwrap()),
    );
    map
}

fn render(document: &Document) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media()));
    let mut offline = Offline::new(EngineConfig::new(SampleRate::HZ_48000, 256), registry);
    offline.sync(document);
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(180_000, |output| out.extend_from_slice(output.channel(0)));
    out
}

// Spec: audio-clips R16
#[test]
fn a_version_1_set_with_split_audio_clips_loads_and_plays_as_it_did() {
    let mut editor = Editor::new();
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (Ticks::WHOLE * 2, Bpm::new(93.0).unwrap()),
    ])
    .unwrap();
    editor.apply(Command::SetTempo { tempo }).unwrap();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: "Audio".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let audio = AudioData {
        start: 0.125,
        ..AudioData::new(MEDIA)
    };
    let start = Ticks::QUARTER + Ticks(4_321);
    let split = start + Ticks::WHOLE + Ticks(12_345);
    let end = start + Ticks::WHOLE * 3;
    let whole = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(whole, start, end - start, ClipData::Audio(audio)),
        })
        .unwrap();
    let expected = render(editor.document());
    assert!(expected.iter().any(|&s| s != 0.0));

    // As version 1 saved the clip split in two: the right part kept the
    // whole clip's Start marker and an offset.
    editor
        .apply(Command::SetClipLength {
            clip: whole,
            length: split - start,
        })
        .unwrap();
    let right = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(right, split, end - split, ClipData::Audio(audio)),
        })
        .unwrap();
    let project = Project {
        document: editor.document().clone(),
        ids: IdAllocator::new(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        folds: bl_model::ClipFolds::new(),
    };
    let mut saved: Value = serde_json::from_slice(&encode_json(&project, 1).unwrap()).unwrap();
    saved["schema"] = json!(1);
    let clips = saved["document"]["tracks"][0]["clips"]
        .as_array_mut()
        .unwrap();
    let part = clips
        .iter_mut()
        .find(|c| c["id"] == json!(right.0.get()))
        .unwrap();
    part["offset"] = json!((split - start).get());

    let (loaded, info) = decode_json(
        &serde_json::to_vec(&saved).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(info.schema, 1);
    let part = loaded.document.clip(right).unwrap();
    assert_eq!(part.offset, Ticks::ZERO);
    let ClipData::Audio(moved) = part.data else {
        unreachable!("an audio clip");
    };
    let tempo = &loaded.document.settings().tempo;
    assert_eq!(
        moved.start,
        0.125 + (tempo.ticks_to_seconds(split) - tempo.ticks_to_seconds(start))
    );
    assert_eq!(render(&loaded.document), expected);
}
