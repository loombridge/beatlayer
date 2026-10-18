//! Impulse, the eight-slot drum sampler (WP-5.17; the eight-slot drum sampler
//! spec): each slot's key playing its own sample at its pitch, Start,
//! Transpose, Stretch in both modes, the filter's types, the saturator, the
//! decay in Trigger and Gate, pan and volume with their velocity and random
//! amounts, mute and solo, the link between slots 7 and 8, parameter
//! changes that wait for the next note, Time, and renders that repeat at any
//! block size. Every block runs as a real-time section under the allocation
//! trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_devices::kitbag::{KEYS, LINK, SLOTS, TIME, TRANSPOSE, VOLUME, kitbag_type, param, slot};
use bl_engine::{
    Block, Collector, DeviceType, EngineConfig, MediaMap, Node, NodeEvent, NoteBuffer, NoteEvent,
    NoteKind, Offline, ParamValue, Registry, SlotSample, TransportCommand, TransportInfo,
};
use bl_model::device_layout::Size;
use bl_model::{Clip, ClipData, Command, Device, DeviceKind, DeviceRole, Editor, MidiData, Note};
use bl_model::{ParamId, TrackId, TrackKind};
use common::{CHANNELS, RATE, device, node};

/// The sample rate the tests run at, in hertz.
const HZ: f64 = 48_000.0;

/// A registry with the drum sampler alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::kitbag::register(&mut registry);
    registry
}

/// A drum sampler of `params`, up to `max_frames` at a time.
fn kitbag(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = kitbag_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// Decoded audio of `frames` frames at the engine's rate, one channel.
fn sample(frames: usize, value: impl Fn(usize) -> f32) -> Arc<bl_media::SampleData> {
    Arc::new(bl_media::SampleData::new(RATE.hz(), vec![(0..frames).map(value).collect()]).unwrap())
}

/// Gives `slot` of `node` the sample, through the collector the engine
/// would.
fn give(node: &mut dyn Node, slot: usize, data: &Arc<bl_media::SampleData>) {
    let collector = Collector::new();
    node.receive_data(collector.handle().data(SlotSample {
        slot,
        sample: Arc::clone(data),
    }));
}

fn note(offset: u32, kind: NoteKind, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel: 0,
        key,
        velocity,
        note_id: None,
    }
}

fn on(offset: u32, slot: usize) -> NoteEvent {
    note(offset, NoteKind::On, KEYS[slot], 1.0)
}

fn off(offset: u32, slot: usize) -> NoteEvent {
    note(offset, NoteKind::Off, KEYS[slot], 1.0)
}

fn set(offset: u32, param: ParamId, value: f64) -> NodeEvent {
    NodeEvent {
        offset,
        param,
        value: ParamValue::Set(value),
    }
}

/// Runs one block of `frames` frames through `node`, silent at the start,
/// playing `notes` after `events`, as a real-time section, and returns each
/// channel.
fn run(
    node: &mut dyn Node,
    frames: usize,
    notes: &[NoteEvent],
    events: &[NodeEvent],
) -> Vec<Vec<f32>> {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut out = NoteBuffer::with_capacity(notes.len().max(1));
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// The first frame of `samples` where `wanted` holds, or the end.
fn first(samples: &[f32], wanted: impl Fn(&f32) -> bool) -> usize {
    samples.iter().position(wanted).unwrap_or(samples.len())
}

/// The last frame of `samples` where `wanted` holds, or 0.
fn last(samples: &[f32], wanted: impl Fn(&f32) -> bool) -> usize {
    samples.iter().rposition(wanted).unwrap_or(0)
}

/// The amplitude of `hz` in `signal`, by a windowed DFT.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n).cos();
        let phase = core::f64::consts::TAU * hz * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
    }
    2.0 * re.hypot(im) / n
}

/// The frequency near `around` where `signal` is strongest, found in 1 Hz
/// steps within 15 %.
fn peak_near(signal: &[f32], around: f64) -> f64 {
    let mut best = (0.0, around);
    let mut hz = around * 0.85;
    while hz <= around * 1.15 {
        let amplitude = amplitude_at(signal, hz);
        if amplitude > best.0 {
            best = (amplitude, hz);
        }
        hz += 1.0;
    }
    best.1
}

/// A gain in decibels.
fn db(gain: f64) -> f64 {
    20.0 * gain.log10()
}

fn sine(frames: usize, hz: f64) -> Arc<bl_media::SampleData> {
    sample(frames, |i| {
        (core::f64::consts::TAU * hz * i as f64 / HZ).sin() as f32 * 0.8
    })
}

/// A sample of 1.0 throughout.
fn flat(frames: usize) -> Arc<bl_media::SampleData> {
    sample(frames, |_| 1.0)
}

/// The slot parameter `local` of `slot`.
fn p(slot: usize, local: u32) -> ParamId {
    param(slot, local)
}

