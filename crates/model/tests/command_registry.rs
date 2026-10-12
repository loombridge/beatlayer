//! The document's commands in the command registry (WP-0.14): every
//! command is complete, every `Command` variant is registered, and every
//! registered command runs by ID with JSON arguments that fit its schema,
//! as an agent would run it (D17).

use std::collections::BTreeSet;

use bl_commands::{Registry, args_schema, tagged_variant, validate};
use bl_core::time::{Bpm, TempoMap, Ticks, TimeSignature, TimeSignatureMap};
use bl_model::registry::{self, RunError};
use bl_model::{
    ArrangementLoop, AudioData, Change, ChangeSet, Clip, ClipData, ClipId, Command, ContentHash,
    Device, DeviceId, DeviceKind, EditError, Editor, Input, MidiData, ModelError, Monitor, Note,
    Output, ParamId, PluginFormat, SendTap, Tap, TrackId, TrackKind,
};
use serde_json::{Value, json};

fn document_registry() -> Registry {
    let mut commands = Registry::new();
    registry::register(&mut commands).expect("no ID is registered twice");
    commands
}

/// The acceptance test for D17: it fails, listing each one, if any command
/// lacks a schema, a description, a permission class, a context, or a
/// label, or has a malformed ID or binding.
#[test]
fn every_command_is_complete() {
    let commands = document_registry();
    if let Err(problems) = commands.check() {
        panic!("{problems}");
    }
}

/// Every variant of `Command` is registered once, with the variant's own
/// schema as its arguments, and so are undo and redo.
#[test]
fn every_document_command_is_registered() {
    let commands = document_registry();
    let enum_schema = args_schema::<Command>();
    let ops: Vec<&str> = enum_schema
        .get("oneOf")
        .and_then(Value::as_array)
        .expect("Command's schema lists its variants")
        .iter()
        .map(|variant| variant["properties"]["op"]["const"].as_str().unwrap())
        .collect();
    assert_eq!(
        ops.len(),
        49,
        "WP-0.6 defined 25 commands, WP-1.5 the two for the Arrangement loop, WP-1.6 five for \
         the mixer, WP-1.4 five for audio clips, WP-1.8b trim_clip_start, WP-2.19 four for \
         routing, WP-2.4 two for devices, and WP-2.19b five for sidechains"
    );

    for op in &ops {
        let variant = tagged_variant(&enum_schema, "op", op).unwrap();
        let registered: Vec<&str> = commands
            .iter()
            .filter(|spec| spec.args.as_ref() == Some(&variant))
            .map(|spec| spec.id.as_ref())
            .collect();
        assert_eq!(
            registered.len(),
            1,
            "`{op}` is registered as {registered:?}"
        );
    }
    assert!(commands.contains(registry::UNDO));
    assert!(commands.contains(registry::REDO));
    assert_eq!(commands.len(), ops.len() + 2);
    assert!(commands.iter().all(|spec| registry::handles(&spec.id)));
    assert!(!registry::handles("transport.play"));
}

/// Runs commands by ID as an agent does, checking each one's arguments and
/// result against its schemas, and records which commands it ran.
struct Agent {
    editor: Editor,
    commands: Registry,
    ran: BTreeSet<String>,
}

impl Agent {
    fn new() -> Self {
        Self {
            editor: Editor::new(),
            commands: document_registry(),
            ran: BTreeSet::new(),
        }
    }

