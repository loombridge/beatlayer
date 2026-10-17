//! The query tool returns what the set saves (agent readback): a set that
//! holds a non-default value of every saved field of a track, clip, device,
//! scene and the song is opened, and the query's `detail: "full"` results
//! are held against the set format's own encoding of it.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_core::time::{Bpm, Ticks, TimeSignature};
use bl_model::{
    AudioData, AutomationTarget, Channel, Clip, ClipData, Command, ContentHash, CountIn,
    CrossfadeAssign, Device, DeviceKind, DeviceRole, Editor, Input, LaunchMode, LaunchQuantization,
    LaunchSettings, MetronomeRhythm, MetronomeSound, MidiData, MixerParam, Monitor, Note, Output,
    ParamId, SendTap, Sidechain, TakeClip, TakeLane, Tap, TrackId, TrackKind,
};
use bl_project::{Media, Project};
use bl_shell::Shell;
use serde_json::{Value, json};

const BAR: i64 = 2_822_400;

fn note(key: u8, start: i64) -> Note {
    Note {
        key,
        velocity: 90,
        release_velocity: 40,
        start: Ticks(start),
        length: Ticks(176_400),
        probability: 0.5,
        velocity_deviation: 7,
        mute: false,
    }
}

/// A set with a non-default value of every field the format writes for a
/// track, clip, device, scene and the song.
fn fixture() -> Project {
    let mut editor = Editor::new();
    let apply = |editor: &mut Editor, command: Command| {
        let what = format!("{command:?}");
        editor
            .apply(command)
            .unwrap_or_else(|error| panic!("{what}: {error:?}"))
    };
    let track = |editor: &mut Editor, kind, name: &str, parent, index| -> TrackId {
        let id: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: name.to_owned(),
                parent,
                index,
            })
            .unwrap();
        id
    };
    let vox = track(&mut editor, TrackKind::Audio, "Vox", None, 0);
    let keys = track(&mut editor, TrackKind::Midi, "Keys", None, 1);
    let bus = track(&mut editor, TrackKind::Group, "Bus", None, 2);
    let drums = track(&mut editor, TrackKind::Audio, "Drums", Some(bus), 0);
    let verb = track(&mut editor, TrackKind::Return, "Verb", None, 0);
    let pad = track(&mut editor, TrackKind::Midi, "Pad", None, 3);

    // The song.
    apply(
        &mut editor,
        Command::SetTempoBreakpoint {
            at: Ticks(8 * BAR),
            bpm: Bpm::new(96.0).unwrap(),
        },
    );
    apply(
        &mut editor,
        Command::SetTimeSignatureChange {
            at: Ticks(4 * BAR),
            signature: TimeSignature::new(3, 4).unwrap(),
        },
    );
    apply(
        &mut editor,
        Command::SetLoop {
            start: Ticks(BAR),
            length: Ticks(2 * BAR),
        },
    );
    for command in [
        Command::SetLoopEnabled { enabled: true },
        Command::SetPunchIn { enabled: true },
        Command::SetPunchOut { enabled: true },
        Command::SetMidiOverdub { enabled: true },
        Command::SetMetronomeEnabled { enabled: true },
        Command::SetMetronomeCountIn {
            count_in: CountIn::Bars2,
        },
        Command::SetMetronomeSound {
            sound: MetronomeSound::Wood,
        },
        Command::SetMetronomeRhythm {
            rhythm: MetronomeRhythm::Bar1,
        },
        Command::SetMetronomeOnlyWhileRecording {
            only_while_recording: true,
        },
        Command::SetCrossfader { position: 0.25 },
        Command::SetSoloInPlace { enabled: false },
    ] {
        apply(&mut editor, command);
    }

    // The tracks: mixer, crossfader assignment, routing, sends, automation.
    for command in [
        Command::SetVolume {
            track: vox,
            volume: 0.5,
        },
        Command::SetPan {
            track: vox,
            pan: -0.25,
        },
        Command::SetMute {
            track: vox,
            mute: true,
        },
        Command::SetSolo {
            track: vox,
            solo: true,
        },
        Command::SetArm {
            track: vox,
            arm: true,
        },
        Command::SetCrossfadeAssign {
            track: vox,
            assign: CrossfadeAssign::A,
        },
        Command::SetInput {
            track: vox,
            input: Input::External {
                channel: Channel::Mono(3),
            },
            channel: None,
        },
        Command::SetMonitor {
            track: vox,
            monitor: Monitor::In,
        },
        Command::SetOutput {
            track: vox,
            output: Output::SendsOnly,
        },
        Command::SetSend {
            track: vox,
            to: verb,
            level: 0.5,
        },
        Command::SetSendTap {
            track: verb,
            tap: SendTap::Pre,
        },
        Command::SetInput {
            track: keys,
            input: Input::ComputerKeyboard,
            channel: None,
        },
        Command::SetMidiOutput {
            track: keys,
            to: Some(pad),
        },
        Command::SetMainOut {
            channel: Channel::Pair(3),
        },
        Command::SetCueOut {
            channel: Channel::Pair(5),
        },
        Command::SetOutput {
            track: drums,
            output: Output::Main,
        },
    ] {
        apply(&mut editor, command);
    }
    let volume = AutomationTarget::Mixer {
        param: MixerParam::Volume,
    };
    let pan = AutomationTarget::Mixer {
        param: MixerParam::Pan,
    };
    for (target, at, value) in [
        (volume, 0, 1.0),
        (volume, BAR, 0.5),
        (volume, 2 * BAR, 0.75),
    ] {
        apply(
            &mut editor,
            Command::SetAutomationBreakpoint {
                track: vox,
                target,
                at: Ticks(at),
                value,
            },
        );
    }
    apply(
        &mut editor,
        Command::SetAutomationCurve {
            track: vox,
            target: volume,
            at: Ticks(BAR),
            bend: 0.5,
        },
    );
    apply(
        &mut editor,
        Command::SetAutomationBreakpoint {
            track: vox,
            target: pan,
            at: Ticks(0),
            value: 0.5,
        },
    );
    // A hand move turns the pan envelope off.
    apply(
        &mut editor,
        Command::SetPan {
            track: vox,
            pan: 0.1,
        },
    );

    // Two take lanes, the first with two passes.
    let pass = |name: &str, start: i64, from: f64| TakeClip {
        name: name.to_owned(),
        start: Ticks(start),
        length: Ticks(BAR),
        data: AudioData {
            start: from,
            end: Some(from + 2.0),
            gain: 0.7,
            ..AudioData::new(ContentHash([3; 32]))
        },
    };
    for lane in [
        TakeLane {
            name: "Take 1".to_owned(),
            clips: vec![pass("a", 0, 0.0), pass("b", BAR, 1.0)],
        },
        TakeLane {
            name: "Take 2".to_owned(),
            clips: vec![pass("c", 0, 0.5)],
        },
    ] {
        apply(&mut editor, Command::AddTakeLane { track: vox, lane });
    }

    // A device with a sidechain, parameters, a window and its own name.
    let compressor: bl_model::DeviceId = editor.new_id();
    let mut device = Device::new(
        compressor,
        "Squash",
        DeviceKind::Native {
            device_type: "beatlayer.utility".to_owned(),
        },
    );
    device.own_name = "Utility".to_owned();
    device.enabled = false;
    device.show_preset_name = false;
    device.floating = true;
    device.set_media(0, Some(ContentHash([9; 32])));
    device.role = DeviceRole::AudioEffect;
    device.params.insert(ParamId(2), 0.75);
    device.sidechain = Some(Sidechain {
        enabled: true,
        input: Input::Track {
            track: drums,
            tap: Tap::PreFx,
        },
        gain: 0.5,
        mix: 0.25,
        listen: true,
    });
    apply(
        &mut editor,
        Command::InsertDevice {
            track: vox,
            index: 0,
            device,
        },
    );

    // A rack of two chains: the first with a device and a rack inside it,
    // every chain setting off its default.
    let rack: bl_model::DeviceId = editor.new_id();
    let (loud, quiet): (bl_model::ChainId, bl_model::ChainId) = (editor.new_id(), editor.new_id());
    let (inside, deeper): (bl_model::DeviceId, bl_model::ChainId) =
        (editor.new_id(), editor.new_id());
    let trim: bl_model::DeviceId = editor.new_id();
    apply(
        &mut editor,
        Command::InsertDevice {
            track: vox,
            index: 1,
            device: Device::rack(rack, bl_model::RackKind::AudioEffect)
                .with_chain(bl_model::Chain::new(loud, "Loud").with_devices(vec![
                    Device::new(
                        trim,
                        "Trim",
                        DeviceKind::Native {
                            device_type: "beatlayer.utility".to_owned(),
                        },
                    ),
                    Device::rack(inside, bl_model::RackKind::AudioEffect)
                        .with_chain(bl_model::Chain::new(deeper, "Deeper")),
                ]))
                .with_chain(bl_model::Chain::new(quiet, "Quiet")),
        },
    );
    for command in [
        Command::SetChainColour {
            chain: loud,
            colour: Some(12),
        },
        Command::SetChainInfo {
            chain: loud,
            info: "the loud one".to_owned(),
        },
        Command::SetChainVolume {
            chain: loud,
            volume: 0.5,
        },
        Command::SetChainPan {
            chain: loud,
            pan: -0.25,
        },
        Command::SetChainEnabled {
            chain: quiet,
            enabled: false,
        },
        Command::SetChainSolo {
            chain: loud,
            solo: true,
        },
    ] {
        apply(&mut editor, command);
    }

    // Clips: an audio clip with everything a clip saves, a MIDI clip with a
    // loop and notes, and slot clips.
    let mut audio = Clip::new(
        editor.new_id(),
        Ticks(BAR),
        Ticks(2 * BAR),
        ClipData::Audio(AudioData {
            media: ContentHash([7; 32]),
            start: 0.5,
            end: Some(3.0),
            looping: true,
            loop_start: 1.0,
            loop_end: Some(2.5),
            gain: 0.5,
            fade_in: 0.25,
            fade_out: 0.5,
        }),
    );
    audio.name = "Verse vox".to_owned();
    audio.mute = true;
    apply(
        &mut editor,
        Command::CreateClip {
            track: vox,
            clip: audio.clone(),
        },
    );
    apply(
        &mut editor,
        Command::SetClipEnvelopeBreakpoint {
            clip: audio.id,
            target: volume,
            at: Ticks(0),
            value: 0.5,
        },
    );
    let mut midi = Clip::new(
        editor.new_id(),
        Ticks(0),
        Ticks(2 * BAR),
        ClipData::Midi(MidiData {
            start: Ticks(176_400),
            end: Some(Ticks(BAR)),
            looping: true,
            loop_start: Ticks(176_400),
            loop_end: Some(Ticks(BAR)),
            signature: Some(TimeSignature::new(7, 8).unwrap()),
            ..MidiData::new([note(60, 0), note(64, 176_400), note(67, 352_800)])
        }),
    );
    midi.name = "Chords".to_owned();
    midi.offset = Ticks(88_200);
    apply(
        &mut editor,
        Command::CreateClip {
            track: keys,
            clip: midi,
        },
    );
    let scenes: Vec<_> = editor.document().scenes().map(|scene| scene.id).collect();
    let mut slot_clip = Clip::new(
        editor.new_id(),
        Ticks(0),
        Ticks(BAR),
        ClipData::Midi(MidiData::new([note(48, 0), note(50, 88_200)])),
    );
    slot_clip.name = "Loop".to_owned();
    slot_clip.launch.mode = LaunchMode::Gate;
    let slot_clip_id = slot_clip.id;
    apply(
        &mut editor,
        Command::CreateSlotClip {
            track: keys,
            scene: scenes[1],
            clip: slot_clip,
        },
    );
    apply(
        &mut editor,
        Command::SetClipLegato {
            clip: slot_clip_id,
            legato: true,
        },
    );
    apply(
        &mut editor,
        Command::SetClipLaunchQuantization {
            clip: slot_clip_id,
            quantization: Some(LaunchQuantization::Eighth),
        },
    );
    apply(
        &mut editor,
        Command::SetClipVelocityAmount {
            clip: slot_clip_id,
            amount: 0.5,
        },
    );
    apply(
        &mut editor,
        Command::SetSlotStopButton {
            track: keys,
            scene: scenes[2],
            stop_button: false,
        },
    );
    apply(
        &mut editor,
        Command::RenameScene {
            scene: scenes[0],
            name: "Verse".to_owned(),
        },
    );
    apply(
        &mut editor,
        Command::SetSceneTempo {
            scene: scenes[0],
            tempo: Some(Bpm::new(96.0).unwrap()),
        },
    );
    apply(
        &mut editor,
        Command::SetSceneTimeSignature {
            scene: scenes[0],
            signature: Some(TimeSignature::new(3, 4).unwrap()),
        },
    );
    editor.clear_history().unwrap();
    Project::from_editor(&editor, Media::new()).with_launch(LaunchSettings {
        quantization: LaunchQuantization::Bars2,
        enabled: false,
    })
}

