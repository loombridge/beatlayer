//! Saturator, the native saturator (WP-5.23; the saturator spec): the eight
//! curves' transfer functions read from a slow ramp, the Waveshaper's
//! controls, Post Clip and Output, the Color filters, Hi-Quality and its
//! latency, the Pre-DC Filter, Dry/Wet, the display, and renders that don't
//! depend on the block size.

use crate::common;
use bl_devices::saturator::{
    AMT_HI, AMT_LO, COLOR, CURVE, CURVES, DRIVE, DRY_WET, FREQUENCY, HI_QUALITY, OUTPUT, POST_CLIP,
    PRE_DC, SATURATOR, THRESHOLD, WIDTH, WS_CURVE, WS_DAMP, WS_DEPTH, WS_DRIVE, WS_LINEAR,
    WS_PERIOD, saturator_type,
};
use bl_engine::{DeviceType, Node, Registry};
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::{RATE, device, node, noise, run};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::saturator::register(&mut registry);
    registry
}

/// A Saturator of `params`, up to `max_frames` at a time.
fn singe(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = saturator_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// The curve numbers.
const ANALOG: f64 = 0.0;
const SOFT_SINE: f64 = 1.0;
const BASS_SHAPER: f64 = 2.0;
const MEDIUM: f64 = 3.0;
const HARD: f64 = 4.0;
const SINOID_FOLD: f64 = 5.0;
const DIGITAL: f64 = 6.0;
const WAVESHAPER: f64 = 7.0;

/// A slow ramp from -1 to 1.
fn ramp(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (-1.0 + 2.0 * i as f64 / (frames - 1) as f64) as f32)
        .collect()
}

/// The Waveshaper's controls, as the spec gives them (all but Period in
/// per cent).
#[derive(Clone, Copy)]
struct Ws {
    drive: f64,
    curve: f64,
    depth: f64,
    linear: f64,
    damp: f64,
    period: f64,
}

impl Ws {
    /// The defaults.
    const DEFAULT: Self = Self {
        drive: 100.0,
        curve: 50.0,
        depth: 0.0,
        linear: 0.0,
        damp: 0.0,
        period: 4.0,
    };

    fn params(self) -> Vec<(ParamId, f64)> {
        vec![
            (WS_DRIVE, self.drive),
            (WS_CURVE, self.curve),
            (WS_DEPTH, self.depth),
            (WS_LINEAR, self.linear),
            (WS_DAMP, self.damp),
            (WS_PERIOD, self.period),
        ]
    }
}

/// A curve linear to `knee`, then easing to 1 by a tanh.
fn tanh_knee(x: f64, knee: f64) -> f64 {
    let m = x.abs();
    if m <= knee {
        x
    } else {
        x.signum() * (knee + (1.0 - knee) * ((m - knee) / (1.0 - knee)).tanh())
    }
}

/// The saturator spec's functions (R2 to R7), written out.
fn spec(curve: f64, threshold_db: f64, ws: Ws, x: f64) -> f64 {
    let m = x.abs();
    match curve as usize {
        0 => tanh_knee(x, 0.5),
        1 => {
            if m <= 0.5 {
                x
            } else {
                x.signum() * (0.5 + 0.5 * (2.0 * (m - 0.5)).min(std::f64::consts::FRAC_PI_2).sin())
            }
        }
        2 => {
            let knee = 10f64.powf(threshold_db / 20.0);
            if m <= knee {
                x
            } else if knee >= 1.0 {
                x.signum()
            } else {
                let over = (m - knee) / (1.0 - knee);
                x.signum() * (knee + (1.0 - knee) * over / (1.0 + over))
            }
        }
        3 => tanh_knee(x, 0.2),
        4 => tanh_knee(x, 0.8),
        5 => {
            if m <= 0.5 {
                x
            } else {
                x.signum() * (0.5 + 0.5 * (2.0 * (m - 0.5)).sin())
            }
        }
        6 => x.clamp(-1.0, 1.0),
        _ => {
            let (d, c, depth, linear, damp) = (
                ws.drive / 100.0,
                ws.curve / 100.0,
                ws.depth / 100.0,
                ws.linear / 100.0,
                ws.damp / 100.0,
            );
            let u = x.clamp(-1.0, 1.0);
            let core = u + c * (0.5 * u - 0.5 * u * u * u);
            let ripple = 0.25 * depth * (std::f64::consts::PI * ws.period * u).sin();
            let shaped = (1.0 - linear) * (core + ripple) + linear * u;
            let gate = if damp > 0.0 {
                let t = (u.abs() / (0.25 * damp)).min(1.0);
                t * t * (3.0 - 2.0 * t)
            } else {
                1.0
            };
            (1.0 - d) * x + d * shaped * gate
        }
    }
}

