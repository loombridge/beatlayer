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
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, Device, DeviceKind, Document, Editor, ParamId,
    TrackKind,
};
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
            clip: Clip::new(whole, start, end - start, ClipData::Audio(audio.clone())),
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
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
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
    let ClipData::Audio(moved) = &part.data else {
        unreachable!("an audio clip");
    };
    let tempo = &loaded.document.settings().tempo;
    assert_eq!(
        moved.start,
        0.125 + (tempo.ticks_to_seconds(split) - tempo.ticks_to_seconds(start))
    );
    assert_eq!(render(&loaded.document), expected);
}

// Spec: audio-clips R30. A set saved before fade curves and crossfades has
// only each clip's two fade lengths: it opens with the same fades, straight,
// and no crossfades, and plays as it did.
#[test]
fn a_set_saved_before_crossfades_opens_with_the_same_fades_and_none_between_clips() {
    let mut editor = Editor::new();
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
    // Two clips that meet, each with the fades Create Fades on Clip Edges
    // gave it, and a third with long ones.
    let faded = |start: f64, fade_in: f64, fade_out: f64| AudioData {
        start,
        fade_in,
        fade_out,
        ..AudioData::new(MEDIA)
    };
    let mut ids = Vec::new();
    for (at, audio) in [
        (Ticks::ZERO, faded(0.0, 0.004, 0.004)),
        (Ticks::WHOLE, faded(0.5, 0.004, 0.004)),
        (Ticks::WHOLE * 3, faded(1.0, 0.25, 0.5)),
    ] {
        let id = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(id, at, Ticks::WHOLE, ClipData::Audio(audio)),
            })
            .unwrap();
        ids.push(id);
    }
    let expected = render(editor.document());
    assert!(expected.iter().any(|&s| s != 0.0));
    let project = Project {
        document: editor.document().clone(),
        ids: IdAllocator::new(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    };
    let saved: Value = serde_json::from_slice(&encode_json(&project, 1).unwrap()).unwrap();
    // The set holds nothing of curves or crossfades, as before.
    for clip in saved["document"]["tracks"][0]["clips"].as_array().unwrap() {
        let audio = clip["data"]["audio"].as_object().unwrap();
        for key in [
            "fade_in_curve",
            "fade_out_curve",
            "fade_in_before",
            "fade_out_after",
        ] {
            assert!(!audio.contains_key(key), "{key}");
        }
    }
    let (loaded, _) = decode_json(
        &serde_json::to_vec(&saved).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    for (id, (fade_in, fade_out)) in ids
        .iter()
        .zip([(0.004, 0.004), (0.004, 0.004), (0.25, 0.5)])
    {
        let ClipData::Audio(audio) = &loaded.document.clip(*id).unwrap().data else {
            unreachable!("an audio clip");
        };
        assert_eq!((audio.fade_in, audio.fade_out), (fade_in, fade_out));
        assert_eq!(
            (audio.fade_in_curve, audio.fade_out_curve),
            (0.0, 0.0),
            "straight"
        );
    }
    let lane = loaded.document.track(track).unwrap();
    assert!(bl_model::crossfades(lane.clips()).is_empty());
    assert_eq!(render(&loaded.document), expected);
}

// Spec: racks R13
#[test]
fn sets_from_before_racks_load_unchanged() {
    use bl_model::{Device, DeviceKind};
    let mut editor = Editor::new();
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
    let mut device = Device::new(
        editor.new_id(),
        "Utility",
        DeviceKind::Native {
            device_type: "beatlayer.utility".to_owned(),
        },
    );
    device.params.insert(bl_model::ParamId(2), 0.5);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let project = Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    };
    // A device that isn't a rack saves no chains, so a set saved before
    // racks existed is what is saved now.
    let saved = encode_json(&project, 1).unwrap();
    let value: Value = serde_json::from_slice(&saved).unwrap();
    let written = &value["document"]["tracks"][0]["devices"][0];
    assert!(written.get("chains").is_none(), "{written}");
    let (loaded, _) = decode_json(&saved, &LoadOptions::default()).unwrap();
    assert_eq!(loaded.document, project.document);
    loaded.document.check_invariants().unwrap();
    assert_eq!(encode_json(&loaded, 1).unwrap(), saved);
    assert_eq!(
        loaded.document.device_count(),
        1,
        "the lookups find the one device"
    );
}

