//! Spectral Time (WP-8.47; the spectral-time spec): its freezer, spectral
//! delay, order switch, Resolution, global controls and spectrogram, each
//! block a real-time section under the allocation trap.

use std::f64::consts::TAU;

use bl_core::id::Id;
use bl_core::time::{Bpm, TempoMap};
use bl_devices::spectral_time::{
    self as st, DELAY_DRY_WET, DELAY_MODE, DELAY_ON, DOTTED, DRY_WET, FADE_IN, FADE_OUT,
    FADE_SHAPE, FEEDBACK, FREEZE, FREEZER_MODE, FREEZER_ON, INPUT_SEND, INTERVAL_DIVISION,
    INTERVAL_MODE, INTERVAL_TIME, MASK, NOTES, ORDER, RESOLUTION, SENSITIVITY, SHIFT, SIXTEENTHS,
    SPRAY, STEREO, TILT, TIME, TRIGGER, TRIPLETS, X_FADE, ZERO_DRY_LATENCY, spectral_time_type,
};
use bl_engine::NodeEvent;
use bl_model::{DeviceId, ParamId};

use crate::common::{self, RATE, dft_at, node, registry, run_blocks, set};

const SECOND: usize = 48_000;
/// Resolution 512: a frame of 512 frames, a hop of 128.
const R512: f64 = 1.0;
const FRAME: usize = 512;
const HOP: usize = 128;
const LATENCY: usize = FRAME - 1;

fn tempo() -> TempoMap {
    TempoMap::constant(Bpm::new(120.0).expect("a tempo in range"))
}

fn device_with(id: u64, params: &[(ParamId, f64)]) -> bl_model::Device {
    let mut device = spectral_time_type().device(DeviceId(Id::new(id).expect("an ID")));
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    device
}

/// Both sections off, Resolution 512, the rest as the description has it.
fn base(params: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut all = vec![
        (RESOLUTION, R512),
        (FREEZER_ON, 0.0),
        (DELAY_ON, 0.0),
        (FREEZE, 0.0),
    ];
    for &(param, value) in params {
        all.retain(|(p, _)| *p != param);
        all.push((param, value));
    }
    all
}

/// Renders `input` (the same on both channels) through Spectral Time with
/// `params` in blocks of `block` frames, the song playing at 120 BPM, and
/// returns both channels.
fn render_events(
    params: &[(ParamId, f64)],
    input: &[f32],
    block: usize,
    events: &[NodeEvent],
) -> Vec<Vec<f32>> {
    let mut node = node(&registry(), &device_with(1, params), block);
    run_blocks(
        node.as_mut(),
        &[input.to_vec(), input.to_vec()],
        block,
        events,
        &tempo(),
    )
}

fn render(params: &[(ParamId, f64)], input: &[f32], block: usize) -> Vec<Vec<f32>> {
    render_events(params, input, block, &[])
}

fn sine(hz: f64, amplitude: f32, from: usize, to: usize) -> Vec<f32> {
    (0..to)
        .map(|i| {
            if i < from {
                0.0
            } else {
                amplitude * (TAU * hz * i as f64 / RATE.as_f64()).sin() as f32
            }
        })
        .collect()
}

fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    (0..a.len().max(b.len()))
        .map(|i| a.get(i).unwrap_or(&0.0) + b.get(i).unwrap_or(&0.0))
        .collect()
}

fn impulse(at: usize, frames: usize) -> Vec<f32> {
    let mut signal = vec![0.0; frames];
    signal[at] = 1.0;
    signal
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.max(1e-12).log10()
}

/// The amplitude of `hz` in `signal[from..from + len]`.
fn at(signal: &[f32], hz: f64, from: usize, len: usize) -> f64 {
    let (re, im) = dft_at(&signal[from..from + len], hz);
    re.hypot(im)
}

/// The index of the largest value in `signal`.
fn peak(signal: &[f32]) -> usize {
    signal
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .map(|(i, _)| i)
        .expect("a signal")
}

/// The seconds' worth of frames.
fn seconds(s: f64) -> usize {
    (s * SECOND as f64).round() as usize
}

// Spec: spectral-time R1
#[test]
fn spectral_time_is_an_audio_effect_with_lives_name_and_a_spectrogram_cell() {
    let kind = spectral_time_type();
    assert_eq!(kind.name, "Spectral Time");
    kind.description.check().expect("a valid description");
    assert_eq!(kind.description.params.len(), 31);
    assert_eq!(kind.description.spectrogram, Some(st::GRID));
    assert_eq!(kind.description.menu_options, vec![ZERO_DRY_LATENCY]);
    assert!(
        bl_devices::device_types()
            .iter()
            .any(|t| t.name == "Spectral Time"),
        "the registry has it"
    );
    assert_eq!(st::SPECTRAL_TIME, "beatlayer.spectral_time");
}

