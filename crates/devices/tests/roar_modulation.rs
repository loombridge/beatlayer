//! Roar, part 2 (WP-8.39; the roar spec R21 to R36): the matrix and its
//! sources on the 32-frame grid, the feedback section and its Note mode, the
//! compressor, the sidechains, and renders that don't depend on the block
//! size. Every block runs under the allocation trap.

use std::f64::consts::TAU;

use crate::common::{self, CHANNELS, RATE, device, node, noise, set};
use bl_core::time::{Bpm, Frames, TempoMap};
use bl_devices::roar::{
    AMOUNT, BLEND, COMPRESSION, DRIVE, ENV_ATTACK, ENV_FREQUENCY, ENV_RELEASE, ENV_THRESHOLD,
    ENV_WIDTH, FB_AMOUNT, FB_FREQUENCY, FB_MODE, FB_NOTE, FB_SYNCED, FB_TIME, FB_WIDTH, FILTER,
    GLOBAL_MODULATION, INPUT_LISTEN, LEVEL, LFO_MODE, LFO_MORPH, LFO_RATE, LFO_SIXTEENTHS,
    LFO_SMOOTH, LFO_SYNCED, LFO_WAVE, MIDI_FB_NOTE, MODES, NOISE_RATE, NOISE_TYPE, ON, OUTPUT_GAIN,
    PEAK_GAIN, ROUTING_MODE, SHAPER, SIDECHAIN_HP, SOURCES, TARGET_IDS, TARGETS, lfo_param,
    matrix_param, roar_type, stage_param,
};
use bl_engine::{Block, DeviceType, NodeEvent, NoteBuffer, NoteEvent, Registry, TransportInfo};
use bl_model::ParamId;

/// The latency of the 4 times oversampler, in frames.
const LATENCY: usize = 48;
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::roar::register(&mut registry);
    registry
}

fn p(stage: usize, offset: u32) -> ParamId {
    stage_param(stage, offset)
}

/// Stage `stage` passes a small signal as it is: a Digital Clip at Amount 0
/// and a Peak filter at 0 dB.
fn clean(stage: usize) -> Vec<(ParamId, f64)> {
    vec![
        (p(stage, SHAPER), 1.0),
        (p(stage, AMOUNT), 0.0),
        (p(stage, FILTER), 4.0),
        (p(stage, PEAK_GAIN), 0.0),
    ]
}

fn with(mut params: Vec<(ParamId, f64)>, more: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    params.extend_from_slice(more);
    params
}

/// What a run holds besides its parameters: both channels' input, the
/// sidechain's signal, the notes the MIDI sidechain hears and parameter
/// changes, with offsets from the run's start.
#[derive(Default, Clone)]
struct Run {
    left: Vec<f32>,
    right: Vec<f32>,
    key: Option<Vec<f32>>,
    notes: Vec<NoteEvent>,
    events: Vec<NodeEvent>,
}

impl Run {
    fn of(input: &[f32]) -> Self {
        Self {
            left: input.to_vec(),
            right: input.to_vec(),
            ..Self::default()
        }
    }
}

