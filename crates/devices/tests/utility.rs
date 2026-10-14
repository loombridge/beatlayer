//! The Utility device (WP-2.18b; the utility spec): gain, channel modes,
//! width, mono, bass mono, phase invert per side, balance, the DC filter,
//! and mute.
//!
//! Every block runs as a real-time section under the allocation trap
//! ([`install_trap`](bl_rt)), so the audio thread's rules are checked here
//! too.

bl_rt::install_trap!();

mod common;

use bl_devices::utility::{
    BALANCE, BASS_MONO, BASS_MONO_FREQUENCY, CHANNEL_MODE, DC, GAIN, MONO, MUTE, PHASE_LEFT,
    PHASE_RIGHT, WIDTH, utility_type,
};
use bl_engine::{Block, Node, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::param::{MINUS_INFINITY_DB, db_to_gain};

use common::{CHANNELS, RATE, device, node};

/// Runs `params`' Utility over `left` and `right` and returns each channel.
fn run(params: &[(ParamId, f64)], left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let mut made = node(&registry, &device(&utility_type(), params), left.len());
    run_stereo(made.as_mut(), left, right)
}

/// One block with a different signal on each channel, under the allocation
/// trap, returning each channel.
fn run_stereo(node: &mut dyn Node, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
    assert_eq!(left.len(), right.len());
    let frames = left.len();
    let mut data: Vec<f32> = left.iter().chain(right).copied().collect();
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            RATE,
        );
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    (data[..frames].to_vec(), data[frames..].to_vec())
}

/// A sine at `hz`, amplitude 1.
fn sine(hz: f32, frames: usize) -> Vec<f32> {
    let step = core::f32::consts::TAU * hz / RATE.as_f64() as f32;
    (0..frames).map(|i| (i as f32 * step).sin()).collect()
}

/// A signal's root mean square.
fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Asserts one frame's two channels, to the last bit of the arithmetic's
/// rounding.
fn assert_frame(left: f32, right: f32, want: (f32, f32)) {
    assert!(
        (left - want.0).abs() < 1e-6 && (right - want.1).abs() < 1e-6,
        "({left}, {right}) isn't {want:?}"
    );
}

// Spec: utility R7 (rule G1)
#[test]
fn the_gain_scales_by_db_to_the_hundredth_and_reaches_silence() {
    for db in [6.0, -6.0, -18.0, 35.0] {
        let (left, right) = run(&[(GAIN, db_to_gain(db))], &[0.5; 512], &[0.25; 512]);
        for (out, input) in [(left, 0.5f32), (right, 0.25f32)] {
            let measured = 20.0 * f64::from(out[511] / input).log10();
            assert!((measured - db).abs() < 0.01, "{db} dB: {measured} dB");
        }
    }
    let (left, right) = run(&[(GAIN, 0.0)], &[1.0; 64], &[1.0; 64]);
    assert!(left.iter().all(|s| *s == 0.0), "-inf dB is silence");
    assert!(right.iter().all(|s| *s == 0.0));
}

// Spec: utility R1 (rules G1 and G2)
#[test]
fn every_parameter_is_automatable_and_the_gain_reaches_silence() {
    let description = utility_type().description;
    description.check().unwrap();
    for param in &description.params {
        assert!(param.automatable, "{}", param.name);
    }
    let gain = description.param(GAIN).expect("the gain");
    assert!(gain.is_gain());
    assert_eq!(gain.min(), 0.0);
    assert_eq!(gain.format(0.0), MINUS_INFINITY_DB);
}

// Spec: utility R2
#[test]
fn each_phase_switch_inverts_only_its_channel() {
    let (left, right) = (vec![0.6f32; 32], vec![-0.4f32; 32]);
    let (l, r) = run(&[(PHASE_LEFT, 1.0)], &left, &right);
    assert_frame(l[0], r[0], (-0.6, -0.4));
    let (l, r) = run(&[(PHASE_RIGHT, 1.0)], &left, &right);
    assert_frame(l[0], r[0], (0.6, 0.4));
    let (l, r) = run(&[(PHASE_LEFT, 1.0), (PHASE_RIGHT, 1.0)], &left, &right);
    assert_frame(l[0], r[0], (-0.6, 0.4));
}