// Spec: eight-slot-drum-sampler R1
#[test]
fn the_drum_sampler_is_described_and_laid_out() {
    let device_type = kitbag_type();
    assert_eq!(device_type.name, "Impulse");
    assert_eq!(device_type.role, DeviceRole::Instrument);
    // Every parameter is described once and placed in the layout, and the
    // slots' parameters follow each other in the order the device reads
    // them by.
    device_type.description.check().unwrap();
    let params = &device_type.description.params;
    assert_eq!(params.len(), SLOTS * slot::COUNT as usize + 4);
    for s in 0..SLOTS {
        for local in 0..slot::COUNT {
            assert_eq!(
                params[s * slot::COUNT as usize + local as usize].id,
                param(s, local)
            );
        }
    }
    assert_eq!(params[SLOTS * slot::COUNT as usize].id, VOLUME);
    assert_eq!(params[SLOTS * slot::COUNT as usize + 3].id, LINK);
    // The panel shows the selected slot's controls and no other's.
    let layout = &device_type.description.layout;
    let size = layout.docked_size();
    let shown = |selected| -> Vec<ParamId> {
        layout
            .place_for(Size::new(size.width, size.height), selected)
            .controls
            .iter()
            .map(|c| c.param)
            .collect()
    };
    let third = shown(2);
    assert!(third.contains(&param(2, slot::PAN)));
    assert!(!third.contains(&param(1, slot::PAN)));
    assert!(third.contains(&VOLUME), "the globals are always there");
    assert_eq!(
        layout
            .place(size)
            .cells
            .iter()
            .filter_map(|c| c.slot)
            .count(),
        SLOTS,
        "a cell for each slot"
    );
}

// Spec: eight-slot-drum-sampler R2
#[test]
fn each_slots_key_plays_its_sample_at_its_pitch() {
    // Each slot's sample is one impulse, at a frame of its own.
    let impulse_at = |s: usize| 100 + 50 * s;
    for (s, key) in KEYS.into_iter().enumerate() {
        let mut node = kitbag(&[], 2048);
        for t in 0..SLOTS {
            give(
                node.as_mut(),
                t,
                &sample(1000, |i| if i == impulse_at(t) { 1.0 } else { 0.0 }),
            );
        }
        let out = run(node.as_mut(), 2048, &[on(0, s)], &[]);
        assert_eq!(
            first(&out[0], |v| v.abs() > 0.5),
            impulse_at(s),
            "key {} plays slot {}'s sample from its start",
            key,
            s + 1
        );
        assert_eq!(
            out[0].iter().filter(|v| v.abs() > 0.01).count(),
            1,
            "and nothing else sounds"
        );
        assert_eq!(out[0], out[1], "centred");
    }
    // Keys outside the eight, and a slot with no sample, are silent.
    let mut node = kitbag(&[], 512);
    give(node.as_mut(), 0, &flat(1000));
    let out = run(
        node.as_mut(),
        512,
        &[
            note(0, NoteKind::On, KEYS[0] - 1, 1.0),
            note(0, NoteKind::On, KEYS[0] + 1, 1.0),
            note(0, NoteKind::On, KEYS[SLOTS - 1] + 1, 1.0),
            on(0, 1),
        ],
        &[],
    );
    assert!(out.iter().flatten().all(|v| *v == 0.0));
}

// Spec: eight-slot-drum-sampler R3
#[test]
fn start_skips_its_milliseconds() {
    // A ramp, so the first frame says where playback began.
    let ramp = sample(4800, |i| i as f32 / 4800.0);
    let mut node = kitbag(&[(p(0, slot::START), 10.0)], 512);
    give(node.as_mut(), 0, &ramp);
    let out = run(node.as_mut(), 512, &[on(0, 0)], &[]);
    // 10 ms is 480 frames in.
    assert!((out[0][0] - 0.1).abs() < 1e-3, "{}", out[0][0]);
    assert!((out[0][100] - (580.0 / 4800.0)).abs() < 1e-3);
}

// Spec: eight-slot-drum-sampler R4
#[test]
fn transpose_by_twelve_halves_or_doubles_the_speed() {
    let impulse = sample(4800, |i| if i == 1000 { 1.0 } else { 0.0 });
    let at = |params: &[(ParamId, f64)], velocity: f32| {
        let mut node = kitbag(params, 4800);
        give(node.as_mut(), 0, &impulse);
        let out = run(
            node.as_mut(),
            4800,
            &[note(0, NoteKind::On, KEYS[0], velocity)],
            &[],
        );
        first(&out[0], |v| v.abs() > 0.5)
    };
    let no_velocity = (p(0, slot::VOLUME_VELOCITY), 0.0);
    assert_eq!(at(&[no_velocity], 1.0), 1000, "untransposed");
    assert_eq!(at(&[(p(0, slot::TRANSPOSE), 12.0), no_velocity], 1.0), 500);
    assert_eq!(
        at(&[(p(0, slot::TRANSPOSE), -12.0), no_velocity], 1.0),
        2000
    );
    assert_eq!(at(&[(TRANSPOSE, 12.0), no_velocity], 1.0), 500, "global");
    assert_eq!(
        at(
            &[
                (TRANSPOSE, 12.0),
                (p(0, slot::TRANSPOSE), -12.0),
                no_velocity
            ],
            1.0
        ),
        1000,
        "the two add"
    );
    // The velocity amount: a note at half velocity plays 12 semitones
    // below the hardest at 100 %, and a full-velocity note doesn't move.
    let amount = (p(0, slot::TRANSPOSE_VELOCITY), 100.0);
    assert_eq!(at(&[amount, no_velocity], 1.0), 1000);
    assert_eq!(at(&[amount, no_velocity], 0.5), 2000);
}

