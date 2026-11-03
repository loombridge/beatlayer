//! Rules G1 and G2 over every device type the framework knows (WP-2.5; the
//! device framework spec, R2 and R3): Beatlayer's own native devices
//! (none ship yet) and the framework's test devices.

use crate::common;
use bl_core::time::{Frames, Ticks};
use bl_devices::{Audio, NativeDevice, NativeNode, ParamBank, testing};
use bl_engine::{
    Collector, Compiler, DeviceType, EngineConfig, Node, NodeKey, Registry, TransportInfo, Update,
};
use bl_model::modulation::Travel;
use bl_model::param::MINUS_INFINITY_DB;
use bl_model::{
    AutomationTarget, Clip, ClipData, Command, DeviceId, Editor, MidiData, ParamId, TrackKind,
};
use common::{RATE, config, device, node, run, run_at, set};

/// The gains of EQ Three, each of which acts on one band.
const BAND_GAINS: [ParamId; 3] = [
    bl_devices::three_band_eq::GAIN_LOW,
    bl_devices::three_band_eq::GAIN_MID,
    bl_devices::three_band_eq::GAIN_HIGH,
];

/// Every native device type: the shipped ones and the test ones.
fn every_type() -> Vec<DeviceType> {
    let mut types = bl_devices::device_types();
    types.extend(testing::device_types());
    types
}

fn registry() -> Registry {
    let mut registry = testing::registry();
    bl_devices::register(&mut registry);
    registry
}

// Spec: device-framework R2 (rule G1)
#[test]
fn every_gain_parameter_reaches_minus_infinity() {
    let registry = registry();
    let input = common::noise(256);
    let mut gains = 0;
    for device_type in every_type() {
        for param in device_type
            .description
            .params
            .iter()
            .filter(|p| p.is_gain())
        {
            gains += 1;
            let who = format!("{}: {}", device_type.name, param.name);
            assert_eq!(param.min(), 0.0, "{who}: its range starts at silence");
            assert_eq!(param.denormalize(0.0), 0.0, "{who}: the control's bottom");
            assert_eq!(param.format(0.0), MINUS_INFINITY_DB, "{who}");
            assert_eq!(param.parse("-inf"), Some(0.0), "{who}");
            // Where the gain acts on the audio, at −∞ the audio is silent. A
            // gain that acts on one band of a device does so with the device's
            // other band gains: all at −∞, the device is silent.
            let band_gain = |p: &bl_model::ParamDescriptor| {
                p.name.split_once(' ').map(|(_, gain)| gain.to_owned())
            };
            let group: Vec<_> = if device_type.name == "EQ Three" {
                BAND_GAINS.to_vec()
            } else if matches!(
                device_type.name.as_str(),
                "Multiband Dynamics" | "Re-Enveloper"
            ) && band_gain(param).is_some()
            {
                let same = |p: &&bl_model::ParamDescriptor| {
                    p.is_gain() && band_gain(p) == band_gain(param)
                };
                device_type
                    .description
                    .params
                    .iter()
                    .filter(same)
                    .map(|p| p.id)
                    .collect()
            } else if device_type.name == "Resonators" && param.name != "Gain" {
                // Each resonator's gain acts on one of five.
                let resonator = |p: &&bl_model::ParamDescriptor| p.is_gain() && p.name != "Gain";
                device_type
                    .description
                    .params
                    .iter()
                    .filter(resonator)
                    .map(|p| p.id)
                    .collect()
            } else {
                vec![param.id]
            };
            let silenced: Vec<_> = group.iter().map(|id| (*id, 0.0)).collect();
            let mut at_default = node(&registry, &device(&device_type, &[]), 256);
            let mut at_silence = node(&registry, &device(&device_type, &silenced), 256);
            let open = run(at_default.as_mut(), &input, &[]);
            let shut = run(at_silence.as_mut(), &input, &[]);
            if shut != open {
                assert!(
                    shut.iter().flatten().all(|s| *s == 0.0),
                    "{who}: at -inf dB the device is silent"
                );
            }
        }
    }
    assert!(gains >= 4, "the test devices have gains to check");
}

/// Writes one parameter's value, frame by frame, into every channel.
struct Probe(usize);

impl NativeDevice for Probe {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        for channel in 0..audio.channels() {
            audio
                .channel_mut(channel)
                .copy_from_slice(params.ramp(self.0));
        }
    }
}

// Spec: device-framework R3 (rule G2)
#[test]
fn every_parameter_is_automatable_to_the_frame() {
    let mut count = 0;
    for device_type in every_type() {
        let description = &device_type.description;
        description.check().unwrap();
        for (index, param) in description.params.iter().enumerate() {
            count += 1;
            let who = format!("{}: {}", device_type.name, param.name);
            assert!(param.automatable, "{who}");
            // An automation event at frame 10 moves it from that frame on.
            let to = if param.default == param.max() {
                param.min()
            } else {
                param.max()
            };
            let mut node = NativeNode::new(
                Probe(index),
                description,
                &device(&device_type, &[]),
                &config(2048),
            );
            let out = run(&mut node, &[0.0; 2048], &[set(10, param.id, to)]);
            let (from, to) = (param.default as f32, to as f32);
            assert!(out[0][..10].iter().all(|v| *v == from), "{who}: before");
            assert_ne!(out[0][10], from, "{who}: on its frame");
            assert_eq!(out[0][2047], to, "{who}: reached");
        }
    }
    assert!(count >= 12);
}

