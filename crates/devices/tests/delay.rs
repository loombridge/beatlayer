//! The tempo-synced delay (WP-2.18b, part 2; the delay spec): repeats land
//! on the synced frame at two tempos and across a tempo change, a hundred
//! per cent feedback with Freeze sustains, and the time-change modes glide,
//! crossfade, and cut.

use crate::common;
use bl_core::time::{Bpm, Frames, TempoMap, TempoSegment, Ticks};
use bl_devices::delay::{
    DRY_WET, FEEDBACK, FILTER, FREEZE, FREQ, L_OFFSET, L_SYNC, L_TIME, LFO_DELAY, LFO_DOTTED,
    LFO_FILTER, LFO_MORPH, LFO_RATE, LFO_RATE_MODE, LFO_SIXTEENTH, LFO_SYNCED, LFO_TIME,
    LFO_TRIPLET, LFO_WAVE, LINK, PING_PONG, R_SYNC, SYNC, TIME_MODE, delay_type,
};
use bl_engine::{Registry, TransportInfo};
use common::{device, node, noise, run_at, run_blocks, set};

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

fn tempo(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).expect("a tempo in range"))
}

/// A silent stereo block with one impulse at its first frame, on both
/// channels.
fn impulse(frames: usize) -> Vec<Vec<f32>> {
    let mut left = vec![0.0f32; frames];
    left[0] = 1.0;
    vec![left.clone(), left]
}

/// The same, on the left channel only.
fn left_impulse(frames: usize) -> Vec<Vec<f32>> {
    let mut left = vec![0.0f32; frames];
    left[0] = 1.0;
    vec![left, vec![0.0f32; frames]]
}

/// A stereo block of ones: a step the delay's repeats carry.
fn step(frames: usize) -> Vec<Vec<f32>> {
    vec![vec![1.0f32; frames], vec![1.0f32; frames]]
}

/// A synced delay of one sixteenth, no feedback, fully wet.
fn clean() -> Vec<(bl_model::ParamId, f64)> {
    vec![
        (SYNC, 1.0),
        (LINK, 1.0),
        (L_SYNC, 1.0),
        (R_SYNC, 1.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 100.0),
    ]
}

