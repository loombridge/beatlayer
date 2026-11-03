//! Pedal, the native guitar pedal (WP-8.41; the pedal spec): the drive law and
//! the latency, each circuit against its formula and against a model of the
//! whole circuit, Fuzz's sputter, DC, the EQ at each Mid position, Output and
//! Dry/Wet, aliasing, the crossfade between types, and renders that don't
//! depend on the block size.

use std::f64::consts::{PI, TAU};

use crate::common;
use bl_devices::pedal::{
    BASS, DRY_WET, GAIN, HI_QUALITY, MID, MID_FREQUENCY, MID_HZ, OUTPUT, PEDAL, SUB, TREBLE, TYPE,
    TYPES, distortion_curve, drive, fuzz_bias, fuzz_curve, overdrive_curve, pedal_type,
};
use bl_engine::{Node, Registry};
use bl_model::ParamId;
use common::response::{Kind, biquad_response, db, rbj};
use common::{RATE, device, node, noise, run, set};

const OVERDRIVE: f64 = 0.0;
const DISTORTION: f64 = 1.0;
const FUZZ: f64 = 2.0;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::pedal::register(&mut registry);
    registry
}

/// A Pedal of `params`, up to `max_frames` at a time.
fn pedal(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    node(&registry(), &device(&pedal_type(), params), max_frames)
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

/// The magnitude of `samples`, taken at `rate`, at `hz`: the window must
/// hold a whole number of its cycles.
fn magnitude(samples: &[f64], rate: f64, hz: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in samples.iter().enumerate() {
        let phase = TAU * hz * i as f64 / rate;
        re += s * phase.cos();
        im -= s * phase.sin();
    }
    re.hypot(im) * 2.0 / samples.len() as f64
}

fn widen(samples: &[f32]) -> Vec<f64> {
    samples.iter().map(|s| f64::from(*s)).collect()
}

fn db_of(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// A model of one circuit and the DC filter after it, written again here in
/// f64 at `rate`: the sine of `hz` and `amplitude`, `frames` long, is
/// generated at that rate, so it needs no oversampler. The EQ is flat.
fn model(kind: f64, gain: f64, hz: f64, amplitude: f64, rate: f64, frames: usize) -> Vec<f64> {
    let one_pole = |x: &[f64], corner: f64, high: bool| -> Vec<f64> {
        let k = (PI * corner / rate).tan();
        let b = if high { 1.0 } else { k } / (1.0 + k);
        let a1 = (k - 1.0) / (1.0 + k);
        let (mut x1, mut y1) = (0.0, 0.0);
        x.iter()
            .map(|x| {
                let y = b * (x + if high { -x1 } else { x1 }) - a1 * y1;
                (x1, y1) = (*x, y);
                y
            })
            .collect()
    };
    let low_pass_2 = |x: &[f64], corner: f64| -> Vec<f64> {
        let w0 = TAU * corner / rate;
        let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b0 = (1.0 - w0.cos()) / 2.0 / a0;
        let (b1, a1, a2) = (2.0 * b0, -2.0 * w0.cos() / a0, (1.0 - alpha) / a0);
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        x.iter()
            .map(|x| {
                let y = b0 * x + b1 * x1 + b0 * x2 - a1 * y1 - a2 * y2;
                (x2, x1, y2, y1) = (x1, *x, y1, y);
                y
            })
            .collect()
    };
    let x: Vec<f64> = (0..frames)
        .map(|i| amplitude * (TAU * hz * i as f64 / rate).sin())
        .collect();
    let g = 10f64.powf(gain / 100.0 * 48.0 / 20.0);
    let y = if kind == OVERDRIVE {
        let u = one_pole(&x, 720.0, true);
        let y: Vec<f64> = u
            .iter()
            .map(|u| g * (u + (g * u).tanh()) / (1.0 + g))
            .collect();
        one_pole(&y, 6_000.0, false)
    } else if kind == DISTORTION {
        let u = one_pole(&x, 100.0, true);
        let y: Vec<f64> = u
            .iter()
            .map(|u| (g * u).signum() * (1.0 - (-(g * u).abs()).exp()))
            .collect();
        low_pass_2(&y, 5_000.0)
    } else {
        let coefficient = |seconds: f64| 1.0 - (-1.0 / (seconds * rate)).exp();
        let (attack, release) = (coefficient(0.002), coefficient(0.08));
        let mut level = 0.0f64;
        let y: Vec<f64> = x
            .iter()
            .map(|x| {
                let input = x.abs();
                level += (input - level) * if input > level { attack } else { release };
                let bias = 0.3 - 0.25 * level.min(1.0);
                let stage = (g * (x + bias)).tanh() - (g * bias).tanh();
                (3.0 * stage).tanh()
            })
            .collect();
        one_pole(&y, 4_000.0, false)
    };
    one_pole(&y, 5.0, true)
}

/// The device's output for a sine, after it has settled, as a window of
/// `window` frames.
fn device_tail(params: &[(ParamId, f64)], hz: f64, amplitude: f64, window: usize) -> Vec<f64> {
    let frames = 48_000;
    let out = run(
        pedal(params, frames).as_mut(),
        &sine(hz, amplitude, frames),
        &[],
    );
    widen(&out[0][frames - window..])
}

/// The model's output for the same sine, at four times the rate.
fn model_tail(kind: f64, gain: f64, hz: f64, amplitude: f64, window: usize) -> Vec<f64> {
    let rate = 4.0 * RATE.as_f64();
    let frames = 4 * 48_000;
    model(kind, gain, hz, amplitude, rate, frames)[frames - 4 * window..].to_vec()
}

// Spec: pedal R1, R2
#[test]
fn the_pedal_is_described_and_laid_out() {
    let device_type = pedal_type();
    device_type.description.check().unwrap();
    assert_eq!(
        device_type.kind,
        bl_model::DeviceKind::Native {
            device_type: PEDAL.to_owned()
        }
    );
    assert!(
        bl_devices::device_types().iter().any(|t| t.name == "Pedal"),
        "the registry lists it"
    );
    let default = |id: ParamId| {
        device_type
            .description
            .params
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .default
    };
    assert_eq!(TYPES, ["Overdrive", "Distortion", "Fuzz"]);
    assert_eq!(default(TYPE), 0.0, "Overdrive in a new device");
    assert_eq!(default(GAIN), 50.0);
    assert_eq!(default(MID_FREQUENCY), 1.0, "1 kHz");
    assert_eq!(MID_HZ, [500.0, 1_000.0, 2_000.0]);
    assert_eq!(default(DRY_WET), 100.0);
    assert_eq!(default(HI_QUALITY), 0.0);
    assert_eq!(device_type.description.menu_options, [HI_QUALITY]);
}

// Spec: pedal R2
#[test]
fn gain_drives_the_circuit_by_zero_to_forty_eight_decibels() {
    assert!((drive(0.0) - 1.0).abs() < 1e-6);
    assert!((20.0 * drive(50.0).log10() - 24.0).abs() < 1e-3);
    assert!((20.0 * drive(100.0).log10() - 48.0).abs() < 1e-3);
    // At small signal each circuit's gain is its drive times its filters'
    // gain at 1 kHz, which the prewarped corners make exact.
    let corner = |hz: f64| (PI * hz / (4.0 * RATE.as_f64())).tan();
    let high_pass = |hz: f64| 1.0 / (1.0 + (corner(hz) / corner(1_000.0)).powi(2)).sqrt();
    let low_pass = |hz: f64| 1.0 / (1.0 + (corner(1_000.0) / corner(hz)).powi(2)).sqrt();
    let low_pass_2 = |hz: f64| 1.0 / (1.0 + (corner(1_000.0) / corner(hz)).powi(4)).sqrt();
    for gain in [0.0, 25.0, 50.0] {
        for (kind, filters) in [
            (OVERDRIVE, high_pass(720.0) * low_pass(6_000.0)),
            (DISTORTION, high_pass(100.0) * low_pass_2(5_000.0)),
        ] {
            let amplitude = 1e-4;
            let out = device_tail(&[(TYPE, kind), (GAIN, gain)], 1_000.0, amplitude, 4_800);
            let measured = magnitude(&out, RATE.as_f64(), 1_000.0) / amplitude;
            let expected = f64::from(drive(gain as f32)) * filters;
            assert!(
                db_of(measured / expected).abs() < 0.3,
                "type {kind}, Gain {gain}%: {measured} against {expected}"
            );
        }
    }
}

// Spec: pedal R1, R8
#[test]
fn the_latency_is_the_oversamplers_for_the_quality_and_delays_the_dry_signal() {
    assert_eq!(pedal(&[], 64).latency(), 48, "four times the rate");
    assert_eq!(
        pedal(&[(HI_QUALITY, 1.0)], 64).latency(),
        56,
        "eight times the rate"
    );
    for (hi, latency) in [(0.0, 48), (1.0, 56)] {
        let input = noise(1_024);
        let out = run(
            pedal(&[(DRY_WET, 0.0), (HI_QUALITY, hi)], 1_024).as_mut(),
            &input,
            &[],
        );
        assert!(out[0][..latency].iter().all(|s| *s == 0.0));
        assert_eq!(out[0][latency..], input[..1_024 - latency]);
    }
}

// Spec: pedal R3 to R6
#[test]
fn each_types_curve_lies_within_a_hundredth_of_its_formula() {
    for gain in [0.0, 30.0, 50.0, 100.0] {
        let g = drive(gain);
        let g64 = f64::from(g);
        for i in 0..=400 {
            let u = -1.0 + i as f32 / 200.0;
            let x = f64::from(u);
            let overdrive = g64 * (x + (g64 * x).tanh()) / (1.0 + g64);
            let distortion = (g64 * x).signum() * (1.0 - (-(g64 * x).abs()).exp());
            assert!(
                (f64::from(overdrive_curve(g, u)) - overdrive).abs() < 0.01,
                "Overdrive at {u}, Gain {gain}%"
            );
            assert!(
                (f64::from(distortion_curve(g, u)) - distortion).abs() < 0.01,
                "Distortion at {u}, Gain {gain}%"
            );
            // Fuzz with its envelope held at three levels.
            for envelope in [0.0, 0.5, 1.0] {
                let bias = 0.3 - 0.25 * f64::from(envelope);
                assert!((f64::from(fuzz_bias(envelope)) - bias).abs() < 1e-6);
                let stage = (g64 * (x + bias)).tanh() - (g64 * bias).tanh();
                let fuzz = (3.0 * stage).tanh();
                assert!(
                    (f64::from(fuzz_curve(g, fuzz_bias(envelope), u)) - fuzz).abs() < 0.01,
                    "Fuzz at {u}, envelope {envelope}, Gain {gain}%"
                );
            }
        }
        // Overdrive's slope at zero is its drive.
        let slope = overdrive_curve(g, 1e-6) / 1e-6;
        assert!((slope / g - 1.0).abs() < 1e-3);
    }
    // The bias falls by 0.25 and no more.
    assert!((fuzz_bias(0.0) - 0.3).abs() < 1e-6);
    assert!((fuzz_bias(1.0) - 0.05).abs() < 1e-6);
    assert!((fuzz_bias(7.0) - 0.05).abs() < 1e-6);
}

// Spec: pedal R6
#[test]
fn each_types_harmonics_on_a_220_hz_sine_are_those_of_its_design_within_a_decibel() {
    // 11 cycles of 220 Hz are 2,400 frames.
    let window = 2_400;
    let amplitude = 10f64.powf(-12.0 / 20.0);
    let rate = RATE.as_f64();
    for kind in [OVERDRIVE, DISTORTION, FUZZ] {
        let out = device_tail(&[(TYPE, kind), (GAIN, 50.0)], 220.0, amplitude, window);
        let design = model_tail(kind, 50.0, 220.0, amplitude, window);
        let harmonic = |samples: &[f64], rate: f64, k: f64| magnitude(samples, rate, 220.0 * k);
        let (first, second, third) = (1.0, 2.0, 3.0);
        let balance = |samples: &[f64], rate: f64| {
            let h = |k| harmonic(samples, rate, k);
            (db_of(h(second) / h(third)), db_of(h(third) / h(first)))
        };
        let (heard_balance, heard_third) = balance(&out, rate);
        let (design_balance, design_third) = balance(&design, 4.0 * rate);
        // A symmetric curve has no second harmonic to speak of; Fuzz's has
        // one and it matches the design's.
        if kind == FUZZ {
            assert!(
                (heard_balance - design_balance).abs() < 1.0,
                "Fuzz: second against third {heard_balance} dB, design {design_balance} dB"
            );
        } else {
            assert!(heard_balance < -60.0, "type {kind}: {heard_balance} dB");
        }
        assert!(
            (heard_third - design_third).abs() < 1.0,
            "type {kind}: third against first {heard_third} dB, design {design_third} dB"
        );
        let level = |samples: &[f64], rate: f64| db_of(harmonic(samples, rate, first));
        assert!(
            (level(&out, rate) - level(&design, 4.0 * rate)).abs() < 1.0,
            "type {kind}: the fundamental"
        );
    }
}

// Spec: pedal R5
#[test]
fn fuzz_sputters_on_a_decaying_tone_and_repeats_run_after_run() {
    let frames = 48_000;
    let input: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f64 / RATE.as_f64();
            (0.6 * (-t / 0.15).exp() * (TAU * 220.0 * t).sin()) as f32
        })
        .collect();
    let render = || {
        let params = [(TYPE, FUZZ), (GAIN, 70.0)];
        run(pedal(&params, frames).as_mut(), &input, &[])
    };
    let (first, second) = (render(), render());
    assert!(first == second, "the same render twice");
    // Loud, the stage is open; as the tone decays the bias comes back up and
    // the stage gates, so the output falls faster than the input.
    let peak = |samples: &[f32], from: usize| {
        samples[from..from + 2_400]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    };
    let (loud, quiet) = (peak(&first[0], 4_800), peak(&first[0], 36_000));
    let (in_loud, in_quiet) = (peak(&input, 4_800), peak(&input, 36_000));
    let drop_in = db_of(f64::from(in_quiet / in_loud));
    let drop_out = db_of(f64::from(quiet / loud));
    assert!(
        drop_out < drop_in - 20.0,
        "the input falls {drop_in} dB, the output {drop_out} dB"
    );
}

