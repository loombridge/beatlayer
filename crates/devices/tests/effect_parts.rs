//! The effects' shared parts (WP-5.42; the device framework spec, R26 to
//! R29), through the framework's test effects device: the LFO's waves,
//! rates, stereo modes and sync, the envelope follower and the seeded
//! random source. Every block runs as a real-time section under the
//! allocation trap, at block sizes 1, 64, 512, and 4,096.

use crate::common;
use bl_core::id::Id;
use bl_core::time::{Bpm, Frames, TempoMap, TempoSegment, Ticks};
use bl_devices::lfo::Wave;
use bl_devices::testing::{
    self, EFFECTS_ATTACK, EFFECTS_DIVISION, EFFECTS_HOLD, EFFECTS_OUTPUT, EFFECTS_PERIOD,
    EFFECTS_PHASE, EFFECTS_RATE, EFFECTS_RATE_MODE, EFFECTS_RELEASE, EFFECTS_SPIN, EFFECTS_STEREO,
    EFFECTS_WAVE,
};
use bl_engine::{NodeEvent, TransportInfo};
use bl_model::{Device, DeviceId, ParamId};
use common::{CHANNELS, RATE, node};

const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

/// The test effects as device number `id`, with `params` set.
fn effects(id: u64, params: &[(ParamId, f64)]) -> Device {
    let mut device = testing::effects_type().device(DeviceId(Id::new(id).unwrap()));
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    device
}

/// Renders `input` (the same on both channels) through the test effects
/// with `params`, `block` frames at a time, the song playing from its start
/// when `playing`.
fn render(
    id: u64,
    params: &[(ParamId, f64)],
    input: &[f32],
    block: usize,
    tempo: &TempoMap,
    playing: bool,
) -> Vec<Vec<f32>> {
    let registry = testing::registry();
    let mut node = node(&registry, &effects(id, params), block);
    let mut out = vec![Vec::new(); CHANNELS];
    for (n, chunk) in input.chunks(block).enumerate() {
        let channels = vec![chunk.to_vec(); CHANNELS];
        let transport = TransportInfo {
            playing,
            position: Frames((n * block) as i64),
        };
        let events: [NodeEvent; 0] = [];
        let done = common::run_at(node.as_mut(), &channels, &events, tempo, transport);
        for (all, part) in out.iter_mut().zip(done) {
            all.extend(part);
        }
    }
    out
}

/// The LFO's value on both channels for `frames` frames.
fn lfo(
    params: &[(ParamId, f64)],
    frames: usize,
    block: usize,
    tempo: &TempoMap,
    playing: bool,
) -> Vec<Vec<f32>> {
    let mut all = vec![(EFFECTS_OUTPUT, 1.0)];
    all.extend_from_slice(params);
    render(1, &all, &vec![0.0; frames], block, tempo, playing)
}

fn steady() -> TempoMap {
    TempoMap::constant(Bpm::DEFAULT)
}

/// The frames at which `values` rises through zero, to a fraction of a
/// frame.
fn rising_zero_crossings(values: &[f32]) -> Vec<f64> {
    values
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] < 0.0 && pair[1] >= 0.0)
        .map(|(i, pair)| i as f64 + f64::from(-pair[0] / (pair[1] - pair[0])))
        .collect()
}

/// The mean frames between the first and last rising crossing.
fn period(values: &[f32]) -> f64 {
    let crossings = rising_zero_crossings(values);
    assert!(crossings.len() > 3, "{} crossings", crossings.len());
    (crossings[crossings.len() - 1] - crossings[0]) / (crossings.len() - 1) as f64
}

// Spec: device-framework R26
#[test]
fn each_wave_keeps_to_its_range_and_repeats_at_its_rate() {
    // 10 Hz at 48 kHz: 4,800 frames to the cycle, three cycles.
    let tempo = steady();
    for (index, wave) in Wave::ALL.iter().enumerate() {
        let params = [(EFFECTS_WAVE, index as f64), (EFFECTS_RATE, 10.0)];
        let reference = lfo(&params, 14_400, 4096, &tempo, false);
        for channel in &reference {
            assert!(
                channel.iter().all(|v| (-1.0..=1.0).contains(v)),
                "{wave:?} keeps to -1..1"
            );
        }
        let random = matches!(wave, Wave::Random | Wave::Wander | Wave::SampleHold);
        if !random && *wave != Wave::TriangleAnalog {
            // A wave that jumps may land a frame either side of its jump.
            let apart = (0..9_600)
                .filter(|&i| (reference[0][i] - reference[0][i + 4_800]).abs() > 1e-5)
                .count();
            assert!(apart <= 16, "{wave:?} repeats: {apart} frames differ");
        }
        for block in BLOCKS {
            assert_eq!(
                lfo(&params, 14_400, block, &tempo, false),
                reference,
                "{wave:?} at block size {block}"
            );
        }
    }
}

