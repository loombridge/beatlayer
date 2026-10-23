//! Vinyl Distortion, the native vinyl-distortion effect (WP-5.24; the vinyl distortion
//! spec): the tracing model's even harmonics, the pinch's odd ones, the
//! overall drive, Soft and Hard, and the crackle.

use crate::common;
use bl_devices::wax::{
    CRACKLE_DENSITY, CRACKLE_VOLUME, DRIVE, PINCH_DRIVE, PINCH_FREQ, PINCH_MODE, SOFT_HARD,
    TRACING_DRIVE, TRACING_FREQ, wax_type,
};
use bl_engine::Node;
use bl_model::ParamId;
use common::{db, device, dft_at, node, noise, sine, stream};

/// A Vinyl Distortion of `params`, for blocks of up to 4,096 frames.
fn wax(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    node(&common::registry(), &device(&wax_type(), params), 4_096)
}

/// `input` on both channels through a Vinyl Distortion, in `block`-sized blocks.
fn render(params: &[(ParamId, f64)], input: &[f32], block: usize) -> Vec<Vec<f32>> {
    stream(
        wax(params).as_mut(),
        &[input.to_vec(), input.to_vec()],
        block,
    )
}

/// The amplitude of `channel` at `hz`, over the last second of 96,000 frames
/// of a sine: the first second lets the filters settle.
fn partial(channel: &[f32], hz: f64) -> f64 {
    let (re, im) = dft_at(&channel[48_000..], hz);
    re.hypot(im)
}

const TWO_SECONDS: usize = 96_000;

/// Both models off but for one, and Crackle off: the base of the harmonic
/// tests.
fn only(drive: ParamId, others: ParamId) -> [(ParamId, f64); 3] {
    [(drive, 100.0), (others, 0.0), (SOFT_HARD, 1.0)]
}

// Spec: vinyl-distortion R1
#[test]
fn the_tracing_models_second_harmonic_exceeds_its_third() {
    let input = sine(3_000.0, 0.5, TWO_SECONDS);
    let mut params = only(TRACING_DRIVE, PINCH_DRIVE).to_vec();
    params.push((TRACING_FREQ, 3_000.0));
    let out = render(&params, &input, 512);
    let (second, third) = (partial(&out[0], 6_000.0), partial(&out[0], 9_000.0));
    assert!(db(second) > -30.0, "{} dB", db(second));
    assert!(
        db(second) - db(third) > 40.0,
        "{} over {}",
        db(second),
        db(third)
    );
    assert!(
        (partial(&out[0], 3_000.0) - 0.5).abs() < 0.01,
        "the input stays"
    );
    // Even harmonics are the same in both channels.
    let (left, right) = (
        dft_at(&out[0][48_000..], 6_000.0),
        dft_at(&out[1][48_000..], 6_000.0),
    );
    assert!((left.0 - right.0).abs() < 1e-6 && (left.1 - right.1).abs() < 1e-6);
}

// Spec: vinyl-distortion R2
#[test]
fn the_pinchs_third_harmonic_exceeds_its_second_in_opposite_phase_in_stereo_and_in_phase_in_mono() {
    let input = sine(6_000.0, 0.5, TWO_SECONDS);
    let mut params = only(PINCH_DRIVE, TRACING_DRIVE).to_vec();
    params.push((PINCH_FREQ, 6_000.0));
    let phase = |mode: f64| {
        let mut with_mode = params.clone();
        with_mode.push((PINCH_MODE, mode));
        let out = render(&with_mode, &input, 512);
        let (second, third) = (partial(&out[0], 12_000.0), partial(&out[0], 18_000.0));
        assert!(db(third) > -40.0, "{} dB", db(third));
        assert!(
            db(third) - db(second) > 40.0,
            "{} over {}",
            db(third),
            db(second)
        );
        let left = dft_at(&out[0][48_000..], 18_000.0);
        let right = dft_at(&out[1][48_000..], 18_000.0);
        // The cosine of the angle between the two channels' third harmonic.
        (left.0 * right.0 + left.1 * right.1) / (left.0.hypot(left.1) * right.0.hypot(right.1))
    };
    assert!(phase(0.0) < -0.999, "stereo: opposite, {}", phase(0.0));
    assert!(phase(1.0) > 0.999, "mono: alike, {}", phase(1.0));
}