// Spec: scales R8
#[test]
fn a_set_saved_before_the_scales_loads_unchanged() {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Keys".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let clip = midi_clip(&mut editor, track);
    let project = Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    };
    let saved = encode_json(&project, 1).unwrap();
    let value: Value = serde_json::from_slice(&saved).unwrap();
    let written = &value["document"]["tracks"][0]["clips"][0];
    assert!(written.get("scale").is_none(), "{written}");
    assert!(
        value["document"]["settings"].get("scale").is_none(),
        "the set's scale isn't written either"
    );
    let (loaded, _) = decode_json(&saved, &LoadOptions::default()).unwrap();
    assert_eq!(loaded.document, project.document);
    assert_eq!(
        loaded.document.clip(clip).unwrap().scale,
        bl_model::Scale::default(),
        "every clip is off, at C, in Major"
    );
    assert_eq!(loaded.document.settings().scale, bl_model::Scale::default());
    assert_eq!(encode_json(&loaded, 1).unwrap(), saved);
}

// Spec: note-editing R20. The lanes, the footer and the value keys save
// nothing new: a set saved before them loads unchanged, and every note value
// a lane edits is saved in the shape it always was and read back.
#[test]
fn a_set_saved_before_the_lanes_loads_unchanged_and_every_note_value_round_trips() {
    use bl_model::{MidiData, Note};
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Keys".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let plain = Note::new(60, Ticks::ZERO, Ticks::QUARTER, 100);
    let mut edited = Note::new(64, Ticks::QUARTER, Ticks::QUARTER, 33);
    edited.release_velocity = 99;
    edited.probability = 0.25;
    edited.velocity_deviation = -20;
    edited.mute = true;
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new([plain.clone(), edited.clone()])),
            ),
        })
        .unwrap();
    let project = Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    };
    let saved = encode_json(&project, 1).unwrap();
    let value: Value = serde_json::from_slice(&saved).unwrap();
    let notes = &value["document"]["tracks"][0]["clips"][0]["data"]["midi"]["notes"];
    for field in [
        "release_velocity",
        "probability",
        "velocity_deviation",
        "mute",
    ] {
        assert!(
            notes[0].get(field).is_none(),
            "a plain note writes no {field}"
        );
        assert!(
            notes[1].get(field).is_some(),
            "an edited note writes {field}"
        );
    }
    let (loaded, _) = decode_json(&saved, &LoadOptions::default()).unwrap();
    assert_eq!(loaded.document, project.document);
    let ClipData::Midi(midi) = &loaded.document.clip(clip).unwrap().data else {
        panic!("a MIDI clip");
    };
    let notes: Vec<Note> = midi.notes.iter().cloned().collect();
    assert_eq!(notes, [plain, edited], "every value comes back");
    assert_eq!(encode_json(&loaded, 1).unwrap(), saved);
}

