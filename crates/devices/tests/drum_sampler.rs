//! Strike, the one-shot drum sampler (WP-5.18; the one-shot drum sampler
//! spec): the region and the pitch, the Trigger and Gate envelopes, each of
//! the nine playback effects measured, the filter's types, the global
//! controls, the modulation route, per-note bend and slide, and a render
//! that is the same at any block size. Every block runs as a real-time
//! section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::f64::consts::TAU;
use std::sync::Arc;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_devices::drum_sampler::{
    ATTACK, DECAY, DETUNE, ENV_FOLLOWS_PITCH, ENV_MODE, FILTER_FREQ, FILTER_GAIN, FILTER_ON,
    FILTER_RES, FILTER_TYPE, FX_ON, FX_TYPE, FX1, FX2, HOLD, HOLD_INF, LENGTH, MOD_AMOUNT,
    MOD_DEST, MOD_SOURCE, PAN, PER_NOTE_BEND, SAMPLE_GAIN, START, TRANSPOSE, VELOCITY_TO_VOLUME,
    VOLUME, drum_sampler_type,
};
use bl_engine::{
    Block, Collector, DeviceType, EngineConfig, MediaMap, Node, NodeEvent, NoteBuffer, NoteEvent,
    NoteExpression, NoteKind, Offline, Registry, TransportCommand, TransportInfo,
};
use bl_model::param::db_to_gain;
use bl_model::{
    Clip, ClipData, Command, Device, DeviceRole, Editor, MidiData, Note, ParamId, TrackId,
    TrackKind,
};
use common::{CHANNELS, RATE, device, node, set};

/// The sample rate the tests run at, in hertz.
const HZ: f64 = 48_000.0;

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 100, 512, 4096];

/// The effects' chooser indexes.
const STRETCH: f64 = 0.0;
const LOOP: f64 = 1.0;
const PITCH_ENV: f64 = 2.0;
const PUNCH: f64 = 3.0;
const EIGHT_BIT: f64 = 4.0;
const FM: f64 = 5.0;
const RING_MOD: f64 = 6.0;
const SUB_OSC: f64 = 7.0;
const NOISE_OSC: f64 = 8.0;

/// The modulation destinations' chooser indexes.
const TO_FILTER: f64 = 0.0;
const TO_ATTACK: f64 = 1.0;
const TO_HOLD: f64 = 2.0;
const TO_DECAY: f64 = 3.0;
const TO_FX1: f64 = 4.0;
const TO_FX2: f64 = 5.0;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::drum_sampler::register(&mut registry);
    registry
}

/// Strike of `params`, up to 4,096 frames at a time.
fn strike(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let device_type: DeviceType = drum_sampler_type();
    node(&registry(), &device(&device_type, params), 4096)
}

/// Decoded audio of `frames` frames at the engine's rate, one channel.
fn sample(frames: usize, value: impl Fn(usize) -> f32) -> Arc<bl_media::SampleData> {
    Arc::new(bl_media::SampleData::new(RATE.hz(), vec![(0..frames).map(value).collect()]).unwrap())
}

/// A sine of `hz` and `amplitude`.
fn sine(frames: usize, hz: f64, amplitude: f64) -> Arc<bl_media::SampleData> {
    sample(frames, |i| {
        (amplitude * (TAU * hz * i as f64 / HZ).sin()) as f32
    })
}

/// Strike with the sample, through the collector the engine would.
fn loaded(
    params: &[(ParamId, f64)],
    data: &Arc<bl_media::SampleData>,
) -> (Box<dyn Node>, Collector) {
    let collector = Collector::new();
    let mut node = strike(params);
    node.receive_data(collector.handle().data(Arc::clone(data)));
    (node, collector)
}

fn on(offset: u32, key: u8, velocity: f32) -> NoteEvent {
    note(offset, NoteKind::On, key, velocity, None)
}

fn off(offset: u32, key: u8) -> NoteEvent {
    note(offset, NoteKind::Off, key, 0.0, None)
}

fn note(offset: u32, kind: NoteKind, key: u8, velocity: f32, id: Option<u32>) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel: 0,
        key,
        velocity,
        note_id: id,
    }
}

fn expression(
    offset: u32,
    expression: NoteExpression,
    value: f32,
    key: u8,
    id: Option<u32>,
) -> NoteEvent {
    note(
        offset,
        NoteKind::Expression { expression, value },
        key,
        0.0,
        id,
    )
}

/// Plays `total` frames in blocks of `block`, with the notes and parameter
/// changes at their absolute frames, each block a real-time section, and
/// returns each channel.
fn render(
    node: &mut dyn Node,
    total: usize,
    block: usize,
    notes: &[(usize, NoteEvent)],
    events: &[(usize, NodeEvent)],
) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(total)).collect();
    let mut start = 0;
    while start < total {
        let frames = block.min(total - start);
        let end = start + frames;
        let in_block = |at: usize| at >= start && at < end;
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|(at, _)| in_block(*at))
            .map(|(at, note)| NoteEvent {
                offset: (*at - start) as u32,
                ..*note
            })
            .collect();
        let block_events: Vec<NodeEvent> = events
            .iter()
            .filter(|(at, _)| in_block(*at))
            .map(|(at, event)| NodeEvent {
                offset: (*at - start) as u32,
                ..*event
            })
            .collect();
        let mut data = vec![0.0f32; CHANNELS * frames];
        let mut sent = NoteBuffer::with_capacity(block_notes.len().max(1));
        {
            let mut block = Block::new(
                &mut data,
                CHANNELS,
                frames,
                frames,
                &block_events,
                TransportInfo::default(),
                RATE,
            )
            .with_notes(&block_notes, &mut sent);
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (channel, samples) in data.chunks(frames).enumerate() {
            out[channel].extend_from_slice(samples);
        }
        start = end;
    }
    out
}