// Spec: modulation R1 (rule G2): a clip's modulation envelope moves every
// parameter of every device type, on the frame it plays, by its share of the
// parameter's travel.
#[test]
fn every_parameter_is_modulatable_to_the_frame() {
    let mut count = 0;
    for device_type in every_type() {
        let description = &device_type.description;
        for (index, param) in description.params.iter().enumerate() {
            count += 1;
            let who = format!("{}: {}", device_type.name, param.name);
            let mut editor = Editor::new();
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
            let id: DeviceId = editor.new_id();
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device: device_type.device(id),
                })
                .unwrap();
            let clip = editor.new_id();
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
            editor
                .apply(Command::SetClipEnvelopeBreakpoint {
                    kind: bl_model::EnvelopeKind::Modulation,
                    clip,
                    target: AutomationTarget::Device {
                        device: id,
                        param: param.id,
                    },
                    at: Ticks::ZERO,
                    value: 0.5,
                })
                .unwrap();
            let collector = Collector::new();
            let mut compiler = Compiler::new(
                EngineConfig::new(RATE, 2048),
                registry(),
                collector.handle(),
            );
            let update: Update = compiler.sync(editor.document());
            let data = update
                .data
                .into_iter()
                .find(|(key, _)| matches!(key, NodeKey::Device(device) if *device == id))
                .unwrap_or_else(|| panic!("{who}: its node is sent its envelopes"))
                .1;
            let mut node = NativeNode::new(
                Probe(index),
                description,
                &device(&device_type, &[]),
                &config(2048),
            );
            node.receive_data(data);
            let out = run_at(
                &mut node,
                &vec![vec![0.0; 64]; 2],
                &[],
                &editor.document().settings().tempo,
                TransportInfo {
                    playing: true,
                    position: Frames(0),
                    ..TransportInfo::default()
                },
            );
            let want = param.clamp(Travel::Param(param).played(param.default, 0.5)) as f32;
            assert!(out[0].iter().all(|v| *v == want), "{who}: {want}");
        }
    }
    assert!(count >= 12);
}

// Spec: modulation R4 (rule G2): a clip's modulation envelope on a parameter a
// device reads through the bank's value, not its ramp, still changes what the
// real device plays.
#[test]
fn a_clip_modulation_reaches_a_parameter_a_device_reads_by_value() {
    let play = |device_type: &DeviceType, param: ParamId, modulated: bool| {
        let mut editor = Editor::new();
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
        let id: DeviceId = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: device_type.device(id),
            })
            .unwrap();
        let clip = editor.new_id();
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
        if modulated {
            editor
                .apply(Command::SetClipEnvelopeBreakpoint {
                    kind: bl_model::EnvelopeKind::Modulation,
                    clip,
                    target: AutomationTarget::Device { device: id, param },
                    at: Ticks::ZERO,
                    value: 0.5,
                })
                .unwrap();
        }
        let collector = Collector::new();
        let mut compiler = Compiler::new(
            EngineConfig::new(RATE, 2048),
            registry(),
            collector.handle(),
        );
        let update: Update = compiler.sync(editor.document());
        let mut node = node(&registry(), &device(device_type, &[]), 2048);
        if let Some((_, data)) = update
            .data
            .into_iter()
            .find(|(key, _)| matches!(key, NodeKey::Device(device) if *device == id))
        {
            node.receive_data(data);
        }
        let input = common::noise(512);
        run_at(
            &mut *node,
            &[input.clone(), input],
            &[],
            &editor.document().settings().tempo,
            TransportInfo {
                playing: true,
                position: Frames(0),
                ..TransportInfo::default()
            },
        )
    };
    for (device_type, param) in [
        (
            bl_devices::phaser_flanger::phaser_flanger_type(),
            bl_devices::phaser_flanger::DRY_WET,
        ),
        (
            bl_devices::chorus_ensemble::chorus_ensemble_type(),
            bl_devices::chorus_ensemble::DRY_WET,
        ),
    ] {
        assert_ne!(
            play(&device_type, param, true),
            play(&device_type, param, false),
            "{}: Dry/Wet's modulation is heard",
            device_type.name
        );
    }
}

// Spec: info-view R6 (D17, WP-2.24): the Info View reads a parameter's
// registered description, so every parameter of every device type
// Beatlayer registers — shipped, kit plug-ins, and the framework's test
// devices — has one written.
#[test]
fn every_parameter_has_a_description() {
    let mut types = bl_devices::device_types();
    types.extend(bl_devices::kit_host().device_types());
    types.extend(testing::device_types());
    let mut count = 0;
    for device_type in types {
        for param in &device_type.description.params {
            count += 1;
            let who = format!("{}: {}", device_type.name, param.name);
            assert!(
                !param.description.trim().is_empty(),
                "{who} has no description"
            );
        }
    }
    assert!(count >= 12, "at least the test devices' parameters");
}