// Spec: vinyl-distortion R3
#[test]
fn drive_scales_both_models_and_at_zero_the_signal_passes_whole() {
    let input = sine(3_000.0, 0.5, TWO_SECONDS);
    let second = |drive: f64| {
        let out = render(
            &[
                (DRIVE, drive),
                (TRACING_FREQ, 3_000.0),
                (PINCH_DRIVE, 0.0),
                (TRACING_DRIVE, 100.0),
            ],
            &input,
            512,
        );
        partial(&out[0], 6_000.0)
    };
    assert!((second(50.0) / second(100.0) - 0.5).abs() < 0.01);
    let out = render(&[(DRIVE, 0.0)], &input, 512);
    assert_eq!(out[0], input);
    assert_eq!(out[1], input);
}

// Spec: vinyl-distortion R4
#[test]
fn soft_distorts_less_than_hard() {
    let input = sine(3_000.0, 0.5, TWO_SECONDS);
    let third = |mode: f64| {
        let out = render(
            &[
                (SOFT_HARD, mode),
                (TRACING_FREQ, 3_000.0),
                (PINCH_FREQ, 3_000.0),
            ],
            &input,
            512,
        );
        partial(&out[0], 6_000.0) + partial(&out[0], 9_000.0)
    };
    assert!(
        third(0.0) < third(1.0) * 0.5,
        "{} {}",
        third(0.0),
        third(1.0)
    );
}

/// The starts of the clicks in `channel`: each time it rises past 0.1 from
/// below.
fn clicks(channel: &[f32]) -> usize {
    channel
        .windows(2)
        .filter(|w| w[0].abs() <= 0.1 && w[1].abs() > 0.1)
        .count()
}

// Spec: vinyl-distortion R5
#[test]
fn the_crackles_clicks_per_second_are_within_ten_per_cent_of_density() {
    let silence = vec![0.0f32; 40 * 48_000];
    for density in [25.0, 100.0, 250.0] {
        let out = render(
            &[(CRACKLE_DENSITY, density), (CRACKLE_VOLUME, 1.0)],
            &silence,
            4_096,
        );
        for (side, channel) in out.iter().enumerate() {
            let per_second = clicks(channel) as f64 / 40.0;
            assert!(
                (per_second / density - 1.0).abs() < 0.1,
                "density {density}, channel {side}: {per_second} a second"
            );
        }
        assert_ne!(out[0], out[1], "the channels crackle apart");
    }
}

// Spec: vinyl-distortion R6
#[test]
fn crackle_volume_at_minus_infinity_is_silent_and_no_density_is_no_crackle() {
    let silence = vec![0.0f32; 48_000];
    let shut = render(
        &[(CRACKLE_DENSITY, 100.0), (CRACKLE_VOLUME, 0.0)],
        &silence,
        512,
    );
    assert!(shut.iter().flatten().all(|s| *s == 0.0));
    let none = render(
        &[(CRACKLE_DENSITY, 0.0), (CRACKLE_VOLUME, 1.0)],
        &silence,
        512,
    );
    assert!(none.iter().flatten().all(|s| *s == 0.0));
    let some = render(
        &[(CRACKLE_DENSITY, 100.0), (CRACKLE_VOLUME, 1.0)],
        &silence,
        512,
    );
    assert!(some.iter().flatten().any(|s| *s != 0.0));
}

// Spec: vinyl-distortion R7
#[test]
fn a_render_repeats_run_after_run_and_at_any_block_size() {
    let input = noise(20_000);
    let params = [
        (TRACING_DRIVE, 70.0),
        (PINCH_DRIVE, 80.0),
        (DRIVE, 90.0),
        (CRACKLE_DENSITY, 80.0),
        (CRACKLE_VOLUME, 0.3),
    ];
    let first = render(&params, &input, 4_096);
    assert_ne!(first[0], input);
    for block in [1, 64, 512, 4_096] {
        assert_eq!(render(&params, &input, block), first, "block {block}");
    }
    assert_eq!(wax(&[]).latency(), 0);
}
