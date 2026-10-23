//! The External Instrument (WP-5.34; the external devices spec, R9, R11): a
//! gain on what the engine brings back from the interface, with nothing
//! in its own sound.

use crate::common;
use bl_devices::hardware::{GAIN, hardware_instrument_type};
use bl_engine::{Block, Registry, TransportInfo};
use bl_model::param::db_to_gain;
use common::{CHANNELS, RATE, device, node};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::hardware::register(&mut registry);
    registry
}

/// One block of the interface's return: `left` and `right`, through the
/// device with its gain set to `db` dB.
fn play(db: f64, left: &[f32], right: &[f32]) -> Vec<Vec<f32>> {
    let frames = left.len();
    let mut hardware = node(
        &registry(),
        &device(&hardware_instrument_type(), &[(GAIN, db_to_gain(db))]),
        frames,
    );
    // The track's buffer starts with whatever came before the instrument.
    let mut data = vec![0.9f32; CHANNELS * frames];
    let returned: Vec<f32> = left.iter().chain(right).copied().collect();
    let mut block = Block::new(
        &mut data,
        CHANNELS,
        frames,
        frames,
        &[],
        TransportInfo::default(),
        RATE,
    )
    .with_external(&returned);
    bl_rt::rt_test!({ hardware.process(&mut block) });
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

// Spec: external-devices R11
#[test]
fn the_return_is_the_chosen_inputs_scaled_by_gain() {
    let left: Vec<f32> = (0..256).map(|i| (i as f32 / 256.0).sin()).collect();
    let right: Vec<f32> = left.iter().map(|s| -s).collect();
    // 0 dB is unity: the return replaces what was in the buffer.
    let out = play(0.0, &left, &right);
    for (got, want) in out[0].iter().zip(&left) {
        assert!((got - want).abs() < 1e-6);
    }
    for (got, want) in out[1].iter().zip(&right) {
        assert!((got - want).abs() < 1e-6);
    }
    // +24 dB, -6 dB and minus infinity.
    let out = play(24.0, &left, &right);
    assert!((out[0][100] / left[100] - db_to_gain(24.0) as f32).abs() < 1e-3);
    let out = play(-6.0, &left, &right);
    assert!((out[1][100] / right[100] - db_to_gain(-6.0) as f32).abs() < 1e-3);
    let out = play(f64::NEG_INFINITY, &left, &right);
    assert!(out.iter().flatten().all(|s| *s == 0.0));
}

// Spec: external-devices R11
#[test]
fn without_a_return_it_is_silent() {
    // No audio device: the block carries no return.
    let mut hardware = node(&registry(), &device(&hardware_instrument_type(), &[]), 64);
    let mut data = vec![0.5f32; CHANNELS * 64];
    let mut block = Block::new(
        &mut data,
        CHANNELS,
        64,
        64,
        &[],
        TransportInfo::default(),
        RATE,
    );
    bl_rt::rt_test!({ hardware.process(&mut block) });
    assert!(data.iter().all(|s| *s == 0.0));
}