/// One block of `frames` frames with `notes` at their offsets.
fn play(
    params: &[(ParamId, f64)],
    data: &Arc<bl_media::SampleData>,
    frames: usize,
    notes: &[(usize, NoteEvent)],
) -> Vec<Vec<f32>> {
    let (mut node, _collector) = loaded(params, data);
    render(node.as_mut(), frames, frames.min(4096), notes, &[])
}

/// The first frame of `samples` where `wanted` holds, or the end.
fn first(samples: &[f32], wanted: impl Fn(&f32) -> bool) -> usize {
    samples.iter().position(wanted).unwrap_or(samples.len())
}

/// The amplitude of `hz` in `signal`, by a windowed DFT.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let phase = TAU * hz * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
    }
    4.0 * re.hypot(im) / n
}

fn db(gain: f64) -> f64 {
    20.0 * gain.log10()
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

// Spec: one-shot-drum-sampler R1
#[test]
fn strike_is_described_laid_out_and_lists_its_two_menu_options() {
    let device_type = drum_sampler_type();
    assert_eq!(device_type.name, "Strike");
    assert_eq!(device_type.role, DeviceRole::Instrument);
    device_type.description.check().unwrap();
    // The device reads its parameters by their numbers, in order.
    for (index, param) in device_type.description.params.iter().enumerate() {
        assert_eq!(param.id.0 as usize, index, "{}", param.name);
    }
    assert_eq!(
        device_type.description.menu_options,
        [PER_NOTE_BEND, ENV_FOLLOWS_PITCH]
    );
    // Per-note pitch bend is on in a new device, Envelope Follows Pitch off.
    let param = |id| device_type.description.param(id).unwrap();
    assert_eq!(param(PER_NOTE_BEND).default, 1.0);
    assert_eq!(param(ENV_FOLLOWS_PITCH).default, 0.0);
}

// Spec: one-shot-drum-sampler R2
#[test]
fn a_note_plays_the_region_start_and_length_give_at_its_pitch() {
    let impulse = sample(4800, |i| if i == 1000 { 1.0 } else { 0.0 });
    let at_c3 = play(&[], &impulse, 4800, &[(0, on(0, 60, 1.0))]);
    assert_eq!(first(&at_c3[0], |s| s.abs() > 0.9), 1000, "C3 plays at 1x");
    let at_c4 = play(&[], &impulse, 4800, &[(0, on(0, 72, 1.0))]);
    assert_eq!(first(&at_c4[0], |s| s.abs() > 0.9), 500, "an octave up, 2x");
    let transposed = play(&[(TRANSPOSE, 12.0)], &impulse, 4800, &[(0, on(0, 60, 1.0))]);
    assert_eq!(first(&transposed[0], |s| s.abs() > 0.9), 500, "Transpose");
    let detuned = play(&[(DETUNE, 50.0)], &impulse, 4800, &[(0, on(0, 60, 1.0))]);
    let wanted = 1000.0 / 2f64.powf(0.5 / 12.0);
    let got = first(&detuned[0], |s| s.abs() > 0.4) as f64;
    assert!((got - wanted).abs() <= 1.0, "Detune: {got} for {wanted}");

    // 25 % of 4,800 is 1,200: the voice plays frames 1,200 to 2,400.
    let whole = sample(4800, |_| 1.0);
    let region = play(
        &[(START, 25.0), (LENGTH, 25.0)],
        &whole,
        4800,
        &[(0, on(0, 60, 1.0))],
    );
    assert!(region[0][..1200].iter().all(|s| (s - 1.0).abs() < 1e-6));
    assert!(region[0][1200..].iter().all(|s| *s == 0.0));
    // No sample, no sound.
    let mut empty = strike(&[]);
    let silent = render(empty.as_mut(), 512, 512, &[(0, on(0, 60, 1.0))], &[]);
    assert!(silent.iter().flatten().all(|s| *s == 0.0));
}

// Spec: one-shot-drum-sampler R3
#[test]
fn sample_gain_and_volume_reach_minus_infinity_from_their_tops() {
    let flat = sample(4800, |_| 1.0);
    let notes = [(0, on(0, 60, 1.0))];
    for param in [SAMPLE_GAIN, VOLUME] {
        let shut = play(&[(param, 0.0)], &flat, 4800, &notes);
        assert!(shut.iter().flatten().all(|s| *s == 0.0), "{param:?}");
    }
    let device_type = drum_sampler_type();
    let top = |id| device_type.description.param(id).unwrap().max();
    assert!((db(top(SAMPLE_GAIN)) - 24.0).abs() < 1e-6);
    assert!((db(top(VOLUME)) - 36.0).abs() < 1e-6);
    // Volume 6 dB down, Sample Gain 6 dB up.
    let quieter = play(&[(VOLUME, db_to_gain(-6.0))], &flat, 4800, &notes);
    assert!((quieter[0][1000] - db_to_gain(-6.0) as f32).abs() < 0.01);
    let louder = play(&[(SAMPLE_GAIN, db_to_gain(6.0))], &flat, 4800, &notes);
    assert!((louder[0][1000] - db_to_gain(6.0) as f32).abs() < 0.01);
}

// Spec: one-shot-drum-sampler R4
#[test]
fn trigger_mode_attacks_holds_and_decays_and_ignores_the_notes_end() {
    let flat = sample(48_000, |_| 1.0);
    let params = [
        (HOLD_INF, 0.0),
        (ATTACK, 100.0),
        (HOLD, 100.0),
        (DECAY, 100.0),
    ];
    let notes = [(0, on(0, 60, 1.0)), (1000, off(1000, 60))];
    let out = play(&params, &flat, 24_000, &notes);
    let left = &out[0];
    // 100 ms of attack at 48 kHz: half way at frame 2,400.
    assert!((left[2400] - 0.5).abs() < 0.01, "attack: {}", left[2400]);
    // 100 ms of hold from the peak at 4,800, whatever the note did.
    assert!((left[7000] - 1.0).abs() < 1e-6, "hold: {}", left[7000]);
    // 100 ms of decay from 9,600 to silence at 14,400.
    assert!((left[12_000] - 0.5).abs() < 0.01, "decay: {}", left[12_000]);
    let silent = first(&left[9600..], |s| *s == 0.0) + 9600;
    assert!((silent as f64 - 14_400.0).abs() <= 2.0, "end: {silent}");
}

// Spec: one-shot-drum-sampler R4
#[test]
fn hold_at_infinity_plays_the_whole_sample_to_its_end() {
    let flat = sample(24_000, |_| 1.0);
    // Decay is short and the note short, but Hold Inf plays on.
    let out = play(
        &[(DECAY, 10.0)],
        &flat,
        48_000,
        &[(0, on(0, 60, 1.0)), (10, off(10, 60))],
    );
    assert!(out[0][..24_000].iter().all(|s| (s - 1.0).abs() < 1e-6));
    assert!(out[0][24_000..].iter().all(|s| *s == 0.0));
}

// Spec: one-shot-drum-sampler R5
#[test]
fn gate_mode_decays_from_the_notes_end_and_ignores_hold() {
    let flat = sample(48_000, |_| 1.0);
    let params = [
        (ENV_MODE, 1.0),
        (HOLD, 50.0),
        (HOLD_INF, 0.0),
        (DECAY, 100.0),
    ];
    let notes = [(0, on(0, 60, 1.0)), (20_000, off(20_000, 60))];
    let out = play(&params, &flat, 40_000, &notes);
    let left = &out[0];
    // Held past Hold's 50 ms until the note ends, then 100 ms to silence.
    assert!((left[19_990] - 1.0).abs() < 1e-6);
    assert!((left[22_400] - 0.5).abs() < 0.01, "decay: {}", left[22_400]);
    let silent = first(&left[20_000..], |s| *s == 0.0) + 20_000;
    assert!((silent as f64 - 24_800.0).abs() <= 2.0, "end: {silent}");
}

// Spec: one-shot-drum-sampler R6
#[test]
fn envelope_follows_pitch_scales_the_times_with_the_pitch() {
    let flat = sample(48_000, |_| 1.0);
    let params = |follows| {
        [
            (HOLD_INF, 0.0),
            (HOLD, 0.0),
            (DECAY, 200.0),
            (ENV_FOLLOWS_PITCH, follows),
        ]
    };
    let notes = [(0, on(0, 72, 1.0))];
    // An octave up, the 200 ms decay is 100 ms when it follows the pitch.
    let plain = play(&params(0.0), &flat, 24_000, &notes);
    let follows = play(&params(1.0), &flat, 24_000, &notes);
    let end = |out: &[Vec<f32>]| first(&out[0], |s| *s == 0.0) as f64;
    assert!((end(&plain) - 9600.0).abs() <= 2.0, "{}", end(&plain));
    assert!((end(&follows) - 4800.0).abs() <= 2.0, "{}", end(&follows));
}

// Spec: one-shot-drum-sampler R7
#[test]
fn stretch_lengthens_the_sample_without_changing_its_pitch() {
    let tone = sine(24_000, 1000.0, 0.5);
    // Factor 2 and a 40 ms grain.
    let fx1 = (2.0f64 / 0.25).ln() / 16f64.ln() * 100.0;
    let fx2 = (40.0f64 / 10.0).ln() / 20f64.ln() * 100.0;
    let stretched = play(
        &[(FX_ON, 1.0), (FX_TYPE, STRETCH), (FX1, fx1), (FX2, fx2)],
        &tone,
        64_000,
        &[(0, on(0, 60, 1.0))],
    );
    let left = &stretched[0];
    // It lasts about twice as long, ...
    let end = left.iter().rposition(|s| s.abs() > 0.01).unwrap();
    assert!((end as f64 - 48_000.0).abs() < 2000.0, "ends at {end}");
    // ... in the same pitch, at the same level.
    let window = &left[30_000..34_096];
    assert!((amplitude_at(window, 1000.0) - 0.5).abs() < 0.05);
    assert!(amplitude_at(window, 500.0) < 0.02);
    // At a factor of 1 the grains cross-fade into the sample unchanged.
    let plain = play(&[], &tone, 24_000, &[(0, on(0, 60, 1.0))]);
    let unity = play(
        &[(FX_ON, 1.0), (FX_TYPE, STRETCH), (FX1, 50.0)],
        &tone,
        24_000,
        &[(0, on(0, 60, 1.0))],
    );
    let worst = plain[0][..23_000]
        .iter()
        .zip(&unity[0])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 0.002, "unity stretch differs by {worst}");
}