fn open(project: &Project, name: &str) -> Client<InProcess> {
    let path = std::env::temp_dir().join(format!("{}-{name}.blset", std::process::id()));
    std::fs::write(&path, bl_project::encode(project, 1).unwrap()).unwrap();
    let mut shell = Shell::headless();
    shell
        .run("set.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();
    client
}

fn query(client: &mut Client<InProcess>, args: Value) -> Value {
    client
        .call("query", args.clone())
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query {args}: {error}"))
}

/// Whether `got` holds everything `expected`, the set format's encoding,
/// holds: every key, and every list's items, which a query returns a page at
/// a time.
fn covers(expected: &Value, got: &Value, at: &str) -> Result<(), String> {
    match (expected, got) {
        (Value::Object(expected), Value::Object(got)) => {
            expected.iter().try_for_each(|(key, value)| {
                let found = got
                    .get(key)
                    .ok_or_else(|| format!("{at}/{key} is missing"))?;
                covers(value, found, &format!("{at}/{key}"))
            })
        }
        (Value::Array(expected), Value::Object(page)) => {
            let total = page
                .get("total")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("{at} isn't a page"))?;
            if total != expected.len() as u64 {
                return Err(format!(
                    "{at} has {total} items, the set {}",
                    expected.len()
                ));
            }
            let offset = page.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let items = page
                .get("items")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            expected[offset.min(expected.len())..]
                .iter()
                .zip(items)
                .enumerate()
                .try_for_each(|(n, (value, found))| covers(value, found, &format!("{at}[{n}]")))
        }
        (Value::Array(expected), Value::Array(got)) if expected.len() == got.len() => expected
            .iter()
            .zip(got)
            .enumerate()
            .try_for_each(|(n, (value, found))| covers(value, found, &format!("{at}[{n}]"))),
        (expected, got) if expected == got => Ok(()),
        _ => Err(format!("{at} is {got}, the set has {expected}")),
    }
}