// Spec: pedal R6
#[test]
fn each_type_leaves_no_dc_above_minus_eighty_dbfs() {
    let window = 4_800;
    for kind in [OVERDRIVE, DISTORTION, FUZZ] {
        let out = device_tail(&[(TYPE, kind), (GAIN, 100.0)], 220.0, 0.5, window);
        let mean = out.iter().sum::<f64>() / window as f64;
        assert!(
            db_of(mean.abs().max(1e-12)) < -80.0,
            "type {kind}: DC at {} dBFS",
            db_of(mean.abs())
        );
    }
}

/// The EQ's response at `hz`, in dB: the device's impulse response with the
/// EQ as `params` set it, over its response with the EQ flat. Gain is at 0%,
/// where Overdrive is nearly linear for a small signal.
fn eq_response(params: &[(ParamId, f64)], hz: f64) -> f64 {
    let frames = 24_000;
    let amplitude = 1e-3f32;
    let ir = |extra: &[(ParamId, f64)]| {
        let mut input = vec![0.0f32; frames];
        input[0] = amplitude;
        let mut all = vec![(GAIN, 0.0)];
        all.extend_from_slice(extra);
        run(pedal(&all, frames).as_mut(), &input, &[])[0].clone()
    };
    let at = |ir: &[f32]| common::response::response_at(ir, hz);
    let (shaped, flat) = (at(&ir(params)), at(&ir(&[])));
    db(shaped) - db(flat)
}

