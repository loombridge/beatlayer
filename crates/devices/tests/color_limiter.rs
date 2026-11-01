//! Color Limiter, the native limiter with character (WP-8.50; the Color
//! Limiter spec): Loudness, Saturation and Color before the Limiter's
//! look-ahead stage, Ceiling, Lookahead and its latency, Release, the
//! gain-reduction display, and renders that don't depend on the block size.
//! Every block is a real-time section under the allocation trap.

use crate::common;
use core::f64::consts::TAU;

use bl_devices::color_limiter::{
    CEILING, COLOR, COLOR_LIMITER, LOOKAHEAD, LOUDNESS, RELEASE, SATURATION, color_limiter_type,
};
use bl_dsp::Oversampler;
use bl_engine::{Block, DeviceType, DisplayReader, Node, Registry, TransportInfo};
use bl_model::param::db_to_gain;
use bl_model::{DeviceId, ParamId};

use common::{RATE, device, node};

const RATE_HZ: f64 = 48_000.0;

/// A registry with Color Limiter alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::color_limiter::register(&mut registry);
    registry
}

/// A Color Limiter node of `params`, up to `max_frames` at a time, and the
/// reader of its display.
fn rig(params: &[(ParamId, f64)], max_frames: usize) -> (Box<dyn Node>, DisplayReader) {
    let device_type: DeviceType = color_limiter_type();
    let mut node = node(&registry(), &device(&device_type, params), max_frames);
    let reader = node.display().expect("it has a display");
    (node, reader)
}

/// One block of `input` on both channels, as a real-time section.
fn run(node: &mut dyn Node, input: &[f32]) -> Vec<f32> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..2).flat_map(|_| input.iter().copied()).collect();
    let mut block = Block::new(
        &mut data,
        2,
        frames,
        frames,
        &[],
        TransportInfo::default(),
        RATE,
    );
    bl_rt::rt_test!({ node.process(&mut block) });
    data.truncate(frames);
    data
}

/// `input` through `node` in blocks of `block` frames.
fn stream(node: &mut dyn Node, input: &[f32], block: usize) -> Vec<f32> {
    input
        .chunks(block)
        .flat_map(|chunk| run(node, chunk))
        .collect()
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (amplitude * (TAU * hz * n as f64 / RATE_HZ).sin()) as f32)
        .collect()
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn rms_db(signal: &[f64]) -> f64 {
    let mean = signal.iter().map(|s| s * s).sum::<f64>() / signal.len() as f64;
    10.0 * mean.max(1e-18).log10()
}

/// The magnitude of `signal` at `hz`, over the whole of it (a Goertzel
/// filter, on whole cycles).
fn tone_level(signal: &[f32], hz: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, s) in signal.iter().enumerate() {
        let phase = TAU * hz * n as f64 / RATE_HZ;
        re += f64::from(*s) * phase.cos();
        im += f64::from(*s) * phase.sin();
    }
    2.0 * re.hypot(im) / signal.len() as f64
}

/// The frames of latency a Lookahead choice reports: its look-ahead and
/// the oversampler's.
fn latency_of(lookahead_ms: f64) -> u32 {
    (lookahead_ms * 0.001 * RATE_HZ).ceil() as u32 + Oversampler::new(1, 2, 64).latency()
}

// Spec: color-limiter R1, R2, R4 and R9
#[test]
fn color_limiter_is_described_with_one_bank_of_six() {
    let device_type = color_limiter_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Color Limiter");
    assert!(matches!(
        device_type.kind,
        bl_model::DeviceKind::Native { ref device_type } if device_type == COLOR_LIMITER
    ));
    let description = &device_type.description;
    let names: Vec<_> = description.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Loudness",
            "Ceiling",
            "Lookahead",
            "Release",
            "Saturation",
            "Color"
        ]
    );
    assert!(description.param(LOUDNESS).unwrap().is_gain());
    let ceiling = description.param(CEILING).unwrap();
    assert_eq!(
        (ceiling.min(), ceiling.max(), ceiling.default),
        (-24.0, 0.0, -0.3)
    );
    let banks = description.banks();
    assert_eq!(banks.len(), 1);
    assert_eq!(banks[0].params.len(), 6);
}