/// Plays `run` through a Roar of `params` in blocks of `block` at 120 BPM
/// with the song playing from its start, and returns both channels.
fn render(params: &[(ParamId, f64)], run: &Run, block: usize) -> Vec<Vec<f32>> {
    let device_type: DeviceType = roar_type();
    let mut node = node(&registry(), &device(&device_type, params), block);
    let tempo = TempoMap::constant(Bpm::DEFAULT);
    let frames = run.left.len();
    let mut out: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(frames)).collect();
    let mut notes_out = NoteBuffer::with_capacity(8);
    let mut done = 0;
    while done < frames {
        let length = block.min(frames - done);
        let range = done..done + length;
        let mut data: Vec<f32> = run.left[range.clone()]
            .iter()
            .chain(&run.right[range.clone()])
            .copied()
            .collect();
        let key: Option<Vec<f32>> = run.key.as_ref().map(|k| {
            let part = &k[range.clone()];
            part.iter().chain(part).copied().collect()
        });
        let here = |at: u32| (at as usize) >= done && (at as usize) < done + length;
        let notes: Vec<NoteEvent> = run
            .notes
            .iter()
            .filter(|n| here(n.offset))
            .map(|n| NoteEvent {
                offset: n.offset - done as u32,
                ..*n
            })
            .collect();
        let events: Vec<NodeEvent> = run
            .events
            .iter()
            .filter(|e| here(e.offset))
            .map(|e| NodeEvent {
                offset: e.offset - done as u32,
                ..*e
            })
            .collect();
        {
            let transport = TransportInfo {
                playing: true,
                position: Frames(done as i64),
                ..TransportInfo::default()
            };
            let mut block = Block::new(
                &mut data, CHANNELS, length, length, &events, transport, RATE,
            )
            .with_tempo(&tempo)
            .with_notes(&notes, &mut notes_out);
            if let Some(key) = &key {
                block = block.with_sidechain(key);
            }
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (channel, values) in out.iter_mut().zip(data.chunks(length)) {
            channel.extend_from_slice(values);
        }
        done += length;
    }
    out
}

/// A sine's samples.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

/// The largest magnitude of `samples` from `from` up to `to`.
fn peak(samples: &[f32], from: usize, to: usize) -> f64 {
    samples[from..to]
        .iter()
        .fold(0.0f64, |m, s| m.max(f64::from(s.abs())))
}

/// A parameter on the target with matrix place `target`: source `source`
/// moves it by `amount` per cent.
fn matrix(source: usize, target: ParamId, amount: f64) -> (ParamId, f64) {
    let place = TARGET_IDS
        .iter()
        .position(|id| *id == target)
        .expect("a target");
    (matrix_param(source, place), amount)
}

// Spec: roar R23
#[test]
fn the_matrix_has_an_automatable_amount_for_each_source_and_target() {
    let description = roar_type().description;
    description.check().unwrap();
    assert_eq!(TARGETS, 31);
    assert_eq!(SOURCES.len(), 4);
    for (source, name) in SOURCES.iter().enumerate() {
        for target in 0..TARGETS {
            let amount = description.param(matrix_param(source, target)).unwrap();
            assert_eq!((amount.min(), amount.max()), (-100.0, 100.0));
            assert_eq!(amount.default, 0.0);
            assert!(amount.name.starts_with(name), "{}", amount.name);
        }
    }
    let value = |id: ParamId| description.param(id).unwrap();
    assert_eq!(value(GLOBAL_MODULATION).default, 100.0);
    assert_eq!(value(FB_AMOUNT).max(), 100.0);
    assert_eq!(value(FB_NOTE).max(), 127.0);
    assert_eq!(value(FB_TIME).min(), 1.0);
    assert_eq!(value(FB_TIME).max(), 2_000.0);
    assert_eq!(value(COMPRESSION).default, 0.0);
    assert_eq!(value(lfo_param(0, LFO_RATE)).min(), 0.01);
    assert_eq!(value(lfo_param(1, LFO_RATE)).max(), 40.0);
    assert_eq!(value(lfo_param(0, LFO_SIXTEENTHS)).max(), 16.0);
    assert_eq!(value(ENV_ATTACK).min(), 0.1);
    assert_eq!(value(ENV_RELEASE).max(), 2_000.0);
    assert_eq!(value(ENV_THRESHOLD).min(), -60.0);
    assert_eq!(value(NOISE_RATE).max(), 40.0);
    // Every parameter has a descriptor number that is its place.
    for (i, param) in description.params.iter().enumerate() {
        assert_eq!(param.id.0 as usize, i);
    }
}

