//! Echo (WP-8.37; the echo spec): the two lines and their times, the
//! channel modes, Input and Feedback, the filters, the modulation, the
//! characters, the reverb, the global controls, and the display, each run
//! as a real-time section under the allocation trap.

use crate::common;
use crate::common::response::{Kind, biquad_response, db as complex_db, mul, rbj, response_at};
use bl_core::time::{Bpm, Frames, TempoMap};
use bl_devices::echo::{self, echo_type, line_param};
use bl_engine::{Node, Registry, TransportInfo};
use bl_model::ParamId;
use common::{db, device, node, noise, run_at, run_blocks, set, sine, spectrum};

/// The block sizes every render must agree at (R19).
const BLOCKS: [usize; 4] = [1, 64, 512, 4_096];

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

fn tempo(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).expect("a tempo in range"))
}

/// Fully wet, no feedback, both lines a sixteenth note long: 6,000 frames
/// at 120 BPM.
fn clean() -> Vec<(ParamId, f64)> {
    vec![
        (echo::DRY_WET, 100.0),
        (echo::FEEDBACK, 0.0),
        (line_param(0, echo::NOTES), 2.0),
        (line_param(1, echo::NOTES), 2.0),
    ]
}

/// `clean` with `changes` on top.
fn with(changes: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = clean();
    params.extend_from_slice(changes);
    params
}

fn echo_node(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    node(&registry(), &device(&echo_type(), params), 4_096)
}

/// A silent stereo signal of `frames` frames with an impulse of `amplitude`
/// on the left channel at `at`, and on the right too when `both`.
fn impulse(frames: usize, at: usize, amplitude: f32, both: bool) -> Vec<Vec<f32>> {
    let mut left = vec![0.0f32; frames];
    left[at] = amplitude;
    let right = if both { left.clone() } else { vec![0.0; frames] };
    vec![left, right]
}

fn render(
    params: &[(ParamId, f64)],
    input: &[Vec<f32>],
    block: usize,
    map: &TempoMap,
) -> Vec<Vec<f32>> {
    run_blocks(echo_node(params).as_mut(), input, block, &[], map)
}

/// Where `signal`'s samples are, as a weighted mean, over `range`: for an
/// impulse read through a cubic, the frame it was read at.
fn centroid(signal: &[f32], range: std::ops::Range<usize>) -> f64 {
    let (mut weight, mut moment) = (0.0, 0.0);
    for n in range {
        weight += f64::from(signal[n]);
        moment += f64::from(signal[n]) * n as f64;
    }
    moment / weight
}

fn silent_but(signal: &[f32], lands: &[usize], amplitude: f32) -> bool {
    signal.iter().enumerate().all(|(n, s)| {
        if lands.contains(&n) {
            (s - amplitude).abs() < 1e-5
        } else {
            s.abs() < 1e-6
        }
    })
}

// Spec: echo R2
#[test]
fn a_synced_echo_lands_on_its_frame_at_every_block_size() {
    // 120 BPM: a sixteenth is 6,000 frames; at 60 BPM, 12,000.
    for (bpm, delay) in [(120.0, 6_000), (60.0, 12_000)] {
        let input = impulse(14_000 + delay, 100, 0.5, true);
        for block in BLOCKS {
            let out = render(&clean(), &input, block, &tempo(bpm));
            for channel in &out {
                assert!(
                    silent_but(channel, &[100 + delay], 0.5),
                    "{bpm} BPM, blocks of {block}: lands at {}",
                    100 + delay
                );
            }
        }
    }
}

// Spec: echo R2
#[test]
fn a_free_echo_lands_on_its_frame_at_every_block_size() {
    // 100 ms is 4,800 frames, at any tempo.
    let params = with(&[(line_param(0, echo::SYNC), 0.0), (line_param(0, echo::TIME), 100.0)]);
    let input = impulse(9_000, 100, 0.5, false);
    for block in BLOCKS {
        let out = render(&params, &input, block, &tempo(77.0));
        assert!(silent_but(&out[0], &[4_900], 0.5), "blocks of {block}");
    }
}

/// `input` through the device at `first` for `split` frames and at `second`
/// after, in blocks of `block`.
fn through_a_tempo_change(
    params: &[(ParamId, f64)],
    input: &[Vec<f32>],
    split: usize,
    (first, second): (f64, f64),
    block: usize,
) -> Vec<Vec<f32>> {
    let mut node = echo_node(params);
    let mut out = vec![Vec::new(); 2];
    let mut at = 0;
    while at < input[0].len() {
        let (map, end) = if at < split {
            (tempo(first), split)
        } else {
            (tempo(second), input[0].len())
        };
        let length = block.min(end - at);
        let part: Vec<Vec<f32>> = input.iter().map(|c| c[at..at + length].to_vec()).collect();
        let transport = TransportInfo {
            playing: true,
            position: Frames(at as i64),
            ..TransportInfo::default()
        };
        for (all, chunk) in out
            .iter_mut()
            .zip(run_at(node.as_mut(), &part, &[], &map, transport))
        {
            all.extend(chunk);
        }
        at += length;
    }
    out
}

// Spec: echo R2
#[test]
fn a_synced_echo_follows_a_tempo_change_at_every_block_size() {
    // The tempo halves at frame 3,000: the echo of the impulse at 0 moves
    // from 6,000 to 12,000, the new time's frame.
    let input = impulse(15_000, 0, 0.5, true);
    for block in BLOCKS {
        let out = through_a_tempo_change(&clean(), &input, 3_000, (120.0, 60.0), block);
        assert!(silent_but(&out[0], &[12_000], 0.5), "blocks of {block}");
    }
}

