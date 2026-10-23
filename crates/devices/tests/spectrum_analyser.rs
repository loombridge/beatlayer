//! Spectrum analyser acceptance tests under the real-time allocation trap.

use crate::common;
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
        round_trip_latency: 0,
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

// Spec: spectrum-analyser R1
#[test]
fn renders_are_bit_identical_at_block_sizes_1_64_512_and_4096() {
    let input = common::noise(16_384);
    let mut renders = Vec::new();
    for block in [1usize, 64, 512, 4096] {
        let mut node = make_node(
            &[(BLOCK, 5.0), (CHANNEL, 2.0), (REFRESH, 20.0), (AVG, 4.0)],
            block,
        );
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            let (left, _) = process_stereo(&mut node, chunk, chunk);
            out.extend_from_slice(&left);
        }
        renders.push(out);
    }
    assert_eq!(
        renders[0], input,
        "the output is the input, at every block size"
    );
    for render in &renders[1..] {
        assert_eq!(render, &renders[0]);
    }
}

// Spec: spectrum-analyser R6
#[test]
fn clear_max_resets_accumulated_peak_hold() {
    let block_size = 1024;
    let mut node = make_node(
        &[(BLOCK, 2.0), (MAX, 1.0), (REFRESH, 10.0), (CLEAR_MAX, 0.0)],
        block_size,
    );
    let bin_k = 16;
    let hz = bin_k as f64 * RATE.as_f64() / block_size as f64;
    let signal = sine(hz, 1.0, block_size);
    let silent = vec![0.0f32; block_size];

    // Feed signal to establish peak
    process_stereo(&mut node, &signal, &signal);
    let peak_before = node.device().max_held()[bin_k];
    assert!(peak_before.abs() < 0.1, "peak is held at 0 dB");

    // Feed silence: Max peak hold retains the peak
    for _ in 0..4 {
        process_stereo(&mut node, &silent, &silent);
    }
    assert!(
        (node.device().max_held()[bin_k] - peak_before).abs() < 1e-4,
        "peak remains held during silence"
    );

    // Trigger Clear Max via parameter event
    let clear_event = common::set(0, CLEAR_MAX, 1.0);
    process_stereo_events(&mut node, &silent, &silent, &[clear_event]);
    let peak_after = node.device().max_held()[bin_k];
    assert!(
        peak_after < -100.0,
        "Clear Max reset the held peak to the floor: {peak_after} dB"
    );
}

// Spec: spectrum-analyser R10
#[test]
fn auto_range_mode_scales_vertical_span_to_signal() {
    let desc = spectrum_type();
    let id = bl_model::DeviceId(bl_core::id::Id::new(1).unwrap());
    let mut values = bl_model::OrdMap::new();
    values.insert(bl_devices::spectrum_analyser::RANGE_MODE, 0.0); // Range
    values.insert(bl_devices::spectrum_analyser::RANGE_MIN, -100.0);
    values.insert(bl_devices::spectrum_analyser::RANGE_MAX, 6.0);

    let mut fake_bins = vec![-120.0f32; bl_devices::spectrum_analyser::DISPLAY_BINS];
    fake_bins[50] = -21.4;

    // Manual Range mode uses RANGE_MIN and RANGE_MAX directly
    let manual_display =
        bl_devices::spectrum_analyser::display(id, &desc.description, &values, Some(&fake_bins));
    let series = manual_display.series("Spectrum").unwrap();
    assert_eq!(series.y_range, [-100.0, 6.0]);

    // Auto mode scales to the peak (-21.4 dB -> ceil(-21.4 / 6.0) * 6.0 = -18.0 dB max, -90.0 dB min)
    values.insert(bl_devices::spectrum_analyser::RANGE_MODE, 1.0);
    let auto_display =
        bl_devices::spectrum_analyser::display(id, &desc.description, &values, Some(&fake_bins));
    let auto_series = auto_display.series("Spectrum").unwrap();
    assert_eq!(auto_series.y_range, [-90.0, -18.0]);

    // Silent signal in Auto mode defaults to [-120.0, 0.0]
    let silent_bins = vec![-150.0f32; bl_devices::spectrum_analyser::DISPLAY_BINS];
    let silent_display =
        bl_devices::spectrum_analyser::display(id, &desc.description, &values, Some(&silent_bins));
    let silent_series = silent_display.series("Spectrum").unwrap();
    assert_eq!(silent_series.y_range, [-120.0, 0.0]);
}
