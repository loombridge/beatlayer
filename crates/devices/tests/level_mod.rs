//! The Level Mod (WP-5.11; the modulation spec, R24): its output follows the
//! level of the signal it hears, or of its sidechain, through Gain, Rise and
//! Fall, and is held back by Delay; its audio passes untouched. Every block
//! runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Frames, TempoMap};
use bl_devices::level_mod::{
    DELAY_BEATS, DELAY_MODE, DELAY_TIME, FALL, GAIN, RISE, level_mod_type,
};
use bl_engine::{Node, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::modulator::{self, POLARITY};
use common::{Signal, device, node, signal};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::level_mod::register(&mut registry);
    registry
}

fn playing(at: usize) -> TransportInfo {
    TransportInfo {
        playing: true,
        position: Frames(at as i64),
    }
}

/// A Level Mod whose slot 1 gives its output itself (unipolar, full amount
/// and depth), with Rise and Fall at 0 so it follows at once; `params` over
/// those.
fn follower(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![
        (modulator::amount(0), 100.0),
        (POLARITY, 1.0),
        (RISE, 0.0),
        (FALL, 0.0),
    ];
    all.extend_from_slice(params);
    node(&registry(), &device(&level_mod_type(), &all), 512)
}

/// A signal of `level` on both channels for `frames` frames.
fn tone(level: f32, frames: usize) -> Vec<Vec<f32>> {
    vec![vec![level; frames]; 2]
}

/// `parts` of signals one after another, each `(level, frames)`.
fn steps(parts: &[(f32, usize)]) -> Vec<Vec<f32>> {
    let one: Vec<f32> = parts
        .iter()
        .flat_map(|&(level, frames)| std::iter::repeat_n(level, frames))
        .collect();
    vec![one.clone(), one]
}

fn run(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    input: &[Vec<f32>],
    sidechain: Option<&[Vec<f32>]>,
) -> Signal {
    signal(
        node,
        frames,
        block,
        input,
        sidechain,
        &[],
        &[],
        &TempoMap::default(),
        playing,
    )
}

// Spec: modulation R24
#[test]
fn the_output_follows_the_input_level() {
    let input = steps(&[(0.0, 4_096), (0.5, 4_096), (0.25, 4_096), (0.0, 4_096)]);
    let mut node = follower(&[]);
    let out = run(node.as_mut(), 16_384, 512, &input, None);
    let level = out.rows.contribution(0);
    let at = |frame: usize| level[frame];
    assert!(at(4_000) < 1e-4, "silence gives nothing: {}", at(4_000));
    assert!((at(6_000) - 0.5).abs() < 1e-3, "half scale: {}", at(6_000));
    assert!(
        (at(10_000) - 0.25).abs() < 1e-3,
        "a quarter: {}",
        at(10_000)
    );
    assert!(at(16_000) < 1e-4, "and silence again: {}", at(16_000));
    assert!(
        level.iter().all(|v| (0.0..=1.0).contains(v)),
        "the output keeps to 0 to 1"
    );

    // Gain scales the input before the detector, and the output stops at
    // full scale.
    let mut node = follower(&[(GAIN, 1.5)]);
    let out = run(node.as_mut(), 8_192, 512, &input, None);
    assert!((out.rows.contribution(0)[6_000] - 0.75).abs() < 1e-3);
    let mut node = follower(&[(GAIN, 4.0)]);
    let out = run(node.as_mut(), 8_192, 512, &input, None);
    assert_eq!(
        out.rows.contribution(0)[6_000],
        1.0,
        "clamped at full scale"
    );

    // The audio and the track's other signals pass untouched.
    let mut node = follower(&[]);
    let out = run(node.as_mut(), 4_096, 512, &input, None);
    assert_eq!(
        out.audio,
        input[..]
            .iter()
            .map(|c| c[..4_096].to_vec())
            .collect::<Vec<_>>()
    );
}

