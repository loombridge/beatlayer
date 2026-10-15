//! Rules G1 and G2 over every device type the framework knows (WP-2.5; the
//! device framework spec, R2 and R3): Beatlayer's own native devices
//! (none ship yet) and the framework's test devices.

bl_rt::install_trap!();

mod common;

use bl_devices::{Audio, NativeDevice, NativeNode, ParamBank, testing};
use bl_engine::{DeviceType, Registry};
use bl_model::param::MINUS_INFINITY_DB;
use common::{config, device, node, run, set};

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
            // Where the gain acts on the audio, at −∞ the audio is silent.
            let mut at_default = node(&registry, &device(&device_type, &[]), 256);
            let mut at_silence = node(&registry, &device(&device_type, &[(param.id, 0.0)]), 256);
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