// Spec: eight-slot-drum-sampler R5
#[test]
fn stretch_changes_the_length_and_keeps_the_pitch_in_both_modes() {
    for (mode, hz, tolerance) in [(0.0, 440.0, 0.005), (1.0, 1_000.0, 0.005)] {
        let tone = sine(24_000, hz);
        let length_of = |stretch: f64| {
            let mut node = kitbag(
                &[
                    (p(0, slot::STRETCH), stretch),
                    (p(0, slot::STRETCH_MODE), mode),
                    (p(0, slot::DECAY), 10_000.0),
                ],
                100_000,
            );
            give(node.as_mut(), 0, &tone);
            let out = run(node.as_mut(), 100_000, &[on(0, 0)], &[]);
            let end = last(&out[0], |v| v.abs() > 0.02);
            (end, out)
        };
        let (plain, _) = length_of(0.0);
        assert!((plain as f64 - 24_000.0).abs() < 200.0, "{plain}");
        for (stretch, factor) in [(100.0, 2.0), (-100.0, 0.5), (50.0, 2f64.powf(0.5))] {
            let (end, out) = length_of(stretch);
            let expected = 24_000.0 * factor;
            assert!(
                (end as f64 - expected).abs() < expected * 0.05 + 1_000.0,
                "mode {mode} at {stretch} %: {end} frames, not about {expected}"
            );
            // The pitch, measured in the middle of the stretched sound.
            let middle = (expected as usize / 4)..(expected as usize * 3 / 4);
            let heard = peak_near(&out[0][middle], hz);
            assert!(
                (heard - hz).abs() < hz * tolerance,
                "mode {mode} at {stretch} %: {heard} Hz, not {hz} Hz"
            );
        }
    }
}

// Spec: eight-slot-drum-sampler R5
#[test]
fn a_stretched_hit_keeps_its_attack() {
    // A click at the start: the first grain begins at the top of its
    // window, so the click isn't faded in.
    let click = sample(24_000, |i| if i < 4 { 1.0 } else { 0.0 });
    for mode in [0.0, 1.0] {
        let mut node = kitbag(
            &[
                (p(0, slot::STRETCH), 100.0),
                (p(0, slot::STRETCH_MODE), mode),
            ],
            4096,
        );
        give(node.as_mut(), 0, &click);
        let out = run(node.as_mut(), 4096, &[on(0, 0)], &[]);
        assert!(out[0][0] > 0.9, "mode {mode}: {}", out[0][0]);
    }
}

// Spec: eight-slot-drum-sampler R6
#[test]
fn the_filters_types_match_their_prototypes_within_point_two_db() {
    let frequencies = [
        200.0, 400.0, 700.0, 1_000.0, 1_400.0, 2_000.0, 4_000.0, 8_000.0,
    ];
    // One sample holding every test frequency at once.
    let tones = sample(24_000, |i| {
        let t = i as f64 / HZ;
        let sum: f64 = frequencies
            .iter()
            .map(|hz| (core::f64::consts::TAU * hz * t).sin())
            .sum();
        (sum / frequencies.len() as f64 * 0.8) as f32
    });
    let cutoff = 1_000.0;
    let s_at = |hz: f64| {
        (core::f64::consts::PI * hz / HZ).tan() / (core::f64::consts::PI * cutoff / HZ).tan()
    };
    // The kit's prototypes, written out independently: a second-order
    // section with damping k. The low-pass and high-pass run at a
    // Butterworth's damping and the band-pass and notch at 1 (unity at the
    // centre).
    let second = |k: f64, s: f64| 1.0 / ((1.0 - s * s).powi(2) + (k * s).powi(2)).sqrt();
    let prototype = |kind: usize, s: f64| -> f64 {
        let k = 2f64.sqrt();
        let lp = |k: f64| second(k, s);
        let hp = |k: f64| s * s * second(k, s);
        let bp = |k: f64| s * second(k, s);
        let notch = |k: f64| (1.0 - s * s).abs() * second(k, s);
        match kind {
            0 => lp(k),
            // The fourth-order pair: 2 sin(3π/8) then 2 sin(π/8).
            1 => lp(1.847_759) * second(0.765_366_9, s),
            2 => hp(k),
            3 => hp(1.847_759) * s * s * second(0.765_366_9, s),
            4 => bp(1.0),
            5 => bp(1.0).powi(2),
            6 => notch(2.0),
            7 => notch(2.0).powi(2),
            _ => unreachable!(),
        }
    };
    for kind in 0..8usize {
        for hz in frequencies {
            // Slot 3 has the filter, to show every slot has its own.
            let measured = filter_gain_at_slot(
                2,
                &[
                    (p(2, slot::FILTER), 1.0),
                    (p(2, slot::FILTER_TYPE), kind as f64),
                    (p(2, slot::FILTER_FREQ), cutoff),
                ],
                &tones,
                hz,
            );
            let expected = db(prototype(kind, s_at(hz)));
            if expected.is_infinite() {
                assert!(
                    measured < -30.0,
                    "type {kind} at {hz} Hz: a notch of only {measured:.1} dB"
                );
            } else {
                assert!(
                    (measured - expected).abs() < 0.2,
                    "type {kind} at {hz} Hz: {measured:.3} dB, the prototype {expected:.3} dB"
                );
            }
        }
    }
}