// Spec: one-shot-drum-sampler R8
#[test]
fn loop_repeats_the_chosen_stretch_of_the_sample() {
    let ramp = sample(48_000, |i| i as f32 / 48_000.0);
    // Loop start at the region's start, 100 ms long.
    let fx2 = (100.0f64 / 5.0).ln() / 400f64.ln() * 100.0;
    let out = play(
        &[(FX_ON, 1.0), (FX_TYPE, LOOP), (FX1, 0.0), (FX2, fx2)],
        &ramp,
        60_000,
        &[(0, on(0, 60, 1.0))],
    );
    let left = &out[0];
    for t in [10usize, 1000, 4000] {
        assert!((left[t] - left[t + 4800]).abs() < 1e-4, "{t}");
        assert!((left[t] - left[t + 9600]).abs() < 1e-4, "{t}");
    }
    // Hold Inf lasts the sample's length, not forever.
    assert!(left[47_900].abs() > 0.0);
    assert!(left[48_100..].iter().all(|s| *s == 0.0));
}

// Spec: one-shot-drum-sampler R9
#[test]
fn pitch_env_bends_the_pitch_by_its_amount_and_brings_it_back() {
    // A ramp reads out its own position: its slope is the playback rate.
    let ramp = sample(48_000, |i| i as f32 / 480_000.0);
    // +12 semitones (amount 0.5), back to the base over 100 ms.
    let fx2 = (100.0f64 / 10.0).ln() / 200f64.ln() * 100.0;
    let out = play(
        &[(FX_ON, 1.0), (FX_TYPE, PITCH_ENV), (FX1, 75.0), (FX2, fx2)],
        &ramp,
        24_000,
        &[(0, on(0, 60, 1.0))],
    );
    let left = &out[0];
    let cents = |at: usize| {
        let rate = f64::from(left[at + 50] - left[at]) * 480_000.0 / 50.0;
        1200.0 * rate.log2()
    };
    for (at, wanted) in [
        (10, 1200.0 * (1.0 - 10.0 / 4800.0)),
        (2400, 600.0),
        (4700, 25.0),
    ] {
        assert!(
            (cents(at) - wanted).abs() < 15.0,
            "{at}: {} for {wanted}",
            cents(at)
        );
    }
    assert!(cents(8000).abs() < 5.0, "back to the base pitch");
}