/// The worst distance between `node`'s output for the ramp and the spec's
/// function of the ramp times the Drive gain.
fn transfer_error(params: &[(ParamId, f64)], drive_db: f64, threshold_db: f64, ws: Ws) -> f64 {
    let curve = params
        .iter()
        .find(|(p, _)| *p == CURVE)
        .map_or(0.0, |(_, v)| *v);
    let input = ramp(4096);
    let mut all = vec![(DRIVE, drive_db), (DRY_WET, 100.0)];
    all.extend_from_slice(params);
    let mut node = singe(&all, 4096);
    let out = run(node.as_mut(), &input, &[]);
    let gain = db_to_gain(drive_db);
    input
        .iter()
        .zip(&out[0])
        .map(|(x, y)| (f64::from(*y) - spec(curve, threshold_db, ws, gain * f64::from(*x))).abs())
        .fold(0.0, f64::max)
}

// Spec: saturator R1
#[test]
fn the_saturator_is_described_and_laid_out() {
    let device_type = saturator_type();
    let description = &device_type.description;
    description.check().unwrap();
    assert_eq!(
        device_type.kind,
        bl_model::DeviceKind::Native {
            device_type: SATURATOR.to_owned()
        }
    );
    assert!(
        description.layout.has_expanded(),
        "the Waveshaper and Color's second filter are expanded"
    );
    assert_eq!(description.menu_options, vec![HI_QUALITY, PRE_DC]);
    assert_eq!(
        description.param(CURVE).map(|p| p.format(0.0)).as_deref(),
        Some("Analog Clip")
    );
    for (i, name) in CURVES.iter().enumerate() {
        assert_eq!(description.param(CURVE).unwrap().format(i as f64), *name);
    }
    let threshold = description.param(THRESHOLD).unwrap();
    assert_eq!((threshold.min(), threshold.max()), (-50.0, 0.0));
    // Saturator is registered with the shipped devices.
    assert!(
        bl_devices::device_types()
            .iter()
            .any(|t| t.name == "Saturator")
    );
}

// Spec: saturator R2
#[test]
fn each_curves_transfer_matches_the_spec_at_three_drive_settings() {
    for curve in [
        ANALOG,
        SOFT_SINE,
        BASS_SHAPER,
        MEDIUM,
        HARD,
        SINOID_FOLD,
        DIGITAL,
        WAVESHAPER,
    ] {
        for drive_db in [-6.0, 6.0, 18.0] {
            let error = transfer_error(&[(CURVE, curve)], drive_db, -12.0, Ws::DEFAULT);
            assert!(
                error < 0.001,
                "{}: {error} at {drive_db} dB",
                CURVES[curve as usize]
            );
        }
    }
}

// Spec: saturator R3
#[test]
fn digital_clip_is_flat_past_its_point_and_analog_clip_is_linear_below_theirs() {
    let input = ramp(4096);
    let mut digital = singe(&[(CURVE, DIGITAL), (DRIVE, 0.0)], 4096);
    let out = run(digital.as_mut(), &input, &[]);
    for (x, y) in input.iter().zip(&out[0]) {
        if x.abs() >= 1.0 {
            assert_eq!(y.abs(), 1.0);
        } else {
            assert!((x - y).abs() < 1e-6, "linear below its point");
        }
    }
    let mut analog = singe(&[(CURVE, ANALOG), (DRIVE, 0.0)], 4096);
    let out = run(analog.as_mut(), &input, &[]);
    let linear = input
        .iter()
        .zip(&out[0])
        .filter(|(x, _)| x.abs() <= 0.5)
        .all(|(x, y)| (x - y).abs() < 1e-6);
    assert!(linear, "Analog Clip is the identity up to 0.5");
    let bent = input
        .iter()
        .zip(&out[0])
        .any(|(x, y)| x.abs() > 0.9 && (x.abs() - y.abs()) > 0.05);
    assert!(bent, "and eases off above it");
}

// Spec: saturator R4
#[test]
fn sinoid_fold_folds_back_past_its_point() {
    let input = ramp(4096);
    let mut fold = singe(&[(CURVE, SINOID_FOLD), (DRIVE, 6.0206)], 4096);
    let out = run(fold.as_mut(), &input, &[]);
    // The input after Drive reaches 2: past 1.285 the output falls from 1.
    let peak = out[0].iter().copied().fold(0.0f32, f32::max);
    let end = *out[0].last().unwrap();
    assert!((peak - 1.0).abs() < 0.01, "it peaks at 1: {peak}");
    assert!(
        end < 0.9,
        "and has folded back by the end of the ramp: {end}"
    );
    let mut soft = singe(&[(CURVE, SOFT_SINE), (DRIVE, 6.0206)], 4096);
    let held = *run(soft.as_mut(), &input, &[])[0].last().unwrap();
    assert!((held - 1.0).abs() < 1e-6, "Soft Sine holds instead: {held}");
}