/// The ratio of the filtered sample's level at `hz` to the plain one's, in
/// decibels, with the parameters in `params`, playing slot `slot`.
fn filter_gain_at_slot(
    slot: usize,
    params: &[(ParamId, f64)],
    tones: &Arc<bl_media::SampleData>,
    hz: f64,
) -> f64 {
    let heard = |params: &[(ParamId, f64)]| {
        let mut node = kitbag(params, 24_000);
        give(node.as_mut(), slot, tones);
        let out = run(node.as_mut(), 24_000, &[on(0, slot)], &[]);
        amplitude_at(&out[0][8_000..16_000], hz)
    };
    db(heard(params) / heard(&[]))
}

// Spec: eight-slot-drum-sampler R6
#[test]
fn the_filters_frequency_follows_velocity_and_a_random_amount() {
    let tones = sine(24_000, 4_000.0);
    // A low-pass at 1 kHz takes a 4 kHz tone down; velocity at 100 % moves
    // the cutoff four octaves below the hardest note at silence, so a soft
    // note is darker, and a full-velocity one is as the knob says.
    let heard = |velocity: f32, extra: &[(ParamId, f64)]| {
        let mut params = vec![
            (p(0, slot::FILTER), 1.0),
            (p(0, slot::FILTER_FREQ), 4_000.0),
            (p(0, slot::VOLUME_VELOCITY), 0.0),
        ];
        params.extend_from_slice(extra);
        let mut node = kitbag(&params, 24_000);
        give(node.as_mut(), 0, &tones);
        let out = run(
            node.as_mut(),
            24_000,
            &[note(0, NoteKind::On, KEYS[0], velocity)],
            &[],
        );
        amplitude_at(&out[0][8_000..16_000], 4_000.0)
    };
    let amount = [(p(0, slot::FILTER_VELOCITY), 100.0)];
    let hard = heard(1.0, &amount);
    let soft = heard(0.5, &amount);
    assert!(db(soft / hard) < -6.0, "{}", db(soft / hard));
    // Two notes with a random amount differ, and a render repeats.
    let mut node = kitbag(
        &[
            (p(0, slot::FILTER), 1.0),
            (p(0, slot::FILTER_FREQ), 4_000.0),
            (p(0, slot::FILTER_RANDOM), 100.0),
        ],
        24_000,
    );
    give(node.as_mut(), 0, &tones);
    let first = run(node.as_mut(), 24_000, &[on(0, 0)], &[]);
    let second = run(node.as_mut(), 24_000, &[on(0, 0)], &[]);
    assert_ne!(first, second, "each note draws its own value");
}

