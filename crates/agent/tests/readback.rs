//! The query tool returns what the set saves (agent readback): a set that
//! holds a non-default value of every saved field of a track, clip, device,
//! scene and the song is opened, and the query's `detail: "full"` results
//! are held against the set format's own encoding of it.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_core::time::{Bpm, Ticks, TimeSignature};
use bl_model::{
    AudioData, AutomationTarget, Breakpoint, Channel, Clip, ClipData, Command, ContentHash,
    ControlKind, CountIn, CrossfadeAssign, Device, DeviceId, DeviceKind, DeviceOutput, DeviceRole,
    DeviceWarp, Editor, External, GroupKind, HardwareLatency, Input, LaunchMode,
    LaunchQuantization, LaunchSettings, MetronomeRhythm, MetronomeSound, MidiData, MidiTo,
    MixerParam, Monitor, NewTakeLane, Note, NoteGroup, Output, PanMode, ParamId, RemoteMapping,
    RemoteSource, RemoteTarget, SendTap, Sidechain, TakeClip, TakeClipData, Tap, TonesSettings,
    TrackDelay, TrackId, TrackKind, TransientEdits, WarpMarker, WarpMode, WarpSettings,
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
        pitch: vec![Breakpoint {
            at: Ticks(0),
            value: -12.5,
            bend: -0.25,
        }],
        slide: vec![Breakpoint {
            at: Ticks(44_100),
            value: 100.0,
            bend: 0.0,
        }],
        pressure: vec![Breakpoint {
            at: Ticks(88_200),
            value: 64.0,
            bend: 0.0,
        }],
        group: None,
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
    // Spec: linked-track editing R1, R16. Two tracks linked.
    let link: bl_model::LinkGroupId = editor.new_id();
    apply(
        &mut editor,
        Command::SetTrackLinks {
            tracks: vec![vox, pad],
            group: Some(link),
        },
    );

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
        // Spec: automation-editing R8. Lock Envelopes is saved with the
        // set, so the fixture has it on.
        Command::SetLockEnvelopes { enabled: true },
        Command::SetRemoteMappings {
            mappings: vec![
                RemoteMapping::new(
                    RemoteSource::Key {
                        key: "S".to_owned(),
                    },
                    RemoteTarget::StopAll,
                    ControlKind::Button,
                    None,
                    None,
                )
                .unwrap(),
            ],
        },
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
        Command::SetCueOut {
            channel: Channel::Pair(3),
        },
        Command::SetCueMode { enabled: true },
        Command::SetCueVolume { volume: 0.5 },
        Command::SetReducedLatencyMonitoring { enabled: true },
        // Spec: tunings R2, R14. A tuning inside the set, with a track that
        // bypasses it.
        Command::SetTuning {
            tuning: bl_model::Tuning::new(
                "Five".to_owned(),
                "Five equal steps".to_owned(),
                (1..=5).map(|i| 240.0 * f64::from(i)).collect(),
            )
            .unwrap(),
            retune: false,
        },
        Command::SetTuningBypass {
            track: pad,
            bypass: true,
        },
        // Spec: tunings R22, R24.
        Command::SetTuningLayout {
            track: pad,
            layout: Some(bl_model::LayoutKind::BlackKeysOnly),
            keys: None,
        },
        // Spec: scales R9.
        Command::SetScale {
            enabled: Some(true),
            root: Some(2),
            name: Some("dorian".to_owned()),
        },
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
        Command::SetPanMode {
            track: vox,
            mode: PanMode::Split,
        },
        Command::SetSplitPanLeft {
            track: vox,
            pan: -1.0,
        },
        Command::SetSplitPanRight {
            track: vox,
            pan: 1.0,
        },
        Command::SetTrackDelay {
            track: vox,
            delay: TrackDelay::Samples { value: 480 },
        },
        Command::SetKeepMonitoringLatency {
            track: vox,
            enabled: false,
        },
        // Switched off only now: a track's delay can't be set while delay
        // compensation is off (the delay compensation spec, R7).
        Command::SetDelayCompensation { enabled: false },
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
            to: Some(MidiTo::Track(pad)),
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
    // The drums take an extra output of the Keys track's instrument (WP-5.45).
    let impulse: DeviceId = editor.new_id();
    apply(
        &mut editor,
        Command::InsertDevice {
            track: keys,
            index: 0,
            device: Device::new(
                impulse,
                "Impulse",
                DeviceKind::Native {
                    device_type: "beatlayer.kitbag".to_owned(),
                },
            )
            .with_role(DeviceRole::Instrument),
        },
    );
    apply(
        &mut editor,
        Command::SetInput {
            track: drums,
            input: Input::Track {
                track: keys,
                tap: Tap::PostFx,
                chain: None,
                output: Some(DeviceOutput {
                    device: impulse,
                    output: 2,
                }),
            },
            channel: None,
        },
    );
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

    // Two take lanes, the first with two clips.
    let pass = |editor: &mut Editor, name: &str, start: i64, from: f64| TakeClip {
        id: Some(editor.new_id()),
        name: name.to_owned(),
        start: Ticks(start),
        length: Ticks(BAR),
        data: TakeClipData::Audio(AudioData {
            start: from,
            end: Some(from + 2.0),
            gain: 0.7,
            ..AudioData::new(ContentHash([3; 32]))
        }),
    };
    let mut lanes = Vec::new();
    for (name, clips) in [
        (
            "Take 1",
            vec![
                pass(&mut editor, "a", 0, 0.0),
                pass(&mut editor, "b", BAR, 1.0),
            ],
        ),
        ("Take 2", vec![pass(&mut editor, "c", 0, 0.5)]),
    ] {
        let id = editor.new_id();
        lanes.push(id);
        let lane = NewTakeLane {
            id: Some(id),
            name: name.to_owned(),
            clips,
        };
        apply(
            &mut editor,
            Command::AddTakeLane {
                track: vox,
                lane,
                index: None,
            },
        );
    }
    // The second is auditioned.
    apply(
        &mut editor,
        Command::SetAudition {
            lane: lanes[1],
            on: true,
        },
    );

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
    // The edits made to that sample (WP-5.15).
    device.sample_edits = vec![bl_model::SampleEdit {
        kind: bl_model::SampleEditKind::FadeIn,
        range: None,
    }];
    // A sample's warp controls and the slices made by hand (WP-5.15b).
    device.warp = DeviceWarp {
        warp: true,
        mode: WarpMode::Tones,
        settings: WarpSettings {
            tones: TonesSettings { grain_size: 40.0 },
            ..WarpSettings::default()
        },
        bpm: Some(90.0),
    };
    device.slices = vec![1_000, 2_000];
    // The flags that bound what a Simpler plays (sampler R22).
    device.region = Some(bl_model::SampleRegion::Frames {
        start: 500,
        end: 4_000,
    });
    // A multisampler's layers and options (WP-5.13).
    device.layers = bl_model::Layers {
        list: vec![bl_model::Layer::new(
            bl_model::LayerId(1),
            "Kick",
            ContentHash([5; 32]),
            48_000,
            48_000,
            1,
        )],
        last_id: 1,
        ram_mode: true,
        snap: true,
        linear_loop_fades: true,
    };
    device.role = DeviceRole::AudioEffect;
    device.params.insert(ParamId(2), 0.75);
    // B active, with A's values kept (device compare R1, R10).
    device.b_active = true;
    device.other_params = Some([(ParamId(2), 0.25)].into_iter().collect());
    device.external = Some(External {
        port: Some("Synth".to_owned()),
        channel: 5,
        mpe: Some(bl_model::MpeSettings {
            zone: bl_model::MpeZone::Upper,
            channels: 7,
            ..bl_model::MpeSettings::default()
        }),
        audio_from: Channel::Mono(3),
        audio_to: None,
        hardware_latency: HardwareLatency::Samples { value: 96 },
        // A CV device's ports and calibration (the CV tools spec, R4, R20).
        cv_outputs: vec![bl_model::CvPort::new("Pitch")],
        cv_calibration: bl_model::CvCalibration::fit(&[(0.0, 130.0), (1.0, 260.0)]),
        ..External::default()
    });
    device.sidechain = Some(Sidechain {
        enabled: true,
        input: Input::Track {
            track: drums,
            tap: Tap::PreFx,
            chain: None,
            output: None,
        },
        gain: 0.5,
        mix: 0.25,
        listen: true,
    });
    device.midi_sidechain = Some(bl_model::MidiSidechain {
        source: Some(keys),
        tap: None,
    });
    apply(
        &mut editor,
        Command::InsertDevice {
            track: vox,
            index: 0,
            device,
        },
    );
    // A Shaper mapping that device, with a shape of its own.
    let curve: bl_model::DeviceId = editor.new_id();
    let mut modulator = Device::new(
        curve,
        "Shaper",
        DeviceKind::Native {
            device_type: bl_model::modulator::CURVE_MOD.to_owned(),
        },
    );
    modulator.shape = Some(bl_model::modulator::shape_preset(6).unwrap());
    apply(
        &mut editor,
        Command::InsertDevice {
            track: vox,
            index: 1,
            device: modulator,
        },
    );
    apply(
        &mut editor,
        Command::MapModulator {
            device: curve,
            slot: 2,
            target: bl_model::modulator::ModTarget::Device {
                device: compressor,
                param: ParamId(2),
            },
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
    // The first chain's chain select zone, off a new chain's (WP-5.3).
    apply(
        &mut editor,
        Command::SetChainZone {
            chain: loud,
            kind: bl_model::ZoneKind::Select,
            zone: bl_model::Zone {
                low: 2,
                high: 20,
                fade_low: 4,
                fade_high: 6,
            },
        },
    );

    // Clips: an audio clip with everything a clip saves, a MIDI clip with a
    // loop and notes, and slot clips.
    let mut audio = Clip::new(
        editor.new_id(),
        Ticks(BAR),
        Ticks(2 * BAR),
        ClipData::Audio(AudioData {
            media: ContentHash([7; 32]),
            // Warped (WP-4.1), so its markers and brace are ticks.
            start: 352_800.0,
            end: Some(2_116_800.0),
            looping: true,
            loop_start: 705_600.0,
            loop_end: Some(1_764_000.0),
            gain: 0.5,
            fade_in: 0.25,
            fade_out: 0.5,
            fade_in_curve: 0.5,
            fade_out_curve: -0.25,
            fade_in_before: 0.125,
            fade_out_after: 0.0625,
            warp: true,
            warp_markers: vec![
                WarpMarker {
                    time: 0.25,
                    beat: Ticks(-176_400),
                },
                WarpMarker {
                    time: 2.0,
                    beat: Ticks(BAR),
                },
            ],
            tempo: Some(100.0),
            signature: Some(TimeSignature::new(3, 4).unwrap()),
            warp_mode: WarpMode::Tones,
            warp_settings: WarpSettings {
                tones: TonesSettings { grain_size: 40.0 },
                ..WarpSettings::default()
            },
            ram: true,
            high_quality: false,
            tempo_leader: true,
            video: true,
            transpose: 7,
            detune: -12.5,
            transient_edits: TransientEdits {
                added: vec![0.5],
                removed: vec![1.25],
            },
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
            kind: bl_model::EnvelopeKind::Modulation,
            clip: audio.id,
            target: volume,
            at: Ticks(0),
            value: 0.5,
        },
    );
    // Spec: note-editing R30. The first two chords' notes make a group.
    let mut chords = [note(60, 0), note(64, 176_400), note(67, 352_800)];
    chords[0].group = Some(1);
    chords[1].group = Some(1);
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
            groups: vec![NoteGroup {
                id: 1,
                kind: GroupKind::PlayOne,
            }],
            ..MidiData::new(chords)
        }),
    );
    midi.name = "Chords".to_owned();
    midi.offset = Ticks(88_200);
    let midi_id = midi.id;
    apply(
        &mut editor,
        Command::CreateClip {
            track: keys,
            clip: midi,
        },
    );
    // Spec: scales R5. The clip's own scale.
    apply(
        &mut editor,
        Command::SetClipScale {
            clips: vec![midi_id],
            enabled: Some(true),
            root: Some(5),
            name: Some("minor".to_owned()),
        },
    );
    // WP-6.19: a groove in the pool, used by the clip, with the Global
    // Amount and Auto Load Groove off their defaults, so the format writes
    // `grooves` and the clip's `groove`.
    let groove: bl_model::GrooveId = editor.new_id();
    apply(
        &mut editor,
        Command::AddGroove {
            groove: bl_model::Groove::new(
                groove,
                "Swing".to_owned(),
                Ticks::WHOLE,
                vec![bl_model::GrooveNote {
                    position: Ticks::SIXTEENTH,
                    velocity: 90,
                }],
            ),
        },
    );
    apply(
        &mut editor,
        Command::SetClipGroove {
            clips: vec![midi_id],
            groove: Some(groove),
        },
    );
    apply(&mut editor, Command::SetGlobalGrooveAmount { amount: 80.0 });
    apply(
        &mut editor,
        Command::SetAutoLoadGroove {
            enabled: Some(true),
            groove: Some(groove),
        },
    );
    // MIDI controller envelopes (WP-2.29): a modulation wheel one and an
    // unlinked pitch bend one.
    for (target, value) in [
        (AutomationTarget::MidiCc { number: 1 }, 100.0),
        (AutomationTarget::MidiPitchBend, -4096.0),
    ] {
        apply(
            &mut editor,
            Command::SetClipEnvelopeBreakpoint {
                clip: midi_id,
                kind: bl_model::EnvelopeKind::Modulation,
                target,
                at: Ticks(88_200),
                value,
            },
        );
    }
    apply(
        &mut editor,
        Command::SetClipEnvelopeLinked {
            clip: midi_id,
            kind: bl_model::EnvelopeKind::Modulation,
            target: AutomationTarget::MidiPitchBend,
            linked: false,
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
    let follow = bl_model::FollowPatch {
        enabled: Some(true),
        ..bl_model::FollowPatch::default()
    };
    apply(
        &mut editor,
        Command::SetClipFollowActions {
            clip: slot_clip_id,
            patch: follow,
        },
    );
    apply(
        &mut editor,
        Command::SetSceneFollowActions {
            scene: scenes[0],
            patch: follow,
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
    // WP-6.58: the user's own info text on a track, a clip, a scene and a
    // device, which a new one doesn't have.
    for command in [
        Command::SetTrackInfoText {
            track: vox,
            info_text: "Lead vocal".to_owned(),
        },
        Command::SetClipInfoText {
            clip: slot_clip_id,
            info_text: "Verse loop".to_owned(),
        },
        Command::SetSceneInfoText {
            scene: scenes[0],
            info_text: "Starts the song".to_owned(),
        },
        Command::SetDeviceInfoText {
            device: rack,
            info_text: "Parallel drive".to_owned(),
        },
    ] {
        apply(&mut editor, command);
    }
    // WP-6.30b: a colour on the scene, which a new one doesn't have.
    apply(
        &mut editor,
        Command::SetSceneColour {
            scenes: vec![scenes[0]],
            colour: Some(47),
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
    // WP-6.24: a locator and a song start time, so the format writes
    // `locators` and `song_start`.
    let locator: bl_model::LocatorId = editor.new_id();
    apply(
        &mut editor,
        Command::AddLocator {
            locator: bl_model::Locator::new(locator, Ticks::WHOLE * 2, "Intro".to_owned()),
        },
    );
    apply(
        &mut editor,
        Command::SetSongStart {
            start: Some(bl_model::SongStart::Locator(locator)),
        },
    );
    // WP-4.14: a frozen track, so the format writes `frozen`.
    apply(
        &mut editor,
        Command::FreezeTracks {
            frozen: vec![(vox, bl_model::FrozenTrack::default())],
        },
    );
    editor.clear_history().unwrap();
    Project::from_editor(&editor, Media::new()).with_launch(LaunchSettings {
        quantization: LaunchQuantization::Bars2,
        enabled: false,
        ..LaunchSettings::default()
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
    let drums = named("Drums");
    for (value, paths) in [
        (
            &document["settings"],
            &[
                "tempo",
                "time_signatures",
                "crossfader",
                "arrangement_loop",
                "solo_in_place",
                "lock_envelopes",
                "metronome",
                "punch_in",
                "punch_out",
                "midi_overdub",
                "locators",
                "song_start",
                "grooves.pool.0.name",
                "grooves.global_amount",
                "grooves.auto_load",
                "grooves.auto_load_groove",
                "scale",
                "tuning.name",
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
                "devices.0.midi_sidechain.source",
                "take_lanes.0.clips.1.data.audio.gain",
                "clips.0",
            ][..],
        ),
        (&verb, &["mixer.tap"][..]),
        (
            &drums,
            &["routing.input.output.device", "routing.input.output.output"][..],
        ),
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
                "chains.0.zones.select",
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
                "external.port",
                "external.channel",
                "external.audio_from",
                "external.hardware_latency",
                "midi_sidechain",
                "floating",
                "media",
                "layers",
                "layers.ram_mode",
                "layers.snap",
                "layers.linear_loop_fades",
                "b_active",
                "other_params",
            ][..],
        ),
        (
            &devices
                .iter()
                .find(|d| d["kind"]["native"]["device_type"] == bl_model::modulator::CURVE_MOD)
                .unwrap()
                .clone(),
            &["mappings", "shape"][..],
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
                "data.audio.fade_in_curve",
                "data.audio.fade_out_curve",
                "data.audio.fade_in_before",
                "data.audio.fade_out_after",
                "data.audio.warp",
                "data.audio.warp_markers.0.time",
                "data.audio.warp_markers.1.beat",
                "data.audio.tempo",
                "data.audio.signature",
                "data.audio.warp_mode",
                "data.audio.warp_settings.tones.grain_size",
                "data.audio.ram",
                "data.audio.high_quality",
                "data.audio.transient_edits.added.0",
                "data.audio.transient_edits.removed.0",
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
                "scale",
                "groove",
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

// Spec: midi-controller-data R9. The fixture's MIDI clip holds controller
// envelopes and a note with Pitch, Slide and Pressure envelopes
// (mpe R9).
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
    assert_eq!(lane["clips"]["total"], 2);
    assert_eq!(lane["clips"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(lane["clips"]["items"][0]["data"]["audio"]["gain"], 0.7);
    assert_eq!(lane["audition"], false);
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
    // The device's whole results too, for the fields listed beside `saved`.
    let device_results: Vec<Value> = devices
        .iter()
        .map(|saved| query(&mut client, json!({ "id": saved["id"], "detail": "full" })))
        .collect();
    // The track's whole results too, for the fields listed beside `saved`.
    let track_results: Vec<Value> = tracks
        .iter()
        .map(|saved| query(&mut client, json!({ "id": saved["id"], "detail": "full" })))
        .collect();
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
            Object::Track => {
                tracks.iter().any(|saved| present(saved, field))
                    || (Object::Track.listed().contains(field)
                        && track_results.iter().any(|result| present(result, field)))
            }
            Object::Clip => clips.iter().any(|saved| present(saved, field)),
            Object::Device => {
                devices.iter().any(|saved| present(saved, field))
                    || (Object::Device.listed().contains(field)
                        && device_results.iter().any(|result| present(result, field)))
            }
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

// Spec: racks R36. A chain's zones, with their fades, read back through the
// query tool: in its summary, and in the rack's chains.
#[test]
fn a_chains_zones_read_back() {
    let project = fixture();
    let mut client = open(&project, "zones");
    let document = serde_json::to_value(&project.document).unwrap();
    let (_, _, devices) = saved_parts(&document);
    let rack = devices
        .iter()
        .find(|d| d["name"] == "Audio Effect Rack")
        .unwrap();
    let chain = rack["chains"][0]["id"].clone();
    let got = query(&mut client, json!({ "id": chain }));
    assert_eq!(
        got["zones"]["select"],
        json!({ "low": 2, "high": 20, "fade_low": 4, "fade_high": 6 })
    );
    // A chain nobody changed reports a new chain's zones, fades included.
    let other = query(&mut client, json!({ "id": rack["chains"][1]["id"] }));
    assert_eq!(
        other["zones"],
        json!({
            "key": { "low": 0, "high": 127, "fade_low": 0, "fade_high": 0 },
            "velocity": { "low": 1, "high": 127, "fade_low": 0, "fade_high": 0 },
            "select": { "low": 0, "high": 0, "fade_low": 0, "fade_high": 0 },
        })
    );
    // The rack's own object lists its chains with theirs.
    let object = query(&mut client, json!({ "id": rack["id"] }));
    assert_eq!(object["chains"][0]["zones"], got["zones"]);
}

// Spec: note-editing R20. The note value commands run by ID through the agent
// API, and the query tool reads each note's values back.
#[test]
fn the_note_value_commands_run_by_id_and_read_back() {
    let mut client = open(&fixture(), "note-values");
    let keys = query(
        &mut client,
        json!({ "path": "track \"Keys\"", "detail": "full" }),
    );
    let clip = keys["saved"]["clips"]["items"][0]["id"].clone();
    let notes = |client: &mut Client<InProcess>| -> Vec<Value> {
        let full = query(client, json!({ "id": clip, "detail": "full" }));
        full["saved"]["data"]["midi"]["notes"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("the clip's notes: {full}"))
            .clone()
    };
    let before = notes(&mut client);
    assert_eq!(before.len(), 3);
    let run = |client: &mut Client<InProcess>, label: &str, command: &str, args: Value| {
        client
            .call(
                "run",
                json!({ "label": label, "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap()
            .ok()
            .unwrap_or_else(|error| panic!("{command}: {error}"))
    };
    run(
        &mut client,
        "Quieter",
        "note.adjust_value",
        json!({ "clip": clip, "notes": [0], "field": "velocity", "by": -10 }),
    );
    run(
        &mut client,
        "Deactivate",
        "note.deactivate",
        json!({ "clip": clip, "notes": [1] }),
    );
    run(
        &mut client,
        "Randomize",
        "note.randomize",
        json!({
            "clip": clip, "notes": [2], "field": "release_velocity",
            "amount": 20, "seed": 5
        }),
    );
    let after = notes(&mut client);
    assert_eq!(after[0]["velocity"], 80, "adjusted by -10");
    assert_eq!(after[1]["mute"], true, "deactivated");
    assert_ne!(after[2]["release_velocity"], before[2]["release_velocity"]);
    assert_eq!(after[0]["mute"], before[0]["mute"], "the others untouched");
}

// Spec: themes R25. The colour commands run by ID through the agent API, and
// the query tool's track, clip and scene objects read the swatch back.
#[test]
fn the_colour_commands_run_by_id_and_read_back() {
    let mut client = open(&fixture(), "colours");
    let keys = query(
        &mut client,
        json!({ "path": "track \"Keys\"", "detail": "full" }),
    );
    let track = keys["id"].clone();
    let clip = keys["saved"]["clips"]["items"][0]["id"].clone();
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        client
            .call(
                "run",
                json!({ "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap()
            .ok()
            .unwrap_or_else(|error| panic!("{command}: {error}"))
    };
    let colour = |client: &mut Client<InProcess>, what: &Value| -> Value {
        query(client, json!({ "id": what }))["colour"].clone()
    };

    run(
        &mut client,
        "track.set_colour",
        json!({ "tracks": [track], "colour": 44 }),
    );
    assert_eq!(colour(&mut client, &track), 44);
    run(
        &mut client,
        "clip.set_colour",
        json!({ "clips": [clip], "colour": 7 }),
    );
    assert_eq!(colour(&mut client, &clip), 7);
    // Assign Track Color to Clips gives the clip its track's colour back.
    run(
        &mut client,
        "track.assign_colour_to_clips",
        json!({ "tracks": [track] }),
    );
    assert_eq!(colour(&mut client, &clip), 44);

    let scenes = query(&mut client, json!({ "path": "scene 1" }));
    let scene = scenes["id"].clone();
    assert!(
        scenes["colour"].is_number(),
        "the fixture's scene has one: {scenes}"
    );
    run(
        &mut client,
        "scene.set_colour",
        json!({ "scenes": [scene], "colour": null }),
    );
    assert_eq!(colour(&mut client, &scene), Value::Null);
    run(
        &mut client,
        "scene.set_colour",
        json!({ "scenes": [scene], "colour": 69 }),
    );
    assert_eq!(colour(&mut client, &scene), 69);
    // A swatch outside the palette is refused.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{
                "command": "track.set_colour",
                "args": { "tracks": [track], "colour": 70 }
            }] }),
        )
        .unwrap();
    assert!(refused.ok().is_err());
}

// Spec: note-editing R32. Group Notes and Ungroup Notes run by ID through the
// agent API, and the query tool reads the clip's groups and each note's group
// back.
#[test]
fn the_group_commands_run_by_id_and_read_back() {
    let mut client = open(&fixture(), "note-groups");
    let keys = query(
        &mut client,
        json!({ "path": "track \"Keys\"", "detail": "full" }),
    );
    let clip = keys["saved"]["clips"]["items"][0]["id"].clone();
    let midi = |client: &mut Client<InProcess>| -> Value {
        query(client, json!({ "id": clip, "detail": "full" }))["saved"]["data"]["midi"].clone()
    };
    let run = |client: &mut Client<InProcess>, label: &str, command: &str, args: Value| {
        client
            .call(
                "run",
                json!({ "label": label, "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap()
            .ok()
            .unwrap_or_else(|error| panic!("{command}: {error}"))
    };
    let before = midi(&mut client);
    assert_eq!(before["groups"]["items"][0]["kind"], "play_one");
    assert_eq!(before["notes"]["items"][0]["group"], 1);
    // Notes 1 and 2 leave note 0's group, which ends, for a new Play All.
    run(
        &mut client,
        "Group",
        "note.group",
        json!({ "clip": clip, "notes": [1, 2], "kind": "play_all" }),
    );
    let grouped = midi(&mut client);
    let groups = grouped["groups"]["items"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0]["kind"], "play_all");
    let notes = grouped["notes"]["items"].as_array().unwrap();
    assert!(notes[0].get("group").is_none(), "note 0 left its group");
    assert_eq!(notes[1]["group"], groups[0]["id"]);
    assert_eq!(notes[2]["group"], groups[0]["id"]);
    run(
        &mut client,
        "Ungroup",
        "note.ungroup",
        json!({ "clip": clip, "notes": [1, 2] }),
    );
    let ungrouped = midi(&mut client);
    assert!(ungrouped.get("groups").is_none() || ungrouped["groups"]["items"] == json!([]));
    assert!(
        ungrouped["notes"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|note| note.get("group").is_none())
    );
}

// Spec: multi-clip-editing R9, R11, R18. The clip property commands take
// `clips`, Duplicate Loop runs by ID, and the view commands report Focus Mode;
// each effect reads back through the query tool.
#[test]
fn the_multi_clip_commands_run_by_id_and_read_back() {
    let mut client = open(&fixture(), "multi-clip");
    let keys = query(
        &mut client,
        json!({ "path": "track \"Keys\"", "detail": "full" }),
    );
    let clip = keys["saved"]["clips"]["items"][0]["id"].clone();
    let midi = |client: &mut Client<InProcess>| -> Value {
        query(client, json!({ "id": clip, "detail": "full" }))["saved"]["data"]["midi"].clone()
    };
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        client
            .call(
                "run",
                json!({ "label": "Multi", "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap()
            .ok()
            .unwrap_or_else(|error| panic!("{command}: {error}"))
    };
    run(
        &mut client,
        "clip.set_midi_loop",
        json!({ "clips": [clip], "start": 0, "end": BAR }),
    );
    run(
        &mut client,
        "clip.set_signature",
        json!({ "clips": [clip], "signature": { "numerator": 3, "denominator": 4 } }),
    );
    assert_eq!(midi(&mut client)["loop_end"], BAR);
    assert_eq!(midi(&mut client)["signature"]["numerator"], 3);
    run(
        &mut client,
        "clip.duplicate_loop",
        json!({ "clips": [clip] }),
    );
    let after = midi(&mut client);
    assert_eq!(after["loop_end"], 2 * BAR, "the loop doubles");
    assert_eq!(
        after["notes"]["items"].as_array().unwrap().len(),
        6,
        "the notes in the loop are copied"
    );
    let state = run(&mut client, "clip_view.focus_mode", json!({ "on": true }));
    assert!(state.to_string().contains("\"focus_mode\":true"), "{state}");
}

// Spec: second-window R23. The copies between the Session and the
// Arrangement run by ID through the agent API and read back through the
// query tool's track: its Arrangement clips and its slots.
#[test]
fn the_copies_between_the_views_run_by_id_and_read_back() {
    let mut client = open(&fixture(), "copies");
    let full = |client: &mut Client<InProcess>| {
        query(
            client,
            json!({ "path": "track \"Keys\"", "detail": "full", "limit": 100 }),
        )
    };
    let track = full(&mut client)["id"].clone();
    let before = full(&mut client)["saved"]["clips"]["total"]
        .as_u64()
        .unwrap();
    let slot = query(&mut client, json!({ "path": "track \"Keys\" / slot 2" }));
    let clip = slot["clip"]["id"].clone();
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        client
            .call(
                "run",
                json!({ "label": "Copies", "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap()
            .ok()
            .unwrap_or_else(|error| panic!("{command}: {error}"))
    };
    run(
        &mut client,
        "session.copy_to_arrangement",
        json!({ "clips": [clip], "track": track, "at": 40 * BAR }),
    );
    let after = full(&mut client);
    assert_eq!(
        after["saved"]["clips"]["total"].as_u64().unwrap(),
        before + 1
    );
    let copy = after["saved"]["clips"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["start"] == 40 * BAR)
        .unwrap_or_else(|| panic!("{after}"))
        .clone();
    assert_eq!(copy["name"], "Loop");
    let scene = query(&mut client, json!({ "path": "scene 3" }))["id"].clone();
    run(
        &mut client,
        "session.copy_to_session",
        json!({ "clips": [copy["id"]], "track": track, "scene": scene }),
    );
    let third = query(&mut client, json!({ "path": "track \"Keys\" / slot 3" }));
    assert_eq!(third["clip"]["name"], "Loop");
    // The Session clip it came from is where it was.
    let again = query(&mut client, json!({ "path": "track \"Keys\" / slot 2" }));
    assert_eq!(again["clip"]["id"], slot["clip"]["id"]);
}

// Spec: agent-perception. What `clip.trim_start` and `clip.set_loop_enabled`
// declare as their readback is a field their edit changes, for audio and
// MIDI clips alike.
#[test]
fn the_trim_and_loop_switch_readbacks_change_with_their_commands() {
    use bl_commands::readback::{Object, READBACK, Readback};

    let declared = |command: &str| -> &'static str {
        match READBACK.iter().find(|(id, _)| *id == command) {
            Some((
                _,
                Readback::Query {
                    object: Object::Clip,
                    field,
                },
            )) => field,
            other => panic!("{command}: {other:?}"),
        }
    };
    let mut client = open(&fixture(), "trim-loop");
    let first_clip = |client: &mut Client<InProcess>, track: &str| -> Value {
        let full = query(
            client,
            json!({ "path": format!("track \"{track}\""), "detail": "full" }),
        );
        full["saved"]["clips"]["items"][0]["id"].clone()
    };
    let read = |client: &mut Client<InProcess>, clip: &Value, field: &str| -> Value {
        let mut here = query(client, json!({ "id": clip, "detail": "full" }))["saved"].clone();
        for name in field.split('.') {
            here = here[name].clone();
        }
        here
    };
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        client
            .call(
                "run",
                json!({ "label": command, "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap()
            .ok()
            .unwrap_or_else(|error| panic!("{command}: {error}"));
    };

    let audio = first_clip(&mut client, "Vox");
    let field = declared("clip.trim_start");
    let start = read(&mut client, &audio, "start");
    let before = read(&mut client, &audio, field);
    let to = start.as_i64().unwrap() + BAR / 4;
    run(
        &mut client,
        "clip.trim_start",
        json!({ "clip": audio, "start": to }),
    );
    assert_ne!(read(&mut client, &audio, field), before, "{field}");

    let midi = first_clip(&mut client, "Keys");
    let field = declared("clip.set_loop_enabled");
    let looping = read(&mut client, &midi, "data.midi.looping");
    let before = read(&mut client, &midi, field);
    run(
        &mut client,
        "clip.set_loop_enabled",
        json!({ "clip": midi, "enabled": !looping.as_bool().unwrap() }),
    );
    assert_ne!(read(&mut client, &midi, field), before, "{field}");
}