// Spec: one-shot-drum-sampler R10
#[test]
fn punch_ducks_the_sound_after_its_transient_and_releases() {
    let flat = sample(48_000, |_| 1.0);
    // 12 dB of ducking returning over 200 ms.
    let out = play(
        &[(FX_ON, 1.0), (FX_TYPE, PUNCH), (FX1, 50.0), (FX2, 50.0)],
        &flat,
        24_000,
        &[(0, on(0, 60, 1.0))],
    );
    let left = &out[0];
    assert!((left[100] - 1.0).abs() < 1e-6, "the transient passes");
    // The duck is fully in 1 ms after the 5 ms transient (frame 288).
    for (at, wanted) in [(300, -11.94), (5 * 48 + 4800, -6.0), (5 * 48 + 9000, -0.75)] {
        let got = db(f64::from(left[at]));
        assert!((got - wanted).abs() < 0.2, "{at}: {got} dB for {wanted}");
    }
    assert!((left[11_000] - 1.0).abs() < 1e-6, "released");
}

// Spec: one-shot-drum-sampler R11
#[test]
fn eight_bit_holds_the_sample_at_its_rate_and_crushes_it() {
    let tone = sine(24_000, 1000.0, 0.8);
    for hz in [8000.0f64, 12_000.0] {
        let fx1 = (hz / 500.0).ln() / 88f64.ln() * 100.0;
        let out = play(
            &[(FX_ON, 1.0), (FX_TYPE, EIGHT_BIT), (FX1, fx1), (FX2, 0.0)],
            &tone,
            24_000,
            &[(0, on(0, 60, 1.0))],
        );
        // Holding at `hz` folds a 1 kHz tone into images at hz ± 1 kHz.
        let window = &out[0][4000..12_192];
        let image = amplitude_at(window, hz - 1000.0);
        assert!(image > 0.02, "{hz} Hz: image {image}");
        assert!(amplitude_at(window, hz - 2500.0) < image / 5.0, "{hz} Hz");
    }
    let clean = play(&[], &tone, 24_000, &[(0, on(0, 60, 1.0))]);
    assert!(amplitude_at(&clean[0][4000..12_192], 7000.0) < 0.001);
}

