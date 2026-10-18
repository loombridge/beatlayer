//! Overdrive, the native overdrive (WP-5.23; the overdrive spec): the band-pass's
//! centre and width, the distortion at Drive 0%, Tone, Dynamics across a
//! Drive sweep, Dry/Wet, the display, and renders that don't depend on the
//! block size.

bl_rt::install_trap!();

mod common;

use bl_devices::overdrive::{
    DRIVE, DRY_WET, DYNAMICS, FREQUENCY, OVERDRIVE, TONE, WIDTH, overdrive_type,
};
use bl_engine::{DeviceType, Node, Registry};
use bl_model::ParamId;
use common::{RATE, device, node, noise, run};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::overdrive::register(&mut registry);
    registry
}

/// An Overdrive of `params`, up to `max_frames` at a time.
fn grit(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = overdrive_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (std::f64::consts::TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

/// The RMS of the last `frames` frames of `samples`.
fn rms(samples: &[f32], frames: usize) -> f64 {
    let tail = &samples[samples.len() - frames..];
    (tail.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / frames as f64).sqrt()
}

/// The magnitude of `samples` at `bin` of a DFT of its length.
fn magnitude(samples: &[f32], bin: usize) -> f64 {
    let n = samples.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in samples.iter().enumerate() {
        let phase = std::f64::consts::TAU * bin as f64 * i as f64 / n;
        re += f64::from(*s) * phase.cos();
        im -= f64::from(*s) * phase.sin();
    }
    (re * re + im * im).sqrt() * 2.0 / n
}

/// The gain, as output over input RMS, of a sine of `hz` at a small
/// `amplitude`, once it has settled.
fn small_signal_gain(params: &[(ParamId, f64)], hz: f64) -> f64 {
    let frames = 14_400;
    let input = sine(hz, 1e-5, frames);
    let out = run(grit(params, frames).as_mut(), &input, &[]);
    rms(&out[0], 4_800) / rms(&input, 4_800)
}

/// Where the band-pass is 3 dB under its peak, below and above its peak,
/// by interpolating across a grid of frequencies from `from` to `to`.
fn edges(params: &[(ParamId, f64)], from: f64, to: f64) -> (f64, f64) {
    let steps = 240;
    let at = |i: usize| from * (to / from).powf(i as f64 / steps as f64);
    let gains: Vec<f64> = (0..=steps)
        .map(|i| small_signal_gain(params, at(i)))
        .collect();
    let peak = gains.iter().copied().fold(0.0, f64::max);
    let level = peak / 2f64.sqrt();
    let crossing = |a: usize, b: usize| {
        let t = (level - gains[a]) / (gains[b] - gains[a]);
        (at(a).ln() + t * (at(b).ln() - at(a).ln())).exp()
    };
    let first = (1..=steps)
        .find(|i| gains[*i] >= level && gains[*i - 1] < level)
        .unwrap();
    let last = (1..=steps)
        .rev()
        .find(|i| gains[*i - 1] >= level && gains[*i] < level)
        .unwrap();
    (crossing(first - 1, first), crossing(last - 1, last))
}

// Spec: overdrive R1
#[test]
fn the_overdrive_is_described_and_laid_out() {
    let device_type = overdrive_type();
    device_type.description.check().unwrap();
    assert_eq!(
        device_type.kind,
        bl_model::DeviceKind::Native {
            device_type: OVERDRIVE.to_owned()
        }
    );
    assert_eq!(device_type.description.params.len(), 6);
    assert!(
        bl_devices::device_types()
            .iter()
            .any(|t| t.name == "Overdrive")
    );
    assert_eq!(grit(&[], 64).latency(), 0, "no latency");
}

// Spec: overdrive R2
#[test]
fn the_band_pass_has_its_centre_and_width_within_two_per_cent() {
    for (centre, octaves) in [(1_200.0, 1.5), (3_000.0, 0.5)] {
        let params = [(FREQUENCY, centre), (WIDTH, octaves), (DRY_WET, 100.0)];
        let (low, high) = edges(&params, centre / 8.0, centre * 8.0);
        let found_centre = (low * high).sqrt();
        let found_width = (high / low).log2();
        assert!(
            (found_centre / centre - 1.0).abs() < 0.02,
            "{centre} Hz: centre {found_centre}"
        );
        assert!(
            (found_width / octaves - 1.0).abs() < 0.02,
            "{octaves} octaves: width {found_width}"
        );
    }
}

// Spec: overdrive R3
#[test]
fn harmonics_are_present_at_drive_zero() {
    let frames = 24_000;
    let input = sine(1_000.0, 0.3, frames);
    let out = run(
        grit(&[(DRIVE, 0.0), (FREQUENCY, 1_000.0), (WIDTH, 2.0)], frames).as_mut(),
        &input,
        &[],
    );
    // A whole number of cycles at the end: 4,800 frames of 1 kHz.
    let tail = &out[0][frames - 4_800..];
    let third = 20.0 * (magnitude(tail, 300) / magnitude(tail, 100)).log10();
    assert!(
        third > -40.0,
        "the third harmonic is {third} dB under the fundamental"
    );
    // And a hard drive distorts more.
    let hard = run(
        grit(
            &[(DRIVE, 100.0), (FREQUENCY, 1_000.0), (WIDTH, 2.0)],
            frames,
        )
        .as_mut(),
        &input,
        &[],
    );
    let tail = &hard[0][frames - 4_800..];
    let hard_third = 20.0 * (magnitude(tail, 300) / magnitude(tail, 100)).log10();
    assert!(hard_third > third, "{hard_third} against {third}");
}

// Spec: overdrive R4
#[test]
fn tone_raises_the_highs() {
    let at =
        |tone: f64| small_signal_gain(&[(TONE, tone), (FREQUENCY, 6_000.0), (WIDTH, 4.0)], 6_000.0);
    let (dark, flat, bright) = (at(0.0), at(50.0), at(100.0));
    assert!(dark < flat && flat < bright, "{dark} {flat} {bright}");
    assert!(20.0 * (bright / dark).log10() > 18.0, "a wide range");
}

// Spec: overdrive R5
#[test]
fn dynamics_levels_the_output_across_a_drive_sweep() {
    let frames = 24_000;
    let input = sine(1_000.0, 0.1, frames);
    let sweep = |dynamics: f64| {
        let levels: Vec<f64> = [0.0, 25.0, 50.0, 75.0, 100.0]
            .iter()
            .map(|drive| {
                let out = run(
                    grit(
                        &[
                            (DRIVE, *drive),
                            (DYNAMICS, dynamics),
                            (FREQUENCY, 1_000.0),
                            (WIDTH, 2.0),
                        ],
                        frames,
                    )
                    .as_mut(),
                    &input,
                    &[],
                );
                20.0 * rms(&out[0], 4_800).log10()
            })
            .collect();
        let most = levels.iter().copied().fold(f64::MIN, f64::max);
        let least = levels.iter().copied().fold(f64::MAX, f64::min);
        most - least
    };
    let (levelled, free) = (sweep(0.0), sweep(100.0));
    assert!(
        levelled < 6.0,
        "at Dynamics 0% the output moves {levelled} dB"
    );
    assert!(free > 12.0, "at Dynamics 100% it moves {free} dB");
    assert!(levelled * 2.0 < free);
}

// Spec: overdrive R6
#[test]
fn dry_wet_at_zero_is_the_input() {
    let input = noise(2048);
    let out = run(
        grit(&[(DRIVE, 100.0), (DRY_WET, 0.0)], 2048).as_mut(),
        &input,
        &[],
    );
    assert_eq!(out[0], input);
}

// Spec: overdrive R7
#[test]
fn the_display_holds_the_curve_and_the_inputs_level() {
    let device_type = overdrive_type();
    let description = &device_type.description;
    let mut values = bl_model::OrdMap::new();
    values.insert(DRIVE, 100.0);
    let id = bl_model::DeviceId(bl_core::id::Id::new(1).unwrap());
    let shown = bl_devices::display(OVERDRIVE, id, description, &values, None).unwrap();
    assert!(!shown.live);
    let curve = shown.series("Drive Curve").unwrap();
    assert_eq!(curve.values.len(), bl_devices::overdrive::CURVE_POINTS);
    assert!(curve.values[0] < -0.99 && curve.values[curve.values.len() - 1] > 0.99);
    let mut node = grit(&[], 256);
    let mut reader = node.display().expect("the device has a display");
    run(node.as_mut(), &[0.5; 256], &[]);
    let data = reader.read();
    assert_eq!(data.published, 1);
    assert!((data.values[0] - 20.0 * 0.5f32.log10()).abs() < 1e-4);
    let live =
        bl_devices::display(OVERDRIVE, id, description, &values, Some(&data.values)).unwrap();
    assert!(live.live);
    assert!((live.reading("Input").unwrap().value + 6.02).abs() < 0.01);
    assert_eq!(live.markers.len(), 1);
}

// Spec: overdrive R8
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let input = noise(8192);
    let params = [
        (DRIVE, 70.0),
        (TONE, 80.0),
        (FREQUENCY, 900.0),
        (WIDTH, 1.0),
        (DYNAMICS, 20.0),
        (DRY_WET, 85.0),
    ];
    let render = |block: usize| -> Vec<f32> {
        let mut node = grit(&params, block);
        input
            .chunks(block)
            .flat_map(|chunk| run(node.as_mut(), chunk, &[])[0].clone())
            .collect()
    };
    let reference = render(4096);
    for block in [1, 64, 512] {
        assert!(render(block) == reference, "block size {block}");
    }
}
