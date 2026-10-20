//! Tuner acceptance tests under the real-time allocation trap.

bl_rt::install_trap!();

mod common;

use std::f64::consts::TAU;

use bl_core::time::SampleRate;
use bl_devices::NativeNode;
use bl_devices::tuner::{
    CLASSIC_MODE, HERTZ_CENTS, REFERENCE, SPELLING_BOTH, SPELLING_FLATS, SPELLING_SHARPS, Tuner,
    tuner_type,
};
use bl_engine::{Block, Node, NodeConfig, TransportInfo};
use bl_model::ParamId;
use common::device;

const RATE: SampleRate = SampleRate::HZ_48000;

fn make_node(params: &[(ParamId, f64)], max_frames: usize) -> NativeNode<Tuner> {
    let desc = tuner_type();
    let dev = device(&desc, params);
    let config = NodeConfig {
        sample_rate: RATE,
        max_frames,
        channels: 2,
    };
    NativeNode::new(Tuner::new(&config), &desc.description, &dev, &config)
}

fn sine(hz: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (TAU * hz * i as f64 / RATE.as_f64()).sin() as f32)
        .collect()
}

fn process_mono(node: &mut NativeNode<Tuner>, input: &[f32]) -> Vec<f32> {
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

// Spec: tuner R2
#[test]
fn sines_from_41_hz_to_2_khz_read_their_note_within_one_cent() {
    let test_pitches = [
        (41.2, "E1"),
        (55.0, "A1"),
        (110.0, "A2"),
        (220.0, "A3"),
        (440.0, "A4"),
        (880.0, "A5"),
        (1760.0, "A6"),
        (1975.5, "B6"),
    ];

    for (target_hz, expected_note) in test_pitches {
        let frames = 4096;
        let mut node = make_node(&[], frames);
        let input = sine(target_hz, frames);

        process_mono(&mut node, &input);

        let detected = node
            .device()
            .detected_pitch()
            .unwrap_or_else(|| panic!("failed to detect pitch for {target_hz} Hz"));
        let cents_error = 1200.0 * (detected / target_hz).log2();
        assert!(
            cents_error.abs() < 1.0,
            "target {target_hz} Hz detected as {detected:.2} Hz (error: {cents_error:.3} cents)"
        );

        let note = node
            .device()
            .note_name()
            .unwrap_or_else(|| panic!("failed to get note name for {target_hz} Hz"));
        assert_eq!(
            note, expected_note,
            "at {target_hz} Hz expected note {expected_note}, got {note}"
        );
    }
}

// Spec: tuner R3
#[test]
fn a_432_hz_reference_moves_the_cents_reading() {
    let frames = 4096;
    let mut node = make_node(&[(REFERENCE, 432.0)], frames);
    let input = sine(440.0, frames);

    process_mono(&mut node, &input);

    let cents = node
        .device()
        .cents()
        .expect("cents deviation should be computed");

    // With A4 = 432 Hz, a 440 Hz tone is sharp:
    // semitones = 12 * log2(440 / 432) ≈ 0.31766
    // cents = 31.766 ct
    let expected_cents = 1200.0 * (440.0f64 / 432.0).log2();
    assert!(
        (cents - expected_cents).abs() < 1.0,
        "expected cents {expected_cents:.2}, got {cents:.2}"
    );
}

// Spec: tuner R4
#[test]
fn each_spelling_names_the_note_as_stated() {
    let frames = 4096;
    // C#4 / Db4 is 277.18 Hz (MIDI 61)
    let c_sharp_hz = 440.0 * 2.0f64.powf((61.0 - 69.0) / 12.0);
    let input = sine(c_sharp_hz, frames);

    // Sharps: default
    let mut node_sharps = make_node(
        &[
            (SPELLING_SHARPS, 1.0),
            (SPELLING_FLATS, 0.0),
            (SPELLING_BOTH, 0.0),
        ],
        frames,
    );
    process_mono(&mut node_sharps, &input);
    assert_eq!(node_sharps.device().note_name().as_deref(), Some("C#4"));

    // Flats
    let mut node_flats = make_node(
        &[
            (SPELLING_SHARPS, 0.0),
            (SPELLING_FLATS, 1.0),
            (SPELLING_BOTH, 0.0),
        ],
        frames,
    );
    process_mono(&mut node_flats, &input);
    assert_eq!(node_flats.device().note_name().as_deref(), Some("Db4"));

    // Both (Sharps and Flats)
    let mut node_both = make_node(
        &[
            (SPELLING_SHARPS, 0.0),
            (SPELLING_FLATS, 0.0),
            (SPELLING_BOTH, 1.0),
        ],
        frames,
    );
    process_mono(&mut node_both, &input);
    assert_eq!(node_both.device().note_name().as_deref(), Some("C#/Db4"));
}

// Spec: tuner R5
#[test]
fn views_and_modes_switch_cleanly() {
    let desc = tuner_type();
    assert_eq!(desc.description.views, vec!["Classic", "Histogram"]);
    let mut node = make_node(&[(CLASSIC_MODE, 1.0), (HERTZ_CENTS, 0.0)], 1024);
    let input = sine(440.0, 1024);
    process_mono(&mut node, &input);
    assert!(node.device().detected_pitch().is_some());
}

// Spec: tuner R1
#[test]
fn the_output_is_the_input_bit_for_bit() {
    let frames = 512;
    let mut node = make_node(&[], frames);
    let left = common::noise(frames);
    let mut data: Vec<f32> = left.iter().chain(&left).copied().collect();
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
    assert_eq!(&data[..frames], left.as_slice(), "output bit-identical");
}