// Spec: one-shot-drum-sampler R12
#[test]
fn fm_puts_sidebands_at_the_modulators_frequency_and_fades_with_decay() {
    let tone = sine(48_000, 2000.0, 0.5);
    let tone_1k = sine(48_000, 1000.0, 0.5);
    // A 500 Hz modulator at C3, a light depth, a long decay.
    let fx2 = (500.0f64 / 20.0).ln() / 250f64.ln() * 100.0;
    let params = [
        (FX_ON, 1.0),
        (FX_TYPE, FM),
        (FX1, 5.0),
        (FX2, fx2),
        (DECAY, 5000.0),
    ];
    let out = play(&params, &tone, 24_000, &[(0, on(0, 60, 1.0))]);
    let window = &out[0][..8192];
    let carrier = amplitude_at(window, 2000.0);
    for sideband in [1500.0, 2500.0] {
        assert!(amplitude_at(window, sideband) > carrier * 0.1, "{sideband}");
    }
    assert!(amplitude_at(window, 2250.0) < carrier * 0.03);
    // The modulation follows the played pitch: an octave up, a 1 kHz
    // modulator.
    let up = play(&params, &tone_1k, 24_000, &[(0, on(0, 72, 1.0))]);
    assert!(amplitude_at(&up[0][..8192], 3000.0) > 0.03);
    assert!(amplitude_at(&up[0][..8192], 2500.0) < 0.01);
    // Decay confines it to the transient.
    let short = play(
        &[
            (FX_ON, 1.0),
            (FX_TYPE, FM),
            (FX1, 5.0),
            (FX2, fx2),
            (DECAY, 20.0),
        ],
        &tone,
        24_000,
        &[(0, on(0, 60, 1.0))],
    );
    assert!(amplitude_at(&short[0][4000..12_192], 2500.0) < 0.002);
}

// Spec: one-shot-drum-sampler R13
#[test]
fn ring_mod_puts_sidebands_at_the_sum_and_difference_frequencies() {
    let tone = sine(48_000, 1000.0, 0.8);
    // A carrier of 20 Hz * 250^0.5 = 316.2 Hz at full amount.
    let carrier = 20.0 * 250f64.sqrt();
    let out = play(
        &[
            (FX_ON, 1.0),
            (FX_TYPE, RING_MOD),
            (FX1, 100.0),
            (FX2, 50.0),
            (DECAY, 5000.0),
        ],
        &tone,
        24_000,
        &[(0, on(0, 60, 1.0))],
    );
    let window = &out[0][..8192];
    for sideband in [1000.0 - carrier, 1000.0 + carrier] {
        let got = amplitude_at(window, sideband);
        assert!((got - 0.4).abs() < 0.06, "{sideband}: {got}");
    }
    assert!(
        amplitude_at(window, 1000.0) < 0.1,
        "the tone itself is gone"
    );
}

// Spec: one-shot-drum-sampler R14
#[test]
fn sub_osc_adds_a_sine_that_follows_the_pitch_and_the_envelope() {
    let blip = sample(480, |_| 0.0);
    let params = [
        (FX_ON, 1.0),
        (FX_TYPE, SUB_OSC),
        (FX1, 50.0),
        (FX2, 50.0),
        (HOLD_INF, 0.0),
        (HOLD, 500.0),
        (ATTACK, 100.0),
    ];
    let c3 = play(&params, &blip, 24_000, &[(0, on(0, 60, 1.0))]);
    // 30 + 90 * 0.5 = 75 Hz at C3, half the full level.
    let window = &c3[0][8000..16_192];
    assert!((amplitude_at(window, 75.0) - 0.5).abs() < 0.06);
    assert!(amplitude_at(window, 110.0) < 0.05);
    // The attack shapes it: quiet in the first 20 ms.
    assert!(c3[0][..960].iter().map(|s| s.abs()).fold(0.0, f32::max) < 0.12);
    let c4 = play(&params, &blip, 24_000, &[(0, on(0, 72, 1.0))]);
    assert!((amplitude_at(&c4[0][8000..16_192], 150.0) - 0.5).abs() < 0.06);
}

// Spec: one-shot-drum-sampler R15
#[test]
fn noise_osc_adds_noise_whose_colour_follows_fx2() {
    let blip = sample(480, |_| 0.0);
    let noise = |fx2| {
        play(
            &[
                (FX_ON, 1.0),
                (FX_TYPE, NOISE_OSC),
                (FX1, 100.0),
                (FX2, fx2),
                (HOLD_INF, 0.0),
                (HOLD, 500.0),
            ],
            &blip,
            12_000,
            &[(0, on(0, 60, 1.0))],
        )
    };
    let (dark, bright) = (noise(0.0), noise(100.0));
    let bright_rms = rms(&bright[0][2000..10_000]);
    assert!(bright_rms > 0.05, "the noise sounds: {bright_rms}");
    let roughness = |out: &[Vec<f32>]| {
        let diff: Vec<f32> = out[0][2000..10_000]
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();
        rms(&diff) / rms(&out[0][2000..10_000])
    };
    assert!(roughness(&dark) < roughness(&bright) / 3.0);
    // A new note repeats the run: the noise is the same each time.
    assert_eq!(noise(100.0), bright);
}