// Spec: spectral-time R2
#[test]
fn the_reported_latency_is_the_analysis_frame_less_one_at_each_resolution() {
    let registry = registry();
    for (entry, size) in [256usize, 512, 1024, 2048, 4096].into_iter().enumerate() {
        let device = device_with(1, &[(RESOLUTION, entry as f64)]);
        let node = node(&registry, &device, 512);
        assert_eq!(node.latency() as usize, size - 1, "Resolution {entry}");
        assert_eq!(st::frame_size(entry, 48_000.0), size);
    }
    // The sizes scale with the sample rate to the nearest power of two.
    assert_eq!(st::frame_size(3, 96_000.0), 4096);
    assert_eq!(st::frame_size(3, 44_100.0), 2048);
    assert_eq!(st::frame_size(0, 22_050.0), 256);
}

// Spec: spectral-time R2
#[test]
fn input_send_at_minus_infinity_leaves_the_wet_path_silent() {
    let input = sine(1_000.0, 0.5, 0, SECOND);
    let wet = render(&base(&[(INPUT_SEND, 0.0)]), &input, 512);
    assert!(rms(&wet[0][2_000..]) < 1e-6, "silent");
    let open = render(&base(&[]), &input, 512);
    assert!(rms(&open[0][2_000..]) > 0.3, "and not at 0 dB");
}

// Spec: spectral-time R3
#[test]
fn a_change_of_resolution_crossfades_makes_nothing_and_reports_the_new_latency() {
    let input = sine(1_000.0, 0.5, 0, 2 * SECOND);
    let params = base(&[]);
    let mut node = node(&registry(), &device_with(1, &params), 480);
    assert_eq!(node.latency() as usize, LATENCY);
    let out = run_blocks(
        node.as_mut(),
        &[input.clone(), input],
        480,
        &[set(SECOND as u32, RESOLUTION, 3.0)],
        &tempo(),
    );
    assert_eq!(node.latency(), 2_047, "the new Resolution's latency");
    // No step in the signal while it moves from one size to the other.
    let steepest = out[0][3_000..]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(steepest < 0.15, "a smooth sine, not {steepest}");
    // Before and after, the tone is whole.
    let before = rms(&out[0][seconds(0.8)..seconds(0.95)]);
    let after = rms(&out[0][seconds(1.6)..]);
    assert!((db(before / after)).abs() < 0.5, "{before} {after}");
    assert!((before - 0.5 / 2f64.sqrt()).abs() < 0.02);
}

// Spec: spectral-time R4
#[test]
fn zero_dry_signal_latency_passes_the_dry_signal_undelayed_and_reports_none() {
    let input = common::noise(4_096);
    let params = base(&[(DRY_WET, 0.0)]);
    let delayed = render(&params, &input, 512);
    for n in 0..4_096 - LATENCY {
        assert_eq!(
            delayed[0][n + LATENCY],
            input[n],
            "the dry signal, lined up"
        );
    }
    let mut zero = params.clone();
    zero.push((ZERO_DRY_LATENCY, 1.0));
    let undelayed = render(&zero, &input, 512);
    assert_eq!(undelayed[0], input, "the dry signal, as it came");
    let registry = registry();
    assert_eq!(
        node(&registry, &device_with(1, &params), 64).latency() as usize,
        LATENCY
    );
    assert_eq!(node(&registry, &device_with(1, &zero), 64).latency(), 0);
    // The wet signal arrives late by the frame less one.
    let mut wet = zero.clone();
    wet.retain(|(p, _)| *p != DRY_WET);
    let out = render(&wet, &impulse(100, 4_096), 512);
    assert_eq!(peak(&out[0]), 100 + LATENCY);
}

// Spec: spectral-time R5
#[test]
fn a_frozen_spectrum_holds_its_level_within_half_a_decibel_for_thirty_seconds() {
    let chord = add(
        &add(
            &sine(440.0, 0.2, 0, 2 * SECOND),
            &sine(660.7, 0.2, 0, 2 * SECOND),
        ),
        &sine(1_234.5, 0.2, 0, 2 * SECOND),
    );
    let mut input = chord.clone();
    input.resize(32 * SECOND, 0.0);
    let out = render_events(
        // At 2,048 frames the chord's partials are far enough apart for each
        // bin to hold one.
        &base(&[(RESOLUTION, 3.0), (FREEZER_ON, 1.0), (FADE_IN, 0.0)]),
        &input,
        4_096,
        &[set(SECOND as u32, FREEZE, 1.0)],
    );
    let early = rms(&out[0][3 * SECOND..4 * SECOND]);
    let late = rms(&out[0][31 * SECOND..32 * SECOND]);
    assert!(db(late / early).abs() < 0.5, "{} dB", db(late / early));
    for hz in [440.0, 660.7, 1_234.5] {
        let first = at(&out[0], hz, 3 * SECOND, SECOND);
        let last = at(&out[0], hz, 31 * SECOND, SECOND);
        assert!(
            db(first / 0.2).abs() < 0.5,
            "{hz} Hz is held at what was captured"
        );
        assert!(
            db(last / first).abs() < 0.5,
            "{hz} Hz holds for thirty seconds"
        );
    }
}

