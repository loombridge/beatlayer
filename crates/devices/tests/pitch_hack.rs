//! Pitch Hack (WP-8.51; the pitch-hack spec): echoes at their time, synced
//! and free, transposed within 5 cents, reversed or given a random interval
//! by chance from the seed, fed back to transpose again, an Input that
//! reaches silence, and renders that repeat at any block size. Every block
//! runs as a real-time section under the allocation trap.

use crate::common;
use bl_core::time::{Bpm, Frames, TempoMap, TempoSegment, Ticks};
use bl_devices::pitch_hack::{
    CENTS, DIVISION, DRY_WET, FEEDBACK, INPUT, INTERVALS, RANDOM, REVERSE, SYNC, TIME, TRANSPOSE,
    draws, pitch_hack_type,
};
use bl_engine::{Registry, TransportInfo};
use bl_model::ParamId;
use common::{RATE, device, node};

const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::pitch_hack::register(&mut registry);
    registry
}

fn constant(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).expect("a tempo in range"))
}

/// 120 BPM for a quarter note, then 90.
fn changing() -> TempoMap {
    TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(1),
            bpm: Bpm::new(90.0).unwrap(),
        },
    ])
    .unwrap()
}

/// `input` on both channels through Pitch Hack with `params`, in blocks of
/// `block`, the song playing from frame 0 under `tempo`.
fn render(
    params: &[(ParamId, f64)],
    input: &[f32],
    block: usize,
    tempo: &TempoMap,
) -> Vec<Vec<f32>> {
    let mut hack = node(&registry(), &device(&pitch_hack_type(), params), block);
    common::run_blocks(
        hack.as_mut(),
        &[input.to_vec(), input.to_vec()],
        block,
        &[],
        tempo,
    )
}

/// Free time 250 ms (12,000 frames), no shift, no feedback, echoes only.
fn plain() -> Vec<(ParamId, f64)> {
    vec![
        (SYNC, 0.0),
        (TRANSPOSE, 0.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 100.0),
    ]
}

/// A clean sine's frequency from its positive-going zero crossings.
fn crossing_hz(signal: &[f32]) -> f64 {
    let mut crossings = Vec::new();
    for (i, pair) in signal.windows(2).enumerate() {
        if pair[0] < 0.0 && pair[1] >= 0.0 {
            crossings.push(i as f64 + f64::from(-pair[0]) / f64::from(pair[1] - pair[0]));
        }
    }
    assert!(crossings.len() > 3, "a sine in the stretch");
    let cycles = (crossings.len() - 1) as f64;
    cycles * RATE.as_f64() / (crossings[crossings.len() - 1] - crossings[0])
}

fn cents(hz: f64, reference: f64) -> f64 {
    1_200.0 * (hz / reference).log2()
}

/// A click on frame 100 and nothing else.
fn click(frames: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames];
    input[100] = 1.0;
    input
}

// Spec: pitch-hack R1, R7
#[test]
fn pitch_hack_is_described_with_every_parameter_in_its_layout_and_no_latency() {
    let device_type = pitch_hack_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Pitch Hack");
    assert_eq!(device_type.description.params.len(), 10);
    assert_eq!(
        device_type.description.layout.grows(),
        (false, false),
        "no display cell"
    );
    let hack = node(&registry(), &device(&device_type, &[]), 64);
    assert_eq!(hack.latency(), 0);
    let default = |id: ParamId| device_type.description.param(id).unwrap().default;
    assert_eq!(default(TRANSPOSE), 7.0);
    assert_eq!(default(FEEDBACK), 30.0);
    assert_eq!(default(DRY_WET), 50.0);
    assert_eq!(default(SYNC), 1.0);
    assert_eq!(default(TIME), 250.0);
}

// Spec: pitch-hack R2
#[test]
fn an_echo_lands_one_delay_time_after_its_input_free_and_synced() {
    // Free: 250 ms is 12,000 frames.
    let out = render(&plain(), &click(30_000), 4096, &constant(120.0));
    assert_eq!(out[0][100 + 12_000], 1.0);
    assert_eq!(out[0].iter().filter(|s| **s != 0.0).count(), 1, "one echo");
    // Synced, a 1/8 note: 12,000 frames at 120 BPM, 16,000 at 90.
    let synced = [(SYNC, 1.0), (DIVISION, 5.0)];
    for (bpm, frames) in [(120.0, 12_000), (90.0, 16_000)] {
        let mut params = plain();
        params.extend(synced);
        let out = render(&params, &click(30_000), 4096, &constant(bpm));
        assert_eq!(out[0][100 + frames], 1.0, "{bpm} BPM");
    }
    // A longer free time moves it: 1 s.
    let mut params = plain();
    params.push((TIME, 1_000.0));
    let out = render(&params, &click(60_000), 4096, &constant(120.0));
    assert_eq!(out[0][100 + 48_000], 1.0);
}

