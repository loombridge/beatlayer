//! PitchLoop89, the two-voice pitch-shifting looping delay (WP-8.52; the
//! PitchLoop89 spec): Bandwidth, Delay, Speed, the filters, Feedback and
//! Level of each voice, the routings, Hold, the vibrato and position LFOs,
//! Random, Input and Dry/Wet. Every block runs as a real-time section under
//! the allocation trap.

use crate::common;
use bl_core::id::Id;
use bl_core::time::{Bpm, TempoMap};
use bl_devices::pitchloop89::{
    BANDWIDTH_A, BANDWIDTH_B, BANDWIDTHS_MS, DELAY_A, DELAY_B, DRY_WET, FEEDBACK_A, FEEDBACK_B,
    HIGH_PASS_A, HOLD_A, INPUT, LEVEL_A, LEVEL_B, LOW_PASS_A, PITCHLOOP89, POSITION_DEPTH,
    POSITION_RATE, POSITION_WAVE, POSITION_WAVES, RANDOM, ROUTING, SPEED_A, SPEED_B, VIBRATO_DEPTH,
    VIBRATO_RATE, pitchloop89_type,
};
use bl_dsp::Random;
use bl_engine::{Node, Registry, TransportInfo};
use bl_model::{DeviceId, DeviceKind, ParamId};
use common::{node, noise, peaks, run_at, run_blocks, set};

const RATE: f64 = 48_000.0;
/// The block the one-block runs fit in.
const MAX_FRAMES: usize = 131_072;
/// Voice B's parameter for voice A's `param`.
fn b(param: ParamId) -> ParamId {
    ParamId(param.0 + 8)
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

fn tempo() -> TempoMap {
    TempoMap::constant(Bpm::new(120.0).unwrap())
}

/// PitchLoop89 as device number `id`, fully wet, with `params`.
fn pitchloop(id: u64, params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut device = pitchloop89_type().device(DeviceId(Id::new(id).unwrap()));
    device.params.insert(DRY_WET, 100.0);
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    node(&registry(), &device, MAX_FRAMES)
}

/// One block of `left` and `right` through `node`.
fn run(node: &mut dyn Node, left: &[f32], right: &[f32]) -> Vec<Vec<f32>> {
    run_at(
        node,
        &[left.to_vec(), right.to_vec()],
        &[],
        &tempo(),
        TransportInfo::default(),
    )
}

fn sine(hz: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            let ramp = 0.1 + 0.8 * n as f64 / frames as f64;
            (ramp * (std::f64::consts::TAU * hz * n as f64 / RATE).sin()) as f32
        })
        .collect()
}

fn impulse(frames: usize, at: usize) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    input[at] = 1.0;
    input
}

