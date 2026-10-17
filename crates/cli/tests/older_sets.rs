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
        launch: bl_model::LaunchSettings::default(),
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

/// A one-bar MIDI clip, on the track from the start of the song.
fn midi_clip(editor: &mut Editor, track: bl_model::TrackId) -> bl_model::ClipId {
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(bl_model::MidiData::default()),
            ),
        })
        .unwrap();
    clip
}

/// Two MIDI tracks of a steady signal: the first's fader at 0.5, the second
/// through a Utility. With `envelopes`, a clip envelope on each, flat, as a
/// set from before relative modulation held them: values in the parameters'
/// own units, added to the control. Without, the controls are set to what
/// those sums gave, which is what the set played.
fn dc_song(envelopes: bool) -> Document {
    use bl_devices::utility::{GAIN, UTILITY};
    use bl_engine::testing::{DC, VALUE};
    use bl_model::{AutomationTarget, Device, DeviceKind, MixerParam};

    let mut editor = Editor::new();
    let add = |editor: &mut Editor, device_type: &str, params: &[(bl_model::ParamId, f64)]| {
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        for (index, (device_type, params)) in
            [(DC, vec![(VALUE, 0.5)]), (device_type, params.to_vec())]
                .into_iter()
                .enumerate()
        {
            let id = editor.new_id();
            let mut device = Device::new(
                id,
                "",
                DeviceKind::Native {
                    device_type: device_type.to_owned(),
                },
            );
            device.params.extend(params);
            editor
                .apply(Command::InsertDevice {
                    track,
                    index,
                    device,
                })
                .unwrap();
        }
        track
    };
    let a = add(&mut editor, UTILITY, &[]);
    let b = add(
        &mut editor,
        UTILITY,
        &[(GAIN, if envelopes { 1.0 } else { 2.0 })],
    );
    let volume = if envelopes { 0.5 } else { 0.25 };
    editor
        .apply(Command::SetVolume { track: a, volume })
        .unwrap();
    if envelopes {
        let gain_device = editor
            .document()
            .track(b)
            .unwrap()
            .devices()
            .nth(1)
            .unwrap()
            .id;
        for (track, target, old) in [
            (
                a,
                AutomationTarget::Mixer {
                    param: MixerParam::Volume,
                },
                -0.25,
            ),
            (
                b,
                AutomationTarget::Device {
                    device: gain_device,
                    param: GAIN,
                },
                1.0,
            ),
        ] {
            let clip = midi_clip(&mut editor, track);
            editor
                .apply(Command::SetClipEnvelopeBreakpoint {
                    kind: bl_model::EnvelopeKind::Modulation,
                    clip,
                    target,
                    at: Ticks::ZERO,
                    value: old,
                })
                .unwrap();
        }
    }
    editor.document().clone()
}

/// Renders `document` with the test devices and the shipped ones.
fn render_devices(document: &Document) -> Vec<f32> {
    let probe = std::sync::Arc::new(bl_engine::testing::Probe::default());
    let mut registry = bl_engine::testing::registry(&probe);
    bl_devices::register(&mut registry);
    let mut offline = Offline::new(EngineConfig::new(SampleRate::HZ_48000, 256), registry);
    offline.sync(document);
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(24_000, |output| out.extend_from_slice(output.channel(0)));
    out
}

// Spec: modulation R5
#[test]
fn an_older_clip_envelope_renders_the_same() {
    // The set as version 4 saved it: values in the parameters' own units.
    let project = Project {
        document: dc_song(true),
        ids: IdAllocator::new(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
    };
    let mut saved: Value = serde_json::from_slice(&encode_json(&project, 1).unwrap()).unwrap();
    saved["schema"] = json!(4);
    let (mut loaded, info) = decode_json(
        &serde_json::to_vec(&saved).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert!(info.before_relative_modulation());
    bl_devices::relative_clip_envelopes(&mut loaded.document);
    let (now, played) = (
        render_devices(&loaded.document),
        render_devices(&dc_song(false)),
    );
    assert!(played[20_000] > 0.5, "the signal plays: {}", played[20_000]);
    // 0.5 · 0.25 through the first track and 0.5 · 2.0 through the second.
    assert!((played[20_000] - 1.125).abs() < 1e-4, "{}", played[20_000]);
    for (frame, (a, b)) in now.iter().zip(&played).enumerate().skip(10_000) {
        assert!((a - b).abs() < 1e-3, "frame {frame}: {a} isn't {b}");
    }
}