/// A reference low-pass, high-pass or peak section's magnitude at `hz`:
/// the audio-EQ cookbook's biquads.
fn section(kind: &str, centre: f64, q: f64, gain_db: f64, hz: f64) -> f64 {
    let w0 = TAU * centre / HZ;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q);
    let a = 10f64.powf(gain_db / 40.0);
    let (b0, b1, b2, a0, a1, a2) = match kind {
        "lp" => (
            (1.0 - cos) / 2.0,
            1.0 - cos,
            (1.0 - cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        ),
        "hp" => (
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        ),
        _ => (
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        ),
    };
    let w = TAU * hz / HZ;
    let z = |k: f64| (f64::cos(-k * w), f64::sin(-k * w));
    let (z1, z2) = (z(1.0), z(2.0));
    let top = (b0 + b1 * z1.0 + b2 * z2.0, b1 * z1.1 + b2 * z2.1);
    let bottom = (a0 + a1 * z1.0 + a2 * z2.0, a1 * z1.1 + a2 * z2.1);
    top.0.hypot(top.1) / bottom.0.hypot(bottom.1)
}

// Spec: one-shot-drum-sampler R16
#[test]
fn the_filters_types_match_their_prototypes_within_point_two_db() {
    use std::f64::consts::FRAC_1_SQRT_2 as BUTTERWORTH;
    let resonance = 40.0;
    let q = BUTTERWORTH + 11.3 * resonance / 100.0;
    // The cascades of cookbook sections each type is.
    let prototype = |kind: u32, hz: f64| match kind {
        0 => section("lp", 1000.0, q, 0.0, hz),
        1 => section("lp", 1000.0, q, 0.0, hz) * section("lp", 1000.0, BUTTERWORTH, 0.0, hz),
        2 => section("hp", 1000.0, q, 0.0, hz) * section("hp", 1000.0, BUTTERWORTH, 0.0, hz),
        _ => section("peak", 1000.0, 1.0, 12.0, hz),
    };
    let cases = [
        (0, "low-pass 12"),
        (1, "low-pass 24"),
        (2, "high-pass 24"),
        (3, "peak"),
    ];
    for (kind, name) in cases {
        for hz in [250.0, 900.0, 1000.0, 1500.0, 4000.0] {
            let tone = sine(16_384, hz, 0.5);
            let out = play(
                &[
                    (FILTER_ON, 1.0),
                    (FILTER_TYPE, f64::from(kind)),
                    (FILTER_FREQ, 1000.0),
                    (FILTER_RES, resonance),
                    (FILTER_GAIN, 12.0),
                ],
                &tone,
                16_384,
                &[(0, on(0, 60, 1.0))],
            );
            // Away from the start, where the filter has settled.
            let got = amplitude_at(&out[0][4096..12_288], hz) / 0.5;
            let wanted = prototype(kind, hz);
            assert!(
                (db(got) - db(wanted)).abs() < 0.2,
                "{name} at {hz} Hz: {} dB for {} dB",
                db(got),
                db(wanted)
            );
        }
    }
    // Off, the filter isn't there.
    let tone = sine(16_384, 4000.0, 0.5);
    let out = play(
        &[(FILTER_FREQ, 100.0)],
        &tone,
        16_384,
        &[(0, on(0, 60, 1.0))],
    );
    assert!((amplitude_at(&out[0][4096..12_288], 4000.0) - 0.5).abs() < 0.01);
}

// Spec: one-shot-drum-sampler R16
#[test]
fn a_filter_switched_on_mid_note_sounds_from_its_first_frame() {
    let flat = sample(4800, |_| 1.0);
    let (mut node, _collector) =
        loaded(&[(FILTER_FREQ, 20_000.0), (VELOCITY_TO_VOLUME, 0.0)], &flat);
    // Frame 1,001 is not a multiple of the 16 frames the filter is
    // redesigned at.
    let out = render(
        node.as_mut(),
        2000,
        512,
        &[(0, on(0, 60, 1.0))],
        &[(1001, set(0, FILTER_ON, 1.0))],
    );
    assert!(out[0][1001] > 0.3, "{}", out[0][1001]);
    assert!(out[0][1100] > 0.9, "{}", out[0][1100]);
}

// Spec: one-shot-drum-sampler R17
#[test]
fn velocity_scales_the_volume_and_pan_places_the_sound() {
    let flat = sample(4800, |_| 1.0);
    let notes = [(0, on(0, 60, 0.5))];
    let scaled = play(&[], &flat, 4800, &notes);
    assert!((scaled[0][1000] - 0.5).abs() < 0.01, "{}", scaled[0][1000]);
    let unscaled = play(&[(VELOCITY_TO_VOLUME, 0.0)], &flat, 4800, &notes);
    assert!((unscaled[0][1000] - 1.0).abs() < 0.01);
    let left = play(&[(PAN, -50.0)], &flat, 4800, &[(0, on(0, 60, 1.0))]);
    assert!((left[0][1000] - 1.0).abs() < 0.01 && left[1][1000].abs() < 0.01);
    let right = play(&[(PAN, 50.0)], &flat, 4800, &[(0, on(0, 60, 1.0))]);
    assert!(right[0][1000].abs() < 0.01 && (right[1][1000] - 1.0).abs() < 0.01);
}

