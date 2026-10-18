//! Crush, the native bit-crusher (WP-5.24; the bit-crusher spec): the
//! sample-rate reduction with its jitter and filters, and the bit reduction
//! with its shape and DC shift.

bl_rt::install_trap!();

mod common;

use std::collections::BTreeSet;

use bl_devices::crusher::{
    BITS, DC_SHIFT, DRY_WET, JITTER, POST, POST_OCTAVE, PRE, RATE, SHAPE, crusher_type,
};
use bl_engine::Node;
use bl_model::ParamId;
use common::{db, device, node, noise, sine, spectrum, stream};

/// A Crush of `params`, for blocks of up to 4,096 frames.
fn crush(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    node(&common::registry(), &device(&crusher_type(), params), 4_096)
}

/// `input` on both channels through a Crush, in `block`-sized blocks.
fn render(params: &[(ParamId, f64)], input: &[f32], block: usize) -> Vec<Vec<f32>> {
    stream(
        crush(params).as_mut(),
        &[input.to_vec(), input.to_vec()],
        block,
    )
}

/// A ramp from −1 up to (not including) 1 over `frames`.
fn ramp(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (i as f64 / frames as f64 * 2.0 - 1.0) as f32)
        .collect()
}

/// The levels `signal` takes.
fn levels(signal: &[f32]) -> BTreeSet<u32> {
    signal.iter().map(|s| s.to_bits()).collect()
}

/// The RMS of `signal`.
fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

// Spec: bit-crusher R1
#[test]
fn below_the_engines_rate_the_images_sit_at_multiples_of_the_rate_around_the_input() {
    let rate = 6_000.0;
    let input = sine(1_000.0, 0.5, 52_800);
    let out = render(&[(RATE, rate), (BITS, 16.0)], &input, 512);
    let spectrum = spectrum(&out[0][4_800..52_800]);
    // Every line is at k × Rate ± the input's frequency, to the hertz.
    let mut expected = BTreeSet::new();
    for k in 0..=4 {
        for side in [-1_000.0, 1_000.0] {
            let hz = f64::from(k) * rate + side;
            if hz > 0.0 && hz < 24_000.0 {
                expected.insert(hz as usize);
            }
        }
    }
    let peaks: Vec<usize> = (1..spectrum.len() - 1)
        .filter(|i| {
            spectrum[*i] > 0.015
                && spectrum[*i] >= spectrum[i - 1]
                && spectrum[*i] > spectrum[i + 1]
        })
        .collect();
    assert!(peaks.len() >= 8, "{peaks:?}");
    for peak in &peaks {
        assert!(
            expected.iter().any(|hz| hz.abs_diff(*peak) <= 1),
            "a line at {peak} Hz"
        );
    }
    for hz in expected.iter().filter(|hz| **hz < 14_000) {
        assert!(
            peaks.iter().any(|p| p.abs_diff(*hz) <= 1),
            "no line at {hz} Hz"
        );
    }
}

// Spec: bit-crusher R2
#[test]
fn bits_n_gives_two_to_the_n_levels_on_a_ramp() {
    for bits in [2, 3, 8, 12] {
        let out = render(
            &[(RATE, 48_000.0), (BITS, f64::from(bits))],
            &ramp(65_536),
            4_096,
        );
        assert_eq!(levels(&out[0]).len(), 1 << bits, "{bits} bits");
        let step = 2.0 / f64::from(1u32 << bits);
        assert!(
            out[0]
                .iter()
                .all(|s| (f64::from(*s) / step).fract().abs() < 1e-6)
        );
    }
}

// Spec: bit-crusher R3
#[test]
fn shape_makes_the_steps_finer_near_zero() {
    let near_zero = |shape: f64| {
        let out = render(
            &[(RATE, 48_000.0), (BITS, 4.0), (SHAPE, shape)],
            &ramp(65_536),
            4_096,
        );
        out[0]
            .iter()
            .zip(ramp(65_536))
            .filter(|(_, input)| input.abs() < 0.1)
            .map(|(output, _)| output.to_bits())
            .collect::<BTreeSet<_>>()
            .len()
    };
    let (flat, shaped) = (near_zero(0.0), near_zero(100.0));
    assert!(flat <= 3, "{flat}");
    assert!(shaped >= 8, "{shaped}");
    // Full scale still reaches the same top level, give or take a step.
    let top = |shape: f64| {
        let out = render(
            &[(RATE, 48_000.0), (BITS, 4.0), (SHAPE, shape)],
            &ramp(65_536),
            4_096,
        );
        out[0].iter().fold(f32::MIN, |m, s| m.max(*s))
    };
    assert!(
        (top(0.0) - top(100.0)).abs() < 1e-6,
        "{} {}",
        top(0.0),
        top(100.0)
    );
}