// Spec: utility R3
#[test]
fn the_channel_modes_pick_the_input_channels() {
    let (left, right) = (vec![1.0f32; 32], vec![2.0f32; 32]);
    let (l, r) = run(&[(CHANNEL_MODE, 1.0)], &left, &right);
    assert_eq!((l[0], r[0]), (1.0, 1.0), "Left on both outputs");
    let (l, r) = run(&[(CHANNEL_MODE, 2.0)], &left, &right);
    assert_eq!((l[0], r[0]), (2.0, 2.0), "Right on both outputs");
    let (l, r) = run(&[(CHANNEL_MODE, 3.0)], &left, &right);
    assert_eq!((l[0], r[0]), (2.0, 1.0), "Swap");
    // Width has no function once one channel is chosen.
    let (l, r) = run(&[(CHANNEL_MODE, 1.0), (WIDTH, 0.0)], &left, &right);
    assert_eq!((l[0], r[0]), (1.0, 1.0));
}

// Spec: utility R4
#[test]
fn width_zero_is_mono_and_two_hundred_doubles_the_side() {
    // A mid of 1.0 and a side of 0.5: left 1.5, right 0.5.
    let (left, right) = (vec![1.5f32; 64], vec![0.5f32; 64]);
    let (l, r) = run(&[(WIDTH, 0.0)], &left, &right);
    assert!(l.iter().all(|s| (*s - 1.0).abs() < 1e-6), "the mid alone");
    assert!(r.iter().all(|s| (*s - 1.0).abs() < 1e-6));
    let (l, r) = run(&[(WIDTH, 200.0)], &left, &right);
    assert!(
        l.iter().all(|s| (*s - 2.0).abs() < 1e-6),
        "mid plus twice the side"
    );
    assert!(r.iter().all(|s| s.abs() < 1e-6), "mid less twice the side");
    let (l, r) = run(&[(WIDTH, 100.0)], &left, &right);
    assert_frame(l[0], r[0], (1.5, 0.5));
}

// Spec: utility R5
#[test]
fn the_mono_switch_converts_the_input_to_mono() {
    let (l, r) = run(&[(MONO, 1.0)], &[1.0f32; 32], &[2.0f32; 32]);
    assert!(l.iter().all(|s| (*s - 1.5).abs() < 1e-6));
    assert_eq!(l, r);
}

// Spec: utility R6
#[test]
fn bass_mono_removes_only_the_side_below_its_frequency() {
    let frames = 16_384;
    let params = [(BASS_MONO, 1.0), (BASS_MONO_FREQUENCY, 500.0)];
    let side = |x: &[f32]| {
        let inverted: Vec<f32> = x.iter().map(|s| -s).collect();
        run(&params, x, &inverted)
    };
    let half = frames / 2;
    let low = sine(50.0, frames);
    let (l, _) = side(&low);
    let removed = rms(&l[half..]) / rms(&low[half..]);
    assert!(removed < 0.2, "50 Hz below the cut is gone: {removed}");
    let high = sine(5000.0, frames);
    let (l, _) = side(&high);
    let kept = rms(&l[half..]) / rms(&high[half..]);
    assert!(kept > 0.9, "5 kHz above the cut is kept: {kept}");
    // The mid is untouched: a common signal has no side to remove.
    let common = sine(50.0, frames);
    let (l, _) = run(&params, &common, &common);
    assert!((rms(&l[half..]) / rms(&common[half..]) - 1.0).abs() < 0.02);
}

// Spec: utility R8
#[test]
fn balance_pans_and_mute_silences() {
    let (left, right) = (vec![1.0f32; 32], vec![1.0f32; 32]);
    let (l, r) = run(&[(BALANCE, -50.0)], &left, &right);
    assert!((l[0] - 1.0).abs() < 1e-6 && r[0].abs() < 1e-6, "hard left");
    let (l, r) = run(&[(BALANCE, 50.0)], &left, &right);
    assert!(l[0].abs() < 1e-6 && (r[0] - 1.0).abs() < 1e-6, "hard right");
    let (l, r) = run(&[(MUTE, 1.0)], &left, &right);
    assert!(l.iter().all(|s| *s == 0.0) && r.iter().all(|s| *s == 0.0));
}

// Spec: utility R9
#[test]
fn the_dc_filter_removes_a_dc_offset() {
    let frames = 48_000;
    let (l, r) = run(&[(DC, 1.0)], &[0.5; 48_000], &[0.5; 48_000]);
    assert!(
        l[frames - 1].abs() < 1e-3,
        "the offset is gone: {}",
        l[frames - 1]
    );
    assert!(r[frames - 1].abs() < 1e-3);
    assert!(l.iter().all(|s| *s <= 0.5 + 1e-6), "never above the input");
    // Without it the offset stays.
    let (l, _) = run(&[], &[0.5; 48_000], &[0.5; 48_000]);
    assert!((l[frames - 1] - 0.5).abs() < 1e-6);
}