// Spec: echo R2
#[test]
fn the_sync_modes_count_their_divisions() {
    // At 120 BPM a quarter note is 24,000 frames and a bar 96,000.
    let at = |changes: &[(ParamId, f64)]| {
        let input = impulse(100_000, 0, 0.5, false);
        let out = render(&with(changes), &input, 4_096, &tempo(120.0));
        out[0].iter().position(|s| s.abs() > 0.4).expect("an echo")
    };
    let mode = |m| (line_param(0, echo::SYNC_MODE), m);
    assert_eq!(at(&[mode(0.0), (line_param(0, echo::NOTES), 4.0)]), 24_000);
    assert_eq!(at(&[mode(1.0), (line_param(0, echo::TRIPLET), 4.0)]), 16_000);
    assert_eq!(at(&[mode(2.0), (line_param(0, echo::DOTTED), 4.0)]), 36_000);
    assert_eq!(at(&[mode(3.0), (line_param(0, echo::SIXTEENTHS), 5.0)]), 30_000);
    assert_eq!(at(&[mode(0.0), (line_param(0, echo::NOTES), 6.0)]), 96_000);
}

// Spec: echo R3
#[test]
fn stereo_link_gives_the_right_line_the_left_time_and_offsets_stay_apart() {
    let right = line_param(1, echo::NOTES);
    let at = |params: &[(ParamId, f64)], channel: usize| {
        let out = render(params, &impulse(30_000, 0, 0.5, true), 4_096, &tempo(120.0));
        out[channel].iter().position(|s| s.abs() > 0.4).expect("an echo")
    };
    // Unlinked: the right line's own division, a sixteenth later each.
    let unlinked = with(&[(right, 3.0)]);
    assert_eq!(at(&unlinked, 0), 6_000);
    assert_eq!(at(&unlinked, 1), 12_000);
    // Linked: the left line's.
    let linked = with(&[(right, 3.0), (echo::LINK, 1.0)]);
    assert_eq!(at(&linked, 1), 6_000);
    // The offset stays each line's own: +30 % of 6,000 on the left only.
    let offset = with(&[(echo::LINK, 1.0), (line_param(0, echo::OFFSET), 30.0)]);
    assert_eq!(at(&offset, 0), 7_800);
    assert_eq!(at(&offset, 1), 6_000);
    // And it applies only while the line is synced.
    let free = with(&[
        (line_param(0, echo::SYNC), 0.0),
        (line_param(0, echo::TIME), 100.0),
        (line_param(0, echo::OFFSET), 30.0),
    ]);
    assert_eq!(at(&free, 0), 4_800);
}

// Spec: echo R4
#[test]
fn ping_pong_alternates_the_echoes_between_the_sides() {
    let params = with(&[(echo::CHANNEL_MODE, 1.0), (echo::FEEDBACK, 50.0)]);
    let out = render(&params, &impulse(32_000, 0, 1.0, false), 512, &tempo(120.0));
    // The input is summed into the left line; its output feeds the right
    // line, whose output returns to the left through Feedback.
    let peak = |channel: usize, at: usize| out[channel][at];
    assert!((peak(0, 6_000) - 1.0).abs() < 1e-5, "left, first");
    assert!((peak(1, 12_000) - 1.0).abs() < 1e-5, "right, second");
    assert!((peak(0, 18_000) - 0.5).abs() < 1e-5, "left, third, by Feedback");
    assert!((peak(1, 24_000) - 0.5).abs() < 1e-5, "right, fourth");
    for (channel, at) in [(1, 6_000), (0, 12_000), (1, 18_000), (0, 24_000)] {
        assert!(peak(channel, at).abs() < 1e-5, "not on {channel} at {at}");
    }
}

// Spec: echo R4
#[test]
fn mid_side_runs_the_mid_and_the_side_through_the_lines_and_round_trips() {
    // The mid line is the left one, the side line the right: an impulse on
    // the left is half mid and half side.
    let params = with(&[
        (echo::CHANNEL_MODE, 2.0),
        (line_param(1, echo::NOTES), 3.0),
    ]);
    let out = render(&params, &impulse(20_000, 0, 1.0, false), 512, &tempo(120.0));
    assert!((out[0][6_000] - 0.5).abs() < 1e-5 && (out[1][6_000] - 0.5).abs() < 1e-5);
    assert!((out[0][12_000] - 0.5).abs() < 1e-5 && (out[1][12_000] + 0.5).abs() < 1e-5);
    // With the lines' times equal the encode and decode cancel: the same
    // as Stereo, to rounding.
    let input = vec![noise(20_000), noise(20_000).into_iter().rev().collect()];
    let stereo = render(&clean(), &input, 512, &tempo(120.0));
    let sides = render(&with(&[(echo::CHANNEL_MODE, 2.0)]), &input, 512, &tempo(120.0));
    for (a, b) in stereo.iter().flatten().zip(sides.iter().flatten()) {
        assert!((a - b).abs() < 1e-6);
    }
}

// Spec: echo R5
#[test]
fn input_at_minus_infinity_lets_the_echoes_already_in_the_lines_play_out() {
    // Rule G1, IMP-08: an impulse at 100 enters; Input goes to −∞ at 3,000
    // and a second impulse at 4,000 doesn't enter. The first plays out,
    // with its feedback.
    let params = with(&[(echo::FEEDBACK, 50.0)]);
    let mut input = impulse(24_000, 100, 0.5, false);
    input[0][4_000] = 0.5;
    let out = run_blocks(
        echo_node(&params).as_mut(),
        &input,
        512,
        &[set(3_000, echo::INPUT, 0.0)],
        &tempo(120.0),
    );
    assert!((out[0][6_100] - 0.5).abs() < 1e-5, "the first echo");
    assert!((out[0][12_100] - 0.25).abs() < 1e-5, "and its feedback");
    assert!(out[0][10_000].abs() < 1e-6, "no echo of the second impulse");
    assert!(out[0][16_100].abs() < 1e-6, "nor its feedback");
}