// Spec: note-editing R30. A group is saved with its clip and loads again; a
// set saved before groups loads with none; a group that isn't whole loads
// its notes ungrouped.
#[test]
fn probability_groups_are_saved_with_the_clip_and_broken_ones_load_ungrouped() {
    use bl_model::{GroupKind, MidiData, Note, NoteGroup};
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Keys".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut notes: Vec<Note> = (0..3)
        .map(|place| {
            Note::new(
                60 + place,
                Ticks::QUARTER * i64::from(place),
                Ticks::QUARTER,
                100,
            )
        })
        .collect();
    for note in &mut notes[..2] {
        note.probability = 0.5;
        note.group = Some(7);
    }
    let mut data = MidiData::new(notes);
    data.groups = vec![NoteGroup {
        id: 7,
        kind: GroupKind::PlayOne,
    }];
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(data.clone()),
            ),
        })
        .unwrap();
    let lane: bl_model::TakeLaneId = editor.new_id();
    let project = Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    };
    let saved = encode_json(&project, 1).unwrap();
    let value: Value = serde_json::from_slice(&saved).unwrap();
    let midi = &value["document"]["tracks"][0]["clips"][0]["data"]["midi"];
    assert_eq!(midi["groups"], json!([{ "id": 7, "kind": "play_one" }]));
    assert_eq!(midi["notes"][0]["group"], json!(7));
    assert!(midi["notes"][2].get("group").is_none(), "an ungrouped note");
    let midi_of = |value: &Value| -> MidiData {
        let (loaded, _) =
            decode_json(&serde_json::to_vec(value).unwrap(), &LoadOptions::default()).unwrap();
        match &loaded.document.clip(clip).unwrap().data {
            ClipData::Midi(midi) => midi.clone(),
            ClipData::Audio(_) => panic!("a MIDI clip"),
        }
    };
    assert_eq!(midi_of(&value), data, "the group comes back");

    // A set saved before groups has neither field.
    let mut older = value.clone();
    let path = "/document/tracks/0/clips/0/data/midi";
    older
        .pointer_mut(path)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("groups");
    for index in 0..3 {
        older
            .pointer_mut(&format!("{path}/notes/{index}"))
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("group");
    }
    let loaded = midi_of(&older);
    assert!(loaded.groups.is_empty() && loaded.notes.iter().all(|n| n.group.is_none()));

    // Groups that aren't whole: chances that differ, a note naming a group
    // the clip doesn't list, and a group of one note.
    let mut differ = value.clone();
    *differ
        .pointer_mut(&format!("{path}/notes/1/probability"))
        .unwrap() = json!(0.25);
    let mut missing = value.clone();
    missing
        .pointer_mut(path)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("groups");
    let mut single = value.clone();
    single
        .pointer_mut(&format!("{path}/notes/1"))
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("group");
    for (name, broken) in [
        ("differ", differ.clone()),
        ("missing", missing),
        ("single", single),
    ] {
        let loaded = midi_of(&broken);
        assert!(loaded.groups.is_empty(), "{name}");
        assert!(loaded.notes.iter().all(|n| n.group.is_none()), "{name}");
    }

    // A take lane's clip with a broken group loads it ungrouped too.
    let mut taken = differ;
    let track_value = taken.pointer_mut("/document/tracks/0").unwrap();
    let moved = track_value["clips"].as_array_mut().unwrap().remove(0);
    track_value["take_lanes"] = json!([{ "id": lane, "name": "Take 1", "clips": [moved] }]);
    let (loaded, _) = decode_json(
        &serde_json::to_vec(&taken).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    let lanes: Vec<_> = loaded
        .document
        .tracks()
        .next()
        .unwrap()
        .take_lanes()
        .collect();
    let ClipData::Midi(midi) = &lanes[0].clips[0].data else {
        panic!("a MIDI clip");
    };
    assert!(midi.groups.is_empty() && midi.notes.iter().all(|n| n.group.is_none()));
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
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
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

// Spec: device-compare R10
#[test]
fn a_set_without_b_loads_with_b_equal_to_a() {
    let mut editor = Editor::new();
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
    let device = editor.new_id();
    let mut utility = Device::new(
        device,
        "Utility",
        DeviceKind::Native {
            device_type: "beatlayer.utility".to_owned(),
        },
    );
    utility.params.insert(ParamId(2), -3.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: utility,
        })
        .unwrap();
    let project = Project {
        document: editor.document().clone(),
        ids: IdAllocator::new(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    };
    // A set from before the two states has neither field.
    let saved: Value = serde_json::from_slice(&encode_json(&project, 1).unwrap()).unwrap();
    let saved_device = &saved["document"]["tracks"][0]["devices"][0];
    assert!(saved_device.get("b_active").is_none());
    assert!(saved_device.get("other_params").is_none());

    let (loaded, _) = decode_json(
        &serde_json::to_vec(&saved).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    let loaded = loaded.document.device(device).unwrap();
    assert!(!loaded.b_active);
    assert_eq!(loaded.other_params(), &loaded.params);
}

// Spec: warping R1, R11. A set saved before warping has no warp field on any
// clip; it opens with every clip unwarped, its Start, End and loop still
// seconds in the file, and renders the same to the bit.
#[test]
fn a_set_saved_before_warping_opens_unwarped_and_plays_as_it_did() {
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
    // One clip plays whole, one is looped (which an unwarped clip could be
    // before warping, and keeps).
    for (start, audio) in [
        (Ticks::ZERO, AudioData::new(MEDIA)),
        (
            Ticks::WHOLE * 4,
            AudioData {
                start: 0.125,
                end: Some(0.75),
                looping: true,
                loop_start: 0.25,
                loop_end: Some(0.5),
                ..AudioData::new(MEDIA)
            },
        ),
    ] {
        let clip = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(clip, start, Ticks::WHOLE * 3, ClipData::Audio(audio)),
            })
            .unwrap();
    }
    let expected = render(editor.document());
    assert!(expected.iter().any(|&s| s != 0.0));

    let project = Project::from_editor(&editor, Media::default());
    let saved = encode_json(&project, 1).unwrap();
    let text = String::from_utf8(saved.clone()).unwrap();
    for field in ["\"warp", "\"tempo\":", "\"signature\""] {
        let clips = &text[text.find("\"clips\"").unwrap()..];
        assert!(!clips.contains(field), "an unwarped clip saves {field}");
    }
    let (loaded, _) = decode_json(&saved, &LoadOptions::default()).unwrap();
    let mut clips = 0;
    for clip in loaded.document.track(track).unwrap().clips() {
        let ClipData::Audio(audio) = &clip.data else {
            unreachable!("an audio clip");
        };
        assert!(!audio.warp && audio.warp_markers.is_empty() && audio.tempo.is_none());
        clips += 1;
    }
    assert_eq!(clips, 2);
    assert_eq!(render(&loaded.document), expected);
}