// Spec: spectral-time R5
#[test]
fn freeze_fades_in_over_fade_in_and_out_over_fade_out() {
    // A until the freeze has been captured, then B: while the freeze fades
    // in, B fades out of the live signal as the frozen A comes in.
    let (a, b) = (1_000.0, 1_500.0);
    let input = add(
        &sine(a, 0.4, 0, seconds(0.6)),
        &sine(b, 0.4, seconds(0.6), 3 * SECOND),
    );
    let params = base(&[(FREEZER_ON, 1.0), (FADE_IN, 400.0), (FADE_OUT, 400.0)]);
    let out = render_events(
        &params,
        &input,
        512,
        &[
            set(seconds(0.5) as u32, FREEZE, 1.0),
            set(seconds(2.0) as u32, FREEZE, 0.0),
        ],
    );
    let window = 960;
    let level = |hz: f64, t: f64| at(&out[0], hz, seconds(t), window);
    // A rises linearly while B falls, their sum steady.
    let ts = [0.65, 0.75, 0.85];
    let frozen: Vec<f64> = ts.iter().map(|&t| level(a, t)).collect();
    let live: Vec<f64> = ts.iter().map(|&t| level(b, t)).collect();
    for k in 0..2 {
        assert!(frozen[k + 1] > frozen[k], "A rises {frozen:?}");
        assert!(live[k + 1] < live[k], "B falls {live:?}");
    }
    let steps: Vec<f64> = frozen.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        steps.iter().all(|s| (s - steps[0]).abs() < 0.3 * steps[0]),
        "linear {steps:?}"
    );
    for k in 0..3 {
        let sum = frozen[k] + live[k];
        assert!((sum - 0.4).abs() < 0.06, "the blend holds the level: {sum}");
    }
    // Released: the frozen A fades out linearly.
    let outs: Vec<f64> = [2.1, 2.2, 2.3].iter().map(|&t| level(a, t)).collect();
    let steps: Vec<f64> = outs.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        steps
            .iter()
            .all(|s| *s < 0.0 && (s - steps[0]).abs() < 0.3 * steps[0].abs()),
        "falls linearly {outs:?}"
    );
    assert!(level(a, 2.7) < 0.02, "and is gone");
}

/// Tones that change every `every` frames, `f(k)` Hz in the kth, each
/// beginning a few frames before its grid point so that the frames around a
/// capture hold it alone.
fn steps(every: usize, count: usize, f: impl Fn(usize) -> f64) -> Vec<f32> {
    (0..count * every)
        .map(|i| {
            let k = ((i + 1_024) / every).min(count - 1);
            0.5 * (TAU * f(k) * i as f64 / RATE.as_f64()).sin() as f32
        })
        .collect()
}

// Spec: spectral-time R6
#[test]
fn retrigger_refreezes_on_each_onset_in_the_input() {
    // 800 Hz begins at 0.5 s, 1,200 Hz at 1.5 s, each a half second.
    let input = add(
        &sine(800.0, 0.5, seconds(0.5), seconds(1.0)),
        &sine(1_200.0, 0.5, seconds(1.5), seconds(2.0)),
    );
    let mut input = input;
    input.resize(3 * SECOND, 0.0);
    let params = base(&[
        (FREEZER_ON, 1.0),
        (FREEZE, 1.0),
        (FREEZER_MODE, 1.0),
        (TRIGGER, 0.0),
        (FADE_SHAPE, 1.0),
        (FADE_IN, 20.0),
        (FADE_OUT, 800.0),
    ]);
    let out = render(&params, &input, 512);
    let window = 2_400;
    let at = |hz, t: f64| at(&out[0], hz, seconds(t), window);
    assert!(
        at(800.0, 0.3) < 0.01 && at(1_200.0, 0.3) < 0.01,
        "silent before the first onset"
    );
    assert!(
        at(800.0, 0.9) > 0.1,
        "the first onset is frozen: {}",
        at(800.0, 0.9)
    );
    assert!(at(1_200.0, 0.9) < 0.01);
    assert!(
        at(1_200.0, 1.9) > 0.1,
        "the second is frozen: {}",
        at(1_200.0, 1.9)
    );
    assert!(
        at(800.0, 2.4) < 0.01,
        "the first is gone: {}",
        at(800.0, 2.4)
    );
}

/// The first frame at which `signal` is above `level` at or after `from`.
fn first_above(signal: &[f32], level: f32, from: usize) -> usize {
    from + signal[from..]
        .iter()
        .position(|s| s.abs() > level)
        .expect("a sound")
}

fn sync_params() -> Vec<(ParamId, f64)> {
    base(&[
        (FREEZER_ON, 1.0),
        (FREEZE, 1.0),
        (FREEZER_MODE, 1.0),
        (TRIGGER, 1.0),
        (INTERVAL_MODE, 1.0),
        (INTERVAL_DIVISION, 4.0),
        (FADE_SHAPE, 1.0),
        (FADE_IN, 0.0),
        (FADE_OUT, 100.0),
    ])
}