// Spec: echo R5
#[test]
fn d_drives_the_signal_after_input_into_a_clip_on_both_paths() {
    let drive = [(echo::DRIVE, 1.0), (echo::DRY_WET, 0.0)];
    let input = vec![vec![0.5f32; 64], vec![-0.25f32; 64]];
    let out = run_at(
        echo_node(&drive).as_mut(),
        &input,
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    // The dry path hears the clip, tanh(x) with a unit slope at zero.
    assert!((f64::from(out[0][63]) - 0.5f64.tanh()).abs() < 1e-6);
    assert!((f64::from(out[1][63]) + 0.25f64.tanh()).abs() < 1e-6);
    // Input comes first: +12 dB on 0.5 clips harder.
    let loud = [(echo::DRIVE, 1.0), (echo::DRY_WET, 0.0), (echo::INPUT, 3.981_071_7)];
    let out = run_at(
        echo_node(&loud).as_mut(),
        &input,
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!((f64::from(out[0][63]) - (0.5 * 3.981_071_7f64).tanh()).abs() < 1e-4);
    // Off, the dry path is untouched.
    let off = [(echo::DRY_WET, 0.0)];
    let out = run_at(
        echo_node(&off).as_mut(),
        &input,
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert_eq!(out[0][63], 0.5);
}

// Spec: echo R6
#[test]
fn feedback_returns_the_echo_and_invert_flips_it() {
    let at = |changes: &[(ParamId, f64)]| {
        render(&with(changes), &impulse(20_000, 0, 0.5, false), 512, &tempo(120.0))
    };
    let plain = at(&[(echo::FEEDBACK, 50.0)]);
    assert!((plain[0][6_000] - 0.5).abs() < 1e-5);
    assert!((plain[0][12_000] - 0.25).abs() < 1e-5);
    assert!((plain[0][18_000] - 0.125).abs() < 1e-5);
    let inverted = at(&[(echo::FEEDBACK, 50.0), (echo::FEEDBACK_INVERT, 1.0)]);
    assert!((inverted[0][6_000] - 0.5).abs() < 1e-5, "the first echo isn't returned");
    assert!((inverted[0][12_000] + 0.25).abs() < 1e-5);
    assert!((inverted[0][18_000] - 0.125).abs() < 1e-5);
}

// Spec: echo R6
#[test]
fn the_loop_stays_bounded_at_a_hundred_per_cent_with_every_stage_on() {
    // Resonant filters, the reverb inside the loop, noise, wobble, the
    // characters and the modulation, all at their strongest: the echoes
    // neither blow up nor stop.
    let params = with(&[
        (echo::FEEDBACK, 100.0),
        (echo::FILTER, 1.0),
        (echo::HP_FREQ, 400.0),
        (echo::HP_RES, 100.0),
        (echo::LP_FREQ, 3_000.0),
        (echo::LP_RES, 100.0),
        (echo::REVERB, 100.0),
        (echo::REVERB_LOCATION, 2.0),
        (echo::DECAY, 10.0),
        (echo::NOISE, 1.0),
        (echo::NOISE_AMOUNT, 100.0),
        (echo::WOBBLE, 1.0),
        (echo::WOBBLE_AMOUNT, 100.0),
        (echo::MOD_DELAY, 100.0),
        (echo::MOD_X4, 1.0),
        (echo::MOD_FILTER, 100.0),
        (echo::GATE, 1.0),
        (echo::DUCKING, 1.0),
        (echo::INPUT, 3.981_071_7),
        (echo::DRIVE, 1.0),
        (line_param(0, echo::NOTES), 0.0),
        (line_param(1, echo::NOTES), 0.0),
    ]);
    let seconds = 20;
    let frames = seconds * 48_000;
    let mut input = vec![vec![0.0f32; frames]; 2];
    input[0][..2_000].copy_from_slice(&noise(2_000));
    input[1][..2_000].copy_from_slice(&noise(2_000));
    let out = render(&params, &input, 4_096, &tempo(120.0));
    let peak = |from: usize, to: usize| {
        out.iter()
            .flat_map(|c| c[from..to].iter())
            .fold(0.0f32, |p, s| p.max(s.abs()))
    };
    assert!(out.iter().flatten().all(|s| s.is_finite()), "finite");
    let late = peak(15 * 48_000, frames);
    assert!(late < 40.0, "bounded: {late}");
    assert!(late > 1e-4, "and still sounding: {late}");
}

/// The first echo of an impulse through the device with `changes`: 100 ms
/// is 4,800 frames, then `length` frames of it.
fn first_echo(changes: &[(ParamId, f64)], length: usize) -> Vec<f32> {
    let params = with(
        &[
            &[
                (line_param(0, echo::SYNC), 0.0),
                (line_param(0, echo::TIME), 100.0),
                (echo::FILTER, 1.0),
            ][..],
            changes,
        ]
        .concat(),
    );
    let input = impulse(4_800 + length, 0, 1.0, false);
    render(&params, &input, 4_096, &tempo(120.0))[0][4_800..].to_vec()
}

/// The 12 dB per octave filters' design for a Q.
fn design(highpass: (f64, f64), lowpass: (f64, f64)) -> impl Fn(f64) -> f64 {
    move |hz| {
        let hp = biquad_response(rbj(Kind::HighPass, highpass.0, 0.0, highpass.1), hz);
        let lp = biquad_response(rbj(Kind::LowPass, lowpass.0, 0.0, lowpass.1), hz);
        complex_db(mul(hp, lp))
    }
}

// Spec: echo R7
#[test]
fn the_filters_measured_responses_are_within_half_a_decibel_of_their_design() {
    // Resonance 0 % is a Q of 0.5, 100 % a Q of 10, evenly on a log scale.
    let q = |percent: f64| 0.5 * 20f64.powf(percent / 100.0);
    let cases = [
        // (HP Hz, HP %, LP Hz, LP %)
        (20.0, 0.0, 20_000.0, 0.0),
        (200.0, 0.0, 5_000.0, 100.0),
        (1_000.0, 50.0, 8_000.0, 30.0),
        (4_000.0, 100.0, 12_000.0, 0.0),
    ];
    for (hp, hp_res, lp, lp_res) in cases {
        let echo = first_echo(
            &[
                (echo::HP_FREQ, hp),
                (echo::HP_RES, hp_res),
                (echo::LP_FREQ, lp),
                (echo::LP_RES, lp_res),
            ],
            16_384,
        );
        let want = design((hp, q(hp_res)), (lp, q(lp_res)));
        let mut hz = 20.0;
        while hz < 20_000.0 {
            let got = complex_db(response_at(&echo, hz));
            assert!(
                (got - want(hz)).abs() < 0.5,
                "HP {hp} Hz {hp_res} %, LP {lp} Hz {lp_res} % at {hz:.0} Hz: {got:.2} against {:.2}",
                want(hz)
            );
            hz *= 1.12;
        }
    }
}

// Spec: echo R7
#[test]
fn with_filter_off_the_loop_passes_everything() {
    let echo = first_echo(&[(echo::FILTER, 0.0), (echo::HP_FREQ, 5_000.0)], 64);
    assert!((echo[0] - 1.0).abs() < 1e-6);
    assert!(echo[1..].iter().all(|s| s.abs() < 1e-6));
}

/// A free line of 100 ms, 4,800 frames, for the modulation tests.
fn hundred_ms() -> Vec<(ParamId, f64)> {
    with(&[
        (line_param(0, echo::SYNC), 0.0),
        (line_param(0, echo::TIME), 100.0),
        (echo::LFO_SYNC, 1.0),
        (echo::LFO_RATE, 4.0),
    ])
}

/// Where the echo of an impulse at 100 lands when the LFO moves the time by
/// `depth` of it: the frame `n` with `n = 100 + 4,800 (1 + depth sin(2π n /
/// 24,000))`, the LFO being a sine of a quarter note at 120 BPM.
fn lands(depth: f64) -> f64 {
    let mut n = 4_900.0;
    for _ in 0..50 {
        n = 100.0 + 4_800.0 * (1.0 + depth * (std::f64::consts::TAU * n / 24_000.0).sin());
    }
    n
}

// Spec: echo R9, R10
#[test]
fn a_synced_lfo_moves_the_time_on_the_beat_and_x4_multiplies_the_depth() {
    let input = impulse(9_000, 100, 0.5, false);
    for (extra, depth) in [(0.0, 0.02), (1.0, 0.08)] {
        let params = [
            hundred_ms(),
            vec![(echo::MOD_DELAY, 100.0), (echo::MOD_X4, extra)],
        ]
        .concat();
        let whole = render(&params, &input, 4_096, &tempo(120.0));
        let at = centroid(&whole[0], 4_000..6_000);
        assert!(
            (at - lands(depth)).abs() < 0.5,
            "depth {depth}: {at} against {}",
            lands(depth)
        );
        for block in [1, 64, 512] {
            assert!(render(&params, &input, block, &tempo(120.0)) == whole);
        }
    }
}

// Spec: echo R9, R10
#[test]
fn env_mix_runs_from_the_lfo_alone_to_the_envelope_alone() {
    let input = vec![noise(9_000), noise(9_000)];
    let with_wave = |wave: f64, mix: f64| {
        let params = [
            hundred_ms(),
            vec![
                (echo::MOD_DELAY, 100.0),
                (echo::WAVE, wave),
                (echo::ENV_MIX, mix),
            ],
        ]
        .concat();
        render(&params, &input, 512, &tempo(120.0))
    };
    // At 100 % the waveform makes no difference; at 0 % it does.
    assert!(with_wave(0.0, 100.0) == with_wave(3.0, 100.0), "envelope alone");
    assert!(with_wave(0.0, 0.0) != with_wave(3.0, 0.0), "LFO alone");
    // And at 0 % the envelope makes none: the input's level is no matter.
    let quiet = vec![input[0].iter().map(|s| s * 0.1).collect::<Vec<_>>(); 2];
    let echoes = |input: &[Vec<f32>]| {
        let params = [hundred_ms(), vec![(echo::MOD_DELAY, 100.0)]].concat();
        render(&params, input, 512, &tempo(120.0))
    };
    let (loud, soft) = (echoes(&input), echoes(&quiet));
    for (a, b) in loud[0].iter().zip(&soft[0]).skip(4_000) {
        assert!((a * 0.1 - b).abs() < 1e-5);
    }
}

// Spec: echo R9, R10
#[test]
fn mod_filter_moves_both_cutoffs_with_the_source() {
    // A 3 kHz tone through a 1 kHz low-pass whose cutoff the LFO moves two
    // octaves: at the sine's crest (a quarter of the cycle) it is 4 kHz, at
    // its trough 250 Hz.
    let params = with(&[
        (line_param(0, echo::SYNC), 0.0),
        (line_param(0, echo::TIME), 1.0),
        (echo::FILTER, 1.0),
        (echo::LP_FREQ, 1_000.0),
        (echo::LFO_SYNC, 1.0),
        (echo::LFO_RATE, 4.0),
        (echo::MOD_FILTER, 100.0),
    ]);
    let tone = sine(3_000.0, 0.5, 24_000);
    let out = render(&params, &[tone.clone(), tone], 512, &tempo(120.0));
    let level = |from: usize| {
        out[0][from..from + 480]
            .iter()
            .fold(0.0f32, |p, s| p.max(s.abs()))
    };
    assert!(level(6_000) > 10.0 * level(18_000), "{} and {}", level(6_000), level(18_000));
}

/// The peak of `signal` over `range`.
fn peak(signal: &[f32], range: std::ops::Range<usize>) -> f32 {
    signal[range].iter().fold(0.0, |p, s| p.max(s.abs()))
}

/// A 1 kHz tone of `amplitude` over `seconds`, then `tail` seconds of one
/// at `quiet`.
fn burst_then_quiet(amplitude: f32, seconds: f64, quiet: f32, tail: f64) -> Vec<Vec<f32>> {
    let mut tone = sine(1_000.0, amplitude, (seconds * 48_000.0) as usize);
    tone.extend(sine(1_000.0, quiet, (tail * 48_000.0) as usize));
    vec![tone.clone(), tone]
}

/// A short free line, so the wet signal is the input a moment later.
fn short() -> Vec<(ParamId, f64)> {
    with(&[
        (line_param(0, echo::SYNC), 0.0),
        (line_param(0, echo::TIME), 1.0),
        (line_param(1, echo::SYNC), 0.0),
        (line_param(1, echo::TIME), 1.0),
    ])
}

/// The frame after `from` where `signal`'s 1 ms peak first drops below
/// `below`.
fn falls_below(signal: &[f32], from: usize, below: f32) -> usize {
    (from..signal.len() - 48)
        .step_by(24)
        .find(|n| peak(signal, *n..*n + 48) < below)
        .expect("it falls")
}

// Spec: echo R11
#[test]
fn the_gate_follows_a_burst_with_its_threshold_and_release() {
    // A loud burst of 100 ms, then a tone at −40 dBFS, under the threshold.
    let input = burst_then_quiet(0.5, 0.1, 0.01, 0.6);
    let gated = |release: f64| {
        let params = [short(), vec![(echo::GATE, 1.0), (echo::GATE_RELEASE, release)]].concat();
        render(&params, &input, 512, &tempo(120.0))
    };
    let open = render(&short(), &input, 512, &tempo(120.0));
    let slow = gated(100.0);
    assert!(peak(&slow[0], 3_000..4_800) > 0.45, "open in the burst");
    assert!((peak(&open[0], 20_000..21_000) - 0.01).abs() < 1e-4, "the quiet tone passes ungated");
    // It closes over Release once the level is under the threshold: the
    // tone's amplitude falls to zero a Release later.
    let shut_slow = falls_below(&slow[0], 4_800, 0.0005);
    let shut_fast = falls_below(&gated(50.0)[0], 4_800, 0.0005);
    let difference = (shut_slow - shut_fast) as f64 / 48.0;
    assert!((difference - 50.0).abs() < 8.0, "Release doubled: {difference} ms apart");
    assert!(peak(&slow[0], 25_000..26_000) < 1e-4, "closed");
    // A loud burst opens it again within a few milliseconds.
    let mut again = input.clone();
    for channel in &mut again {
        channel.extend(sine(1_000.0, 0.5, 4_800));
    }
    let params = [short(), vec![(echo::GATE, 1.0)]].concat();
    let out = render(&params, &again, 512, &tempo(120.0));
    let start = input[0].len();
    assert!(peak(&out[0], start + 240..start + 288) > 0.45, "open again");
}

// Spec: echo R12
#[test]
fn ducking_lowers_the_wet_signal_in_proportion_and_comes_back_over_its_release() {
    let tone = |amplitude: f32| {
        let tone = sine(1_000.0, amplitude, 24_000);
        vec![tone.clone(), tone]
    };
    let ducked = |amplitude: f32| {
        let params = [short(), vec![(echo::DUCKING, 1.0)]].concat();
        let out = render(&params, &tone(amplitude), 512, &tempo(120.0));
        db(f64::from(peak(&out[0], 20_000..20_480) / amplitude)) as f32
    };
    // 6 dB more input ducks 6 dB more, under the 30 dB limit.
    let (soft, loud) = (ducked(0.1), ducked(0.2));
    assert!((loud - soft + 6.0).abs() < 1.0, "{soft} dB, {loud} dB");
    assert!(soft < -4.0 && loud > -30.5, "{soft} dB, {loud} dB");
    // After a burst a quiet tone comes back: by a time constant of Release
    // the lowering has fallen to 1/e of what it was.
    let input = burst_then_quiet(0.5, 0.2, 0.001, 1.5);
    let params = [short(), vec![(echo::DUCKING, 1.0), (echo::DUCK_RELEASE, 200.0)]].concat();
    let out = render(&params, &input, 512, &tempo(120.0));
    let lowered = |at: usize| -db(f64::from(peak(&out[0], at..at + 48) / 0.001));
    let start = lowered(9_700);
    let target = start / std::f64::consts::E;
    let back = (9_700..70_000)
        .step_by(48)
        .find(|n| lowered(*n) < target)
        .expect("it comes back");
    let milliseconds = (back - 9_700) as f64 / 48.0;
    assert!((150.0..250.0).contains(&milliseconds), "{milliseconds} ms from {start} dB");
}

// Spec: echo R12
#[test]
fn ducking_reads_the_input_and_never_the_echoes() {
    // A burst, then a long line and strong feedback: the echoes are as loud
    // as the burst, yet they aren't ducked, because the input is silent.
    let mut input = vec![sine(1_000.0, 0.5, 4_800); 2];
    for channel in &mut input {
        channel.resize(60_000, 0.0);
    }
    let params = with(&[
        (line_param(0, echo::SYNC), 0.0),
        (line_param(0, echo::TIME), 300.0),
        (echo::FEEDBACK, 90.0),
        (echo::DUCKING, 1.0),
        (echo::DUCK_RELEASE, 10.0),
    ]);
    let out = render(&params, &input, 512, &tempo(120.0));
    assert!(peak(&out[0], 14_400..19_200) > 0.49, "the first echo at full level");
    assert!(peak(&out[0], 28_800..33_600) > 0.44, "and the second at its feedback");
}

// Spec: echo R13
#[test]
fn noise_and_wobble_repeat_run_after_run_and_follow_the_device() {
    let noisy = [
        short(),
        vec![
            (echo::NOISE, 1.0),
            (echo::NOISE_AMOUNT, 100.0),
            (echo::WOBBLE, 1.0),
            (echo::WOBBLE_AMOUNT, 100.0),
        ],
    ]
    .concat();
    let silence = vec![vec![0.0f32; 48_000]; 2];
    let first = render(&noisy, &silence, 512, &tempo(120.0));
    assert!(first == render(&noisy, &silence, 4_096, &tempo(120.0)), "run after run");
    let mut other = device(&echo_type(), &noisy);
    other.id = bl_model::DeviceId(bl_core::id::Id::new(2).expect("an ID"));
    let second = run_blocks(
        node(&registry(), &other, 4_096).as_mut(),
        &silence,
        512,
        &[],
        &tempo(120.0),
    );
    assert!(first != second, "another device, another hiss");
    // At Amount 100 % the hiss peaks at −40 dBFS: white and flat, an RMS
    // of 0.01 / √3.
    let rms = (first[0].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / 48_000.0).sqrt();
    assert!((db(rms / (0.01 / 3f64.sqrt()))).abs() < 1.0, "{rms}");
}

// Spec: echo R13
#[test]
fn noise_morph_tilts_the_hiss_down_above_a_kilohertz() {
    let band = |spectrum: &[f64], from: usize, to: usize| {
        (spectrum[from..to].iter().map(|a| a * a).sum::<f64>() / (to - from) as f64).sqrt()
    };
    let silence = vec![vec![0.0f32; 48_000]; 2];
    let tilt = |morph: f64| {
        let params = [
            short(),
            vec![
                (echo::NOISE, 1.0),
                (echo::NOISE_AMOUNT, 100.0),
                (echo::NOISE_MORPH, morph),
            ],
        ]
        .concat();
        let out = render(&params, &silence, 4_096, &tempo(120.0));
        let spectrum = spectrum(&out[0]);
        db(band(&spectrum, 200, 400) / band(&spectrum, 6_000, 8_000))
    };
    assert!(tilt(0.0).abs() < 1.5, "white: {}", tilt(0.0));
    assert!(tilt(100.0) > 12.0, "falling: {}", tilt(100.0));
}

// Spec: echo R13
#[test]
fn wobble_moves_the_time_by_a_fraction_of_a_per_cent() {
    let input = impulse(9_000, 100, 0.5, false);
    let wobbling = [hundred_ms(), vec![(echo::WOBBLE, 1.0), (echo::WOBBLE_AMOUNT, 100.0)]].concat();
    let out = render(&wobbling, &input, 512, &tempo(120.0));
    let at = centroid(&out[0], 4_000..6_000);
    let moved = (at - 4_900.0).abs();
    assert!(moved > 1.0 && moved <= 4_800.0 * 0.005 + 0.5, "moved {moved} frames");
    let still = render(&hundred_ms(), &input, 512, &tempo(120.0));
    assert!((centroid(&still[0], 4_000..6_000) - 4_900.0).abs() < 1e-6);
}

/// The frequency and the level of the strongest line of `signal`, and the
/// level of the strongest line farther than 200 Hz from it, in dB below it.
fn strongest(signal: &[f32]) -> (f64, f64) {
    let lines = spectrum(signal);
    let width = 48_000.0 / signal.len() as f64;
    let (top, peak) = lines
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, a)| (i, *a))
        .expect("a spectrum");
    let spur = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| ((*i as f64 - top as f64) * width).abs() > 200.0)
        .map(|(_, a)| *a)
        .fold(0.0, f64::max);
    (top as f64 * width, db(spur / peak))
}