// Spec: warping R14. A looped unwarped clip from a set saved before warping
// keeps its loop and plays as it did; its loop can be switched off, and
// comes back on only with Warp.
#[test]
fn an_older_looped_clip_keeps_its_loop() {
    let mut editor = Editor::new();
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
    let clip = editor.new_id();
    let looped = AudioData {
        start: 0.125,
        looping: true,
        loop_start: 0.25,
        loop_end: Some(0.5),
        ..AudioData::new(MEDIA)
    };
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE * 3, ClipData::Audio(looped)),
        })
        .unwrap();
    let expected = render(editor.document());
    assert!(expected.iter().any(|&s| s != 0.0));

    let project = Project::from_editor(&editor, Media::default());
    let (loaded, _) =
        decode_json(&encode_json(&project, 1).unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(render(&loaded.document), expected);
    let mut editor = loaded.editor();
    let enabled = |enabled| Command::SetClipLoopEnabled { clip, enabled };
    editor.apply(enabled(false)).unwrap();
    assert!(editor.apply(enabled(true)).is_err());
    editor
        .apply(Command::SetClipWarp { clip, warp: true })
        .unwrap();
    editor.apply(enabled(true)).unwrap();
}

// Spec: automation-editing R8. Lock Envelopes is saved with the set, and a
// set saved before it has none and loads with it off.
#[test]
fn an_older_set_loads_with_lock_envelopes_off() {
    let mut editor = Editor::new();
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
    let project = Project::from_editor(&editor, Media::default());
    let saved: Value = serde_json::from_slice(&encode_json(&project, 1).unwrap()).unwrap();
    assert!(
        saved["document"]["settings"]
            .get("lock_envelopes")
            .is_none(),
        "a set saved before Lock Envelopes has no such field"
    );

    let (loaded, _) = decode_json(
        &serde_json::to_vec(&saved).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert!(!loaded.document.settings().lock_envelopes);

    // A set saved with it on keeps it.
    editor
        .apply(Command::SetLockEnvelopes { enabled: true })
        .unwrap();
    let project = Project::from_editor(&editor, Media::default());
    let (loaded, _) =
        decode_json(&encode_json(&project, 1).unwrap(), &LoadOptions::default()).unwrap();
    assert!(loaded.document.settings().lock_envelopes);
}

/// A set saved before locators existed loads with none (the time editing
/// spec, R1).
#[test]
fn a_set_saved_before_locators_loads_with_none() {
    let mut editor = Editor::new();
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
    let project = Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    };
    let saved = encode_json(&project, 1).unwrap();
    let value: Value = serde_json::from_slice(&saved).unwrap();
    assert!(
        value["document"]["settings"].get("locators").is_none(),
        "a set saved before locators writes none"
    );
    assert!(value["document"]["settings"].get("song_start").is_none());
    let (loaded, _) = decode_json(&saved, &LoadOptions::default()).unwrap();
    assert!(loaded.document.locators().is_empty());
    assert!(loaded.document.song_start_ticks().is_none());
    loaded.document.check_invariants().unwrap();
}