// Spec: pitch-hack R2, R9
#[test]
fn input_reaches_silence_and_gains_up_to_twelve_decibels() {
    let input = common::noise(30_000);
    let mut params = plain();
    params.push((INPUT, 0.0));
    let out = render(&params, &input, 4096, &constant(120.0));
    assert!(out.iter().flatten().all(|s| *s == 0.0), "Input at -inf dB");
    // The dry signal doesn't pass through Input.
    params.push((DRY_WET, 0.0));
    let out = render(&params, &input, 4096, &constant(120.0));
    assert_eq!(out[0], input, "dry is untouched");
    // +12 dB is a gain of 3.98.
    let unity = render(&plain(), &input, 4096, &constant(120.0));
    let mut loud = plain();
    loud.push((INPUT, 10f64.powf(12.0 / 20.0)));
    let loud = render(&loud, &input, 4096, &constant(120.0));
    let peak = |out: &[Vec<f32>]| out[0].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!((peak(&loud) / peak(&unity) - 3.98).abs() < 0.01);
}

// Spec: pitch-hack R3
#[test]
fn a_steady_tones_echo_is_at_the_transposition_within_five_cents() {
    for (hz, transpose, fine) in [
        (1_000.0, 7.0, 0.0),
        (1_000.0, 7.0, 50.0),
        (440.0, -12.0, 0.0),
        (440.0, 12.0, -50.0),
        (2_000.0, -5.0, 0.0),
    ] {
        let mut params = plain();
        params.extend([(TRANSPOSE, transpose), (CENTS, fine)]);
        let out = render(
            &params,
            &common::sine(hz, 0.5, 96_000),
            4096,
            &constant(120.0),
        );
        let heard = crossing_hz(&out[0][24_000..90_000]);
        let want = hz * ((transpose + fine / 100.0) / 12.0).exp2();
        assert!(
            cents(heard, want).abs() < 5.0,
            "{hz} Hz by {transpose} st and {fine} cents: {heard} Hz, {} cents off",
            cents(heard, want)
        );
    }
}

// Spec: pitch-hack R4
#[test]
fn reverse_plays_each_echo_backwards_and_at_nothing_none() {
    let input: Vec<f32> = (0..48_000).map(|n| (n as f32 + 1.0) / 48_000.0).collect();
    // With no transposition, echo n is the input of the period before it,
    // backwards, from the fade's end to the fade's start.
    let mut params = plain();
    params.push((REVERSE, 100.0));
    let out = render(&params, &input, 4096, &constant(120.0));
    for j in 300..11_700 {
        assert_eq!(out[0][12_000 + j], input[12_000 - 1 - j], "frame {j}");
    }
    // At 0 % every echo is the input, delayed.
    let out = render(&plain(), &input, 4096, &constant(120.0));
    for j in 0..12_000 {
        assert_eq!(out[0][12_000 + j], input[j], "frame {j}");
    }
}

/// The intervals the first `echoes` echoes take at Random 100 %, from
/// device 1's seed.
fn intervals(echoes: usize) -> Vec<f64> {
    (0..echoes)
        .map(|n| INTERVALS[(draws(1, n as i64)[2] * INTERVALS.len() as f64) as usize])
        .collect()
}

