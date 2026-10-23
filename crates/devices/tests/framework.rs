//! The native device framework (WP-2.5; the device framework spec, R4 to
//! R9): parameters land on their frame, glide or step, stay in range, and
//! run without allocating; descriptions that break a rule never register.

use crate::common;
use bl_devices::testing::{self, DRIVE_OVERSAMPLING, UTILITY_GAIN, UTILITY_MUTE};
use bl_devices::{NativeDevice, NativeNode, ParamBank};
use bl_engine::{DeviceType, Registry};
use bl_model::param::db_to_gain;
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId};
use common::{config, device, node, run, set};

/// 20 ms at 48 kHz.
const GLIDE: usize = 960;

// Spec: device-framework R4, R6
#[test]
fn a_continuous_change_lands_on_its_frame_and_glides_for_20_ms() {
    let registry = testing::registry();
    let mut utility = node(&registry, &device(&testing::utility_type(), &[]), 2048);
    let out = run(
        utility.as_mut(),
        &[1.0; 2048],
        &[set(10, UTILITY_GAIN, 0.0)],
    );
    for channel in &out {
        assert!(
            channel[..10].iter().all(|s| *s == 1.0),
            "unchanged before its frame"
        );
        assert!(
            (channel[10] - (1.0 - 1.0 / GLIDE as f32)).abs() < 1e-6,
            "{}",
            channel[10]
        );
        assert!(
            channel[10..10 + GLIDE].windows(2).all(|w| w[1] < w[0]),
            "a straight glide"
        );
        assert!(
            channel[10 + GLIDE - 1..].iter().all(|s| *s == 0.0),
            "silent once it lands"
        );
    }
}

// Spec: device-framework R4, R6
#[test]
fn a_stepped_change_lands_at_once_on_its_frame() {
    let registry = testing::registry();
    let mut utility = node(&registry, &device(&testing::utility_type(), &[]), 64);
    let out = run(utility.as_mut(), &[0.5; 64], &[set(5, UTILITY_MUTE, 1.0)]);
    assert!(out[0][..5].iter().all(|s| *s == 0.5));
    assert!(out[0][5..].iter().all(|s| *s == 0.0));
}

// Spec: device-framework R4
#[test]
fn a_switched_off_device_takes_changes_at_once() {
    let registry = testing::registry();
    let mut utility = node(&registry, &device(&testing::utility_type(), &[]), 64);
    utility.flush(&[set(0, UTILITY_GAIN, 0.5)]);
    let out = run(utility.as_mut(), &[1.0; 64], &[]);
    assert!(
        out[1].iter().all(|s| *s == 0.5),
        "no glide when it comes back on"
    );
}

// Spec: device-framework R1, R4
#[test]
fn values_outside_the_range_are_clamped() {
    let registry = testing::registry();
    let loud = device(&testing::utility_type(), &[(UTILITY_GAIN, 1000.0)]);
    let mut utility = node(&registry, &loud, 64);
    let out = run(utility.as_mut(), &[0.1; 64], &[]);
    let top = (0.1 * db_to_gain(12.0)) as f32;
    assert!(
        out[0].iter().all(|s| (s - top).abs() < 1e-6),
        "{}",
        out[0][0]
    );
    let out = run(utility.as_mut(), &[0.1; 64], &[set(0, UTILITY_GAIN, -5.0)]);
    assert!(out[0][63] < top, "heading for silence, not below it");
}

// Spec: device-framework R8
#[test]
fn an_oversampled_device_runs_without_allocating_and_reports_its_latency() {
    let registry = testing::registry();
    let drive = device(&testing::drive_type(), &[(DRIVE_OVERSAMPLING, 3.0)]);
    let mut node = node(&registry, &drive, 512);
    let input = common::noise(512);
    // Blocks of several sizes, as a device's stream gives them.
    for frames in [512, 1, 37, 256] {
        run(node.as_mut(), &input[..frames], &[]);
    }
    assert_eq!(node.latency(), 56, "8 times: 32 + 16 + 8 frames");
    run(
        node.as_mut(),
        &input[..64],
        &[set(0, DRIVE_OVERSAMPLING, 0.0)],
    );
    assert_eq!(node.latency(), 0);
}

struct Nothing;

impl NativeDevice for Nothing {
    fn process(&mut self, _: &mut bl_devices::Audio<'_, '_>, _: &ParamBank) {}
}

// Spec: device-framework R3 (rule G2)
#[test]
#[should_panic(expected = "G2")]
fn a_device_with_a_parameter_that_cant_be_automated_never_registers() {
    let mut fixed = ParamDescriptor::toggle(ParamId(0), "Fixed", false);
    fixed.automatable = false;
    let bad = DeviceType::native("test.fixed", "Fixed", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(vec![fixed]));
    bl_devices::add(&mut Registry::empty(), bad, |_, _| Nothing);
}

// Spec: device-framework R4
#[test]
fn the_bank_starts_at_the_devices_values() {
    let utility = testing::utility_type();
    let half = device(&utility, &[(UTILITY_GAIN, 0.5)]);
    let node = NativeNode::new(Nothing, &utility.description, &half, &config(64));
    let bank = node.params();
    assert_eq!(bank.len(), 4);
    assert_eq!(bank.value(bank.index(UTILITY_GAIN).unwrap()), 0.5);
    assert_eq!(bank.value(3), 0.0, "the default where the device sets none");
}