/// The locators and the song start time are saved with the set and load
/// again (the time editing spec, R1 and R10).
#[test]
fn the_locators_and_the_song_start_time_are_saved_and_load_again() {
    let mut editor = Editor::new();
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
    let locator = editor.new_id();
    editor
        .apply(Command::AddLocator {
            locator: bl_model::Locator::new(locator, Ticks::WHOLE * 2, "Intro".to_owned()),
        })
        .unwrap();
    editor
        .apply(Command::SetSongStart {
            start: Some(bl_model::SongStart::Locator(locator)),
        })
        .unwrap();
    let project = Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    };
    let saved = encode_json(&project, 1).unwrap();
    let (loaded, _) = decode_json(&saved, &LoadOptions::default()).unwrap();
    assert_eq!(loaded.document.locators().len(), 1);
    assert_eq!(loaded.document.locators()[0].name, "Intro");
    assert_eq!(loaded.document.song_start_ticks(), Some(Ticks::WHOLE * 2));
}

/// A project holding `editor`'s document.
fn project_of(editor: &Editor) -> Project {
    Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        media: Media::default(),
        windows: bl_model::DeviceWindows::new(),
        rack_views: bl_model::RackViewMap::new(),
        folds: bl_model::ClipFolds::new(),
        launch: bl_model::LaunchSettings::default(),
        multisamplers: bl_model::MultisamplerViews::new(),
    }
}

/// A MIDI track holding a clip, and the clip's ID.
fn midi_clip_song() -> (Editor, bl_model::ClipId) {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Keys".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: bl_model::Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                bl_model::ClipData::Midi(bl_model::MidiData::default()),
            ),
        })
        .unwrap();
    (editor, clip)
}

/// A set saved before grooves loads with an empty pool, a Global Amount of
/// 100 % and Auto Load Groove off, and a clip with no groove (the grooves
/// spec, R22).
#[test]
fn a_set_saved_before_grooves_loads_with_an_empty_pool() {
    let (editor, clip) = midi_clip_song();
    let saved = encode_json(&project_of(&editor), 1).unwrap();
    let value: Value = serde_json::from_slice(&saved).unwrap();
    assert!(value["document"]["settings"].get("grooves").is_none());
    assert!(
        value["document"]["tracks"][0]["clips"][0]
            .get("groove")
            .is_none()
    );
    let (loaded, _) = decode_json(&saved, &LoadOptions::default()).unwrap();
    let pool = &loaded.document.settings().grooves;
    assert!(pool.pool.is_empty());
    assert_eq!(pool.global_amount, 100.0);
    assert!(!pool.auto_load && pool.auto_load_groove.is_none());
    assert_eq!(loaded.document.clip(clip).unwrap().groove, None);
    loaded.document.check_invariants().unwrap();
}

