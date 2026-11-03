//! The Drum Synths (WP-8.24 and WP-8.25; the drum synths spec, R1 to R20):
//! one hit at a time with a short cut, velocity to level, Decay's
//! law, a pitch that ignores the key, the display, seeded noise, and each
//! device's own laws measured. Every block runs as a real-time section
//! under the allocation trap.

use crate::common;
use std::f64::consts::TAU;

use bl_devices::drum_synth::{DECAY, VOLUME};
use bl_devices::{ds_clang, ds_clap, ds_cymbal, ds_fm, ds_hh, ds_kick, ds_snare, ds_tom};
use bl_engine::{Block, DeviceType, Node, NodeEvent, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_engine::{Collector, TransportInfo};
use bl_model::{DeviceKind, DeviceRole, ParamId};
use common::{CHANNELS, RATE, device, node, set};

const HZ: f64 = 48_000.0;

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

/// One of the eight devices.
#[derive(Clone, Copy)]
struct Kind {
    name: &'static str,
    id: &'static str,
    device_type: fn() -> DeviceType,
    register: fn(&mut Registry),
    /// Settings with which the longest part of the sound is the one that
    /// decays over Decay alone.
    tail: &'static [(ParamId, f64)],
}

const KINDS: [Kind; 8] = [
    Kind {
        name: "DS Kick",
        id: ds_kick::DS_KICK,
        device_type: ds_kick::ds_kick_type,
        register: ds_kick::register,
        tail: &[(ds_kick::ENV, 0.0)],
    },
    Kind {
        name: "DS Snare",
        id: ds_snare::DS_SNARE,
        device_type: ds_snare::ds_snare_type,
        register: ds_snare::register,
        tail: &[(ds_snare::TONE, 100.0)],
    },
    Kind {
        name: "DS Tom",
        id: ds_tom::DS_TOM,
        device_type: ds_tom::ds_tom_type,
        register: ds_tom::register,
        tail: &[(ds_tom::TONE, 0.0)],
    },
    Kind {
        name: "DS Clap",
        id: ds_clap::DS_CLAP,
        device_type: ds_clap::ds_clap_type,
        register: ds_clap::register,
        tail: &[(ds_clap::TAIL, 100.0)],
    },
    Kind {
        name: "DS HH",
        id: ds_hh::DS_HH,
        device_type: ds_hh::ds_hh_type,
        register: ds_hh::register,
        tail: &[],
    },
    Kind {
        name: "DS Cymbal",
        id: ds_cymbal::DS_CYMBAL,
        device_type: ds_cymbal::ds_cymbal_type,
        register: ds_cymbal::register,
        tail: &[],
    },
    Kind {
        name: "DS FM",
        id: ds_fm::DS_FM,
        device_type: ds_fm::ds_fm_type,
        register: ds_fm::register,
        // With no modulation the carrier alone falls over Decay.
        tail: &[(ds_fm::AMNT, 0.0)],
    },
    Kind {
        name: "DS Clang",
        id: ds_clang::DS_CLANG,
        device_type: ds_clang::ds_clang_type,
        register: ds_clang::register,
        tail: &[],
    },
];

fn synth(kind: Kind, params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut registry = Registry::empty();
    (kind.register)(&mut registry);
    node(&registry, &device(&(kind.device_type)(), params), 4096)
}

fn on(offset: u32, key: u8, velocity: f32) -> NoteEvent {
    note(offset, NoteKind::On, key, velocity)
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

/// One hit of `kind` at velocity 1, `frames` long, at full Volume.
fn hit(kind: Kind, params: &[(ParamId, f64)], frames: usize) -> Vec<Vec<f32>> {
    let mut all = vec![(VOLUME, 1.0)];
    all.extend_from_slice(params);
    let mut node = synth(kind, &all);
    render(node.as_mut(), frames, 4096, &[(0, on(0, 60, 1.0))], &[])
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len().max(1) as f64).sqrt()
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

/// The mean amplitude over `count` frequencies from `low` to `high`.
fn band(signal: &[f32], low: f64, high: f64) -> f64 {
    let count = 24;
    (0..count)
        .map(|n| amplitude_at(signal, low + (high - low) * n as f64 / f64::from(count - 1)))
        .sum::<f64>()
        / f64::from(count)
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// The Decay a signal's tail has, in milliseconds: the energy left in it
/// after 60 ms falls from `from_db` to `to_db` below what it was, and an
/// envelope that falls 60 dB over Decay does that in the share of Decay
/// those decibels are of 60. The remaining energy is smooth where a noise's
/// peaks are not.
fn decay_ms(signal: &[f32], from_db: f64, to_db: f64) -> f64 {
    let start = (0.06 * HZ) as usize;
    let mut left = vec![0.0f64; signal.len() + 1];
    for i in (start..signal.len()).rev() {
        left[i] = left[i + 1] + f64::from(signal[i]).powi(2);
    }
    let first_below = |db: f64| {
        let wanted = left[start] * 10f64.powf(-db / 10.0);
        (start..signal.len())
            .find(|&i| left[i] < wanted)
            .expect("the energy falls that far")
    };
    let (a, b) = (first_below(from_db), first_below(to_db));
    (b - a) as f64 / HZ * 1000.0 * 60.0 / (to_db - from_db)
}

/// The frequency of a signal over `[from, to)` frames, from its rising zero
/// crossings, in hertz.
fn crossings_hz(signal: &[f32], from: usize, to: usize) -> f64 {
    let rises = signal[from..to]
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    rises as f64 / ((to - from) as f64 / HZ)
}

// Spec: drum-synths R1
#[test]
fn each_device_is_an_instrument_with_its_own_type_id_and_a_description_that_checks() {
    for kind in KINDS {
        let device_type = (kind.device_type)();
        assert_eq!(device_type.name, kind.name);
        assert_eq!(device_type.role, DeviceRole::Instrument, "{}", kind.name);
        assert!(
            matches!(&device_type.kind, DeviceKind::Native { device_type } if device_type == kind.id)
        );
        device_type.description.check().unwrap();
        // The device reads its parameters by their numbers, in order.
        for (index, param) in device_type.description.params.iter().enumerate() {
            assert_eq!(param.id.0 as usize, index, "{}: {}", kind.name, param.name);
        }
        assert!(
            bl_devices::device_types()
                .iter()
                .any(|t| t.kind == device_type.kind),
            "{} is registered",
            kind.name
        );
    }
}

// Spec: drum-synths R2
#[test]
fn a_note_starts_a_hit_that_runs_its_own_length_whatever_the_notes_length() {
    for kind in KINDS {
        let mut held = synth(kind, &[(DECAY, 100.0)]);
        let long = render(held.as_mut(), 12_000, 512, &[(0, on(0, 60, 1.0))], &[]);
        let mut released = synth(kind, &[(DECAY, 100.0)]);
        let short = render(
            released.as_mut(),
            12_000,
            512,
            &[(0, on(0, 60, 1.0)), (100, note(0, NoteKind::Off, 60, 0.0))],
            &[],
        );
        assert_eq!(long, short, "{}: the note's end changes nothing", kind.name);
        assert!(peak(&long[0]) > 0.01, "{}: it sounds", kind.name);
        // 1.5 x Decay, the master envelope's -90 dB, is 7,200 frames.
        assert!(
            peak(&long[0][8_000..]) == 0.0 && peak(&long[1][8_000..]) == 0.0,
            "{}: the hit ends",
            kind.name
        );
    }
}

// Spec: drum-synths R2
#[test]
fn a_new_hit_cuts_the_old_one_over_two_milliseconds() {
    for kind in KINDS {
        let params = [(VOLUME, 1.0), (DECAY, 2_000.0)];
        let second = 6_000;
        let mut twice = synth(kind, &params);
        let both = render(
            twice.as_mut(),
            20_000,
            512,
            &[(0, on(0, 60, 1.0)), (second, on(0, 60, 1.0))],
            &[],
        );
        let mut once = synth(kind, &params);
        let alone = render(
            once.as_mut(),
            20_000 - second,
            512,
            &[(0, on(0, 60, 1.0))],
            &[],
        );
        // The old hit is gone once its 96-frame fade is over: what is left
        // is the second hit alone.
        let cut = second + (bl_devices::drum_synth::CUT_MS / 1000.0 * HZ) as usize + 1;
        for channel in 0..2 {
            for frame in cut..20_000 {
                let diff = (both[channel][frame] - alone[channel][frame - second]).abs();
                assert!(
                    diff < 1e-6,
                    "{}: frame {frame} differs by {diff}",
                    kind.name
                );
            }
        }
        // Before it the two overlap, the old one fading out in a straight
        // line from its full level.
        let fade = cut - second - 1;
        let mut first = synth(kind, &params);
        let old = render(first.as_mut(), cut, 512, &[(0, on(0, 60, 1.0))], &[]);
        for k in 0..fade {
            let wanted = alone[0][k] + old[0][second + k] * (fade - k) as f32 / fade as f32;
            let got = both[0][second + k];
            assert!(
                (got - wanted).abs() < 1e-5,
                "{}: frame {k} of the cut",
                kind.name
            );
        }
    }
}

// Spec: drum-synths R3
#[test]
fn a_hits_level_is_volume_times_velocity_and_never_above_volume() {
    for kind in KINDS {
        let play = |volume: f64, velocity: f32| {
            let mut node = synth(kind, &[(VOLUME, volume)]);
            render(node.as_mut(), 9_000, 512, &[(0, on(0, 60, velocity))], &[])
        };
        let full = play(1.0, 1.0);
        let half_velocity = play(1.0, 0.5);
        let half_volume = play(0.5, 1.0);
        let silent = play(0.0, 1.0);
        assert!(
            peak(&full[0]) <= 1.0 && peak(&full[1]) <= 1.0,
            "{}",
            kind.name
        );
        for frame in (0..9_000).step_by(37) {
            let want = full[0][frame] * 0.5;
            assert!(
                (half_velocity[0][frame] - want).abs() < 1e-6,
                "{}",
                kind.name
            );
            assert!((half_volume[0][frame] - want).abs() < 1e-6, "{}", kind.name);
        }
        assert_eq!(peak(&silent[0]), 0.0, "{}: -inf dB is silent", kind.name);
    }
}

// Spec: drum-synths R4
#[test]
fn decay_is_the_time_to_fall_60_db_within_five_percent() {
    for kind in KINDS {
        for decay in [300.0, 1_000.0] {
            let mut params = vec![(DECAY, decay)];
            params.extend_from_slice(kind.tail);
            let signal = hit(kind, &params, (decay * 1.6 / 1000.0 * HZ) as usize + 2_000);
            let measured = decay_ms(&signal[0], 10.0, 30.0);
            assert!(
                (measured - decay).abs() <= decay * 0.05,
                "{}: Decay {decay} ms measures {measured:.1} ms",
                kind.name
            );
        }
    }
}

// Spec: drum-synths R4
#[test]
fn a_hit_ends_below_minus_90_db() {
    for kind in KINDS {
        let signal = hit(kind, &[(DECAY, 1_000.0)], 90_000);
        // 1.5 x Decay is 72,000 frames.
        assert_eq!(peak(&signal[0][72_100..]), 0.0, "{}", kind.name);
        assert!(peak(&signal[0][60_000..72_000]) > 0.0, "{}", kind.name);
    }
}

// Spec: drum-synths R5
#[test]
fn the_key_chooses_a_pad_and_sets_no_pitch() {
    for kind in KINDS {
        let play = |key: u8| {
            let mut node = synth(kind, &[]);
            render(node.as_mut(), 9_000, 512, &[(0, on(0, key, 0.8))], &[])
        };
        assert_eq!(play(36), play(96), "{}", kind.name);
    }
}

// Spec: drum-synths R8
#[test]
fn every_hit_sounds_the_same_and_two_renders_are_identical() {
    // Settings that use every random part.
    let settings: [(Kind, &[(ParamId, f64)]); 8] = [
        (KINDS[0], &[(ds_kick::CLICK, 1.0)]),
        (KINDS[1], &[(ds_snare::TONE, 70.0)]),
        (KINDS[2], &[]),
        (KINDS[3], &[(ds_clap::TAIL, 80.0)]),
        (KINDS[4], &[(ds_hh::NOISE, 1.0), (ds_hh::SLOPE, 1.0)]),
        (KINDS[5], &[]),
        (KINDS[6], &[(ds_fm::FEEDB, 70.0), (ds_fm::AMNT, 80.0)]),
        (KINDS[7], &[(ds_clang::NOISE, 50.0)]),
    ];
    for (kind, params) in settings {
        let mut all = vec![(DECAY, 100.0)];
        all.extend_from_slice(params);
        let play = || {
            let mut node = synth(kind, &all);
            render(
                node.as_mut(),
                40_000,
                512,
                &[(0, on(0, 60, 1.0)), (20_000, on(0, 60, 1.0))],
                &[],
            )
        };
        let first = play();
        assert_eq!(first, play(), "{}: run after run", kind.name);
        assert!(peak(&first[0]) > 0.01);
        assert_eq!(
            first[0][..15_000],
            first[0][20_000..35_000],
            "{}: every hit the same",
            kind.name
        );
        assert_eq!(
            first[1][..15_000],
            first[1][20_000..35_000],
            "{}",
            kind.name
        );
    }
}

// Spec: drum-synths R10
#[test]
fn renders_are_identical_at_any_block_size_with_a_parameter_moving() {
    let moves: [(Kind, ParamId, f64); 8] = [
        (KINDS[0], ds_kick::PITCH, 80.0),
        (KINDS[1], ds_snare::COLOR, 90.0),
        (KINDS[2], ds_tom::BEND, 90.0),
        (KINDS[3], ds_clap::SPREAD, 100.0),
        (KINDS[4], ds_hh::TONE, 12_000.0),
        (KINDS[5], ds_cymbal::PITCH, 9.0),
        (KINDS[6], ds_fm::FEEDB, 90.0),
        (KINDS[7], ds_clang::FILTER, 3_000.0),
    ];
    for (kind, param, value) in moves {
        let run = |block: usize| {
            let mut node = synth(kind, &[(DECAY, 200.0), (VOLUME, 0.8)]);
            render(
                node.as_mut(),
                30_000,
                block,
                &[(0, on(100, 60, 0.9)), (7_000, on(333, 61, 0.6))],
                &[(2_000, set(0, param, value)), (9_000, set(0, DECAY, 400.0))],
            )
        };
        let golden = run(512);
        assert!(peak(&golden[0]) > 0.01, "{}: it sounds", kind.name);
        assert!(golden.iter().flatten().all(|s| s.is_finite()));
        for block in BLOCKS {
            assert_eq!(run(block), golden, "{}: blocks of {block}", kind.name);
        }
    }
}

// Spec: drum-synths R6
#[test]
fn the_display_draws_the_hits_envelope_from_its_parameters() {
    for kind in KINDS {
        let device_type = (kind.device_type)();
        let values = device(&device_type, &[(DECAY, 400.0)]).params;
        let display = bl_devices::display_for_view(
            kind.id,
            bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()),
            &device_type.description,
            &values,
            None,
            None,
        )
        .unwrap_or_else(|| panic!("{} draws a display", kind.name));
        let envelope = display.series("Envelope").expect("the envelope");
        assert_eq!(envelope.x_unit, "ms");
        // It spans 1.5 x Decay and falls.
        assert!((f64::from(envelope.x_range[1]) - 600.0).abs() < 1e-3);
        let tail = envelope.values.len() * 3 / 4;
        let top = envelope.values.iter().copied().fold(0.0f32, f32::max);
        assert!(top > 0.3, "{}: {top}", kind.name);
        assert!(envelope.values[tail] < top * 0.2, "{}", kind.name);
    }
}

// Spec: drum-synths R6
#[test]
fn a_preview_plays_a_hit_at_velocity_100() {
    use bl_devices::drum_synth::PreviewHit;
    for kind in KINDS {
        let collector = Collector::new();
        let mut previewed = synth(kind, &[(VOLUME, 1.0)]);
        previewed.receive_data(collector.handle().data(PreviewHit));
        let heard = render(previewed.as_mut(), 9_000, 512, &[], &[]);
        let mut played = synth(kind, &[(VOLUME, 1.0)]);
        let wanted = render(
            played.as_mut(),
            9_000,
            512,
            &[(0, on(0, 60, 100.0 / 127.0))],
            &[],
        );
        assert_eq!(heard, wanted, "{}", kind.name);
    }
}

// Spec: drum-synths R11
#[test]
fn ds_kick_sweeps_from_above_down_to_its_pitch() {
    let flat = hit(
        KINDS[0],
        &[(ds_kick::PITCH, 80.0), (ds_kick::ENV, 0.0)],
        24_000,
    );
    let at_rest = crossings_hz(&flat[0], 2_000, 20_000);
    assert!((at_rest - 80.0).abs() < 3.0, "Pitch 80 Hz: {at_rest}");
    let swept = hit(
        KINDS[0],
        &[
            (ds_kick::PITCH, 50.0),
            (ds_kick::ENV, 100.0),
            (DECAY, 2_000.0),
        ],
        48_000,
    );
    // Four octaves up at the start: 800 Hz. The first 5 ms hold several
    // crossings, and by 300 ms it has settled to 50 Hz.
    let early = crossings_hz(&swept[0], 0, 480);
    let late = crossings_hz(&swept[0], 14_400, 40_000);
    assert!(early > 300.0, "starts high: {early}");
    assert!((late - 50.0).abs() < 2.5, "settles on its pitch: {late}");
    // The time constant is 30 ms: the pitch is 2^(4/e) x 50 Hz at 30 ms.
    let at_tau = crossings_hz(&swept[0], 1_300, 1_700);
    let wanted = 50.0 * 2f64.powf(4.0 / std::f64::consts::E);
    assert!(
        (at_tau / wanted - 1.0).abs() < 0.25,
        "{at_tau} for {wanted}"
    );
}

// Spec: drum-synths R11
#[test]
fn ds_kick_attack_ot_drive_and_click() {
    let base = [(ds_kick::ENV, 0.0), (ds_kick::PITCH, 50.0)];
    let hard = hit(KINDS[0], &base, 9_000);
    let soft = hit(
        KINDS[0],
        &[base[0], base[1], (ds_kick::ATTACK, 100.0)],
        9_000,
    );
    // Attack fades the sine in over 10 ms.
    assert!(
        peak(&soft[0][..240]) < peak(&hard[0][..240]) * 0.6,
        "Attack smooths the start"
    );

    // OT adds the second and third harmonics, 6 dB and 12 dB below it.
    let rich = hit(
        KINDS[0],
        &[base[0], base[1], (ds_kick::OT, 100.0), (DECAY, 4_000.0)],
        24_000,
    );
    let window = &rich[0][2_000..18_000];
    let (f1, f2, f3) = (
        amplitude_at(window, 50.0),
        amplitude_at(window, 100.0),
        amplitude_at(window, 150.0),
    );
    assert!((db(f2 / f1) + 6.0).abs() < 0.5, "second: {}", db(f2 / f1));
    assert!((db(f3 / f1) + 12.0).abs() < 0.5, "third: {}", db(f3 / f1));
    let plain = hit(KINDS[0], &[base[0], base[1], (DECAY, 4_000.0)], 24_000);
    assert!(amplitude_at(&plain[0][2_000..18_000], 100.0) < f1 * 0.01);

    // Drive adds odd harmonics, and never raises the peak past Volume.
    let driven = hit(
        KINDS[0],
        &[base[0], base[1], (ds_kick::DRIVE, 100.0), (DECAY, 4_000.0)],
        24_000,
    );
    let window = &driven[0][2_000..18_000];
    assert!(
        amplitude_at(window, 150.0) > amplitude_at(window, 50.0) * 0.05,
        "harmonics"
    );
    assert!(amplitude_at(&plain[0][2_000..18_000], 150.0) < 1e-3);
    assert!(peak(&driven[0]) <= 1.0);

    // Click adds a 1 ms burst around 4 kHz, and nothing after it.
    let clicked = hit(KINDS[0], &[base[0], base[1], (ds_kick::CLICK, 1.0)], 9_000);
    let extra: Vec<f32> = clicked[0]
        .iter()
        .zip(&hard[0])
        .map(|(a, b)| a - b)
        .collect();
    assert!(rms(&extra[..48]) > 0.05, "the click: {}", rms(&extra[..48]));
    assert!(peak(&extra[480..]) < 1e-3, "and nothing after it");
    let burst = &extra[..96];
    assert!(
        band(burst, 3_000.0, 5_000.0) > band(burst, 300.0, 800.0) * 2.0,
        "around 4 kHz"
    );
}

// Spec: drum-synths R12
#[test]
fn ds_snare_body_pitch_color_and_noise_filter_types() {
    let body = [(ds_snare::TONE, 0.0), (DECAY, 4_000.0)];
    let sines = hit(
        KINDS[1],
        &[body[0], body[1], (ds_snare::COLOR, 0.0)],
        24_000,
    );
    let late = &sines[0][4_800..16_800];
    let (low, high) = (amplitude_at(late, 185.0), amplitude_at(late, 330.0));
    assert!(low > 0.01 && high > 0.01, "two sines: {low} {high}");
    assert!(
        amplitude_at(late, 555.0) < low * 0.05,
        "sines have no third harmonic"
    );
    // Tune moves both by its semitones.
    let tuned = hit(
        KINDS[1],
        &[
            body[0],
            body[1],
            (ds_snare::COLOR, 0.0),
            (ds_snare::TUNE, 12.0),
        ],
        24_000,
    );
    let late = &tuned[0][4_800..16_800];
    assert!(amplitude_at(late, 370.0) > 0.01 && amplitude_at(late, 660.0) > 0.01);
    assert!(amplitude_at(late, 185.0) < 0.002);
    // Color morphs towards triangles, which have a third harmonic at a
    // ninth.
    let triangles = hit(
        KINDS[1],
        &[body[0], body[1], (ds_snare::COLOR, 100.0)],
        24_000,
    );
    let late = &triangles[0][4_800..16_800];
    let ratio = amplitude_at(late, 555.0) / amplitude_at(late, 185.0);
    assert!(ratio > 0.05, "the third harmonic: {ratio}");

    // The body falls four times faster than Decay.
    let quick = hit(
        KINDS[1],
        &[body[0], (DECAY, 1_000.0), (ds_snare::COLOR, 0.0)],
        24_000,
    );
    let measured = decay_ms(&quick[0], 10.0, 30.0);
    assert!(
        (measured - 250.0).abs() < 12.5,
        "the body's Decay is a quarter: {measured}"
    );

    // The noise through each filter at 2 kHz (Color 0).
    let noise = |filter: f64| {
        let signal = hit(
            KINDS[1],
            &[
                (ds_snare::TONE, 100.0),
                (ds_snare::COLOR, 0.0),
                (ds_snare::FILTER, filter),
                (DECAY, 1_000.0),
            ],
            12_000,
        );
        let part = signal[0][1_000..9_192].to_vec();
        (
            band(&part, 200.0, 800.0),
            band(&part, 1_700.0, 2_400.0),
            band(&part, 8_000.0, 14_000.0),
        )
    };
    let (low_pass, high_pass, band_pass) = (noise(0.0), noise(1.0), noise(2.0));
    assert!(low_pass.0 > low_pass.2 * 4.0, "low-pass: {low_pass:?}");
    assert!(high_pass.2 > high_pass.0 * 4.0, "high-pass: {high_pass:?}");
    assert!(
        band_pass.1 > band_pass.0 * 1.5 && band_pass.1 > band_pass.2 * 1.5,
        "band-pass: {band_pass:?}"
    );
    // Color raises the cutoff: a low-pass at 8 kHz lets the highs through.
    let open = hit(
        KINDS[1],
        &[
            (ds_snare::TONE, 100.0),
            (ds_snare::COLOR, 100.0),
            (ds_snare::FILTER, 0.0),
            (DECAY, 1_000.0),
        ],
        12_000,
    );
    assert!(band(&open[0][1_000..9_192], 5_000.0, 7_000.0) > low_pass.2 * 3.0);
}

// Spec: drum-synths R13
#[test]
fn ds_tom_bends_down_to_its_pitch_and_rings_the_membrane_modes() {
    let sine = [
        (ds_tom::TONE, 0.0),
        (ds_tom::COLOR, 100.0),
        (ds_tom::PITCH, 110.0),
    ];
    let flat = hit(
        KINDS[2],
        &[sine[0], sine[1], sine[2], (ds_tom::BEND, 0.0)],
        24_000,
    );
    let at_rest = crossings_hz(&flat[0], 1_000, 20_000);
    assert!((at_rest - 110.0).abs() < 4.0, "{at_rest}");
    // Bend 100 % starts an octave up and reaches the pitch at half of
    // Decay (250 ms at 500 ms).
    let bent = hit(
        KINDS[2],
        &[sine[0], sine[1], sine[2], (ds_tom::BEND, 100.0)],
        36_000,
    );
    let start = crossings_hz(&bent[0], 0, 480);
    let settled = crossings_hz(&bent[0], 14_400, 34_000);
    assert!(start > 190.0, "an octave up: {start}");
    assert!(
        (settled - 110.0).abs() < 4.0,
        "back to its pitch: {settled}"
    );
    let halfway = crossings_hz(&bent[0], 4_800, 7_200);
    assert!(
        halfway > 125.0 && halfway < 190.0,
        "in between at 125 ms: {halfway}"
    );

    // Tone rings the four modes of a circular membrane.
    let rung = hit(
        KINDS[2],
        &[
            (ds_tom::TONE, 100.0),
            (ds_tom::COLOR, 100.0),
            (ds_tom::BEND, 0.0),
            (DECAY, 1_000.0),
        ],
        12_000,
    );
    let dry = hit(
        KINDS[2],
        &[
            (ds_tom::TONE, 0.0),
            (ds_tom::COLOR, 100.0),
            (ds_tom::BEND, 0.0),
            (DECAY, 1_000.0),
        ],
        12_000,
    );
    for ratio in [1.594, 2.136, 2.296, 2.653] {
        let hz = 110.0 * ratio;
        let with = amplitude_at(&rung[0][500..8_000], hz);
        let without = amplitude_at(&dry[0][500..8_000], hz);
        assert!(
            with > without * 8.0 && with > 0.005,
            "{hz} Hz: {with} against {without}"
        );
    }

    // Color raises the low-pass: the mode at 2.653 x 400 Hz is above 1 kHz.
    let at = |color: f64| {
        let signal = hit(
            KINDS[2],
            &[
                (ds_tom::TONE, 100.0),
                (ds_tom::COLOR, color),
                (ds_tom::BEND, 0.0),
                (ds_tom::PITCH, 400.0),
                (DECAY, 1_000.0),
            ],
            12_000,
        );
        amplitude_at(&signal[0][100..3_000], 400.0 * 2.653)
    };
    assert!(
        at(100.0) > at(0.0) * 2.5,
        "{} against {}",
        at(100.0),
        at(0.0)
    );
}

// Spec: drum-synths R14
#[test]
fn ds_clap_has_four_bursts_a_tail_a_spread_and_a_sloppy() {
    let mono = [
        (ds_clap::TAIL, 0.0),
        (ds_clap::SPREAD, 0.0),
        (ds_clap::SLOPPY, 0.0),
        (DECAY, 300.0),
    ];
    let bursts = hit(KINDS[3], &mono, 4_800);
    let ms = |from: f64, to: f64| rms(&bursts[0][(from * 48.0) as usize..(to * 48.0) as usize]);
    for n in 0..4 {
        let start = f64::from(n) * 8.0;
        assert!(
            ms(start, start + 1.5) > ms(start + 5.5, start + 7.5) * 8.0,
            "a burst at {start} ms"
        );
    }
    assert!(ms(24.0, 25.5) > 0.05);
    assert!(
        ms(36.0, 60.0) < ms(24.0, 25.5) * 0.01,
        "no tail without Tail"
    );
    // Tail adds noise that carries on and decays over Decay.
    let tailed = hit(
        KINDS[3],
        &[mono[1], mono[2], (ds_clap::TAIL, 100.0), (DECAY, 300.0)],
        4_800,
    );
    assert!(rms(&tailed[0][2_400..2_900]) > rms(&bursts[0][2_400..2_900]) * 20.0 + 0.01);

    // Spread 0 is mono; at 100 % with no Sloppy the lines still line up; and
    // Sloppy delays the right line by its time.
    let sloppy = |spread: f64, sloppy: f64| {
        hit(
            KINDS[3],
            &[
                (ds_clap::TAIL, 0.0),
                (ds_clap::SPREAD, spread),
                (ds_clap::SLOPPY, sloppy),
            ],
            4_800,
        )
    };
    let mono_out = sloppy(0.0, 20.0);
    assert_eq!(mono_out[0], mono_out[1], "Spread 0 is mono");
    let tight = sloppy(100.0, 0.0);
    assert_eq!(tight[0], tight[1], "no Sloppy lines them up");
    let wide = sloppy(100.0, 10.0);
    let lag = |a: &[f32], b: &[f32]| {
        (0..800)
            .max_by(|&x, &y| {
                let dot = |lag: usize| {
                    a.iter()
                        .zip(&b[lag..])
                        .map(|(p, q)| f64::from(p * q))
                        .sum::<f64>()
                };
                dot(x).total_cmp(&dot(y))
            })
            .unwrap()
    };
    assert_eq!(
        lag(&wide[0][..2_000], &wide[1]),
        480,
        "Sloppy 10 ms is 480 frames"
    );
    let half = sloppy(50.0, 10.0);
    assert_ne!(half[0], half[1]);
    assert!(rms(&half[0]) > 0.0);

    // Tune and Tone move the band-pass.
    let centre = |tune: f64, tone: f64| {
        let signal = hit(
            KINDS[3],
            &[
                (ds_clap::TAIL, 0.0),
                (ds_clap::SPREAD, 0.0),
                (ds_clap::TUNE, tune),
                (ds_clap::TONE, tone),
            ],
            2_400,
        );
        let part = &signal[0][..1_900];
        let low = band(part, 500.0, 1_500.0);
        let high = band(part, 2_000.0, 6_000.0);
        high / low
    };
    assert!(centre(12.0, 50.0) > centre(0.0, 50.0) * 1.5, "Tune up");
    assert!(centre(0.0, 100.0) > centre(0.0, 0.0) * 1.5, "Tone brighter");
}

// Spec: drum-synths R15
#[test]
fn ds_hh_noise_colour_slope_tone_pitch_and_attack() {
    let tilt = |params: &[(ParamId, f64)]| {
        let mut all = vec![(ds_hh::TONE, 2_000.0), (DECAY, 1_000.0)];
        all.extend_from_slice(params);
        let part = hit(KINDS[4], &all, 12_000)[0][1_000..9_192].to_vec();
        band(&part, 10_000.0, 16_000.0) / band(&part, 2_500.0, 4_000.0)
    };
    // Pink noise falls 3 dB an octave where white is level: about 6 dB
    // between these two bands, less where the squares' lines add to both.
    let (white, pink) = (tilt(&[]), tilt(&[(ds_hh::NOISE, 1.0)]));
    assert!(white > pink * 1.5, "white {white}, pink {pink}");

    // The high-pass: 24 dB per octave takes more below its cutoff than 12.
    let below = |params: &[(ParamId, f64)]| {
        let mut all = vec![(ds_hh::TONE, 8_000.0), (DECAY, 1_000.0)];
        all.extend_from_slice(params);
        let part = hit(KINDS[4], &all, 12_000)[0][1_000..9_192].to_vec();
        band(&part, 1_000.0, 2_000.0) / band(&part, 10_000.0, 14_000.0)
    };
    let (gentle, steep) = (below(&[]), below(&[(ds_hh::SLOPE, 1.0)]));
    assert!(gentle > steep * 4.0, "12 dB {gentle}, 24 dB {steep}");

    // Tone sets the cutoff.
    let low_mids = |tone: f64| {
        let part = hit(KINDS[4], &[(ds_hh::TONE, tone), (DECAY, 1_000.0)], 12_000)[0][1_000..9_192]
            .to_vec();
        band(&part, 3_000.0, 5_000.0)
    };
    assert!(low_mids(3_000.0) > low_mids(14_000.0) * 4.0);

    // Attack fades the squares in over 20 ms and leaves the noise alone:
    // what it changes is the squares, and only until the fade is over.
    let params = [(ds_hh::TONE, 2_000.0), (DECAY, 500.0)];
    let snappy = hit(KINDS[4], &params, 4_800);
    let slow = hit(
        KINDS[4],
        &[params[0], params[1], (ds_hh::ATTACK, 100.0)],
        4_800,
    );
    let diff: Vec<f32> = snappy[0].iter().zip(&slow[0]).map(|(a, b)| a - b).collect();
    assert!(
        rms(&diff[..480]) > 1e-3,
        "the squares come in: {}",
        rms(&diff[..480])
    );
    assert!(
        peak(&diff[1_500..]) < 1e-4,
        "and then the same: {}",
        peak(&diff[1_500..])
    );

    // Pitch moves the squares: the first of them is a line at 400 Hz x 2^(P/12)
    // above the noise, seen at its ninth harmonic through the high-pass.
    let line = |pitch: f64| {
        let part = hit(
            KINDS[4],
            &[
                (ds_hh::TONE, 2_000.0),
                (ds_hh::PITCH, pitch),
                (DECAY, 4_000.0),
            ],
            72_000,
        )[0][..65_536]
            .to_vec();
        let hz = 400.0 * 2f64.powf(pitch / 12.0) * 9.0;
        (amplitude_at(&part, hz), amplitude_at(&part, hz * 1.013))
    };
    for pitch in [0.0, 7.0] {
        let (on_line, off) = line(pitch);
        assert!(
            on_line > off * 2.5,
            "Pitch {pitch}: {on_line} against {off}"
        );
    }
}

// Spec: drum-synths R16
#[test]
fn ds_cymbal_tone_pitch_and_its_long_decay() {
    let device_type = ds_cymbal::ds_cymbal_type();
    assert_eq!(
        device_type.description.param(DECAY).unwrap().default,
        1_500.0
    );
    let low_mids = |tone: f64| {
        let part = hit(
            KINDS[5],
            &[(ds_cymbal::TONE, tone), (DECAY, 1_000.0)],
            12_000,
        )[0][1_000..9_192]
            .to_vec();
        band(&part, 2_000.0, 4_000.0)
    };
    assert!(low_mids(1_000.0) > low_mids(12_000.0) * 8.0);
    // The squares' first line is at 500 Hz x 2^(Pitch / 12), above the noise.
    for pitch in [0.0, 12.0] {
        let part = hit(
            KINDS[5],
            &[
                (ds_cymbal::TONE, 1_000.0),
                (ds_cymbal::PITCH, pitch),
                (DECAY, 4_000.0),
            ],
            72_000,
        )[0][..65_536]
            .to_vec();
        let hz = 500.0 * 2f64.powf(pitch / 12.0);
        let (on_line, off) = (amplitude_at(&part, hz), amplitude_at(&part, hz * 1.013));
        assert!(
            on_line > off * 2.5,
            "Pitch {pitch}: {on_line} against {off}"
        );
    }
}

// Spec: drum-synths R17
#[test]
fn ds_fm_pitch_amount_feedback_mod_tone_and_the_modulators_envelope() {
    // Without FM it is the carrier's sine at 200 Hz x 2^(Pitch / 12).
    let sine = |pitch: f64| {
        hit(
            KINDS[6],
            &[(ds_fm::AMNT, 0.0), (ds_fm::PITCH, pitch), (DECAY, 4_000.0)],
            24_000,
        )
    };
    for (pitch, hz) in [(0.0, 200.0), (12.0, 400.0), (-12.0, 100.0)] {
        let found = crossings_hz(&sine(pitch)[0], 1_000, 20_000);
        assert!((found / hz - 1.0).abs() < 0.03, "Pitch {pitch}: {found} Hz");
    }

    // Amnt makes sidebands round the carrier, at multiples of the
    // modulator's frequency (a ratio of 1 at Mod 0).
    let sidebands = |amnt: f64, from: usize| {
        let signal = hit(
            KINDS[6],
            &[
                (ds_fm::AMNT, amnt),
                (ds_fm::MOD, 0.0),
                (ds_fm::FEEDB, 0.0),
                (ds_fm::TONE, 20_000.0),
                (DECAY, 4_000.0),
            ],
            72_000,
        );
        let window = &signal[0][from..from + 4_800];
        (
            amplitude_at(window, 200.0),
            amplitude_at(window, 400.0) + amplitude_at(window, 600.0),
        )
    };
    let (_, none) = sidebands(0.0, 0);
    let (_, strong) = sidebands(100.0, 0);
    assert!(none < 1e-3 && strong > 0.05, "{none} then {strong}");
    // The modulator falls twice as fast as the carrier: its index is 8 at
    // the start and 8 x 10^(-3 x 1.05 / 2) = 0.21 at 1.05 s into 4 s, where
    // the first sideband is about J1(I) / J0(I) = I / 2 of the carrier. Had
    // it fallen with the carrier the index would be 1.4 and the ratio near 1.
    let (carrier, side) = sidebands(100.0, 50_400);
    let wanted = 0.5 * 8.0 * 10f64.powf(-3.0 * 1.05 / 2.0);
    assert!(
        (side / carrier / wanted - 1.0).abs() < 0.25,
        "{} for {wanted}",
        side / carrier
    );

    // Mod moves the modulator's ratio, so the sidebands move: 200 Hz
    // spacing at 1, 700 Hz at 3.5.
    let at = |mod_percent: f64| {
        let signal = hit(
            KINDS[6],
            &[
                (ds_fm::AMNT, 30.0),
                (ds_fm::MOD, mod_percent),
                (ds_fm::FEEDB, 0.0),
                (ds_fm::TONE, 20_000.0),
                (DECAY, 4_000.0),
            ],
            24_000,
        );
        let window = &signal[0][..4_800];
        (amplitude_at(window, 400.0), amplitude_at(window, 900.0))
    };
    let (unison, _) = at(0.0);
    let (_, far) = at(100.0);
    assert!(unison > at(100.0).0 * 5.0 && far > at(0.0).1 * 5.0);
    for (share, ratio) in [(0.0, 1.0), (1.0 / 3.0, 1.41), (2.0 / 3.0, 2.76), (1.0, 3.5)] {
        assert!((ds_fm::ratio_at(share) - ratio).abs() < 1e-9);
    }
    let blend = ds_fm::ratio_at(1.0 / 6.0);
    assert!(
        (blend - 1.205).abs() < 1e-9,
        "between the first two: {blend}"
    );

    // Feedb. adds harmonics to the modulator, so the high band rises.
    let highs = |feedb: f64| {
        let signal = hit(
            KINDS[6],
            &[
                (ds_fm::AMNT, 50.0),
                (ds_fm::MOD, 33.0),
                (ds_fm::FEEDB, feedb),
                (ds_fm::TONE, 20_000.0),
                (DECAY, 4_000.0),
            ],
            24_000,
        );
        band(&signal[0][..4_800], 2_000.0, 8_000.0)
    };
    assert!(
        highs(100.0) > highs(0.0) * 1.3,
        "{} against {}",
        highs(100.0),
        highs(0.0)
    );

    // Tone is a low-pass.
    let top = |tone: f64| {
        let signal = hit(
            KINDS[6],
            &[(ds_fm::AMNT, 80.0), (ds_fm::TONE, tone), (DECAY, 4_000.0)],
            24_000,
        );
        band(&signal[0][..4_800], 2_000.0, 8_000.0)
    };
    assert!(top(20_000.0) > top(500.0) * 10.0);
}

// Spec: drum-synths R18
#[test]
fn ds_clang_tones_filter_noise_and_the_claves_repeats() {
    let square_only = [
        (ds_clang::NOISE, 0.0),
        (ds_clang::FILTER, 1_000.0),
        (DECAY, 4_000.0),
    ];
    let lines = |a: f64, b: f64| {
        let mut all = square_only.to_vec();
        all.extend([(ds_clang::TONE_A, a), (ds_clang::TONE_B, b)]);
        let signal = hit(KINDS[7], &all, 24_000);
        let late = &signal[0][4_800..16_800];
        // The third harmonics of the 540 Hz and the 800 Hz squares.
        (amplitude_at(late, 1_620.0), amplitude_at(late, 2_400.0))
    };
    let (a_only, b_only) = (lines(100.0, 0.0), lines(0.0, 100.0));
    assert!(
        a_only.0 > 0.01 && a_only.1 < a_only.0 * 0.05,
        "Tone A: {a_only:?}"
    );
    assert!(
        b_only.1 > 0.01 && b_only.0 < b_only.1 * 0.05,
        "Tone B: {b_only:?}"
    );

    // Noise is white noise through the filters, around Filter.
    let noisy = |filter: f64| {
        let signal = hit(
            KINDS[7],
            &[
                (ds_clang::TONE_A, 0.0),
                (ds_clang::TONE_B, 0.0),
                (ds_clang::NOISE, 100.0),
                (ds_clang::FILTER, filter),
                (DECAY, 1_000.0),
            ],
            12_000,
        );
        let part = &signal[0][1_000..9_192];
        (
            band(part, filter * 0.8, filter * 1.25),
            band(part, filter / 10.0, filter / 7.0),
        )
    };
    for filter in [800.0, 3_000.0] {
        let (centre, below) = noisy(filter);
        assert!(
            centre > below * 3.0,
            "Filter {filter}: {centre} against {below}"
        );
    }
    let quiet = hit(KINDS[7], &square_only, 9_000);
    let loud = hit(
        KINDS[7],
        &[square_only[1], square_only[2], (ds_clang::NOISE, 100.0)],
        9_000,
    );
    assert!(
        band(&loud[0][..9_000], 5_000.0, 9_000.0)
            > band(&quiet[0][..9_000], 5_000.0, 9_000.0) * 3.0
    );

    // Clave: a 2.5 kHz sine burst, Repeat times, each on a frame that is a
    // whole number of 25 ms (1,200 frames) from the hit's start.
    let clave = |repeat: f64, pitch: f64| {
        hit(
            KINDS[7],
            &[
                (ds_clang::CLAVE, 1.0),
                (ds_clang::REPEAT, repeat),
                (ds_clang::PITCH, pitch),
                (DECAY, 10.0),
            ],
            14_400,
        )
    };
    let one = clave(1.0, 0.0);
    let found = crossings_hz(&one[0], 0, 480);
    assert!((found / 2_500.0 - 1.0).abs() < 0.05, "{found} Hz");
    assert!(
        (crossings_hz(&clave(1.0, 12.0)[0], 0, 480) / 5_000.0 - 1.0).abs() < 0.05,
        "Pitch moves it"
    );
    // It dies in about 20 ms (60 dB), and is silent after its 30 ms.
    assert!(rms(&one[0][..48]) > 0.1);
    assert_eq!(peak(&one[0][1_440..]), 0.0);
    let eight = clave(8.0, 0.0);
    for k in 0..8 {
        for j in 0..1_200 {
            let (got, want) = (eight[0][k * 1_200 + j], one[0][j]);
            assert!(
                (got - want).abs() < 1e-3,
                "repeat {k}, frame {j}: {got} for {want}"
            );
        }
    }
    // The last repeat is heard whatever Decay is, and then the hit ends.
    assert!(rms(&eight[0][8_400..8_600]) > 0.05);
    assert_eq!(peak(&eight[0][7 * 1_200 + 1_440..]), 0.0);
    let three = clave(3.0, 0.0);
    assert_eq!(peak(&three[0][2 * 1_200 + 1_440..]), 0.0);
    assert!(rms(&three[0][2_400..2_600]) > 0.05);
    assert_eq!(three[0][..2_400], eight[0][..2_400]);
}

// Spec: drum-synths R18, R10
#[test]
fn the_claves_repeats_land_on_the_same_frames_at_any_block_size() {
    let run = |block: usize| {
        let mut node = synth(
            KINDS[7],
            &[
                (DECAY, 10.0),
                (ds_clang::CLAVE, 1.0),
                (ds_clang::REPEAT, 8.0),
            ],
        );
        render(
            node.as_mut(),
            24_000,
            block,
            &[(0, on(77, 60, 1.0)), (13_000, on(5, 60, 0.7))],
            &[],
        )
    };
    let golden = run(512);
    assert!(peak(&golden[0]) > 0.05);
    for block in BLOCKS {
        assert_eq!(run(block), golden, "blocks of {block}");
    }
}

// Spec: drum-synths R19
#[test]
fn ds_clang_at_its_highest_pitch_folds_back_nothing_above_minus_80_db() {
    // Pitch +24 puts the squares at 2,160 Hz and 3,200 Hz; their odd
    // harmonics above 24 kHz would fold to the frequencies below if the
    // squares were not band-limited. A wide filter lets them through.
    let signal = hit(
        KINDS[7],
        &[
            (ds_clang::NOISE, 0.0),
            (ds_clang::FILTER, 10_000.0),
            (ds_clang::PITCH, 24.0),
            (DECAY, 4_000.0),
        ],
        24_000,
    );
    let window = &signal[0][2_000..18_384];
    let real: Vec<f64> = [2_160.0, 3_200.0]
        .into_iter()
        .flat_map(|f| (1..200).step_by(2).map(move |h| f * f64::from(h)))
        .filter(|hz| *hz < HZ / 2.0)
        .collect();
    let loudest = real
        .iter()
        .map(|hz| amplitude_at(window, *hz))
        .fold(0.0, f64::max);
    assert!(loudest > 0.05, "{loudest}");
    // The folded frequencies of the first 20 harmonics above Nyquist, the
    // ones that land on a real line (3,200 Hz divides the rate) left out.
    let folded: Vec<f64> = [2_160.0, 3_200.0]
        .into_iter()
        .flat_map(|f| (1..40).step_by(2).map(move |h| f * f64::from(h)))
        .filter(|hz| *hz > HZ / 2.0)
        .map(|hz| {
            let wrapped = hz % HZ;
            if wrapped > HZ / 2.0 {
                HZ - wrapped
            } else {
                wrapped
            }
        })
        .filter(|hz| real.iter().all(|line| (line - hz).abs() > 40.0))
        .collect();
    assert!(folded.len() > 10, "{} frequencies to check", folded.len());
    for hz in folded {
        let seen = amplitude_at(window, hz);
        assert!(
            db(seen / loudest) < -80.0,
            "{hz} Hz is {:.1} dB",
            db(seen / loudest)
        );
    }
}

/// What a golden render compares: the level of each of twelve 25 ms
/// stretches of a hit at its settings, in dB, to a tenth. A hash of the
/// samples would differ in its last bit between platforms' maths libraries.
fn profile(signal: &[f32]) -> Vec<f64> {
    (0..12)
        .map(|n| {
            let part = &signal[n * 1_200..(n + 1) * 1_200];
            (db(rms(part).max(1e-9)) * 10.0).round() / 10.0
        })
        .collect()
}

/// A hit of each device at settings that use all of its parts, golden.
const GOLDEN: [[f64; 12]; 8] = [
    [
        -5.1, -8.9, -12.7, -16.2, -20.0, -24.2, -27.4, -31.3, -35.3, -39.0, -42.4, -46.3,
    ],
    [
        -12.2, -18.5, -24.6, -30.5, -36.6, -42.5, -48.5, -54.6, -60.8, -66.5, -72.8, -78.6,
    ],
    [
        -7.1, -9.9, -13.1, -16.5, -19.3, -22.5, -25.5, -28.4, -31.5, -34.6, -37.4, -40.5,
    ],
    [
        -19.1, -20.4, -26.6, -30.3, -34.2, -40.5, -45.9, -50.2, -54.7, -61.2, -64.7, -70.9,
    ],
    [
        -24.9, -28.9, -33.9, -39.1, -44.1, -49.3, -54.3, -59.2, -64.2, -69.3, -74.4, -78.8,
    ],
    [
        -13.0, -14.1, -15.0, -15.9, -16.9, -18.0, -18.8, -20.0, -21.0, -22.0, -23.1, -24.0,
    ],
    [
        -7.4, -12.3, -17.1, -22.3, -27.2, -32.2, -37.2, -42.2, -47.2, -52.2, -57.2, -62.2,
    ],
    [
        -13.4, -19.3, -25.2, -31.5, -37.1, -43.2, -49.2, -55.4, -61.3, -67.3, -73.3, -79.4,
    ],
];

// Spec: drum-synths R11, R12, R13, R14, R15, R16, R17, R18
#[test]
fn each_device_renders_its_golden_hit() {
    let settings: [&[(ParamId, f64)]; 8] = [
        &[
            (ds_kick::ENV, 70.0),
            (ds_kick::OT, 40.0),
            (ds_kick::DRIVE, 30.0),
            (ds_kick::CLICK, 1.0),
        ],
        &[(ds_snare::TONE, 60.0), (ds_snare::COLOR, 40.0)],
        &[(ds_tom::TONE, 60.0), (ds_tom::BEND, 80.0)],
        &[
            (ds_clap::TAIL, 70.0),
            (ds_clap::SLOPPY, 12.0),
            (ds_clap::SPREAD, 80.0),
        ],
        &[
            (ds_hh::NOISE, 1.0),
            (ds_hh::SLOPE, 1.0),
            (ds_hh::ATTACK, 50.0),
            (ds_hh::TONE, 6_000.0),
            (DECAY, 300.0),
        ],
        &[(ds_cymbal::TONE, 3_000.0), (ds_cymbal::PITCH, 5.0)],
        &[
            (ds_fm::AMNT, 70.0),
            (ds_fm::FEEDB, 50.0),
            (ds_fm::MOD, 60.0),
        ],
        &[
            (ds_clang::NOISE, 30.0),
            (ds_clang::FILTER, 1_500.0),
            (ds_clang::PITCH, 3.0),
        ],
    ];
    let profiles: Vec<Vec<f64>> = KINDS
        .into_iter()
        .zip(settings)
        .map(|(kind, params)| profile(&hit(kind, params, 14_400)[0]))
        .collect();
    for (n, (kind, got)) in KINDS.into_iter().zip(profiles).enumerate() {
        for (i, (got, want)) in got.iter().zip(GOLDEN[n]).enumerate() {
            assert!(
                (got - want).abs() <= 0.2,
                "{} stretch {i}: {got} dB for {want}",
                kind.name
            );
        }
    }
}

// Spec: drum-synths R1
#[test]
fn each_device_has_factory_presets_within_its_ranges_that_all_sound() {
    let presets = [
        ds_kick::PRESETS,
        ds_snare::PRESETS,
        ds_tom::PRESETS,
        ds_clap::PRESETS,
        ds_hh::PRESETS,
        ds_cymbal::PRESETS,
        ds_fm::PRESETS,
        ds_clang::PRESETS,
    ];
    for (kind, presets) in KINDS.into_iter().zip(presets) {
        let description = (kind.device_type)().description;
        assert!(presets.len() >= 3, "{}", kind.name);
        for preset in presets {
            for (id, value) in preset.params {
                let param = description.param(*id).expect("a parameter of the device");
                assert!(
                    (param.min()..=param.max()).contains(value),
                    "{} {}: {} is out of range",
                    kind.name,
                    preset.name,
                    param.name
                );
            }
            let signal = hit(kind, preset.params, 9_000);
            assert!(
                peak(&signal[0]) > 0.05,
                "{} {} sounds",
                kind.name,
                preset.name
            );
        }
    }
}
