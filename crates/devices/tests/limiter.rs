//! Limiter, the native limiter (WP-2.18a, part 3; the limiter spec): the
//! sample peak, the true peak at 4 times the rate, the release time, Auto
//! Release, the stereo Link, the mid/side routing, and soft clipping.

use crate::common;
use bl_devices::limiter::{
    AUTO, CEILING, CEILING_MODE, GAIN, LIMITER, LINK, LOOKAHEAD, MAXIMIZE, OUTPUT, RELEASE,
    ROUTING, THRESHOLD, limiter_type,
};
use bl_dsp::Oversampler;
use bl_engine::{Block, DeviceType, Node, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::param::db_to_gain;

use common::{RATE, device, node};

/// A registry with the limiter alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::limiter::register(&mut registry);
    registry
}

/// A Limiter of `params`, up to `max_frames` at a time.
fn brickwall(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = limiter_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// Runs `left` and `right` through `node` as one block under the trap.
fn run_stereo(node: &mut dyn Node, left: &[f32], right: &[f32]) -> [Vec<f32>; 2] {
    let frames = left.len();
    let mut data: Vec<f32> = left.iter().chain(right.iter()).copied().collect();
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
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    let (l, r) = data.split_at(frames);
    [l.to_vec(), r.to_vec()]
}

/// Runs `input` (the same on both channels) through `node` in `block`-sized
/// blocks and returns the left channel's output.
fn stream(node: &mut dyn Node, input: &[f32], block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    for chunk in input.chunks(block) {
        out.extend_from_slice(&run_stereo(node, chunk, chunk)[0]);
    }
    out
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine(hz: f64, amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| amplitude * (std::f64::consts::TAU * hz * i as f64 / RATE.as_f64()).sin() as f32)
        .collect()
}

/// A quarter-rate sine at 45 degrees: its samples miss its peak of 1.0.
fn inter_sample_sine(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            (std::f64::consts::TAU * 12_000.0 * i as f64 / RATE.as_f64()
                + std::f64::consts::FRAC_PI_4)
                .sin() as f32
        })
        .collect()
}

/// The peak of `samples` sampled at 4 times the rate.
fn true_peak(samples: &[f32]) -> f32 {
    let mut oversampler = Oversampler::new(2, 1, samples.len().max(1));
    let mut audio = samples.to_vec();
    let mut peak = 0.0f32;
    oversampler.process(0, &mut audio, |high| {
        for sample in high {
            peak = peak.max(sample.abs());
        }
    });
    peak
}

// Spec: limiter R5
#[test]
fn the_output_is_delayed_by_the_look_ahead_and_reported_as_latency() {
    let lookahead = (0.003 * RATE.as_f64()).ceil() as usize;
    let mut node = brickwall(&[], 1024);
    assert_eq!(node.latency() as usize, lookahead);
    let mut input = vec![0.0f32; 1024];
    input[0] = 1.0;
    let out = run_stereo(node.as_mut(), &input, &input);
    assert!(out[0][lookahead].abs() > 0.0, "the peak comes out late");
    assert!(
        out[0][..lookahead].iter().all(|s| *s == 0.0),
        "nothing before it"
    );
    assert!(
        out[0][lookahead + 1..].iter().all(|s| *s == 0.0),
        "nothing after it"
    );
    // A set's own choice is reported before the device first runs, too:
    // the engine asks for the latency when it builds the node.
    let six = brickwall(&[(LOOKAHEAD, 2.0)], 1024);
    assert_eq!(
        six.latency() as usize,
        (0.006 * RATE.as_f64()).ceil() as usize
    );
}

// Spec: limiter R2
#[test]
fn the_sample_peak_never_passes_the_ceiling() {
    let ceiling = db_to_gain(-6.0) as f32;
    let mut node = brickwall(&[(CEILING, -6.0)], 512);
    let input = sine(1000.0, 2.0, 48_000);
    let out = stream(node.as_mut(), &input, 512);
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak <= ceiling + 1e-4, "{peak}");
    assert!(
        peak > ceiling * 0.9,
        "it limits, rather than only attenuating"
    );
}