// Spec: delay R2
#[test]
fn link_gives_both_channels_the_left_time_and_offsets_stay_apart() {
    // Unlinked: the right channel's own division, two sixteenths.
    let params = vec![
        (SYNC, 1.0),
        (LINK, 0.0),
        (L_SYNC, 1.0),
        (R_SYNC, 2.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 100.0),
    ];
    let mut delay = node(&registry(), &device(&delay_type(), &params), 16_384);
    let out = run_at(
        delay.as_mut(),
        &impulse(14_000),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!(out[0][6_000].abs() > 0.99, "the left, one sixteenth");
    assert!(out[1][6_000].abs() < 1e-6, "not the right");
    assert!(out[1][12_000].abs() > 0.99, "the right, two sixteenths");

    // Linked: the left channel's division for both.
    let mut params = params.clone();
    params.push((LINK, 1.0));
    let mut delay = node(&registry(), &device(&delay_type(), &params), 16_384);
    let out = run_at(
        delay.as_mut(),
        &impulse(14_000),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!(out[0][6_000].abs() > 0.99, "the left, one sixteenth");
    assert!(out[1][6_000].abs() > 0.99, "the right follows it");
    assert!(out[1][12_000].abs() < 1e-6, "not its own division");

    // The offset moves a repeat within its division, even linked.
    let mut params = params.clone();
    params.push((L_OFFSET, 50.0));
    let mut delay = node(&registry(), &device(&delay_type(), &params), 16_384);
    let out = run_at(
        delay.as_mut(),
        &impulse(14_000),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!(out[0][9_000].abs() > 0.99, "half a division later");
    assert!(out[0][6_000].abs() < 1e-6, "not on the division");
}

// Spec: delay R7
#[test]
fn dry_wet_balances_the_dry_signal_and_the_repeat() {
    let params = vec![
        (SYNC, 1.0),
        (LINK, 1.0),
        (L_SYNC, 1.0),
        (R_SYNC, 1.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 50.0),
    ];
    let mut delay = node(&registry(), &device(&delay_type(), &params), 16_384);
    let out = run_at(
        delay.as_mut(),
        &impulse(14_000),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!(
        (out[0][0] - 0.5).abs() < 1e-6,
        "half the dry: {}",
        out[0][0]
    );
    assert!(
        (out[0][6_000] - 0.5).abs() < 1e-6,
        "half the repeat: {}",
        out[0][6_000]
    );
}

// Spec: delay R1
#[test]
fn a_synced_repeat_lands_on_its_frame() {
    // 120 BPM: a sixteenth is 6,000 frames at 48 kHz.
    let mut delay = node(&registry(), &device(&delay_type(), &clean()), 16_384);
    let out = run_at(
        delay.as_mut(),
        &impulse(14_000),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!(out[0][6_000].abs() > 0.99, "the repeat: {}", out[0][6_000]);
    assert!(
        out[0][5_999].abs() < 1e-6,
        "not before it: {}",
        out[0][5_999]
    );
    assert!(
        out[0][6_001].abs() < 1e-6,
        "not after it: {}",
        out[0][6_001]
    );
    assert!(out[1][6_000].abs() > 0.99, "on the right channel too");

    // 60 BPM: the same division is 12,000 frames.
    let mut delay = node(&registry(), &device(&delay_type(), &clean()), 16_384);
    let out = run_at(
        delay.as_mut(),
        &impulse(14_000),
        &[],
        &tempo(60.0),
        TransportInfo::default(),
    );
    assert!(out[0][12_000].abs() > 0.99, "at 60 BPM: {}", out[0][12_000]);
    assert!(out[0][6_000].abs() < 1e-6, "not at the other tempo's frame");
}

// Spec: delay R1
#[test]
fn a_synced_repeat_follows_a_tempo_change() {
    let mut delay = node(&registry(), &device(&delay_type(), &clean()), 16_384);
    let first = run_at(
        delay.as_mut(),
        &impulse(3_000),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!(first[0].iter().all(|s| s.abs() < 1e-6), "nothing yet");
    let second = run_at(
        delay.as_mut(),
        &[vec![0.0f32; 15_000], vec![0.0f32; 15_000]],
        &[],
        &tempo(60.0),
        TransportInfo {
            playing: true,
            position: Frames(3_000),
            ..TransportInfo::default()
        },
    );
    // The tempo halves, so the repeat moves from 6,000 to 12,000.
    assert!(
        second[0][9_000].abs() > 0.99,
        "at 12,000: {}",
        second[0][9_000]
    );
    assert!(
        second[0][3_000].abs() < 1e-6,
        "not at the old 6,000: {}",
        second[0][3_000]
    );
}

// Spec: device-framework S9, export R14
#[test]
fn a_synced_delay_under_a_tempo_ramp_renders_the_same_at_any_block_size() {
    let ramp = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (Ticks::QUARTER * 4, Bpm::new(240.0).unwrap()),
    ])
    .unwrap();
    let frames = 40_000;
    let input = vec![noise(frames), noise(frames)];
    let mut params = clean();
    params.push((FEEDBACK, 50.0));
    let render = |block: usize| {
        let mut delay = node(&registry(), &device(&delay_type(), &params), frames);
        run_blocks(delay.as_mut(), &input, block, &[], &ramp)
    };
    let whole = render(frames);
    for block in [256, 1_000, 4_096] {
        assert!(render(block) == whole, "blocks of {block} frames");
    }
}

// Spec: delay R2
#[test]
fn a_hundred_per_cent_feedback_with_freeze_sustains() {
    let params = vec![
        (SYNC, 0.0),
        (LINK, 1.0),
        (L_TIME, 100.0),
        (FEEDBACK, 100.0),
        (DRY_WET, 100.0),
    ];
    let mut delay = node(&registry(), &device(&delay_type(), &params), 8_192);
    // 100 ms at 48 kHz: the impulse goes into the line, no repeat yet.
    let first = run_at(
        delay.as_mut(),
        &impulse(4_800),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!(
        first[0].iter().all(|s| s.abs() < 1e-6),
        "fully wet, no repeat"
    );
    let mut peaks = Vec::new();
    for block in 0..12 {
        let events = if block == 0 {
            vec![set(0, FREEZE, 1.0)]
        } else {
            vec![]
        };
        let out = run_at(
            delay.as_mut(),
            &[vec![0.0f32; 4_800], vec![0.0f32; 4_800]],
            &events,
            &tempo(120.0),
            TransportInfo::default(),
        );
        peaks.push(out[0].iter().fold(0.0f32, |peak, s| peak.max(s.abs())));
    }
    for peak in &peaks {
        assert!(
            (0.99..=1.01).contains(peak),
            "the frozen repeat sustains: {peak}"
        );
    }
}

// Spec: delay R3
#[test]
fn the_time_change_modes_glide_crossfade_and_cut() {
    let base = vec![
        (SYNC, 0.0),
        (LINK, 1.0),
        (L_TIME, 100.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 100.0),
    ];
    // The time changes from 100 ms (4,800 frames) to 50 ms (2,400) at the
    // start of the second block; the impulse's repeat follows.
    let repeat = |mode: f64| {
        let mut params = base.clone();
        params.push((TIME_MODE, mode));
        let mut delay = node(&registry(), &device(&delay_type(), &params), 8_192);
        run_at(
            delay.as_mut(),
            &step(2_000),
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        );
        let out = run_at(
            delay.as_mut(),
            &[vec![0.0f32; 6_000], vec![0.0f32; 6_000]],
            &[set(0, L_TIME, 50.0)],
            &tempo(120.0),
            TransportInfo::default(),
        );
        out[0].clone()
    };

    // Jump cuts: the repeat lands at the new time at once.
    let jump = repeat(2.0);
    assert!(jump[400].abs() > 0.99, "Jump cuts to 2,400: {}", jump[400]);
    assert!(jump[800].abs() > 0.99, "and stays: {}", jump[800]);

    // Fade crossfades: the repeat arrives at the new time, still fading in.
    let fade = repeat(1.0);
    assert!(
        (0.1..0.9).contains(&fade[400].abs()),
        "Fade arrives part-way: {}",
        fade[400]
    );
    assert!(fade[1_000].abs() > 0.99, "and lands: {}", fade[1_000]);

    // Repitch glides: the read pointer sweeps, so the repeat is later.
    let repitch = repeat(0.0);
    assert!(
        repitch[400].abs() < 1e-6,
        "Repitch does not cut: {}",
        repitch[400]
    );
    assert!(
        repitch[800].abs() > 0.99,
        "the glide carries the repeat: {}",
        repitch[800]
    );
}

// Spec: delay R4
#[test]
fn ping_pong_moves_the_repeats_between_the_channels() {
    let params = vec![
        (SYNC, 1.0),
        (LINK, 1.0),
        (L_SYNC, 1.0),
        (R_SYNC, 1.0),
        (FEEDBACK, 50.0),
        (DRY_WET, 100.0),
        (PING_PONG, 1.0),
    ];
    // A left click, then the same click on both channels: Live sums both
    // channels into the left line, so the centred click is twice as loud and
    // follows the same pattern. The repeats come every 6,000 frames: left,
    // right at the same level, then the pair at the Feedback (-6 dB).
    for (input, gain) in [(left_impulse(26_000), 1.0f32), (impulse(26_000), 2.0)] {
        let mut delay = node(&registry(), &device(&delay_type(), &params), 32_768);
        let out = run_at(
            delay.as_mut(),
            &input,
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        );
        let at = |channel: usize, frame: usize| out[channel][frame];
        let near = |value: f32, expected: f32| (value - expected).abs() < 1e-4;
        assert!(near(at(0, 6_000), gain), "left repeat: {}", at(0, 6_000));
        assert!(near(at(1, 6_000), 0.0), "not on the right");
        assert!(near(at(1, 12_000), gain), "right repeat: {}", at(1, 12_000));
        assert!(near(at(0, 12_000), 0.0), "not on the left");
        assert!(near(at(0, 18_000), gain * 0.5), "left: {}", at(0, 18_000));
        assert!(near(at(1, 24_000), gain * 0.5), "right: {}", at(1, 24_000));
    }
}

// Spec: delay R5
#[test]
fn the_filter_shapes_the_repeats() {
    let alternating = || {
        let samples: Vec<f32> = (0..10_000)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        vec![samples.clone(), samples]
    };
    let shaped = |filter_on: f64| {
        let params = vec![
            (SYNC, 0.0),
            (LINK, 1.0),
            (L_TIME, 100.0),
            (FEEDBACK, 0.0),
            (DRY_WET, 100.0),
            (FILTER, filter_on),
            (FREQ, 100.0),
        ];
        let mut delay = node(&registry(), &device(&delay_type(), &params), 16_384);
        let out = run_at(
            delay.as_mut(),
            &alternating(),
            &[],
            &tempo(120.0),
            TransportInfo::default(),
        );
        out[0][4_800..9_600]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()))
    };
    assert!(shaped(0.0) > 0.9, "unfiltered, the repeat is whole");
    assert!(shaped(1.0) < 0.1, "the band-pass takes it out");
}

// Spec: transport R24
#[test]
fn a_synced_delay_follows_the_played_tempo() {
    let params = vec![
        (SYNC, 1.0),
        (LINK, 1.0),
        (L_SYNC, 4.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 100.0),
    ];
    let run_with = |bpm: f64, speed: f64| {
        let mut delay = node(&registry(), &device(&delay_type(), &params), 16_384);
        run_at(
            delay.as_mut(),
            &impulse(14_000),
            &[],
            &tempo(bpm),
            TransportInfo {
                playing: true,
                position: Frames(0),
                speed,
                ..TransportInfo::default()
            },
        )
    };
    // A quarter note at 240 BPM is 12,000 frames.
    let followed = run_with(120.0, 2.0);
    assert!(
        followed[0][12_000].abs() > 0.99,
        "the repeat at the played tempo"
    );
    assert_eq!(
        followed,
        run_with(240.0, 1.0),
        "the same as that tempo in the map"
    );
}

// Spec: delay R1
#[test]
fn a_synced_repeat_longer_than_the_free_time_range_lands_on_its_frame() {
    // Sixteen sixteenths at 40 BPM is 6.0 s, 288,000 frames at 48 kHz.
    let params = vec![
        (SYNC, 1.0),
        (LINK, 1.0),
        (L_SYNC, 16.0),
        (R_SYNC, 16.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 100.0),
    ];
    let mut delay = node(&registry(), &device(&delay_type(), &params), 300_000);
    let out = run_at(
        delay.as_mut(),
        &impulse(300_000),
        &[],
        &tempo(40.0),
        TransportInfo::default(),
    );
    assert!(out[0][288_000].abs() > 0.99, "at 6.0 s");
    assert!(out[0][245_760].abs() < 1e-6, "not at the old 5.12 s cap");
}

// Spec: delay R7
#[test]
fn freeze_keeps_the_dry_signal_and_ignores_new_input() {
    let params = vec![
        (SYNC, 0.0),
        (LINK, 1.0),
        (L_TIME, 100.0),
        (FEEDBACK, 50.0),
        (DRY_WET, 50.0),
        (FREEZE, 1.0),
    ];
    let mut delay = node(&registry(), &device(&delay_type(), &params), 12_000);
    let out = run_at(
        delay.as_mut(),
        &step(12_000),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    // The lines are empty and stay empty, so only the dry half is heard.
    for channel in &out {
        assert!(
            channel.iter().all(|s| (s - 0.5).abs() < 1e-6),
            "half dry, nothing frozen in"
        );
    }
}

// The LFO's modulation read back: a ramp (x[n] = n / RAMP) through a delay
// with no feedback and no time change comes out as n - d[n], so the delay
// time d[n] in frames is n - RAMP * y[n], and the LFO's value is how far d
// is from the base time, in units of the full modulation depth.
const RAMP: f32 = 200_000.0;
/// The base time in Time mode, in frames at 48 kHz (100 ms), and the
/// modulation's depth at 100 % (20 ms).
const BASE: f64 = 4_800.0;
const DEPTH: f64 = 960.0;

/// The LFO's value at each frame from `from` on, as read back from the
/// delay, with the song playing from its start at `tempo`.
fn lfo_values(
    params: &[(bl_model::ParamId, f64)],
    frames: usize,
    block: usize,
    tempo: &TempoMap,
    from: usize,
) -> Vec<f64> {
    let mut all = vec![
        (SYNC, 0.0),
        (LINK, 1.0),
        (L_TIME, 100.0),
        (TIME_MODE, 2.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 100.0),
        (LFO_DELAY, 100.0),
    ];
    all.extend_from_slice(params);
    let mut delay = node(&registry(), &device(&delay_type(), &all), block);
    let ramp: Vec<f32> = (0..frames).map(|n| n as f32 / RAMP).collect();
    let out = run_blocks(delay.as_mut(), &[ramp.clone(), ramp], block, &[], tempo);
    (from..frames)
        .map(|n| (n as f64 - f64::from(out[0][n]) * f64::from(RAMP) - BASE) / DEPTH)
        .collect()
}

/// A tempo that changes from 120 to 90 BPM at the fourth beat.
fn changing() -> TempoMap {
    TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(4),
            bpm: Bpm::new(90.0).unwrap(),
        },
    ])
    .unwrap()
}

// Spec: delay R10
#[test]
fn the_beat_modes_land_their_cycles_on_the_beat_and_across_a_tempo_change() {
    // Ramp Up: a cycle's start reads -1 and its middle 0. Each mode's cycle in
    // quarter notes: Synced 1/4, Triplet 1/8 (a third of a beat), Dotted 1/8
    // (three quarters of one) and Sixteenth 4 (one beat).
    let modes = [
        (2.0, LFO_SYNCED, 4.0, 1.0),
        (3.0, LFO_TRIPLET, 3.0, 1.0 / 3.0),
        (4.0, LFO_DOTTED, 3.0, 0.75),
        (5.0, LFO_SIXTEENTH, 4.0, 1.0),
    ];
    let frames = 192_000;
    let from = 6_000;
    for tempo in [tempo(120.0), tempo(75.0), changing()] {
        for (mode, param, value, beats) in modes {
            let params = [(LFO_WAVE, 2.0), (LFO_RATE_MODE, mode), (param, value)];
            let values = lfo_values(&params, frames, 4_096, &tempo, from);
            for (i, read) in values.iter().enumerate().step_by(250) {
                let quarters = tempo
                    .frames_to_ticks(Frames((from + i) as i64), common::RATE)
                    .as_quarters();
                let expected = (quarters / beats).rem_euclid(1.0) * 2.0 - 1.0;
                // A cycle's end and start are the same place on the circle.
                let apart = (read - expected).abs();
                assert!(
                    apart < 0.002 || (apart - 2.0).abs() < 0.002,
                    "mode {mode} at frame {}: {read} for {expected}",
                    from + i
                );
            }
        }
    }
}

// Spec: delay R10
#[test]
fn the_lfo_is_the_same_at_any_block_size() {
    let params = [(LFO_WAVE, 2.0), (LFO_RATE_MODE, 2.0), (LFO_SYNCED, 3.0)];
    let tempo = changing();
    let reference = lfo_values(&params, 96_000, 4_096, &tempo, 6_000);
    for block in [1, 64] {
        assert_eq!(
            lfo_values(&params, 96_000, block, &tempo, 6_000),
            reference,
            "block size {block}"
        );
    }
}

// Spec: delay R10
#[test]
fn rate_and_time_free_run_and_each_mode_keeps_its_own_amount() {
    let tempo = tempo(120.0);
    // 2 Hz is half a second a cycle, and 1.5 Hz is 4/3 of a beat at 120 BPM:
    // a triplet of a half note.
    let rate = lfo_values(
        &[(LFO_RATE_MODE, 0.0), (LFO_RATE, 2.0)],
        96_000,
        512,
        &tempo,
        6_000,
    );
    let time = lfo_values(
        &[(LFO_RATE_MODE, 1.0), (LFO_TIME, 0.5)],
        96_000,
        512,
        &tempo,
        6_000,
    );
    assert_eq!(rate, time, "Time is the length of a cycle in seconds");
    let triplet = lfo_values(
        &[(LFO_RATE_MODE, 3.0), (LFO_TRIPLET, 5.0)],
        96_000,
        512,
        &tempo,
        6_000,
    );
    let hertz = lfo_values(
        &[(LFO_RATE_MODE, 0.0), (LFO_RATE, 1.5)],
        96_000,
        512,
        &tempo,
        6_000,
    );
    for (a, b) in triplet.iter().zip(&hertz) {
        assert!(
            (a - b).abs() < 0.002,
            "a triplet half note is 1.5 Hz at 120 BPM"
        );
    }
    // A dotted quarter is a Sixteenth of six.
    let dotted = lfo_values(
        &[(LFO_RATE_MODE, 4.0), (LFO_DOTTED, 4.0)],
        96_000,
        512,
        &tempo,
        6_000,
    );
    let sixteenth = lfo_values(
        &[(LFO_RATE_MODE, 5.0), (LFO_SIXTEENTH, 6.0)],
        96_000,
        512,
        &tempo,
        6_000,
    );
    assert_eq!(dotted, sixteenth);
    // The modes not chosen keep their values: they don't move the LFO.
    let others = [
        (LFO_RATE_MODE, 0.0),
        (LFO_RATE, 2.0),
        (LFO_TIME, 7.0),
        (LFO_SYNCED, 9.0),
        (LFO_TRIPLET, 0.0),
        (LFO_DOTTED, 0.0),
        (LFO_SIXTEENTH, 16.0),
    ];
    assert_eq!(lfo_values(&others, 96_000, 512, &tempo, 6_000), rate);
}

// Spec: delay R11
#[test]
fn morph_reaches_the_lfo_and_the_random_waves_ignore_it() {
    let tempo = tempo(120.0);
    // A 1 Hz Ramp Up with Morph at 100 % has finished its rise in the first
    // tenth of the cycle and holds its end value after it.
    let base = [(LFO_RATE_MODE, 0.0), (LFO_RATE, 1.0), (LFO_WAVE, 2.0)];
    let at = |morph: f64| {
        let mut params = base.to_vec();
        params.push((LFO_MORPH, morph));
        lfo_values(&params, 60_000, 512, &tempo, 6_000)
    };
    let (flat, half, full) = (at(0.0), at(50.0), at(100.0));
    // The values start at frame 6,000. Frame 24,000 is the middle of the
    // cycle: a plain ramp reads 0 there.
    assert!(flat[18_000].abs() < 0.01);
    // Frame 30,000 is 62 % through: a half-morphed ramp (a 55 % cycle) and a
    // fully morphed one (10 %) have both finished their rise.
    assert!(half[24_000] > 0.99 && full[24_000] > 0.99);
    // Frame 20,000 is 42 % through: the half-morphed ramp is still rising
    // and the fully morphed one has long finished.
    assert!(half[14_000] < 0.99 && full[14_000] > 0.99);
    // S&H and Wander ignore it.
    for wave in [5.0, 6.0] {
        let run = |morph: f64| {
            let params = [
                (LFO_RATE_MODE, 0.0),
                (LFO_RATE, 4.0),
                (LFO_WAVE, wave),
                (LFO_MORPH, morph),
            ];
            lfo_values(&params, 48_000, 512, &tempo, 6_000)
        };
        assert_eq!(run(0.0), run(100.0), "wave {wave}");
    }
}

// Spec: delay R12
#[test]
fn a_delay_saved_before_the_rate_modes_opens_in_rate_mode_and_plays_as_it_did() {
    // The 21 parameters a delay had: no mode, no other amounts.
    let old = [(LFO_WAVE, 1.0), (LFO_RATE, 3.0), (LFO_DELAY, 100.0)];
    let tempo = tempo(120.0);
    let render = |params: &[(bl_model::ParamId, f64)]| {
        let mut all = vec![
            (SYNC, 1.0),
            (L_SYNC, 2.0),
            (FEEDBACK, 40.0),
            (DRY_WET, 70.0),
        ];
        all.extend_from_slice(params);
        let mut delay = node(&registry(), &device(&delay_type(), &all), 4_096);
        let input = vec![noise(48_000), noise(48_000)];
        run_blocks(delay.as_mut(), &input, 4_096, &[], &tempo)
    };
    let new = {
        let mut params = old.to_vec();
        params.push((LFO_RATE_MODE, 0.0));
        params.push((LFO_TIME, 1.0));
        render(&params)
    };
    assert_eq!(
        render(&old),
        new,
        "the defaults of the new parameters change nothing"
    );
    // The old Rate parameter keeps its number and is the Rate mode's amount.
    assert_eq!(LFO_RATE.0, 16);
    let period = lfo_values(
        &[(LFO_RATE, 3.0), (LFO_WAVE, 0.0)],
        48_000,
        512,
        &tempo,
        6_000,
    );
    // A sine at 3 Hz rises through zero every 16,000 frames.
    let rising = period
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(i, _)| i + 6_000)
        .collect::<Vec<_>>();
    assert!(rising.iter().any(|&i| i.abs_diff(16_000) < 3), "{rising:?}");
}

// Spec: delay R10, R11
#[test]
fn a_render_is_the_same_at_block_sizes_1_64_and_4096() {
    let params = [
        (SYNC, 1.0),
        (L_SYNC, 3.0),
        (R_SYNC, 5.0),
        (FEEDBACK, 60.0),
        (DRY_WET, 60.0),
        (FILTER, 1.0),
        (LFO_DELAY, 70.0),
        (LFO_FILTER, 50.0),
        (LFO_WAVE, 1.0),
        (LFO_MORPH, 40.0),
        (LFO_RATE_MODE, 3.0),
        (LFO_TRIPLET, 4.0),
    ];
    let tempo = changing();
    let input = vec![noise(48_000), noise(48_000)];
    let render = |block: usize| {
        let mut delay = node(&registry(), &device(&delay_type(), &params), block);
        run_blocks(delay.as_mut(), &input, block, &[], &tempo)
    };
    let reference = render(4_096);
    assert!(reference[0].iter().any(|s| s.abs() > 0.01));
    for block in [1, 64] {
        assert_eq!(render(block), reference, "block size {block}");
    }
}