// Spec: eight-slot-drum-sampler R7
#[test]
fn the_saturator_adds_harmonics_and_its_drive_boosts_the_level() {
    let tone = sine(24_000, 500.0);
    let heard = |params: &[(ParamId, f64)]| {
        let mut node = kitbag(params, 24_000);
        give(node.as_mut(), 0, &tone);
        run(node.as_mut(), 24_000, &[on(0, 0)], &[])
    };
    let clean = heard(&[]);
    let off = heard(&[(p(0, slot::DRIVE), 100.0)]);
    assert_eq!(clean, off, "with the saturator off, Drive does nothing");
    let driven = heard(&[(p(0, slot::SATURATOR), 1.0), (p(0, slot::DRIVE), 100.0)]);
    let window = 8_000..16_000;
    let third = |out: &[Vec<f32>]| amplitude_at(&out[0][window.clone()], 1_500.0);
    assert!(third(&clean) < 1e-3, "a sine has no third harmonic");
    assert!(
        third(&driven) > 0.05,
        "driving it adds one: {}",
        third(&driven)
    );
    // Drive 0 is a gentle curve: the peak is kept.
    let gentle = heard(&[(p(0, slot::SATURATOR), 1.0)]);
    let peak = |out: &[Vec<f32>]| out[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((peak(&gentle) - peak(&clean)).abs() < 0.3 * peak(&clean));
}

// Spec: eight-slot-drum-sampler R8
#[test]
fn trigger_plays_the_decay_through_and_gate_starts_it_at_note_off() {
    let long = flat(96_000);
    let decay = (p(0, slot::DECAY), 100.0);
    // Trigger: 100 ms is 4,800 frames from the note, whatever the note
    // does.
    let mut node = kitbag(&[decay], 12_000);
    give(node.as_mut(), 0, &long);
    let out = run(node.as_mut(), 12_000, &[on(0, 0), off(2_400, 0)], &[]);
    assert!((out[0][0] - 1.0).abs() < 0.01);
    assert!((out[0][2_400] - 0.5).abs() < 0.02, "{}", out[0][2_400]);
    assert!(out[0][4_800..].iter().all(|v| v.abs() < 1e-3), "gone");
    // Gate: the sound holds until the note ends, then decays.
    let gate = (p(0, slot::DECAY_MODE), 1.0);
    let mut node = kitbag(&[decay, gate], 12_000);
    give(node.as_mut(), 0, &long);
    let out = run(node.as_mut(), 12_000, &[on(0, 0), off(2_400, 0)], &[]);
    assert!((out[0][2_300] - 1.0).abs() < 0.01, "held");
    assert!(
        (out[0][2_400 + 2_400] - 0.5).abs() < 0.02,
        "{}",
        out[0][4_800]
    );
    assert!(out[0][7_300..].iter().all(|v| v.abs() < 1e-3), "gone");
    // With no note-off a gated sound plays to its sample's end.
    let mut node = kitbag(&[decay, gate], 12_000);
    give(node.as_mut(), 0, &long);
    let out = run(node.as_mut(), 12_000, &[on(0, 0)], &[]);
    assert!((out[0][11_999] - 1.0).abs() < 0.01);
}

// Spec: eight-slot-drum-sampler R9
#[test]
fn pan_and_volume_place_and_scale_the_slot() {
    let level = |params: &[(ParamId, f64)], velocity: f32| {
        let mut node = kitbag(params, 512);
        give(node.as_mut(), 0, &flat(2_000));
        let out = run(
            node.as_mut(),
            512,
            &[note(0, NoteKind::On, KEYS[0], velocity)],
            &[],
        );
        (out[0][400], out[1][400])
    };
    let near = |(l, r): (f32, f32), want: (f32, f32)| {
        assert!(
            (l - want.0).abs() < 0.01 && (r - want.1).abs() < 0.01,
            "{l} {r}"
        );
    };
    near(level(&[], 1.0), (1.0, 1.0));
    near(level(&[(p(0, slot::PAN), -50.0)], 1.0), (1.0, 0.0));
    near(level(&[(p(0, slot::PAN), 25.0)], 1.0), (0.5, 1.0));
    near(level(&[(p(0, slot::VOLUME), 0.5)], 1.0), (0.5, 0.5));
    near(level(&[(p(0, slot::VOLUME), 0.0)], 1.0), (0.0, 0.0));
    near(level(&[(VOLUME, 0.0)], 1.0), (0.0, 0.0));
    // Velocity scales the level by its amount (100 % by default), and pan
    // moves with its own amount.
    near(level(&[], 0.5), (0.5, 0.5));
    near(
        level(&[(p(0, slot::VOLUME_VELOCITY), 0.0)], 0.5),
        (1.0, 1.0),
    );
    near(
        level(&[(p(0, slot::VOLUME_VELOCITY), 50.0)], 0.5),
        (0.75, 0.75),
    );
    near(
        level(
            &[
                (p(0, slot::PAN_VELOCITY), 100.0),
                (p(0, slot::VOLUME_VELOCITY), 0.0),
            ],
            0.5,
        ),
        (1.0, 0.5),
    );
    // A random pan moves between notes and stays inside the range.
    let mut node = kitbag(&[(p(0, slot::PAN_RANDOM), 100.0)], 512);
    give(node.as_mut(), 0, &flat(2_000));
    let mut pans = Vec::new();
    for _ in 0..6 {
        let out = run(node.as_mut(), 512, &[on(0, 0)], &[]);
        assert!(out[0][400] <= 1.001 && out[1][400] <= 1.001);
        pans.push(out[0][400] - out[1][400]);
    }
    assert!(
        pans.windows(2).any(|w| (w[0] - w[1]).abs() > 0.01),
        "{pans:?}"
    );
}

// Spec: eight-slot-drum-sampler R10
#[test]
fn mute_silences_a_slot_and_solo_silences_the_others() {
    let sounds = |params: &[(ParamId, f64)]| {
        let mut node = kitbag(params, 512);
        for s in 0..SLOTS {
            give(node.as_mut(), s, &flat(2_000));
        }
        let out = run(node.as_mut(), 512, &[on(0, 0), on(0, 1), on(0, 2)], &[]);
        out[0][400]
    };
    assert!((sounds(&[]) - 3.0).abs() < 0.05);
    assert!((sounds(&[(p(1, slot::MUTE), 1.0)]) - 2.0).abs() < 0.05);
    assert!((sounds(&[(p(2, slot::SOLO), 1.0)]) - 1.0).abs() < 0.05);
    assert!((sounds(&[(p(0, slot::SOLO), 1.0), (p(2, slot::SOLO), 1.0)]) - 2.0).abs() < 0.05);
    assert_eq!(
        sounds(&[(p(1, slot::SOLO), 1.0), (p(1, slot::MUTE), 1.0)]),
        0.0
    );
}

// Spec: eight-slot-drum-sampler R11
#[test]
fn slot_eight_cuts_slot_seven_and_the_reverse_once_linked() {
    let (seven, eight) = (SLOTS - 2, SLOTS - 1);
    let play = |link: f64, first: usize, second: usize| {
        let mut node = kitbag(&[(LINK, link)], 8_000);
        give(node.as_mut(), seven, &sample(40_000, |_| 1.0));
        give(node.as_mut(), eight, &sample(40_000, |_| 0.5));
        let out = run(
            node.as_mut(),
            8_000,
            &[on(0, first), on(2_000, second)],
            &[],
        );
        (out[0][1_900], out[0][7_900])
    };
    // Unlinked, both ring.
    let (before, after) = play(0.0, seven, eight);
    assert!(
        (before - 1.0).abs() < 0.03 && (after - 1.5).abs() < 0.03,
        "{after}"
    );
    // Linked, the second stops the first.
    let (_, after) = play(1.0, seven, eight);
    assert!((after - 0.5).abs() < 0.03, "slot 8 stops slot 7: {after}");
    let (_, after) = play(1.0, eight, seven);
    assert!((after - 1.0).abs() < 0.03, "slot 7 stops slot 8: {after}");
    // The cut is a short fade, not a click.
    let mut node = kitbag(&[(LINK, 1.0)], 8_000);
    give(node.as_mut(), seven, &sample(40_000, |_| 1.0));
    give(node.as_mut(), eight, &sample(40_000, |_| 0.0));
    let out = run(node.as_mut(), 8_000, &[on(0, seven), on(2_000, eight)], &[]);
    let worst = out[0][1_990..2_400]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 0.01, "{worst}");
    // Other slots are not linked.
    let mut node = kitbag(&[(LINK, 1.0)], 8_000);
    give(node.as_mut(), 0, &sample(40_000, |_| 1.0));
    give(node.as_mut(), 1, &sample(40_000, |_| 0.5));
    let out = run(node.as_mut(), 8_000, &[on(0, 0), on(2_000, 1)], &[]);
    assert!((out[0][7_900] - 1.5).abs() < 0.03);
}