// Spec: spectral-time R6
#[test]
fn retrigger_sync_captures_on_the_songs_grid_at_any_block_size() {
    // A quarter note at 120 BPM is 24,000 frames; the song starts on a
    // grid point, so a capture is asked for there, takes the next whole
    // frame, and a burst of the frozen tone begins a frame and the latency
    // later.
    let input = sine(1_000.0, 0.5, 0, 2 * SECOND);
    let one = render(&sync_params(), &input, 1);
    for block in [64, 512, 4_096] {
        assert_eq!(one, render(&sync_params(), &input, block), "block {block}");
    }
    for k in 1..4 {
        let grid = k * 24_000;
        let start = first_above(&one[0], 0.05, grid - 4_000);
        // The next whole frame, a hop's rounding after the grid, comes out
        // a latency later and rises under the synthesis window.
        let wanted = grid + LATENCY + HOP;
        assert!(
            start.abs_diff(wanted) <= 2 * HOP,
            "the {k}th: {start} for {wanted}"
        );
    }
    // Between two, the burst has faded away.
    assert!(rms(&one[0][grid_end(1)..grid_end(1) + 2_000]) < 0.01);
}

fn grid_end(k: usize) -> usize {
    k * 24_000 + LATENCY + 5_000
}

// Spec: spectral-time R6
#[test]
fn retrigger_sync_in_milliseconds_follows_the_song_not_the_blocks() {
    let mut params = sync_params();
    params.push((INTERVAL_MODE, 0.0));
    params.push((INTERVAL_TIME, 500.0));
    let input = sine(1_000.0, 0.5, 0, SECOND);
    let one = render(&params, &input, 1);
    assert_eq!(one, render(&params, &input, 4_096));
    let start = first_above(&one[0], 0.05, 20_000);
    assert!(start.abs_diff(24_000 + LATENCY + HOP) <= 2 * HOP, "{start}");
}

// Spec: spectral-time R7
#[test]
fn crossfade_trades_one_frozen_spectrum_for_the_next_at_equal_power() {
    // 1 kHz for a second, then 1.5 kHz; a freeze every half second
    // crossfading over 40% of it.
    let input = steps(seconds(0.5), 4, |k| if k < 2 { 1_000.0 } else { 1_500.0 });
    let mut params = sync_params();
    params.push((FADE_SHAPE, 0.0));
    params.push((X_FADE, 40.0));
    params.push((INTERVAL_MODE, 0.0));
    params.push((INTERVAL_TIME, 500.0));
    let out = render(&params, &input, 512);
    let window = 960;
    let level = |hz, t: f64| at(&out[0], hz, seconds(t), window);
    // Before the third capture: the second alone, whole.
    assert!(
        (level(1_000.0, 0.95) - 0.5).abs() < 0.05,
        "{}",
        level(1_000.0, 0.95)
    );
    // During the crossfade, the powers add to one.
    for t in [1.1, 1.15, 1.2] {
        let (a, b) = (level(1_000.0, t), level(1_500.0, t));
        let power = (a * a + b * b) / 0.25;
        assert!((0.8..1.2).contains(&power), "{t}: {a} {b} {power}");
    }
    assert!(level(1_000.0, 1.1) > level(1_000.0, 1.2), "the first falls");
    assert!(
        level(1_500.0, 1.1) < level(1_500.0, 1.2),
        "the second rises"
    );
    assert!(
        (level(1_500.0, 1.35) - 0.5).abs() < 0.05,
        "and it ends whole"
    );
    assert!(level(1_000.0, 1.35) < 0.02);
}

// Spec: spectral-time R7
#[test]
fn envelope_stacks_up_to_eight_freezes_and_a_ninth_drops_the_oldest() {
    // A tone of its own every 100 ms, frozen each time; the fall is long, so
    // they stay.
    let tone = |k: usize| 600.0 + 200.0 * k as f64;
    let input = steps(seconds(0.1), 12, tone);
    let mut params = sync_params();
    params.push((INTERVAL_MODE, 0.0));
    params.push((INTERVAL_TIME, 100.0));
    params.push((FADE_OUT, 5_000.0));
    let out = render(&params, &input, 512);
    let from = seconds(1.14);
    let amplitude = |k: usize| at(&out[0], tone(k), from, 2_400);
    let sounding: Vec<usize> = (0..12).filter(|&k| amplitude(k) > 0.2).collect();
    assert_eq!(sounding, (4..12).collect::<Vec<_>>(), "the last eight");
    for k in 0..4 {
        assert!(
            amplitude(k) < 0.05,
            "tone {k} was dropped: {}",
            amplitude(k)
        );
    }
    // Eight and no more, though the ninth had begun before the eighth ended.
    let eight = rms(&out[0][from..from + 2_400]);
    assert!(eight > 0.3, "{eight}");
}

/// A delay alone: Resolution 512, echoes only, no feedback.
fn delay(params: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut all = base(&[(DELAY_ON, 1.0), (FEEDBACK, 0.0)]);
    for &(param, value) in params {
        all.retain(|(p, _)| *p != param);
        all.push((param, value));
    }
    all
}