/// The pool, each groove's notes and parameters, the Global Amount, Auto Load
/// Groove and its groove, and each clip's groove are saved with the set and
/// load again; a clip naming a groove that isn't in the pool loads with none
/// (the grooves spec, R22).
#[test]
fn the_grooves_are_saved_with_the_set_and_load_again() {
    let (mut editor, clip) = midi_clip_song();
    let groove: bl_model::GrooveId = editor.new_id();
    editor
        .apply(Command::AddGroove {
            groove: bl_model::Groove::new(
                groove,
                "Swing".to_owned(),
                Ticks::WHOLE,
                vec![bl_model::GrooveNote {
                    position: Ticks::SIXTEENTH,
                    velocity: 90,
                }],
            ),
        })
        .unwrap();
    editor
        .apply(Command::SetGroove {
            groove,
            base: Some(bl_model::GrooveBase::EighthTriplet),
            quantize: Some(25.0),
            timing: None,
            random: Some(5.0),
            velocity: Some(-50.0),
        })
        .unwrap();
    editor
        .apply(Command::SetClipGroove {
            clips: vec![clip],
            groove: Some(groove),
        })
        .unwrap();
    editor
        .apply(Command::SetGlobalGrooveAmount { amount: 85.0 })
        .unwrap();
    editor
        .apply(Command::SetAutoLoadGroove {
            enabled: Some(true),
            groove: Some(groove),
        })
        .unwrap();
    let saved = encode_json(&project_of(&editor), 1).unwrap();
    let (loaded, _) = decode_json(&saved, &LoadOptions::default()).unwrap();
    assert_eq!(
        loaded.document.settings().grooves,
        editor.document().settings().grooves
    );
    assert_eq!(loaded.document.clip(clip).unwrap().groove, Some(groove));
    // A clip that names a groove the pool doesn't hold loads with none.
    let mut value: Value = serde_json::from_slice(&saved).unwrap();
    value["document"]["settings"]["grooves"]["pool"] = serde_json::json!([]);
    let (loaded, _) = decode_json(
        &serde_json::to_vec(&value).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(loaded.document.clip(clip).unwrap().groove, None);
    assert_eq!(loaded.document.settings().grooves.auto_load_groove, None);
}

// Spec: comping R3. A set saved with WP-2.13's take lanes, whose passes had no
// IDs, opens with each pass a clip of its lane under a new ID, and plays as
// it did: a take lane plays only while it's auditioned.
#[test]
fn a_version_7_set_with_take_lanes_loads_and_plays_as_it_did() {
    let mut editor = Editor::new();
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
    let audio = AudioData::new(MEDIA);
    let main = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE * 2,
        ClipData::Audio(audio.clone()),
    );
    editor
        .apply(Command::CreateClip { track, clip: main })
        .unwrap();
    let expected = render(editor.document());
    assert!(expected.iter().any(|&s| s != 0.0));

    // As version 7 saved a track with two take lanes of passes.
    let lane = |editor: &mut Editor, name: &str, start: f64| bl_model::NewTakeLane {
        id: Some(editor.new_id()),
        name: name.to_owned(),
        clips: vec![bl_model::TakeClip {
            id: Some(editor.new_id()),
            name: "pass".to_owned(),
            start: Ticks::QUARTER,
            length: Ticks::QUARTER * 4,
            data: bl_model::TakeClipData::Audio(AudioData {
                start,
                ..audio.clone()
            }),
        }],
    };
    for (name, start) in [("Take 1", 0.5), ("Take 2", 1.0)] {
        let lane = lane(&mut editor, name, start);
        editor
            .apply(Command::AddTakeLane {
                track,
                lane,
                index: None,
            })
            .unwrap();
    }
    let project = project_of(&editor);
    let mut saved: Value = serde_json::from_slice(&encode_json(&project, 1).unwrap()).unwrap();
    saved["schema"] = json!(7);
    for lane in saved["document"]["tracks"][0]["take_lanes"]
        .as_array_mut()
        .unwrap()
    {
        let lane = lane.as_object_mut().unwrap();
        lane.remove("id");
        for pass in lane["clips"].as_array_mut().unwrap() {
            let pass = pass.as_object_mut().unwrap();
            pass.remove("id");
            pass.remove("mute");
            // The pass's data was the audio's own fields.
            let data = pass.remove("data").unwrap();
            pass.insert("data".to_owned(), data["audio"].clone());
        }
    }

    let (loaded, info) = decode_json(
        &serde_json::to_vec(&saved).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(info.schema, 7);
    loaded.document.check_invariants().unwrap();
    let before = project.document.track(track).unwrap();
    let after = loaded.document.track(track).unwrap();
    assert_eq!(after.take_lanes().len(), 2);
    for (was, now) in before.take_lanes().zip(after.take_lanes()) {
        assert_eq!(was.name, now.name);
        assert_eq!(was.clips.len(), now.clips.len());
        assert_eq!(was.clips[0].data, now.clips[0].data);
        assert_eq!(
            (was.clips[0].start, was.clips[0].length),
            (now.clips[0].start, now.clips[0].length)
        );
    }
    assert_eq!(render(&loaded.document), expected, "plays as it did");
    // Auditioned, a lane plays in its place.
    let mut audition = Editor::from_document(loaded.document.clone(), loaded.ids);
    let lane = after.take_lanes().next().unwrap().id;
    audition
        .apply(Command::SetAudition { lane, on: true })
        .unwrap();
    assert_ne!(render(audition.document()), expected);
}
