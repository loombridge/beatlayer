//! Erosion, the native digital-degradation effect (WP-5.24; the digital
//! degradation spec): a short delay moved by a sine and by band-filtered
//! noise.

bl_rt::install_trap!();

mod common;

use bl_devices::degrade::{
    AMOUNT, DEGRADE, FILTER_WIDTH, FREQ, LATENCY_SECONDS, NOISE_BLEND, STEREO_WIDTH, degrade_type,
};
use bl_engine::Node;
use bl_model::{DeviceKind, ParamId};
use common::{RATE, db, device, node, noise, sine, spectrum, stream};

/// An Erosion of `params`, for blocks of up to 4,096 frames.
fn degrade(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    node(&common::registry(), &device(&degrade_type(), params), 4_096)
}

/// `input` on both channels through an Erosion, in `block`-sized blocks.
fn render(params: &[(ParamId, f64)], input: &[f32], block: usize) -> Vec<Vec<f32>> {
    stream(
        degrade(params).as_mut(),
        &[input.to_vec(), input.to_vec()],
        block,
    )
}

/// A second of the output, after the first 4,800 frames, as a spectrum with
/// 1 Hz bins.
fn analyse(out: &[f32]) -> Vec<f64> {
    spectrum(&out[4_800..4_800 + 48_000])
}

/// The frequency, in hertz, of the loudest bin from `low` to `high` Hz.
fn loudest(spectrum: &[f64], low: usize, high: usize) -> usize {
    (low..high)
        .max_by(|a, b| spectrum[*a].total_cmp(&spectrum[*b]))
        .unwrap()
}

const SECOND: usize = 52_800;

// Spec: digital-degradation R1
#[test]
fn a_sine_input_gains_sidebands_at_its_frequency_plus_and_minus_freq_growing_with_amount() {
    let input = sine(3_000.0, 0.5, SECOND);
    let mut previous = 0.0;
    for amount in [1.0, 2.0, 4.0] {
        let params = [(FREQ, 500.0), (AMOUNT, amount), (NOISE_BLEND, 0.0)];
        let spectrum = analyse(&render(&params, &input, 512)[0]);
        for (side, centre) in [("upper", 3_500), ("lower", 2_500)] {
            let at = loudest(&spectrum, centre - 250, centre + 250);
            let error = (at as f64 - centre as f64).abs() / centre as f64;
            assert!(error < 0.01, "{side} sideband at {at} Hz");
            assert!(db(spectrum[at]) > -60.0, "{side}: {} dB", db(spectrum[at]));
        }
        let level = spectrum[3_500];
        assert!(level > previous * 1.5, "amount {amount}: {level}");
        previous = level;
        // Nothing sits halfway between the carrier and the sideband.
        assert!(db(spectrum[3_250]) < db(spectrum[3_500]) - 40.0);
    }
}

// Spec: digital-degradation R2
#[test]
fn at_a_noise_blend_of_100_per_cent_the_sidebands_are_not_lines() {
    let input = sine(3_000.0, 0.5, SECOND);
    let ratio = |blend: f64| {
        let params = [(FREQ, 500.0), (AMOUNT, 4.0), (NOISE_BLEND, blend)];
        let spectrum = analyse(&render(&params, &input, 512)[0]);
        // The strongest bin against the mean power, over the band around the
        // upper sideband.
        let band = &spectrum[3_250..3_750];
        let peak = band.iter().fold(0.0f64, |m, v| m.max(*v));
        let mean = band.iter().map(|v| v * v).sum::<f64>() / band.len() as f64;
        peak * peak / mean
    };
    assert!(ratio(0.0) > 100.0, "a line: {}", ratio(0.0));
    assert!(ratio(100.0) < 40.0, "no line: {}", ratio(100.0));
}