// Spec: device-framework R27
#[test]
fn the_rate_in_hertz_in_seconds_and_in_beats_holds_within_a_tenth_of_a_per_cent() {
    let tempo = steady();
    let frames = 192_000;
    // 7.3 Hz.
    let hz = period(&lfo(&[(EFFECTS_RATE, 7.3)], frames, 512, &tempo, false)[0]);
    assert!((hz - 48_000.0 / 7.3).abs() < 48_000.0 / 7.3 * 0.001, "{hz}");
    // 0.137 s to the cycle.
    let seconds = period(
        &lfo(
            &[(EFFECTS_RATE_MODE, 1.0), (EFFECTS_PERIOD, 0.137)],
            frames,
            512,
            &tempo,
            false,
        )[0],
    );
    assert!(
        (seconds - 0.137 * 48_000.0).abs() < 0.137 * 48.0,
        "{seconds}"
    );
    // A quarter note at 120 BPM is half a second, playing or not.
    for playing in [true, false] {
        let beats = period(
            &lfo(
                &[(EFFECTS_RATE_MODE, 2.0), (EFFECTS_DIVISION, 2.0)],
                frames,
                512,
                &tempo,
                playing,
            )[0],
        );
        assert!((beats - 24_000.0).abs() < 24.0, "{beats} playing {playing}");
    }
}

// Spec: device-framework R27
#[test]
fn a_synced_cycle_starts_on_the_beat_at_two_tempos_and_across_a_tempo_change() {
    let constant = |bpm| TempoMap::constant(Bpm::new(bpm).unwrap());
    let changing = TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(4),
            bpm: Bpm::new(90.0).unwrap(),
        },
    ])
    .unwrap();
    // A ramp up from the saw's bottom: a cycle's start reads -1.
    let params = [
        (EFFECTS_WAVE, 6.0),
        (EFFECTS_RATE_MODE, 2.0),
        (EFFECTS_DIVISION, 2.0),
    ];
    for tempo in [constant(120.0), constant(75.0), changing] {
        let reference = lfo(&params, 4 * 48_000, 4096, &tempo, true);
        for beat in 1..8 {
            let at = tempo.ticks_to_frames(Ticks::from_quarters(beat), RATE).0 as usize;
            if at >= reference[0].len() {
                continue;
            }
            let phase = f64::from(reference[0][at] + 1.0) / 2.0;
            assert!(
                phase.min(1.0 - phase) < 0.001,
                "beat {beat}: the cycle is at {phase}"
            );
            let half = tempo
                .ticks_to_frames(Ticks(Ticks::from_quarters(beat).0 + 352_800), RATE)
                .0 as usize;
            if half < reference[0].len() {
                let phase = f64::from(reference[0][half] + 1.0) / 2.0;
                assert!((phase - 0.5).abs() < 0.001, "half a beat on: {phase}");
            }
        }
        for block in BLOCKS {
            assert_eq!(
                lfo(&params, 4 * 48_000, block, &tempo, true),
                reference,
                "at block size {block}"
            );
        }
    }
}

// Spec: device-framework R27
#[test]
fn phase_puts_the_channels_in_antiphase_and_spin_detunes_the_second_rate() {
    let tempo = steady();
    let base = [(EFFECTS_RATE, 5.0)];
    // Phase 180: the right channel is the left upside down.
    let mut phase = base.to_vec();
    phase.push((EFFECTS_PHASE, 180.0));
    for block in BLOCKS {
        let out = lfo(&phase, 48_000, block, &tempo, false);
        for (l, r) in out[0].iter().zip(&out[1]) {
            assert!((l + r).abs() < 1e-6, "{l} {r}");
        }
    }
    // Spin 20 %: the right runs at 6 Hz against the left's 5.
    let mut spin = base.to_vec();
    spin.extend([(EFFECTS_STEREO, 1.0), (EFFECTS_SPIN, 20.0)]);
    let out = lfo(&spin, 4 * 48_000, 512, &tempo, false);
    let ratio = period(&out[0]) / period(&out[1]);
    assert!((ratio - 1.2).abs() < 1.2 * 0.001, "{ratio}");
    for block in BLOCKS {
        assert_eq!(lfo(&spin, 4 * 48_000, block, &tempo, false), out);
    }
}