/// The frequency of a sine over `from..to`, from its rising zero crossings.
fn frequency(signal: &[f32], from: usize, to: usize) -> f64 {
    let mut crossings = Vec::new();
    for n in from..to - 1 {
        if signal[n] <= 0.0 && signal[n + 1] > 0.0 {
            crossings.push(n as f64 + f64::from(-signal[n]) / f64::from(signal[n + 1] - signal[n]));
        }
    }
    (crossings.len() - 1) as f64 * RATE / (crossings[crossings.len() - 1] - crossings[0])
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

// Spec: pitchloop89 R1 and R2
#[test]
fn it_is_an_audio_effect_named_for_live_with_no_display() {
    let t = pitchloop89_type();
    assert_eq!(t.name, "PitchLoop89");
    assert_eq!(t.role, bl_model::DeviceRole::AudioEffect);
    assert!(matches!(&t.kind, DeviceKind::Native { device_type } if device_type == PITCHLOOP89));
    let layout = &t.description.layout;
    assert!(
        layout.place(layout.docked_size()).cells.is_empty(),
        "no display cell"
    );
    let bandwidth = t.description.param(BANDWIDTH_A).unwrap();
    assert_eq!(bandwidth.default, 2.0, "100 ms");
    assert_eq!(bandwidth.format(0.0), "25 ms");
    assert_eq!(bandwidth.format(5.0), "800 ms");
    assert_eq!(t.description.param(LEVEL_A).unwrap().default, 1.0);
    assert_eq!(t.description.param(DRY_WET).unwrap().default, 50.0);
}

// Spec: pitchloop89 R2
#[test]
fn each_halving_of_bandwidth_loops_a_snippet_an_octave_higher() {
    // Hold freezes the last buffer's worth, and an impulse inside it comes
    // round once a loop: the loop is Delay's share of the buffer long.
    let frames = 60_000;
    let spacing = |bandwidth: usize| {
        let (ms, hold_at) = (BANDWIDTHS_MS[bandwidth], 700);
        let mut node = pitchloop(
            1,
            &[
                (BANDWIDTH_A, bandwidth as f64),
                (DELAY_A, 100.0),
                (LEVEL_B, 0.0),
            ],
        );
        let both = [impulse(frames, 100), vec![0.0; frames]];
        let events = [set(hold_at, HOLD_A, 1.0)];
        let out = run_blocks(node.as_mut(), &both, frames, &events, &tempo());
        let found = peaks(&out[0], 0.3);
        assert!(found.len() >= 2, "{ms} ms: the snippet loops: {found:?}");
        (found[1] - found[0], ms)
    };
    // The impulse comes out first as Delay's echo, then round and round.
    let long = spacing(2);
    for bandwidth in [0, 1] {
        let short = spacing(bandwidth);
        let octaves = (long.1 / short.1).log2();
        let measured = (long.0 as f64 / short.0 as f64).log2();
        assert!(
            (measured - octaves).abs() < 0.002,
            "{} ms against 100 ms: {measured} octaves, want {octaves}",
            short.1
        );
    }
}

// Spec: pitchloop89 R3
#[test]
fn each_voice_transposes_within_five_cents_and_reads_backwards_below_zero() {
    let frames = 70_000;
    // (Speed %, Delay %, the frames to measure, the frequency of a 1 kHz tone)
    let cases = [
        (50.0, 0.0, 10_000..60_000, 500.0),
        (150.0, 100.0, 30_000..60_000, 1_500.0),
        (200.0, 100.0, 21_000..37_000, 2_000.0),
        (-100.0, 0.0, 21_500..36_000, 1_000.0),
    ];
    let tone = sine(1_000.0, frames);
    let silent = vec![0.0; frames];
    for voice in [0, 1] {
        for (speed, delay, window, want) in &cases {
            let at = |param: ParamId| if voice == 0 { param } else { b(param) };
            let mut node = pitchloop(
                1,
                &[
                    (at(BANDWIDTH_A), 5.0),
                    (at(DELAY_A), *delay),
                    (at(SPEED_A), *speed),
                ],
            );
            let out = if voice == 0 {
                run(node.as_mut(), &tone, &silent)
            } else {
                run(node.as_mut(), &silent, &tone)
            };
            let heard = &out[voice];
            let measured = frequency(heard, window.start, window.end);
            let cents = 1_200.0 * (measured / want).log2();
            assert!(
                cents.abs() < 5.0,
                "voice {voice} at {speed} %: {measured} Hz, {cents} cents"
            );
            if *speed < 0.0 {
                // The tone's loudness rises with time: reversed, it falls.
                let (first, last) = (
                    rms(&heard[window.start..window.start + 2_000]),
                    rms(&heard[window.end - 2_000..window.end]),
                );
                assert!(first > last * 1.2, "reversed: {first} then {last}");
            }
        }
    }
}

/// The level of a tone of `hz` through a voice A set to `params`, over the
/// last 4,000 frames of a run, against the tone's own.
fn gain_at(hz: f64, params: &[(ParamId, f64)]) -> f64 {
    let frames = 20_000;
    let tone: Vec<f32> = (0..frames)
        .map(|n| (0.5 * (std::f64::consts::TAU * hz * n as f64 / RATE).sin()) as f32)
        .collect();
    let mut node = pitchloop(1, &[&[(DELAY_A, 10.0)], params].concat());
    let out = run(node.as_mut(), &tone, &vec![0.0; frames]);
    rms(&out[0][frames - 4_000..]) / rms(&tone[frames - 4_000..])
}

// Spec: pitchloop89 R4
#[test]
fn the_filters_cut_12_db_an_octave_and_level_scales_the_voice() {
    let low = [(LOW_PASS_A, 500.0)];
    assert!(
        gain_at(100.0, &low) > 0.95,
        "the low-pass passes what is below"
    );
    assert!(
        gain_at(8_000.0, &low) < 0.01,
        "four octaves above the low-pass: {}",
        gain_at(8_000.0, &low)
    );
    let high = [(HIGH_PASS_A, 5_000.0)];
    assert!(gain_at(15_000.0, &high) > 0.8);
    assert!(
        gain_at(200.0, &high) < 0.01,
        "five octaves below the high-pass: {}",
        gain_at(200.0, &high)
    );
    let open = gain_at(1_000.0, &[]);
    assert!((open - 1.0).abs() < 0.02, "open: {open}");
    let half = gain_at(1_000.0, &[(LEVEL_A, 0.5)]);
    assert!((half / open - 0.5).abs() < 0.01, "Level 0.5: {half}");
    assert_eq!(gain_at(1_000.0, &[(LEVEL_A, 0.0)]), 0.0, "Level at -inf dB");
}

// Spec: pitchloop89 R4
#[test]
fn feedback_returns_the_voice_to_its_buffer() {
    // A tone with a whole number of cycles in the 2,410 frames of the loop
    // (Delay 50 % of 100 ms plus the 10-frame margin) adds up each time round:
    // at 50 % it doubles.
    let hz = RATE * 25.0 / 2_410.0;
    let level = |feedback: f64| {
        let frames = 30_000;
        let tone: Vec<f32> = (0..frames)
            .map(|n| (0.5 * (std::f64::consts::TAU * hz * n as f64 / RATE).sin()) as f32)
            .collect();
        let mut node = pitchloop(1, &[(FEEDBACK_A, feedback)]);
        let out = run(node.as_mut(), &tone, &vec![0.0; frames]);
        rms(&out[0][frames - 4_000..])
    };
    let ratio = level(50.0) / level(0.0);
    assert!(
        (ratio - 2.0).abs() < 0.1,
        "50 % doubles a resonant tone: {ratio}"
    );
}

// Spec: pitchloop89 R5
#[test]
fn the_routings_send_the_voices_where_they_say() {
    let frames = 20_000;
    let tone = sine(1_000.0, frames);
    let silent = vec![0.0; frames];
    let routed = |routing: f64, feedback_a: f64| {
        let mut node = pitchloop(1, &[(ROUTING, routing), (FEEDBACK_A, feedback_a)]);
        run(node.as_mut(), &tone, &silent)
    };
    // L/R: the left input stays on the left.
    let apart = routed(0.0, 0.0);
    assert!(rms(&apart[0]) > 0.1);
    assert!(
        apart[1].iter().all(|s| *s == 0.0),
        "nothing reaches the right"
    );
    // Sum: both outputs carry the sum of the inputs.
    let sum = routed(1.0, 0.0);
    assert_eq!(sum[0], sum[1]);
    assert!(rms(&sum[0]) > 0.05);
    // Cross: voice A's feedback goes into voice B's buffer, so the right
    // hears it; without Cross it doesn't.
    let cross = routed(2.0, 60.0);
    assert!(rms(&cross[1]) > 0.05, "the right hears voice A's feedback");
    assert!(
        routed(0.0, 60.0)[1].iter().all(|s| *s == 0.0),
        "L/R keeps it on the left"
    );
}

// Spec: pitchloop89 R6
#[test]
fn hold_loops_what_the_buffer_holds_and_ignores_the_input_after() {
    let frames = 40_000;
    let render = |after: Vec<f32>| {
        let mut input = noise(frames);
        input[10_000..].copy_from_slice(&after);
        let mut node = pitchloop(1, &[(BANDWIDTH_A, 1.0), (DELAY_A, 100.0), (LEVEL_B, 0.0)]);
        let both = [input, vec![0.0; frames]];
        let events = [set(10_000, HOLD_A, 1.0)];
        run_blocks(node.as_mut(), &both, frames, &events, &tempo()).swap_remove(0)
    };
    let a = render(noise(30_000));
    let b = render(vec![0.0; 30_000]);
    assert_eq!(a, b, "what comes in once Hold is on is not heard");
    // The loop is 50 ms (2,400 frames) and the frame that restarts it long;
    // the filters' memory leaves a hair of difference.
    let period = 2_401;
    for n in 20_000..30_000 {
        assert!((a[n] - a[n + period]).abs() < 2e-3, "frame {n}");
    }
    assert!(rms(&a[20_000..30_000]) > 0.1);
}

// Spec: pitchloop89 R7
#[test]
fn the_vibrato_lfo_moves_the_pitch_at_its_rate_and_depth() {
    // A 1 kHz tone through a 4 Hz sine vibrato of 100 cents: the pitch
    // swings between 1 kHz × 2^(±1/12) four times a second.
    let frames = 100_000;
    let tone = sine(1_000.0, frames);
    let mut node = pitchloop(
        1,
        &[
            (VIBRATO_RATE, 4.0),
            (VIBRATO_DEPTH, 100.0),
            (DELAY_A, 100.0),
        ],
    );
    let out = run(node.as_mut(), &tone, &vec![0.0; frames]);
    // The frequency in each 1,200-frame stretch from 0.2 s on.
    let track: Vec<f64> = (0..70)
        .map(|k| frequency(&out[0], 12_000 + k * 1_200, 12_000 + (k + 1) * 1_200 + 100))
        .collect();
    let (high, low) = (
        track.iter().copied().fold(0.0, f64::max),
        track.iter().copied().fold(f64::MAX, f64::min),
    );
    for (got, want) in [(high, 2f64.powf(1.0 / 12.0)), (low, 2f64.powf(-1.0 / 12.0))] {
        let cents = 1_200.0 * (got / (1_000.0 * want)).log2();
        assert!(cents.abs() < 12.0, "{got} Hz against {} Hz", 1_000.0 * want);
    }
    // Turns upward 4 times a second: about 14 times in the 1.75 s measured.
    let upward = track
        .windows(2)
        .filter(|w| w[0] < 1_000.0 && w[1] >= 1_000.0)
        .count();
    assert!((6..=8).contains(&upward), "{upward} rises in 1.4 s at 4 Hz");
}

// Spec: pitchloop89 R7
#[test]
fn the_position_lfo_moves_delay_at_its_rate_and_depth() {
    // Rectangle at 1 Hz and 20 %: Delay is 70 % in the wave's first half
    // and 30 % in its second, so an impulse in each comes out that long
    // later (100 ms buffer, 10 frames of margin).
    let rectangle = POSITION_WAVES
        .iter()
        .position(|w| *w == "Rectangle")
        .unwrap();
    let frames = 30_000;
    let mut node = pitchloop(
        1,
        &[
            (POSITION_WAVE, rectangle as f64),
            (POSITION_RATE, 1.0),
            (POSITION_DEPTH, 20.0),
            (LEVEL_B, 0.0),
        ],
    );
    let mut input = impulse(frames, 100);
    input[24_100] = 1.0;
    let out = run(node.as_mut(), &input, &vec![0.0; frames]);
    let found = peaks(&out[0], 0.3);
    assert_eq!(found, [100 + 10 + 3_360, 24_100 + 10 + 1_440], "{found:?}");
}

// Spec: pitchloop89 R7
#[test]
fn random_draws_each_voices_delay_offset_from_the_devices_seed() {
    let frames = 30_000;
    let depth = 40.0;
    // The position LFO at its slowest sine is near 0 over the first second.
    let render = |id: u64| {
        let mut node = pitchloop(
            id,
            &[
                (POSITION_RATE, 0.01),
                (POSITION_DEPTH, depth),
                (LEVEL_B, 1.0),
            ],
        );
        let mut input = impulse(frames, 0);
        input[20_000] = 1.0;
        let both = [input.clone(), input];
        let events = [set(10_000, RANDOM, 1.0)];
        run_blocks(node.as_mut(), &both, frames, &events, &tempo())
    };
    let first = render(1);
    assert_eq!(render(1), first, "run after run");
    assert_ne!(render(2)[0], first[0], "another device draws other offsets");
    // Delay moves by the draw, in the depth, for each voice: the impulse
    // sent at 20,000 comes out 10 frames plus the buffer's share later.
    let mut random = Random::seeded(1);
    for (voice, signal) in first.iter().enumerate() {
        let offset = random.next_bipolar() * depth / 100.0;
        // The LFO moves a little while the impulse is in the buffer.
        let mut want = 20_000.0;
        for _ in 0..3 {
            let lfo = depth / 100.0 * (std::f64::consts::TAU * 0.01 * want / RATE).sin();
            want = 20_000.0 + 10.0 + (0.5 + offset + lfo).clamp(0.0, 1.0) * 4_800.0;
        }
        let found = peaks(&signal[20_000..], 0.3);
        let at = 20_000.0 + found[0] as f64;
        assert!(
            (at - want).abs() < 3.0,
            "voice {voice}: {at} against {want}"
        );
    }
}

// Spec: pitchloop89 R8, R11
#[test]
fn dry_wet_and_input_balance_the_signal_and_reach_silence() {
    let frames = 12_000;
    let input = noise(frames);
    let silent = vec![0.0; frames];
    let mix = |percent: f64, params: &[(ParamId, f64)]| {
        let mut node = pitchloop(1, &[&[(DRY_WET, percent)], params].concat());
        let out = run(node.as_mut(), &input, &silent);
        assert_eq!(node.latency(), 0, "no latency");
        out
    };
    assert_eq!(mix(0.0, &[])[0], input, "fully dry passes the input");
    let wet = mix(100.0, &[]);
    assert!(
        wet[0][..2_000].iter().all(|s| s.abs() < 1e-6),
        "no dry when wet"
    );
    assert!(rms(&wet[0][6_000..]) > 0.1);
    // Input at -inf dB: nothing is recorded, so the wet side is silent and
    // the dry still passes at 50 %.
    let muted = mix(50.0, &[(INPUT, 0.0)]);
    for n in 0..frames {
        assert_eq!(muted[0][n], 0.5 * input[n], "frame {n}");
    }
    // Input at +12 dB raises what is recorded fourfold.
    let loud = mix(100.0, &[(INPUT, 10f64.powf(12.0 / 20.0))]);
    let ratio = rms(&loud[0][6_000..]) / rms(&wet[0][6_000..]);
    assert!((ratio - 3.98).abs() < 0.05, "{ratio}");
}

// Spec: pitchloop89 R9, R10
#[test]
fn a_render_is_bit_identical_at_four_block_sizes_and_run_after_run() {
    let frames = 24_000;
    let input = [noise(frames), noise(frames).iter().rev().copied().collect()];
    let events = [
        set(3_000, RANDOM, 1.0),
        set(9_000, HOLD_A, 1.0),
        set(12_000, SPEED_A, 150.0),
        set(13_000, SPEED_B, -100.0),
        set(15_000, ROUTING, 2.0),
        set(16_000, FEEDBACK_A, 40.0),
        set(16_000, FEEDBACK_B, 40.0),
        set(18_000, BANDWIDTH_B, 0.0),
        set(20_000, HOLD_A, 0.0),
    ];
    let render = |block: usize| {
        let mut node = pitchloop(
            1,
            &[
                (VIBRATO_DEPTH, 60.0),
                (POSITION_DEPTH, 30.0),
                (POSITION_WAVE, 6.0),
                (POSITION_RATE, 3.0),
                (LOW_PASS_A, 4_000.0),
                (HIGH_PASS_A, 100.0),
                (DELAY_B, 20.0),
            ],
        );
        run_blocks(node.as_mut(), &input, block, &events, &tempo())
    };
    let first = render(4_096);
    assert_eq!(render(4_096), first, "run after run");
    assert!(first[0].iter().any(|s| *s != 0.0) && first[1].iter().any(|s| *s != 0.0));
    for block in [1, 64, 512] {
        assert_eq!(render(block), first, "block size {block}");
    }
}