// Spec: spectral-time R8
#[test]
fn an_echo_lands_within_one_hop_of_its_time_in_every_mode() {
    let input = impulse(1_000, 3 * SECOND);
    let cases: [(Vec<(ParamId, f64)>, f64); 5] = [
        (vec![(DELAY_MODE, 0.0), (TIME, 250.0)], 12_000.0),
        // 1/8 note at 120 BPM: 6,000 frames.
        (vec![(DELAY_MODE, 1.0), (NOTES, 3.0)], 12_000.0),
        // Three sixteenths: 18,000 frames.
        (vec![(DELAY_MODE, 2.0), (SIXTEENTHS, 3.0)], 18_000.0),
        // Three sixteenth triplets: 12,000 frames.
        (vec![(DELAY_MODE, 3.0), (TRIPLETS, 3.0)], 12_000.0),
        // Three dotted sixteenths: 27,000 frames.
        (vec![(DELAY_MODE, 4.0), (DOTTED, 3.0)], 27_000.0),
    ];
    for (params, frames) in cases {
        let out = render(&delay(&params), &input, 512);
        let at = peak(&out[0]);
        let wanted = 1_000.0 + LATENCY as f64 + frames;
        assert!(
            (at as f64 - wanted).abs() <= HOP as f64,
            "{params:?}: {at} for {wanted}"
        );
    }
}

// Spec: spectral-time R8
#[test]
fn a_synced_time_follows_the_tempo() {
    // 1/4 note: 24,000 frames at 120 BPM, 36,000 at 80.
    let params = delay(&[(DELAY_MODE, 1.0), (NOTES, 4.0)]);
    let input = impulse(0, 2 * SECOND);
    let at_120 = peak(&render(&params, &input, 512)[0]);
    assert!((at_120 as f64 - (LATENCY + 24_000) as f64).abs() <= HOP as f64);
    let slow = TempoMap::constant(Bpm::new(80.0).unwrap());
    let mut node = node(&registry(), &device_with(1, &params), 512);
    let out = run_blocks(node.as_mut(), &[input.clone(), input], 512, &[], &slow);
    let at_80 = peak(&out[0]);
    assert!(
        (at_80 as f64 - (LATENCY + 36_000) as f64).abs() <= HOP as f64,
        "{at_80}"
    );
}

/// The time at which a burst's energy at `hz` peaks in `out`, in seconds.
fn peaks_at(out: &[f32], hz: f64) -> f64 {
    let window = 960;
    let mut best = (0.0, 0);
    for start in (0..out.len() - window).step_by(240) {
        let level = at(out, hz, start, window);
        if level > best.0 {
            best = (level, start + window / 2);
        }
    }
    best.1 as f64 / SECOND as f64
}

// Spec: spectral-time R9
#[test]
fn tilt_delays_the_highs_more_or_the_lows_more_as_its_sign_says() {
    // A burst of 200 Hz and 6,400 Hz at 0.2 s, 50 ms long. At Tilt +100 % the
    // delay factor runs from 0 at 20 Hz to 2 at 20 kHz in log frequency:
    // 0.67 at 200 Hz and 1.6 at 6,400 Hz.
    let both = add(
        &sine(200.0, 0.4, seconds(0.2), seconds(0.25)),
        &sine(6_400.0, 0.4, seconds(0.2), seconds(0.25)),
    );
    let mut input = both;
    input.resize(2 * SECOND, 0.0);
    let centre = 0.225 + LATENCY as f64 / SECOND as f64;
    let time = 0.2;
    for (tilt, low, high) in [(100.0, 0.667, 1.6), (-100.0, 1.333, 0.4), (0.0, 1.0, 1.0)] {
        let out = render(&delay(&[(TIME, time * 1_000.0), (TILT, tilt)]), &input, 512);
        let (l, h) = (peaks_at(&out[0], 200.0), peaks_at(&out[0], 6_400.0));
        assert!(
            (l - centre - time * low).abs() < 0.02,
            "Tilt {tilt}: low at {l}"
        );
        assert!(
            (h - centre - time * high).abs() < 0.02,
            "Tilt {tilt}: high at {h}"
        );
    }
}

/// How much of the energy of `out` lies between `from` and `to` frames.
fn share(out: &[f32], from: usize, to: usize) -> f64 {
    let total: f64 = out.iter().map(|s| f64::from(*s).powi(2)).sum();
    let part: f64 = out[from..to].iter().map(|s| f64::from(*s).powi(2)).sum();
    part / total
}

// Spec: spectral-time R9
#[test]
fn spray_scatters_each_bins_delay_the_same_way_every_run() {
    let input = impulse(1_000, 2 * SECOND);
    let echo = 1_000 + LATENCY + 12_000;
    let tight = render(&delay(&[(TIME, 250.0)]), &input, 512);
    assert!(
        share(&tight[0], echo - HOP, echo + HOP) > 0.99,
        "no Spray: one echo"
    );
    let params = delay(&[(TIME, 250.0), (SPRAY, 100.0)]);
    let scattered = render(&params, &input, 512);
    assert!(
        share(&scattered[0], echo - HOP, echo + HOP) < 0.3,
        "the echo is spread"
    );
    assert!(
        share(&scattered[0], 0, echo - 2 * HOP) > 0.1,
        "some earlier than the time"
    );
    assert!(
        share(&scattered[0], echo + 12_000 + 2 * HOP, 2 * SECOND) < 1e-3,
        "none later than twice it"
    );
    // The same every run and at any block size.
    for block in [1, 64, 4_096] {
        assert_eq!(scattered, render(&params, &input, block), "block {block}");
    }
    // Another device scatters otherwise.
    let mut node = node(&registry(), &device_with(2, &params), 512);
    let other = run_blocks(node.as_mut(), &[input.clone(), input], 512, &[], &tempo());
    assert_ne!(scattered, other);
}