// Spec: echo R14
#[test]
fn repitch_glides_the_pitch_with_no_alias_and_off_it_crossfades_at_the_same_pitch() {
    // A 1 kHz tone through 100 ms, which becomes 50 ms at frame 24,000. With
    // Repitch the delay shortens by 2,400 frames over 200 ms: the read runs
    // 1.25 times as fast, so 1,250 Hz.
    let tone = sine(1_000.0, 0.5, 48_000);
    let input = vec![tone.clone(), tone];
    let free = |changes: &[(ParamId, f64)]| {
        with(
            &[
                &[(line_param(0, echo::SYNC), 0.0), (line_param(0, echo::TIME), 100.0)][..],
                changes,
            ]
            .concat(),
        )
    };
    let events = [set(24_000, line_param(0, echo::TIME), 50.0)];
    let run = |params: &[(ParamId, f64)]| {
        run_blocks(echo_node(params).as_mut(), &input, 512, &events, &tempo(120.0))
    };
    let glide = run(&free(&[(echo::REPITCH, 1.0)]));
    let (hz, spur) = strongest(&glide[0][26_000..26_000 + 4_096]);
    assert!((hz - 1_250.0).abs() < 15.0, "glides to {hz} Hz");
    assert!(spur < -80.0, "aliasing at {spur} dB");
    // Without Repitch the change crossfades over 50 ms: the pitch stays.
    let fade = run(&free(&[]));
    let (hz, _) = strongest(&fade[0][24_300..24_300 + 1_800]);
    assert!((hz - 1_000.0).abs() < 40.0, "stays at {hz} Hz");
    // Both arrive at the new time.
    assert!((glide[0][40_000] - fade[0][40_000]).abs() < 1e-4);
}