// Spec: bit-crusher R4
#[test]
fn dc_shift_moves_the_quantisers_offset() {
    // At 4 bits a step is 0.125. A quarter of a step is below the first
    // threshold on its own.
    let held = |shift: f64, level: f32| {
        let out = render(
            &[(RATE, 48_000.0), (BITS, 4.0), (DC_SHIFT, shift)],
            &[level; 256],
            256,
        );
        out[0][255]
    };
    let quarter = 0.3 * 0.125;
    assert_eq!(held(0.0, quarter), 0.0);
    assert_eq!(held(25.0, quarter), 0.125);
    assert_eq!(held(-25.0, -quarter), -0.125);
    assert_eq!(held(-25.0, quarter), 0.0);
}

// Spec: bit-crusher R5
#[test]
fn the_post_filters_corner_sits_at_its_octave_setting() {
    // At the engine's rate the hold passes everything, so Post alone shapes
    // the signal. Half the Rate is 24 kHz; one octave down is 12 kHz.
    let gain = |hz: f64, params: &[(ParamId, f64)]| {
        let input = sine(hz, 0.5, 9_600);
        let out = render(params, &input, 512);
        rms(&out[0][4_800..]) / rms(&input[4_800..])
    };
    let params = [
        (RATE, 48_000.0),
        (BITS, 16.0),
        (POST, 1.0),
        (POST_OCTAVE, -1.0),
    ];
    let half_power = std::f64::consts::FRAC_1_SQRT_2;
    let corner = 12_000.0;
    assert!(gain(corner / 1.02, &params) > half_power);
    assert!(gain(corner * 1.02, &params) < half_power);
    assert!((gain(corner, &params) - half_power).abs() < 0.01);
    // Post off passes the sine whole.
    let off = [(RATE, 48_000.0), (BITS, 16.0), (POST_OCTAVE, -1.0)];
    assert!((gain(corner, &off) - 1.0).abs() < 0.01);
}

// Spec: bit-crusher R6
#[test]
fn the_pre_filter_takes_out_what_the_rate_cannot_hold() {
    let input = sine(9_000.0, 0.5, 9_600);
    let level = |pre: f64| {
        let out = render(&[(RATE, 6_000.0), (BITS, 16.0), (PRE, pre)], &input, 512);
        rms(&out[0][4_800..])
    };
    assert!(db(level(1.0)) < db(level(0.0)) - 20.0);
}

// Spec: bit-crusher R7
#[test]
fn jitter_lowers_the_correlation_between_the_channels() {
    let input = sine(1_000.0, 0.5, 48_000);
    let correlation = |jitter: f64| {
        let out = render(
            &[(RATE, 5_000.0), (BITS, 16.0), (JITTER, jitter)],
            &input,
            512,
        );
        let dot: f64 = out[0]
            .iter()
            .zip(&out[1])
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum();
        dot / (rms(&out[0]) * rms(&out[1]) * out[0].len() as f64)
    };
    assert!((correlation(0.0) - 1.0).abs() < 1e-9);
    assert!(correlation(100.0) < 0.9, "{}", correlation(100.0));
    assert!(correlation(30.0) < 0.95, "{}", correlation(30.0));
}

// Spec: bit-crusher R8
#[test]
fn dry_wet_0_per_cent_nulls_against_the_input() {
    let input = noise(8_192);
    let out = render(&[(RATE, 3_000.0), (BITS, 3.0), (DRY_WET, 0.0)], &input, 512);
    assert_eq!(out[0], input);
    assert_eq!(out[1], input);
}

// Spec: bit-crusher R9
#[test]
fn a_render_repeats_run_after_run_and_at_any_block_size() {
    let input = noise(20_000);
    let params = [
        (RATE, 7_300.0),
        (JITTER, 60.0),
        (PRE, 1.0),
        (POST, 1.0),
        (POST_OCTAVE, 0.5),
        (BITS, 5.0),
        (SHAPE, 40.0),
        (DC_SHIFT, 10.0),
        (DRY_WET, 80.0),
    ];
    let first = render(&params, &input, 4_096);
    assert_ne!(first[0], input);
    for block in [1, 64, 512, 4_096] {
        assert_eq!(render(&params, &input, block), first, "block {block}");
    }
    assert_eq!(crush(&[]).latency(), 0);
}