// Spec: pedal R7
#[test]
fn each_eq_band_is_within_half_a_decibel_of_its_design_at_each_mid_position() {
    let design = |kind, f0, gain, q, hz| db(biquad_response(rbj(kind, f0, gain, q), hz));
    let probes = [
        40.0, 100.0, 250.0, 500.0, 1_000.0, 2_000.0, 3_300.0, 8_000.0,
    ];
    for hz in probes {
        for gain in [-15.0, 12.0] {
            let got = eq_response(&[(BASS, gain)], hz);
            let want = design(Kind::Peak, 100.0, gain, 0.7, hz);
            assert!(
                (got - want).abs() < 0.5,
                "Bass {gain} dB at {hz}: {got} for {want}"
            );
            let got = eq_response(&[(TREBLE, gain)], hz);
            let want = design(Kind::HighShelf, 3_300.0, gain, 0.71, hz);
            assert!(
                (got - want).abs() < 0.5,
                "Treble {gain} dB at {hz}: {got} for {want}"
            );
            for (position, mid_hz) in MID_HZ.iter().enumerate() {
                let got = eq_response(&[(MID, gain), (MID_FREQUENCY, position as f64)], hz);
                let want = design(Kind::Peak, *mid_hz, gain, 1.0, hz);
                assert!(
                    (got - want).abs() < 0.5,
                    "Mid {gain} dB at {mid_hz} Hz, measured at {hz}: {got} for {want}"
                );
            }
        }
    }
}