/// Frames from `from` until `values` crosses `level`, rising or falling.
fn frames_until(values: &[f32], from: usize, level: f32, rising: bool) -> usize {
    values[from..]
        .iter()
        .position(|v| if rising { *v >= level } else { *v <= level })
        .expect("it gets there")
}

// Spec: device-framework R28
#[test]
fn the_followers_attack_and_release_are_time_constants_within_five_per_cent() {
    let tempo = steady();
    // A second of signal, then a second of silence.
    let input: Vec<f32> = (0..96_000)
        .map(|i| if i < 48_000 { 1.0 } else { 0.0 })
        .collect();
    let params = [
        (EFFECTS_OUTPUT, 2.0),
        (EFFECTS_ATTACK, 20.0),
        (EFFECTS_RELEASE, 100.0),
    ];
    let reference = render(1, &params, &input, 4096, &tempo, false);
    let one_minus_e = 1.0 - (-1.0f32).exp();
    let attack = frames_until(&reference[0], 0, one_minus_e, true) as f64 / 48_000.0;
    assert!((attack - 0.020).abs() < 0.020 * 0.05, "attack {attack} s");
    let release = frames_until(&reference[0], 48_000, 1.0 - one_minus_e, false) as f64 / 48_000.0;
    assert!(
        (release - 0.100).abs() < 0.100 * 0.05,
        "release {release} s"
    );
    for block in BLOCKS {
        assert_eq!(render(1, &params, &input, block, &tempo, false), reference);
    }
}

// Spec: device-framework R28
#[test]
fn hold_finishes_the_attack_before_the_release() {
    let tempo = steady();
    // A pulse far shorter than the 50 ms attack.
    let input: Vec<f32> = (0..12_000)
        .map(|i| if i < 24 { 1.0 } else { 0.0 })
        .collect();
    let top = |hold: f64, block: usize| {
        let params = [
            (EFFECTS_OUTPUT, 2.0),
            (EFFECTS_ATTACK, 50.0),
            (EFFECTS_RELEASE, 50.0),
            (EFFECTS_HOLD, hold),
        ];
        render(1, &params, &input, block, &tempo, false)[0]
            .iter()
            .fold(0.0f32, |top, v| top.max(*v))
    };
    for block in BLOCKS {
        assert!(top(0.0, block) < 0.02, "without Hold");
        assert!(top(1.0, block) > 0.9, "with Hold");
    }
}

// Spec: device-framework R29
#[test]
fn the_random_source_repeats_run_after_run_and_differs_between_devices() {
    let tempo = steady();
    let shapes = |wave: Wave| {
        let index = Wave::ALL.iter().position(|w| *w == wave).unwrap() as f64;
        [(EFFECTS_WAVE, index), (EFFECTS_RATE, 40.0)]
    };
    for wave in [Wave::SampleHold, Wave::Wander, Wave::Random] {
        let run = |id: u64, block: usize| {
            let mut params = vec![(EFFECTS_OUTPUT, 1.0)];
            params.extend(shapes(wave));
            render(id, &params, &[0.0; 24_000], block, &tempo, false)
        };
        let first = run(1, 4096);
        assert_eq!(run(1, 4096), first, "{wave:?}: run after run");
        for block in BLOCKS {
            assert_eq!(run(1, block), first, "{wave:?} at block size {block}");
        }
        assert_ne!(run(2, 4096), first, "{wave:?}: two devices");
        let distinct = |channel: &[f32]| {
            let mut values: Vec<i64> = channel.iter().map(|v| (v * 1e6) as i64).collect();
            values.sort_unstable();
            values.dedup();
            values.len()
        };
        assert!(distinct(&first[0]) > 10, "{wave:?} wanders");
    }
}