// Spec: echo R16
#[test]
fn the_reverb_before_and_after_the_lines_is_the_same_and_in_the_loop_it_diffuses_each_echo() {
    let click = impulse(60_000, 0, 0.5, true);
    let reverb = |location: f64| {
        let params = with(&[
            (echo::FEEDBACK, 50.0),
            (echo::REVERB, 100.0),
            (echo::REVERB_LOCATION, location),
            (echo::DECAY, 0.5),
        ]);
        render(&params, &click, 512, &tempo(120.0))
    };
    // Reverb and lines are both linear and fixed: before or after, the same.
    let (pre, post, feedback) = (reverb(0.0), reverb(1.0), reverb(2.0));
    for (a, b) in pre[0].iter().zip(&post[0]) {
        assert!((a - b).abs() < 1e-5, "{a} against {b}");
    }
    // In the loop the first echo is the clean click, and each later one is
    // more diffuse than the last.
    assert!(peak(&feedback[0], 6_000..6_001) > 0.45, "the first echo is clean");
    assert!(peak(&feedback[0], 12_000..18_000) < 0.2, "the second is diffuse");
    assert!(peak(&pre[0], 6_000..12_000) < 0.2, "before, the first already is");
}

// Spec: echo R17
#[test]
fn stereo_sets_the_width_of_the_wet_signal() {
    let at = |percent: f64| {
        let params = with(&[(echo::STEREO, percent)]);
        render(&params, &impulse(8_000, 0, 1.0, false), 512, &tempo(120.0))
    };
    // An impulse on the left alone is mid 0.5 and side 0.5.
    let (mono, same, wide) = (at(0.0), at(100.0), at(200.0));
    assert!((mono[0][6_000] - 0.5).abs() < 1e-6 && (mono[1][6_000] - 0.5).abs() < 1e-6);
    assert!((same[0][6_000] - 1.0).abs() < 1e-6 && same[1][6_000].abs() < 1e-6);
    assert!((wide[0][6_000] - 1.5).abs() < 1e-6 && (wide[1][6_000] + 0.5).abs() < 1e-6);
}