// Spec: saturator R5
#[test]
fn bass_shapers_threshold_moves_it_from_the_soft_shape_to_the_hard_one() {
    let at = |threshold: f64, x: f32| {
        let mut node = singe(
            &[(CURVE, BASS_SHAPER), (DRIVE, 0.0), (THRESHOLD, threshold)],
            64,
        );
        run(node.as_mut(), &[x; 64], &[])[0][63]
    };
    // At 0 dB it is linear to full scale and clips there; at -50 dB it
    // eases from almost nothing.
    assert!((at(0.0, 0.9) - 0.9).abs() < 1e-6);
    assert_eq!(at(0.0, 1.5), 1.0);
    let soft = at(-50.0, 0.9);
    assert!((soft - 0.9 / 1.9).abs() < 0.01, "{soft}");
    // In between, a higher threshold stays linear for longer.
    assert!(at(-6.0, 0.4) > at(-24.0, 0.4));
    assert!(at(-6.0, 0.4) <= 0.4 + 1e-6);
}

// Spec: saturator R6
#[test]
fn drive_is_a_gain_in_decibels_ahead_of_the_curve() {
    for drive_db in [-36.0, 0.0, 12.0, 36.0] {
        let error = transfer_error(&[(CURVE, ANALOG)], drive_db, -12.0, Ws::DEFAULT);
        assert!(error < 0.001, "{drive_db} dB: {error}");
    }
}

// Spec: saturator R7
#[test]
fn each_waveshaper_control_moves_the_curve_as_the_spec_states() {
    let base = Ws::DEFAULT;
    let cases = [
        (
            "Curve",
            Ws {
                curve: 100.0,
                ..base
            },
        ),
        ("Curve", Ws { curve: 0.0, ..base }),
        (
            "Depth",
            Ws {
                depth: 100.0,
                ..base
            },
        ),
        (
            "Linear",
            Ws {
                linear: 60.0,
                depth: 50.0,
                ..base
            },
        ),
        ("Damp", Ws { damp: 80.0, ..base }),
        (
            "Period",
            Ws {
                period: 9.0,
                depth: 100.0,
                ..base
            },
        ),
        (
            "Drive",
            Ws {
                drive: 40.0,
                ..base
            },
        ),
    ];
    let reference = |ws: Ws| {
        (0..200)
            .map(|i| spec(WAVESHAPER, -12.0, ws, -1.0 + i as f64 / 100.0))
            .collect::<Vec<_>>()
    };
    for (name, ws) in cases {
        let mut params = vec![(CURVE, WAVESHAPER)];
        params.extend(ws.params());
        let error = transfer_error(&params, 0.0, -12.0, ws);
        assert!(error < 0.001, "{name}: {error}");
        let moved = reference(ws)
            .iter()
            .zip(reference(base))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(moved > 0.01, "{name} moves the curve: {moved}");
    }
    // Drive 0% leaves it straight, however the other controls are set.
    let loud = Ws {
        drive: 0.0,
        curve: 100.0,
        depth: 100.0,
        damp: 100.0,
        ..base
    };
    let mut params = vec![(CURVE, WAVESHAPER)];
    params.extend(loud.params());
    assert!(transfer_error(&params, 0.0, -12.0, loud) < 0.001);
    let straight = spec(WAVESHAPER, -12.0, loud, 0.37);
    assert!((straight - 0.37).abs() < 1e-12);
    // Linear at 100% is straight too.
    let linear = Ws {
        linear: 100.0,
        depth: 100.0,
        ..base
    };
    assert!((spec(WAVESHAPER, -12.0, linear, 0.37) - 0.37).abs() < 1e-12);
}