// Spec: pitch-hack R5
#[test]
fn random_gives_each_echo_an_interval_from_the_set_by_the_seed_run_after_run() {
    let mut params = plain();
    params.extend([(RANDOM, 100.0), (TRANSPOSE, 7.0)]);
    let input = common::sine(1_000.0, 0.5, 144_000);
    let out = render(&params, &input, 4096, &constant(120.0));
    let wanted = intervals(12);
    let mut seen = std::collections::BTreeSet::new();
    // Echo n plays from frame 12,000 n; the first plays nothing.
    for (n, interval) in wanted.iter().enumerate().skip(1) {
        let from = 12_000 * n + 3_000;
        let heard = crossing_hz(&out[0][from..from + 6_000]);
        let want = 1_000.0 * (interval / 12.0).exp2();
        assert!(
            cents(heard, want).abs() < 5.0,
            "echo {n}, {interval} st: {heard} Hz against {want} Hz"
        );
        seen.insert(*interval as i32);
    }
    assert!(seen.len() >= 3, "more than one interval: {seen:?}");
    let again = render(&params, &input, 512, &constant(120.0));
    assert_eq!(out, again, "the draws repeat run after run");
    // Played from echo 6's frame, the device draws what it drew there from
    // the song's start: the draws count from the song, not the device.
    let mut later = node(&registry(), &device(&pitch_hack_type(), &params), 4096);
    let from = 72_000;
    let mut tail = [Vec::new(), Vec::new()];
    for start in (from..input.len()).step_by(4096) {
        let end = (start + 4096).min(input.len());
        let part = vec![input[start..end].to_vec(); 2];
        let transport = TransportInfo {
            playing: true,
            position: Frames(start as i64),
            ..TransportInfo::default()
        };
        for (all, chunk) in tail.iter_mut().zip(common::run_at(
            later.as_mut(),
            &part,
            &[],
            &constant(120.0),
            transport,
        )) {
            all.extend(chunk);
        }
    }
    for (n, interval) in wanted.iter().enumerate().skip(7) {
        let at = 12_000 * n + 3_000;
        let heard = crossing_hz(&tail[0][at - from..at - from + 6_000]);
        let want = 1_000.0 * (interval / 12.0).exp2();
        assert!(
            cents(heard, want).abs() < 5.0,
            "echo {n} from a later start"
        );
    }
    // At 0 % none is drawn: every echo takes Transpose.
    params.push((RANDOM, 0.0));
    let out = render(&params, &input, 4096, &constant(120.0));
    let heard = crossing_hz(&out[0][50_000..100_000]);
    assert!(cents(heard, 1_000.0 * (7.0f64 / 12.0).exp2()).abs() < 5.0);
}

// Spec: pitch-hack R6
#[test]
fn feedback_transposes_each_pass_again_and_stays_below_one() {
    let mut params = plain();
    params.extend([(TRANSPOSE, 12.0), (FEEDBACK, 60.0), (DRY_WET, 100.0)]);
    let input = common::sine(500.0, 0.2, 144_000);
    let out = render(&params, &input, 4096, &constant(120.0));
    // Frames of whole cycles of every tone, from the fourth echo on.
    let heard = &out[0][48_000..96_000];
    let at = |hz: f64| {
        let (re, im) = common::dft_at(heard, hz);
        re.hypot(im)
    };
    let (one, two, four) = (at(1_000.0), at(2_000.0), at(4_000.0));
    assert!(one > 0.15, "the first echo is an octave up: {one}");
    // Each pass adds an octave at 60 % of the loop's 98 %.
    assert!(((two / one) - 0.588).abs() < 0.12, "{two} to {one}");
    assert!(((four / one) - 0.346).abs() < 0.12, "{four} to {one}");
    // With no feedback there is no second octave.
    params.push((FEEDBACK, 0.0));
    let out = render(&params, &input, 4096, &constant(120.0));
    let (re, im) = common::dft_at(&out[0][48_000..96_000], 2_000.0);
    assert!(re.hypot(im) < 0.01);
    // At 100 % the loop still decays: the click's echoes die away.
    let mut params = plain();
    params.extend([(FEEDBACK, 100.0), (TRANSPOSE, 0.0), (TIME, 100.0)]);
    let out = render(&params, &click(480_000), 4096, &constant(120.0));
    let level = |from: usize| {
        out[0][from..from + 4_800]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    };
    assert!(level(432_000) < level(4_000), "the loop's gain is below 1");
}

// Spec: pitch-hack R7
#[test]
fn dry_wet_balances_the_dry_signal_against_the_echoes() {
    let input = common::noise(30_000);
    let mut params = plain();
    params.push((DRY_WET, 0.0));
    let out = render(&params, &input, 4096, &constant(120.0));
    assert_eq!(out[0], input);
    params.push((DRY_WET, 50.0));
    let out = render(&params, &input, 4096, &constant(120.0));
    for i in [100, 5_000, 12_500, 20_000] {
        let echo = if i >= 12_000 { input[i - 12_000] } else { 0.0 };
        assert!(
            (out[0][i] - 0.5 * (input[i] + echo)).abs() < 1e-6,
            "frame {i}"
        );
    }
}

// Spec: pitch-hack R8
#[test]
fn renders_are_bit_identical_at_four_block_sizes_and_run_after_run() {
    let input = common::noise(96_000);
    let params = [
        (SYNC, 1.0),
        (DIVISION, 2.0),
        (TRANSPOSE, -5.0),
        (REVERSE, 50.0),
        (RANDOM, 50.0),
        (FEEDBACK, 50.0),
    ];
    for tempo in [constant(120.0), changing()] {
        let first = render(&params, &input, BLOCKS[3], &tempo);
        assert!(first[0].iter().any(|s| *s != 0.0));
        assert_eq!(
            first,
            render(&params, &input, BLOCKS[3], &tempo),
            "run after run"
        );
        for block in &BLOCKS[..3] {
            assert_eq!(
                first,
                render(&params, &input, *block, &tempo),
                "block {block}"
            );
        }
    }
}