// Spec: echo R17
#[test]
fn output_scales_the_wet_signal_and_equal_loudness_keeps_the_sum_steady() {
    let input = vec![noise(48_000), noise(48_000).into_iter().rev().collect()];
    let rms = |signal: &[f32]| {
        (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
    };
    let level = |changes: &[(ParamId, f64)]| {
        let params = [
            vec![
                (line_param(0, echo::SYNC), 0.0),
                (line_param(0, echo::TIME), 100.0),
                (line_param(1, echo::SYNC), 0.0),
                (line_param(1, echo::TIME), 130.0),
            ],
            changes.to_vec(),
        ]
        .concat();
        let out = render(&with(&params), &input, 512, &tempo(120.0));
        let skip = 12_000;
        db(rms(&out[0][skip..]) / rms(&input[0][skip..]))
    };
    // Output is a gain on the wet signal alone: −6 dB halves it.
    let full = level(&[(echo::DRY_WET, 100.0)]);
    let half = level(&[(echo::DRY_WET, 100.0), (echo::OUTPUT, 0.5)]);
    assert!((full - half - 6.02).abs() < 0.1, "{full} and {half}");
    // With Equal-Loudness the two follow sine and cosine laws.
    for percent in [0.0, 10.0, 25.0, 50.0, 75.0, 90.0, 100.0] {
        let steady = level(&[(echo::DRY_WET, percent), (echo::EQUAL_LOUDNESS, 1.0)]);
        assert!(steady.abs() < 0.5, "{percent} %: {steady} dB");
    }
    // Without it the middle of Dry/Wet dips by 3 dB.
    assert!(level(&[(echo::DRY_WET, 50.0)]) < -2.5);
}

/// Every stage on at once.
fn everything() -> Vec<(ParamId, f64)> {
    with(&[
        (echo::CHANNEL_MODE, 1.0),
        (echo::FEEDBACK, 70.0),
        (echo::FILTER, 1.0),
        (echo::HP_FREQ, 300.0),
        (echo::HP_RES, 40.0),
        (echo::LP_FREQ, 6_000.0),
        (echo::LP_RES, 60.0),
        (echo::LFO_SYNC, 1.0),
        (echo::LFO_RATE, 3.0),
        (echo::WAVE, 5.0),
        (echo::PHASE, 90.0),
        (echo::MOD_DELAY, 60.0),
        (echo::MOD_FILTER, 50.0),
        (echo::ENV_MIX, 30.0),
        (echo::GATE, 1.0),
        (echo::GATE_THRESHOLD, -40.0),
        (echo::DUCKING, 1.0),
        (echo::NOISE, 1.0),
        (echo::NOISE_AMOUNT, 60.0),
        (echo::NOISE_MORPH, 50.0),
        (echo::WOBBLE, 1.0),
        (echo::WOBBLE_AMOUNT, 80.0),
        (echo::WOBBLE_MORPH, 40.0),
        (echo::REPITCH, 1.0),
        (echo::REVERB, 40.0),
        (echo::REVERB_LOCATION, 2.0),
        (echo::DECAY, 2.0),
        (echo::STEREO, 150.0),
        (echo::EQUAL_LOUDNESS, 1.0),
        (echo::DRY_WET, 60.0),
        (echo::DRIVE, 1.0),
        (line_param(0, echo::OFFSET), 20.0),
    ])
}

// Spec: echo R18, R19
#[test]
fn a_render_is_the_same_at_four_block_sizes_with_every_stage_on_and_the_tempo_moving() {
    let ramp = TempoMap::from_breakpoints([
        (bl_core::time::Ticks::ZERO, Bpm::new(100.0).unwrap()),
        (bl_core::time::Ticks::QUARTER * 8, Bpm::new(160.0).unwrap()),
    ])
    .unwrap();
    let frames = 60_000;
    let input = vec![noise(frames), noise(frames).into_iter().rev().collect()];
    let params = everything();
    let whole = render(&params, &input, 4_096, &ramp);
    assert!(whole[0].iter().any(|s| s.abs() > 0.01), "it sounds");
    for block in [1, 64, 512] {
        assert!(render(&params, &input, block, &ramp) == whole, "blocks of {block}");
    }
    assert!(render(&params, &input, 4_096, &ramp) == whole, "run after run");
}

// Spec: echo R22
#[test]
fn echo_reports_no_latency() {
    assert_eq!(echo_node(&clean()).latency(), 0);
}

fn shown(
    params: &[(ParamId, f64)],
    data: Option<&[f32]>,
    view: Option<&str>,
) -> bl_model::DeviceDisplay {
    let kind = echo_type();
    let device = device(&kind, params);
    echo::display(device.id, &kind.description, &device.params, data, view)
}

fn marker<'a>(display: &'a bl_model::DeviceDisplay, name: &str) -> &'a bl_model::DisplayMarker {
    display
        .markers
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no marker {name}"))
}

