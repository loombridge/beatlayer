//! The Wave Mod (WP-5.10; the modulation spec, R21 and R22): nine waveforms
//! that keep to 0 to 1 and repeat at the rate, Shape and Steps that go
//! inactive where they should, a cycle locked to the song position, Hold and
//! R, and random parts that start again with the transport. Every block runs
//! as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Frames, TempoMap};
use bl_devices::modulator::{
    HOLD, PHASE, RATE_BEATS, RATE_HZ, RESTART, SHAPE, STEPS, TIME_MODE, WAVE, WAVES, wave_mod_type,
};
use bl_engine::{Node, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::modulator::{self, POLARITY};
use common::{control, device, node, set};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::modulator::register(&mut registry);
    registry
}

fn playing(position: i64) -> TransportInfo {
    TransportInfo {
        playing: true,
        position: Frames(position),
    }
}

/// A Wave Mod whose slot 1 gives its output itself: unipolar at full amount
/// and depth. `params` set over those.
fn wave(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![(modulator::amount(0), 100.0), (POLARITY, 1.0)];
    all.extend_from_slice(params);
    node(&registry(), &device(&wave_mod_type(), &all), 4_800)
}

/// The output over `frames` frames from song position 0.
fn output(node: &mut dyn Node, frames: usize, block: usize) -> Vec<f32> {
    control(node, frames, block, &[], &TempoMap::default(), playing(0))
        .contribution(0)
        .to_vec()
}

/// 10 Hz at 48 kHz is a cycle of 4,800 frames: a whole number of the
/// 32-frame grid, so a cycle repeats to the frame.
const CYCLE: usize = 4_800;

// Spec: modulation R22
#[test]
fn every_waveform_keeps_to_its_range_and_repeats_at_the_rate() {
    for (index, name) in WAVES.iter().enumerate() {
        let mut node = wave(&[(WAVE, index as f64), (RATE_HZ, 10.0)]);
        let out = output(node.as_mut(), CYCLE * 3, 512);
        assert!(
            out.iter().all(|&v| (0.0..=1.0).contains(&v)),
            "{name} keeps to 0 to 1"
        );
        let (low, high) = out
            .iter()
            .fold((1.0f32, 0.0f32), |(l, h), &v| (l.min(v), h.max(v)));
        assert!(
            high - low > 0.2 || index >= 5,
            "{name} moves: {low} to {high}"
        );
        if index < 5 {
            // The deterministic waves repeat to the frame.
            for i in 0..CYCLE {
                assert!(
                    (out[i] - out[i + CYCLE]).abs() < 1e-5
                        && (out[i] - out[i + 2 * CYCLE]).abs() < 1e-5,
                    "{name} repeats at frame {i}"
                );
            }
        } else {
            // The random ones draw once per cycle: steady through the middle
            // of each cycle (the ramp between draws takes a grid step).
            for cycle in 0..3 {
                let middle = &out[cycle * CYCLE + 64..(cycle + 1) * CYCLE - 64];
                if index == 8 {
                    assert!(
                        middle.windows(2).all(|w| (w[1] - w[0]).abs() < 0.01),
                        "{name} glides"
                    );
                } else {
                    assert!(
                        middle.iter().all(|&v| (v - middle[0]).abs() < 1e-6),
                        "{name} holds within a cycle"
                    );
                }
            }
        }
    }
}

// Spec: modulation R22
#[test]
fn shape_and_steps_go_inactive_where_the_manual_says() {
    // Shape does nothing to the random waves; it does bend the others.
    for (index, name) in WAVES.iter().enumerate() {
        let out = |shape: f64| {
            let mut node = wave(&[(WAVE, index as f64), (SHAPE, shape), (RATE_HZ, 10.0)]);
            output(node.as_mut(), CYCLE, 512)
        };
        if index >= 5 {
            assert!(out(0.0) == out(80.0), "{name}: Shape is inactive");
        } else {
            assert!(out(0.0) != out(80.0), "{name}: Shape bends it");
        }
    }
    // Steps does nothing to Random, Bin and Square; on the ramp it leaves
    // the number of steps asked for.
    for index in [4usize, 5, 6] {
        let out = |steps: f64| {
            let mut node = wave(&[(WAVE, index as f64), (STEPS, steps), (RATE_HZ, 10.0)]);
            output(node.as_mut(), CYCLE, 512)
        };
        assert!(out(0.0) == out(7.0), "{}: Steps is inactive", WAVES[index]);
    }
    let mut node = wave(&[(WAVE, 1.0), (STEPS, 5.0), (RATE_HZ, 10.0)]);
    let out = output(node.as_mut(), CYCLE, 512);
    // At the grid points, where the output is a worked-out value.
    let mut levels: Vec<f32> = out.iter().step_by(32).copied().collect();
    levels.sort_by(f32::total_cmp);
    levels.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    assert_eq!(levels.len(), 5, "five steps: {levels:?}");
}

