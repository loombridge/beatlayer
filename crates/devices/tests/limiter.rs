//! Limiter, the native limiter (WP-2.18a, part 3; the limiter spec): the
//! sample peak, the true peak at 4 times the rate, the release time, Auto
//! Release, the stereo Link, the mid/side routing, and soft clipping.

use crate::common;
use bl_devices::limiter::{
    AUTO, CEILING, CEILING_MODE, LIMITER, LINK, LOOKAHEAD, RELEASE, ROUTING, limiter_type,
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
fn mid_side_routing_limits_the_mid_and_the_side() {
    // A mid of 0.5 and a side of 1.0 at a -6 dB ceiling: the side is held
    // at the ceiling, the mid is left alone, so the left sample passes the
    // ceiling, as the mid/side domain's ceiling allows.
    let ceiling = db_to_gain(-6.0) as f32;
    let left = vec![1.5f32; 2048];
    let right = vec![-0.5f32; 2048];
    let mut node = brickwall(&[(CEILING, -6.0), (ROUTING, 1.0), (LINK, 0.0)], 2048);
    let out = run_stereo(node.as_mut(), &left, &right);
    let (l, r) = (out[0][2047], out[1][2047]);
    let mid = (l + r) * 0.5;
    let side = (l - r) * 0.5;
    assert!((mid - 0.5).abs() < 1e-3, "mid {mid}");
    assert!((side - ceiling).abs() < 1e-3, "side {side}");
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
    assert_eq!(device_type.description.params.len(), 8);
    assert!(
        device_type
            .description
            .params
            .iter()
            .any(|p| p.id == CEILING && !p.is_gain())
    );
}