// Spec: spectral-time R10
#[test]
fn mask_limits_the_delay_to_the_highs_or_the_lows() {
    let both = add(
        &sine(200.0, 0.4, 0, seconds(0.5)),
        &sine(5_000.0, 0.4, 0, seconds(0.5)),
    );
    let mut input = both;
    input.resize(seconds(1.5), 0.0);
    // The delay is 0.4 s; the part left out comes through at once.
    for (mask, early, late) in [(50.0, 200.0, 5_000.0), (-50.0, 5_000.0, 200.0)] {
        let out = render(&delay(&[(TIME, 400.0), (MASK, mask)]), &input, 512);
        let first = seconds(0.2);
        assert!(
            at(&out[0], early, first, 4_800) > 0.3,
            "Mask {mask}: {early} Hz at once"
        );
        assert!(
            at(&out[0], late, first, 4_800) < 0.02,
            "Mask {mask}: {late} Hz waits"
        );
        let later = seconds(0.7);
        assert!(
            at(&out[0], late, later, 4_800) > 0.3,
            "Mask {mask}: {late} Hz arrives"
        );
    }
    // At 0 the whole spectrum waits.
    let out = render(&delay(&[(TIME, 400.0)]), &input, 512);
    assert!(at(&out[0], 200.0, seconds(0.2), 4_800) < 0.02);
}

// Spec: spectral-time R11
#[test]
fn each_repeat_is_feedback_times_the_last_to_within_half_a_decibel() {
    let input = impulse(500, 3 * SECOND);
    for feedback in [50.0, 80.0] {
        let out = render(&delay(&[(TIME, 400.0), (FEEDBACK, feedback)]), &input, 512);
        let echoes: Vec<f32> = (1..=3)
            .map(|k| {
                let at = 500 + LATENCY + k * 19_200;
                out[0][at - HOP..at + HOP]
                    .iter()
                    .fold(0.0f32, |m, s| m.max(s.abs()))
            })
            .collect();
        for pair in echoes.windows(2) {
            let step = db(f64::from(pair[1] / pair[0]));
            assert!(
                (step - db(feedback / 100.0)).abs() < 0.5,
                "Feedback {feedback}: {echoes:?}"
            );
        }
    }
}

// Spec: spectral-time R11
#[test]
fn shift_moves_each_repeat_by_its_interval_but_not_the_first() {
    // 984.375 Hz is the centre of bin 42 at 2,048 frames.
    let hz = 984.375;
    let mut input = sine(hz, 0.4, 0, seconds(0.5));
    input.resize(3 * SECOND, 0.0);
    let params = |shift: f64| {
        base(&[
            (RESOLUTION, 3.0),
            (DELAY_ON, 1.0),
            (TIME, 500.0),
            (FEEDBACK, 70.0),
            (SHIFT, shift),
        ])
    };
    let loudest = |out: &[f32], from: f64| {
        let from = seconds(from);
        let window = &out[from..from + 16_384];
        let spectrum = common::spectrum(window);
        let bin = spectrum
            .iter()
            .enumerate()
            .skip(5)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap();
        (bin as f64 * SECOND as f64 / 16_384.0, spectrum[bin])
    };
    let latency = 2_047.0 / SECOND as f64;
    let shifted = render(&params(12.0), &input, 512);
    let (first, level) = loudest(&shifted[0], 0.5 + latency + 0.1);
    assert!(
        (first - hz).abs() < 24.0,
        "the first repeat is where it was: {first}"
    );
    assert!(level > 0.1);
    let (second, level) = loudest(&shifted[0], 1.0 + latency + 0.1);
    assert!(
        (second - 2.0 * hz).abs() < 48.0,
        "the second is an octave up: {second}"
    );
    assert!(level > 0.05, "{level}");
    let (third, _) = loudest(&shifted[0], 1.5 + latency + 0.1);
    assert!(third > 3.0 * hz, "and the third higher still: {third}");
    let plain = render(&params(0.0), &input, 512);
    let (second, _) = loudest(&plain[0], 1.0 + latency + 0.1);
    assert!(
        (second - hz).abs() < 24.0,
        "without Shift they stay: {second}"
    );
}

// Spec: spectral-time R11
#[test]
fn stereo_delays_the_right_channel_up_to_half_the_time_longer() {
    let input = impulse(0, 2 * SECOND);
    let out = render(&delay(&[(TIME, 400.0), (STEREO, 100.0)]), &input, 512);
    let (left, right) = (peak(&out[0]), peak(&out[1]));
    assert!((left as f64 - (LATENCY + 19_200) as f64).abs() <= HOP as f64);
    assert!(
        (right as f64 - (LATENCY + 28_800) as f64).abs() <= HOP as f64,
        "{right}"
    );
}