// Spec: pedal R7
#[test]
fn sub_raises_the_lows_with_a_shelf_below_250_hz() {
    for gain in [6.0, 15.0] {
        for hz in [30.0, 100.0, 250.0, 1_000.0, 8_000.0] {
            let got = eq_response(&[(SUB, gain)], hz);
            let want = db(biquad_response(rbj(Kind::LowShelf, 250.0, gain, 0.71), hz));
            assert!(
                (got - want).abs() < 0.5,
                "Sub {gain} dB at {hz}: {got} for {want}"
            );
        }
        let low = eq_response(&[(SUB, gain)], 30.0);
        let high = eq_response(&[(SUB, gain)], 8_000.0);
        assert!(
            low > gain - 1.0 && high.abs() < 0.5,
            "{low} dB down low, {high} dB up high"
        );
    }
    assert!(eq_response(&[], 100.0).abs() < 0.01, "flat at 0 dB");
}

// Spec: pedal R8
#[test]
fn output_sets_the_wet_level_down_to_silence_and_dry_wet_balances_it() {
    let frames = 4_800;
    let input = sine(1_000.0, 0.01, frames);
    let rms = |out: &[f32]| {
        (out[2_400..]
            .iter()
            .map(|s| f64::from(*s).powi(2))
            .sum::<f64>()
            / 2_400.0)
            .sqrt()
    };
    let wet = |params: &[(ParamId, f64)]| rms(&run(pedal(params, frames).as_mut(), &input, &[])[0]);
    let base = wet(&[(GAIN, 0.0)]);
    let twelve = wet(&[(GAIN, 0.0), (OUTPUT, 10f64.powf(12.0 / 20.0))]);
    assert!((db_of(twelve / base) - 12.0).abs() < 0.01, "+12 dB");
    assert!(wet(&[(OUTPUT, 0.0)]) == 0.0, "−∞ is silence");
    let half = wet(&[(GAIN, 0.0), (DRY_WET, 50.0), (OUTPUT, 0.0)]);
    let dry = rms(&run(pedal(&[(DRY_WET, 0.0)], frames).as_mut(), &input, &[])[0]);
    // With the wet silent, half the dry signal remains.
    assert!((half / dry - 0.5).abs() < 1e-3, "{half} against {dry}");
}