// Spec: limiter R4
#[test]
fn true_peak_mode_holds_the_inter_sample_peaks_at_the_ceiling() {
    let ceiling = db_to_gain(-0.3) as f32;
    let input = inter_sample_sine(48_000);
    let mut node = brickwall(&[(CEILING, -0.3), (CEILING_MODE, 2.0)], 512);
    let out = stream(node.as_mut(), &input, 512);
    let peak = true_peak(&out[4800..]);
    assert!(peak <= ceiling + 1e-3, "true peak {peak} over {ceiling}");
    assert!(peak > ceiling * 0.9, "it limits the inter-sample peak");
}

// Spec: limiter R2
#[test]
fn standard_mode_leaves_the_inter_sample_peak_alone() {
    let ceiling = db_to_gain(-0.3) as f32;
    let input = inter_sample_sine(48_000);
    let mut node = brickwall(&[(CEILING, -0.3)], 512);
    let out = stream(node.as_mut(), &input, 512);
    let sample_peak = out[4800..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let root_half = std::f64::consts::FRAC_1_SQRT_2 as f32;
    assert!((sample_peak - root_half).abs() < 1e-3, "{sample_peak}");
    assert!(
        true_peak(&out[4800..]) > ceiling + 0.02,
        "its true peak passes the ceiling"
    );
}

// Spec: limiter R3
#[test]
fn the_release_recovers_with_its_time_constant() {
    const RELEASE_MS: f64 = 200.0;
    let ceiling = db_to_gain(-6.0) as f32;
    let step = 8000;
    let mut input = vec![1.0f32; step];
    input.extend(vec![0.25f32; 60_000]);
    let mut node = brickwall(&[(CEILING, -6.0), (RELEASE, RELEASE_MS), (LINK, 0.0)], 256);
    let out = stream(node.as_mut(), &input, 256);
    let lookahead = node.latency() as usize;
    // After the step, the input is 0.25, so the gain is the output over it.
    let gain: Vec<f64> = (step + 2 * lookahead..input.len())
        .map(|n| f64::from(out[n]) / 0.25)
        .collect();
    assert!(gain[0] < ceiling as f64 + 0.01, "still reduced at the step");
    // Fit ln(1 - gain) = a - n / tau over the straight part.
    let (mut sx, mut sy, mut sxx, mut sxy, mut count) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    for (n, g) in gain.iter().enumerate() {
        let gap = 1.0 - g;
        if gap > 0.05 && gap < 0.6 {
            let (x, y) = (n as f64, gap.ln());
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
            count += 1.0;
        }
    }
    assert!(count > 500.0, "enough of the curve to fit: {count}");
    let slope = (count * sxy - sx * sy) / (count * sxx - sx * sx);
    let tau = -1.0 / slope;
    let expected = RELEASE_MS * 0.001 * RATE.as_f64();
    assert!(
        (tau - expected).abs() / expected < 0.05,
        "release {tau} samples, expected {expected}"
    );
}

// Spec: limiter R3
#[test]
fn auto_release_recovers_more_quickly_from_a_peak() {
    let mut input = vec![0.05f32; 80_000];
    input[0] = 1.0;
    let mut auto = brickwall(&[(CEILING, -20.0), (AUTO, 1.0), (LINK, 0.0)], 256);
    let mut fixed = brickwall(&[(CEILING, -20.0), (RELEASE, 500.0), (LINK, 0.0)], 256);
    let auto_out = stream(auto.as_mut(), &input, 256);
    let fixed_out = stream(fixed.as_mut(), &input, 256);
    // The gain is the output over the steady 0.05 input.
    let recover = |out: &[f32]| -> usize {
        (256..out.len())
            .find(|n| f64::from(out[*n]) / 0.05 > 0.9)
            .unwrap_or(out.len())
    };
    let (auto_at, fixed_at) = (recover(&auto_out), recover(&fixed_out));
    assert!(
        auto_at * 4 < fixed_at,
        "auto recovered at {auto_at}, fixed at {fixed_at}"
    );
}

// Spec: limiter R6
#[test]
fn the_link_shares_the_gain_reduction_across_the_channels() {
    let left = vec![2.0f32; 2048];
    let right = vec![0.2f32; 2048];
    let target = (db_to_gain(-6.0) / 2.0) as f32;

    let mut shared = brickwall(&[(CEILING, -6.0), (LINK, 100.0)], 2048);
    let out = run_stereo(shared.as_mut(), &left, &right);
    let quiet = out[1][2047];
    assert!(
        (quiet - 0.2 * target).abs() < 1e-3,
        "shared {quiet}, expected {}",
        0.2 * target
    );

    let mut free = brickwall(&[(CEILING, -6.0), (LINK, 0.0)], 2048);
    let out = run_stereo(free.as_mut(), &left, &right);
    let quiet = out[1][2047];
    assert!((quiet - 0.2).abs() < 1e-3, "free {quiet}");
}

// Spec: limiter R7
#[test]
fn mid_side_routing_keeps_the_left_and_right_under_the_ceiling() {
    // Live holds a hard-left signal at the ceiling in M/S with the right
    // channel silent: the ceiling bounds left and right, and mid and side get
    // the same reduction.
    let ceiling = db_to_gain(-6.0) as f32;
    let left = vec![1.5f32; 2048];
    let right = vec![0.0f32; 2048];
    let mut node = brickwall(&[(CEILING, -6.0), (ROUTING, 1.0), (LINK, 0.0)], 2048);
    let out = run_stereo(node.as_mut(), &left, &right);
    let (l, r) = (out[0][2047], out[1][2047]);
    assert!((l - ceiling).abs() < 1e-3, "left {l}");
    assert!(r.abs() < 1e-3, "right {r}");

    let right = vec![-0.5f32; 2048];
    let mut node = brickwall(&[(CEILING, -6.0), (ROUTING, 1.0), (LINK, 0.0)], 2048);
    let out = run_stereo(node.as_mut(), &left, &right);
    let peak = out.iter().flatten().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak <= ceiling + 1e-3, "{peak}");
}