// Spec: roar R21, R23
#[test]
fn a_square_lfo_moves_a_stage_level_by_its_amount_of_the_range() {
    // 1 Hz: high for the first half second, then low. At 10 % of the level's
    // range (0 to +12 dB, 3.98) the gain is 1 plus and minus 0.398.
    let range = 10f64.powf(12.0 / 20.0);
    let params = with(
        clean(0),
        &[
            (lfo_param(0, LFO_WAVE), 2.0),
            (lfo_param(0, LFO_RATE), 1.0),
            matrix(0, p(0, LEVEL), 10.0),
        ],
    );
    let frames = 48_000;
    let input = sine(1_000.0, 0.1, frames);
    let out = render(&params, &Run::of(&input), 4_096);
    let high = peak(&out[0], 6_000, 18_000);
    let low = peak(&out[0], 30_000, 42_000);
    assert!((high / 0.1 - (1.0 + 0.1 * range)).abs() < 0.03, "{high}");
    assert!((low / 0.1 - (1.0 - 0.1 * range)).abs() < 0.03, "{low}");
    // Global Modulation Amount scales it.
    let half = with(params.clone(), &[(GLOBAL_MODULATION, 50.0)]);
    let out = render(&half, &Run::of(&input), 4_096);
    let high = peak(&out[0], 6_000, 18_000);
    assert!((high / 0.1 - (1.0 + 0.05 * range)).abs() < 0.03, "{high}");
}

// Spec: roar R21 to R23, R32, R33
#[test]
fn each_source_moves_its_targets_identically_at_any_block_size() {
    let frames = 24_000;
    let input = noise(frames);
    let run = Run::of(&input);
    for (source, name) in SOURCES.iter().enumerate() {
        let params = with(
            clean(0),
            &[
                (lfo_param(0, LFO_WAVE), 1.0),
                (lfo_param(0, LFO_RATE), 7.0),
                (lfo_param(0, LFO_SMOOTH), 20.0),
                (lfo_param(1, LFO_WAVE), 3.0),
                (lfo_param(1, LFO_MORPH), 30.0),
                (lfo_param(1, LFO_MODE), 4.0),
                (lfo_param(1, LFO_SIXTEENTHS), 3.0),
                (NOISE_TYPE, (source % 4) as f64),
                (NOISE_RATE, 12.0),
                (ENV_THRESHOLD, -50.0),
                (ENV_ATTACK, 1.0),
                (ENV_RELEASE, 20.0),
                (ENV_FREQUENCY, 800.0),
                (ENV_WIDTH, 2.0),
                (DRIVE, 3.0),
                matrix(source, DRIVE, 60.0),
                matrix(source, p(0, LEVEL), -40.0),
                (matrix_param(source, 0), 60.0),
            ],
        );
        let reference = render(&params, &run, 4_096);
        let plain = with(
            params.clone(),
            &[matrix(source, DRIVE, 0.0), matrix(source, p(0, LEVEL), 0.0)],
        );
        let unmoved = render(&plain, &run, 4_096);
        assert!(
            reference[0] != unmoved[0],
            "{name} moves its targets at all"
        );
        for block in BLOCKS {
            assert!(
                render(&params, &run, block) == reference,
                "{name} at block size {block}"
            );
        }
    }
}

// Spec: roar R21, R32
#[test]
fn a_synced_lfo_lands_on_the_beat() {
    // A square LFO a quarter note long at 120 BPM: a cycle is 24,000 frames,
    // high for the first half. Its edges are on the beat.
    let params = with(
        clean(0),
        &[
            (lfo_param(0, LFO_WAVE), 2.0),
            (lfo_param(0, LFO_MODE), 1.0),
            (lfo_param(0, LFO_SYNCED), 4.0),
            matrix(0, p(0, LEVEL), 10.0),
        ],
    );
    let frames = 72_000;
    let out = render(&params, &Run::of(&sine(1_000.0, 0.1, frames)), 512);
    // The loudness of a 96-frame window, two periods of the tone.
    let level = |at: usize| peak(&out[0], at, at + 96);
    let edge = |from: usize, to: usize, falling: bool| {
        let middle = 0.1;
        (from..to)
            .step_by(8)
            .find(|at| (level(*at + LATENCY) > middle) != falling)
            .expect("an edge")
    };
    for (from, to, falling, at) in [
        (11_000, 13_000, true, 12_000),
        (23_000, 25_000, false, 24_000),
        (35_000, 37_000, true, 36_000),
        (47_000, 49_000, false, 48_000),
    ] {
        let found = edge(from, to, falling);
        assert!(
            (found as i64 - at as i64).abs() < 200,
            "edge near {at}: {found}"
        );
    }
}

