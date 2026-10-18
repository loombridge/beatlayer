//! Filter Delay, the filter delay (WP-5.25; the filter delay spec): three lines
//! with their inputs, On switches, bands, times, Pan and Volume, one
//! Feedback and one Dry. Every block runs as a real-time section under the
//! allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, Frames, TempoMap};
use bl_devices::filter_delay::{
    DIVISION, DRY, FEEDBACK, FILTER, FREQ, ON, PAN, SYNC, TIME, VOLUME, WIDTH, filter_delay_type,
    line_param,
};
use bl_engine::{Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::{device, node, run_at, run_blocks, set};

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

fn tempo(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).expect("a tempo in range"))
}

/// Only line `line` on: synced at `division` sixteenths, no band, no
/// Feedback, no Dry.
fn alone(line: usize, division: f64) -> Vec<(ParamId, f64)> {
    let mut params = vec![(FEEDBACK, 0.0), (DRY, 0.0)];
    for other in 0..3 {
        params.push((line_param(other, ON), f64::from(u8::from(other == line))));
        params.push((line_param(other, FILTER), 0.0));
    }
    params.push((line_param(line, SYNC), 1.0));
    params.push((line_param(line, DIVISION), division));
    params
}

fn with(mut params: Vec<(ParamId, f64)>, more: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    params.extend_from_slice(more);
    params
}

fn trident(params: &[(ParamId, f64)]) -> Box<dyn bl_engine::Node> {
    node(&registry(), &device(&filter_delay_type(), params), 65_536)
}

/// An impulse on frame 0 of the left channel, the right, or both.
fn impulse(frames: usize, left: f32, right: f32) -> Vec<Vec<f32>> {
    let mut l = vec![0.0f32; frames];
    let mut r = vec![0.0f32; frames];
    l[0] = left;
    r[0] = right;
    vec![l, r]
}

fn peak_at(out: &[Vec<f32>], channel: usize, frame: usize) -> f32 {
    out[channel][frame]
}

// Spec: filter-delay R2 and R4
#[test]
fn each_lines_echo_lands_on_its_synced_frame_at_two_tempos() {
    for (bpm, sixteenth) in [(120.0, 6_000usize), (60.0, 12_000)] {
        for line in 0..3 {
            let division = (line + 1) as f64;
            let mut node = trident(&alone(line, division));
            let out = run_at(
                node.as_mut(),
                &impulse(40_000, 1.0, 1.0),
                &[],
                &tempo(bpm),
                TransportInfo::default(),
            );
            let frame = sixteenth * (line + 1);
            // The first line leaves on the left, the third on the right, the
            // second on both at constant power.
            let (left, right) = match line {
                0 => (1.0, 0.0),
                1 => (
                    std::f32::consts::FRAC_1_SQRT_2,
                    std::f32::consts::FRAC_1_SQRT_2,
                ),
                _ => (0.0, 1.0),
            };
            assert!(
                (out[0][frame] - left).abs() < 1e-4 && (out[1][frame] - right).abs() < 1e-4,
                "line {line} at {bpm} BPM: {} {}",
                out[0][frame],
                out[1][frame]
            );
            for channel in &out {
                assert!(channel[frame - 1].abs() < 1e-6, "not before it");
                assert!(channel[frame + 1].abs() < 1e-6, "not after it");
            }
        }
    }
}