// Spec: limiter R8
#[test]
fn soft_clip_keeps_the_output_below_the_ceiling() {
    let ceiling = db_to_gain(-6.0) as f32;
    let mut node = brickwall(&[(CEILING, -6.0), (CEILING_MODE, 1.0)], 512);
    let input = sine(1000.0, 2.0, 48_000);
    let out = stream(node.as_mut(), &input, 512);
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak < ceiling, "{peak}");
    assert!(peak > ceiling * 0.8, "it is near the ceiling");
}

// Spec: limiter R1
#[test]
fn the_limiter_is_described_and_laid_out() {
    let device_type: DeviceType = limiter_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Limiter");
    assert!(matches!(
        device_type.kind,
        bl_model::DeviceKind::Native { ref device_type } if device_type == LIMITER
    ));
    assert_eq!(device_type.description.params.len(), 11);
    assert!(
        device_type
            .description
            .params
            .iter()
            .any(|p| p.id == CEILING && !p.is_gain())
    );
}

/// The peak of `samples` after the look-ahead's first 400 frames.
fn settled_peak(samples: &[f32]) -> f32 {
    samples[400..].iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

// Spec: limiter R10
#[test]
fn maximize_is_off_in_a_new_limiter_and_threshold_and_output_have_their_own_ranges() {
    let device_type: DeviceType = limiter_type();
    device_type.description.check().unwrap();
    let param = |id: ParamId| device_type.description.param(id).unwrap().clone();
    assert_eq!(param(MAXIMIZE).default, 0.0);
    assert_eq!(
        (param(THRESHOLD).min(), param(THRESHOLD).max()),
        (-24.0, 0.0)
    );
    assert_eq!(param(THRESHOLD).default, 0.0);
    assert!(param(OUTPUT).is_gain() && param(GAIN).is_gain());
    assert!((param(OUTPUT).default - db_to_gain(-0.3)).abs() < 1e-9);
}

// Spec: limiter R11
#[test]
fn maximize_lifts_by_output_minus_threshold_and_a_lower_threshold_is_louder_by_as_much() {
    // A quiet sine, which nothing limits: it comes out lifted by Output minus
    // Threshold, and 6 dB lower Threshold is 6 dB louder.
    let input = sine(1000.0, 0.05, 24_000);
    let render = |threshold: f64| {
        let params = [
            (MAXIMIZE, 1.0),
            (THRESHOLD, threshold),
            (OUTPUT, db_to_gain(-1.0)),
        ];
        settled_peak(&stream(brickwall(&params, 512).as_mut(), &input, 512))
    };
    let (at_minus_9, at_minus_15) = (render(-9.0), render(-15.0));
    let lifted = 0.05 * db_to_gain(-1.0 + 9.0) as f32;
    assert!(
        (at_minus_9 - lifted).abs() < 1e-3,
        "{at_minus_9} for {lifted}"
    );
    let ratio = f64::from(at_minus_15 / at_minus_9);
    assert!((ratio - db_to_gain(6.0)).abs() < 0.01, "{ratio}");
}

// Spec: limiter R11
#[test]
fn with_maximize_the_output_never_passes_output_in_any_ceiling_mode() {
    let input = sine(1000.0, 0.9, 48_000);
    for mode in [0.0, 1.0, 2.0] {
        for block in [1, 64, 4_096] {
            let params = [
                (MAXIMIZE, 1.0),
                (THRESHOLD, -12.0),
                (OUTPUT, db_to_gain(-3.0)),
                (CEILING_MODE, mode),
                // The ceiling and gain it would have without Maximize are
                // set to values that would show if they were read.
                (CEILING, -18.0),
                (GAIN, db_to_gain(-30.0)),
            ];
            let out = stream(brickwall(&params, block).as_mut(), &input, block);
            let limit = db_to_gain(-3.0) as f32;
            let peak = settled_peak(&out);
            let measured = if mode == 2.0 {
                true_peak(&out[400..])
            } else {
                peak
            };
            assert!(
                measured <= limit + 1e-3,
                "mode {mode}, block {block}: {measured}"
            );
            assert!(
                peak > limit * 0.8,
                "it limits at Output, mode {mode}: {peak}"
            );
        }
    }
}

// Spec: limiter R12
#[test]
fn a_limiter_saved_before_maximize_renders_identically_with_it_off() {
    let input = sine(1000.0, 1.5, 24_000);
    let old = [(CEILING, -6.0), (GAIN, db_to_gain(3.0)), (RELEASE, 50.0)];
    let mut new = old.to_vec();
    // Parameters of Maximize's own keep their values while it is off.
    new.extend([(THRESHOLD, -20.0), (OUTPUT, db_to_gain(-12.0))]);
    for block in [1, 64, 4_096] {
        let reference = stream(brickwall(&old, block).as_mut(), &input, block);
        assert_eq!(
            stream(brickwall(&new, block).as_mut(), &input, block),
            reference
        );
    }
}

// Spec: limiter R12
#[test]
fn maximize_turns_a_control_between_two_parameters_that_keep_their_values() {
    let description = limiter_type().description;
    let mut values = bl_model::OrdMap::new();
    values.insert(CEILING, -6.0);
    values.insert(THRESHOLD, -12.0);
    // Off: Ceiling and Gain are shown, Threshold and Output are not.
    assert_eq!(description.drawn(&values, CEILING), CEILING);
    assert!(description.shown(&values, CEILING) && description.shown(&values, GAIN));
    assert!(!description.shown(&values, THRESHOLD) && !description.shown(&values, OUTPUT));
    // On: the controls are Threshold's and Output's, and every value stays.
    values.insert(MAXIMIZE, 1.0);
    assert_eq!(description.drawn(&values, CEILING), THRESHOLD);
    assert_eq!(description.drawn(&values, GAIN), OUTPUT);
    assert!(!description.shown(&values, CEILING) && !description.shown(&values, GAIN));
    assert!(description.shown(&values, THRESHOLD) && description.shown(&values, OUTPUT));
    assert_eq!(description.value(&values, CEILING), Some(-6.0));
    assert_eq!(description.value(&values, THRESHOLD), Some(-12.0));
    // Each of the three new parameters is in a bank.
    let banked: Vec<ParamId> = description
        .banks()
        .iter()
        .flat_map(|b| b.params.clone())
        .collect();
    for id in [MAXIMIZE, THRESHOLD, OUTPUT] {
        assert!(banked.contains(&id));
    }
}

// Spec: limiter R11
#[test]
fn a_maximize_render_is_the_same_at_block_sizes_1_64_and_4096() {
    let input: Vec<f32> = sine(440.0, 0.8, 12_000)
        .iter()
        .zip(sine(3_100.0, 0.5, 12_000))
        .map(|(a, b)| a + b)
        .collect();
    let params = [
        (MAXIMIZE, 1.0),
        (THRESHOLD, -10.0),
        (OUTPUT, db_to_gain(-1.0)),
        (RELEASE, 80.0),
    ];
    let reference = stream(brickwall(&params, 4_096).as_mut(), &input, 4_096);
    assert!(settled_peak(&reference) > 0.5, "it plays");
    for block in [1, 64] {
        assert_eq!(
            stream(brickwall(&params, block).as_mut(), &input, block),
            reference,
            "block size {block}"
        );
    }
}