// Spec: color-limiter R5. Loudness at +24 dB and Saturation and Color at the
// ends of their ranges, on noise and on a tone.
#[test]
fn the_output_never_passes_ceiling() {
    let noise = common::noise(24_000);
    let tone = sine(1_000.0, 0.9, 24_000);
    for ceiling in [-0.3, -6.0, -24.0] {
        let limit = db_to_gain(ceiling) as f32 * (1.0 + 1e-4);
        for saturation in [0.0, 100.0] {
            for color in [-100.0, 0.0, 100.0] {
                let params = [
                    (LOUDNESS, db_to_gain(24.0)),
                    (CEILING, ceiling),
                    (SATURATION, saturation),
                    (COLOR, color),
                ];
                for input in [&noise, &tone] {
                    let (mut node, _) = rig(&params, 256);
                    let out = stream(node.as_mut(), input, 256);
                    let at = peak(&out);
                    assert!(
                        at <= limit,
                        "ceiling {ceiling}, saturation {saturation}, color {color}: {at}"
                    );
                    // Saturation's clipper caps the level at 1 / k, so only
                    // without it does the signal reach the ceiling.
                    assert!(at > 0.0, "it doesn't mute");
                    if saturation == 0.0 {
                        assert!(at > limit * 0.9, "it limits at the ceiling: {at}");
                    }
                }
            }
        }
    }
}

// Spec: color-limiter R4. The look-ahead and the oversampler are both
// reported, before the device first runs, and an impulse comes out that
// late.
#[test]
fn the_lookahead_and_the_oversampler_are_reported_as_latency() {
    for (choice, ms) in [(0.0, 1.0), (1.0, 3.0), (2.0, 6.0)] {
        let (mut node, _) = rig(&[(LOOKAHEAD, choice)], 1_024);
        let latency = latency_of(ms);
        assert_eq!(node.latency(), latency, "{ms} ms");
        let mut input = vec![0.0f32; 1_024];
        input[0] = 0.01;
        let out = run(node.as_mut(), &input);
        let at = out
            .iter()
            .enumerate()
            .fold((0, 0.0f32), |most, (n, s)| {
                if s.abs() > most.1 { (n, s.abs()) } else { most }
            })
            .0;
        assert_eq!(at as u32, latency, "{ms} ms: the impulse's centre");
    }
}

// Spec: color-limiter R3 and R6. With a ceiling out of the way, a sine's
// level through Saturation is the level of the law tanh(k x) / k.
#[test]
fn saturations_measured_curve_is_within_a_hundredth_of_a_decibel_of_its_law() {
    let input = sine(100.0, 0.8, 48_000);
    let latency = latency_of(3.0) as usize;
    for saturation in [0.0, 25.0, 50.0, 100.0] {
        let k = 1.0 + 3.0 * saturation / 100.0;
        let (mut node, _) = rig(&[(CEILING, 0.0), (SATURATION, saturation)], 512);
        let out = stream(node.as_mut(), &input, 512);
        let law: Vec<f64> = input
            .iter()
            .map(|x| (k * f64::from(*x)).tanh() / k)
            .collect();
        let from = 9_600;
        let got: Vec<f64> = out[from + latency..]
            .iter()
            .map(|s| f64::from(*s))
            .collect();
        let want = &law[from..from + got.len()];
        let error = rms_db(&got) - rms_db(want);
        assert!(error.abs() < 0.01, "saturation {saturation} %: {error} dB");
    }
}