// Spec: digital-degradation R3
#[test]
fn amount_0_passes_the_input_delayed_only_by_the_reported_latency() {
    let input = noise(8_192);
    let mut node = degrade(&[(AMOUNT, 0.0), (NOISE_BLEND, 50.0), (STEREO_WIDTH, 100.0)]);
    let latency = node.latency() as usize;
    assert_eq!(latency, (LATENCY_SECONDS * RATE.as_f64()).round() as usize);
    let out = stream(node.as_mut(), &[input.clone(), input.clone()], 512);
    for channel in &out {
        assert!(channel[..latency].iter().all(|s| *s == 0.0));
        let error: f64 = channel[latency..]
            .iter()
            .zip(&input)
            .map(|(o, i)| f64::from(o - i).powi(2))
            .sum();
        assert!(db(error.sqrt()) < -100.0, "{} dB", db(error.sqrt()));
    }
}

// Spec: digital-degradation R5
#[test]
fn stereo_width_0_gives_identical_channels_and_more_does_not() {
    let input = noise(16_384);
    for blend in [0.0, 50.0, 100.0] {
        let alike = render(&[(NOISE_BLEND, blend), (STEREO_WIDTH, 0.0)], &input, 512);
        assert_eq!(alike[0], alike[1], "blend {blend}");
        let apart = render(&[(NOISE_BLEND, blend), (STEREO_WIDTH, 100.0)], &input, 512);
        assert_ne!(apart[0], apart[1], "blend {blend}");
    }
}

// Spec: digital-degradation R4
#[test]
fn filter_width_sets_how_wide_the_noise_band_is() {
    let input = sine(3_000.0, 0.5, SECOND);
    // The share of the upper sidebands' power within a third of an octave
    // of Freq.
    let share = |width: f64| {
        let params = [
            (FREQ, 500.0),
            (AMOUNT, 4.0),
            (NOISE_BLEND, 100.0),
            (FILTER_WIDTH, width),
        ];
        let spectrum = analyse(&render(&params, &input, 512)[0]);
        let power =
            |low: usize, high: usize| spectrum[low..high].iter().map(|v| v * v).sum::<f64>();
        power(3_397, 3_630) / power(3_020, 5_000)
    };
    let (narrow, middle, wide) = (share(0.0), share(50.0), share(100.0));
    assert!(narrow > 0.6, "{narrow}");
    assert!(narrow > middle && middle > wide, "{narrow} {middle} {wide}");
    assert!(wide < 0.15, "{wide}");
}

// Spec: digital-degradation R4
#[test]
fn filter_width_does_nothing_at_a_noise_blend_of_0() {
    let input = noise(16_384);
    let at = |width: f64| render(&[(NOISE_BLEND, 0.0), (FILTER_WIDTH, width)], &input, 512);
    assert_eq!(at(0.0), at(100.0));
}

// Spec: digital-degradation R6
#[test]
fn a_render_repeats_run_after_run_and_at_any_block_size() {
    let input = noise(20_000);
    let params = [
        (FREQ, 700.0),
        (AMOUNT, 60.0),
        (NOISE_BLEND, 50.0),
        (STEREO_WIDTH, 50.0),
    ];
    let first = render(&params, &input, 4_096);
    for block in [1, 64, 512, 4_096] {
        assert_eq!(render(&params, &input, block), first, "block {block}");
    }
}

// Spec: digital-degradation R7
#[test]
fn the_latency_is_about_two_milliseconds() {
    assert_eq!(degrade(&[]).latency(), 96);
}

// Spec: digital-degradation R8
#[test]
fn the_display_shows_the_inputs_spectrum() {
    let device_type = degrade_type();
    assert!(matches!(
        &device_type.kind,
        DeviceKind::Native { device_type } if device_type == DEGRADE
    ));
    let id = device(&device_type, &[]).id;
    let shown = bl_devices::display(
        DEGRADE,
        id,
        &device_type.description,
        &Default::default(),
        Some(&[-20.0; 64]),
    )
    .expect("Erosion draws a display");
    assert!(shown.live);
    assert_eq!(shown.series("Spectrum").unwrap().values.len(), 64);
    let idle = bl_devices::display(
        DEGRADE,
        id,
        &device_type.description,
        &Default::default(),
        None,
    )
    .unwrap();
    assert!(!idle.live && idle.series.is_empty());
}