/// The complex Hann-windowed component of `samples[at..at + n]` at `hz`,
/// with the phase counted from the start of `samples`.
fn component(samples: &[f32], at: usize, n: usize, hz: f64) -> (f64, f64) {
    let (mut re, mut im) = (0.0, 0.0);
    for i in 0..n {
        let window = 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos();
        let phase = TAU * hz * (at + i) as f64 / RATE.as_f64();
        re += f64::from(samples[at + i]) * window * phase.cos();
        im -= f64::from(samples[at + i]) * window * phase.sin();
    }
    (re, im)
}

/// How many cents the ring of `samples` between `from` and `to` is above
/// `hz`: the phase its fundamental gains between the two halves of the
/// stretch against a tone at `hz`, which resolves a hundredth of a cent.
fn ring_cents(samples: &[f32], from: usize, to: usize, hz: f64) -> f64 {
    let half = (to - from) / 2;
    let (a, b) = (
        component(samples, from, half, hz),
        component(samples, from + half, half, hz),
    );
    let gained = (b.1.atan2(b.0) - a.1.atan2(a.0) + std::f64::consts::PI).rem_euclid(TAU)
        - std::f64::consts::PI;
    let off_hz = gained * RATE.as_f64() / (TAU * half as f64);
    1_200.0 * ((hz + off_hz) / hz).log2()
}

/// A Roar in Feedback mode, its stages clean, Note mode on `key` and its
/// band-pass centred on the pitch.
fn ringing(key: f64) -> Vec<(ParamId, f64)> {
    let hz = 440.0 * ((key - 69.0) / 12.0).exp2();
    with(
        with(clean(0), &clean(1)),
        &[
            (ROUTING_MODE, 5.0),
            (BLEND, 97.0),
            (FB_MODE, 4.0),
            (FB_NOTE, key),
            (FB_AMOUNT, 100.0),
            (FB_FREQUENCY, hz),
            (FB_WIDTH, 4.0),
        ],
    )
}

/// A small impulse: the loop builds up many times its size, and a ring that
/// reaches a Digital Clip's edges is no longer a measure of the loop's length.
fn impulse(frames: usize, at: usize) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    input[at] = 0.01;
    input
}

/// The stretch of a ring to measure, `periods` periods of `hz` long, a few
/// periods after `start`: the loop returns about 0.98 of what it carries
/// each time, so a ring is loud for a hundred trips and no more.
fn window(start: usize, hz: f64, periods: usize) -> (usize, usize) {
    let period = RATE.as_f64() / hz;
    let from = start + (5.0 * period) as usize;
    (from, from + (periods as f64 * period) as usize)
}

// Spec: roar R24
#[test]
fn note_mode_rings_at_the_keys_pitch_within_a_cent() {
    for key in (36..100).step_by(3).map(f64::from) {
        let hz = 440.0 * ((key - 69.0) / 12.0).exp2();
        let out = render(&ringing(key), &Run::of(&impulse(50_000, 0)), 4_096);
        let (from, to) = window(LATENCY, hz, 60);
        let cents = ring_cents(&out[0], from, to, hz);
        assert!(
            cents.abs() < 1.0,
            "key {key} ({hz:.1} Hz) is {cents:.2} cents out"
        );
    }
}