// Spec: one-shot-drum-sampler R18
#[test]
fn velocity_moves_the_chosen_destination_by_the_amount() {
    let flat = sample(96_000, |_| 1.0);
    let notes = [(0, on(0, 60, 1.0 / 3.0))];
    // A third of 100 %: the time destinations double (2^(3 * 1/3)).
    let trigger = [
        (VELOCITY_TO_VOLUME, 0.0),
        (HOLD_INF, 0.0),
        (MOD_AMOUNT, 100.0),
    ];
    let with = |dest: f64, extra: &[(ParamId, f64)]| {
        let mut params = trigger.to_vec();
        params.push((MOD_DEST, dest));
        params.extend_from_slice(extra);
        play(&params, &flat, 48_000, &notes)
    };
    // Decay: 100 ms becomes 200 ms; Hold: 100 ms of peak before 100 ms.
    let decay = with(TO_DECAY, &[(HOLD, 0.0), (DECAY, 100.0)]);
    assert!((first(&decay[0], |s| *s == 0.0) as f64 - 9600.0).abs() <= 2.0);
    let hold = with(TO_HOLD, &[(HOLD, 100.0), (DECAY, 100.0)]);
    assert!((first(&hold[0], |s| *s == 0.0) as f64 - 14_400.0).abs() <= 2.0);
    let attack = with(TO_ATTACK, &[(HOLD, 0.0), (DECAY, 0.0), (ATTACK, 100.0)]);
    assert!((attack[0][4800] - 0.5).abs() < 0.01, "{}", attack[0][4800]);

    // Filter: a quarter of the route is an octave, so a 1 kHz low-pass
    // passes 2 kHz at its 3 dB point.
    let tone = sine(16_384, 2000.0, 0.5);
    let out = play(
        &[
            (VELOCITY_TO_VOLUME, 0.0),
            (FILTER_ON, 1.0),
            (FILTER_FREQ, 1000.0),
            (MOD_DEST, TO_FILTER),
            (MOD_AMOUNT, 100.0),
        ],
        &tone,
        16_384,
        &[(0, on(0, 60, 0.25))],
    );
    let got = db(amplitude_at(&out[0][4096..12_288], 2000.0) / 0.5);
    assert!((got + 3.01).abs() < 0.3, "{got} dB");

    // FX1 and FX2 are the effect's two parameters: Sub Osc's level and
    // frequency.
    let blip = sample(480, |_| 0.0);
    let sub = |dest: f64| {
        play(
            &[
                (VELOCITY_TO_VOLUME, 0.0),
                (FX_ON, 1.0),
                (FX_TYPE, SUB_OSC),
                (FX1, if dest == TO_FX1 { 0.0 } else { 100.0 }),
                (FX2, 0.0),
                (HOLD_INF, 0.0),
                (HOLD, 500.0),
                (MOD_DEST, dest),
                (MOD_AMOUNT, 100.0),
            ],
            &blip,
            16_384,
            &[(0, on(0, 60, 0.5))],
        )
    };
    let level = sub(TO_FX1);
    assert!((amplitude_at(&level[0][4096..12_288], 30.0) - 0.5).abs() < 0.06);
    let pitch = sub(TO_FX2);
    assert!((amplitude_at(&pitch[0][4096..12_288], 75.0) - 1.0).abs() < 0.1);
}

// Spec: one-shot-drum-sampler R18
#[test]
fn slide_moves_the_destination_while_the_note_sounds() {
    let blip = sample(480, |_| 0.0);
    let params = [
        (VELOCITY_TO_VOLUME, 0.0),
        (FX_ON, 1.0),
        (FX_TYPE, SUB_OSC),
        (FX1, 0.0),
        (FX2, 0.0),
        (HOLD_INF, 0.0),
        (HOLD, 1000.0),
        (MOD_SOURCE, 1.0),
        (MOD_DEST, TO_FX1),
        (MOD_AMOUNT, 50.0),
    ];
    let notes = [
        (0, on(0, 60, 1.0)),
        (
            12_000,
            expression(12_000, NoteExpression::Slide, 0.8, 60, None),
        ),
    ];
    let out = play(&params, &blip, 24_000, &notes);
    // No slide, no sub; then 0.8 * 50 % = 0.4 of its level.
    assert!(out[0][..11_000].iter().all(|s| *s == 0.0));
    let after = amplitude_at(&out[0][14_000..22_192], 30.0);
    assert!((after - 0.4).abs() < 0.05, "{after}");
}

// Spec: one-shot-drum-sampler R19
#[test]
fn a_per_note_bend_moves_only_its_note_and_only_while_enabled() {
    // Two notes read a ramp: their sum's slope is the sum of their rates.
    let ramp = sample(48_000, |i| i as f32 / 480_000.0);
    let slope = |bend_note: u32, enabled: f64| {
        let notes = [
            (0, note(0, NoteKind::On, 60, 1.0, Some(1))),
            (0, note(0, NoteKind::On, 64, 1.0, Some(2))),
            (
                0,
                expression(
                    0,
                    NoteExpression::Bend,
                    0.25,
                    if bend_note == 1 { 60 } else { 64 },
                    Some(bend_note),
                ),
            ),
        ];
        let out = play(&[(PER_NOTE_BEND, enabled)], &ramp, 2000, &notes);
        f64::from(out[0][1000] - out[0][500]) * 480_000.0 / 500.0
    };
    // A quarter of the 48-semitone range is 12 semitones, an octave.
    let e = |semitones: f64| 2f64.powf(semitones / 12.0);
    let wanted_first = e(12.0) + e(4.0);
    let wanted_second = e(0.0) + e(16.0);
    assert!(
        (slope(1, 1.0) - wanted_first).abs() < 0.01,
        "{}",
        slope(1, 1.0)
    );
    assert!(
        (slope(2, 1.0) - wanted_second).abs() < 0.01,
        "{}",
        slope(2, 1.0)
    );
    // Off, a bend is ignored.
    assert!((slope(1, 0.0) - (e(0.0) + e(4.0))).abs() < 0.01);
}

