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
    let mut node = make_node(&[(CLASSIC_MODE, 1.0), (HERTZ_CENTS, 0.0)], 2048);
    let input = sine(440.0, 2048);
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

// Spec: tuner R1
#[test]
fn renders_are_bit_identical_at_block_sizes_1_64_512_and_4096() {
    let input = common::noise(16_384);
    let mut renders = Vec::new();
    for block in [1usize, 64, 512, 4096] {
        let mut node = make_node(&[(REFERENCE, 432.0)], block);
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            out.extend_from_slice(&process_mono(&mut node, chunk));
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

// Spec: tuner R2. The analysis buffer holds a whole period of the lowest
// pitch it reads at every rate the engine runs at (WP-5.28 review fix: at
// 88.2 kHz and above the old fixed 4,096-sample buffer ran off its front).
#[test]
fn a_96_khz_stream_reads_its_pitch_within_one_cent() {
    let rate = SampleRate::HZ_96000;
    let desc = tuner_type();
    let dev = device(&desc, &[(REFERENCE, 440.0)]);
    let config = NodeConfig {
        sample_rate: rate,
        max_frames: 4096,
        channels: 2,
    };
    let mut node = NativeNode::new(Tuner::new(&config), &desc.description, &dev, &config);

    let frames = 4096;
    let input: Vec<f32> = (0..frames)
        .map(|i| (TAU * 440.0 * i as f64 / rate.as_f64()).sin() as f32)
        .collect();
    for _ in 0..2 {
        let mut data: Vec<f32> = input.iter().chain(&input).copied().collect();
        {
            let mut block = Block::new(
                &mut data,
                2,
                frames,
                frames,
                &[],
                TransportInfo::default(),
                rate,
            );
            bl_rt::rt_test!({
                node.process(&mut block);
            });
        }
    }

    let detected = node.device().detected_pitch().expect("the tone is heard");
    let cents = 1200.0 * (detected / 440.0).log2();
    assert!(
        cents.abs() < 1.0,
        "440 Hz at 96 kHz reads {detected:.2} Hz ({cents:.2} cents)"
    );
    assert_eq!(node.device().note_name().as_deref(), Some("A4"));
}

// Spec: tuner R5
#[test]
fn histogram_view_shows_pitch_history_and_auto_centring() {
    let desc = tuner_type();
    let id = bl_model::DeviceId(bl_core::id::Id::new(1).unwrap());
    let mut values = bl_model::OrdMap::new();
    values.insert(bl_devices::tuner::AUTO, 1.0); // Auto centring enabled

    // Published data: [pitch, midi, cents, in_tune, valid, strobe_phase, ..history]
    let mut data = vec![0.0f32; bl_devices::tuner::TUNER_DISPLAY_LEN];
    data[0] = 440.0;
    data[1] = 69.0;
    data[2] = 5.0; // +5 cents
    data[3] = 0.0;
    data[4] = 1.0; // Valid
    data[5] = 0.25; // Strobe phase
    for i in 0..bl_devices::tuner::HIST_LEN {
        data[6 + i] = (i as f32) - 10.0;
    }

    // Histogram view with Auto centring: y_range is [-50.0, 50.0]
    let disp_auto = bl_devices::tuner::display_for_view(
        id,
        &desc.description,
        &values,
        Some(&data),
        Some("Histogram"),
    );
    assert_eq!(disp_auto.view.as_deref(), Some("Histogram"));
    let hist_series = disp_auto
        .series("Histogram")
        .expect("Histogram series exists");
    assert_eq!(hist_series.values.len(), bl_devices::tuner::HIST_LEN);
    assert_eq!(hist_series.y_range, [-50.0, 50.0]);
    assert_eq!(
        hist_series.x_range,
        [0.0, bl_devices::tuner::HIST_LEN as f32]
    );
    let pitch_marker = disp_auto
        .markers
        .iter()
        .find(|m| m.name == "Pitch")
        .expect("Pitch marker exists");
    assert_eq!(pitch_marker.y, 5.0);

    // Histogram view with Auto centring disabled: y_range is [-100.0, 100.0]
    values.insert(bl_devices::tuner::AUTO, 0.0);
    let disp_manual = bl_devices::tuner::display_for_view(
        id,
        &desc.description,
        &values,
        Some(&data),
        Some("Histogram"),
    );
    let manual_hist = disp_manual.series("Histogram").unwrap();
    assert_eq!(manual_hist.y_range, [-100.0, 100.0]);
}

// Spec: tuner R5, R6
#[test]
fn classic_strobe_mode_and_readout_chooser() {
    let desc = tuner_type();
    let id = bl_model::DeviceId(bl_core::id::Id::new(1).unwrap());
    let mut values = bl_model::OrdMap::new();
    values.insert(CLASSIC_MODE, 0.0); // Target mode
    values.insert(HERTZ_CENTS, 0.0); // Hz readout

    let mut data = vec![0.0f32; bl_devices::tuner::TUNER_DISPLAY_LEN];
    data[0] = 442.5;
    data[1] = 69.0;
    data[2] = 9.8; // +9.8 cents
    data[3] = 0.0;
    data[4] = 1.0; // Valid
    data[5] = 0.6; // Strobe phase

    // Target mode with Hz readout
    let disp_target = bl_devices::tuner::display_for_view(
        id,
        &desc.description,
        &values,
        Some(&data),
        Some("Classic"),
    );
    assert!(disp_target.series("Tuning").is_some());
    assert!(disp_target.series("Strobe").is_none());
    let needle = disp_target
        .markers
        .iter()
        .find(|m| m.name == "Needle")
        .unwrap();
    assert_eq!(needle.text, "A4 442.5 Hz");
    assert_eq!(disp_target.readings[1].name, "Pitch"); // Primary readout after note is Pitch

    // Strobe mode with Cents readout
    values.insert(CLASSIC_MODE, 1.0); // Strobe mode
    values.insert(HERTZ_CENTS, 1.0); // Cents readout
    let disp_strobe = bl_devices::tuner::display_for_view(
        id,
        &desc.description,
        &values,
        Some(&data),
        Some("Classic"),
    );
    assert!(disp_strobe.series("Strobe").is_some());
    assert!(disp_strobe.series("Tuning").is_none());
    let strobe_reading = disp_strobe.reading("Strobe").unwrap();
    assert_eq!(strobe_reading.text, "Rotating Right (Sharp)");
    let strobe_marker = disp_strobe
        .markers
        .iter()
        .find(|m| m.name == "Strobe")
        .unwrap();
    assert!(strobe_marker.text.contains("A4 +9.8 ct"));
    assert!(strobe_marker.text.contains("Rotating Right (Sharp)"));
    assert_eq!(disp_strobe.readings[1].name, "Cents"); // Primary readout after note is Cents
}