// Spec: modulation R24
#[test]
fn rise_and_fall_smooth_the_output_and_the_rows_are_the_same_at_any_block_size() {
    let input = steps(&[(0.0, 2_048), (1.0, 8_192), (0.0, 12_288)]);
    let params = [(RISE, 50.0), (FALL, 200.0)];
    let mut node = follower(&params);
    let out = run(node.as_mut(), 22_528, 512, &input, None);
    let level = out.rows.contribution(0);
    // One time constant after the edge the output is at 1 − 1/e of the way.
    let rise_at = 2_048 + (0.050 * 48_000.0) as usize;
    assert!(
        (level[rise_at] - (1.0 - (-1.0f32).exp())).abs() < 0.03,
        "{}",
        level[rise_at]
    );
    let fall_at = 10_240 + (0.200 * 48_000.0) as usize;
    assert!(
        (level[fall_at] - (-1.0f32).exp()).abs() < 0.03,
        "{}",
        level[fall_at]
    );
    assert!(
        level[2_048 + 480] < level[10_240 + 480],
        "it rises faster than it falls"
    );

    for block in [1, 32, 64] {
        let mut again = follower(&params);
        let other = run(again.as_mut(), 22_528, block, &input, None);
        assert_eq!(
            other.rows.0, out.rows.0,
            "blocks of {block} frames give the same rows"
        );
    }
}

// Spec: modulation R24
#[test]
fn sidechain_mix_crossfades_the_two_inputs() {
    // The engine turns the sidechain's source and mix into the signal the
    // detector hears (its sidechain spec); a device that follows it hears
    // that instead of its own input.
    let own = tone(0.2, 8_192);
    let heard = tone(0.6, 8_192);
    let mut node = follower(&[]);
    let alone = run(node.as_mut(), 8_192, 512, &own, None);
    assert!((alone.rows.contribution(0)[6_000] - 0.2).abs() < 1e-3);
    let mut node = follower(&[]);
    let keyed = run(node.as_mut(), 8_192, 512, &own, Some(&heard));
    assert!(
        (keyed.rows.contribution(0)[6_000] - 0.6).abs() < 1e-3,
        "{}",
        keyed.rows.contribution(0)[6_000]
    );
    assert_eq!(keyed.audio[0], own[0], "its own audio still passes");
}

// Spec: modulation R24
#[test]
fn delay_holds_the_output_back() {
    let input = steps(&[(0.0, 2_048), (1.0, 8_192)]);
    let first_above = |out: &Signal| {
        out.rows
            .contribution(0)
            .iter()
            .position(|&v| v > 0.5)
            .expect("the output rises")
    };
    let mut node = follower(&[]);
    let plain = first_above(&run(node.as_mut(), 10_240, 512, &input, None));
    // A time of 20 ms is 960 frames.
    let mut node = follower(&[(DELAY_TIME, 20.0)]);
    let timed = first_above(&run(node.as_mut(), 10_240, 512, &input, None));
    assert_eq!(timed - plain, 960, "{plain} then {timed}");
    // A beat division, at 120 BPM: a 1/16 is 6,000 frames.
    let mut node = follower(&[(DELAY_MODE, 1.0), (DELAY_BEATS, 1.0)]);
    let beats = first_above(&run(node.as_mut(), 10_240, 512, &input, None));
    assert!(
        (beats - plain).abs_diff(6_000) <= 32,
        "{plain} then {beats}"
    );
}

// Spec: modulation R28
#[test]
fn the_late_rows_hold_what_was_written_32_frames_before() {
    let input = steps(&[(0.0, 1_024), (0.8, 3_072)]);
    let mut node = follower(&[]);
    let out = run(node.as_mut(), 4_096, 32, &input, None);
    let (now, late) = (out.rows.contribution(0), out.late.contribution(0));
    assert!(late[..32].iter().all(|&v| v == 0.0), "idle at first");
    for frame in 32..4_096 {
        assert_eq!(late[frame], now[frame - 32], "frame {frame}");
    }
    let (hold_now, hold_late) = (out.rows.hold(0), out.late.hold(0));
    assert!(
        hold_late[..32]
            .iter()
            .all(|&v| v == bl_engine::modbus::NOT_HELD)
    );
    assert_eq!(hold_late[100], hold_now[68]);
}
