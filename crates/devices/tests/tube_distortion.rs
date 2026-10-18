//! Dynamic Tube, the native tube distortion (WP-5.23; the tube distortion spec):
//! the three models' distortion against level and Bias, Tone, the envelope
//! and its times, Hi-Quality and its latency, the display, and renders that
//! don't depend on the block size.

bl_rt::install_trap!();

mod common;

use bl_devices::tube::{
    ATTACK, BIAS, DRIVE, ENVELOPE, HI_QUALITY, MODEL, OUTPUT, RELEASE, TONE, TUBE, tube_type,
};
use bl_engine::{DeviceType, Node, Registry};
use bl_model::ParamId;
use common::{RATE, device, node, noise, run};

const MODEL_A: f64 = 0.0;
const MODEL_B: f64 = 1.0;
const MODEL_C: f64 = 2.0;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::tube::register(&mut registry);
    registry
}

/// A Dynamic Tube of `params`, up to `max_frames` at a time.
fn valve(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = tube_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (std::f64::consts::TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
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

/// The harmonic distortion, in dB, of a 1 kHz sine of `amplitude` through a
/// Dynamic Tube of `params`: harmonics 2 to 9 against the fundamental, over 4,800
/// frames (100 cycles) after the envelope has settled.
fn distortion_db(params: &[(ParamId, f64)], amplitude: f64) -> f64 {
    let frames = 24_000;
    let out = run(
        valve(params, frames).as_mut(),
        &sine(1_000.0, amplitude, frames),
        &[],
    );
    let tail = &out[0][frames - 4_800..];
    let harmonics: f64 = (2..=9).map(|h| magnitude(tail, h * 100).powi(2)).sum();
    10.0 * (harmonics / magnitude(tail, 100).powi(2))
        .max(1e-30)
        .log10()
}

/// Model A at Bias 0 and Drive 0 dB, the clean case.
fn clean(model: f64) -> Vec<(ParamId, f64)> {
    vec![(MODEL, model), (BIAS, 0.0), (DRIVE, 0.0)]
}

// Spec: tube-distortion R1
#[test]
fn the_tube_is_described_and_laid_out() {
    let device_type = tube_type();
    device_type.description.check().unwrap();
    assert_eq!(
        device_type.kind,
        bl_model::DeviceKind::Native {
            device_type: TUBE.to_owned()
        }
    );
    assert_eq!(device_type.description.menu_options, vec![HI_QUALITY]);
    assert!(
        bl_devices::device_types()
            .iter()
            .any(|t| t.name == "Dynamic Tube")
    );
    assert_eq!(valve(&[], 64).latency(), 0, "no latency without Hi-Quality");
}

// Spec: tube-distortion R2
#[test]
fn tube_a_is_clean_under_its_threshold_at_low_bias_and_distorts_past_it() {
    let params = clean(MODEL_A);
    for amplitude in [0.01, 0.1, 0.4] {
        let d = distortion_db(&params, amplitude);
        assert!(d < -80.0, "{amplitude}: {d} dB");
    }
    let past = distortion_db(&params, 0.9);
    assert!(past > -40.0, "past the threshold: {past} dB");
    // More Bias brings the threshold down.
    let mut biased = params.clone();
    biased.push((BIAS, 40.0));
    assert!(distortion_db(&biased, 0.4) > -40.0);
}

// Spec: tube-distortion R3
#[test]
fn tube_c_distorts_at_every_level() {
    let params = clean(MODEL_C);
    for amplitude in [0.01, 0.1, 0.4] {
        let d = distortion_db(&params, amplitude);
        assert!(d > -80.0, "{amplitude}: {d} dB");
    }
}

// Spec: tube-distortion R4
#[test]
fn tube_b_lies_between_a_and_c() {
    for amplitude in [0.1, 0.3] {
        let [a, b, c] = [MODEL_A, MODEL_B, MODEL_C].map(|m| distortion_db(&clean(m), amplitude));
        assert!(a < b && b < c, "{amplitude}: A {a}, B {b}, C {c}");
    }
}

// Spec: tube-distortion R5
#[test]
fn the_envelope_makes_loud_sounds_dirtier_or_cleaner() {
    let base = [(MODEL, MODEL_A), (DRIVE, 0.0), (BIAS, 30.0)];
    let with = |envelope: f64| {
        let mut params = base.to_vec();
        params.push((ENVELOPE, envelope));
        distortion_db(&params, 0.25)
    };
    let (up, flat, down) = (with(100.0), with(0.0), with(-100.0));
    assert!(up > flat + 3.0, "positive: {up} against {flat}");
    assert!(down < flat - 3.0, "negative: {down} against {flat}");
    // At Envelope 0 the times do nothing.
    let input = noise(4096);
    let mut slow = base.to_vec();
    slow.extend([(ATTACK, 500.0), (RELEASE, 4_000.0)]);
    let mut fast = base.to_vec();
    fast.extend([(ATTACK, 0.2), (RELEASE, 2.0)]);
    assert!(
        run(valve(&slow, 4096).as_mut(), &input, &[])
            == run(valve(&fast, 4096).as_mut(), &input, &[])
    );
}

/// Model C's output for a constant `driven` signal when the envelope has
/// reached `level`, at Bias 0 and Envelope 100 %: the tanh at the operating
/// point `0.15 + level`, less the tanh at that point alone.
fn through(driven: f64, level: f64) -> f64 {
    (driven + 0.15 + level).tanh() - (0.15 + level).tanh()
}

/// The output of a Dynamic Tube (Model C, Bias 0, Envelope 100 %) for `steps`:
/// each a constant input held for the frames it names.
fn follow(attack_ms: f64, release_ms: f64, steps: &[(f32, usize)]) -> Vec<f32> {
    let params = [
        (MODEL, MODEL_C),
        (BIAS, 0.0),
        (DRIVE, 0.0),
        (ENVELOPE, 100.0),
        (ATTACK, attack_ms),
        (RELEASE, release_ms),
    ];
    let input: Vec<f32> = steps
        .iter()
        .flat_map(|(value, frames)| std::iter::repeat_n(*value, *frames))
        .collect();
    run(valve(&params, input.len()).as_mut(), &input, &[])[0].clone()
}

// Spec: tube-distortion R6
#[test]
fn attack_and_release_are_time_constants_within_five_per_cent() {
    let rate = RATE.as_f64();
    // Attack: a step from silence to 0.8. After n frames the level has
    // covered 1 - e^(-n/(tau * rate)) of the way.
    let attack_ms = 20.0;
    let out = follow(attack_ms, 100.0, &[(0.8, 24_000)]);
    for multiple in [0.5, 1.0, 2.0] {
        let n = (attack_ms / 1_000.0 * rate * multiple) as usize;
        let at = |scale: f64| {
            let level =
                0.8 * (1.0 - (-((n + 1) as f64) / (attack_ms / 1_000.0 * scale * rate)).exp());
            through(0.8, level)
        };
        let (low, high) = (at(0.95).min(at(1.05)), at(0.95).max(at(1.05)));
        let heard = f64::from(out[n]);
        assert!(
            heard >= low - 1e-6 && heard <= high + 1e-6,
            "attack at {multiple} tau: {heard} not in {low}..{high}"
        );
        // And the time constant isn't far off in either direction.
        let (slow, quick) = (at(1.2), at(0.8));
        assert!(heard > slow.min(quick) && heard < slow.max(quick));
    }
    // Release: 0.8 held for a second, then 0.1.
    let release_ms = 50.0;
    let hold = 48_000;
    let out = follow(5.0, release_ms, &[(0.8, hold), (0.1, 24_000)]);
    for multiple in [0.5, 1.0, 2.0] {
        let n = (release_ms / 1_000.0 * rate * multiple) as usize;
        let at = |scale: f64| {
            let level =
                0.1 + 0.7 * (-((n + 1) as f64) / (release_ms / 1_000.0 * scale * rate)).exp();
            through(0.1, level)
        };
        let (low, high) = (at(0.95).min(at(1.05)), at(0.95).max(at(1.05)));
        let heard = f64::from(out[hold + n]);
        assert!(
            heard >= low - 1e-6 && heard <= high + 1e-6,
            "release at {multiple} tau: {heard} not in {low}..{high}"
        );
    }
}

/// The magnitude at `hz` (a whole number of cycles in 4,800 frames) of the
/// output for `params` from two sines, 300 Hz and 5 kHz.
fn two_tone(params: &[(ParamId, f64)], amplitude: f64) -> impl Fn(f64) -> f64 + use<> {
    let frames = 24_000;
    let low = sine(300.0, amplitude, frames);
    let high = sine(5_000.0, amplitude, frames);
    let input: Vec<f32> = low.iter().zip(&high).map(|(a, b)| a + b).collect();
    let out = run(valve(params, frames).as_mut(), &input, &[]);
    let tail: Vec<f32> = out[0][frames - 4_800..].to_vec();
    move |hz: f64| magnitude(&tail, (hz / 10.0) as usize)
}

// Spec: tube-distortion R7
#[test]
fn tone_steers_the_distortion_and_leaves_the_clean_signal_alone() {
    // Where the tube is linear, Tone changes nothing.
    let small = |tone: f64| {
        let mut params = clean(MODEL_A);
        params.push((TONE, tone));
        two_tone(&params, 0.05)
    };
    let (dark, bright) = (small(0.0), small(100.0));
    for hz in [300.0, 5_000.0] {
        let (a, b) = (dark(hz), bright(hz));
        assert!((a / b - 1.0).abs() < 1e-3, "{hz} Hz: {a} against {b}");
    }
    // Hard driven, a bright Tone makes the 5 kHz sine's harmonic stronger
    // against the 300 Hz sine's.
    let hard = |tone: f64| {
        let mut params = clean(MODEL_C);
        params.push((TONE, tone));
        params.push((DRIVE, 6.0));
        let heard = two_tone(&params, 0.2);
        heard(10_000.0) / heard(600.0)
    };
    assert!(
        hard(100.0) > 1.5 * hard(0.0),
        "{} against {}",
        hard(100.0),
        hard(0.0)
    );
}

/// How far below the 7 kHz fundamental the loudest folded-back component
/// of `samples` (4,800 frames of a 7 kHz sine's output at 48 kHz) is: every
/// whole kilohertz but the fundamental and the harmonics below the Nyquist
/// rate (14 and 21 kHz).
fn alias_db(samples: &[f32]) -> f64 {
    let fundamental = magnitude(samples, 700);
    let loudest = (1..24)
        .filter(|k| ![7, 14, 21].contains(k))
        .map(|k| magnitude(samples, k * 100))
        .fold(0.0, f64::max);
    20.0 * (loudest / fundamental).log10()
}

// Spec: tube-distortion R8
#[test]
fn hi_quality_keeps_a_hard_driven_sines_harmonics_from_folding_back() {
    let input = sine(7_000.0, 0.5, 4800 + 56);
    for model in [MODEL_A, MODEL_B, MODEL_C] {
        let params = [(MODEL, model), (DRIVE, 18.0)];
        let plain = run(valve(&params, 4856).as_mut(), &input, &[]);
        let mut hq = params.to_vec();
        hq.push((HI_QUALITY, 1.0));
        let mut node = valve(&hq, 4856);
        let latency = node.latency() as usize;
        assert_eq!(latency, 56, "8 times the rate: three stages");
        let clean = run(node.as_mut(), &input, &[]);
        let (plain, clean) = (
            alias_db(&plain[0][56..]),
            alias_db(&clean[0][latency..latency + 4800]),
        );
        assert!(
            clean < -60.0,
            "model {model}: folded-back harmonics {clean} dB down"
        );
        assert!(
            plain > clean + 10.0,
            "model {model}: and well above that without it: {plain} dB"
        );
    }
}

// Spec: tube-distortion R9
#[test]
fn output_scales_the_result() {
    let input = sine(1_000.0, 0.3, 4_800);
    let full = run(valve(&[(MODEL, MODEL_C)], 4_800).as_mut(), &input, &[]);
    let half = run(
        valve(&[(MODEL, MODEL_C), (OUTPUT, 0.5)], 4_800).as_mut(),
        &input,
        &[],
    );
    for (f, h) in full[0].iter().zip(&half[0]) {
        assert!((f * 0.5 - h).abs() < 1e-6);
    }
}

// Spec: tube-distortion R10
#[test]
fn the_display_holds_the_curve_and_the_inputs_level() {
    let device_type = tube_type();
    let description = &device_type.description;
    let mut values = bl_model::OrdMap::new();
    values.insert(MODEL, MODEL_A);
    values.insert(BIAS, 0.0);
    values.insert(DRIVE, 0.0);
    let id = bl_model::DeviceId(bl_core::id::Id::new(1).unwrap());
    let shown = bl_devices::display(TUBE, id, description, &values, None).unwrap();
    assert!(!shown.live);
    let curve = shown.series("Tube Curve").unwrap();
    assert_eq!(curve.values.len(), bl_devices::tube::CURVE_POINTS);
    assert_eq!(
        curve.values[curve.values.len() / 2],
        0.0,
        "through the middle"
    );
    let mut node = valve(&[], 256);
    let mut reader = node.display().expect("the device has a display");
    run(node.as_mut(), &[0.5; 256], &[]);
    let data = reader.read();
    assert_eq!(data.published, 1);
    assert!((data.values[0] - 20.0 * 0.5f32.log10()).abs() < 1e-4);
    let live = bl_devices::display(TUBE, id, description, &values, Some(&data.values)).unwrap();
    assert!(live.live);
    assert!((live.reading("Input").unwrap().value + 6.02).abs() < 0.01);
    assert_eq!(live.markers.len(), 1);
}

// Spec: tube-distortion R11
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let input = noise(8192);
    let params = [
        (MODEL, MODEL_B),
        (TONE, 70.0),
        (DRIVE, 14.0),
        (BIAS, 35.0),
        (ENVELOPE, 60.0),
        (ATTACK, 5.0),
        (RELEASE, 50.0),
        (HI_QUALITY, 1.0),
    ];
    let render = |block: usize| -> Vec<f32> {
        let mut node = valve(&params, block);
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