/// The loudest line below 20 kHz that isn't the 5 kHz sine or one of its
/// harmonics, against the sine, in dB. Every line a harmonic folds to is on
/// a whole-kilohertz grid, so the window of 4,800 frames holds it in a bin.
fn alias_db(samples: &[f32]) -> f64 {
    let samples = widen(samples);
    let at = |khz: usize| magnitude(&samples, RATE.as_f64(), khz as f64 * 1_000.0);
    let loudest = (1..20).filter(|k| k % 5 != 0).map(at).fold(0.0, f64::max);
    db_of(loudest / at(5))
}

/// A 5 kHz sine of `amplitude` at Gain 100% through `kind`, as a window.
fn alias_of(kind: f64, hi_quality: f64, amplitude: f64) -> f64 {
    let latency = if hi_quality > 0.5 { 56 } else { 48 };
    let frames = 9_600 + latency + 4_800;
    let out = run(
        pedal(
            &[(TYPE, kind), (GAIN, 100.0), (HI_QUALITY, hi_quality)],
            frames,
        )
        .as_mut(),
        &sine(5_000.0, amplitude, frames),
        &[],
    );
    alias_db(&out[0][frames - 4_800..])
}

// Spec: pedal R9
#[test]
fn a_5_khz_sine_at_gain_100_aliases_less_than_60_db_and_with_hi_quality_75_db() {
    // At full drive the clipped edges' harmonics past 96 kHz (192 kHz at
    // four times the rate, 384 kHz at eight) fold back into the band and no
    // filter takes them out again, so the levels hold for a sine at -36 dBFS
    // (0.016), where the drive of 48 dB doesn't yet make a square of it.
    // Fuzz's stage is closed to a signal that small.
    for kind in [OVERDRIVE, DISTORTION] {
        let plain = alias_of(kind, 0.0, 0.016);
        let clean = alias_of(kind, 1.0, 0.016);
        assert!(plain < -60.0, "type {kind}: {plain} dB");
        assert!(clean < -75.0, "type {kind} with Hi-Quality: {clean} dB");
    }
}