// Spec: eight-slot-drum-sampler R12
#[test]
fn a_changed_slot_parameter_reaches_the_next_note_and_leaves_the_sounding_one() {
    let mut node = kitbag(&[], 12_000);
    give(node.as_mut(), 0, &flat(40_000));
    // The first note sounds; at frame 1,000 its slot goes hard left and its
    // volume halves; the second note starts at 6,000.
    let out = run(
        node.as_mut(),
        12_000,
        &[on(0, 0), on(6_000, 0)],
        &[
            set(1_000, p(0, slot::PAN), -50.0),
            set(1_000, p(0, slot::VOLUME), 0.5),
        ],
    );
    assert!((out[0][5_000] - 1.0).abs() < 0.03 && (out[1][5_000] - 1.0).abs() < 0.03);
    assert!((out[0][11_000] - 0.5).abs() < 0.03 && out[1][11_000].abs() < 0.01);
    // Switching the filter on mid-note leaves the note unfiltered too.
    let tones = sine(40_000, 4_000.0);
    let mut node = kitbag(&[(p(0, slot::FILTER_FREQ), 200.0)], 12_000);
    give(node.as_mut(), 0, &tones);
    let out = run(
        node.as_mut(),
        12_000,
        &[on(0, 0)],
        &[set(1_000, p(0, slot::FILTER), 1.0)],
    );
    let heard = amplitude_at(&out[0][4_000..8_000], 4_000.0);
    assert!(heard > 0.3, "{heard}"); // the DFT's window halves it
}