    fn run(&mut self, id: &str, args: Value) -> ChangeSet {
        let spec = self
            .commands
            .get(id)
            .unwrap_or_else(|| panic!("{id} isn't registered"));
        validate(spec.args.as_ref().unwrap(), &args)
            .unwrap_or_else(|error| panic!("{id}: the arguments don't fit the schema: {error}"));
        let changes = registry::run(&mut self.editor, id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        let result = serde_json::to_value(&changes).unwrap();
        validate(spec.result.as_ref().unwrap(), &result)
            .unwrap_or_else(|error| panic!("{id}: the result doesn't fit the schema: {error}"));
        self.ran.insert(id.to_owned());
        changes
    }

    fn new_id<T: From<bl_core::id::Id>>(&mut self) -> T {
        self.editor.new_id()
    }
}

/// Every registered command runs by its ID with JSON arguments (D17's
/// "reachable through the agent API"; WP-1.15 serves this over MCP), and
/// each run is one undo step.
#[test]
fn every_command_runs_by_id() {
    let mut agent = Agent::new();
    let quarter = Ticks::QUARTER.0;
    let bar = Ticks::WHOLE.0;

    // Tracks.
    let drums: TrackId = agent.new_id();
    let changes = agent.run(
        "track.create",
        json!({ "id": drums, "kind": "audio", "name": "Drums", "index": 0 }),
    );
    assert!(changes.layout);
    assert_eq!(changes.tracks.get(&drums), Some(&Change::Added));
    let bass: TrackId = agent.new_id();
    agent.run(
        "track.create",
        json!({ "id": bass, "kind": "midi", "name": "Bass", "parent": null, "index": 1 }),
    );
    let group: TrackId = agent.new_id();
    agent.run(
        "track.create",
        json!({ "id": group, "kind": "group", "name": "Low end", "index": 2 }),
    );
    let reverb: TrackId = agent.new_id();
    agent.run(
        "track.create",
        json!({ "id": reverb, "kind": "return", "name": "Reverb", "index": 0 }),
    );
    agent.run(
        "track.move",
        json!({ "track": bass, "parent": group, "index": 0 }),
    );
    agent.run("track.rename", json!({ "track": drums, "name": "Break" }));
    let document = agent.editor.document();
    assert_eq!(document.parent(bass), Some(group));
    assert_eq!(document.track(drums).unwrap().name(), "Break");
    assert_eq!(document.track(reverb).unwrap().kind(), TrackKind::Return);

    // Mixer.
    agent.run("mixer.set_volume", json!({ "track": drums, "volume": 0.5 }));
    agent.run("mixer.set_pan", json!({ "track": drums, "pan": -0.25 }));
    agent.run("mixer.set_mute", json!({ "track": drums, "mute": true }));
    agent.run("mixer.set_solo", json!({ "track": drums, "solo": true }));
    agent.run("mixer.solo_exclusive", json!({ "track": bass }));
    agent.run("mixer.set_arm", json!({ "track": drums, "arm": true }));
    agent.run("mixer.arm_exclusive", json!({ "track": bass }));
    agent.run(
        "mixer.set_send",
        json!({ "track": group, "to": reverb, "level": 0.5 }),
    );
    agent.run(
        "mixer.set_send_tap",
        json!({ "track": reverb, "tap": "pre" }),
    );
    let changes = agent.run("mixer.set_solo_in_place", json!({ "enabled": false }));
    assert!(changes.settings);
    let document = agent.editor.document();
    let mixer = *document.track(drums).unwrap().mixer();
    assert_eq!((mixer.volume, mixer.pan, mixer.mute), (0.5, -0.25, true));
    assert!(
        !mixer.solo && !mixer.arm,
        "released by the exclusive switches"
    );
    let mixer = *document.track(bass).unwrap().mixer();
    assert!(mixer.solo && mixer.arm);
    assert_eq!(document.track(group).unwrap().send(reverb), 0.5);
    assert_eq!(document.track(reverb).unwrap().mixer().tap, SendTap::Pre);
    assert!(!document.settings().solo_in_place);

    // Routing.
    let keys: TrackId = agent.new_id();
    agent.run(
        "track.create",
        json!({ "id": keys, "kind": "midi", "name": "Keys", "index": 0 }),
    );
    agent.run(
        "routing.set_input",
        json!({ "track": drums, "input": { "type": "track", "track": reverb, "tap": "post_fx" } }),
    );
    agent.run(
        "routing.set_monitor",
        json!({ "track": drums, "monitor": "in" }),
    );
    agent.run(
        "routing.set_output",
        json!({ "track": group, "output": { "type": "sends_only" } }),
    );
    agent.run(
        "routing.set_midi_output",
        json!({ "track": keys, "to": bass }),
    );
    let document = agent.editor.document();
    let routing = *document.track(drums).unwrap().routing();
    assert_eq!(
        routing.input,
        Input::Track {
            track: reverb,
            tap: Tap::PostFx
        }
    );
    assert_eq!(routing.monitor, Monitor::In);
    assert_eq!(
        document.track(group).unwrap().routing().output,
        Output::SendsOnly
    );
    assert_eq!(
        document.track(keys).unwrap().routing().midi_output,
        Some(bass)
    );
    assert_eq!(
        agent.editor.undo_history().next(),
        Some("Change \"Routing\"")
    );

    // Song settings.
    let changes = agent.run(
        "song.set_tempo",
        json!({ "tempo": [
            { "kind": "ramp", "start": 0, "from": 90.0, "to": 120.0 },
            { "kind": "constant", "start": bar * 4, "bpm": 120.0 },
        ] }),
    );
    assert!(changes.settings);
    agent.run(
        "song.set_time_signatures",
        json!({ "time_signatures": [{ "at": 0, "signature": { "numerator": 7, "denominator": 8 } }] }),
    );
    let changes = agent.run(
        "transport.set_loop",
        json!({ "start": bar * 2, "length": bar }),
    );
    assert!(changes.settings);
    agent.run("transport.set_loop_enabled", json!({ "enabled": true }));
    let settings = agent.editor.document().settings();
    assert_eq!(
        settings.arrangement_loop,
        ArrangementLoop {
            start: Ticks(bar * 2),
            length: Ticks(bar),
            enabled: true,
        }
    );
    assert_eq!(settings.tempo.bpm_at(Ticks::ZERO), Bpm::new(90.0).unwrap());
    assert_eq!(
        settings.time_signatures.signature_at(Ticks::ZERO),
        TimeSignature::new(7, 8).unwrap()
    );

    // Clips.
    let riff: ClipId = agent.new_id();
    agent.run(
        "clip.create",
        json!({ "track": bass, "clip": {
            "id": riff, "name": "", "start": 0, "length": bar, "mute": false,
            "data": { "midi": { "notes": [
                { "key": 36, "velocity": 100, "start": 0, "length": quarter },
            ] } },
        } }),
    );
    let loop_clip: ClipId = agent.new_id();
    agent.run(
        "clip.create",
        json!({ "track": drums, "clip": {
            "id": loop_clip, "name": "Loop", "start": 0, "length": bar, "mute": false,
            "data": { "audio": { "media": "ab".repeat(32) } },
        } }),
    );
    agent.run(
        "clip.move",
        json!({ "clip": riff, "track": bass, "start": bar * 2 }),
    );
    agent.run("clip.move_by", json!({ "clips": [riff], "by": -quarter }));
    agent.run(
        "clip.set_length",
        json!({ "clip": riff, "length": bar * 2 }),
    );
    agent.run("clip.rename", json!({ "clip": riff, "name": "Riff" }));
    agent.run("clip.set_mute", json!({ "clip": riff, "mute": true }));
    agent.run(
        "clip.set_data",
        json!({ "clip": riff, "data": { "midi": { "notes": [
            { "key": 36, "velocity": 110, "start": 0, "length": quarter },
            { "key": 43, "velocity": 90, "start": quarter * 2, "length": quarter },
        ] } } }),
    );
    let clip = agent.editor.document().clip(riff).unwrap();
    assert_eq!(clip.start.0, bar * 2 - quarter);
    assert_eq!(clip.length.0, bar * 2);
    assert_eq!((clip.name.as_str(), clip.mute), ("Riff", true));
    let ClipData::Midi(midi) = &clip.data else {
        panic!("{clip:?}");
    };
    assert_eq!(midi.notes.len(), 2);
    // Trimming the left edge keeps the end and the contents in place: an
    // audio clip's Start marker moves on by the time of a quarter note.
    agent.run(
        "clip.trim_start",
        json!({ "clip": loop_clip, "start": quarter }),
    );
    let trimmed = agent.editor.document().clip(loop_clip).unwrap();
    assert_eq!(
        (trimmed.start.0, trimmed.end().0, trimmed.offset.0),
        (quarter, bar, 0)
    );
    let ClipData::Audio(audio) = &trimmed.data else {
        panic!("{trimmed:?}");
    };
    let tempo = &agent.editor.document().settings().tempo;
    assert_eq!(
        audio.start,
        tempo.ticks_to_seconds(bl_core::time::Ticks(quarter))
    );
    // An audio clip's settings (WP-1.4).
    agent.run(
        "clip.set_region",
        json!({ "clip": loop_clip, "start": 0.5, "end": 2.5 }),
    );
    agent.run(
        "clip.set_loop",
        json!({ "clip": loop_clip, "start": 1.0, "end": 2.0 }),
    );
    agent.run(
        "clip.set_loop_enabled",
        json!({ "clip": loop_clip, "enabled": true }),
    );
    agent.run("clip.set_gain", json!({ "clip": loop_clip, "gain": 0.5 }));
    agent.run(
        "clip.set_fades",
        json!({ "clip": loop_clip, "fade_in": 0.004, "fade_out": 0.25 }),
    );
    let ClipData::Audio(audio) = agent.editor.document().clip(loop_clip).unwrap().data else {
        panic!("an audio clip");
    };
    assert_eq!(
        audio,
        AudioData {
            start: 0.5,
            end: Some(2.5),
            looping: true,
            loop_start: 1.0,
            loop_end: Some(2.0),
            gain: 0.5,
            fade_in: 0.004,
            fade_out: 0.25,
            ..AudioData::new(ContentHash([0xab; 32]))
        }
    );
    // A null end is the end of the file.
    agent.run(
        "clip.set_region",
        json!({ "clip": loop_clip, "start": 0.0, "end": null }),
    );
    agent.run("clip.delete", json!({ "clip": loop_clip }));
    assert!(agent.editor.document().clip(loop_clip).is_none());

    // Devices.
    let eq: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": drums, "index": 0, "device": {
            "id": eq, "name": "EQ", "enabled": true, "params": [[1, 0.5]],
            "kind": { "native": { "device_type": "beatlayer.eq" } },
        } }),
    );
    let synth: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": reverb, "index": 0, "device": {
            "id": synth, "name": "Space", "enabled": true, "params": [],
            "kind": { "plugin": { "format": "clap", "plugin_id": "org.example.space" } },
        } }),
    );
    agent.run(
        "device.move",
        json!({ "device": eq, "track": reverb, "index": 1 }),
    );
    agent.run("device.rename", json!({ "device": eq, "name": "Tone" }));
    agent.run(
        "device.set_show_preset_name",
        json!({ "device": eq, "shown": false }),
    );
    assert_eq!(agent.editor.document().device(eq).unwrap().title(), "EQ");
    agent.run(
        "device.set_show_preset_name",
        json!({ "device": eq, "shown": true }),
    );
    agent.run(
        "device.set_state",
        json!({ "device": eq, "params": [[1, 0.25]], "name": "Tone" }),
    );
    agent.run(
        "device.set_enabled",
        json!({ "device": eq, "enabled": false }),
    );
    agent.run(
        "device.set_param",
        json!({ "device": eq, "param": 3, "value": -12.5 }),
    );
    let device = agent.editor.document().device(eq).unwrap();
    assert_eq!(agent.editor.document().device_track(eq), Some(reverb));
    assert_eq!((device.name.as_str(), device.enabled), ("Tone", false));
    assert_eq!(device.params.get(&ParamId(3)), Some(&-12.5));
    assert_eq!(device.params.get(&ParamId(1)), Some(&0.25));
    agent.run("device.remove", json!({ "device": synth }));
    assert!(agent.editor.document().device(synth).is_none());

    // A device with a sidechain input (WP-2.19b), listening to another
    // track.
    let duck: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": reverb, "index": 0, "device": {
            "id": duck, "name": "Duck", "enabled": true, "params": [],
            "kind": { "native": { "device_type": "beatlayer.duck" } },
            "sidechain": {},
        } }),
    );
    agent.run(
        "device.set_sidechain_input",
        json!({ "device": duck, "input": { "type": "track", "track": drums, "tap": "post_fx" } }),
    );
    agent.run(
        "device.set_sidechain_enabled",
        json!({ "device": duck, "enabled": true }),
    );
    agent.run(
        "device.set_sidechain_gain",
        json!({ "device": duck, "gain": 2.0 }),
    );
    agent.run(
        "device.set_sidechain_mix",
        json!({ "device": duck, "mix": 0.5 }),
    );
    agent.run(
        "device.set_sidechain_listen",
        json!({ "device": duck, "listen": true }),
    );
    let sidechain = agent
        .editor
        .document()
        .device(duck)
        .unwrap()
        .sidechain
        .unwrap();
    assert_eq!(
        sidechain,
        bl_model::Sidechain {
            enabled: true,
            input: Input::Track {
                track: drums,
                tap: Tap::PostFx
            },
            gain: 2.0,
            mix: 0.5,
            listen: true,
        }
    );

    // Deleting the group deletes the bass inside it; undo brings both back.
    agent.run("track.delete", json!({ "track": group }));
    assert!(agent.editor.document().track(bass).is_none());
    let steps = agent.editor.undo_history().len();
    let changes = agent.run(registry::UNDO, json!({}));
    assert_eq!(changes.tracks.get(&group), Some(&Change::Added));
    assert!(agent.editor.document().clip(riff).is_some());
    agent.run(registry::REDO, json!({}));
    assert!(agent.editor.document().track(group).is_none());
    assert_eq!(agent.editor.undo_history().len(), steps);

    // One undo step per edit, labelled as the document labels it.
    let edits = agent.ran.len() - 2;
    assert!(steps >= edits, "{steps} steps for {edits} kinds of edit");
    assert_eq!(agent.editor.undo_history().next(), Some("Delete track"));

    let registered: BTreeSet<String> = agent.commands.iter().map(|s| s.id.to_string()).collect();
    assert_eq!(
        agent.ran, registered,
        "every registered command is run here; add a run for a new one"
    );
}

