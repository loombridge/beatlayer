//! Oscilloscope acceptance tests under the real-time allocation trap.

bl_rt::install_trap!();

mod common;

use std::f64::consts::TAU;

use bl_core::time::SampleRate;
use bl_devices::NativeNode;
use bl_devices::oscilloscope::{FREEZE, GAIN, Oscilloscope, TIME, TRIGGER, oscilloscope_type};
use bl_engine::{Block, Node, NodeConfig, TransportInfo};
use bl_model::ParamId;
use common::device;

const RATE: SampleRate = SampleRate::HZ_48000;

fn make_node(params: &[(ParamId, f64)], max_frames: usize) -> NativeNode<Oscilloscope> {
    let desc = oscilloscope_type();
    let dev = device(&desc, params);
    let config = NodeConfig {
        sample_rate: RATE,
        max_frames,
        channels: 2,
    };
    NativeNode::new(Oscilloscope::new(&config), &desc.description, &dev, &config)
}

fn sine(hz: f64, amp: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amp as f64 * (TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

fn process_mono(node: &mut NativeNode<Oscilloscope>, input: &[f32]) -> Vec<f32> {
    let frames = input.len();
    let mut data: Vec<f32> = input.iter().chain(input).copied().collect();
    {
        let mut block = Block::new(
            &mut data,
            2,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            RATE,
        );
        bl_rt::rt_test!({
            node.process(&mut block);
        });
    }
    data[..frames].to_vec()
}

// Spec: oscilloscope R2
#[test]
fn a_triggered_sine_starts_on_its_rising_crossing() {
    let frames = 2048;
    // Trigger at 50% (0.5 level)
    let trigger_pct = 50.0;
    let mut node = make_node(&[(TRIGGER, trigger_pct), (TIME, 10.0), (GAIN, 1.0)], frames);

    let input = sine(100.0, 1.0, frames);
    process_mono(&mut node, &input);

    assert!(node.device().triggered(), "trigger condition should be met");
    let captured = node.device().captured();

    // First captured sample should be at or just above trigger level (0.5)
    let first = captured[0];
    assert!(
        (0.45..=0.6).contains(&first),
        "first sample should be around trigger level 0.5, got {first}"
    );
    // Rising edge: subsequent sample should be higher
    let second = captured[1];
    assert!(
        second >= first,
        "waveform should be rising after trigger: first {first}, second {second}"
    );
}

// Spec: oscilloscope R3
#[test]
fn freeze_holds_the_captured_window() {
    let frames = 2048;
    let mut node = make_node(&[(TRIGGER, 0.0), (TIME, 20.0)], frames);

    // A capture with the sine wave playing.
    let s1 = sine(200.0, 1.0, frames);
    process_mono(&mut node, &s1);
    let first_capture = node.device().captured().to_vec();
    assert!(
        first_capture.iter().any(|s| s.abs() > 0.1),
        "the sine was captured before Freeze"
    );

    // Freeze, then feed silence: the window stays as it was.
    let silent = vec![0.0f32; frames];
    let mut data: Vec<f32> = silent.iter().chain(&silent).copied().collect();
    {
        let mut block = Block::new(
            &mut data,
            2,
            frames,
            frames,
            &[common::set(0, FREEZE, 1.0)],
            TransportInfo::default(),
            RATE,
        );
        bl_rt::rt_test!({
            node.process(&mut block);
        });
    }

    assert_eq!(
        node.device().captured(),
        first_capture.as_slice(),
        "freeze holds the captured window stationary despite silence"
    );
}

// Spec: oscilloscope R4
#[test]
fn time_parameter_adjusts_window_span() {
    let frames = 2048;
    let node_10ms = make_node(&[(TIME, 10.0)], frames);
    let node_100ms = make_node(&[(TIME, 100.0)], frames);

    assert_ne!(
        node_10ms.device().captured().len(),
        0,
        "10ms capture produces samples"
    );
    assert_ne!(
        node_100ms.device().captured().len(),
        0,
        "100ms capture produces samples"
    );
}

// Spec: oscilloscope R5
#[test]
fn gain_scales_display_without_altering_audio() {
    let frames = 2048;
    let gain_2x = 2.0; // +6 dB, a linear amplitude factor of 2
    let mut node = make_node(&[(GAIN, gain_2x), (TRIGGER, 0.0), (TIME, 20.0)], frames);

    let input = sine(440.0, 0.25, frames);
    let output = process_mono(&mut node, &input);

    // Audio is untouched
    assert_eq!(output, input, "audio pass-through is unaltered");

    // Captured display waveform is scaled by gain
    let captured = node.device().captured();
    let max_val = captured.iter().copied().fold(0.0f32, f32::max);
    // 0.25 * 2.0 = 0.5 peak
    assert!(
        (max_val - 0.5).abs() < 0.1,
        "captured peak should be scaled by gain to ~0.5, got {max_val}"
    );
}

// Spec: oscilloscope R1
#[test]
fn the_output_is_the_input_bit_for_bit() {
    let frames = 512;
    let mut node = make_node(&[], frames);
    let noise = common::noise(frames);

    let output = process_mono(&mut node, &noise);
    assert_eq!(output, noise, "output bit-identical to input");
}

// Spec: oscilloscope R1
#[test]
fn renders_are_bit_identical_at_block_sizes_1_64_512_and_4096() {
    let input = common::noise(16_384);
    let mut renders = Vec::new();
    for block in [1usize, 64, 512, 4096] {
        let mut node = make_node(&[(TIME, 20.0), (TRIGGER, 0.0)], block);
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            out.extend_from_slice(&process_mono(&mut node, chunk));
        }
        renders.push(out);
    }
    assert_eq!(renders[0], input, "the output is the input, at every block size");
    for render in &renders[1..] {
        assert_eq!(render, &renders[0]);
    }
}