// Spec: modulation R22
#[test]
fn a_synced_lfo_locks_to_the_song_position() {
    // At 120 BPM a quarter note is 24,000 frames.
    let synced = |position: i64| {
        let mut node = wave(&[
            (WAVE, 1.0),
            (TIME_MODE, 1.0),
            (RATE_BEATS, 3.0),
            (PHASE, 0.0),
        ]);
        control(
            node.as_mut(),
            256,
            64,
            &[],
            &TempoMap::default(),
            playing(position),
        )
        .contribution(0)
        .to_vec()
    };
    let here = synced(1_000);
    assert!(
        here == synced(1_000 + 24_000),
        "a beat later, the same place in the cycle"
    );
    assert!(here == synced(1_000 + 5 * 24_000));
    assert!(here != synced(1_000 + 12_000), "half a beat on, another");
    // At 80 BPM a beat is longer; the cycle follows it.
    let mut node = wave(&[(WAVE, 1.0), (TIME_MODE, 1.0), (RATE_BEATS, 3.0)]);
    let slow = control(
        node.as_mut(),
        256,
        64,
        &[],
        &TempoMap::constant(bl_core::time::Bpm::new(80.0).unwrap()),
        playing(1_000),
    )
    .contribution(0)
    .to_vec();
    assert!(here != slow);
}

// Spec: modulation R20
#[test]
fn a_synced_lfo_holds_still_while_the_song_is_stopped() {
    let mut node = wave(&[(WAVE, 1.0), (TIME_MODE, 1.0), (RATE_BEATS, 3.0)]);
    let stopped = TransportInfo {
        playing: false,
        position: Frames(1_000),
    };
    let out = control(
        node.as_mut(),
        1_024,
        512,
        &[],
        &TempoMap::default(),
        stopped,
    )
    .contribution(0)
    .to_vec();
    assert!(
        out.iter().all(|&v| v == out[0]),
        "it stays where the song is"
    );
}

// Spec: modulation R22
#[test]
fn hold_freezes_and_r_restarts_from_phase() {
    // Hold: frozen while on, and on from where it stopped when released.
    let mut node = wave(&[(WAVE, 1.0), (RATE_HZ, 10.0)]);
    let events = [set(1_000, HOLD, 1.0), set(2_000, HOLD, 0.0)];
    let out = control(
        node.as_mut(),
        3_000,
        512,
        &events,
        &TempoMap::default(),
        playing(0),
    )
    .contribution(0)
    .to_vec();
    // The ramp worked out for the grid point after the event holds.
    let held = out[1_100];
    assert!(
        out[1_100..2_000].iter().all(|&v| (v - held).abs() < 1e-6),
        "frozen while held"
    );
    assert!(
        out[2_300] > held && out[2_300] - held < 0.1,
        "carries on from the held value"
    );

    // R: turning it on starts the cycle again from Phase.
    let mut node = wave(&[(WAVE, 1.0), (RATE_HZ, 10.0), (PHASE, 25.0)]);
    let events = [set(2_000, RESTART, 1.0)];
    let out = control(
        node.as_mut(),
        3_000,
        512,
        &events,
        &TempoMap::default(),
        playing(0),
    )
    .contribution(0)
    .to_vec();
    assert!((out[0] - 0.25).abs() < 0.01, "starts at Phase: {}", out[0]);
    assert!(out[1_990] > 0.6, "ran on: {}", out[1_990]);
    assert!(
        (out[2_100] - 0.25).abs() < 0.05,
        "back at Phase after R: {}",
        out[2_100]
    );
}

// Spec: modulation R21
#[test]
fn random_parts_repeat_from_the_transport_start() {
    let play = |node: &mut dyn Node| {
        control(node, 3 * CYCLE, 512, &[], &TempoMap::default(), playing(0))
            .contribution(0)
            .to_vec()
    };
    let stopped = TransportInfo::default();
    for wave_index in [5.0, 6.0, 7.0, 8.0] {
        let mut node = wave(&[(WAVE, wave_index), (bl_devices::modulator::JITTER, 30.0)]);
        // The engine runs a while stopped, then the transport starts.
        control(
            node.as_mut(),
            1_000,
            256,
            &[],
            &TempoMap::default(),
            stopped,
        );
        let first = play(node.as_mut());
        // It stops and starts again: the same values from the start.
        control(node.as_mut(), 700, 256, &[], &TempoMap::default(), stopped);
        let second = play(node.as_mut());
        assert!(first == second, "{} repeats", WAVES[wave_index as usize]);
        // Another device draws other values.
        let mut other = wave(&[(WAVE, wave_index), (bl_devices::modulator::JITTER, 30.0)]);
        let _ = &mut other;
    }
}