/// For each command, what serialising a `Command` writes fits the
/// registered schema, and reads back as the same command through its ID.
#[test]
fn schemas_match_what_commands_serialise() {
    let commands = document_registry();
    let mut ids = bl_core::id::IdAllocator::new();
    let mut fresh = || ids.allocate();
    let track = TrackId(fresh());
    let clip = ClipId(fresh());
    let device = DeviceId(fresh());
    let notes = MidiData::new([Note {
        key: 127,
        velocity: 1,
        start: Ticks::ZERO,
        length: Ticks::QUARTER,
    }]);
    let mut plugin = Device::new(
        device,
        "Synth",
        DeviceKind::Plugin {
            format: PluginFormat::Vst3,
            plugin_id: "ABCDEF".to_owned(),
        },
    );
    plugin.params.insert(ParamId(7), 0.25);
    let examples = [
        (
            "song.set_tempo",
            Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(140.0).unwrap()),
            },
        ),
        (
            "song.set_time_signatures",
            Command::SetTimeSignatures {
                time_signatures: TimeSignatureMap::constant(TimeSignature::new(3, 4).unwrap()),
            },
        ),
        (
            "transport.set_loop",
            Command::SetLoop {
                start: Ticks::QUARTER * 3,
                length: Ticks::WHOLE * 2,
            },
        ),
        (
            "transport.set_loop_enabled",
            Command::SetLoopEnabled { enabled: true },
        ),
        (
            "track.create",
            Command::CreateTrack {
                id: track,
                kind: TrackKind::Group,
                name: "Drums".to_owned(),
                parent: Some(TrackId(fresh())),
                index: 3,
            },
        ),
        ("track.delete", Command::DeleteTrack { track }),
        (
            "track.move",
            Command::MoveTrack {
                track,
                parent: None,
                index: 0,
            },
        ),
        (
            "track.rename",
            Command::RenameTrack {
                track,
                name: "Keys".to_owned(),
            },
        ),
        (
            "mixer.set_volume",
            Command::SetVolume { track, volume: 0.0 },
        ),
        ("mixer.set_pan", Command::SetPan { track, pan: 1.0 }),
        ("mixer.set_mute", Command::SetMute { track, mute: false }),
        ("mixer.set_solo", Command::SetSolo { track, solo: true }),
        ("mixer.solo_exclusive", Command::SoloExclusive { track }),
        ("mixer.set_arm", Command::SetArm { track, arm: true }),
        ("mixer.arm_exclusive", Command::ArmExclusive { track }),
        (
            "mixer.set_send",
            Command::SetSend {
                track,
                to: TrackId(fresh()),
                level: 1.0,
            },
        ),
        (
            "mixer.set_send_tap",
            Command::SetSendTap {
                track,
                tap: SendTap::Pre,
            },
        ),
        (
            "mixer.set_solo_in_place",
            Command::SetSoloInPlace { enabled: false },
        ),
        (
            "routing.set_input",
            Command::SetInput {
                track,
                input: Input::Track {
                    track: TrackId(fresh()),
                    tap: Tap::PreFx,
                },
            },
        ),
        (
            "routing.set_monitor",
            Command::SetMonitor {
                track,
                monitor: Monitor::Off,
            },
        ),
        (
            "routing.set_output",
            Command::SetOutput {
                track,
                output: Output::Track {
                    track: TrackId(fresh()),
                },
            },
        ),
        (
            "routing.set_midi_output",
            Command::SetMidiOutput { track, to: None },
        ),
        (
            "clip.create",
            Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks::WHOLE,
                    Ticks::WHOLE,
                    ClipData::Midi(notes.clone()),
                ),
            },
        ),
        ("clip.delete", Command::DeleteClip { clip }),
        (
            "clip.move",
            Command::MoveClip {
                clip,
                track,
                start: Ticks::QUARTER,
            },
        ),
        (
            "clip.move_by",
            Command::MoveClips {
                clips: vec![clip, ClipId(fresh())],
                by: -Ticks::QUARTER,
            },
        ),
        (
            "clip.set_length",
            Command::SetClipLength {
                clip,
                length: Ticks::WHOLE,
            },
        ),
        (
            "clip.rename",
            Command::RenameClip {
                clip,
                name: "Hook".to_owned(),
            },
        ),
        ("clip.set_mute", Command::SetClipMute { clip, mute: true }),
        (
            "clip.set_data",
            Command::SetClipData {
                clip,
                data: ClipData::Audio(AudioData {
                    end: Some(3.5),
                    gain: 0.25,
                    ..AudioData::new(ContentHash([0xab; 32])).with_edge_fades()
                }),
            },
        ),
        (
            "clip.trim_start",
            Command::TrimClipStart {
                clip,
                start: Ticks::QUARTER,
            },
        ),
        (
            "clip.set_region",
            Command::SetClipRegion {
                clip,
                start: 0.125,
                end: Some(1.5),
            },
        ),
        (
            "clip.set_loop",
            Command::SetClipLoop {
                clip,
                start: 0.5,
                end: None,
            },
        ),
        (
            "clip.set_loop_enabled",
            Command::SetClipLoopEnabled {
                clip,
                enabled: true,
            },
        ),
        ("clip.set_gain", Command::SetClipGain { clip, gain: 2.0 }),
        (
            "clip.set_fades",
            Command::SetClipFades {
                clip,
                fade_in: 0.004,
                fade_out: 1.5,
            },
        ),
        (
            "device.insert",
            Command::InsertDevice {
                track,
                index: 0,
                device: plugin,
            },
        ),
        ("device.remove", Command::RemoveDevice { device }),
        (
            "device.move",
            Command::MoveDevice {
                device,
                track,
                index: 2,
            },
        ),
        (
            "device.rename",
            Command::RenameDevice {
                device,
                name: "Pad".to_owned(),
            },
        ),
        (
            "device.set_show_preset_name",
            Command::SetShowPresetName {
                device,
                shown: false,
            },
        ),
        (
            "device.set_state",
            Command::SetDeviceState {
                device,
                params: [(ParamId(4), 0.5)].into_iter().collect(),
                name: "Warm Pad".to_owned(),
            },
        ),
        (
            "device.set_enabled",
            Command::SetDeviceEnabled {
                device,
                enabled: false,
            },
        ),
        (
            "device.set_param",
            Command::SetDeviceParam {
                device,
                param: ParamId(u32::MAX),
                value: -1e9,
            },
        ),
        (
            "device.set_sidechain_input",
            Command::SetSidechainInput {
                device,
                input: Input::Track {
                    track,
                    tap: Tap::PostMixer,
                },
            },
        ),
        (
            "device.set_sidechain_enabled",
            Command::SetSidechainEnabled {
                device,
                enabled: true,
            },
        ),
        (
            "device.set_sidechain_gain",
            Command::SetSidechainGain { device, gain: 0.5 },
        ),
        (
            "device.set_sidechain_mix",
            Command::SetSidechainMix { device, mix: 0.25 },
        ),
        (
            "device.set_sidechain_listen",
            Command::SetSidechainListen {
                device,
                listen: true,
            },
        ),
    ];
    let covered: BTreeSet<&str> = examples.iter().map(|(id, _)| *id).collect();
    let document_commands: BTreeSet<&str> = commands
        .iter()
        .map(|spec| spec.id.as_ref())
        .filter(|id| *id != registry::UNDO && *id != registry::REDO)
        .collect();
    assert_eq!(
        covered, document_commands,
        "one example per document command"
    );

    for (id, command) in examples {
        let Value::Object(mut args) = serde_json::to_value(&command).unwrap() else {
            panic!("{id} serialises as an object");
        };
        args.remove("op");
        let args = Value::Object(args);
        let schema = commands.get(id).unwrap().args.as_ref().unwrap();
        validate(schema, &args).unwrap_or_else(|error| panic!("{id}: {error}\n{args}"));
        assert_eq!(
            registry::command_from_args(id, args).unwrap(),
            command,
            "{id}"
        );
    }
}

