//! Strip, the native channel EQ (WP-5.21; the channel-eq spec): its
//! sections against the cookbook curves, the high-pass's corner, the
//! shelf's plateaus and changing shape, the mid peak's gain across its
//! sweep, the high band's shelf and low-pass, Output, and the display.

bl_rt::install_trap!();

mod common;

use bl_devices::NativeNode;
use bl_devices::channel_eq::{
    CHANNEL_EQ, HIGH, HIGH_PASS, HIGH_SHELF_HZ, LOW, LOW_SHELF_HZ, MID, MID_FREQ, MID_Q, OUTPUT,
    Strip, channel_eq_type, low_pass_hz, low_shelf_q,
};
use bl_engine::{DeviceType, Node, Registry};
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::response::{
    Kind, biquad_response, crossing, db, impulse_response, mul, rbj, response_at,
};
use common::{config, device, node, noise, run, set};

const FRAMES: usize = 16_384;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::channel_eq::register(&mut registry);
    registry
}

fn strip(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = channel_eq_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// The Strip's response at `hz`, in dB, with `params` set.
fn response_db(params: &[(ParamId, f64)], hz: f64) -> f64 {
    let ir = impulse_response(strip(params, FRAMES).as_mut(), FRAMES);
    db(response_at(&ir, hz))
}

/// The cookbook curve's size at `hz`, in dB.
fn curve_db(curve: common::response::Biquad, hz: f64) -> f64 {
    db(biquad_response(curve, hz))
}

// Spec: channel-eq R1
#[test]
fn the_channel_eq_is_described_with_its_ranges_and_defaults() {
    let description = channel_eq_type().description;
    description.check().unwrap();
    let range = |id: ParamId| {
        let p = description.param(id).unwrap();
        (p.min(), p.max(), p.default)
    };
    assert_eq!(range(LOW), (-15.0, 15.0, 0.0));
    assert_eq!(range(MID), (-12.0, 12.0, 0.0));
    assert_eq!(range(HIGH), (-15.0, 15.0, 0.0));
    let (low, high, _) = range(MID_FREQ);
    assert_eq!((low, high), (120.0, 7_500.0));
    assert_eq!(range(HIGH_PASS).2, 0.0, "the high-pass starts off");
    assert_eq!(range(OUTPUT).2, 1.0, "Output starts at 0 dB");
}

// Spec: channel-eq R2
#[test]
fn flat_with_the_high_pass_off_nulls_against_the_input() {
    let input = noise(4096);
    let out = run(strip(&[], 4096).as_mut(), &input, &[]);
    let worst = out[0]
        .iter()
        .zip(&input)
        .map(|(o, i)| (o - i).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-6, "{worst}");
    assert!(20.0 * f64::from(worst.max(1e-12)).log10() < -120.0);
}

// Spec: channel-eq R3
#[test]
fn the_high_pass_is_3_db_down_at_80_hz() {
    let ir = impulse_response(strip(&[(HIGH_PASS, 1.0)], FRAMES).as_mut(), FRAMES);
    let level = |hz: f64| db(response_at(&ir, hz));
    // The level rises through -3 dB, so the search looks for the negated
    // level falling through +3 dB.
    let corner = crossing(20.0, 400.0, 3.0103, |hz| -level(hz));
    assert!((corner / 80.0 - 1.0).abs() < 0.02, "{corner} Hz");
    // 12 dB per octave below it, and flat well above it.
    assert!((level(20.0) - level(40.0) + 12.0).abs() < 1.5);
    assert!(level(2_000.0).abs() < 0.05);
}

// Spec: channel-eq R4
#[test]
fn the_low_shelf_follows_its_curve_and_reaches_its_plateaus() {
    for gain in [15.0, 7.5, -7.5, -15.0] {
        let ir = impulse_response(strip(&[(LOW, gain)], FRAMES).as_mut(), FRAMES);
        let want = rbj(Kind::LowShelf, 100.0, gain, 0.5 + 0.21 * gain.abs() / 15.0);
        for hz in [10.0, 20.0, 60.0, 100.0, 250.0, 1_000.0] {
            let (got, expected) = (db(response_at(&ir, hz)), curve_db(want, hz));
            assert!(
                (got - expected).abs() < 0.1,
                "{gain} dB at {hz} Hz: {got} against {expected}"
            );
        }
    }
    for gain in [15.0, -15.0] {
        assert!(
            (response_db(&[(LOW, gain)], 10.0) - gain).abs() < 0.1,
            "{gain} dB"
        );
        assert!(response_db(&[(LOW, gain)], 5_000.0).abs() < 0.1);
    }
    assert_eq!(LOW_SHELF_HZ, 100.0);
}

// Spec: channel-eq R4
#[test]
fn the_low_shelfs_shape_changes_with_its_gain() {
    // The same fraction of the gain is reached at 200 Hz for a small boost
    // and a large one only if the shape stays; it doesn't.
    let share = |gain: f64| response_db(&[(LOW, gain)], 200.0) / gain;
    assert!((share(15.0) - share(4.0)).abs() > 0.02);
    assert!(low_shelf_q(0.0) < low_shelf_q(15.0) && low_shelf_q(-15.0) == low_shelf_q(15.0));
}

// Spec: channel-eq R5
#[test]
fn the_mid_peak_reaches_its_gain_across_its_sweep() {
    for hz in [120.0, 1_000.0, 7_500.0] {
        for gain in [12.0, -12.0, 5.0] {
            let ir = impulse_response(
                strip(&[(MID, gain), (MID_FREQ, hz)], FRAMES).as_mut(),
                FRAMES,
            );
            let at_centre = db(response_at(&ir, hz));
            assert!(
                (at_centre - gain).abs() < 0.1,
                "{gain} dB at {hz} Hz: {at_centre}"
            );
            let want = rbj(Kind::Peak, hz, gain, MID_Q);
            for probe in [hz / 2.0, hz * 1.5] {
                let (got, expected) = (db(response_at(&ir, probe)), curve_db(want, probe));
                assert!(
                    (got - expected).abs() < 0.1,
                    "{hz} Hz, at {probe}: {got} against {expected}"
                );
            }
        }
    }
}

// Spec: channel-eq R6
#[test]
fn a_boosting_high_band_is_a_shelf() {
    for gain in [15.0, 6.0] {
        let ir = impulse_response(strip(&[(HIGH, gain)], FRAMES).as_mut(), FRAMES);
        let want = rbj(
            Kind::HighShelf,
            HIGH_SHELF_HZ,
            gain,
            std::f64::consts::FRAC_1_SQRT_2,
        );
        for hz in [500.0, 3_000.0, 6_000.0, 12_000.0, 20_000.0] {
            let (got, expected) = (db(response_at(&ir, hz)), curve_db(want, hz));
            assert!(
                (got - expected).abs() < 0.1,
                "{gain} dB at {hz} Hz: {got} against {expected}"
            );
        }
    }
    assert!((response_db(&[(HIGH, 15.0)], 20_000.0) - 15.0).abs() < 0.1);
}

// Spec: channel-eq R6
#[test]
fn a_cutting_high_band_adds_a_low_pass_whose_corner_falls_with_the_cut() {
    let butterworth = std::f64::consts::FRAC_1_SQRT_2;
    for (gain, corner) in [
        (-0.001, 20_000.0),
        (-7.5, (20_000.0f64 * 8_000.0).sqrt()),
        (-15.0, 8_000.0),
    ] {
        assert!((low_pass_hz(gain) / corner - 1.0).abs() < 1e-3, "{gain} dB");
        let ir = impulse_response(strip(&[(HIGH, gain)], FRAMES).as_mut(), FRAMES);
        let shelf = rbj(Kind::HighShelf, HIGH_SHELF_HZ, gain, butterworth);
        let low_pass = rbj(Kind::LowPass, corner, 0.0, butterworth);
        // The whole curve is the shelf and the low-pass together.
        for hz in [1_000.0, 4_000.0, 8_000.0, 12_000.0, 18_000.0] {
            let want = db(mul(
                biquad_response(shelf, hz),
                biquad_response(low_pass, hz),
            ));
            let got = db(response_at(&ir, hz));
            assert!(
                (got - want).abs() < 0.1,
                "{gain} dB at {hz} Hz: {got} against {want}"
            );
        }
        // With the shelf divided out, the low-pass is 3 dB down at its corner.
        let found = crossing(2_000.0, 23_000.0, -3.0103, |hz| {
            db(response_at(&ir, hz)) - curve_db(shelf, hz)
        });
        assert!(
            (found / corner - 1.0).abs() < 0.02,
            "{gain} dB: corner {found} Hz, wanted {corner}"
        );
    }
}

// Spec: channel-eq R7
#[test]
fn output_scales_the_result_and_reaches_minus_infinity() {
    let input = noise(1024);
    let flat = run(strip(&[], 1024).as_mut(), &input, &[]);
    let half = run(
        strip(&[(OUTPUT, db_to_gain(-6.0))], 1024).as_mut(),
        &input,
        &[],
    );
    for (h, f) in half[0].iter().zip(&flat[0]) {
        assert!((f64::from(*h) - f64::from(*f) * db_to_gain(-6.0)).abs() < 1e-6);
    }
    let silent = run(strip(&[(OUTPUT, 0.0)], 1024).as_mut(), &input, &[]);
    assert!(silent.iter().flatten().all(|s| *s == 0.0));
}

// Spec: channel-eq R2
#[test]
fn switching_the_high_pass_off_leaves_nothing_of_it_behind() {
    let input = noise(2048);
    let mut node = strip(&[(HIGH_PASS, 1.0)], 1024);
    run(node.as_mut(), &input[..1024], &[]);
    // The next block is flat from its first frame on: the output is the input.
    let out = run(node.as_mut(), &input[1024..], &[set(0, HIGH_PASS, 0.0)]);
    assert_eq!(out[0], &input[1024..]);
}

// Spec: channel-eq R8
#[test]
fn renders_are_bit_identical_at_any_block_size_and_add_no_latency() {
    let input = noise(8192);
    let mut renders = Vec::new();
    for block in [1usize, 64, 512, 4096] {
        let mut node = strip(&[], block);
        assert_eq!(node.latency(), 0);
        let mut out = Vec::new();
        for (n, chunk) in input.chunks(block).enumerate() {
            // The settings arrive on the first block and glide for 20 ms.
            let events = if n == 0 {
                vec![
                    set(0, HIGH_PASS, 1.0),
                    set(0, LOW, 9.0),
                    set(0, HIGH, -11.0),
                    set(0, MID, 7.0),
                ]
            } else {
                Vec::new()
            };
            out.extend_from_slice(&run(node.as_mut(), chunk, &events)[0]);
        }
        renders.push(out);
    }
    for render in &renders[1..] {
        assert_eq!(render, &renders[0]);
    }
}

// Spec: channel-eq R9
#[test]
fn the_display_holds_the_response_the_mid_marker_and_the_spectrum() {
    use std::f64::consts::TAU;
    let device_type = channel_eq_type();
    let values = device(&device_type, &[(MID, 12.0), (MID_FREQ, 1_000.0)]);
    let shown = |data: Option<&[f32]>| {
        bl_devices::display(
            CHANNEL_EQ,
            values.id,
            &device_type.description,
            &values.params,
            data,
        )
        .expect("Strip draws a display")
    };
    let from_values = shown(None);
    assert!(!from_values.live && from_values.series.len() == 1);
    let response = &from_values.series[0];
    assert_eq!(response.values.len(), bl_devices::eq::CURVE_POINTS);
    let peak = response.values.iter().copied().fold(f32::MIN, f32::max);
    assert!((peak - 12.0).abs() < 0.5, "{peak}");
    assert_eq!(from_values.markers[0].name, "Mid");

    // A 1 kHz tone through the node puts its peak in the spectrum.
    let config = config(4096);
    let mut node = NativeNode::new(
        Strip::new(&config),
        &device_type.description,
        &values,
        &config,
    );
    let tone: Vec<f32> = (0..4096)
        .map(|n| (0.5 * (TAU * 1_000.0 * n as f64 / 48_000.0).sin()) as f32)
        .collect();
    run(&mut node, &tone, &[]);
    let spectrum = node.device().analyser().to_vec();
    let live = shown(Some(&spectrum));
    assert!(live.live && live.series.iter().any(|s| s.name == "Spectrum"));
    let loudest = spectrum
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    let centre = 20.0 * 1_000.0f64.powf((loudest as f64 + 0.5) / 64.0);
    assert!((centre / 1_000.0).ln().abs() < 0.2, "{centre} Hz");
}