// Spec: color-limiter R3 and R6. A quiet tone through Color: the gain at
// each frequency is the tilt's law, 1/G below the band, G above it and
// unity at 1 kHz.
#[test]
fn colors_measured_tilt_is_within_half_a_decibel_of_its_law() {
    for color in [-100.0, -50.0, 50.0, 100.0] {
        let g = 10f64.powf(color / 100.0 * 6.0 / 20.0);
        for hz in [20.0, 100.0, 1_000.0, 5_000.0, 10_000.0, 16_000.0] {
            // Whole cycles of every test frequency over 19,200 samples.
            let input = sine(hz, 0.01, 24_000);
            let (mut node, _) = rig(&[(CEILING, 0.0), (COLOR, color)], 512);
            let out = stream(node.as_mut(), &input, 512);
            let measured = 20.0 * (tone_level(&out[4_800..], hz) / 0.01).log10();
            let f0 = 1_000.0;
            let law = 20.0
                * (g * (hz * hz + (f0 / g).powi(2)).sqrt() / (hz * hz + (f0 * g).powi(2)).sqrt())
                    .log10();
            assert!(
                (measured - law).abs() < 0.5,
                "color {color} % at {hz} Hz: {measured} dB, law {law} dB"
            );
        }
    }
    // The ends of the band: 6 dB either way at Color ±100 %.
    let (mut node, _) = rig(&[(CEILING, 0.0), (COLOR, 100.0)], 512);
    let out = stream(node.as_mut(), &sine(16_000.0, 0.01, 24_000), 512);
    let top = 20.0 * (tone_level(&out[4_800..], 16_000.0) / 0.01).log10();
    assert!((top - 6.0).abs() < 0.5, "{top} dB");
}

// Spec: color-limiter R4. After a loud stretch the gain recovers with
// Release: 10 ms is back to unity well within 250 ms, 1,000 ms is not.
#[test]
fn release_sets_how_quickly_the_gain_recovers() {
    let mut input = vec![1.0f32; 4_800];
    input.extend(vec![0.25f32; 24_000]);
    let level_after = |release: f64| {
        let (mut node, _) = rig(&[(CEILING, -6.0), (RELEASE, release)], 512);
        let out = stream(node.as_mut(), &input, 512);
        out[4_800 + 12_000]
    };
    // 0.25 through the clipper (k = 1) is tanh(0.25).
    let open = 0.25f64.tanh() as f32;
    assert!((level_after(10.0) - open).abs() < 0.005);
    assert!(level_after(1_000.0) < open * 0.8);
}

// Spec: color-limiter R8. At -inf dB Loudness the device is silent.
#[test]
fn loudness_reaches_minus_infinity() {
    let (mut node, _) = rig(&[(LOUDNESS, 0.0)], 256);
    let out = stream(node.as_mut(), &common::noise(4_096), 256);
    assert!(out.iter().all(|s| *s == 0.0));
}

// Spec: color-limiter R1. The display's reading is the gain reduction.
#[test]
fn the_display_reads_the_gain_reduction() {
    let (mut node, mut reader) = rig(&[(LOUDNESS, db_to_gain(12.0)), (CEILING, -6.0)], 256);
    stream(node.as_mut(), &sine(1_000.0, 0.9, 12_000), 256);
    let published = reader.read().values.clone();
    assert_eq!(published.len(), 1);
    assert!(published[0] < -5.0, "{published:?}");
    assert!((f64::from(node.gain_reduction()) - f64::from(published[0])).abs() < 1.0);

    let id = DeviceId(bl_core::id::Id::new(1).unwrap());
    let shown = bl_devices::display(
        COLOR_LIMITER,
        id,
        &color_limiter_type().description,
        &Default::default(),
        None,
    )
    .expect("it draws a display");
    assert!(!shown.live && shown.readings.is_empty(), "no engine");
    let shown = bl_devices::display(
        COLOR_LIMITER,
        id,
        &color_limiter_type().description,
        &Default::default(),
        Some(&published),
    )
    .unwrap();
    assert!(shown.live);
    assert_eq!(shown.readings[0].name, "Gain Reduction");
    assert_eq!(shown.readings[0].value, published[0]);
}

// Spec: color-limiter R7
#[test]
fn renders_are_the_same_at_any_block_size() {
    let input = common::noise(12_288);
    let params = [
        (LOUDNESS, db_to_gain(12.0)),
        (CEILING, -3.0),
        (LOOKAHEAD, 2.0),
        (RELEASE, 40.0),
        (SATURATION, 60.0),
        (COLOR, 40.0),
    ];
    let render = |block: usize| {
        let (mut node, _) = rig(&params, block);
        stream(node.as_mut(), &input, block)
    };
    let reference = render(4_096);
    assert!(reference.iter().any(|s| *s != 0.0));
    for block in [1, 64, 512] {
        assert_eq!(render(block), reference, "block size {block}");
    }
    assert_eq!(render(512), render(512), "run after run");
}