/// Every device of `devices` and every device inside their racks, with the
/// racks' chains.
fn nested(devices: &[Value], all: &mut Vec<Value>, chains: &mut Vec<Value>) {
    for device in devices {
        all.push(device.clone());
        for chain in device["chains"].as_array().into_iter().flatten() {
            chains.push(chain.clone());
            let inside: Vec<Value> = chain["devices"].as_array().cloned().unwrap_or_default();
            nested(&inside, all, chains);
        }
    }
}

/// Every chain in the document's racks, at any depth.
fn saved_chains(document: &Value) -> Vec<Value> {
    let (_, _, devices) = saved_parts(document);
    devices
        .iter()
        .flat_map(|d| d["chains"].as_array().into_iter().flatten().cloned())
        .collect()
}

/// Every track's saved form in the document, with its group's tracks left
/// out, and the clips and devices in it, those inside racks included.
fn saved_parts(document: &Value) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    fn walk(
        track: &Value,
        tracks: &mut Vec<Value>,
        clips: &mut Vec<Value>,
        devices: &mut Vec<Value>,
    ) {
        let mut own = track.clone();
        own.as_object_mut().unwrap().remove("children");
        clips.extend(track["clips"].as_array().into_iter().flatten().cloned());
        clips.extend(
            track["slots"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|slot| slot["clip"].clone()),
        );
        let top: Vec<Value> = track["devices"].as_array().cloned().unwrap_or_default();
        let mut chains = Vec::new();
        nested(&top, devices, &mut chains);
        tracks.push(own);
        for child in track["children"].as_array().into_iter().flatten() {
            walk(child, tracks, clips, devices);
        }
    }
    let (mut tracks, mut clips, mut devices) = (Vec::new(), Vec::new(), Vec::new());
    let top = document["tracks"].as_array().into_iter().flatten();
    let returns = document["returns"].as_array().into_iter().flatten();
    for track in top.chain(returns).chain([&document["main"]]) {
        walk(track, &mut tracks, &mut clips, &mut devices);
    }
    (tracks, clips, devices)
}