// Spec: saturator R8
#[test]
fn post_clip_holds_the_output_at_or_under_output() {
    // A Waveshaper with a deep ripple reaches 1.25 without Post Clip.
    let shape = [
        (CURVE, WAVESHAPER),
        (WS_CURVE, 100.0),
        (WS_DEPTH, 100.0),
        (WS_PERIOD, 4.5),
        (DRIVE, 12.0),
    ];
    let peak = |post: f64, output_db: f64| {
        let mut params = shape.to_vec();
        params.push((POST_CLIP, post));
        params.push((OUTPUT, db_to_gain(output_db)));
        let mut node = singe(&params, 4096);
        run(node.as_mut(), &ramp(4096), &[])[0]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()))
    };
    for output_db in [0.0, -6.0] {
        let ceiling = db_to_gain(output_db) as f32;
        assert!(peak(0.0, output_db) > ceiling * 1.1, "Off lets it through");
        assert!(peak(1.0, output_db) <= ceiling, "Soft");
        assert!(peak(2.0, output_db) <= ceiling, "Hard");
    }
    // Soft rounds the peaks and Hard reaches the ceiling.
    assert!(peak(1.0, 0.0) < 1.0);
    assert!((peak(2.0, 0.0) - 1.0).abs() < 1e-6);
}

// Spec: saturator R9
#[test]
fn dry_wet_at_zero_is_the_input_and_the_halves_add() {
    let input = noise(2048);
    let mut dry = singe(&[(CURVE, DIGITAL), (DRIVE, 24.0), (DRY_WET, 0.0)], 2048);
    assert_eq!(run(dry.as_mut(), &input, &[])[0], input);
    let wet = run(
        singe(&[(CURVE, DIGITAL), (DRIVE, 24.0)], 2048).as_mut(),
        &input,
        &[],
    );
    let half = run(
        singe(&[(CURVE, DIGITAL), (DRIVE, 24.0), (DRY_WET, 50.0)], 2048).as_mut(),
        &input,
        &[],
    );
    for ((d, w), h) in input.iter().zip(&wet[0]).zip(&half[0]) {
        assert!((0.5 * (d + w) - h).abs() < 1e-6);
    }
}

/// The relative RMS difference, in dB, between two signals.
fn difference_db(a: &[f32], b: &[f32]) -> f64 {
    let error: f64 = a.iter().zip(b).map(|(x, y)| f64::from(x - y).powi(2)).sum();
    let signal: f64 = a.iter().map(|x| f64::from(*x).powi(2)).sum();
    10.0 * (error / signal).max(1e-30).log10()
}