// Spec: roar R24, R32
#[test]
fn a_synced_feedback_time_lands_on_the_beat() {
    // A quarter note at 120 BPM is 24,000 frames.
    let params = with(
        with(clean(0), &clean(1)),
        &[
            (ROUTING_MODE, 5.0),
            (BLEND, 50.0),
            (FB_MODE, 1.0),
            (FB_SYNCED, 4.0),
            (FB_AMOUNT, 100.0),
            (FB_FREQUENCY, 1_000.0),
            (FB_WIDTH, 4.0),
        ],
    );
    let run = Run::of(&impulse(60_000, 0));
    let reference = render(&params, &run, 4_096);
    let (at, _) = reference[0]
        .iter()
        .enumerate()
        .skip(LATENCY + 20_000)
        .take(8_000)
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    assert!(
        (at as i64 - (LATENCY + 24_000) as i64).abs() < 12,
        "the first return lands at {at}"
    );
    for block in [1, 64, 512] {
        assert!(render(&params, &run, block) == reference, "block {block}");
    }
}

// Spec: roar R25
#[test]
fn a_note_from_a_midi_track_sets_the_pitch_and_a_note_off_leaves_it() {
    let hz = |key: f64| 440.0 * ((key - 69.0_f64) / 12.0).exp2();
    let mut params = ringing(57.0);
    params.push((MIDI_FB_NOTE, 1.0));
    // Impulses at the start and again before the note-off, so the ring is
    // there to measure; the note is on from 10,000 to 30,000.
    let mut input = impulse(48_000, 0);
    input[29_000] = 0.01;
    let mut run = Run::of(&input);
    let on = common::note_on(10_000, 69, 1.0, None);
    run.notes = vec![on, common::note_off(30_000, &on, 0.0)];
    let out = render(&params, &run, 512);
    let (from, to) = window(0, hz(57.0), 35);
    let before = ring_cents(&out[0], from, to, hz(57.0));
    assert!(before.abs() < 1.0, "before the note: {before}");
    let (from, to) = window(10_000, hz(69.0), 60);
    let after = ring_cents(&out[0], from, to, hz(69.0));
    assert!(after.abs() < 1.0, "from the note: {after}");
    // The note-off leaves the key: the ring from the second impulse is at
    // the note's pitch.
    let (from, to) = window(30_000, hz(69.0), 60);
    let left = ring_cents(&out[0], from, to, hz(69.0));
    assert!(left.abs() < 1.0, "after the note-off: {left}");
    for block in [1, 64, 4_096] {
        assert!(render(&params, &run, block) == out, "block {block}");
    }
    // With the switch off the note changes nothing.
    params.pop();
    let ignored = render(&params, &run, 512);
    let (from, to) = window(10_000, hz(57.0), 35);
    let cents = ring_cents(&ignored[0], from, to, hz(57.0));
    assert!(cents.abs() < 1.0, "off: {cents}");
}

// Spec: roar R27
#[test]
fn the_compressors_gain_reduction_on_a_step_is_within_half_a_decibel_of_its_law() {
    // A step to -6 dBFS with both stages off: at Amount 100 % the threshold
    // is -30 dBFS and the ratio 8 : 1, so 24 dB over takes off 21 dB.
    for (amount, level_db) in [(100.0, -6.0), (50.0, -6.0), (100.0, -20.0)] {
        let level = 10f64.powf(level_db / 20.0);
        let params = vec![(p(0, ON), 0.0), (p(1, ON), 0.0), (COMPRESSION, amount)];
        let frames = 24_000;
        let input = vec![level as f32; frames];
        let out = render(&params, &Run::of(&input), 4_096);
        let law = {
            let amount = amount / 100.0;
            let (threshold, slope) = (-30.0 * amount, 1.0 - 1.0 / (1.0 + 7.0 * amount));
            -slope * (level_db - threshold).max(0.0)
        };
        let now = 20.0 * (f64::from(out[0][frames - 1]) / level).log10();
        assert!(
            (now - law).abs() < 0.5,
            "{amount} % at {level_db} dBFS: {now:.2} dB against {law:.2}"
        );
    }
}