// Spec: echo R8
#[test]
fn the_tunnel_shows_each_lines_echoes_as_the_times_and_feedback_give_them() {
    assert_eq!(echo_type().description.views, ["Tunnel", "Filter"]);
    // 1/8 at 120 BPM is 250 ms; Feedback 40 % makes each echo 40 % of the last.
    let display = shown(&[], None, None);
    assert_eq!(display.view.as_deref(), Some("Tunnel"));
    assert!(!display.live);
    let (first, second) = (marker(&display, "Left echo 1"), marker(&display, "Left echo 2"));
    assert_eq!((first.x, first.y), (250.0, 0.0));
    assert_eq!(second.x, 500.0);
    assert!((second.y - 20.0 * 0.4f32.log10()).abs() < 1e-4);
    assert!(first.text.contains("250 ms"), "{}", first.text);
    // Sixteen at most, and none under the floor: 0.4^n falls below −60 dB
    // after eight.
    let left = display.markers.iter().filter(|m| m.name.starts_with("Left echo")).count();
    assert_eq!(left, 8);
    // No feedback, one echo.
    let none = shown(&[(echo::FEEDBACK, 0.0)], None, None);
    assert_eq!(none.markers.iter().filter(|m| m.name.starts_with("Right echo")).count(), 1);
    // The engine's published times replace the default tempo's.
    let live = shown(&[], Some(&[300.0, 450.0]), None);
    assert!(live.live);
    assert_eq!(marker(&live, "Left echo 1").x, 300.0);
    assert_eq!(marker(&live, "Right echo 1").x, 450.0);
    // Ping Pong sends the left line's echoes through the right line.
    let ping = shown(&[(echo::CHANNEL_MODE, 1.0)], None, None);
    assert_eq!(marker(&ping, "Left echo 2").x, 250.0 * 2.0 + 250.0);
    assert_eq!(marker(&ping, "Right echo 1").x, 500.0);
}