/// A bad request is refused with a reason and changes nothing.
#[test]
fn bad_requests_change_nothing() {
    let commands = document_registry();
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    registry::run(
        &mut editor,
        "track.create",
        json!({ "id": track, "kind": "audio", "name": "A", "index": 0 }),
    )
    .unwrap();
    let before = editor.document().clone();
    let steps = editor.undo_history().len();

    type Expected<'a> = &'a dyn Fn(RunError) -> bool;
    let invalid = |error: RunError| matches!(error, RunError::InvalidArgs { .. });
    let cases: [(&str, Value, Expected<'_>); 11] = [
        ("track.explode", json!({}), &|e| {
            e == RunError::UnknownCommand("track.explode".to_owned())
        }),
        ("track.rename", json!([track, "B"]), &invalid),
        ("track.rename", json!({ "track": track }), &invalid),
        (
            "mixer.set_volume",
            json!({ "track": track, "volume": "loud" }),
            &invalid,
        ),
        ("edit.redo", json!({ "steps": 2 }), &invalid),
        ("edit.redo", Value::Null, &invalid),
        ("edit.redo", json!({}), &|e| {
            e == RunError::Edit(EditError::NothingToRedo)
        }),
        (
            "mixer.set_volume",
            json!({ "track": track, "volume": -1.0 }),
            &|e| {
                matches!(
                    e,
                    RunError::Edit(EditError::Command(ModelError::InvalidValue { .. }))
                )
            },
        ),
        (
            "track.create",
            json!({ "id": 999, "kind": "audio", "name": "Stolen", "index": 0 }),
            &|e| matches!(e, RunError::Edit(EditError::IdNotIssued(_))),
        ),
        // Spec: transport R17
        (
            "transport.set_loop",
            json!({ "start": -1, "length": 705_600 }),
            &|e| {
                matches!(
                    e,
                    RunError::Edit(EditError::Command(ModelError::NegativePosition(_)))
                )
            },
        ),
        (
            "transport.set_loop",
            json!({ "start": 0, "length": 0 }),
            &|e| {
                matches!(
                    e,
                    RunError::Edit(EditError::Command(ModelError::EmptyLength(_)))
                )
            },
        ),
    ];
    for (id, args, expected) in cases {
        let error = registry::run(&mut editor, id, args.clone()).unwrap_err();
        assert!(!error.to_string().is_empty());
        assert!(expected(error.clone()), "{id} {args}: {error:?}");
        assert_eq!(editor.document(), &before, "{id} {args}");
        assert_eq!(editor.undo_history().len(), steps, "{id} {args}");
    }

    // The schema rejects the out-of-range volume before the document does.
    let schema = commands
        .get("mixer.set_volume")
        .unwrap()
        .args
        .as_ref()
        .unwrap();
    assert!(validate(schema, &json!({ "track": track, "volume": -1.0 })).is_err());
    assert_eq!(
        registry::command_from_args(registry::UNDO, json!({})),
        Err(RunError::NotADocumentEdit(registry::UNDO))
    );
}

/// The default bindings follow the command bindings spec.
#[test]
fn bindings_follow_the_spec() {
    let commands = document_registry();
    let bindings = |id: &str| -> Vec<String> {
        commands
            .get(id)
            .unwrap()
            .bindings
            .iter()
            .map(|binding| {
                let mut text = binding.shortcut.to_string();
                for (name, value) in &binding.args {
                    text.push_str(&format!(" {name}={value}"));
                }
                if let Some(arg) = &binding.toggles {
                    text.push_str(&format!(" toggles {arg}"));
                }
                text
            })
            .collect()
    };

    // Spec: command-bindings R1
    assert_eq!(bindings("edit.undo"), ["S-EDIT-006"]);
    assert_eq!(bindings("edit.redo"), ["S-EDIT-007"]);
    // Spec: command-bindings R2
    assert_eq!(
        bindings("track.create"),
        [
            "S-TRACK-001 kind=\"audio\"",
            "S-TRACK-002 kind=\"midi\"",
            "S-TRACK-003 kind=\"return\""
        ]
    );
    // Spec: command-bindings R3
    assert_eq!(bindings("track.delete"), ["S-TRACK-018"]);
    // The Arrangement's Delete deletes its selection, clips or time
    // (`arrangement.delete`, WP-1.8b), so `clip.delete` has no key.
    assert!(bindings("clip.delete").is_empty());
    assert_eq!(bindings("device.remove"), ["S-EDIT-005"]);
    // Spec: command-bindings R4
    assert_eq!(bindings("track.rename"), ["S-TRACK-004"]);
    assert_eq!(bindings("clip.rename"), ["S-EDIT-008"]);
    assert_eq!(bindings("device.rename"), ["S-EDIT-008"]);
    // Spec: command-bindings R5
    assert_eq!(
        bindings("mixer.set_mute"),
        ["S-TRACK-016 toggles mute", "S-TRANS-010 toggles mute"]
    );
    assert_eq!(bindings("mixer.set_solo"), ["S-TRACK-014 toggles solo"]);
    assert_eq!(bindings("mixer.set_arm"), ["S-TRACK-013 toggles arm"]);
    // Spec: command-bindings R6
    assert_eq!(bindings("clip.set_mute"), ["S-ARR-027 toggles mute"]);

    // Spec: command-bindings R7
    let bound: BTreeSet<&str> = commands
        .iter()
        .filter(|spec| !spec.bindings.is_empty())
        .map(|spec| spec.id.as_ref())
        .collect();
    let expected: BTreeSet<&str> = [
        "edit.undo",
        "edit.redo",
        "track.create",
        "track.delete",
        "device.remove",
        "track.rename",
        "clip.rename",
        "device.rename",
        "mixer.set_mute",
        "mixer.set_solo",
        "mixer.set_arm",
        "clip.set_mute",
    ]
    .into_iter()
    .collect();
    assert_eq!(bound, expected, "only these commands have Live shortcuts");
}