// Spec: pedal R10
#[test]
fn a_change_of_type_crossfades_over_twenty_milliseconds() {
    let frames = 24_000;
    let at = 9_600;
    let input = sine(1_000.0, 0.2, frames);
    let render = |params: &[(ParamId, f64)], events: &[bl_engine::NodeEvent]| {
        run(pedal(params, frames).as_mut(), &input, events)[0].clone()
    };
    let before = render(&[(TYPE, OVERDRIVE)], &[]);
    let after = render(&[(TYPE, DISTORTION)], &[]);
    let switched = render(&[(TYPE, OVERDRIVE)], &[set(at, TYPE, DISTORTION)]);
    // Until the change it is Overdrive, exactly.
    assert!(switched[..at as usize] == before[..at as usize]);
    // Where the two differ most, right at the change the output hasn't
    // jumped, and a crossfade later it is Distortion.
    let gap = |a: &[f32], b: &[f32], from: usize, len: usize| {
        a[from..from + len]
            .iter()
            .zip(&b[from..from + len])
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()))
    };
    let start = at as usize;
    let apart = gap(&before, &after, start, 480);
    assert!(apart > 0.05, "the types differ by {apart}");
    // The first frames after the change are still Overdrive's, nearly.
    assert!(
        gap(&switched, &before, start, 16) < apart / 8.0,
        "no jump at the change"
    );
    // Halfway through the 960-sample fade it is neither.
    let half = start + 480;
    assert!(gap(&switched, &before, half, 8) > 0.01 && gap(&switched, &after, half, 8) > 0.01);
    // Once it is over it is Distortion.
    assert!(
        gap(&switched, &after, start + 4_800, 480) < 0.01,
        "Distortion once the fade is over"
    );
}

// Spec: pedal R9
#[test]
fn renders_are_bit_identical_at_four_block_sizes_and_run_after_run() {
    let input = noise(8_192);
    for (kind, hi_quality) in [
        (OVERDRIVE, 0.0),
        (DISTORTION, 1.0),
        (FUZZ, 0.0),
        (FUZZ, 1.0),
    ] {
        let params = [
            (TYPE, kind),
            (GAIN, 80.0),
            (BASS, 6.0),
            (MID, -9.0),
            (MID_FREQUENCY, 2.0),
            (TREBLE, 4.0),
            (SUB, 7.0),
            (DRY_WET, 80.0),
            (HI_QUALITY, hi_quality),
        ];
        let render = |block: usize| -> Vec<f32> {
            let mut node = pedal(&params, block);
            input
                .chunks(block)
                .flat_map(|chunk| run(node.as_mut(), chunk, &[])[0].clone())
                .collect()
        };
        let reference = render(4_096);
        assert!(render(4_096) == reference, "run after run, type {kind}");
        for block in [1, 64, 512] {
            assert!(
                render(block) == reference,
                "type {kind}, block size {block}"
            );
        }
    }
}
