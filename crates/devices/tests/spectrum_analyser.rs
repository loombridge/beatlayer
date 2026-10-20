//! Spectrum analyser acceptance tests under the real-time allocation trap.

bl_rt::install_trap!();

mod common;

use std::f64::consts::TAU;

use bl_core::time::SampleRate;
use bl_devices::NativeNode;
use bl_devices::spectrum_analyser::{
    AVG, BLOCK, BLOCK_SIZES, CHANNEL, CLEAR_MAX, MAX, REFRESH, Spectrum, spectrum_type,
};
use bl_engine::{Block, Node, NodeConfig, NodeEvent, TransportInfo};
use bl_model::ParamId;
use common::device;

const RATE: SampleRate = SampleRate::HZ_48000;

fn make_node(params: &[(ParamId, f64)], max_frames: usize) -> NativeNode<Spectrum> {
    let desc = spectrum_type();
    let dev = device(&desc, params);
    let config = NodeConfig {
        sample_rate: RATE,
        max_frames,
        channels: 2,
    };
    NativeNode::new(Spectrum::new(&config), &desc.description, &dev, &config)
}

fn sine(hz: f64, amp: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amp as f64 * (TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

fn process_stereo(
    node: &mut NativeNode<Spectrum>,
    left: &[f32],
    right: &[f32],
) -> (Vec<f32>, Vec<f32>) {
    process_stereo_events(node, left, right, &[])
}

fn process_stereo_events(
    node: &mut NativeNode<Spectrum>,
    left: &[f32],
    right: &[f32],
    events: &[NodeEvent],
) -> (Vec<f32>, Vec<f32>) {
    let frames = left.len();
    let mut data: Vec<f32> = left.iter().chain(right).copied().collect();
    {
        let mut block = Block::new(
            &mut data,
            2,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        );
        bl_rt::rt_test!({
            node.process(&mut block);
        });
    }
    let (l, r) = data.split_at(frames);
    (l.to_vec(), r.to_vec())
}

// Spec: spectrum-analyser R2
#[test]
fn a_full_scale_sine_reads_0_db_within_point_one_db_at_every_block_size() {
    for (block_idx, &block_size) in BLOCK_SIZES.iter().enumerate() {
        let mut node = make_node(
            &[(BLOCK, block_idx as f64), (REFRESH, 10.0), (AVG, 1.0)],
            block_size,
        );
        // Place sine at bin 16
        let bin_k = 16.min(block_size / 4);
        let hz = bin_k as f64 * RATE.as_f64() / block_size as f64;
        let input = sine(hz, 1.0, block_size);

        process_stereo(&mut node, &input, &input);

        let bins = node.device().bins();
        let reading = bins[bin_k];
        assert!(
            reading.abs() < 0.1,
            "block size {block_size} at bin {bin_k} ({hz:.1} Hz): reading is {reading:.3} dB, expected 0.0 ± 0.1 dB"
        );
    }
}

// Spec: spectrum-analyser R3
#[test]
fn the_bins_are_the_rate_over_block_apart() {
    for (block_idx, &block_size) in BLOCK_SIZES.iter().enumerate() {
        let mut node = make_node(&[(BLOCK, block_idx as f64)], block_size);
        let silent = vec![0.0f32; block_size];
        process_stereo(&mut node, &silent, &silent);
        let expected_spacing = RATE.as_f64() / block_size as f64;
        let spacing = node.device().bin_spacing();
        assert!(
            (spacing - expected_spacing).abs() < 1e-6,
            "expected spacing {expected_spacing}, got {spacing}"
        );
        assert_eq!(
            node.device().bins().len(),
            block_size / 2,
            "number of bins is block / 2"
        );
    }
}

// Spec: spectrum-analyser R5
#[test]
fn avg_n_averages_n_blocks() {
    let block_size = 1024;
    let mut node = make_node(&[(BLOCK, 2.0), (REFRESH, 10.0), (AVG, 2.0)], block_size);

    let bin_k = 10;
    let hz = bin_k as f64 * RATE.as_f64() / block_size as f64;

    // Block 1: amplitude 1.0 (0 dB)
    let s1 = sine(hz, 1.0, block_size);
    process_stereo(&mut node, &s1, &s1);

    // Block 2: amplitude 0.5 (-6.02 dB)
    let s2 = sine(hz, 0.5, block_size);
    process_stereo(&mut node, &s2, &s2);

    let bins = node.device().bins();
    let db = bins[bin_k];
    // Expected RMS power average of 1.0 and 0.5:
    // mean_power = (1.0^2 + 0.5^2) / 2 = 1.25 / 2 = 0.625
    // amp = sqrt(0.625) ≈ 0.79057 => 20 log10(0.79057) ≈ -2.04 dB
    let expected_db = 20.0 * (0.625f64).sqrt().log10() as f32;
    assert!(
        (db - expected_db).abs() < 0.2,
        "averaged db {db} vs expected {expected_db}"
    );
}

// Spec: spectrum-analyser R6
#[test]
fn max_holds_the_highest_reading_until_cleared() {
    let block_size = 1024;
    let mut node = make_node(&[(BLOCK, 2.0), (REFRESH, 10.0), (MAX, 1.0)], block_size);

    let bin_k = 12;
    let hz = bin_k as f64 * RATE.as_f64() / block_size as f64;
    let loud = sine(hz, 1.0, block_size);
    process_stereo(&mut node, &loud, &loud);

    let held_peak = node.device().max_held()[bin_k];
    assert!(held_peak.abs() < 0.1);

    // Run silent blocks
    let silent = vec![0.0f32; block_size];
    for _ in 0..3 {
        process_stereo(&mut node, &silent, &silent);
    }
    // Max still holds the peak
    assert!(
        (node.device().max_held()[bin_k] - held_peak).abs() < 1e-4,
        "peak hold retained after silence"
    );

    // Clear max via parameter event
    process_stereo_events(
        &mut node,
        &silent,
        &silent,
        &[common::set(0, CLEAR_MAX, 1.0)],
    );
    assert!(
        node.device().max_held()[bin_k] < -100.0,
        "peak hold cleared"
    );
}

// Spec: spectrum-analyser R4
#[test]
fn each_channel_choice_reads_its_channel() {
    let block_size = 1024;
    let bin_k = 14;
    let hz = bin_k as f64 * RATE.as_f64() / block_size as f64;
    let signal = sine(hz, 1.0, block_size);
    let silent = vec![0.0f32; block_size];

    // Left channel only (CHANNEL = 0)
    let mut node_l = make_node(&[(BLOCK, 2.0), (CHANNEL, 0.0), (REFRESH, 10.0)], block_size);
    // Put signal on right, silence on left
    process_stereo(&mut node_l, &silent, &signal);
    assert!(
        node_l.device().bins()[bin_k] < -100.0,
        "left mode ignores right input"
    );
    // Put signal on left
    process_stereo(&mut node_l, &signal, &silent);
    assert!(
        node_l.device().bins()[bin_k].abs() < 0.1,
        "left mode reads left input"
    );

    // Right channel only (CHANNEL = 1)
    let mut node_r = make_node(&[(BLOCK, 2.0), (CHANNEL, 1.0), (REFRESH, 10.0)], block_size);
    process_stereo(&mut node_r, &signal, &silent);
    assert!(
        node_r.device().bins()[bin_k] < -100.0,
        "right mode ignores left input"
    );
    process_stereo(&mut node_r, &silent, &signal);
    assert!(
        node_r.device().bins()[bin_k].abs() < 0.1,
        "right mode reads right input"
    );
}

// Spec: spectrum-analyser R1
#[test]
fn the_output_is_the_input_bit_for_bit() {
    let frames = 512;
    let mut node = make_node(&[], frames);
    let left = common::noise(frames);
    let right = common::noise(frames);

    let (out_l, out_r) = process_stereo(&mut node, &left, &right);
    assert_eq!(out_l, left, "left channel bit-identical");
    assert_eq!(out_r, right, "right channel bit-identical");
}