// Spec: roar R27
#[test]
fn the_sidechain_high_pass_keeps_lows_from_squashing_the_rest() {
    // A loud 60 Hz tone with a quiet 2 kHz one: with the high-pass on the
    // detector hears less of the 60 Hz, so the device reduces less.
    let frames = 24_000;
    let input: Vec<f32> = sine(60.0, 0.5, frames)
        .iter()
        .zip(sine(2_000.0, 0.02, frames))
        .map(|(a, b)| a + b)
        .collect();
    let loud = |high_pass: f64| {
        let params = vec![
            (p(0, ON), 0.0),
            (p(1, ON), 0.0),
            (COMPRESSION, 100.0),
            (SIDECHAIN_HP, high_pass),
        ];
        let out = render(&params, &Run::of(&input), 4_096);
        peak(&out[0], 12_000, 24_000)
    };
    assert!(loud(1.0) > loud(0.0) * 1.2);
}

// Spec: roar R22, R28, R29
#[test]
fn the_external_sidechain_drives_env() {
    // Env moves stage 1's level up by 25 % of its range. The input is
    // quiet, below Env's threshold; the sidechain is a loud tone at its
    // frequency. With the sidechain on, Env opens the level.
    let frames = 24_000;
    let params = with(
        clean(0),
        &[
            (ENV_THRESHOLD, -30.0),
            (ENV_FREQUENCY, 1_000.0),
            (ENV_WIDTH, 2.0),
            (ENV_ATTACK, 1.0),
            matrix(2, p(0, LEVEL), 25.0),
        ],
    );
    let input = sine(1_000.0, 0.01, frames);
    let quiet = render(&params, &Run::of(&input), 512);
    let mut keyed = Run::of(&input);
    keyed.key = Some(sine(1_000.0, 0.5, frames));
    let driven = render(&params, &keyed, 512);
    let quiet = peak(&quiet[0], 12_000, 24_000);
    let driven = peak(&driven[0], 12_000, 24_000);
    assert!(quiet < 0.0105, "the quiet input leaves Env at 0: {quiet}");
    assert!(driven > 0.015, "the key opens it: {driven}");
    // With nothing in the sidechain Env follows the input, as the routing
    // spec says; a loud input moves it.
    let loud = render(&params, &Run::of(&sine(1_000.0, 0.5, frames)), 512);
    assert!(peak(&loud[0], 12_000, 24_000) > 0.5 * 1.5);
}

// Spec: roar R22
#[test]
fn input_listen_sends_what_envs_detector_hears_to_the_output() {
    let frames = 4_800;
    let input = sine(1_000.0, 0.25, frames);
    let params = with(
        clean(0),
        &[
            (ENV_FREQUENCY, 1_000.0),
            (ENV_WIDTH, 4.0),
            (INPUT_LISTEN, 1.0),
            (OUTPUT_GAIN, 1.0),
        ],
    );
    let heard = render(&params, &Run::of(&input), 512);
    let normal = render(
        &with(params.clone(), &[(INPUT_LISTEN, 0.0)]),
        &Run::of(&input),
        512,
    );
    // Listen plays the detector's signal in time with the input, with no
    // latency; the band-pass at 1 kHz leaves a 1 kHz tone at its level.
    let level = peak(&heard[0], 2_400, 4_800);
    assert!((level - 0.25).abs() < 0.02, "{level}");
    assert!(heard[0] != normal[0]);
}