// Spec: echo R8
#[test]
fn a_tunnel_handle_drags_the_parameter_that_sets_its_lines_time() {
    let handle = |params: &[(ParamId, f64)], name: &str| {
        marker(&shown(params, None, None), name).handle.expect("a handle").x
    };
    assert_eq!(handle(&[], "Left Time"), line_param(0, echo::NOTES));
    assert_eq!(
        handle(&[(line_param(1, echo::SYNC_MODE), 2.0)], "Right Time"),
        line_param(1, echo::DOTTED)
    );
    assert_eq!(
        handle(&[(line_param(0, echo::SYNC), 0.0)], "Left Time"),
        line_param(0, echo::TIME)
    );
    assert_eq!(handle(&[(line_param(0, echo::SYNC_MODE), 3.0)], "Left Time"), line_param(0, echo::SIXTEENTHS));
    // With Stereo Link the right handle moves the left line's time too.
    assert_eq!(handle(&[(echo::LINK, 1.0)], "Right Time"), line_param(0, echo::NOTES));
}

// Spec: echo R8
#[test]
fn the_filter_view_draws_the_two_filters_response_with_a_handle_on_each_cutoff() {
    let on = [
        (echo::FILTER, 1.0),
        (echo::HP_FREQ, 400.0),
        (echo::HP_RES, 50.0),
        (echo::LP_FREQ, 4_000.0),
    ];
    let display = shown(&on, None, Some("Filter"));
    assert_eq!(display.view.as_deref(), Some("Filter"));
    let curve = display.series("Filter").expect("the response");
    assert!(curve.log_x && curve.x_range == [20.0, 20_000.0]);
    // The response agrees with the design where the filters sit.
    let at = |hz: f64| {
        let index = ((hz / 20.0).log10() / 3.0 * (curve.values.len() - 1) as f64).round() as usize;
        (f64::from(curve.values[index]), curve.x_at(index))
    };
    assert!(at(25.0).0 < -30.0, "the high-pass cuts the bottom");
    assert!(at(1_500.0).0.abs() < 4.0, "and passes the middle");
    assert!(at(18_000.0).0 < -15.0, "the low-pass cuts the top");
    let (hp, lp) = (marker(&display, "HP"), marker(&display, "LP"));
    assert_eq!(hp.x, 400.0);
    assert_eq!(hp.handle, Some(bl_model::DisplayHandle { x: echo::HP_FREQ, y: Some(echo::HP_RES) }));
    assert_eq!(lp.handle, Some(bl_model::DisplayHandle { x: echo::LP_FREQ, y: Some(echo::LP_RES) }));
    // Off, the curve is flat.
    let off = shown(&[(echo::HP_FREQ, 400.0)], None, Some("Filter"));
    assert!(off.series("Filter").unwrap().values.iter().all(|v| *v == 0.0));
}
