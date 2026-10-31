//! The document's commands in the command registry (WP-0.14): every
//! command is complete, every `Command` variant is registered, and every
//! registered command runs by ID with JSON arguments that fit its schema,
//! as an agent would run it (D17).

use std::collections::BTreeSet;

use bl_commands::{Permission, Registry, args_schema, tagged_variant, validate};
use bl_core::time::{Bpm, TempoMap, Ticks, TimeSignature, TimeSignatureMap};
use bl_model::registry::{self, RunError};
use bl_model::{
    ArrangementLoop, AudioData, AutomationTarget, BeatsUpdate, Breakpoint, Chain, ChainId, Change,
    ChangeSet, Channel, Clip, ClipData, ClipEdge, ClipId, ClipLaunch, Command, ComplexProUpdate,
    ContentHash, CountIn, CrossfadeAssign, Device, DeviceId, DeviceKind, DeviceRole, EditError,
    Editor, ExternalPort, FileBeats, FollowAction, FollowActions, FollowPatch, GrooveBase,
    GrooveId, GrooveNote, HardwareLatency, Input, LaunchMode, LaunchQuantization, Locator,
    LocatorId, MacroStyle, MacroTarget, MetronomeRhythm, MetronomeSound, MidiData, MidiTo,
    MixerParam, ModelError, Monitor, NewTakeLane, Note, Output, PanMode, ParamId, PluginFormat,
    Preserve, RackKind, RecordQuantization, SceneId, SendTap, SongStart, TakeClip, TakeClipData,
    TakeLaneId, Tap, TonesUpdate, TrackDelay, TrackId, TrackKind, Tuning, TuningNote, WarpFrom,
    WarpMode, WarpPoint,
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
    // Spec: mixer R21; routing R20; session-view R12
    let commands = document_registry();
    if let Err(problems) = commands.check() {
        panic!("{problems}");
    }
}

/// Clearing a clip's envelope deletes every breakpoint on it, as deleting a
/// track's automation does, so both are destructive.
#[test]
fn clearing_a_clip_envelope_is_as_destructive_as_deleting_automation() {
    // Spec: agent-permissions
    let commands = document_registry();
    for id in ["clip.delete_envelope", "automation.delete"] {
        assert_eq!(
            commands.get(id).unwrap().permission,
            Some(Permission::Destructive)
        );
    }
}