fn has(value: &Value, path: &str) -> bool {
    path.split('.')
        .try_fold(value, |value, key| match key.parse::<usize>() {
            Ok(n) => value.get(n),
            Err(_) => value.get(key),
        })
        .is_some()
}

#[test]
fn the_fixture_sets_a_non_default_value_of_every_saved_field() {
    let document = serde_json::to_value(&fixture().document).unwrap();
    let (tracks, clips, devices) = saved_parts(&document);
    let named = |name: &str| tracks.iter().find(|t| t["name"] == name).unwrap().clone();
    let (vox, keys, verb) = (named("Vox"), named("Keys"), named("Verb"));
    for (value, paths) in [
        (
            &document["settings"],
            &[
                "tempo",
                "time_signatures",
                "crossfader",
                "arrangement_loop",
                "solo_in_place",
                "metronome",
                "punch_in",
                "punch_out",
                "midi_overdub",
            ][..],
        ),
        (
            &vox,
            &[
                "mixer.crossfade",
                "sends.0",
                "routing.input",
                "routing.monitor",
                "routing.output",
                "automation.0.points.1.bend",
                "automation.1.enabled",
                "automation.0.target",
                "devices.0.sidechain.input",
                "take_lanes.0.clips.1.data.gain",
                "clips.0",
            ][..],
        ),
        (&verb, &["mixer.tap"][..]),
        (
            &keys,
            &[
                "routing.midi_output",
                "slots.0.clip",
                "slots.0.scene",
                "removed_stop_buttons.0",
                "clips.0",
            ][..],
        ),
        (
            &document["main"],
            &["routing.main_out", "routing.cue_out"][..],
        ),
        (
            &document["scenes"],
            &["0.name", "0.tempo", "0.signature"][..],
        ),
        (
            &devices
                .iter()
                .find(|d| d["name"] == "Audio Effect Rack")
                .unwrap()
                .clone(),
            &[
                "chains.0.colour",
                "chains.0.info",
                "chains.0.mixer.volume",
                "chains.0.mixer.pan",
                "chains.0.mixer.solo",
                "chains.1.mixer.enabled",
                "chains.0.devices.1.chains.0.name",
            ][..],
        ),
        (
            &devices[0],
            &[
                "own_name",
                "enabled",
                "show_preset_name",
                "params",
                "sidechain",
                "floating",
                "media",
            ][..],
        ),
        (
            &clips
                .iter()
                .find(|c| c["name"] == "Verse vox")
                .unwrap()
                .clone(),
            &[
                "mute",
                "envelopes.0",
                "data.audio.start",
                "data.audio.end",
                "data.audio.looping",
                "data.audio.loop_start",
                "data.audio.loop_end",
                "data.audio.gain",
                "data.audio.fade_in",
                "data.audio.fade_out",
            ][..],
        ),
        (
            &clips
                .iter()
                .find(|c| c["name"] == "Chords")
                .unwrap()
                .clone(),
            &[
                "offset",
                "data.midi.start",
                "data.midi.end",
                "data.midi.looping",
                "data.midi.loop_start",
                "data.midi.loop_end",
                "data.midi.signature",
                "data.midi.notes.0.probability",
            ][..],
        ),
    ] {
        for path in paths {
            assert!(
                has(value, path),
                "the fixture leaves `{path}` at its default"
            );
        }
    }
}