// Spec: saturator R10
#[test]
fn the_color_filters_cancel_at_zero_and_are_undone_after_the_curve() {
    let input = noise(8192);
    // Low drive keeps the curve in its linear part, so what Color does
    // before it, it undoes after it.
    let low = [(CURVE, ANALOG), (DRIVE, -30.0)];
    let plain = run(singe(&low, 8192).as_mut(), &input, &[]);
    let mut with: Vec<_> = low.to_vec();
    with.push((COLOR, 1.0));
    let zero = run(singe(&with, 8192).as_mut(), &input, &[]);
    assert!(difference_db(&plain[0], &zero[0]) < -100.0, "amounts at 0");
    with.extend([
        (AMT_LO, 100.0),
        (AMT_HI, -80.0),
        (FREQUENCY, 2_000.0),
        (WIDTH, 1.0),
    ]);
    let shaped = run(singe(&with, 8192).as_mut(), &input, &[]);
    assert!(
        difference_db(&plain[0], &shaped[0]) < -80.0,
        "the filters undo each other"
    );
    // At a hard drive, the amounts change what the curve hears.
    let hard = [(CURVE, ANALOG), (DRIVE, 12.0), (COLOR, 1.0)];
    let flat = run(singe(&hard, 8192).as_mut(), &input, &[]);
    let boosted = run(
        singe(&[hard.as_slice(), &[(AMT_LO, 100.0)]].concat(), 8192).as_mut(),
        &input,
        &[],
    );
    assert!(difference_db(&flat[0], &boosted[0]) > -30.0, "Amt Lo");
    let cut = run(
        singe(&[hard.as_slice(), &[(AMT_HI, 100.0)]].concat(), 8192).as_mut(),
        &input,
        &[],
    );
    assert!(difference_db(&flat[0], &cut[0]) > -30.0, "Amt Hi");
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

/// How far below the 7 kHz fundamental the loudest folded-back component
/// of `samples` (4,800 frames of a 7 kHz sine's output at 48 kHz) is: every
/// bin of a whole kilohertz but the fundamental and the harmonics below
/// the Nyquist rate (14 and 21 kHz).
fn alias_db(samples: &[f32]) -> f64 {
    let fundamental = magnitude(samples, 700);
    let loudest = (1..24)
        .filter(|k| ![7, 14, 21].contains(k))
        .map(|k| magnitude(samples, k * 100))
        .fold(0.0, f64::max);
    20.0 * (loudest / fundamental).log10()
}

// Spec: saturator R11
#[test]
fn hi_quality_keeps_a_hard_driven_sines_harmonics_from_folding_back() {
    let sine: Vec<f32> = (0..4800 + 56)
        .map(|i| (0.5 * (std::f64::consts::TAU * 7_000.0 * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect();
    let params = [(CURVE, ANALOG), (DRIVE, 18.0)];
    let plain = run(singe(&params, 4856).as_mut(), &sine, &[]);
    let mut hq = params.to_vec();
    hq.push((HI_QUALITY, 1.0));
    let mut node = singe(&hq, 4856);
    let latency = node.latency() as usize;
    assert_eq!(latency, 56, "8 times the rate: three stages");
    let clean = run(node.as_mut(), &sine, &[]);
    let (plain, clean) = (
        alias_db(&plain[0][56..]),
        alias_db(&clean[0][latency..latency + 4800]),
    );
    assert!(clean < -60.0, "folded-back harmonics {clean} dB down");
    assert!(
        plain > clean + 10.0,
        "and well above that without it: {plain} dB"
    );
    assert_eq!(singe(&params, 64).latency(), 0, "no latency without it");
}

// Spec: saturator R11
#[test]
fn hi_quality_delays_the_dry_signal_as_long_as_the_wet_one() {
    let input = noise(1024);
    let mut node = singe(
        &[
            (CURVE, DIGITAL),
            (DRIVE, 24.0),
            (HI_QUALITY, 1.0),
            (DRY_WET, 0.0),
        ],
        1024,
    );
    let out = run(node.as_mut(), &input, &[]);
    assert!(out[0][..56].iter().all(|s| *s == 0.0));
    assert_eq!(out[0][56..], input[..1024 - 56]);
}

// Spec: saturator R12
#[test]
fn the_pre_dc_filter_takes_a_dc_offset_down_within_a_second() {
    let frames = 48_000;
    let input = vec![0.5f32; frames];
    let tail = |dc: f64| {
        let mut node = singe(&[(CURVE, ANALOG), (DRIVE, 0.0), (PRE_DC, dc)], frames);
        let out = run(node.as_mut(), &input, &[]);
        out[0][frames - 100..]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()))
    };
    assert!(tail(0.0) > 0.4, "without it the offset passes");
    // -80 dB under the offset is 5e-5.
    assert!(
        tail(1.0) < 5e-5,
        "with it the offset is gone: {}",
        tail(1.0)
    );
}

// Spec: saturator R13
#[test]
fn the_display_holds_the_curve_and_the_inputs_level() {
    let device_type = saturator_type();
    let description = &device_type.description;
    let mut values = bl_model::OrdMap::new();
    values.insert(CURVE, DIGITAL);
    values.insert(DRIVE, 0.0);
    let id = bl_model::DeviceId(bl_core::id::Id::new(1).unwrap());
    let shown = bl_devices::display(SATURATOR, id, description, &values, None).unwrap();
    assert!(!shown.live);
    let curve = shown.series("Shaper Curve").unwrap();
    assert_eq!(curve.values.len(), bl_devices::saturator::CURVE_POINTS);
    assert_eq!(curve.values[0], -1.0, "clipped at the left end");
    assert_eq!(
        curve.values[curve.values.len() / 2],
        0.0,
        "through the middle"
    );
    // The node publishes the input's peak in dB after each block.
    let mut node = singe(&[(CURVE, DIGITAL), (DRIVE, 0.0)], 256);
    let mut reader = node.display().expect("the device has a display");
    run(node.as_mut(), &[0.25; 256], &[]);
    let data = reader.read();
    assert_eq!(data.published, 1);
    assert!((data.values[0] - 20.0 * 0.25f32.log10()).abs() < 1e-4);
    let live =
        bl_devices::display(SATURATOR, id, description, &values, Some(&data.values)).unwrap();
    assert!(live.live);
    assert!((live.reading("Input").unwrap().value + 12.04).abs() < 0.01);
    assert_eq!(live.markers.len(), 1, "the level now, on the curve");
}

// Spec: saturator R14
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let input = noise(8192);
    let params = [
        (CURVE, WAVESHAPER),
        (DRIVE, 14.0),
        (WS_DEPTH, 40.0),
        (WS_DAMP, 30.0),
        (COLOR, 1.0),
        (AMT_LO, 70.0),
        (AMT_HI, -60.0),
        (PRE_DC, 1.0),
        (HI_QUALITY, 1.0),
        (POST_CLIP, 1.0),
        (DRY_WET, 80.0),
    ];
    let render = |block: usize| -> Vec<f32> {
        let mut node = singe(&params, block);
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