/// Every variant of `Command` is registered once, with the variant's own
/// schema as its arguments, and so are undo and redo.
#[test]
fn every_document_command_is_registered() {
    // Spec: session-view R12
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
        274,
        "WP-0.6 defined 25 commands, WP-1.5 the two for the Arrangement loop, WP-1.6 five for \
             the mixer, WP-1.4 five for audio clips, WP-1.8b trim_clip_start, WP-2.19 four for \
             routing, WP-2.4 two for devices, WP-2.19b five for sidechains, WP-2.5b \
             set_device_floating, WP-2.20 six for tempo breakpoints and time-signature markers, \
             WP-2.1 two for MIDI clips, WP-2.16 set_device_sample, WP-2.15 five for the metronome, \
             WP-2.13 the two for Main Out and Cue Out, WP-3.1 five for scenes and two for slot \
             clips, WP-3.2 set_slot_stop_button, WP-2.12 seven for automation, WP-6.25 \
             set_automation_envelope and set_lock_envelopes, WP-3.15 six for clip \
             envelopes, WP-6.25c set_clip_envelope, WP-3.17 two for the crossfader, WP-2.14 set_midi_overdub, WP-2.13 the \
             two punch switches, WP-2.13 add_take_lane, WP-3.4 three for a clip's launch \
             settings, WP-3.5 two for a scene's tempo and signature, WP-3.3 \
             set_clip_launch_mode, WP-4.1 seven for the warp map, WP-4.3 five for transients and ranged warp markers, WP-5.1 fifteen for racks and \
             their chains, WP-5.12 the two Compare commands, WP-5.10 map_modulator, \
             unmap_modulator and set_shape, WP-5.13 eleven for a multisampler's layers, WP-5.15 \
             three for a sample's edits, WP-5.15b eight for the sampler's warp controls and manual \
             slices, WP-3.10 the two for follow actions, WP-4.2 four for the \
             warp mode and the RAM and HiQ switches, WP-5.27 set_midi_sidechain_source, WP-4.9 \
             four for auto-warp, WP-5.34 set_external, WP-5.6 five for a drum rack's chains, \
    WP-3.18 the two for Cue mode and the Cue Volume, WP-5.3 set_chain_zone, WP-5.40 \
             set_configured_params, WP-4.13 the two for Crop Clip, WP-5.36 seven for track \
             delay, the pan mode, each split side, and the two latency switches, WP-4.11 \
             four for tempo leaders, stretching and sliding, and WP-5.4 eleven \
             for the macros: map_macro, unmap_macro, set_mapping_range, invert_mapping, \
             rename_macro, set_macro_colour, set_macro_info_text, randomize_macros, \
             set_macro_randomize_excluded, set_macro_style and set_mapped_value, and WP-5.5 seven \
             for the macro variations: store_variation, launch_variation, \
             overwrite_variation, rename_variation, duplicate_variation, \
             delete_variation and set_macro_variation_excluded, WP-4.7 the two \
             for a clip's pitch and its reversal, WP-5.33 set_device_role, WP-5.4b \
             set_macro_style, WP-4.15 the two for bouncing a track in place and to a new \
             track, WP-6.7 the two for the scales: set_clip_scale and set_scale, and \
             WP-6.24 seven for \
             locators: add_locator, delete_locator, move_locator, rename_locator, \
             set_locator_info_text, loop_to_next_locator and set_song_start, WP-6.51 the \
             two for crossfades: set_crossfade and delete_crossfade, and WP-6.19 \
             seven for grooves: add_groove, delete_groove, rename_groove, set_groove, \
             set_global_groove_amount, set_auto_load_groove and set_clip_groove, WP-6.9 \
             five for tunings: set_tuning, remove_tuning, set_tuning_reference, \
             set_tuning_range and set_tuning_bypass, WP-6.9b the two for tunings on plug-ins and controllers: set_plugin_tuning and set_tuning_layout, WP-6.22 move_clip_to_lane, and WP-6.24b \
             the two for time: insert_time and delete_time, and WP-6.23 set_track_links, and WP-6.30b the five for colours: set_track_colour, assign_colour_to_clips, assign_colour_to_grouped, set_clip_colour and set_scene_colour, and WP-6.57 the two for clearing envelopes: clear_track_automation and clear_device_automation, and WP-6.58 the four for info text: set_track_info_text, set_clip_info_text, set_scene_info_text and set_device_info_text, and WP-7.1 set_remote_mappings, and WP-7.32 set_record_quantization, and WP-7.33 set_device_region, and WP-7.17 set_cv_calibration, and the copy fixes set_automation_enabled"
    );

    for op in &ops {
        let variant = tagged_variant(&enum_schema, "op", op).unwrap();
        // The fan-out commands (WP-6.6) also take `clips` in place of `clip`.
        let fanned = registry::with_clips(&variant);
        // The linked-track commands (WP-6.23) also take `linked`.
        let linked = registry::with_linked(&variant);
        let registered: Vec<&str> = commands
            .iter()
            .filter(|spec| {
                [&variant, &fanned, &linked]
                    .into_iter()
                    .any(|schema| spec.args.as_ref() == Some(schema))
            })
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
    assert!(commands.contains(registry::GO_TO_STEP));
    assert_eq!(commands.len(), ops.len() + 3);
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
    // Spec: mixer R21; routing R20; session-view R12
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
    // Spec: info-view R12.
    agent.run(
        "track.set_info_text",
        json!({ "track": drums, "info_text": "Chopped break" }),
    );
    assert_eq!(
        agent.editor.document().track(drums).unwrap().info_text(),
        "Chopped break"
    );
    // Spec: themes R25.
    agent.run(
        "track.set_colour",
        json!({ "tracks": [drums], "colour": 16 }),
    );
    assert_eq!(agent.editor.document().track(drums).unwrap().colour(), 16);
    agent.run(
        "track.set_frozen",
        json!({ "frozen": [[drums, { "clips": [], "tails": [], "output": null }]] }),
    );
    agent.run("track.clear_frozen", json!({ "tracks": [drums] }));
    let document = agent.editor.document();
    assert_eq!(document.parent(bass), Some(group));
    assert_eq!(document.track(drums).unwrap().name(), "Break");
    assert_eq!(document.track(reverb).unwrap().kind(), TrackKind::Return);

    // Bounces: a track in place, and a new track beside its source.
    let bounced: TrackId = agent.new_id();
    agent.run(
        "track.set_bounced",
        json!({ "track": drums, "bounced": { "name": "Drums (Bounce)" } }),
    );
    agent.run(
        "track.create_bounced_track",
        json!({ "source": bass, "track": bounced, "bounced": { "name": "Bass (Bounce)" } }),
    );
    let document = agent.editor.document();
    assert_eq!(document.track(drums).unwrap().name(), "Drums (Bounce)");
    assert_eq!(document.track(drums).unwrap().kind(), TrackKind::Audio);
    assert_eq!(document.track(bounced).unwrap().name(), "Bass (Bounce)");

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
    agent.run("mixer.set_crossfader", json!({ "position": 0.75 }));
    agent.run(
        "routing.set_cue_out",
        json!({ "channel": { "type": "pair", "channel": 3 } }),
    );
    let changes = agent.run("mixer.set_cue_mode", json!({ "enabled": true }));
    assert!(changes.settings);
    agent.run("mixer.set_cue_volume", json!({ "volume": 0.5 }));
    let changes = agent.run(
        "mixer.set_crossfade_assign",
        json!({ "track": drums, "assign": "a" }),
    );
    agent.run(
        "mixer.set_pan_mode",
        json!({ "track": drums, "mode": "split" }),
    );
    agent.run(
        "mixer.set_split_pan_left",
        json!({ "track": drums, "pan": -1.0 }),
    );
    agent.run(
        "mixer.set_split_pan_right",
        json!({ "track": drums, "pan": 1.0 }),
    );
    agent.run(
        "mixer.set_track_delay",
        json!({ "track": drums, "delay": { "unit": "samples", "value": 480 } }),
    );
    agent.run(
        "mixer.set_reduced_latency_monitoring",
        json!({ "enabled": true }),
    );
    agent.run("mixer.set_delay_compensation", json!({ "enabled": false }));
    agent.run(
        "mixer.set_keep_monitoring_latency",
        json!({ "track": drums, "enabled": false }),
    );
    let document = agent.editor.document();
    assert!(!document.settings().delay_compensation);
    assert!(document.settings().reduced_latency_monitoring);
    assert!(!document.track(drums).unwrap().keep_monitoring_latency());
    assert_eq!(
        document.track(drums).unwrap().delay(),
        TrackDelay::Samples { value: 480 }
    );
    agent.run("mixer.set_delay_compensation", json!({ "enabled": true }));
    assert_eq!(changes.tracks.get(&drums), Some(&Change::Modified));
    let document = agent.editor.document();
    let mixer = *document.track(drums).unwrap().mixer();
    assert_eq!((mixer.volume, mixer.pan, mixer.mute), (0.5, -0.25, true));
    assert_eq!(mixer.crossfade, CrossfadeAssign::A);
    assert_eq!(document.settings().crossfader, 0.75);
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
    let routing = document.track(drums).unwrap().routing().clone();
    assert_eq!(
        routing.input,
        Input::Track {
            track: reverb,
            tap: Tap::PostFx,
            chain: None,
            output: None,
        }
    );
    assert_eq!(routing.monitor, Monitor::In);
    assert_eq!(
        document.track(group).unwrap().routing().output,
        Output::SendsOnly
    );
    assert_eq!(
        document.track(keys).unwrap().routing().midi_output,
        Some(MidiTo::Track(bass))
    );
    // Spec: external-devices R2. A port takes its name and channel, and the
    // choice reads back as the agent gave it.
    agent.run(
        "routing.set_midi_output",
        json!({ "track": keys, "to": { "name": "Synth", "channel": 3 } }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .track(keys)
            .unwrap()
            .routing()
            .midi_output,
        Some(MidiTo::Port {
            name: "Synth".to_owned(),
            channel: 3,
            mpe: None
        })
    );
    agent.run(
        "routing.set_midi_output",
        json!({ "track": keys, "to": bass }),
    );
    // Ext. In with a channel, and Main's Main Out (WP-2.13): the agent API
    // sets both, as an agent recording from an interface would.
    agent.run(
        "routing.set_input",
        json!({ "track": drums, "input": { "type": "external", "channel": { "type": "mono", "channel": 3 } } }),
    );
    agent.run(
        "routing.set_main_out",
        json!({ "channel": { "type": "pair", "channel": 3 } }),
    );
    agent.run(
        "routing.set_cue_out",
        json!({ "channel": { "type": "pair", "channel": 2 } }),
    );
    let document = agent.editor.document();
    assert_eq!(
        document.track(drums).unwrap().routing().input,
        Input::External {
            channel: Channel::Mono(3)
        }
    );
    assert_eq!(
        document
            .track(document.main_id())
            .unwrap()
            .routing()
            .main_out,
        Channel::Pair(3)
    );
    assert_eq!(
        document
            .track(document.main_id())
            .unwrap()
            .routing()
            .cue_out,
        Channel::Pair(2)
    );
    assert_eq!(
        agent.editor.undo_history().next(),
        Some("Change \"Routing\"")
    );

    // Ext. Out with a channel, and without one for the first pair.
    agent.run(
        "routing.set_output",
        json!({ "track": drums, "output": { "type": "external", "channel": { "type": "mono", "channel": 3 } } }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .track(drums)
            .unwrap()
            .routing()
            .output,
        Output::External {
            channel: Channel::Mono(3)
        }
    );
    agent.run(
        "routing.set_output",
        json!({ "track": drums, "output": { "type": "external" } }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .track(drums)
            .unwrap()
            .routing()
            .output,
        Output::External {
            channel: Channel::Pair(1)
        }
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
    // Tempo breakpoints and time-signature markers, one edit each.
    let changes = agent.run(
        "song.set_tempo_breakpoint",
        json!({ "at": bar * 2, "bpm": 100.0 }),
    );
    assert!(changes.settings);
    agent.run(
        "song.move_tempo_breakpoint",
        json!({ "at": bar * 2, "to": bar * 3 }),
    );
    agent.run("song.delete_tempo_breakpoint", json!({ "at": bar * 3 }));
    agent.run(
        "song.set_time_signature_change",
        json!({ "at": bar, "signature": { "numerator": 4, "denominator": 4 } }),
    );
    agent.run(
        "song.move_time_signature_change",
        json!({ "at": bar, "to": bar * 2 }),
    );
    agent.run(
        "song.delete_time_signature_change",
        json!({ "at": bar * 2 }),
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
        "track.add_take_lane",
        json!({ "track": drums, "lane": { "name": "Take 1", "clips": [
            { "name": "Loop", "start": 0, "length": bar,
              "data": { "media": "ab".repeat(32), "start": 0.5, "end": 1.5 } },
        ] } }),
    );
    // Take lanes (WP-6.21): a clip on one, a rename, an audition, a place
    // among the others, and the three ways to delete them.
    let take_lane = agent
        .editor
        .document()
        .track(drums)
        .unwrap()
        .take_lanes()
        .next()
        .unwrap()
        .id;
    let take_clip: ClipId = agent.new_id();
    agent.run(
        "take_lane.create_clip",
        json!({ "lane": take_lane, "clip": {
            "id": take_clip, "name": "Second", "start": bar * 2, "length": bar, "mute": false,
            "data": { "audio": { "media": "ab".repeat(32) } },
        } }),
    );
    agent.run(
        "take_lane.rename",
        json!({ "lane": take_lane, "name": "Verse" }),
    );
    agent.run(
        "take_lane.set_audition",
        json!({ "lane": take_lane, "on": true }),
    );
    assert_eq!(
        agent.editor.document().track(drums).unwrap().audition(),
        Some(take_lane)
    );
    agent.run(
        "track.add_take_lane",
        json!({ "track": drums, "index": 0, "lane": { "name": "Take 2", "clips": [] } }),
    );
    agent.run(
        "take_lane.move",
        json!({ "lanes": [take_lane], "index": 0 }),
    );
    let first = agent
        .editor
        .document()
        .track(drums)
        .unwrap()
        .take_lanes()
        .next()
        .unwrap()
        .id;
    assert_eq!(first, take_lane, "moved to the front");
    agent.run("take_lane.delete_unused", json!({ "tracks": [drums] }));
    assert_eq!(
        agent
            .editor
            .document()
            .track(drums)
            .unwrap()
            .take_lanes()
            .len(),
        0,
        "neither lane plays what the main lane does"
    );
    assert_eq!(
        agent.editor.document().track(drums).unwrap().audition(),
        None
    );
    agent.run(
        "track.add_take_lane",
        json!({ "track": drums, "lane": { "name": "Take 1", "clips": [] } }),
    );
    let extra = agent
        .editor
        .document()
        .track(drums)
        .unwrap()
        .take_lanes()
        .next()
        .unwrap()
        .id;
    agent.run("take_lane.delete", json!({ "lanes": [extra] }));
    agent.run(
        "track.add_take_lane",
        json!({ "track": drums, "lane": { "name": "Take 1", "clips": [] } }),
    );
    agent.run("take_lane.delete_all", json!({ "tracks": [drums] }));
    agent.run(
        "track.add_take_lane",
        json!({ "track": drums, "lane": { "name": "Take 1", "clips": [
            { "name": "Loop", "start": 0, "length": bar,
              "data": { "media": "ab".repeat(32), "start": 0.5, "end": 1.5 } },
        ] } }),
    );
    agent.run(
        "clip.move",
        json!({ "clip": riff, "track": bass, "start": bar * 2 }),
    );
    agent.run("clip.move_by", json!({ "clips": [riff], "by": -quarter }));
    agent.run(
        "clip.move_to_lane",
        json!({ "clip": riff, "track": bass, "start": bar * 2 - quarter }),
    );
    agent.run(
        "clip.set_length",
        json!({ "clip": riff, "length": bar * 2 }),
    );
    agent.run("clip.rename", json!({ "clip": riff, "name": "Riff" }));
    agent.run(
        "clip.set_info_text",
        json!({ "clip": riff, "info_text": "The hook" }),
    );
    assert_eq!(
        agent.editor.document().clip(riff).unwrap().info_text,
        "The hook"
    );
    // Spec: themes R25. The clip took its track's colour as it went in.
    let own = agent.editor.document().track(bass).unwrap().colour();
    assert_eq!(
        agent.editor.document().clip(riff).unwrap().colour,
        Some(own)
    );
    agent.run("clip.set_colour", json!({ "clips": [riff], "colour": 40 }));
    assert_eq!(agent.editor.document().clip(riff).unwrap().colour, Some(40));
    agent.run("track.assign_colour_to_clips", json!({ "tracks": [bass] }));
    assert_eq!(
        agent.editor.document().clip(riff).unwrap().colour,
        Some(own)
    );
    agent.run(
        "track.assign_colour_to_grouped",
        json!({ "tracks": [group] }),
    );
    let inside = agent.editor.document().track(group).unwrap().colour();
    assert_eq!(
        agent.editor.document().track(bass).unwrap().colour(),
        inside
    );
    assert_eq!(
        agent.editor.document().clip(riff).unwrap().colour,
        Some(inside)
    );
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
    agent.run(
        "clip.set_midi_region",
        json!({ "clip": riff, "start": quarter, "end": bar * 2 }),
    );
    agent.run(
        "clip.set_midi_loop",
        json!({ "clip": riff, "start": quarter, "end": bar * 2 }),
    );
    agent.run(
        "clip.set_signature",
        json!({ "clip": riff, "signature": { "numerator": 3, "denominator": 4 } }),
    );
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
    // Spec: warping R14. The Loop switch is refused for an unwarped clip,
    // so an older set's looped clip is made with its data.
    registry::run(
        &mut agent.editor,
        "clip.set_loop_enabled",
        json!({ "clip": loop_clip, "enabled": true }),
    )
    .expect_err("an unwarped clip can't loop");
    let mut looped = match agent
        .editor
        .document()
        .clip(loop_clip)
        .unwrap()
        .data
        .clone()
    {
        ClipData::Audio(audio) => audio,
        ClipData::Midi(_) => panic!("an audio clip"),
    };
    looped.looping = true;
    agent
        .editor
        .apply(Command::SetClipData {
            clip: loop_clip,
            data: ClipData::Audio(looped),
        })
        .unwrap();
    agent.run("clip.set_gain", json!({ "clip": loop_clip, "gain": 0.5 }));
    // Spec: scales R5, R9.
    let changes = agent.run(
        "clip.set_scale",
        json!({ "clips": [loop_clip], "enabled": true, "root": 2, "name": "dorian" }),
    );
    assert!(changes.clips.contains_key(&loop_clip));
    let changes = agent.run(
        "song.set_scale",
        json!({ "enabled": true, "root": 5, "name": "minor" }),
    );
    assert!(changes.settings);
    // Spec: tunings R2, R7, R8, R9, R14, R19.
    let tuning = Tuning::new(
        "19".into(),
        String::new(),
        (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect(),
    )
    .unwrap();
    let changes = agent.run("tuning.set", json!({ "tuning": tuning, "retune": false }));
    assert!(changes.settings);
    agent.run(
        "tuning.set_reference",
        json!({ "octave": 3, "degree": 2, "frequency": 270.0 }),
    );
    agent.run(
        "tuning.set_range",
        json!({ "lowest": { "octave": -1, "degree": 10 } }),
    );
    let changes = agent.run(
        "track.set_tuning_bypass",
        json!({ "track": bass, "bypass": true }),
    );
    assert!(changes.tracks.contains_key(&bass));
    // Spec: tunings R22, R23, R24.
    agent.run(
        "track.set_tuning_layout",
        json!({ "track": bass, "layout": "black_keys_only" }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .track(bass)
            .unwrap()
            .tuning_layout()
            .layout,
        bl_model::LayoutKind::BlackKeysOnly
    );
    // Spec: linked-track editing R16.
    let link: bl_model::LinkGroupId = agent.new_id();
    agent.run(
        "track.set_link_group",
        json!({ "tracks": [drums, bass], "group": link }),
    );
    assert_eq!(agent.editor.document().linked_with(drums), [drums, bass]);
    agent.run("track.set_link_group", json!({ "tracks": [drums, bass] }));
    assert!(agent.editor.document().linked_with(drums).is_empty());
    agent.run("tuning.remove", json!({ "retune": true }));
    agent.run(
        "clip.set_fades",
        json!({ "clip": loop_clip, "fade_in": 0.004, "fade_out": 0.25 }),
    );
    let ClipData::Audio(audio) = agent
        .editor
        .document()
        .clip(loop_clip)
        .unwrap()
        .data
        .clone()
    else {
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
    // Spec: audio-clips R30. A crossfade between two audio clips that meet,
    // made and ended by ID, kept consistent on both clips.
    let next_clip: ClipId = agent.new_id();
    agent.run(
        "clip.create",
        json!({ "track": drums, "clip": {
            "id": next_clip, "name": "Next", "start": bar, "length": bar, "mute": false,
            "data": { "audio": { "media": "cd".repeat(32), "start": 0.5 } },
        } }),
    );
    agent.run(
        "arrangement.set_crossfade",
        json!({ "track": drums, "boundary": bar, "before": 0.01, "after": 0.02, "curve": 0.25 }),
    );
    let fades = |agent: &Agent, clip: ClipId| {
        let ClipData::Audio(audio) = agent.editor.document().clip(clip).unwrap().data.clone()
        else {
            panic!("an audio clip");
        };
        (
            audio.fade_in,
            audio.fade_in_before,
            audio.fade_in_curve,
            audio.fade_out,
            audio.fade_out_after,
            audio.fade_out_curve,
        )
    };
    assert_eq!(
        (fades(&agent, loop_clip).3, fades(&agent, loop_clip).4),
        (0.01, 0.02)
    );
    assert_eq!(fades(&agent, loop_clip).5, 0.25);
    assert_eq!(
        (
            fades(&agent, next_clip).0,
            fades(&agent, next_clip).1,
            fades(&agent, next_clip).2
        ),
        (0.02, 0.01, 0.25)
    );
    registry::run(
        &mut agent.editor,
        "arrangement.set_crossfade",
        json!({ "track": drums, "boundary": bar * 3, "before": 0.01, "after": 0.02 }),
    )
    .expect_err("no clips meet there");
    agent.run(
        "arrangement.delete_crossfade",
        json!({ "track": drums, "boundary": bar }),
    );
    assert_eq!(fades(&agent, next_clip), (0.0, 0.0, 0.0, 0.0, 0.0, 0.0));
    agent.run("clip.delete", json!({ "clip": next_clip }));
    // A null end is the end of the file.
    agent.run(
        "clip.set_region",
        json!({ "clip": loop_clip, "start": 0.0, "end": null }),
    );
    // The warp map (WP-4.1). The clip is looped over seconds 1 to 2 of its
    // file, and switching Warp on starts it at the set's tempo.
    agent.run("clip.set_warp", json!({ "clip": loop_clip, "warp": true }));
    let warped = |agent: &Agent| {
        let ClipData::Audio(audio) = agent
            .editor
            .document()
            .clip(loop_clip)
            .unwrap()
            .data
            .clone()
        else {
            panic!("an audio clip");
        };
        audio
    };
    let audio = warped(&agent);
    assert!(audio.warp);
    let ticks_per_second = audio.tempo.unwrap() * quarter as f64 / 60.0;
    assert_eq!(
        (audio.loop_start, audio.loop_end),
        (
            ticks_per_second.round(),
            Some((2.0 * ticks_per_second).round())
        ),
        "the brace converts to ticks"
    );
    // A warped clip's positions are ticks; seconds are refused, and so is
    // the other way round.
    let refused = |agent: &mut Agent, id: &str, args: Value| {
        registry::run(&mut agent.editor, id, args).expect_err("refused")
    };
    refused(
        &mut agent,
        "clip.set_region",
        json!({ "clip": loop_clip, "start": 0.0, "end": null }),
    );
    agent.run(
        "clip.set_region",
        json!({ "clip": loop_clip, "start": 0, "end": null, "ticks": true }),
    );
    agent.run(
        "clip.set_loop",
        json!({ "clip": loop_clip, "start": quarter, "end": bar, "ticks": true }),
    );
    agent.run(
        "clip.insert_warp_marker",
        json!({ "clips": [loop_clip], "beat": bar }),
    );
    agent.run(
        "clip.insert_warp_marker",
        json!({ "clips": [loop_clip], "time": 1.0 }),
    );
    agent.run(
        "clip.shift_warp_marker",
        json!({ "clips": [loop_clip], "marker": 1, "by": 0.01 }),
    );
    agent.run(
        "clip.move_warp_markers",
        json!({ "clips": [loop_clip], "markers": [2], "by": quarter / 4 }),
    );
    agent.run(
        "clip.set_segment_bpm",
        json!({ "clip": loop_clip, "marker": 0, "bpm": 100.0 }),
    );
    agent.run(
        "clip.scale_tempo",
        json!({ "clip": loop_clip, "factor": 2.0 }),
    );
    assert_eq!(warped(&agent).warp_markers.len(), 3);
    // Transients (WP-4.3): edits by hand, and markers over a range.
    agent.run(
        "clip.insert_transient",
        json!({ "clip": loop_clip, "time": 0.4 }),
    );
    agent.run(
        "clip.move_transient",
        json!({ "clip": loop_clip, "from": 0.4, "to": 0.45 }),
    );
    agent.run(
        "clip.delete_transients",
        json!({ "clip": loop_clip, "times": [0.45, 0.9] }),
    );
    assert_eq!(warped(&agent).transient_edits.removed, [0.9]);
    agent.run("clip.reset_transients", json!({ "clip": loop_clip }));
    assert!(warped(&agent).transient_edits.is_empty());
    agent.run(
        "clip.insert_warp_markers",
        json!({ "clip": loop_clip, "from": 3 * bar, "to": 4 * bar, "transients": [] }),
    );
    assert_eq!(warped(&agent).warp_markers.len(), 5);
    agent.run(
        "clip.delete_warp_markers",
        json!({ "clips": [loop_clip], "markers": [2] }),
    );
    assert_eq!(warped(&agent).warp_markers.len(), 4);
    agent.run(
        "clip.delete_warp_markers",
        json!({ "clips": [loop_clip], "markers": [3, 2] }),
    );
    assert_eq!(warped(&agent).warp_markers.len(), 2);
    // The loop switch works on a warped clip (WP-4.2).
    agent.run(
        "clip.set_loop_enabled",
        json!({ "clip": loop_clip, "enabled": false }),
    );
    agent.run(
        "clip.set_loop_enabled",
        json!({ "clip": loop_clip, "enabled": true }),
    );
    assert!(warped(&agent).looping);
    // The warp mode, its controls, and the RAM and HiQ switches (WP-4.2).
    agent.run(
        "clip.set_warp_mode",
        json!({ "clip": loop_clip, "mode": "re_pitch" }),
    );
    agent.run(
        "clip.set_warp_settings",
        json!({
            "clip": loop_clip,
            "beats": { "preserve": "1_bar", "transient_envelope": 60 },
            "texture": { "flux": 0.0 },
        }),
    );
    agent.run(
        "clip.set_ram_mode",
        json!({ "clip": loop_clip, "ram": true }),
    );
    agent.run(
        "clip.set_high_quality",
        json!({ "clip": loop_clip, "high_quality": false }),
    );
    agent.run(
        "clip.set_pitch",
        json!({ "clip": loop_clip, "transpose": 7, "detune": -25.0 }),
    );
    agent.run(
        "clip.reverse_file",
        json!({ "clip": loop_clip, "seconds": 4.0 }),
    );
    let audio = warped(&agent);
    assert_eq!(audio.warp_mode, WarpMode::RePitch);
    assert_eq!(
        (
            audio.warp_settings.beats.preserve,
            audio.warp_settings.beats.transient_envelope,
            audio.warp_settings.texture.flux,
            audio.warp_settings.texture.grain_size,
        ),
        (Preserve::Bar, 60, 0.0, 65.0)
    );
    assert!(audio.ram && !audio.high_quality);
    refused(
        &mut agent,
        "clip.set_warp_settings",
        json!({ "clip": loop_clip, "tones": { "grain_size": 5.0 } }),
    );
    agent.run("clip.set_warp", json!({ "clip": loop_clip, "warp": false }));
    assert!(!warped(&agent).looping, "unwarping stops the loop");
    let audio = warped(&agent);
    assert!(
        !audio.warp && audio.warp_markers.len() == 2,
        "the map stays"
    );
    // Auto-warp's commands (WP-4.9).
    agent.run("clip.set_warp", json!({ "clip": loop_clip, "warp": true }));
    agent.run(
        "clip.set_1_1_1_here",
        json!({ "clip": loop_clip, "marker": 0 }),
    );
    agent.run(
        "clip.warp_from_here",
        json!({ "clip": loop_clip, "marker": 0, "method": "bpm", "bpm": 100.0 }),
    );
    let file = json!({ "seconds": 4.0, "bpm": null, "beats": [] });
    agent.run(
        "clip.warp_as_loop",
        json!({ "clip": loop_clip, "analysis": file }),
    );
    agent.run(
        "clip.warp_selection_as_loop",
        json!({ "clip": loop_clip, "from": 0.5, "to": 2.5 }),
    );
    assert!(warped(&agent).looping);
    // Tempo leaders, stretching and sliding (WP-4.11).
    agent.run(
        "clip.set_tempo_leader",
        json!({ "clip": loop_clip, "leader": true }),
    );
    assert!(warped(&agent).tempo_leader);
    agent.run(
        "clip.set_tempo_leader",
        json!({ "clip": loop_clip, "leader": false }),
    );
    assert!(!warped(&agent).tempo_leader);
    let end = agent
        .editor
        .document()
        .clip(loop_clip)
        .expect("the clip is there")
        .end();
    agent.run(
        "clip.stretch",
        json!({ "clip": loop_clip, "edge": "right", "at": end.get() + quarter }),
    );
    agent.run(
        "clip.slide_contents",
        json!({ "clip": loop_clip, "by": -quarter }),
    );
    agent.run("song.unfollow_tempo_automation", json!({}));
    agent.run("clip.delete", json!({ "clip": loop_clip }));
    assert!(agent.editor.document().clip(loop_clip).is_none());

    // Session: scenes and the clips in their slots (WP-3.1).
    let scenes: Vec<SceneId> = agent
        .editor
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect();
    let (first, second) = (scenes[0], scenes[1]);
    let slot_riff: ClipId = agent.new_id();
    agent.run(
        "clip.create_in_slot",
        json!({ "track": bass, "scene": first, "clip": {
            "id": slot_riff, "name": "", "start": 0, "length": bar, "mute": false,
            "data": { "midi": { "notes": [
                { "key": 40, "velocity": 100, "start": 0, "length": quarter },
            ] } },
        } }),
    );
    let slot_loop: ClipId = agent.new_id();
    agent.run(
        "clip.create_in_slot",
        json!({ "track": drums, "scene": first, "clip": {
            "id": slot_loop, "name": "Loop", "start": 0, "length": bar, "mute": false,
            "data": { "audio": { "media": "cd".repeat(32) } },
        } }),
    );
    agent.run(
        "clip.move_to_slot",
        json!({ "clip": slot_riff, "track": bass, "scene": second }),
    );
    let document = agent.editor.document();
    assert_eq!(
        document.slot(bass, second).map(|clip| clip.id),
        Some(slot_riff)
    );
    assert!(document.slot(bass, first).is_none());
    assert_eq!(document.clip_track(slot_loop), Some(drums));

    // WP-3.2: a slot's Clip Stop button comes and goes.
    let changes = agent.run(
        "slot.set_stop_button",
        json!({ "track": bass, "scene": second, "stop_button": false }),
    );
    assert_eq!(changes.tracks.get(&bass), Some(&Change::Modified));
    assert!(
        !agent
            .editor
            .document()
            .track(bass)
            .unwrap()
            .has_stop_button(second)
    );
    agent.run(
        "slot.set_stop_button",
        json!({ "track": bass, "scene": second, "stop_button": true }),
    );
    assert!(
        agent
            .editor
            .document()
            .track(bass)
            .unwrap()
            .has_stop_button(second)
    );

    // WP-3.4: a Session clip's launch settings.
    agent.run(
        "clip.set_legato",
        json!({ "clip": slot_riff, "legato": true }),
    );
    agent.run(
        "clip.set_launch_quantization",
        json!({ "clip": slot_riff, "quantization": "eighth" }),
    );
    agent.run(
        "clip.set_velocity_amount",
        json!({ "clip": slot_riff, "amount": 0.5 }),
    );
    agent.run(
        "clip.set_launch_mode",
        json!({ "clip": slot_riff, "mode": "gate" }),
    );
    agent.run(
        "clip.set_follow_actions",
        json!({ "clip": slot_riff, "enabled": true, "action_b": "jump", "jump_b": 99,
            "chance_a": 70 }),
    );
    assert_eq!(
        agent.editor.document().clip(slot_riff).unwrap().launch,
        ClipLaunch {
            legato: true,
            quantization: Some(LaunchQuantization::Eighth),
            velocity_amount: 0.5,
            mode: LaunchMode::Gate,
            follow: FollowActions {
                enabled: true,
                action_b: FollowAction::Jump,
                jump_b: 8,
                chance_a: 70,
                ..FollowActions::default()
            },
        }
    );

    // The scene commands: rename, move, duplicate, insert, delete.
    agent.run("scene.rename", json!({ "scene": first, "name": "Verse" }));
    agent.run(
        "scene.set_info_text",
        json!({ "scene": first, "info_text": "Quiet start" }),
    );
    assert_eq!(
        agent.editor.document().scene(first).unwrap().info_text,
        "Quiet start"
    );
    agent.run(
        "scene.set_colour",
        json!({ "scenes": [first], "colour": 55 }),
    );
    assert_eq!(
        agent.editor.document().scene(first).unwrap().colour,
        Some(55)
    );
    agent.run("scene.set_tempo", json!({ "scene": first, "tempo": 96.5 }));
    agent.run(
        "scene.set_follow_actions",
        json!({ "scene": first, "enabled": true, "linked": false, "time": 2822400 }),
    );
    agent.run(
        "scene.set_time_signature",
        json!({ "scene": first, "signature": { "numerator": 7, "denominator": 8 } }),
    );
    assert_eq!(agent.editor.document().scene(first).unwrap().name, "Verse");
    agent.run("scene.move", json!({ "scene": first, "index": 4 }));
    assert_eq!(agent.editor.document().scene_position(first), Some(4));
    let copy: SceneId = agent.new_id();
    let copy_clip: ClipId = agent.new_id();
    agent.run(
        "scene.duplicate",
        json!({ "scene": first, "id": copy, "clips": [[drums, copy_clip]] }),
    );
    let document = agent.editor.document();
    assert_eq!(document.scene_position(copy), Some(5));
    assert_eq!(
        document.slot(drums, copy).map(|clip| clip.id),
        Some(copy_clip)
    );
    let new_scene: SceneId = agent.new_id();
    agent.run("scene.create", json!({ "id": new_scene, "index": 0 }));
    assert_eq!(agent.editor.document().scene_position(new_scene), Some(0));
    // Deleting the scene takes the clips in its slots with it; the copies
    // stay.
    agent.run("scene.delete", json!({ "scene": second }));
    assert!(agent.editor.document().slot(bass, second).is_none());
    assert!(agent.editor.document().clip(slot_riff).is_none());
    assert!(agent.editor.document().clip(copy_clip).is_some());
    agent.run("clip.delete", json!({ "clip": slot_loop }));
    assert!(agent.editor.document().clip(slot_loop).is_none());

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
        "device.set_info_text",
        json!({ "device": eq, "info_text": "Tames the highs" }),
    );
    assert_eq!(
        agent.editor.document().device(eq).unwrap().info_text,
        "Tames the highs"
    );
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
        "device.set_floating",
        json!({ "device": eq, "floating": true }),
    );
    assert!(agent.editor.document().device(eq).unwrap().floating);
    agent.run(
        "device.set_floating",
        json!({ "device": eq, "floating": false }),
    );
    // Spec: device-chains R2, R3; IMP-24. A native device keeps its type's
    // role: this one is an audio effect already.
    agent.run(
        "device.set_role",
        json!({ "device": eq, "role": "audio_effect" }),
    );
    // Spec: device-compare R11. A switch makes B active, a copy takes the
    // active state over the other.
    agent.run("device.compare_switch", json!({ "device": eq }));
    assert!(agent.editor.document().device(eq).unwrap().b_active);
    assert_eq!(
        agent.editor.undo_history().next(),
        Some("Compare: Switch to B")
    );
    agent.run(
        "device.set_param",
        json!({ "device": eq, "param": 3, "value": 1.5 }),
    );
    agent.run("device.compare_copy", json!({ "device": eq }));
    assert_eq!(
        agent.editor.undo_history().next(),
        Some("Compare: Copy B to A")
    );
    agent.run("device.compare_switch", json!({ "device": eq }));
    assert!(!agent.editor.document().device(eq).unwrap().b_active);
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
    agent.run(
        "device.set_sample",
        json!({ "device": eq, "media": "0303030303030303030303030303030303030303030303030303030303030303" }),
    );
    // Spec: clap-hosting R5; WP-5.40. Replaces a plug-in device's
    // configured parameter list, in order.
    agent.run(
        "device.set_configured_params",
        json!({ "device": synth, "params": [3, 1] }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .device(synth)
            .unwrap()
            .configured
            .as_deref(),
        Some([ParamId(3), ParamId(1)].as_slice())
    );
    assert_eq!(
        agent.editor.undo_history().next(),
        Some("Configure Plug-In")
    );
    let device = agent.editor.document().device(eq).unwrap();
    assert_eq!(agent.editor.document().device_track(eq), Some(reverb));
    assert_eq!((device.name.as_str(), device.enabled), ("Tone", false));
    assert_eq!(device.params.get(&ParamId(3)), Some(&-12.5));
    assert_eq!(device.params.get(&ParamId(1)), Some(&0.25));
    let media = device.media_at(0).expect("the sample it plays");
    assert_eq!(media, ContentHash([3u8; 32]));
    agent.run("device.remove", json!({ "device": synth }));
    assert!(agent.editor.document().device(synth).is_none());

    // WP-5.13: a multisampler's layers.
    let layer = |name: &str| {
        json!({ "id": 1, "name": name, "media": "0505050505050505050505050505050505050505050505050505050505050505",
            "length": 48000, "sample_rate": 48000, "channels": 1,
            "key": { "low": 0, "high": 127 }, "velocity": { "low": 1, "high": 127 },
            "select": { "low": 0, "high": 127 }, "root_key": 60, "volume": 1.0,
            "start": 0, "end": 48000, "loop_start": 0, "loop_end": 48000, "release_loop": 48000 })
    };
    agent.run(
        "device.insert_layers",
        json!({ "device": eq, "index": null, "layers": [layer("Low"), layer("Mid"), layer("High")] }),
    );
    let ids: Vec<u32> = agent
        .editor
        .document()
        .device(eq)
        .unwrap()
        .layers
        .list
        .iter()
        .map(|l| l.id.0)
        .collect();
    assert_eq!(ids, [1, 2, 3], "the device numbers its layers");
    agent.run(
        "device.rename_layer",
        json!({ "device": eq, "layer": 2, "name": "Middle" }),
    );
    agent.run(
        "device.duplicate_layers",
        json!({ "device": eq, "layers": [2] }),
    );
    agent.run(
        "device.move_layers",
        json!({ "device": eq, "layers": [3], "index": 0 }),
    );
    agent.run(
        "device.sort_layers",
        json!({ "device": eq, "by": "name", "descending": false }),
    );
    agent.run(
        "device.set_layer_zone",
        json!({ "device": eq, "kind": "key", "zones": [
            { "layer": 1, "zone": { "low": 0, "high": 40, "fade_high": 8 } },
            { "layer": 2, "zone": { "low": 41, "high": 127 } }] }),
    );
    agent.run(
        "device.update_layers",
        json!({ "device": eq, "layers": [1, 2], "settings": { "root_key": 48, "pan": -10.0, "sustain": "loop" } }),
    );
    agent.run(
        "device.set_layer_media",
        json!({ "device": eq, "layer": 3,
            "media": "0606060606060606060606060606060606060606060606060606060606060606",
            "length": 96000, "sample_rate": 48000, "channels": 2 }),
    );
    agent.run(
        "device.distribute_layers",
        json!({ "device": eq, "layers": [], "how": "equally", "kind": "key" }),
    );
    // WP-5.15: a layer's edits, and a processed copy that holds them.
    agent.run(
        "device.add_sample_edit",
        json!({ "device": eq, "layer": 3, "edit": { "kind": "cut", "range": { "from": 0, "to": 6000 } } }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .device(eq)
            .unwrap()
            .layers
            .get(bl_model::LayerId(3))
            .unwrap()
            .length,
        90_000
    );
    agent.run(
        "device.add_sample_edit",
        json!({ "device": eq, "layer": null, "edit": { "kind": "reverse" } }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .device(eq)
            .unwrap()
            .sample_edits
            .len(),
        1
    );
    agent.run(
        "device.clear_sample_edits",
        json!({ "device": eq, "layer": null, "length": 0 }),
    );
    assert!(
        agent
            .editor
            .document()
            .device(eq)
            .unwrap()
            .sample_edits
            .is_empty()
    );
    agent.run(
        "device.set_processed_layer_sample",
        json!({ "device": eq, "layer": 3,
            "media": "0707070707070707070707070707070707070707070707070707070707070707",
            "length": 90000, "crop": null }),
    );
    agent.run(
        "device.clear_sample_edits",
        json!({ "device": eq, "layer": 3, "length": 96000 }),
    );
    agent.run(
        "device.delete_layers",
        json!({ "device": eq, "layers": [2] }),
    );
    agent.run(
        "device.set_layer_options",
        json!({ "device": eq, "ram_mode": true, "snap": true, "linear_loop_fades": null }),
    );
    // WP-5.15b: the sampler's warp controls and manual slices.
    agent.run("device.set_warp", json!({ "device": eq, "warp": true }));
    agent.run(
        "device.set_warp_mode",
        json!({ "device": eq, "mode": "beats" }),
    );
    agent.run(
        "device.set_warp_settings",
        json!({ "device": eq, "beats": { "preserve": "quarter", "transient_envelope": 50 } }),
    );
    agent.run(
        "device.set_sample_bpm",
        json!({ "device": eq, "bpm": 95.0 }),
    );
    agent.run("device.scale_tempo", json!({ "device": eq, "factor": 2.0 }));
    agent.run("device.add_slice", json!({ "device": eq, "frame": 30_000 }));
    agent.run(
        "device.move_slice",
        json!({ "device": eq, "from": 30_000, "to": 35_000 }),
    );
    agent.run(
        "device.delete_slice",
        json!({ "device": eq, "frame": 35_000 }),
    );
    // WP-7.33: the sampler's Start and End flags.
    agent.run(
        "device.write_sample_region",
        json!({ "device": eq, "region": { "start": 100, "end": 900 } }),
    );
    assert_eq!(
        agent.editor.document().device(eq).unwrap().region,
        Some(bl_model::SampleRegion::Frames {
            start: 100,
            end: 900
        })
    );
    let warp = &agent.editor.document().device(eq).unwrap().warp;
    assert!(warp.warp && warp.mode == WarpMode::Beats && warp.bpm == Some(190.0));
    assert!(
        agent
            .editor
            .document()
            .device(eq)
            .unwrap()
            .slices
            .is_empty()
    );
    let layers = &agent.editor.document().device(eq).unwrap().layers;
    assert_eq!(layers.list.len(), 3);
    assert!(layers.ram_mode && layers.snap && !layers.linear_loop_fades);
    // WP-5.10: a modulator maps a parameter on another track, replaces its
    // mapping, edits its shape and unmaps; deleting the target takes the
    // mapping.
    let wave: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": drums, "index": 0, "device": {
            "id": wave, "name": "Shaper", "enabled": true, "params": [],
            "kind": { "native": { "device_type": "beatlayer.curve_mod" } },
        } }),
    );
    let victim: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": reverb, "index": 0, "device": {
            "id": victim, "name": "Gone", "enabled": true, "params": [],
            "kind": { "native": { "device_type": "beatlayer.eq" } },
        } }),
    );
    agent.run(
        "modulator.map",
        json!({ "device": wave, "slot": 0,
            "target": { "type": "device", "device": victim, "param": 3 } }),
    );
    agent.run(
        "modulator.map",
        json!({ "device": wave, "slot": 1,
            "target": { "type": "mixer", "track": reverb, "param": "volume" } }),
    );
    assert_eq!(
        agent.editor.document().device(wave).unwrap().mappings.len(),
        2
    );
    agent.run(
        "modulator.set_shape",
        json!({ "device": wave, "points": [
            { "x": 0.0, "y": 0.0 }, { "x": 0.5, "y": 1.0, "bend": 0.5 } ] }),
    );
    agent.run("modulator.unmap", json!({ "device": wave, "slot": 1 }));
    assert_eq!(
        agent.editor.document().device(wave).unwrap().mappings.len(),
        1
    );
    agent.run("device.remove", json!({ "device": victim }));
    assert!(
        agent
            .editor
            .document()
            .device(wave)
            .unwrap()
            .mappings
            .is_empty()
    );
    agent.run("device.remove", json!({ "device": wave }));

    // WP-6.47: an Articulation Map's list is edited by command.
    let map: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": keys, "index": 0, "device": {
            "id": map, "name": "Articulation Map", "enabled": true, "params": [],
            "role": "midi_effect",
            "kind": { "native": { "device_type": "beatlayer.articulation_map" } },
        } }),
    );
    agent.run(
        "articulation.add",
        json!({ "device": map, "name": "Staccato" }),
    );
    agent.run(
        "articulation.set",
        json!({ "device": map, "index": 1,
            "trigger": { "kind": "key", "key": 36 },
            "outputs": [{ "kind": "key", "key": 24, "velocity": 100 }],
            "channel": 5 }),
    );
    agent.run(
        "articulation.move",
        json!({ "device": map, "index": 1, "to": 0 }),
    );
    agent.run("articulation.delete", json!({ "device": map, "index": 0 }));
    let list = &agent.editor.document().device(map).unwrap().articulations;
    assert_eq!(list.len(), 1, "the list keeps one articulation");

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
    // A device with a MIDI sidechain input, listening to a MIDI track.
    let hear: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": reverb, "index": 0, "device": {
            "id": hear, "name": "Hear", "enabled": true, "params": [],
            "kind": { "native": { "device_type": "beatlayer.hear" } },
            "midi_sidechain": {},
        } }),
    );
    agent.run(
        "device.set_midi_sidechain_source",
        json!({ "device": hear }),
    );
    // An external device (WP-5.34): routed to a port, then to samples of
    // latency; the choices read back.
    let outboard: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": bass, "index": 0, "device": {
            "id": outboard, "name": "Outboard", "enabled": true, "params": [],
            "kind": { "native": { "device_type": "beatlayer.outboard" } },
            "external": {},
        } }),
    );
    agent.run(
        "device.set_external",
        json!({ "device": outboard, "midi_to": { "type": "port", "name": "Synth" },
            "channel": 5, "audio_from": { "type": "pair", "channel": 3 } }),
    );
    agent.run(
        "device.set_external",
        json!({ "device": outboard, "hardware_latency": { "unit": "samples", "value": 96 } }),
    );
    let external = agent
        .editor
        .document()
        .device(outboard)
        .unwrap()
        .external
        .clone()
        .unwrap();
    assert_eq!(external.port.as_deref(), Some("Synth"));
    assert_eq!(external.channel, 5);
    assert_eq!(external.audio_from, Channel::Pair(3));
    assert_eq!(
        external.hardware_latency,
        HardwareLatency::Samples { value: 96 }
    );
    agent.run(
        "device.set_external",
        json!({ "device": outboard, "midi_to": { "type": "none" } }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .device(outboard)
            .unwrap()
            .external
            .as_ref()
            .unwrap()
            .port,
        None
    );
    // A CV device's choosers and calibration (WP-7.17): the ports given
    // change, a port the device lacks is refused, and the calibration is one
    // undo step that reads back.
    let cv: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": bass, "index": 0, "device": {
            "id": cv, "name": "Voltage", "enabled": true, "params": [],
            "kind": { "native": { "device_type": "beatlayer.voltage" } },
            "external": { "cv_outputs": [{ "name": "Pitch" }, { "name": "Gate" }] },
        } }),
    );
    agent.run(
        "device.set_external",
        json!({ "device": cv, "cv_outputs": [{ "name": "Gate", "channel": 4 }] }),
    );
    let ports = |agent: &Agent| {
        agent
            .editor
            .document()
            .device(cv)
            .unwrap()
            .external
            .as_ref()
            .unwrap()
            .cv_outputs
            .clone()
    };
    assert_eq!(
        ports(&agent).iter().map(|p| p.channel).collect::<Vec<_>>(),
        [None, Some(4)]
    );
    assert!(
        registry::run(
            &mut agent.editor,
            "device.set_external",
            json!({ "device": cv, "cv_outputs": [{ "name": "Nope", "channel": 1 }] }),
        )
        .is_err()
    );
    agent.run(
        "cv.set_calibration",
        json!({ "device": cv, "calibration": {
            "octaves_per_volt": 1.0, "hz_at_zero_volts": 130.0,
            "steps": [{ "volts": 0.0, "hz": 130.0, "error_cents": 0.0 }],
        } }),
    );
    assert!(
        agent
            .editor
            .document()
            .device(cv)
            .unwrap()
            .external
            .as_ref()
            .unwrap()
            .cv_calibration
            .is_some()
    );
    // A line that doesn't rise is refused.
    assert!(
        registry::run(
            &mut agent.editor,
            "cv.set_calibration",
            json!({ "device": cv, "calibration": {
                "octaves_per_volt": 0.0, "hz_at_zero_volts": 130.0, "steps": [],
            } }),
        )
        .is_err()
    );
    // A plug-in's MPE Mode and settings (WP-6.17b): both are one undo step,
    // and the device keeps them.
    let synth: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": bass, "index": 0, "device": {
            "id": synth, "name": "Synth", "enabled": true, "params": [],
            "kind": { "plugin": { "format": "clap", "plugin_id": "com.example.synth" } },
        } }),
    );
    agent.run("plugin.set_mpe", json!({ "device": synth, "on": true }));
    agent.run(
        "plugin.set_mpe_settings",
        json!({ "device": synth, "settings": { "zone": "upper", "channels": 7 } }),
    );
    let mpe = agent.editor.document().device(synth).unwrap().mpe;
    assert!(mpe.on);
    assert_eq!(mpe.settings.zone, bl_model::MpeZone::Upper);
    assert_eq!(mpe.settings.channels, 7);
    // Spec: tunings R20, R24. The tuning switch is on in a new device.
    assert!(mpe.tuning);
    agent.run("plugin.set_tuning", json!({ "device": synth, "on": false }));
    assert!(!agent.editor.document().device(synth).unwrap().mpe.tuning);
    // Racks (WP-5.1): a rack with a chain, a second chain added with a
    // device in it, a device moved in and switched on and off with
    // everything inside, and the chain's controls.
    let rack: DeviceId = agent.new_id();
    let first: ChainId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": reverb, "index": 0, "device": {
            "id": rack, "name": "Audio Effect Rack", "enabled": true, "params": [],
            "kind": { "rack": { "kind": "audio_effect" } },
            "chains": [{ "id": first, "name": "Dry", "devices": [] }],
        } }),
    );
    let second: ChainId = agent.new_id();
    let inner: DeviceId = agent.new_id();
    agent.run(
        "chain.add",
        json!({ "rack": rack, "index": 1, "chain": {
            "id": second, "name": "Wet", "devices": [{
                "id": inner, "name": "Verb", "enabled": true, "params": [],
                "kind": { "native": { "device_type": "beatlayer.verb" } },
            }],
        } }),
    );
    let tail: DeviceId = agent.new_id();
    agent.run(
        "device.insert_in_chain",
        json!({ "chain": first, "index": 0, "device": {
            "id": tail, "name": "Tail", "enabled": true, "params": [],
            "kind": { "native": { "device_type": "beatlayer.tail" } },
        } }),
    );
    agent.run(
        "device.move_to_chain",
        json!({ "device": eq, "chain": second, "index": 1 }),
    );
    assert_eq!(agent.editor.document().chain_track(second), Some(reverb));
    agent.run(
        "device.set_enabled_all",
        json!({ "device": rack, "enabled": false }),
    );
    assert!(
        [rack, tail, inner, eq].iter().all(|d| !agent
            .editor
            .document()
            .device(*d)
            .unwrap()
            .enabled)
    );
    agent.run("chain.rename", json!({ "chain": first, "name": "Direct" }));
    agent.run("chain.set_colour", json!({ "chain": first, "colour": 12 }));
    agent.run(
        "chain.set_info",
        json!({ "chain": first, "info": "no verb" }),
    );
    agent.run("chain.set_gain", json!({ "chain": first, "volume": 0.5 }));
    agent.run("chain.set_pan", json!({ "chain": first, "pan": -0.25 }));
    agent.run(
        "chain.set_enabled",
        json!({ "chain": first, "enabled": false }),
    );
    agent.run("chain.set_solo", json!({ "chain": second, "solo": true }));
    // WP-5.6: a drum rack's chain takes its notes, choke group and sends,
    // and its return chain a place to send its output.
    let drum: DeviceId = agent.editor.new_id();
    let (pad, ret): (ChainId, ChainId) = (agent.editor.new_id(), agent.editor.new_id());
    let midi: TrackId = agent.editor.new_id();
    agent.run(
        "track.create",
        json!({ "id": midi, "kind": "midi", "name": "Drums", "parent": null, "index": 0 }),
    );
    let return_track: TrackId = agent.editor.new_id();
    agent.run(
        "track.create",
        json!({ "id": return_track, "kind": "return", "name": "Reverb", "parent": null, "index": 0 }),
    );
    agent
        .editor
        .apply(Command::InsertDevice {
            track: midi,
            index: 0,
            device: Device::rack(drum, RackKind::Drum)
                .with_chain(Chain::for_pad(pad, "Kick", 36))
                .with_chain(Chain::new_return(ret, "Room")),
        })
        .unwrap();
    agent.run(
        "chain.set_receive",
        json!({ "chain": pad, "receive": { "note": 38 } }),
    );
    agent.run("chain.set_play", json!({ "chain": pad, "play": 62 }));
    agent.run("chain.set_choke", json!({ "chain": pad, "choke": 2 }));
    agent.run(
        "chain.set_send",
        json!({ "chain": pad, "to": ret, "level": 0.5 }),
    );
    agent.run(
        "chain.set_audio_to",
        json!({ "chain": ret, "track": return_track }),
    );
    let kick = agent.editor.document().chain(pad).unwrap();
    assert_eq!(
        (kick.receive, kick.play, kick.choke, kick.send(ret)),
        (bl_model::Receive::Note(38), 62, Some(2), 0.5)
    );
    assert_eq!(
        agent.editor.document().chain(ret).unwrap().audio_to,
        Some(return_track)
    );
    agent.run(
        "chain.set_zone",
        json!({ "chain": first, "kind": "select", "zone": { "low": 1, "high": 9, "fade_high": 3 } }),
    );
    agent.run(
        "chain.move",
        json!({ "chain": second, "rack": rack, "index": 0 }),
    );
    let direct = agent.editor.document().chain(first).unwrap();
    assert_eq!(
        (direct.name.as_str(), direct.colour, direct.info.as_str()),
        ("Direct", Some(12), "no verb")
    );
    assert_eq!((direct.mixer.volume, direct.mixer.pan), (0.5, -0.25));
    assert!(!direct.mixer.enabled);
    assert_eq!(
        direct.zones.select,
        bl_model::Zone {
            low: 1,
            high: 9,
            fade_low: 0,
            fade_high: 3
        }
    );
    let order: Vec<ChainId> = agent
        .editor
        .document()
        .device(rack)
        .unwrap()
        .chains
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, [second, first]);
    // A device moves out of a rack with `device.move`, and the rest of a
    // chain goes with it when it's deleted.
    agent.run(
        "device.move",
        json!({ "device": eq, "track": reverb, "index": 1 }),
    );
    agent.run("chain.remove", json!({ "chain": second }));
    assert!(agent.editor.document().chain(second).is_none());
    assert!(agent.editor.document().device(inner).is_none());
    assert!(agent.editor.document().device(eq).is_some());
    agent.run("device.remove", json!({ "device": rack }));
    assert!(agent.editor.document().device(tail).is_none());
    // Group and Ungroup, as agents run them.
    let wrap: DeviceId = agent.new_id();
    let wrapped: ChainId = agent.new_id();
    agent.run(
        "device.wrap_in_rack",
        json!({ "devices": [eq], "rack": {
            "id": wrap, "name": "Audio Effect Rack", "enabled": true, "params": [],
            "kind": { "rack": { "kind": "audio_effect" } },
            "chains": [{ "id": wrapped, "name": "Tone", "devices": [] }],
        } }),
    );
    let place = agent.editor.document().device_place(eq).unwrap();
    assert_eq!(place.chain, Some(wrapped));
    agent.run("device.unwrap_rack", json!({ "device": wrap }));
    assert_eq!(
        agent.editor.document().device_place(eq).unwrap().chain,
        None
    );
    assert!(agent.editor.document().device(wrap).is_none());

    let sidechain = agent
        .editor
        .document()
        .device(duck)
        .unwrap()
        .sidechain
        .clone()
        .unwrap();
    assert_eq!(
        sidechain,
        bl_model::Sidechain {
            enabled: true,
            input: Input::Track {
                track: drums,
                tap: Tap::PostFx,
                chain: None,
                output: None,
            },
            gain: 2.0,
            mix: 0.5,
            listen: true,
        }
    );

    // Automation, one envelope per parameter (WP-2.12): a mixer control, a
    // send, and a device parameter; the hand move that overrides an
    // envelope; and the two Re-Enable commands.
    let volume = json!({ "type": "mixer", "param": "volume" });
    let send = json!({ "type": "send", "to": reverb });
    let changes = agent.run(
        "automation.set_breakpoint",
        json!({ "track": drums, "target": volume, "at": 0, "value": 1.0 }),
    );
    assert_eq!(changes.tracks.get(&drums), Some(&Change::Modified));
    agent.run(
        "automation.set_breakpoint",
        json!({ "track": drums, "target": volume, "at": bar, "value": 0.25 }),
    );
    agent.run(
        "automation.set_curve",
        json!({ "track": drums, "target": volume, "at": 0, "bend": -0.5 }),
    );
    // WP-6.25: the whole envelope, and the Lock Envelopes switch.
    agent.run(
        "automation.set_envelope",
        json!({ "track": drums, "target": volume, "points": [
            { "at": 0, "value": 1.0, "bend": -0.5 },
            { "at": bar, "value": 0.25, "bend": 0.0 },
        ] }),
    );
    agent.run("song.set_lock_envelopes", json!({ "enabled": true }));
    agent.run(
        "song.set_remote_mappings",
        json!({ "mappings": [{
            "source": { "type": "note", "channel": 1, "key": 60 },
            "target": { "type": "solo", "track": drums },
            "kind": { "type": "switch" }, "min": 0.0, "max": 1.0,
        }] }),
    );
    agent.run(
        "automation.set_breakpoint",
        json!({ "track": group, "target": send, "at": 0, "value": 0.5 }),
    );
    agent.run(
        "automation.set_breakpoint",
        json!({ "track": reverb, "target": { "type": "device", "device": eq, "param": 3 },
                "at": 0, "value": -6.0 }),
    );
    let document = agent.editor.document();
    let automation = document
        .track(drums)
        .unwrap()
        .automation_for(&AutomationTarget::Mixer {
            param: MixerParam::Volume,
        })
        .expect("an envelope");
    assert!(automation.enabled);
    assert_eq!(
        automation.envelope.points().collect::<Vec<_>>(),
        vec![
            bl_model::Breakpoint {
                at: Ticks::ZERO,
                value: 1.0,
                bend: -0.5,
            },
            bl_model::Breakpoint {
                at: Ticks(bar),
                value: 0.25,
                bend: 0.0,
            }
        ]
    );
    assert!(
        document
            .track(reverb)
            .unwrap()
            .automation_for(&AutomationTarget::Device {
                device: eq,
                param: ParamId(3)
            })
            .is_some()
    );
    // A hand move overrides the envelope: the control's value wins until
    // Re-Enable Automation.
    agent.run(
        "mixer.set_volume",
        json!({ "track": drums, "volume": 0.75 }),
    );
    let document = agent.editor.document();
    assert!(document.track(drums).unwrap().automation_overridden());
    assert_eq!(document.track(drums).unwrap().mixer().volume, 0.75);
    agent.run(
        "automation.re_enable_for",
        json!({ "track": drums, "target": volume }),
    );
    assert!(
        !agent
            .editor
            .document()
            .track(drums)
            .unwrap()
            .automation_overridden()
    );
    // Turning an envelope off without a hand move, and on again.
    agent.run(
        "automation.set_enabled",
        json!({ "track": drums, "target": volume, "enabled": false }),
    );
    assert!(
        agent
            .editor
            .document()
            .track(drums)
            .unwrap()
            .automation_overridden()
    );
    agent.run(
        "automation.set_enabled",
        json!({ "track": drums, "target": volume, "enabled": true }),
    );
    assert!(
        !agent
            .editor
            .document()
            .track(drums)
            .unwrap()
            .automation_overridden()
    );
    agent.run("mixer.set_volume", json!({ "track": drums, "volume": 0.5 }));
    agent.run("automation.re_enable", json!({}));
    assert!(
        !agent
            .editor
            .document()
            .track(drums)
            .unwrap()
            .automation_overridden()
    );
    // Moving and deleting breakpoints, and deleting a whole envelope.
    agent.run(
        "automation.move_breakpoint",
        json!({ "track": drums, "target": volume, "at": bar, "to": bar * 2 }),
    );
    agent.run(
        "automation.delete_breakpoint",
        json!({ "track": drums, "target": volume, "at": bar * 2 }),
    );
    agent.run(
        "automation.delete",
        json!({ "track": group, "target": send }),
    );
    assert!(
        agent
            .editor
            .document()
            .track(group)
            .unwrap()
            .automation_for(&AutomationTarget::Send { to: reverb })
            .is_none()
    );
    // Deleting the last breakpoint takes the envelope with it.
    agent.run(
        "automation.delete_breakpoint",
        json!({ "track": drums, "target": volume, "at": 0 }),
    );
    assert!(
        agent
            .editor
            .document()
            .track(drums)
            .unwrap()
            .automation_for(&AutomationTarget::Mixer {
                param: MixerParam::Volume
            })
            .is_none()
    );

    // Clearing a track's or a device's envelopes (automation R28).
    agent.run("automation.clear_device", json!({ "track": drums }));
    agent.run("automation.clear_track", json!({ "track": drums }));

    // Clip envelopes (WP-3.15): a clip carries envelopes for its track's
    // parameters, linked to the clip unless the Linked switch says
    // otherwise.
    agent.run(
        "clip.set_envelope_breakpoint",
        json!({ "clip": riff, "target": volume, "at": 0, "value": 1.0 }),
    );
    agent.run(
        "clip.set_envelope_breakpoint",
        json!({ "clip": riff, "target": volume, "at": bar, "value": 0.25 }),
    );
    agent.run(
        "clip.set_envelope_curve",
        json!({ "clip": riff, "target": volume, "at": 0, "bend": -0.5 }),
    );
    agent.run(
        "clip.set_envelope_linked",
        json!({ "clip": riff, "target": volume, "linked": false }),
    );
    // WP-6.25c: the whole envelope.
    agent.run(
        "clip.set_envelope",
        json!({ "clip": riff, "target": volume, "points": [
            { "at": 0, "value": 1.0, "bend": 0.0 },
            { "at": bar, "value": 0.5, "bend": 0.25 },
        ] }),
    );
    assert!(
        !agent
            .editor
            .document()
            .clip(riff)
            .unwrap()
            .envelope(&AutomationTarget::Mixer {
                param: MixerParam::Volume
            })
            .unwrap()
            .linked
    );
    agent.run(
        "clip.move_envelope_breakpoint",
        json!({ "clip": riff, "target": volume, "at": bar, "to": bar * 2 }),
    );
    agent.run(
        "clip.delete_envelope_breakpoint",
        json!({ "clip": riff, "target": volume, "at": bar * 2 }),
    );
    agent.run(
        "clip.set_envelope_breakpoint",
        json!({ "clip": riff, "target": send, "at": 0, "value": 0.5 }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .clip(riff)
            .unwrap()
            .envelopes()
            .count(),
        2
    );
    agent.run(
        "clip.delete_envelope",
        json!({ "clip": riff, "target": send }),
    );
    assert_eq!(
        agent
            .editor
            .document()
            .clip(riff)
            .unwrap()
            .envelopes()
            .count(),
        1
    );

    // Punch-In and Punch-Out (WP-2.13): saved with the set, off in a new one.
    assert!(!agent.editor.document().settings().punch_in);
    agent.run("transport.set_punch_in", json!({ "enabled": true }));
    agent.run("transport.set_punch_out", json!({ "enabled": true }));
    let settings = agent.editor.document().settings();
    assert!(settings.punch_in && settings.punch_out);

    // Metronome (WP-2.15).
    agent.run(
        "transport.set_metronome_enabled",
        json!({ "enabled": true }),
    );
    assert!(agent.editor.document().settings().metronome.enabled);
    agent.run(
        "transport.set_metronome_count_in",
        json!({ "count_in": "1_bar" }),
    );
    assert_eq!(
        agent.editor.document().settings().metronome.count_in,
        CountIn::Bars1
    );
    agent.run("transport.set_metronome_sound", json!({ "sound": "wood" }));
    assert_eq!(
        agent.editor.document().settings().metronome.sound,
        MetronomeSound::Wood
    );
    agent.run("transport.set_metronome_rhythm", json!({ "rhythm": "1/8" }));
    assert_eq!(
        agent.editor.document().settings().metronome.rhythm,
        MetronomeRhythm::Eighth
    );
    agent.run(
        "transport.set_metronome_only_while_recording",
        json!({ "only_while_recording": true }),
    );
    agent.run("transport.set_midi_overdub", json!({ "enabled": true }));
    assert!(agent.editor.document().settings().midi_overdub);
    agent.run(
        "transport.set_record_quantization",
        json!({ "quantization": "1/8t" }),
    );
    assert_eq!(
        agent.editor.document().settings().record_quantization,
        RecordQuantization::EighthTriplet
    );
    assert!(
        agent
            .editor
            .document()
            .settings()
            .metronome
            .only_while_recording
    );

    // Crop Clip: the region the clip's markers hold, and a span of song
    // time (WP-4.13).
    let song: ClipId = agent.new_id();
    agent.run(
        "clip.create",
        json!({ "track": drums, "clip": {
            "id": song, "name": "Song", "start": bar * 4, "length": bar, "mute": false,
            "data": { "audio": { "media": "cd".repeat(32), "start": 0.5, "end": 1.5 } },
        } }),
    );
    agent.run(
        "clip.crop_region",
        json!({ "clip": song, "media": "ef".repeat(32), "from": 0.5, "to": 1.5 }),
    );
    agent.run(
        "clip.crop_span",
        json!({ "clip": song, "media": "ef".repeat(32), "start": bar * 4, "end": bar * 4 + quarter }),
    );
    let clip = agent.editor.document().clip(song).unwrap();
    assert_eq!((clip.start.0, clip.length.0), (bar * 4, quarter));
    let ClipData::Audio(audio) = &clip.data else {
        panic!("{clip:?}");
    };
    assert_eq!(audio.start, 0.0, "the markers begin at the region's start");

    // Macros: mapping, its range, naming, randomising and the mapped value
    // (WP-5.4).
    let rack: DeviceId = agent.new_id();
    agent.run(
        "device.insert",
        json!({ "track": drums, "index": 0, "device": {
            "id": rack, "name": "Rack", "role": "audio_effect", "enabled": true, "params": [],
            "kind": { "rack": { "kind": "audio_effect" } },
        } }),
    );
    let chain: ChainId = agent.new_id();
    agent.run(
        "chain.add",
        json!({ "rack": rack, "index": 0, "chain": { "id": chain, "name": "Chain", "devices": [] } }),
    );
    let utility: DeviceId = agent.new_id();
    agent.run(
        "device.insert_in_chain",
        json!({ "chain": chain, "index": 0, "device": {
            "id": utility, "name": "Utility", "role": "audio_effect", "enabled": true, "params": [],
            "kind": { "native": { "device_type": "beatlayer.utility" } },
        } }),
    );
    let target = json!({ "type": "device", "device": utility, "param": 6 });
    agent.run(
        "rack.map_macro",
        json!({ "rack": rack, "macro_number": 1, "target": target }),
    );
    agent.run(
        "rack.set_mapping_range",
        json!({ "rack": rack, "target": target, "min": 0.25, "max": 0.75 }),
    );
    agent.run(
        "rack.invert_mapping",
        json!({ "rack": rack, "target": target }),
    );
    agent.run(
        "rack.rename_macro",
        json!({ "rack": rack, "macro_number": 1, "name": "Width" }),
    );
    agent.run(
        "rack.set_macro_colour",
        json!({ "rack": rack, "macro_number": 1, "colour": 2 }),
    );
    agent.run(
        "rack.set_macro_info_text",
        json!({ "rack": rack, "macro_number": 1, "info": "Opens it up" }),
    );
    agent.run(
        "rack.set_macro_randomize_excluded",
        json!({ "rack": rack, "macro_number": 1, "excluded": true }),
    );
    agent.run(
        "rack.set_macro_style",
        json!({ "rack": rack, "macro_number": 1, "style": "stepped", "steps": 4 }),
    );
    agent.run("rack.randomize_macros", json!({ "rack": rack, "seed": 3 }));
    agent.run(
        "rack.set_mapped_value",
        json!({ "target": target, "value": 120.0 }),
    );
    agent.run(
        "rack.unmap_macro",
        json!({ "rack": rack, "target": target }),
    );

    // Macro variations: storing, launching, overwriting, naming, copying and
    // deleting (WP-5.5).
    agent.run("rack.store_variation", json!({ "rack": rack }));
    agent.run(
        "rack.set_macro_variation_excluded",
        json!({ "rack": rack, "macro_number": 2, "excluded": true }),
    );
    agent.run("rack.launch_variation", json!({ "rack": rack, "index": 0 }));
    agent.run(
        "rack.overwrite_variation",
        json!({ "rack": rack, "index": 0 }),
    );
    agent.run(
        "rack.rename_variation",
        json!({ "rack": rack, "index": 0, "name": "Quiet" }),
    );
    agent.run(
        "rack.duplicate_variation",
        json!({ "rack": rack, "index": 0 }),
    );
    agent.run("rack.delete_variation", json!({ "rack": rack, "index": 1 }));
    let varied = agent.editor.document().device(rack).unwrap();
    assert_eq!(varied.macros.variations.len(), 1);
    assert_eq!(varied.macros.variations[0].name, "Quiet");
    assert!(varied.macros.setting(2).variation_excluded);

    // Locators (WP-6.24): adding, renaming, describing, looping to the next
    // one, the song start time, moving, and deleting.
    let intro: LocatorId = agent.new_id();
    agent.run(
        "song.add_locator",
        json!({ "locator": { "id": intro, "position": bar, "name": "Intro" } }),
    );
    let chorus: LocatorId = agent.new_id();
    agent.run(
        "song.add_locator",
        json!({ "locator": { "id": chorus, "position": bar * 2, "name": "Chorus" } }),
    );
    agent.run(
        "song.rename_locator",
        json!({ "locator": intro, "name": "Verse" }),
    );
    agent.run(
        "song.set_locator_info_text",
        json!({ "locator": intro, "info_text": "Opens the song" }),
    );
    agent.run("song.loop_to_next_locator", json!({ "locator": intro }));
    agent.run("song.set_start", json!({ "start": { "locator": intro } }));
    agent.run(
        "song.move_locator",
        json!({ "locator": chorus, "to": bar * 3 }),
    );
    agent.run("song.delete_locator", json!({ "locator": chorus }));

    // Time (WP-6.24b): opening a bar of silence on every track at 1.1.1, then
    // closing a quarter note after it.
    let before = agent.editor.document().clone();
    agent.run("song.insert_time", json!({ "at": 0, "length": bar }));
    assert_ne!(agent.editor.document(), &before);
    agent.run(
        "song.delete_time",
        json!({ "start": bar, "end": bar + quarter }),
    );

    // Grooves (WP-6.19): adding one, naming it, setting its parameters and
    // the Global Amount, giving a clip it, auto-loading it, and deleting it.
    let swing: GrooveId = agent.new_id();
    agent.run(
        "groove.add",
        json!({ "groove": { "id": swing, "name": "Swing", "length": bar, "notes": [
            { "position": 0, "velocity": 127 },
            { "position": quarter / 2 + quarter / 8, "velocity": 90 },
        ] } }),
    );
    agent.run(
        "groove.rename",
        json!({ "groove": swing, "name": "Swing 60" }),
    );
    agent.run(
        "groove.set",
        json!({ "groove": swing, "base": "1/8", "timing": 50.0, "velocity": -25.0 }),
    );
    agent.run("groove.set_global_amount", json!({ "amount": 120.0 }));
    agent.run(
        "clip.set_groove",
        json!({ "clips": [riff], "groove": swing }),
    );
    agent.run(
        "groove.set_auto_load",
        json!({ "enabled": true, "groove": swing }),
    );
    let pool = &agent.editor.document().settings().grooves;
    assert_eq!(pool.pool[0].name, "Swing 60");
    assert_eq!(
        (pool.pool[0].base, pool.pool[0].timing),
        (GrooveBase::Eighth, 50.0)
    );
    assert_eq!(pool.global_amount, 120.0);
    assert_eq!(pool.auto_loaded(), Some(swing));
    assert_eq!(
        agent.editor.document().clip(riff).unwrap().groove,
        Some(swing)
    );
    agent.run("groove.delete", json!({ "groove": swing }));
    let document = agent.editor.document();
    assert!(document.settings().grooves.pool.is_empty());
    assert_eq!(document.clip(riff).unwrap().groove, None);
    assert_eq!(document.settings().grooves.auto_loaded(), None);

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

    // A jump to a row of the history returns the set to that point, and is
    // no step of its own (undo history R6, R7).
    let listed = agent.editor.history().len();
    let end = agent.editor.document().clone();
    agent.run(registry::GO_TO_STEP, json!({ "index": 1 }));
    assert!(agent.editor.document().track(group).is_none());
    assert_eq!(agent.editor.history().len(), listed);
    agent.run(registry::GO_TO_STEP, json!({ "index": listed }));
    assert_eq!(agent.editor.document(), &end);

    // One undo step per edit, labelled as the document labels it.
    let edits = agent.ran.len() - 3;
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
    // Spec: mixer R21; routing R20; session-view R12
    let commands = document_registry();
    let mut ids = bl_core::id::IdAllocator::new();
    let mut fresh = || ids.allocate();
    let track = TrackId(fresh());
    let clip = ClipId(fresh());
    let device = DeviceId(fresh());
    let rack = DeviceId(fresh());
    let chain = ChainId(fresh());
    let fresh_id = fresh();
    let scene = SceneId(fresh());
    let scene_copy = SceneId(fresh());
    let locator = LocatorId(fresh());
    let groove = GrooveId(fresh());
    let notes = MidiData::new([Note::new(127, Ticks::ZERO, Ticks::QUARTER, 1)]);
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
            "song.set_tempo_breakpoint",
            Command::SetTempoBreakpoint {
                at: Ticks::QUARTER * 4,
                bpm: Bpm::new(140.0).unwrap(),
            },
        ),
        (
            "song.move_tempo_breakpoint",
            Command::MoveTempoBreakpoint {
                at: Ticks::QUARTER * 4,
                to: Ticks::QUARTER * 8,
            },
        ),
        (
            "song.delete_tempo_breakpoint",
            Command::DeleteTempoBreakpoint {
                at: Ticks::QUARTER * 4,
            },
        ),
        (
            "song.unfollow_tempo_automation",
            Command::UnfollowTempoAutomation {},
        ),
        (
            "song.set_time_signature_change",
            Command::SetTimeSignatureChange {
                at: Ticks::QUARTER * 4,
                signature: TimeSignature::new(3, 4).unwrap(),
            },
        ),
        (
            "song.move_time_signature_change",
            Command::MoveTimeSignatureChange {
                at: Ticks::QUARTER * 4,
                to: Ticks::QUARTER * 8,
            },
        ),
        (
            "song.delete_time_signature_change",
            Command::DeleteTimeSignatureChange {
                at: Ticks::QUARTER * 4,
            },
        ),
        (
            "song.add_locator",
            Command::AddLocator {
                locator: Locator::new(locator, Ticks::WHOLE, "Intro".to_owned()),
            },
        ),
        (
            "song.rename_locator",
            Command::RenameLocator {
                locator,
                name: "Verse".to_owned(),
            },
        ),
        (
            "song.set_locator_info_text",
            Command::SetLocatorInfoText {
                locator,
                info_text: "Opens the song".to_owned(),
            },
        ),
        (
            "song.move_locator",
            Command::MoveLocator {
                locator,
                to: Ticks::WHOLE * 2,
            },
        ),
        (
            "song.loop_to_next_locator",
            Command::LoopToNextLocator { locator },
        ),
        (
            "song.set_start",
            Command::SetSongStart {
                start: Some(SongStart::Locator(locator)),
            },
        ),
        ("song.delete_locator", Command::DeleteLocator { locator }),
        (
            "song.insert_time",
            Command::InsertTime {
                at: Ticks::WHOLE,
                length: Ticks::QUARTER,
                clips: Vec::new(),
            },
        ),
        (
            "song.delete_time",
            Command::DeleteTime {
                start: Ticks::WHOLE,
                end: Ticks::WHOLE + Ticks::QUARTER,
                clips: Vec::new(),
            },
        ),
        (
            "groove.add",
            Command::AddGroove {
                groove: bl_model::Groove::new(
                    groove,
                    "Swing".to_owned(),
                    Ticks::WHOLE,
                    vec![GrooveNote {
                        position: Ticks::ZERO,
                        velocity: 100,
                    }],
                ),
            },
        ),
        (
            "groove.rename",
            Command::RenameGroove {
                groove,
                name: "Swing 60".to_owned(),
            },
        ),
        (
            "groove.set",
            Command::SetGroove {
                groove,
                base: Some(GrooveBase::Eighth),
                quantize: Some(10.0),
                timing: None,
                random: None,
                velocity: None,
            },
        ),
        (
            "groove.set_global_amount",
            Command::SetGlobalGrooveAmount { amount: 90.0 },
        ),
        (
            "groove.set_auto_load",
            Command::SetAutoLoadGroove {
                enabled: Some(true),
                groove: Some(groove),
            },
        ),
        (
            "clip.set_groove",
            Command::SetClipGroove {
                clips: vec![clip],
                groove: Some(groove),
            },
        ),
        ("groove.delete", Command::DeleteGroove { groove }),
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
            "track.set_info_text",
            Command::SetTrackInfoText {
                track,
                info_text: "Played live".to_owned(),
            },
        ),
        (
            "track.set_frozen",
            Command::FreezeTracks {
                frozen: vec![(track, bl_model::FrozenTrack::default())],
            },
        ),
        (
            "track.set_bounced",
            Command::BounceTrackInPlace {
                track,
                bounced: bl_model::BouncedTrack::default(),
            },
        ),
        (
            "track.create_bounced_track",
            Command::BounceToNewTrack {
                source: track,
                track: TrackId(fresh()),
                bounced: bl_model::BouncedTrack::default(),
            },
        ),
        (
            "track.clear_frozen",
            Command::UnfreezeTracks {
                tracks: vec![track],
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
            "mixer.set_crossfader",
            Command::SetCrossfader { position: 0.25 },
        ),
        ("mixer.set_cue_mode", Command::SetCueMode { enabled: true }),
        (
            "mixer.set_cue_volume",
            Command::SetCueVolume { volume: 0.5 },
        ),
        (
            "mixer.set_crossfade_assign",
            Command::SetCrossfadeAssign {
                track,
                assign: CrossfadeAssign::B,
            },
        ),
        (
            "mixer.set_track_delay",
            Command::SetTrackDelay {
                track,
                delay: TrackDelay::Samples { value: 480 },
            },
        ),
        (
            "mixer.set_pan_mode",
            Command::SetPanMode {
                track,
                mode: PanMode::Split,
            },
        ),
        (
            "mixer.set_split_pan_left",
            Command::SetSplitPanLeft { track, pan: -1.0 },
        ),
        (
            "mixer.set_split_pan_right",
            Command::SetSplitPanRight { track, pan: 1.0 },
        ),
        (
            "mixer.set_delay_compensation",
            Command::SetDelayCompensation { enabled: false },
        ),
        (
            "mixer.set_reduced_latency_monitoring",
            Command::SetReducedLatencyMonitoring { enabled: true },
        ),
        (
            "mixer.set_keep_monitoring_latency",
            Command::SetKeepMonitoringLatency {
                track,
                enabled: false,
            },
        ),
        (
            "automation.set_breakpoint",
            Command::SetAutomationBreakpoint {
                track,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Volume,
                },
                at: Ticks::QUARTER,
                value: 1.0,
            },
        ),
        (
            "automation.set_curve",
            Command::SetAutomationCurve {
                track,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Pan,
                },
                at: Ticks::QUARTER,
                bend: -0.5,
            },
        ),
        (
            "automation.set_envelope",
            Command::SetAutomationEnvelope {
                track,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Pan,
                },
                points: vec![Breakpoint {
                    at: Ticks::QUARTER,
                    value: 0.5,
                    bend: 0.0,
                }],
            },
        ),
        (
            "song.set_lock_envelopes",
            Command::SetLockEnvelopes { enabled: true },
        ),
        (
            "song.set_remote_mappings",
            Command::SetRemoteMappings {
                mappings: Vec::new(),
            },
        ),
        (
            "automation.move_breakpoint",
            Command::MoveAutomationBreakpoint {
                track,
                target: AutomationTarget::Device {
                    device,
                    param: ParamId(7),
                },
                at: Ticks::QUARTER,
                to: Ticks::WHOLE,
                value: Some(0.5),
            },
        ),
        (
            "automation.delete_breakpoint",
            Command::DeleteAutomationBreakpoint {
                track,
                target: AutomationTarget::Send {
                    to: TrackId(fresh()),
                },
                at: Ticks::QUARTER,
            },
        ),
        (
            "automation.delete",
            Command::DeleteAutomation {
                track,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Mute,
                },
            },
        ),
        (
            "automation.clear_track",
            Command::ClearTrackAutomation { track },
        ),
        (
            "automation.clear_device",
            Command::ClearDeviceAutomation {
                track,
                device: None,
            },
        ),
        ("automation.re_enable", Command::ReEnableAutomation {}),
        (
            "automation.set_enabled",
            Command::SetAutomationEnabled {
                track,
                target: AutomationTarget::Send {
                    to: TrackId(fresh()),
                },
                enabled: false,
            },
        ),
        (
            "automation.re_enable_for",
            Command::ReEnableAutomationFor {
                track,
                target: AutomationTarget::Send {
                    to: TrackId(fresh()),
                },
            },
        ),
        (
            "clip.set_envelope_breakpoint",
            Command::SetClipEnvelopeBreakpoint {
                kind: bl_model::EnvelopeKind::Modulation,
                clip,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Volume,
                },
                at: Ticks::QUARTER,
                value: 0.5,
            },
        ),
        (
            "clip.set_envelope_curve",
            Command::SetClipEnvelopeCurve {
                kind: bl_model::EnvelopeKind::Modulation,
                clip,
                target: AutomationTarget::Device {
                    device,
                    param: ParamId(3),
                },
                at: Ticks::QUARTER,
                bend: 0.25,
            },
        ),
        (
            "clip.move_envelope_breakpoint",
            Command::MoveClipEnvelopeBreakpoint {
                kind: bl_model::EnvelopeKind::Modulation,
                clip,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Pan,
                },
                at: Ticks::QUARTER,
                to: Ticks::WHOLE,
                value: None,
            },
        ),
        (
            "clip.delete_envelope_breakpoint",
            Command::DeleteClipEnvelopeBreakpoint {
                kind: bl_model::EnvelopeKind::Modulation,
                clip,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Mute,
                },
                at: Ticks::QUARTER,
            },
        ),
        (
            "clip.delete_envelope",
            Command::DeleteClipEnvelope {
                kind: bl_model::EnvelopeKind::Modulation,
                clip,
                target: AutomationTarget::Send {
                    to: TrackId(fresh()),
                },
            },
        ),
        (
            "clip.set_envelope_linked",
            Command::SetClipEnvelopeLinked {
                kind: bl_model::EnvelopeKind::Modulation,
                clip,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Volume,
                },
                linked: false,
            },
        ),
        (
            "clip.set_envelope",
            Command::SetClipEnvelope {
                kind: bl_model::EnvelopeKind::Modulation,
                clip,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Volume,
                },
                points: vec![Breakpoint {
                    at: Ticks::QUARTER,
                    value: 0.5,
                    bend: 0.0,
                }],
            },
        ),
        (
            "routing.set_input",
            Command::SetInput {
                track,
                input: Input::Track {
                    track: TrackId(fresh()),
                    tap: Tap::PreFx,
                    chain: None,
                    output: None,
                },
                channel: None,
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
            "routing.set_main_out",
            Command::SetMainOut {
                channel: Channel::Pair(3),
            },
        ),
        (
            "routing.set_cue_out",
            Command::SetCueOut {
                channel: Channel::Pair(2),
            },
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
        (
            "track.add_take_lane",
            Command::AddTakeLane {
                track,
                lane: NewTakeLane {
                    id: Some(TakeLaneId(fresh())),
                    name: "Take 1".to_owned(),
                    clips: vec![TakeClip {
                        id: Some(ClipId(fresh())),
                        name: "Loop".to_owned(),
                        start: Ticks::WHOLE,
                        length: Ticks::WHOLE,
                        data: TakeClipData::Audio(AudioData::new(ContentHash([5; 32]))),
                    }],
                },
                index: Some(0),
            },
        ),
        (
            "take_lane.create_clip",
            Command::CreateTakeClip {
                lane: TakeLaneId(fresh()),
                clip: Clip::new(
                    ClipId(fresh()),
                    Ticks::WHOLE,
                    Ticks::WHOLE,
                    ClipData::Midi(notes.clone()),
                ),
            },
        ),
        (
            "take_lane.rename",
            Command::RenameTakeLane {
                lane: TakeLaneId(fresh()),
                name: "Verse".to_owned(),
            },
        ),
        (
            "take_lane.delete",
            Command::DeleteTakeLanes {
                lanes: vec![TakeLaneId(fresh())],
            },
        ),
        (
            "take_lane.delete_all",
            Command::DeleteAllTakeLanes {
                tracks: vec![track],
            },
        ),
        (
            "take_lane.delete_unused",
            Command::DeleteUnusedTakeLanes {
                tracks: vec![track],
            },
        ),
        (
            "take_lane.move",
            Command::MoveTakeLanes {
                lanes: vec![TakeLaneId(fresh())],
                index: 1,
            },
        ),
        (
            "take_lane.set_audition",
            Command::SetAudition {
                lane: TakeLaneId(fresh()),
                on: true,
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
            "clip.move_to_lane",
            Command::MoveClipToLane {
                clip,
                track,
                lane: None,
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
            "clip.set_info_text",
            Command::SetClipInfoText {
                clip,
                info_text: "Hook".to_owned(),
            },
        ),
        (
            "clip.set_launch_mode",
            Command::SetClipLaunchMode {
                clip,
                mode: LaunchMode::Gate,
            },
        ),
        (
            "clip.crop_region",
            Command::CropClip {
                clip,
                media: Some(ContentHash([0xcd; 32])),
                from: 0.5,
                to: 1.5,
            },
        ),
        (
            "clip.crop_span",
            Command::CropClipToRange {
                clip,
                media: Some(ContentHash([0xcd; 32])),
                start: Ticks::QUARTER,
                end: Ticks::QUARTER * 3,
            },
        ),
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
                ticks: false,
            },
        ),
        (
            "clip.set_loop",
            Command::SetClipLoop {
                clip,
                start: 0.5,
                end: None,
                ticks: false,
            },
        ),
        (
            "clip.set_loop_enabled",
            Command::SetClipLoopEnabled {
                clip,
                enabled: true,
            },
        ),
        (
            "clip.set_midi_region",
            Command::SetMidiClipRegion {
                clip,
                start: Ticks::QUARTER,
                end: Some(Ticks::WHOLE),
            },
        ),
        (
            "clip.set_midi_loop",
            Command::SetMidiClipLoop {
                clip,
                start: Ticks::QUARTER,
                end: None,
            },
        ),
        (
            "clip.set_signature",
            Command::SetClipSignature {
                clip,
                signature: None,
            },
        ),
        ("clip.set_gain", Command::SetClipGain { clip, gain: 2.0 }),
        (
            "clip.set_fades",
            Command::SetClipFades {
                clip,
                fade_in: 0.004,
                fade_out: 1.5,
                fade_in_curve: None,
                fade_out_curve: None,
            },
        ),
        ("clip.set_warp", Command::SetClipWarp { clip, warp: true }),
        (
            "clip.set_warp_mode",
            Command::SetClipWarpMode {
                clip,
                mode: WarpMode::ComplexPro,
            },
        ),
        (
            "clip.set_warp_settings",
            Command::SetClipWarpSettings {
                clip,
                beats: Some(BeatsUpdate {
                    preserve: Some(Preserve::Sixteenth),
                    transient_loop_mode: None,
                    transient_envelope: Some(50),
                }),
                tones: Some(TonesUpdate {
                    grain_size: Some(40.0),
                }),
                texture: None,
                complex_pro: Some(ComplexProUpdate {
                    formants: Some(25.0),
                    envelope: Some(256),
                }),
            },
        ),
        (
            "clip.set_ram_mode",
            Command::SetClipRamMode { clip, ram: true },
        ),
        (
            "clip.set_high_quality",
            Command::SetClipHighQuality {
                clip,
                high_quality: false,
            },
        ),
        (
            "clip.set_pitch",
            Command::SetClipPitch {
                clip,
                transpose: Some(7),
                detune: Some(-25.0),
            },
        ),
        (
            "clip.reverse_file",
            Command::ReverseClip {
                clip,
                media: None,
                seconds: 4.0,
            },
        ),
        (
            "clip.set_tempo_leader",
            Command::SetTempoLeader { clip, leader: true },
        ),
        (
            "clip.stretch",
            Command::Stretch {
                clip,
                edge: ClipEdge::Right,
                at: Ticks::QUARTER * 8,
            },
        ),
        (
            "clip.slide_contents",
            Command::SlideContents {
                clip,
                by: Ticks::SIXTEENTH,
            },
        ),
        (
            "clip.insert_warp_marker",
            Command::InsertWarpMarker {
                clips: vec![clip],
                beat: Some(Ticks::QUARTER),
                time: None,
                pin_neighbours: false,
                transients: None,
            },
        ),
        (
            "clip.move_warp_markers",
            Command::MoveWarpMarkers {
                clips: vec![clip],
                markers: vec![0, 1],
                by: Ticks::SIXTEENTH,
            },
        ),
        (
            "clip.shift_warp_marker",
            Command::ShiftWarpMarker {
                clips: vec![clip],
                marker: 1,
                by: -0.25,
            },
        ),
        (
            "clip.delete_warp_markers",
            Command::DeleteWarpMarkers {
                clips: vec![clip],
                markers: vec![1],
            },
        ),
        (
            "clip.insert_warp_markers",
            Command::InsertWarpMarkers {
                clip,
                from: Ticks::QUARTER,
                to: Ticks::WHOLE,
                transients: Some(vec![0.5, 1.0]),
            },
        ),
        (
            "clip.insert_transient",
            Command::InsertTransient { clip, time: 0.5 },
        ),
        (
            "clip.delete_transients",
            Command::DeleteTransients {
                clip,
                times: vec![0.5],
            },
        ),
        ("clip.reset_transients", Command::ResetTransients { clip }),
        (
            "clip.move_transient",
            Command::MoveTransient {
                clip,
                from: 0.5,
                to: 0.6,
            },
        ),
        (
            "clip.set_segment_bpm",
            Command::SetSegmentBpm {
                clip,
                marker: Some(0),
                bpm: 90.0,
            },
        ),
        (
            "clip.scale_tempo",
            Command::ScaleTempo { clip, factor: 0.5 },
        ),
        (
            "clip.set_1_1_1_here",
            Command::SetOneOneOneHere {
                clip,
                at: WarpPoint {
                    marker: Some(0),
                    ..WarpPoint::default()
                },
            },
        ),
        (
            "clip.warp_from_here",
            Command::WarpFromHere {
                clip,
                at: WarpPoint {
                    beat: Some(Ticks::WHOLE),
                    ..WarpPoint::default()
                },
                method: WarpFrom::StartAt,
                bpm: Some(100.0),
                analysis: Some(FileBeats {
                    seconds: 4.0,
                    bpm: Some(120.0),
                    beats: vec![0.0, 0.5, 1.0],
                }),
            },
        ),
        (
            "clip.warp_as_loop",
            Command::WarpAsLoop {
                clip,
                analysis: None,
            },
        ),
        (
            "clip.warp_selection_as_loop",
            Command::WarpSelectionAsLoop {
                clip,
                from: 0.5,
                to: 2.5,
            },
        ),
        (
            "clip.set_legato",
            Command::SetClipLegato { clip, legato: true },
        ),
        (
            "clip.set_follow_actions",
            Command::SetClipFollowActions {
                clip,
                patch: FollowPatch {
                    action_a: Some(FollowAction::Stop),
                    chance_a: Some(25),
                    ..FollowPatch::default()
                },
            },
        ),
        (
            "clip.set_launch_quantization",
            Command::SetClipLaunchQuantization {
                clip,
                quantization: Some(LaunchQuantization::Quarter),
            },
        ),
        (
            "clip.set_velocity_amount",
            Command::SetClipVelocityAmount { clip, amount: 0.25 },
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
            "device.set_info_text",
            Command::SetDeviceInfoText {
                device,
                info_text: "Warm".to_owned(),
            },
        ),
        (
            "device.set_show_preset_name",
            Command::SetShowPresetName {
                device,
                shown: false,
            },
        ),
        ("device.compare_switch", Command::CompareSwitch { device }),
        ("device.compare_copy", Command::CompareCopy { device }),
        (
            "device.set_floating",
            Command::SetDeviceFloating {
                device,
                floating: true,
            },
        ),
        (
            "device.set_role",
            Command::SetDeviceRole {
                device,
                role: Some(DeviceRole::MidiEffect),
            },
        ),
        (
            "device.set_state",
            Command::SetDeviceState {
                articulations: None,
                device,
                params: [(ParamId(4), 0.5)].into_iter().collect(),
                name: "Warm Pad".to_owned(),
                shape: None,
                mpe: None,
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
            "device.set_configured_params",
            Command::SetConfiguredParams {
                device,
                params: vec![ParamId(7), ParamId(3)],
            },
        ),
        (
            "device.set_sample",
            Command::SetDeviceSample {
                device,
                media: Some(ContentHash([3u8; 32])),
                slot: 0,
            },
        ),
        (
            "device.insert_layers",
            Command::InsertLayers {
                device,
                index: Some(0),
                layers: vec![bl_model::Layer::new(
                    bl_model::LayerId(1),
                    "Kick",
                    ContentHash([4u8; 32]),
                    48_000,
                    48_000,
                    2,
                )],
            },
        ),
        (
            "device.delete_layers",
            Command::DeleteLayers {
                device,
                layers: vec![bl_model::LayerId(1)],
            },
        ),
        (
            "device.duplicate_layers",
            Command::DuplicateLayers {
                device,
                layers: vec![bl_model::LayerId(1)],
            },
        ),
        (
            "device.rename_layer",
            Command::RenameLayer {
                device,
                layer: bl_model::LayerId(1),
                name: "Snare".to_owned(),
            },
        ),
        (
            "device.move_layers",
            Command::MoveLayers {
                device,
                layers: vec![bl_model::LayerId(1)],
                index: 2,
            },
        ),
        (
            "device.sort_layers",
            Command::SortLayers {
                device,
                by: bl_model::LayerSort::Key,
                descending: true,
            },
        ),
        (
            "device.set_layer_zone",
            Command::SetLayerZones {
                device,
                kind: bl_model::ZoneKind::Velocity,
                zones: vec![bl_model::LayerZone {
                    layer: bl_model::LayerId(1),
                    zone: bl_model::Zone {
                        low: 1,
                        high: 100,
                        fade_low: 3,
                        fade_high: 0,
                    },
                }],
            },
        ),
        (
            "device.update_layers",
            Command::UpdateLayers {
                device,
                layers: vec![bl_model::LayerId(1)],
                settings: bl_model::LayerSettings {
                    root_key: Some(48),
                    sustain: Some(bl_model::SustainMode::BackAndForth),
                    ..Default::default()
                },
            },
        ),
        (
            "modulator.map",
            Command::MapModulator {
                device,
                slot: 3,
                target: bl_model::modulator::ModTarget::Device {
                    device,
                    param: ParamId(2),
                },
            },
        ),
        (
            "device.set_layer_media",
            Command::SetLayerMedia {
                device,
                layer: bl_model::LayerId(1),
                media: ContentHash([5u8; 32]),
                length: 1000,
                sample_rate: 44_100,
                channels: 1,
            },
        ),
        (
            "device.distribute_layers",
            Command::DistributeLayers {
                device,
                layers: vec![],
                how: bl_model::Distribution::AroundRootKey,
                kind: bl_model::ZoneKind::Key,
            },
        ),
        (
            "device.add_sample_edit",
            Command::AddSampleEdit {
                device,
                layer: None,
                edit: bl_model::SampleEdit {
                    kind: bl_model::SampleEditKind::FadeOut,
                    range: Some(bl_model::SampleRange { from: 10, to: 20 }),
                },
            },
        ),
        (
            "device.clear_sample_edits",
            Command::ClearSampleEdits {
                device,
                layer: Some(bl_model::LayerId(1)),
                length: 1000,
            },
        ),
        // WP-5.15b: the sampler's warp controls and manual slices.
        (
            "device.set_warp",
            Command::SetDeviceWarp { device, warp: true },
        ),
        (
            "device.set_warp_mode",
            Command::SetDeviceWarpMode {
                device,
                mode: WarpMode::Beats,
            },
        ),
        (
            "device.set_warp_settings",
            Command::SetDeviceWarpSettings {
                device,
                beats: Some(BeatsUpdate {
                    preserve: Some(Preserve::Quarter),
                    transient_loop_mode: None,
                    transient_envelope: Some(50),
                }),
                tones: Some(TonesUpdate {
                    grain_size: Some(40.0),
                }),
                texture: None,
                complex_pro: Some(ComplexProUpdate {
                    formants: Some(50.0),
                    envelope: Some(64),
                }),
            },
        ),
        (
            "device.set_sample_bpm",
            Command::SetDeviceSampleBpm { device, bpm: 95.0 },
        ),
        (
            "device.scale_tempo",
            Command::ScaleDeviceTempo {
                device,
                factor: 2.0,
            },
        ),
        (
            "device.add_slice",
            Command::AddSlice {
                device,
                frame: 30_000,
            },
        ),
        (
            "device.move_slice",
            Command::MoveSlice {
                device,
                from: 30_000,
                to: 35_000,
            },
        ),
        (
            "device.delete_slice",
            Command::DeleteSlice {
                device,
                frame: 35_000,
            },
        ),
        (
            "device.write_sample_region",
            Command::SetDeviceRegion {
                device,
                region: Some(bl_model::SampleRegion::Frames {
                    start: 100,
                    end: 900,
                }),
            },
        ),
        (
            "device.set_processed_layer_sample",
            Command::SetProcessedLayerSample {
                device,
                layer: bl_model::LayerId(1),
                media: ContentHash([6u8; 32]),
                length: 900,
                crop: Some(bl_model::SampleRange {
                    from: 100,
                    to: 1000,
                }),
            },
        ),
        (
            "device.set_layer_options",
            Command::SetLayerOptions {
                device,
                ram_mode: Some(true),
                snap: None,
                linear_loop_fades: Some(true),
            },
        ),
        (
            "modulator.unmap",
            Command::UnmapModulator { device, slot: 3 },
        ),
        (
            "articulation.add",
            Command::AddArticulation {
                device,
                index: None,
                name: "Staccato".to_owned(),
            },
        ),
        (
            "articulation.delete",
            Command::DeleteArticulation { device, index: 0 },
        ),
        (
            "articulation.move",
            Command::MoveArticulation {
                device,
                index: 0,
                to: Some(1),
            },
        ),
        (
            "articulation.set",
            Command::SetArticulation {
                device,
                index: 0,
                name: Some("Legato".to_owned()),
                trigger: Some(bl_model::ArticulationTrigger::Key { key: 36 }),
                outputs: Some(vec![bl_model::ArticulationOutput::Key {
                    key: 24,
                    velocity: 100,
                }]),
                channel: Some(Some(5)),
            },
        ),
        (
            "modulator.set_shape",
            Command::SetShape {
                device,
                points: bl_model::modulator::shape_preset(3).unwrap(),
            },
        ),
        (
            "device.set_sidechain_input",
            Command::SetSidechainInput {
                device,
                input: Input::Track {
                    track,
                    tap: Tap::PostMixer,
                    chain: None,
                    output: None,
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
        (
            "device.set_midi_sidechain_source",
            Command::SetMidiSidechainSource {
                device,
                track: None,
                tap: None,
            },
        ),
        (
            "plugin.set_mpe",
            Command::SetPluginMpeMode { device, on: true },
        ),
        (
            "plugin.set_tuning",
            Command::SetPluginTuning { device, on: false },
        ),
        (
            "plugin.set_mpe_settings",
            Command::SetPluginMpeSettings {
                device,
                settings: bl_model::MpeSettings {
                    zone: bl_model::MpeZone::Upper,
                    channels: 7,
                    ..bl_model::MpeSettings::default()
                },
            },
        ),
        (
            "device.set_external",
            Command::SetExternal {
                device,
                audio_to: None,
                midi_to: Some(ExternalPort::Port {
                    name: "Synth".to_owned(),
                }),
                channel: Some(2),
                mpe: None,
                audio_from: Some(Channel::Pair(3)),
                hardware_latency: Some(HardwareLatency::Samples { value: 96 }),
                cv_outputs: None,
                cv_inputs: None,
            },
        ),
        (
            "cv.set_calibration",
            Command::SetCvCalibration {
                device,
                calibration: bl_model::CvCalibration::fit(&[(0.0, 130.0), (1.0, 260.0)]),
            },
        ),
        (
            "device.insert_in_chain",
            Command::InsertChainDevice {
                chain: ChainId(fresh_id),
                index: 1,
                device: Device::new(
                    DeviceId(fresh_id),
                    "Verb",
                    DeviceKind::Native {
                        device_type: "beatlayer.verb".to_owned(),
                    },
                ),
            },
        ),
        (
            "device.move_to_chain",
            Command::MoveDeviceToChain {
                device,
                chain: ChainId(fresh_id),
                index: 1,
            },
        ),
        (
            "device.set_enabled_all",
            Command::SetDeviceEnabledAll {
                device,
                enabled: false,
            },
        ),
        (
            "device.wrap_in_rack",
            Command::GroupDevices {
                devices: vec![device],
                rack: Device::rack(DeviceId(fresh_id), bl_model::RackKind::AudioEffect)
                    .with_chain(Chain::new(ChainId(fresh_id), "Chain")),
            },
        ),
        ("device.unwrap_rack", Command::UngroupDevice { device }),
        (
            "chain.add",
            Command::AddChain {
                rack: device,
                index: 1,
                chain: Chain::new(ChainId(fresh_id), "Wet"),
            },
        ),
        (
            "chain.remove",
            Command::RemoveChain {
                chain: ChainId(fresh_id),
            },
        ),
        (
            "chain.move",
            Command::MoveChain {
                chain: ChainId(fresh_id),
                rack: device,
                index: 1,
            },
        ),
        (
            "chain.rename",
            Command::RenameChain {
                chain: ChainId(fresh_id),
                name: "Wet".to_owned(),
            },
        ),
        (
            "chain.set_colour",
            Command::SetChainColour {
                chain: ChainId(fresh_id),
                colour: Some(3),
            },
        ),
        (
            "track.set_colour",
            Command::SetTrackColour {
                tracks: vec![track],
                colour: 3,
            },
        ),
        (
            "track.assign_colour_to_clips",
            Command::AssignColourToClips {
                tracks: vec![track],
            },
        ),
        (
            "track.assign_colour_to_grouped",
            Command::AssignColourToGrouped {
                tracks: vec![track],
            },
        ),
        (
            "clip.set_colour",
            Command::SetClipColour {
                clips: vec![clip],
                colour: 3,
            },
        ),
        (
            "scene.set_colour",
            Command::SetSceneColour {
                scenes: vec![scene],
                colour: Some(3),
            },
        ),
        (
            "chain.set_info",
            Command::SetChainInfo {
                chain: ChainId(fresh_id),
                info: "sums with the dry chain".to_owned(),
            },
        ),
        (
            "chain.set_gain",
            Command::SetChainVolume {
                chain: ChainId(fresh_id),
                volume: 0.5,
            },
        ),
        (
            "chain.set_pan",
            Command::SetChainPan {
                chain: ChainId(fresh_id),
                pan: -0.5,
            },
        ),
        (
            "chain.set_enabled",
            Command::SetChainEnabled {
                chain: ChainId(fresh_id),
                enabled: false,
            },
        ),
        (
            "chain.set_solo",
            Command::SetChainSolo {
                chain: ChainId(fresh_id),
                solo: true,
            },
        ),
        (
            "chain.set_zone",
            Command::SetChainZone {
                chain: ChainId(fresh_id),
                kind: bl_model::ZoneKind::Select,
                zone: bl_model::Zone {
                    low: 0,
                    high: 10,
                    fade_low: 0,
                    fade_high: 4,
                },
            },
        ),
        (
            "chain.set_receive",
            Command::SetChainReceive {
                chain: ChainId(fresh_id),
                receive: bl_model::Receive::Note(38),
            },
        ),
        (
            "chain.set_play",
            Command::SetChainPlay {
                chain: ChainId(fresh_id),
                play: 62,
            },
        ),
        (
            "chain.set_choke",
            Command::SetChainChoke {
                chain: ChainId(fresh_id),
                choke: Some(2),
            },
        ),
        (
            "chain.set_send",
            Command::SetChainSend {
                chain: ChainId(fresh_id),
                to: ChainId(fresh_id),
                level: 0.5,
            },
        ),
        (
            "chain.set_audio_to",
            Command::SetChainAudioTo {
                chain: ChainId(fresh_id),
                track: Some(TrackId(fresh_id)),
            },
        ),
        (
            "transport.set_punch_in",
            Command::SetPunchIn { enabled: true },
        ),
        (
            "transport.set_punch_out",
            Command::SetPunchOut { enabled: true },
        ),
        (
            "transport.set_metronome_enabled",
            Command::SetMetronomeEnabled { enabled: true },
        ),
        (
            "transport.set_metronome_count_in",
            Command::SetMetronomeCountIn {
                count_in: CountIn::Bars2,
            },
        ),
        (
            "transport.set_metronome_sound",
            Command::SetMetronomeSound {
                sound: MetronomeSound::Click,
            },
        ),
        (
            "transport.set_metronome_rhythm",
            Command::SetMetronomeRhythm {
                rhythm: MetronomeRhythm::Sixteenth,
            },
        ),
        (
            "transport.set_metronome_only_while_recording",
            Command::SetMetronomeOnlyWhileRecording {
                only_while_recording: true,
            },
        ),
        (
            "transport.set_midi_overdub",
            Command::SetMidiOverdub { enabled: true },
        ),
        (
            "transport.set_record_quantization",
            Command::SetRecordQuantization {
                quantization: Some(RecordQuantization::Eighth),
                on: None,
            },
        ),
        (
            "clip.create_in_slot",
            Command::CreateSlotClip {
                track,
                scene,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(notes.clone()),
                ),
            },
        ),
        (
            "clip.move_to_slot",
            Command::MoveSlotClip { clip, track, scene },
        ),
        (
            "slot.set_stop_button",
            Command::SetSlotStopButton {
                track,
                scene,
                stop_button: false,
            },
        ),
        (
            "scene.create",
            Command::CreateScene {
                id: scene,
                index: 1,
            },
        ),
        ("scene.delete", Command::DeleteScene { scene }),
        ("scene.move", Command::MoveScene { scene, index: 0 }),
        (
            "scene.duplicate",
            Command::DuplicateScene {
                scene,
                id: scene_copy,
                clips: vec![(track, clip)],
            },
        ),
        (
            "scene.rename",
            Command::RenameScene {
                scene,
                name: "Verse".to_owned(),
            },
        ),
        (
            "scene.set_info_text",
            Command::SetSceneInfoText {
                scene,
                info_text: "Verse".to_owned(),
            },
        ),
        (
            "scene.set_tempo",
            Command::SetSceneTempo {
                scene,
                tempo: Some(Bpm::new(90.0).unwrap()),
            },
        ),
        (
            "scene.set_follow_actions",
            Command::SetSceneFollowActions {
                scene,
                patch: FollowPatch {
                    enabled: Some(true),
                    time: Some(Ticks::WHOLE),
                    ..FollowPatch::default()
                },
            },
        ),
        (
            "scene.set_time_signature",
            Command::SetSceneTimeSignature {
                scene,
                signature: Some(TimeSignature::new(3, 4).unwrap()),
            },
        ),
        (
            "rack.map_macro",
            Command::MapMacro {
                rack,
                macro_number: 1,
                target: MacroTarget::Device {
                    device,
                    param: ParamId(7),
                },
            },
        ),
        (
            "rack.unmap_macro",
            Command::UnmapMacro {
                rack,
                target: MacroTarget::Chain {
                    chain,
                    param: MixerParam::Volume,
                },
            },
        ),
        (
            "rack.set_mapping_range",
            Command::SetMappingRange {
                rack,
                target: MacroTarget::Device {
                    device,
                    param: ParamId(7),
                },
                min: 0.25,
                max: 0.75,
            },
        ),
        (
            "rack.invert_mapping",
            Command::InvertMapping {
                rack,
                target: MacroTarget::Device {
                    device,
                    param: ParamId(7),
                },
            },
        ),
        (
            "rack.rename_macro",
            Command::RenameMacro {
                rack,
                macro_number: 1,
                name: "Width".to_owned(),
            },
        ),
        (
            "rack.set_macro_colour",
            Command::SetMacroColour {
                rack,
                macro_number: 1,
                colour: Some(2),
            },
        ),
        (
            "rack.set_macro_info_text",
            Command::SetMacroInfo {
                rack,
                macro_number: 1,
                info: "Opens it up".to_owned(),
            },
        ),
        (
            "rack.randomize_macros",
            Command::RandomizeMacros {
                rack,
                seed: Some(7),
            },
        ),
        (
            "rack.set_macro_randomize_excluded",
            Command::SetMacroRandomizeExcluded {
                rack,
                macro_number: 1,
                excluded: true,
            },
        ),
        (
            "rack.set_macro_style",
            Command::SetMacroStyle {
                rack,
                macro_number: 1,
                style: MacroStyle::Stepped,
                steps: Some(4),
            },
        ),
        (
            "rack.set_mapped_value",
            Command::SetMappedValue {
                target: MacroTarget::Device {
                    device,
                    param: ParamId(7),
                },
                value: 1.0,
            },
        ),
        ("rack.store_variation", Command::StoreVariation { rack }),
        (
            "rack.launch_variation",
            Command::LaunchVariation { rack, index: 0 },
        ),
        (
            "rack.overwrite_variation",
            Command::OverwriteVariation { rack, index: 0 },
        ),
        (
            "rack.rename_variation",
            Command::RenameVariation {
                rack,
                index: 0,
                name: "Quiet".to_owned(),
            },
        ),
        (
            "rack.duplicate_variation",
            Command::DuplicateVariation { rack, index: 0 },
        ),
        (
            "rack.delete_variation",
            Command::DeleteVariation { rack, index: 0 },
        ),
        (
            "rack.set_macro_variation_excluded",
            Command::SetMacroVariationExcluded {
                rack,
                macro_number: 1,
                excluded: true,
            },
        ),
        // Spec: scales R5, R9.
        (
            "clip.set_scale",
            Command::SetClipScale {
                clips: vec![clip],
                enabled: Some(true),
                root: Some(2),
                name: Some("dorian".to_owned()),
            },
        ),
        // Spec: audio-clips R30.
        (
            "arrangement.set_crossfade",
            Command::SetCrossfade {
                track,
                boundary: Ticks::WHOLE,
                before: 0.002,
                after: 0.002,
                curve: 0.5,
            },
        ),
        (
            "arrangement.delete_crossfade",
            Command::DeleteCrossfade {
                track,
                boundary: Ticks::WHOLE,
            },
        ),
        (
            "song.set_scale",
            Command::SetScale {
                enabled: Some(true),
                root: Some(5),
                name: Some("minor".to_owned()),
            },
        ),
        // Spec: tunings R2, R7, R8, R9, R14.
        (
            "tuning.set",
            Command::SetTuning {
                tuning: Tuning::new(
                    "5".into(),
                    "five".into(),
                    vec![240.0, 480.0, 720.0, 960.0, 1200.0],
                )
                .unwrap(),
                retune: true,
            },
        ),
        ("tuning.remove", Command::RemoveTuning { retune: false }),
        (
            "tuning.set_reference",
            Command::SetTuningReference {
                octave: Some(4),
                degree: Some(1),
                frequency: Some(300.0),
            },
        ),
        (
            "tuning.set_range",
            Command::SetTuningRange {
                lowest: Some(TuningNote {
                    octave: 0,
                    degree: 2,
                }),
                highest: None,
            },
        ),
        (
            "track.set_tuning_bypass",
            Command::SetTuningBypass {
                track,
                bypass: true,
            },
        ),
        (
            "track.set_tuning_layout",
            Command::SetTuningLayout {
                track,
                layout: Some(bl_model::LayoutKind::Custom),
                keys: Some(vec![None; 128]),
            },
        ),
        (
            "track.set_link_group",
            Command::SetTrackLinks {
                tracks: vec![track],
                group: None,
            },
        ),
    ];
    let covered: BTreeSet<&str> = examples.iter().map(|(id, _)| *id).collect();
    let document_commands: BTreeSet<&str> = commands
        .iter()
        .map(|spec| spec.id.as_ref())
        .filter(|id| *id != registry::UNDO && *id != registry::REDO && *id != registry::GO_TO_STEP)
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
    let main = editor.document().main_id();

    type Expected<'a> = &'a dyn Fn(RunError) -> bool;
    let invalid = |error: RunError| matches!(error, RunError::InvalidArgs { .. });
    let cases: [(&str, Value, Expected<'_>); 18] = [
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
        // Spec: routing R2, R16
        (
            "routing.set_input",
            json!({ "track": track, "input": { "type": "all_ins" } }),
            &|e| {
                matches!(
                    e,
                    RunError::Edit(EditError::Command(ModelError::WrongTrackKind { .. }))
                )
            },
        ),
        ("mixer.set_crossfader", json!({ "position": 2.0 }), &|e| {
            matches!(
                e,
                RunError::Edit(EditError::Command(ModelError::InvalidValue { .. }))
            )
        }),
        // Spec: mixer R24, R27
        ("mixer.set_cue_volume", json!({ "volume": 2.5 }), &|e| {
            matches!(
                e,
                RunError::Edit(EditError::Command(ModelError::InvalidValue { .. }))
            )
        }),
        ("mixer.set_cue_mode", json!({ "enabled": true }), &|e| {
            matches!(
                e,
                RunError::Edit(EditError::Command(ModelError::CueOverlapsMain))
            )
        }),
        (
            "mixer.set_crossfade_assign",
            json!({ "track": main, "assign": "a" }),
            &|e| {
                matches!(
                    e,
                    RunError::Edit(EditError::Command(ModelError::WrongTrackKind { .. }))
                )
            },
        ),
        (
            "routing.set_input",
            json!({
                "track": track,
                "input": { "type": "port", "name": "Keys" },
                "channel": 17,
            }),
            &|e| {
                matches!(
                    e,
                    RunError::Edit(EditError::Command(ModelError::InvalidInput(_)))
                )
            },
        ),
        (
            "routing.set_monitor",
            json!({ "track": track, "monitor": "sometimes" }),
            &invalid,
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
    let schema = commands
        .get("mixer.set_crossfader")
        .unwrap()
        .args
        .as_ref()
        .unwrap();
    assert!(validate(schema, &json!({ "position": 2.0 })).is_err());
    assert_eq!(
        registry::command_from_args(registry::UNDO, json!({})),
        Err(RunError::NotADocumentEdit(registry::UNDO))
    );
}

/// The default bindings follow the command bindings spec.
#[test]
fn bindings_follow_the_spec() {
    // Spec: mixer R21
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
    assert_eq!(
        bindings("mixer.set_arm"),
        ["S-TRACK-013 toggles arm", "S-TRANS-007 toggles arm"]
    );
    // Spec: command-bindings R6
    assert_eq!(bindings("clip.set_mute"), ["S-ARR-027 toggles mute"]);
    // Spec: command-bindings S-TRANS-011
    assert_eq!(
        bindings("transport.set_metronome_enabled"),
        ["S-TRANS-011 toggles enabled"]
    );
    assert_eq!(
        bindings("device.set_enabled_all"),
        ["S-DEV-003 toggles enabled"]
    );

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
        // WP-3.2: a slot's Clip Stop button, Cmd-E (M-EDIT-032).
        "slot.set_stop_button",
        // Beatlayer's own key, Option-F on macOS (IMP-14, S-DEV-015).
        "device.set_floating",
        // Spec: device-compare R6, S-DEV-011.
        "device.compare_switch",
        "transport.set_metronome_enabled",
        // WP-5.1: Option-click on a rack's Device Activator (S-DEV-003).
        "device.set_enabled_all",
        // WP-5.2: Rename in a rack's chain list (S-EDIT-008).
        "chain.rename",
        // WP-4.10: the Sample Editor's keys that name no argument but the
        // clip (S-SAMPLE-009 to S-SAMPLE-012).
        "clip.insert_warp_marker",
        "clip.delete_warp_markers",
        "clip.insert_transient",
        "clip.delete_transients",
        // WP-4.11: the two Arrangement mouse gestures (S-ARR-005, S-ARR-006).
        "clip.stretch",
        "clip.slide_contents",
    ]
    .into_iter()
    .collect();
    assert_eq!(bound, expected, "only these commands have Live shortcuts");
    // Spec: sample-editor R10
    assert_eq!(bindings("clip.insert_warp_marker"), ["S-SAMPLE-009"]);
    assert_eq!(bindings("clip.delete_warp_markers"), ["S-SAMPLE-010"]);
    assert_eq!(bindings("clip.insert_transient"), ["S-SAMPLE-011"]);
    assert_eq!(bindings("clip.delete_transients"), ["S-SAMPLE-012"]);
    // Spec: warping R47, R54
    assert_eq!(bindings("clip.stretch"), ["S-ARR-006"]);
    assert_eq!(bindings("clip.slide_contents"), ["S-ARR-005"]);
}