#[test]
fn full_query_results_hold_everything_the_set_format_encodes() {
    let project = fixture();
    let document = serde_json::to_value(&project.document).unwrap();
    let mut client = open(&project, "covers");
    let (tracks, clips, devices) = saved_parts(&document);

    for track in &tracks {
        let got = query(&mut client, json!({ "id": track["id"], "detail": "full" }));
        covers(track, &got["saved"], "track")
            .unwrap_or_else(|why| panic!("{} {why}", track["name"]));
    }
    for clip in &clips {
        let got = query(&mut client, json!({ "id": clip["id"], "detail": "full" }));
        covers(clip, &got["saved"], "clip").unwrap_or_else(|why| panic!("{} {why}", clip["name"]));
    }
    for device in &devices {
        let got = query(&mut client, json!({ "id": device["id"], "detail": "full" }));
        covers(device, &got["saved"], "device").unwrap();
    }
    for chain in saved_chains(&document) {
        let got = query(&mut client, json!({ "id": chain["id"], "detail": "full" }));
        covers(&chain, &got["saved"], "chain").unwrap_or_else(|why| panic!("{why}"));
    }
    for (n, scene) in document["scenes"].as_array().unwrap().iter().enumerate() {
        let path = format!("scene {}", n + 1);
        let got = query(&mut client, json!({ "path": path, "detail": "full" }));
        assert_eq!(got["number"], n + 1);
        covers(scene, &got["saved"], "scene").unwrap();
    }
    let song = query(&mut client, json!({ "path": "song", "detail": "full" }));
    covers(
        &document["settings"],
        &song["saved"]["settings"],
        "settings",
    )
    .unwrap();
    covers(&document["scenes"], &song["saved"]["scenes"], "scenes").unwrap();
    assert_eq!(
        song["saved"]["launch"],
        serde_json::to_value(project.launch).unwrap()
    );

    // The check does fail on a field the query leaves out.
    let track = &tracks[0];
    let mut got =
        query(&mut client, json!({ "id": track["id"], "detail": "full" }))["saved"].clone();
    got.as_object_mut().unwrap().remove("name");
    assert!(covers(track, &got, "track").unwrap_err().contains("name"));
}