// Spec: spectral-time R11
#[test]
fn the_delays_own_dry_wet_balances_its_input_against_its_echoes() {
    let input = impulse(0, SECOND);
    let out = render(&delay(&[(TIME, 200.0), (DELAY_DRY_WET, 25.0)]), &input, 512);
    let direct = out[0][LATENCY];
    let echo = out[0][LATENCY + 9_600];
    assert!((direct - 0.75).abs() < 0.01, "{direct}");
    assert!((echo - 0.25).abs() < 0.01, "{echo}");
}

// Spec: spectral-time R12
#[test]
fn a_time_beyond_the_delay_memory_is_held_at_it() {
    // Four bars at 120 BPM is 8 s, a hop's rounding beyond which is the end
    // of the memory; 16 bars is far past it.
    let params = delay(&[(DELAY_MODE, 1.0), (NOTES, 8.0)]);
    assert!(st::delay_is_capped(|id| value(&params, id), 60.0));
    assert!(!st::delay_is_capped(|id| value(&params, id), 120.0));
    assert!(!st::delay_is_capped(|id| value(&delay(&[]), id), 60.0));
    // 16 beats at 60 BPM is 16 s.
    let slow = TempoMap::constant(Bpm::new(60.0).unwrap());
    let input = impulse(0, 9 * SECOND);
    let mut node = node(&registry(), &device_with(1, &params), 4_096);
    let out = run_blocks(node.as_mut(), &[input.clone(), input], 4_096, &[], &slow);
    let at = peak(&out[0]);
    let end = (st::CAP_SECONDS * SECOND as f64) as usize;
    assert!(
        (end..=end + HOP + LATENCY + HOP).contains(&at),
        "held at the memory's end: {at} for {end}"
    );
}

fn value(params: &[(ParamId, f64)], id: ParamId) -> f64 {
    params
        .iter()
        .find(|(p, _)| *p == id)
        .map(|(_, v)| *v)
        .or_else(|| {
            spectral_time_type()
                .description
                .param(id)
                .map(|p| p.default)
        })
        .unwrap()
}

// Spec: spectral-time R13
#[test]
fn the_order_switch_runs_the_freezer_into_the_delay_or_the_delay_into_the_freezer() {
    // A until one second. The freeze is made at 0.1 s, before any echo of A
    // has come back from a 0.3 s delay.
    let input = sine(1_000.0, 0.4, 0, SECOND);
    let mut input = input;
    input.resize(4 * SECOND, 0.0);
    let params = |order: f64| {
        base(&[
            (FREEZER_ON, 1.0),
            (DELAY_ON, 1.0),
            (TIME, 300.0),
            (FEEDBACK, 0.0),
            (FADE_IN, 0.0),
            (ORDER, order),
        ])
    };
    let events = [set(seconds(0.1) as u32, FREEZE, 1.0)];
    // Frz > Dly: A is frozen, and what is frozen is delayed.
    let freezer_first = render_events(&params(0.0), &input, 512, &events);
    let held = rms(&freezer_first[0][3 * SECOND..4 * SECOND]);
    assert!(held > 0.2, "the frozen tone is delayed and sounds: {held}");
    // Dly > Frz: the freeze takes the delay's output, silent at 0.1 s.
    let delay_first = render_events(&params(1.0), &input, 512, &events);
    let silent = rms(&delay_first[0][3 * SECOND..4 * SECOND]);
    assert!(silent < 0.01, "the freeze holds the silence: {silent}");
}

// Spec: spectral-time R13
#[test]
fn with_a_section_off_it_is_skipped_in_either_order() {
    let input = common::noise(8_192);
    for order in [0.0, 1.0] {
        let skipped = render(&base(&[(ORDER, order)]), &input, 512);
        let delayed = render(
            &base(&[(DELAY_ON, 1.0), (DELAY_DRY_WET, 0.0), (ORDER, order)]),
            &input,
            512,
        );
        let frozen = render(&base(&[(FREEZER_ON, 1.0), (ORDER, order)]), &input, 512);
        // Neither, or a section that changes nothing: the signal, unchanged
        // but for the frame.
        for n in 0..8_192 - LATENCY {
            assert!((skipped[0][n + LATENCY] - input[n]).abs() < 1e-4);
        }
        assert_eq!(
            skipped, delayed,
            "a delay with nothing wet passes the sound"
        );
        assert_eq!(skipped, frozen, "a freezer with no freeze passes the sound");
    }
}

// Spec: spectral-time R14
#[test]
fn dry_wet_balances_the_dry_signal_against_the_wet() {
    let input = impulse(0, SECOND);
    let out = render(&delay(&[(TIME, 200.0), (DRY_WET, 50.0)]), &input, 512);
    assert!(
        (out[0][LATENCY] - 0.5).abs() < 0.01,
        "the dry signal at half"
    );
    assert!(
        (out[0][LATENCY + 9_600] - 0.5).abs() < 0.01,
        "and the echo at half"
    );
}