// Spec: eight-slot-drum-sampler R13
#[test]
fn time_scales_the_decay_and_the_stretch() {
    let time = (TIME, 200.0);
    // Decay 100 ms is 200 ms at a Time of 200 %.
    let mut node = kitbag(&[(p(0, slot::DECAY), 100.0), time], 24_000);
    give(node.as_mut(), 0, &flat(96_000));
    let out = run(node.as_mut(), 24_000, &[on(0, 0)], &[]);
    assert!((out[0][4_800] - 0.5).abs() < 0.02, "{}", out[0][4_800]);
    assert!(out[0][9_700..].iter().all(|v| v.abs() < 1e-3));
    // A sample plays twice as long, at its own pitch.
    let tone = sine(12_000, 440.0);
    let mut node = kitbag(&[time], 40_000);
    give(node.as_mut(), 0, &tone);
    let out = run(node.as_mut(), 40_000, &[on(0, 0)], &[]);
    let end = last(&out[0], |v| v.abs() > 0.02);
    assert!((end as f64 - 24_000.0).abs() < 1_500.0, "{end}");
    let heard = peak_near(&out[0][6_000..18_000], 440.0);
    assert!((heard - 440.0).abs() < 5.0, "{heard}");
    // At 50 % it halves them.
    let mut node = kitbag(&[(TIME, 50.0), (p(0, slot::DECAY), 100.0)], 24_000);
    give(node.as_mut(), 0, &flat(96_000));
    let out = run(node.as_mut(), 24_000, &[on(0, 0)], &[]);
    assert!(out[0][2_500..].iter().all(|v| v.abs() < 1e-3));
}

// Spec: eight-slot-drum-sampler R14
#[test]
fn renders_with_random_amounts_repeat_bit_for_bit() {
    let params = [
        (p(0, slot::TRANSPOSE_RANDOM), 20.0),
        (p(0, slot::PAN_RANDOM), 100.0),
        (p(0, slot::FILTER), 1.0),
        (p(0, slot::FILTER_RANDOM), 50.0),
        (p(0, slot::STRETCH), 30.0),
    ];
    let render = || {
        let mut node = kitbag(&params, 8_192);
        give(node.as_mut(), 0, &sine(8_000, 300.0));
        run(
            node.as_mut(),
            8_192,
            &[on(0, 0), on(3_000, 0), on(6_000, 0)],
            &[],
        )
    };
    assert_eq!(render(), render());
}

/// Renders `total` frames of a busy pattern in blocks of `block` frames:
/// every slot, both stretch modes, the filter, the saturator, the random
/// amounts, the link, a parameter change, and Gate.
fn pattern(block: usize, total: usize) -> Vec<Vec<f32>> {
    let mut params = vec![(LINK, 1.0)];
    for s in 0..SLOTS {
        params.push((p(s, slot::STRETCH), if s % 2 == 0 { 40.0 } else { -40.0 }));
        params.push((p(s, slot::STRETCH_MODE), (s / 2 % 2) as f64));
        params.push((p(s, slot::FILTER), (s % 3 == 0) as u8 as f64));
        params.push((p(s, slot::SATURATOR), (s % 2 == 1) as u8 as f64));
        params.push((p(s, slot::DRIVE), 60.0));
        params.push((p(s, slot::PAN_RANDOM), 80.0));
        params.push((p(s, slot::TRANSPOSE_RANDOM), 10.0));
        params.push((p(s, slot::TRANSPOSE_VELOCITY), 20.0));
        params.push((p(s, slot::FILTER_RANDOM), 30.0));
        params.push((p(s, slot::DECAY), 300.0));
        params.push((p(s, slot::DECAY_MODE), (s % 4 == 3) as u8 as f64));
    }
    let mut node = kitbag(&params, block);
    for s in 0..SLOTS {
        give(
            node.as_mut(),
            s,
            &sine(6_000 + 700 * s, 200.0 + 90.0 * s as f64),
        );
    }
    let notes: Vec<(usize, NoteEvent)> = (0..SLOTS)
        .flat_map(|s| {
            let at = 100 + 1_700 * s;
            [
                (at, note(0, NoteKind::On, KEYS[s], 0.4 + 0.07 * s as f32)),
                (at + 900, note(0, NoteKind::Off, KEYS[s], 0.5)),
            ]
        })
        .collect();
    let events = [(7_777usize, set(0, p(2, slot::PAN), -30.0))];
    let mut out = vec![Vec::new(), Vec::new()];
    for start in (0..total).step_by(block) {
        let frames = block.min(total - start);
        let here: Vec<NoteEvent> = notes
            .iter()
            .filter(|(at, _)| (start..start + frames).contains(at))
            .map(|(at, n)| NoteEvent {
                offset: (*at - start) as u32,
                ..*n
            })
            .collect();
        let changes: Vec<NodeEvent> = events
            .iter()
            .filter(|(at, _)| (start..start + frames).contains(at))
            .map(|(at, e)| NodeEvent {
                offset: (*at - start) as u32,
                ..*e
            })
            .collect();
        let block_out = run(node.as_mut(), frames, &here, &changes);
        for (all, one) in out.iter_mut().zip(block_out) {
            all.extend(one);
        }
    }
    out
}