#[test]
fn scenes_slots_and_automation_are_reached_by_path() {
    let mut client = open(&fixture(), "paths");

    let scene = query(&mut client, json!({ "path": "scene \"Verse\"" }));
    assert_eq!(scene["number"], 1);
    let second = query(&mut client, json!({ "path": "scene 2" }));
    assert_eq!(second["slots"]["total"], 1);
    assert_eq!(second["slots"]["items"][0]["clip"]["name"], "Loop");

    let slot = query(&mut client, json!({ "path": "track \"Keys\" / slot 2" }));
    assert_eq!(slot["clip"]["name"], "Loop");
    assert_eq!(slot["clip"]["scene"], second["id"]);
    assert_eq!(slot["stop_button"], true);
    let full = query(
        &mut client,
        json!({ "path": "track \"Keys\" / slot 2", "detail": "full" }),
    );
    assert_eq!(full["clip"]["content"]["notes"]["total"], 2);
    let removed = query(&mut client, json!({ "path": "track \"Keys\" / slot 3" }));
    assert_eq!(removed["clip"], Value::Null);
    assert_eq!(removed["stop_button"], false);
    let by_name = query(
        &mut client,
        json!({ "path": "track \"Keys\" / slot \"Verse\"" }),
    );
    assert_eq!(by_name["clip"], Value::Null);

    let lane = query(
        &mut client,
        json!({ "path": "track \"Vox\" / lane \"Take 1\"", "limit": 1 }),
    );
    assert_eq!(lane["number"], 1);
    assert_eq!(lane["passes"]["total"], 2);
    assert_eq!(lane["passes"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(lane["passes"]["items"][0]["data"]["gain"], 0.7);
    let second_lane = query(&mut client, json!({ "path": "track \"Vox\" / lane 2" }));
    assert_eq!(second_lane["name"], "Take 2");

    let list = query(&mut client, json!({ "path": "track \"Vox\" / automation" }));
    assert_eq!(list["automation"]["total"], 2);
    let volume = query(
        &mut client,
        json!({ "path": "track \"Vox\" / automation \"volume\"", "detail": "full", "limit": 2 }),
    );
    assert_eq!(volume["points"]["total"], 3);
    assert_eq!(volume["points"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(volume["points"]["next_offset"], 2);
    assert_eq!(volume["points"]["items"][1]["bend"], 0.5);
    let pan = query(
        &mut client,
        json!({ "path": "track \"Vox\" / automation \"pan\"" }),
    );
    assert_eq!(pan["enabled"], false);
    assert_eq!(pan["points"], 1);

    for (path, message) in [
        ("track \"Vox\" / slot 99", "no slot matches"),
        ("track \"Bus\" / slot 1", "no Session slots"),
        ("track \"Vox\" / automation \"send 99\"", "no envelope"),
        ("scene 99", "no scene matches"),
        ("track \"Vox\" / lane 3", "no lane matches"),
    ] {
        let result = client.call("query", json!({ "path": path })).unwrap();
        assert!(
            result.is_error && result.text.contains(message),
            "{path}: {}",
            result.text
        );
    }
}

#[test]
fn saved_forms_come_a_page_at_a_time() {
    let mut client = open(&fixture(), "pages");
    // A list in a list shrinks to its length; the item's own path reads it.
    let keys = query(
        &mut client,
        json!({ "path": "track \"Keys\"", "detail": "full", "limit": 1 }),
    );
    let clips = &keys["saved"]["clips"];
    assert_eq!(clips["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        clips["items"][0]["data"]["midi"]["notes"],
        json!({ "total": 3 })
    );
    let slots = &keys["saved"]["slots"];
    assert_eq!(slots["total"], 1);
    assert_eq!(
        slots["items"][0]["clip"]["data"]["midi"]["notes"],
        json!({ "total": 2 })
    );
    // The default stays compact: no saved form.
    let summary = query(&mut client, json!({ "path": "track \"Keys\"" }));
    assert!(summary.get("saved").is_none());
    let vox = query(
        &mut client,
        json!({ "path": "track \"Vox\"", "detail": "full", "limit": 1, "offset": 1 }),
    );
    assert_eq!(
        vox["saved"]["automation"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(vox["saved"]["automation"]["offset"], 1);
    assert_eq!(
        vox["saved"]["automation"]["items"][0]["points"],
        json!({ "total": 1 })
    );
}

/// Whether `field` (dotted names, `[]` after a list) is in `value`, looking
/// through a page's `items`.
fn present(value: &Value, field: &str) -> bool {
    let mut here = vec![value];
    for segment in field.split('.') {
        let (key, is_list) = segment
            .strip_suffix("[]")
            .map_or((segment, false), |key| (key, true));
        let mut next: Vec<&Value> = here.iter().filter_map(|value| value.get(key)).collect();
        if is_list {
            next = next
                .iter()
                .flat_map(|list| {
                    let items = list.get("items").unwrap_or(list);
                    items.as_array().into_iter().flatten()
                })
                .collect();
        }
        if next.is_empty() {
            return false;
        }
        here = next;
    }
    true
}

#[test]
fn every_query_readback_the_command_table_names_is_in_a_real_full_result() {
    use bl_commands::readback::{Object, READBACK, Readback};

    let project = fixture();
    let document = serde_json::to_value(&project.document).unwrap();
    let mut client = open(&project, "declared");
    let (tracks, clips, devices) = saved_parts(&document);
    let mut full = |ids: &[Value]| -> Vec<Value> {
        ids.iter()
            .map(|saved| {
                query(&mut client, json!({ "id": saved["id"], "detail": "full" }))["saved"].clone()
            })
            .collect()
    };
    let (tracks, clips, devices) = (full(&tracks), full(&clips), full(&devices));
    let scenes: Vec<Value> = (1..=document["scenes"].as_array().unwrap().len())
        .map(|n| {
            let path = format!("scene {n}");
            query(&mut client, json!({ "path": path, "detail": "full" }))["saved"].clone()
        })
        .collect();
    let song = query(&mut client, json!({ "path": "song", "detail": "full" }));

    let mut missing: Vec<String> = Object::Song
        .listed()
        .iter()
        .filter(|field| !present(&song, field))
        .map(|field| format!("the song's listed field `{field}`"))
        .collect();
    for (command, readback) in READBACK {
        let Readback::Query { object, field } = readback else {
            continue;
        };
        let found = match object {
            Object::Song => present(&song, field) || present(&song["saved"], field),
            Object::Track => tracks.iter().any(|saved| present(saved, field)),
            Object::Clip => clips.iter().any(|saved| present(saved, field)),
            Object::Device => devices.iter().any(|saved| present(saved, field)),
            Object::Scene => scenes.iter().any(|saved| present(saved, field)),
        };
        if !found {
            missing.push(format!("{command}: `{field}` of a {} query", object.path()));
        }
    }
    assert!(
        missing.is_empty(),
        "the fixture's full results lack:\n{}",
        missing.join("\n")
    );
}
