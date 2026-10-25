//! The tempo-synced delay (WP-2.18b, part 2; the delay spec): repeats land
//! on the synced frame at two tempos and across a tempo change, a hundred
//! per cent feedback with Freeze sustains, and the time-change modes glide,
//! crossfade, and cut.

use crate::common;
use bl_core::time::{Bpm, Frames, TempoMap};
use bl_devices::delay::{
    DRY_WET, FEEDBACK, FILTER, FREEZE, FREQ, L_OFFSET, L_SYNC, L_TIME, LINK, PING_PONG, R_SYNC,
    SYNC, TIME_MODE, delay_type,
};
use bl_engine::{Registry, TransportInfo};
use common::{device, node, run_at, set};

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
    let mut delay = node(&registry(), &device(&delay_type(), &params), 16_384);
    let out = run_at(
        delay.as_mut(),
        &left_impulse(14_000),
        &[],
        &tempo(120.0),
        TransportInfo::default(),
    );
    assert!(
        out[0][6_000].abs() > 0.99,
        "the first repeat is on the left"
    );
    assert!(out[1][6_000].abs() < 1e-6, "and not on the right");
    assert!(out[1][12_000].abs() > 0.49, "the second is on the right");
    assert!(out[0][12_000].abs() < 1e-6, "and not on the left");
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