// Spec: one-shot-drum-sampler R20
#[test]
fn a_repeated_key_fades_the_earlier_note_out_and_the_ninth_note_takes_the_oldest() {
    let flat = sample(96_000, |_| 1.0);
    let params = [(VELOCITY_TO_VOLUME, 0.0)];
    // With an attack as long as the 5 ms fade, the two cross and the level
    // holds.
    let repeat = play(
        &[(VELOCITY_TO_VOLUME, 0.0), (ATTACK, 5.0)],
        &flat,
        24_000,
        &[(0, on(0, 60, 1.0)), (6000, on(6000, 60, 1.0))],
    );
    assert!((repeat[0][5000] - 1.0).abs() < 1e-6);
    assert!((repeat[0][6100] - 1.0).abs() < 0.02, "crossfade, not a sum");
    assert!((repeat[0][8000] - 1.0).abs() < 1e-6, "one voice afterwards");
    // Eight voices sound; a ninth fades the oldest out.
    let notes: Vec<_> = (0..9u8)
        .map(|k| {
            let at = 100 * usize::from(k);
            (at, on(at as u32, 60 + k, 1.0))
        })
        .collect();
    let nine = play(&params, &flat, 12_000, &notes);
    assert!(
        (nine[0][700] - 8.0).abs() < 1e-3,
        "eight at once: {}",
        nine[0][700]
    );
    assert!((nine[0][4000] - 8.0).abs() < 1e-3, "{}", nine[0][4000]);
}

// Spec: one-shot-drum-sampler R21
#[test]
fn a_render_is_the_same_at_every_block_size_for_every_effect() {
    let tone = sample(30_000, |i| {
        ((TAU * 300.0 * i as f64 / HZ).sin() * 0.6 + (i % 97) as f64 / 400.0 - 0.1) as f32
    });
    let notes = [
        (0, note(0, NoteKind::On, 60, 0.8, Some(1))),
        (2500, note(2500, NoteKind::On, 67, 0.4, Some(2))),
        (
            3000,
            expression(3000, NoteExpression::Bend, 0.1, 60, Some(1)),
        ),
        (
            3500,
            expression(3500, NoteExpression::Slide, 0.6, 67, Some(2)),
        ),
        (9000, note(9000, NoteKind::Off, 60, 0.0, Some(1))),
    ];
    for effect in 0..9 {
        let params = [
            (FX_ON, 1.0),
            (FX_TYPE, f64::from(effect)),
            (FX1, 70.0),
            (FX2, 40.0),
            (FILTER_ON, 1.0),
            (FILTER_TYPE, 1.0),
            (FILTER_FREQ, 2500.0),
            (FILTER_RES, 30.0),
            (MOD_SOURCE, 1.0),
            (MOD_DEST, TO_FX2),
            (MOD_AMOUNT, 40.0),
            (ENV_MODE, 1.0),
            (HOLD_INF, 0.0),
            (DECAY, 80.0),
            (PAN, -20.0),
        ];
        // A sweep that crosses block boundaries at every size.
        let events = [
            (4000, set(0, FX1, 20.0)),
            (4001, set(0, FILTER_FREQ, 400.0)),
            (7000, set(0, FX2, 90.0)),
        ];
        let mut reference: Option<Vec<Vec<f32>>> = None;
        for block in BLOCKS {
            let (mut node, _collector) = loaded(&params, &tone);
            let out = render(node.as_mut(), 14_000, block, &notes, &events);
            assert!(
                out[0].iter().any(|s| s.abs() > 0.01),
                "effect {effect} sounds"
            );
            match &reference {
                None => reference = Some(out),
                Some(first) => {
                    assert!(&out == first, "effect {effect} differs at block {block}");
                }
            }
        }
    }
}

// Spec: one-shot-drum-sampler R22
#[test]
fn the_engine_plays_the_sample_the_set_names() {
    let hash = bl_model::ContentHash([9u8; 32]);
    let flat = sample(96_000, |_| 1.0);
    let mut media = MediaMap::new();
    media.insert(hash, Arc::clone(&flat));

    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let strike = drum_sampler_type();
    let mut device: Device = strike.device(editor.new_id());
    device.media = vec![Some(hash)];
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let note = Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4), 100);
    let clip: bl_model::ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 4),
                ClipData::Midi(MidiData::new([note])),
            ),
        })
        .unwrap();

    let registry = registry().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry);
    let unhosted = offline.sync(editor.document());
    assert!(unhosted.is_empty(), "Strike is hosted: {unhosted:?}");
    assert!(offline.missing_media().is_empty(), "the sample is found");
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(48_000, |block| {
        out.extend_from_slice(block.channel(0));
    });
    assert_eq!(offline.heap_operations(), 0, "no allocation in a block");
    // Velocity 100 of 127 scales the level; Hold Inf plays on.
    let wanted = 100.0 / 127.0;
    assert!((out[1000] - wanted).abs() < 0.01, "plays: {}", out[1000]);
    assert!(
        (out[47_000] - wanted).abs() < 0.01,
        "holds: {}",
        out[47_000]
    );
}