// Spec: spectral-time R15
#[test]
fn a_render_is_the_same_to_the_bit_at_blocks_of_1_64_512_and_4096_and_run_after_run() {
    // Everything on: noise through a retriggering freezer on onsets and a
    // delay with Tilt, Spray, Mask, Feedback, Shift and Stereo, the
    // Resolution changing partway.
    let bursts = add(
        &common::noise(seconds(1.0)),
        &sine(700.0, 0.5, seconds(0.3), seconds(0.6)),
    );
    let params = base(&[
        (FREEZER_ON, 1.0),
        (DELAY_ON, 1.0),
        (FREEZE, 1.0),
        (FREEZER_MODE, 1.0),
        (TRIGGER, 0.0),
        (SENSITIVITY, 80.0),
        (FADE_SHAPE, 0.0),
        (TIME, 120.0),
        (TILT, 40.0),
        (SPRAY, 30.0),
        (MASK, -20.0),
        (FEEDBACK, 60.0),
        (SHIFT, 5.0),
        (STEREO, 50.0),
        (DELAY_DRY_WET, 70.0),
        (DRY_WET, 80.0),
    ]);
    let events = [
        set(seconds(0.4) as u32, RESOLUTION, 2.0),
        set(seconds(0.7) as u32, TIME, 200.0),
    ];
    let one = render_events(&params, &bursts, 1, &events);
    assert!(rms(&one[0]) > 0.01, "it makes sound");
    for block in [64, 512, 4_096] {
        assert_eq!(
            one,
            render_events(&params, &bursts, block, &events),
            "block {block}"
        );
    }
    assert_eq!(
        one,
        render_events(&params, &bursts, 1, &events),
        "run after run"
    );
}

// Spec: spectral-time R16
#[test]
fn the_spectrogram_shows_the_wet_output_against_log_frequency_over_four_seconds() {
    let input = sine(1_000.0, 1.0, SECOND, 6 * SECOND);
    let params = base(&[(RESOLUTION, 3.0)]);
    let publish = |block: usize| {
        let mut node = node(&registry(), &device_with(1, &params), block);
        let mut reader = node.display().expect("the device has a display");
        run_blocks(
            node.as_mut(),
            &[input.clone(), input.clone()],
            block,
            &[],
            &tempo(),
        );
        reader.read().values.clone()
    };
    let values = publish(512);
    assert_eq!(values.len(), st::GRID.columns * st::GRID.rows);
    let description = spectral_time_type().description;
    let id = DeviceId(Id::new(1).unwrap());
    let shown = st::display(id, &description, Some(&values));
    assert!(shown.live);
    let spectrogram = &shown.spectrograms[0];
    assert_eq!(spectrogram.grid, st::GRID);
    let (hz, level) = spectrogram.peak().expect("a newest column");
    let row = (st::GRID.frequencies[1] / st::GRID.frequencies[0]).ln() / (st::GRID.rows - 1) as f32;
    assert!((hz / 1_000.0).ln().abs() < row, "{hz} Hz");
    assert!((-3.0..=0.5).contains(&level), "{level} dB");
    assert!(!st::display(id, &description, None).live);
    // Six seconds in, the tone has filled the last four.
    assert!(spectrogram.column(0).unwrap().iter().any(|&v| v > -20.0));
    for block in [1, 64, 4_096] {
        assert_eq!(values, publish(block), "block {block}");
    }
    // Before the tone, a window of four seconds ends at the floor.
    let early = {
        let mut node = node(&registry(), &device_with(1, &params), 512);
        let mut reader = node.display().unwrap();
        let quiet = vec![0.0; SECOND / 2];
        run_blocks(node.as_mut(), &[quiet.clone(), quiet], 512, &[], &tempo());
        reader.read().values.clone()
    };
    assert!(
        early.iter().all(|&v| v == st::GRID.levels[0]),
        "silence is the floor"
    );
}

// Spec: spectral-time R6
#[test]
fn sensitivity_lowers_the_threshold_an_onset_must_cross() {
    // A steady bed of noise, and a tone that joins it at one second.
    let mut input = add(
        &common::noise(3 * SECOND)
            .iter()
            .map(|s| s * 0.25)
            .collect::<Vec<_>>(),
        &sine(1_500.0, 0.25, SECOND, 3 * SECOND),
    );
    input[..4_000].fill(0.0);
    let params = |sensitivity: f64| {
        base(&[
            (FREEZER_ON, 1.0),
            (FREEZE, 1.0),
            (FREEZER_MODE, 1.0),
            (TRIGGER, 0.0),
            (SENSITIVITY, sensitivity),
            (FADE_SHAPE, 1.0),
            (FADE_IN, 0.0),
            (FADE_OUT, 5_000.0),
        ])
    };
    let frozen_tone = |sensitivity: f64| {
        let out = render(&params(sensitivity), &input, 512);
        at(&out[0], 1_500.0, seconds(1.5), 4_800)
    };
    let (keen, deaf) = (frozen_tone(100.0), frozen_tone(0.0));
    assert!(keen > 0.05, "the tone is frozen at 100 %: {keen}");
    assert!(deaf < 0.5 * keen, "and not at 0 %: {deaf}");
}