// Spec: eight-slot-drum-sampler R15
#[test]
fn renders_are_identical_at_any_block_size_under_the_allocation_trap() {
    let reference = pattern(512, 20_000);
    assert!(
        reference[0].iter().any(|v| v.abs() > 0.1),
        "the pattern makes sound"
    );
    for block in [1, 100, 4_096] {
        let got = pattern(block, 20_000);
        let at = got[0].iter().zip(&reference[0]).position(|(a, b)| a != b);
        if let Some(at) = at {
            panic!(
                "block {block}: first difference at {at}: {} vs {}",
                got[0][at], reference[0][at]
            );
        }
    }
}

/// An editor with a MIDI track holding a drum sampler with `samples` in its
/// slots, and a clip playing each slot's key once a quarter note apart.
fn song(samples: &[(usize, bl_model::ContentHash)]) -> (Editor, Device) {
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
    let id = editor.new_id();
    let mut device = Device::new(
        id,
        "Impulse",
        DeviceKind::Native {
            device_type: bl_devices::kitbag::KITBAG.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    for (slot, hash) in samples {
        device.set_media(*slot, Some(*hash));
    }
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: device.clone(),
        })
        .unwrap();
    let ticks = |quarters: i64| bl_core::time::Ticks(bl_core::time::TICKS_PER_QUARTER * quarters);
    let notes: Vec<Note> = (0..SLOTS)
        .map(|s| Note::new(KEYS[s], ticks(s as i64), ticks(1), 100))
        .collect();
    let clip: bl_model::ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                bl_core::time::Ticks::ZERO,
                ticks(SLOTS as i64),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    (editor, device)
}

// Spec: eight-slot-drum-sampler R16
#[test]
fn the_engine_plays_the_eight_samples_the_set_names() {
    // Eight files of one constant each, so the output says which slot
    // sounds.
    let mut media = MediaMap::new();
    let hashes: Vec<(usize, bl_model::ContentHash)> = (0..SLOTS)
        .map(|s| {
            let hash = bl_model::ContentHash([s as u8 + 1; 32]);
            media.insert(
                hash,
                Arc::new(
                    bl_media::SampleData::new(RATE.hz(), vec![vec![0.1 * (s + 1) as f32; 480_000]])
                        .unwrap(),
                ),
            );
            (s, hash)
        })
        .collect();
    let (editor, _) = song(&hashes);
    let registry = registry().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry);
    let unhosted = offline.sync(editor.document());
    assert!(
        unhosted.is_empty(),
        "the drum sampler is hosted: {unhosted:?}"
    );
    assert!(offline.missing_media().is_empty(), "the samples are found");
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    let quarter = 24_000; // At 120 bpm.
    offline.render(quarter * SLOTS as u64, |block| {
        out.extend_from_slice(block.channel(0));
    });
    assert_eq!(offline.heap_operations(), 0, "no allocation in a block");
    // Each slot's note sounds its own level and the hits ring on, so the
    // sum climbs. Velocity 100 of 127 scales the level, and the decay,
    // ten seconds at its default, takes a linear share off each hit.
    let at = |s: usize| s * quarter as usize + quarter as usize - 100;
    for s in 0..SLOTS {
        let want: f32 = (0..=s)
            .map(|k| {
                let age = (at(s) - k * quarter as usize) as f32;
                0.1 * (k + 1) as f32 * (1.0 - age / 480_000.0) * 100.0 / 127.0
            })
            .sum();
        assert!(
            (out[at(s)] - want).abs() < 0.005,
            "slot {}: {}, not {want}",
            s + 1,
            out[at(s)]
        );
    }
}

// Spec: eight-slot-drum-sampler R16
#[test]
fn a_missing_sample_leaves_its_slot_silent_and_the_rest_playing() {
    let mut media = MediaMap::new();
    let hash = bl_model::ContentHash([1; 32]);
    media.insert(
        hash,
        Arc::new(bl_media::SampleData::new(RATE.hz(), vec![vec![0.5; 96_000]]).unwrap()),
    );
    // Slot 2 names a file the library doesn't hold.
    let (editor, _) = song(&[(0, hash), (1, bl_model::ContentHash([9; 32]))]);
    let registry = registry().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, 256), registry);
    offline.sync(editor.document());
    assert_eq!(
        offline.missing_media(),
        vec![bl_model::ContentHash([9; 32])]
    );
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(48_000, |block| out.extend_from_slice(block.channel(0)));
    assert!(out[1_000] > 0.3, "slot 1 plays");
    assert!(
        (out[40_000] - out[23_000]).abs() < 0.1,
        "slot 2 adds nothing: the note retriggers nothing"
    );
}