// Spec: roar R32, R33, R35
#[test]
fn part_twos_renders_are_bit_identical_at_four_block_sizes_and_run_after_run() {
    let frames = 8_192;
    let left = noise(frames);
    let right: Vec<f32> = left.iter().rev().map(|s| s * 0.7).collect();
    let mut run = Run {
        left,
        right,
        key: Some(noise(frames).iter().map(|s| s * 0.4).collect()),
        notes: vec![
            common::note_on(2_000, 60, 1.0, None),
            common::note_on(4_000, 72, 1.0, None),
        ],
        events: vec![
            set(3_000, FB_AMOUNT, 70.0),
            set(5_000, lfo_param(0, LFO_RATE), 9.0),
            set(6_000, matrix_param(1, 3), -30.0),
        ],
    };
    run.notes.push(common::note_off(6_500, &run.notes[1], 0.0));
    let base = with(
        with(clean(0), &clean(1)),
        &[
            (BLEND, 40.0),
            (FB_MODE, 4.0),
            (FB_NOTE, 52.0),
            (FB_AMOUNT, 60.0),
            (MIDI_FB_NOTE, 1.0),
            (COMPRESSION, 40.0),
            (SIDECHAIN_HP, 1.0),
            (lfo_param(0, LFO_WAVE), 1.0),
            (lfo_param(0, LFO_RATE), 5.0),
            (lfo_param(1, LFO_MODE), 1.0),
            (NOISE_TYPE, 0.0),
            (ENV_THRESHOLD, -40.0),
            matrix(0, DRIVE, 50.0),
            matrix(1, p(0, LEVEL), -20.0),
            matrix(2, FB_AMOUNT, 30.0),
            matrix(3, FB_FREQUENCY, 25.0),
            matrix(0, FB_TIME, 10.0),
        ],
    );
    for (mode, name) in MODES.iter().enumerate() {
        let params = with(base.clone(), &[(ROUTING_MODE, mode as f64)]);
        let reference = render(&params, &run, 4_096);
        assert!(
            render(&params, &run, 4_096) == reference,
            "{name}: run after run"
        );
        for block in [1, 64, 512] {
            assert!(
                render(&params, &run, block) == reference,
                "{name} at block size {block}"
            );
        }
        assert!(
            reference[0].iter().all(|s| s.is_finite()),
            "{name} is finite"
        );
        assert!(reference[0].iter().any(|s| *s != 0.0), "{name} makes sound");
    }
}

// Spec: roar R31
#[test]
fn each_source_publishes_its_last_two_seconds_to_device_display() {
    // LFO 1 at 2 Hz fills the 64 points of its trace with four cycles.
    let params = with(clean(0), &[(lfo_param(0, LFO_RATE), 2.0)]);
    let device_type = roar_type();
    let mut node = node(&registry(), &device(&device_type, &params), 4_096);
    let mut reader = node.display().expect("the device has a display");
    let input = noise(4_096);
    for _ in 0..30 {
        common::run(node.as_mut(), &input, &[]);
    }
    let data = reader.read();
    let id = bl_model::DeviceId(bl_core::id::Id::new(1).unwrap());
    let mut set = bl_model::OrdMap::new();
    for (param, value) in &params {
        set.insert(*param, *value);
    }
    let shown = bl_devices::display_for_view(
        bl_devices::roar::ROAR,
        id,
        &device_type.description,
        &set,
        Some(&data.values),
        Some("Stage 1"),
    )
    .expect("Roar draws a display");
    for name in SOURCES {
        let trace = shown
            .series(name)
            .unwrap_or_else(|| panic!("{name}'s trace"));
        assert_eq!(trace.values.len(), 64);
    }
    let lfo = shown.series("LFO 1").unwrap();
    let (low, high) = lfo
        .values
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
    assert!(low < -0.8 && high > 0.8, "a full cycle: {low} to {high}");
    assert_eq!(lfo.x_range, [-2.0, 0.0]);
    assert!(
        shown
            .series("Env")
            .unwrap()
            .values
            .iter()
            .all(|v| (0.0..=1.0).contains(v))
    );
}