// Spec: filter-delay R2
#[test]
fn an_echo_follows_a_tempo_change() {
    let mut node = trident(&alone(0, 1.0));
    run_at(
        node.as_mut(),
        &impulse(3_000, 1.0, 1.0),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    let second = run_at(
        node.as_mut(),
        &[vec![0.0f32; 15_000], vec![0.0f32; 15_000]],
        &[],
        &tempo(60.0),
        TransportInfo {
            playing: true,
            position: Frames(3_000),
        },
    );
    // The tempo halves, so the echo moves from 6,000 to 12,000.
    assert!(
        second[0][9_000].abs() > 0.99,
        "at 12,000: {}",
        second[0][9_000]
    );
    assert!(second[0][3_000].abs() < 1e-6, "not at the old 6,000");
}

// Spec: filter-delay R3
#[test]
fn a_free_time_lands_on_its_frame_and_the_percentage_moves_a_synced_one() {
    // 100 ms and 37.5 ms at 48 kHz.
    for (milliseconds, frame) in [(100.0, 4_800usize), (37.5, 1_800)] {
        let params = with(
            alone(0, 4.0),
            &[
                (line_param(0, SYNC), 0.0),
                (line_param(0, TIME), milliseconds),
            ],
        );
        let mut node = trident(&params);
        let out = run_at(
            node.as_mut(),
            &impulse(8_000, 1.0, 1.0),
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        );
        assert!(
            (out[0][frame] - 1.0).abs() < 1e-4,
            "{milliseconds} ms: {}",
            out[0][frame]
        );
        assert!(out[0][frame + 1].abs() < 1e-6 && out[0][frame - 1].abs() < 1e-6);
    }
    // +50 % of a sixteenth (6,000) is 9,000; −50 % is 3,000.
    for (percent, frame) in [(50.0, 9_000usize), (-50.0, 3_000)] {
        let params = with(
            alone(0, 1.0),
            &[(line_param(0, bl_devices::filter_delay::PERCENT), percent)],
        );
        let mut node = trident(&params);
        let out = run_at(
            node.as_mut(),
            &impulse(10_000, 1.0, 1.0),
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        );
        assert!(
            (out[0][frame] - 1.0).abs() < 1e-4,
            "{percent} %: {}",
            out[0][frame]
        );
    }
}

// Spec: filter-delay R1
#[test]
fn the_first_line_hears_the_left_the_third_the_right_and_the_second_both() {
    let heard = |line: usize, left: f32, right: f32| -> f32 {
        let mut node = trident(&alone(line, 1.0));
        let out = run_at(
            node.as_mut(),
            &impulse(8_000, left, right),
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        );
        // What the line sends out, over both channels.
        (out[0][6_000].powi(2) + out[1][6_000].powi(2)).sqrt()
    };
    assert!((heard(0, 1.0, 0.0) - 1.0).abs() < 1e-4);
    assert!(heard(0, 0.0, 1.0) < 1e-6, "not the right");
    assert!(heard(2, 1.0, 0.0) < 1e-6, "not the left");
    assert!((heard(2, 0.0, 1.0) - 1.0).abs() < 1e-4);
    assert!(
        (heard(1, 1.0, 0.0) - 0.5).abs() < 1e-4,
        "the second hears half of either"
    );
    assert!((heard(1, 0.0, 1.0) - 0.5).abs() < 1e-4);
}

// Spec: filter-delay R5
#[test]
fn a_lines_pan_moves_it() {
    let at = |pan: f64| {
        let params = with(alone(0, 1.0), &[(line_param(0, PAN), pan)]);
        let mut node = trident(&params);
        let out = run_at(
            node.as_mut(),
            &impulse(8_000, 1.0, 1.0),
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        );
        (peak_at(&out, 0, 6_000), peak_at(&out, 1, 6_000))
    };
    let (left, right) = at(-100.0);
    assert!(
        (left - 1.0).abs() < 1e-4 && right.abs() < 1e-4,
        "left: {left} {right}"
    );
    let (left, right) = at(100.0);
    assert!(
        left.abs() < 1e-4 && (right - 1.0).abs() < 1e-4,
        "right: {left} {right}"
    );
    let (left, right) = at(0.0);
    assert!(
        (left - right).abs() < 1e-4 && left > 0.7,
        "centre: {left} {right}"
    );
}

/// The gain of line 0's loop at `hz`: the steady level of a sine of `hz` an
/// echo later, against the input's.
fn gain_at(hz: f64, centre: f64, width: f64) -> f64 {
    let params = with(
        alone(0, 1.0),
        &[
            (line_param(0, SYNC), 0.0),
            (line_param(0, TIME), 1.0),
            (line_param(0, FILTER), 1.0),
            (line_param(0, FREQ), centre),
            (line_param(0, WIDTH), width),
        ],
    );
    let mut node = trident(&params);
    let sine: Vec<f32> = (0..16_384)
        .map(|n| (0.5 * (std::f64::consts::TAU * hz * n as f64 / 48_000.0).sin()) as f32)
        .collect();
    let out = run_at(
        node.as_mut(),
        &[sine.clone(), sine],
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    let rms =
        |s: &[f32]| (s.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / s.len() as f64).sqrt();
    rms(&out[0][8_192..]) / (0.5 / 2.0f64.sqrt())
}

/// The frequency between `low` and `high` at which the gain crosses 3 dB
/// below `peak`, found by bisection in the logarithm.
fn crossing(low: f64, high: f64, peak: f64, centre: f64, width: f64) -> f64 {
    let target = peak / 2.0f64.sqrt();
    let rising = gain_at(low, centre, width) < target;
    let (mut low, mut high) = (low.ln(), high.ln());
    for _ in 0..18 {
        let middle = f64::midpoint(low, high);
        let below = gain_at(middle.exp(), centre, width) < target;
        if below == rising {
            low = middle;
        } else {
            high = middle;
        }
    }
    f64::midpoint(low, high).exp()
}

// Spec: filter-delay R6
#[test]
fn the_bands_centre_and_width_hold_within_two_per_cent() {
    for (centre, width) in [(1_000.0, 4.0), (1_000.0, 6.0), (2_000.0, 5.0)] {
        let peak = gain_at(centre, centre, width);
        assert!((peak - 1.0).abs() < 0.02, "flat in the band: {peak}");
        let lower = crossing(20.0, centre, peak, centre, width);
        let upper = crossing(centre, 20_000.0, peak, centre, width);
        let measured_centre = (lower * upper).sqrt();
        let measured_width = (upper / lower).log2();
        assert!(
            (measured_centre / centre - 1.0).abs() < 0.02,
            "centre {centre} at width {width}: {measured_centre}"
        );
        assert!(
            (measured_width / width - 1.0).abs() < 0.02,
            "width {width} at centre {centre}: {measured_width}"
        );
    }
}

// Spec: filter-delay R7
#[test]
fn each_repeat_falls_by_the_feedback_gain_through_the_band() {
    let params = with(
        alone(0, 1.0),
        &[
            (line_param(0, SYNC), 0.0),
            (line_param(0, TIME), 200.0),
            (line_param(0, FILTER), 1.0),
            (line_param(0, FREQ), 1_000.0),
            (line_param(0, WIDTH), 4.0),
            (FEEDBACK, 50.0),
        ],
    );
    let mut node = trident(&params);
    // A 100 ms burst of a 1 kHz sine, then silence.
    let burst: Vec<f32> = (0..48_000)
        .map(|n| {
            if n < 4_800 {
                (0.5 * (std::f64::consts::TAU * 1_000.0 * n as f64 / 48_000.0).sin()) as f32
            } else {
                0.0
            }
        })
        .collect();
    let out = run_at(
        node.as_mut(),
        &[burst.clone(), burst],
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    let rms = |from: usize| {
        let s = &out[0][from + 1_000..from + 3_800];
        (s.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    };
    let first = rms(9_600);
    let second = rms(19_200);
    let third = rms(28_800);
    for (name, ratio) in [("second", second / first), ("third", third / second)] {
        let db = 20.0 * ratio.log10();
        // 50 % is −6.02 dB, and the band costs a few hundredths of a dB.
        assert!((db - (-6.02)).abs() < 0.1, "the {name} repeat: {db} dB");
    }
}

// Spec: filter-delay R8
#[test]
fn volume_reaches_plus_six_and_minus_infinity_and_dry_reaches_minus_infinity() {
    let description = filter_delay_type().description;
    let top = description.param(line_param(0, VOLUME)).unwrap().max();
    assert!((top - db_to_gain(6.0)).abs() < 1e-9);
    let run = |extra: &[(ParamId, f64)]| {
        let mut node = trident(&with(alone(0, 1.0), extra));
        run_at(
            node.as_mut(),
            &impulse(8_000, 1.0, 1.0),
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        )
    };
    let loud = run(&[(line_param(0, VOLUME), top)]);
    assert!(
        (loud[0][6_000] - 1.995).abs() < 0.01,
        "+6 dB: {}",
        loud[0][6_000]
    );
    let silent = run(&[(line_param(0, VOLUME), 0.0)]);
    assert!(silent.iter().flatten().all(|s| *s == 0.0), "−∞ dB");
    // Dry passes the input at its level; at −∞ it is gone.
    let dry = |level: f64| {
        let mut node = trident(&with(alone(0, 1.0), &[(DRY, level)]));
        run_at(
            node.as_mut(),
            &impulse(100, 1.0, 1.0),
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        )
    };
    assert_eq!(dry(1.0)[0][0], 1.0);
    assert_eq!(dry(0.0)[0][0], 0.0);
    // A line switched off sends nothing.
    let off = run(&[(line_param(0, ON), 0.0)]);
    assert!(off.iter().flatten().all(|s| *s == 0.0));
}

// Spec: filter-delay R9
#[test]
fn renders_are_bit_identical_at_every_block_size_and_run_after_run() {
    let input = {
        let n = common::noise(30_000);
        vec![n.clone(), n.iter().rev().copied().collect()]
    };
    let params = vec![
        (FEEDBACK, 60.0),
        (DRY, 0.5),
        (line_param(1, SYNC), 0.0),
        (line_param(1, TIME), 33.0),
        (line_param(2, FREQ), 600.0),
    ];
    let events = [
        set(5_000, FEEDBACK, 30.0),
        set(12_345, line_param(0, PAN), 20.0),
    ];
    let render = |block: usize| {
        let mut node = trident(&params);
        run_blocks(node.as_mut(), &input, block, &events, &tempo(133.0))
    };
    let reference = render(4096);
    assert_eq!(render(4096), reference, "run after run");
    for block in [1, 64, 512] {
        assert_eq!(render(block), reference, "block size {block}");
    }
    assert!(reference[0].iter().any(|s| *s != 0.0));
}
