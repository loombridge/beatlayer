//! Corpus (WP-8.44; the Corpus spec): each resonator type against the
//! shared model it drives, the tuning, Spread and Width, the LFO, the input
//! filter, Bleed, Gain's limiter and the MIDI sidechain's notes, and renders
//! that repeat at any block size. Every block runs as a real-time section
//! under the allocation trap.

use std::f64::consts::PI;

use crate::common;
use bl_core::time::{Bpm, Frames, SampleRate, TempoMap};
use bl_devices::corpus::{
    self, BLEED, BRIGHT, DECAY, DRY_WET, FILTER_FREQ, FILTER_ON, FILTER_WIDTH, FINE, FREQUENCY,
    GAIN, HIT, INHARM, LFO_AMOUNT, LFO_DIVISION, LFO_HZ, LFO_MODE, LFO_PHASE, LFO_SPIN, LFO_STEREO,
    LFO_WAVE, MATERIAL, OFF_AMOUNT, OFF_DECAY, OPENING, PB_RANGE, POS_L, POS_R, PRIORITY, QUALITY,
    RADIUS, RATIO, SPREAD, TUNE, TUNE_SEMITONES, TYPE, WIDTH, corpus_type, unused,
};
use bl_dsp::physical::{
    Kind, ModalBank, ModalParams, ModalTable, Quality, Shape, WaveguideBank, WaveguideParams,
};
use bl_engine::{
    Block, Node, NodeConfig, NodeEvent, NoteBuffer, NoteEvent, NoteKind, Registry, TransportInfo,
};
use bl_model::ParamId;
use common::CHANNELS;

const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    corpus::register(&mut registry);
    registry
}

fn node_at(rate: SampleRate, params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device = common::device(&corpus_type(), params);
    registry()
        .create(
            &device,
            &NodeConfig {
                round_trip_latency: 0,
                sample_rate: rate,
                max_frames,
                channels: CHANNELS,
            },
        )
        .expect("the registry makes Corpus")
}

fn corpus(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    node_at(common::RATE, params, max_frames)
}

/// What a run delivers besides the audio.
#[derive(Default)]
struct Extras<'a> {
    notes: &'a [NoteEvent],
    events: &'a [NodeEvent],
    tempo: Option<&'a TempoMap>,
}

/// Runs per-channel `input` through `node` in blocks of `block`, each a
/// real-time section, the notes and changes landing on their frames
/// (offsets from the run's start), and returns each channel's output.
fn stream_at(
    node: &mut dyn Node,
    rate: SampleRate,
    input: &[Vec<f32>],
    block: usize,
    extras: &Extras<'_>,
) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut out: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(frames)).collect();
    for start in (0..frames).step_by(block) {
        let end = (start + block).min(frames);
        let length = end - start;
        let inside = |offset: u32| (offset as usize) >= start && (offset as usize) < end;
        let notes: Vec<NoteEvent> = extras
            .notes
            .iter()
            .filter(|n| inside(n.offset))
            .map(|n| NoteEvent {
                offset: n.offset - start as u32,
                ..*n
            })
            .collect();
        let events: Vec<NodeEvent> = extras
            .events
            .iter()
            .filter(|e| inside(e.offset))
            .map(|e| NodeEvent {
                offset: e.offset - start as u32,
                ..*e
            })
            .collect();
        let mut data: Vec<f32> = input.iter().flat_map(|c| c[start..end].to_vec()).collect();
        let transport = TransportInfo {
            playing: extras.tempo.is_some(),
            position: Frames(start as i64),
            ..TransportInfo::default()
        };
        let mut sent = NoteBuffer::with_capacity(8);
        {
            let mut block = Block::new(
                &mut data, CHANNELS, length, length, &events, transport, rate,
            )
            .with_notes(&notes, &mut sent);
            if let Some(tempo) = extras.tempo {
                block = block.with_tempo(tempo);
            }
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (channel, values) in out.iter_mut().zip(data.chunks(length)) {
            channel.extend_from_slice(values);
        }
    }
    out
}

fn stream(
    node: &mut dyn Node,
    input: &[Vec<f32>],
    block: usize,
    extras: &Extras<'_>,
) -> Vec<Vec<f32>> {
    stream_at(node, common::RATE, input, block, extras)
}

/// An impulse of `size` on frame `at`.
fn impulse(frames: usize, at: usize, size: f32) -> Vec<f32> {
    let mut x = vec![0.0; frames];
    x[at] = size;
    x
}

/// Hann-windowed DFT magnitude of `x` at `f` hertz, scaled so a sine of
/// amplitude 1 reads about 1.
fn dft(x: &[f32], rate: f64, f: f64) -> f64 {
    let n = x.len();
    let (sin, cos) = (2.0 * PI * f / rate).sin_cos();
    let (mut c, mut s) = (1.0_f64, 0.0_f64);
    let (mut re, mut im, mut norm) = (0.0, 0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
        re += window * f64::from(v) * c;
        im += window * f64::from(v) * s;
        norm += window;
        (c, s) = (c * cos - s * sin, s * cos + c * sin);
    }
    2.0 * re.hypot(im) / norm
}

/// The frequency of the spectrum's peak within `span` (a fraction) of `f`.
fn peak(x: &[f32], rate: f64, f: f64, span: f64) -> f64 {
    let step = 2.0 * span * f / 40.0;
    let (mut best, mut at) = (-1.0, f);
    for i in 0..=40 {
        let candidate = f * (1.0 - span) + step * f64::from(i);
        let magnitude = dft(x, rate, candidate);
        if magnitude > best {
            (best, at) = (magnitude, candidate);
        }
    }
    let (mut lo, mut hi) = (at - step, at + step);
    for _ in 0..40 {
        let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if dft(x, rate, a) < dft(x, rate, b) {
            lo = a;
        } else {
            hi = b;
        }
    }
    (lo + hi) / 2.0
}

/// How many cents `a` is above `b`.
fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

/// The rate and its `f64`.
fn hz(rate: SampleRate) -> f64 {
    rate.as_f64()
}

// Spec: corpus R1, R16
#[test]
fn corpus_is_a_tapped_midi_sidechain_audio_effect_with_the_xy_cell_and_its_banks() {
    let device_type = corpus_type();
    assert_eq!(device_type.name, "Corpus");
    assert_eq!(device_type.role, bl_model::DeviceRole::AudioEffect);
    assert!(device_type.midi_sidechain && device_type.midi_tap);
    let device = device_type.device(bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()));
    assert_eq!(
        device.midi_sidechain.unwrap().tap,
        Some(bl_model::Tap::PostFx),
        "a new Corpus takes the notes Post FX"
    );
    let description = &device_type.description;
    let layout = &description.layout;
    layout
        .check(&description.params)
        .expect("every control is placed");
    let drawn = serde_json::to_string(layout).unwrap();
    assert!(
        drawn.contains(&format!("\"{}\"", corpus::XY_CELL)),
        "the X–Y display cell"
    );
    assert!(
        layout.has_expanded(),
        "the sidechain's controls are in the expanded part"
    );
    let banks = description.banks();
    let names = |bank: usize| -> Vec<&str> {
        banks[bank]
            .params
            .iter()
            .map(|id| description.param(*id).unwrap().name.as_str())
            .collect()
    };
    assert_eq!(
        names(0),
        [
            "Type", "Decay", "Material", "Bright", "Tune", "Spread", "Gain", "Dry/Wet"
        ]
    );
    assert_eq!(
        names(1),
        [
            "Radius", "Inharm", "Opening", "Ratio", "Hit", "Pos. L", "Pos. R", "Quality"
        ]
    );
}

// Spec: corpus R2
#[test]
fn a_type_dims_the_controls_it_doesnt_use_and_they_keep_their_values() {
    let names = |kind: usize| -> Vec<&'static str> {
        let description = corpus_type().description;
        unused(kind)
            .iter()
            .map(|id| match id {
                &QUALITY => "Quality",
                &MATERIAL => "Material",
                &BRIGHT => "Bright",
                &INHARM => "Inharm",
                &HIT => "Hit",
                &POS_L => "Pos. L",
                &POS_R => "Pos. R",
                &BLEED => "Bleed",
                &RADIUS => "Radius",
                &OPENING => "Opening",
                &RATIO => "Ratio",
                other => panic!("{:?}", description.param(*other).map(|p| &p.name)),
            })
            .collect()
    };
    for kind in [0, 1] {
        assert_eq!(
            names(kind),
            ["Radius", "Opening", "Ratio"],
            "{}",
            corpus::TYPES[kind]
        );
    }
    for kind in [3, 4] {
        assert_eq!(
            names(kind),
            ["Radius", "Opening"],
            "{}",
            corpus::TYPES[kind]
        );
    }
    // A string has no partials to count or weight.
    assert_eq!(
        names(2),
        ["Quality", "Bright", "Radius", "Opening", "Ratio"]
    );
    let plain = [
        "Quality", "Material", "Bright", "Inharm", "Hit", "Pos. L", "Pos. R", "Bleed",
    ];
    let mut pipe: Vec<&str> = plain.to_vec();
    pipe.push("Ratio");
    assert_eq!(names(5), pipe, "a pipe uses Radius and Opening");
    let mut tube: Vec<&str> = plain.to_vec();
    tube.extend(["Opening", "Ratio"]);
    assert_eq!(names(6), tube, "a tube uses Radius");
    // Dimmed, they still hold what they were set to: a render with a
    // dimmed control moved is the same.
    let input = vec![impulse(4_800, 0, 1.0), impulse(4_800, 0, 1.0)];
    let render = |ratio: f64| {
        let mut node = corpus(&[(TYPE, 0.0), (RATIO, ratio)], 512);
        stream(node.as_mut(), &input, 512, &Extras::default())
    };
    assert_eq!(render(10.0), render(90.0), "Ratio does nothing on a beam");
}

/// The tail of `a` against `b`, in the largest difference over the largest
/// value of `b`.
fn mismatch(a: &[f32], b: &[f32]) -> f64 {
    let peak = b.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from((x - y).abs()))
        .fold(0.0, f64::max)
        / f64::from(peak)
}

// Spec: corpus R2, R3
#[test]
fn each_modal_type_drives_its_shared_model_with_the_controls_through_the_specs_mapping() {
    let frames = 9_600;
    // Different amounts on the two channels: each resonator hears its own.
    let input = vec![impulse(frames, 5, 0.3), impulse(frames, 5, 0.2)];
    for (kind, shape) in [
        (0, Shape::Beam),
        (1, Shape::Marimba),
        (3, Shape::Membrane),
        (4, Shape::Plate),
    ] {
        let mut node = corpus(
            &[
                (TYPE, kind as f64),
                (QUALITY, 1.0),
                (DECAY, 40.0),
                (MATERIAL, 30.0),
                (BRIGHT, 20.0),
                (INHARM, 25.0),
                (HIT, 60.0),
                (POS_L, 20.0),
                (POS_R, 80.0),
                (RATIO, 70.0),
                (TUNE, 330.0),
                (GAIN, 0.001),
            ],
            512,
        );
        let out = stream(node.as_mut(), &input, 512, &Extras::default());
        // Gain is low enough that the limiter does nothing.
        let make = 0.001 * corpus::makeup(48_000.0, 0.05 * (30.0_f64 / 0.05).powf(40.0 / 100.0));
        // The model, set as the spec says.
        let aspect = (0.25 * 16.0_f64.powf(70.0 / 100.0)) as f32;
        let mut bank = ModalBank::new(48_000.0, 2);
        drop(bank.set_table(ModalTable::new(shape, Quality::High, aspect, 0.25)));
        for (voice, pos) in [(0, 0.2), (1, 0.8)] {
            bank.reset_voice(
                voice,
                &ModalParams {
                    hz: 330.0,
                    decay: (0.05 * (30.0_f64 / 0.05).powf(40.0 / 100.0)) as f32,
                    material: ((30.0 / 100.0 + 1.0) / 2.0) as f32,
                    brightness: 0.2,
                    hit: 0.6,
                    pos_l: pos,
                    pos_r: pos,
                },
            );
        }
        let stride = bank.stride();
        let mut excitation = vec![0.0; frames * stride];
        for f in 0..frames {
            excitation[f * stride] = input[0][f];
            excitation[f * stride + 1] = input[1][f];
        }
        let mut model = vec![0.0; excitation.len() * 2];
        bank.process(&excitation, &mut model);
        for (channel, voice) in [(0, 0), (1, 1)] {
            let wanted: Vec<f32> = (0..frames).map(|f| model[f * 2 * stride + voice]).collect();
            let wanted: Vec<f32> = wanted
                .iter()
                .map(|&v| (f64::from(v) * make) as f32)
                .collect();
            assert!(rms(&wanted) > 1e-7, "{shape:?} rings");
            let off = mismatch(&out[channel], &wanted);
            assert!(off < 1e-5, "{shape:?}, channel {channel}: {off}");
        }
    }
}

// Spec: corpus R2, R3
#[test]
fn each_waveguide_type_drives_its_shared_model_with_the_controls_through_the_specs_mapping() {
    let frames = 9_600;
    let input = vec![impulse(frames, 5, 0.3), impulse(frames, 5, 0.2)];
    let decay = (0.05 * (30.0_f64 / 0.05).powf(40.0 / 100.0)) as f32;
    // (type, kind, controls, what each voice is set to)
    let string = |pos: f64| WaveguideParams {
        hz: 330.0,
        decay,
        material: ((30.0 / 100.0 + 1.0) / 2.0) as f32,
        inharm: 0.25,
        excite: (0.5 * (1.0 - 60.0 / 100.0)) as f32,
        pickup_l: (0.5 * (1.0 - pos)) as f32,
        pickup_r: (0.5 * (1.0 - pos)) as f32,
        ..WaveguideParams::default()
    };
    let pipe = WaveguideParams {
        hz: 330.0,
        decay,
        opening: 0.3,
        radius: 0.6,
        ..WaveguideParams::default()
    };
    let cases: [(usize, Kind, [WaveguideParams; 2]); 3] = [
        (2, Kind::String, [string(0.2), string(0.8)]),
        (5, Kind::Pipe, [pipe, pipe]),
        (6, Kind::Tube, [pipe, pipe]),
    ];
    for (kind, model_kind, voices) in cases {
        let mut node = corpus(
            &[
                (TYPE, kind as f64),
                (DECAY, 40.0),
                (MATERIAL, 30.0),
                (INHARM, 25.0),
                (HIT, 60.0),
                (POS_L, 20.0),
                (POS_R, 80.0),
                (OPENING, 30.0),
                (RADIUS, 60.0),
                (TUNE, 330.0),
                (GAIN, 0.001),
            ],
            512,
        );
        let out = stream(node.as_mut(), &input, 512, &Extras::default());
        let make = 0.001 * corpus::makeup(48_000.0, f64::from(decay));
        let mut bank = WaveguideBank::new(48_000.0, 2, Kind::String, 5.0);
        bank.set_kind(model_kind);
        for (voice, params) in voices.iter().enumerate() {
            bank.reset_voice(voice, params);
        }
        let stride = bank.stride();
        let mut excitation = vec![0.0; frames * stride];
        for f in 0..frames {
            excitation[f * stride] = input[0][f];
            excitation[f * stride + 1] = input[1][f];
        }
        let mut model = vec![0.0; excitation.len() * 2];
        bank.process(&excitation, &mut model);
        for (channel, voice) in [(0, 0), (1, 1)] {
            let wanted: Vec<f32> = (0..frames)
                .map(|f| (f64::from(model[f * 2 * stride + voice]) * make) as f32)
                .collect();
            assert!(rms(&wanted) > 1e-7, "{model_kind:?} rings");
            let off = mismatch(&out[channel], &wanted);
            assert!(off < 1e-5, "{model_kind:?}, channel {channel}: {off}");
        }
    }
}

/// A beam struck at its start, ringing for `seconds`, with `params`.
fn ring(rate: SampleRate, params: &[(ParamId, f64)], seconds: f64) -> Vec<Vec<f32>> {
    let frames = (seconds * hz(rate)) as usize;
    let mut node = node_at(rate, params, 4096);
    let input = vec![impulse(frames, 0, 1.0), impulse(frames, 0, 1.0)];
    stream_at(node.as_mut(), rate, &input, 4096, &Extras::default())
}

// Spec: corpus R4
#[test]
fn tune_and_fine_set_the_fundamental_within_a_cent_from_40_hz_to_4_khz_at_three_rates() {
    let long = [
        (DECAY, 100.0),
        (MATERIAL, 100.0),
        (HIT, 30.0),
        (POS_L, 30.0),
    ];
    for rate in [
        SampleRate::HZ_44100,
        SampleRate::HZ_48000,
        SampleRate::HZ_96000,
    ] {
        for tune in [40.0, 100.0, 440.0, 1_000.0, 4_000.0] {
            let mut params = long.to_vec();
            params.push((TUNE, tune));
            let out = ring(rate, &params, 2.5);
            let r = hz(rate);
            let window = &out[0][(0.2 * r) as usize..(2.4 * r) as usize];
            let heard = peak(window, r, tune, 0.03);
            assert!(
                cents(heard, tune).abs() < 1.0,
                "{tune} Hz at {r}: heard {heard}"
            );
        }
    }
    for fine in [-50.0, 25.0, 50.0] {
        let mut params = long.to_vec();
        params.extend([(TUNE, 440.0), (FINE, fine)]);
        let out = ring(SampleRate::HZ_48000, &params, 2.5);
        let window = &out[0][9_600..115_200];
        let wanted = 440.0 * (fine / 1_200.0f64).exp2();
        let heard = peak(window, 48_000.0, wanted, 0.03);
        assert!(
            cents(heard, wanted).abs() < 1.0,
            "Fine {fine}: heard {heard}"
        );
    }
}

// Spec: corpus R4, R5
#[test]
fn spread_detunes_the_resonators_and_width_mixes_them() {
    let base = [
        (DECAY, 100.0),
        (MATERIAL, 100.0),
        (HIT, 30.0),
        (POS_L, 30.0),
        (POS_R, 30.0),
        (TUNE, 440.0),
        (SPREAD, 100.0),
    ];
    let (low, high) = (
        440.0 * (-50.0f64 / 1_200.0).exp2(),
        440.0 * (50.0f64 / 1_200.0).exp2(),
    );
    let window = |x: &[f32]| x[9_600..115_200].to_vec();
    // At full Width each resonator is on its own side: the left down, the
    // right up.
    let out = ring(common::RATE, &base, 2.5);
    let (left, right) = (window(&out[0]), window(&out[1]));
    assert!(cents(peak(&left, 48_000.0, 440.0, 0.03), low).abs() < 1.0);
    assert!(cents(peak(&right, 48_000.0, 440.0, 0.03), high).abs() < 1.0);
    assert!(dft(&left, 48_000.0, low) > 10.0 * dft(&left, 48_000.0, high));
    assert!(dft(&right, 48_000.0, high) > 10.0 * dft(&right, 48_000.0, low));
    // At 0 Width both are in the centre.
    let mut centred = base.to_vec();
    centred.push((WIDTH, 0.0));
    let out = ring(common::RATE, &centred, 2.5);
    assert!(mismatch(&out[0], &out[1]) < 1e-6, "both channels alike");
    let both = window(&out[0]);
    assert!(dft(&both, 48_000.0, low) > 0.1 * dft(&left, 48_000.0, low));
    assert!(dft(&both, 48_000.0, high) > 0.1 * dft(&left, 48_000.0, low));
    // Spread at 0 tunes both alike.
    let mut alike = base.to_vec();
    alike.push((SPREAD, 0.0));
    let out = ring(common::RATE, &alike, 1.0);
    assert!(mismatch(&out[0], &out[1]) < 1e-6);
}

/// The fundamental heard in `from..to` seconds of `x` near `near`.
fn heard(x: &[f32], rate: SampleRate, from: f64, to: f64, near: f64) -> f64 {
    let r = hz(rate);
    peak(&x[(from * r) as usize..(to * r) as usize], r, near, 0.1)
}

// Spec: corpus R6
#[test]
fn the_lfo_moves_the_tuning_at_its_rate_free_and_synced() {
    // A Square LFO of 50 % moves the tuning an octave up for the first half
    // of its cycle and an octave down for the second.
    let body = [
        (DECAY, 100.0),
        (MATERIAL, 100.0),
        (HIT, 30.0),
        (POS_L, 30.0),
        (LFO_WAVE, 1.0),
        (LFO_AMOUNT, 50.0),
        (TUNE, 440.0),
    ];
    let check = |out: &[Vec<f32>], cycle: f64, what: &str| {
        for (from, to, wanted) in [
            (0.06, 0.40, 880.0),
            (0.56, 0.90, 220.0),
            (1.06, 1.40, 880.0),
        ] {
            let (from, to) = (from * cycle, to * cycle);
            let (from, to) = (from.min(0.95), to.min(0.95));
            let heard = heard(&out[0], common::RATE, from, to, wanted);
            assert!(
                cents(heard, wanted).abs() < 30.0,
                "{what}: {from}..{to} s heard {heard}, wanted {wanted}"
            );
        }
    };
    // Free: 2 Hz, a cycle every half second.
    let mut free = body.to_vec();
    free.push((LFO_HZ, 2.0));
    let long_input = vec![impulse(96_000, 0, 1.0), impulse(96_000, 0, 1.0)];
    let mut node = corpus(&free, 4096);
    let out = stream(node.as_mut(), &long_input, 4096, &Extras::default());
    check(&out, 0.5, "free");
    // Synced: a quarter note at 120 BPM is the same half second, and at 90
    // BPM two thirds of a second.
    let mut synced = body.to_vec();
    synced.extend([(LFO_MODE, 1.0), (LFO_DIVISION, 4.0)]);
    for bpm in [120.0, 90.0] {
        let tempo = TempoMap::constant(Bpm::new(bpm).unwrap());
        let mut node = corpus(&synced, 4096);
        let out = stream(
            node.as_mut(),
            &long_input,
            4096,
            &Extras {
                tempo: Some(&tempo),
                ..Extras::default()
            },
        );
        check(&out, 60.0 / bpm, &format!("synced at {bpm}"));
    }
}

// Spec: corpus R6
#[test]
fn phase_and_spin_move_the_resonators_lfos_apart_and_not_the_noise_waves() {
    let body = [
        (DECAY, 100.0),
        (MATERIAL, 100.0),
        (HIT, 30.0),
        (POS_L, 30.0),
        (POS_R, 30.0),
        (LFO_AMOUNT, 50.0),
        (LFO_WAVE, 1.0),
        (LFO_HZ, 2.0),
        (TUNE, 440.0),
    ];
    let input = vec![impulse(48_000, 0, 1.0), impulse(48_000, 0, 1.0)];
    let play = |extra: &[(ParamId, f64)]| {
        let mut params = body.to_vec();
        params.extend_from_slice(extra);
        let mut node = corpus(&params, 4096);
        stream(node.as_mut(), &input, 4096, &Extras::default())
    };
    // Phase 180: the right LFO is opposite: while the left is up an octave
    // the right is down.
    let out = play(&[(LFO_PHASE, 180.0)]);
    let l = heard(&out[0], common::RATE, 0.03, 0.2, 880.0);
    let r = heard(&out[1], common::RATE, 0.03, 0.2, 220.0);
    assert!(
        cents(l, 880.0).abs() < 30.0 && cents(r, 220.0).abs() < 30.0,
        "{l} {r}"
    );
    // Spin: the right LFO runs faster, so it is a cycle ahead in time.
    let out = play(&[(LFO_STEREO, 1.0), (LFO_SPIN, 50.0)]);
    assert!(mismatch(&out[0], &out[1]) > 0.1, "the channels part");
    // The noise waves ignore both.
    for (wave, extra) in [(5.0, 180.0), (6.0, 180.0)] {
        let alike = play(&[(LFO_WAVE, wave), (LFO_PHASE, extra)]);
        let none = play(&[(LFO_WAVE, wave)]);
        assert_eq!(alike, none, "wave {wave}");
        let spun = play(&[(LFO_WAVE, wave), (LFO_STEREO, 1.0), (LFO_SPIN, 50.0)]);
        assert_eq!(spun, none, "wave {wave}");
    }
}

// Spec: corpus R7
#[test]
fn the_input_filter_passes_its_band_and_cuts_the_rest_before_the_resonators() {
    // A beam tuned to 1 kHz rings loudly to a 1 kHz sine; with the filter
    // at 4 kHz, half an octave wide, it hears almost none of it.
    // The sine fades in over a quarter of a second, so no start-up transient
    // rings the beam.
    let mut sine = common::sine(1_000.0, 0.5, 48_000);
    for (i, v) in sine.iter_mut().enumerate().take(12_000) {
        *v *= i as f32 / 12_000.0;
    }
    let input = vec![sine.clone(), sine];
    // Gain is low enough that the limiter holds nothing back.
    let base = [
        (DECAY, 100.0),
        (MATERIAL, 100.0),
        (TUNE, 1_000.0),
        (GAIN, 0.02),
    ];
    let play = |extra: &[(ParamId, f64)]| {
        let mut params = base.to_vec();
        params.extend_from_slice(extra);
        let mut node = corpus(&params, 4096);
        stream(node.as_mut(), &input, 4096, &Extras::default())
    };
    let at_1k = |out: &[Vec<f32>]| dft(&out[0][24_000..], 48_000.0, 1_000.0);
    let open = at_1k(&play(&[]));
    let cut = at_1k(&play(&[
        (FILTER_ON, 1.0),
        (FILTER_FREQ, 4_000.0),
        (FILTER_WIDTH, 0.5),
    ]));
    let kept = at_1k(&play(&[
        (FILTER_ON, 1.0),
        (FILTER_FREQ, 1_000.0),
        (FILTER_WIDTH, 2.0),
    ]));
    assert!(open > 1e-3, "it rings: {open}");
    assert!(cut < 0.15 * open, "cut to {cut} from {open}");
    assert!(
        (kept / open - 1.0).abs() < 0.06,
        "the band passes: {kept} against {open}"
    );
}

// Spec: corpus R8
#[test]
fn bleed_mixes_the_input_in_and_dry_wet_balances_it_and_neither_touches_a_pipe() {
    let input = vec![common::noise(4_096), common::noise(4_096)];
    let play = |kind: f64, extra: &[(ParamId, f64)]| {
        let mut params = vec![(TYPE, kind)];
        params.extend_from_slice(extra);
        let mut node = corpus(&params, 4_096);
        stream(node.as_mut(), &input, 4_096, &Extras::default())
    };
    // With the resonators silent (Gain at −∞), Bleed is all that is left.
    let half = play(0.0, &[(GAIN, 0.0), (BLEED, 50.0)]);
    for (out, wanted) in half[0].iter().zip(&input[0]) {
        assert!((out - 0.5 * wanted).abs() < 1e-6);
    }
    // A pipe has no Bleed.
    let pipe = play(5.0, &[(GAIN, 0.0), (BLEED, 100.0)]);
    assert!(pipe[0].iter().all(|&v| v == 0.0));
    // Dry/Wet 0 is the dry signal.
    let dry = play(0.0, &[(DRY_WET, 0.0)]);
    assert_eq!(dry[0], input[0]);
    // 50 % is half of each.
    let wet = play(0.0, &[]);
    let mix = play(0.0, &[(DRY_WET, 50.0)]);
    for ((m, w), d) in mix[0].iter().zip(&wet[0]).zip(&input[0]) {
        assert!((m - 0.5 * (w + d)).abs() < 1e-6);
    }
}

// Spec: corpus R8
#[test]
fn the_limiter_holds_the_resonators_at_zero_dbfs_and_its_lamp_lights_while_it_reduces() {
    let input = vec![common::noise(9_600), common::noise(9_600)];
    let loud = |gain: f64| {
        let mut node = corpus(&[(GAIN, gain), (DECAY, 100.0), (MATERIAL, 100.0)], 512);
        let out = stream(node.as_mut(), &input, 512, &Extras::default());
        (out, node.gain_reduction())
    };
    // +24 dB (15.85 times) into a ringing body would clip many times over.
    let (out, reduction) = loud(15.849);
    let peak = out.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak <= 1.0, "no sample passes 0 dBFS: {peak}");
    assert!(peak > 0.9, "the limiter holds it there: {peak}");
    assert!(reduction < -0.5, "the lamp: {reduction} dB");
    // Quietly, it does nothing.
    let (quiet, reduction) = loud(0.001);
    assert_eq!(reduction, 0.0);
    assert!(quiet.iter().flatten().all(|v| v.abs() < 1.0));
    // The display reads the lamp.
    let lit = corpus::display(
        bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()),
        Some(&[-3.0]),
    );
    assert!(lit.readings[0].value < corpus::LAMP_DB);
    let dark = corpus::display(
        bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()),
        Some(&[0.0]),
    );
    assert!(dark.readings[0].value >= corpus::LAMP_DB);
}

/// A note-on of `key` on frame `offset`.
fn on(offset: u32, key: u8) -> NoteEvent {
    common::note_on(offset, key, 1.0, None)
}

fn off(offset: u32, key: u8) -> NoteEvent {
    NoteEvent {
        kind: NoteKind::Off,
        ..common::note_on(offset, key, 0.0, None)
    }
}

fn bend(offset: u32, value: f32) -> NoteEvent {
    NoteEvent {
        kind: NoteKind::PitchBend { value },
        ..common::note_on(offset, 0, 0.0, None)
    }
}

/// The frequency of key `key` in twelve equal steps from A3 = 440 Hz at
/// key 69.
fn key_hz(key: f64) -> f64 {
    440.0 * ((key - 69.0) / 12.0).exp2()
}

/// A beam struck once and left to ring, with the sidechain's Frequency on.
fn played(params: &[(ParamId, f64)], notes: &[NoteEvent], seconds: f64, block: usize) -> Vec<f32> {
    let mut all = vec![
        (FREQUENCY, 1.0),
        (DECAY, 100.0),
        (MATERIAL, 100.0),
        (HIT, 30.0),
        (POS_L, 30.0),
        (TUNE, 330.0),
    ];
    all.extend_from_slice(params);
    let frames = (seconds * 48_000.0) as usize;
    let input = vec![impulse(frames, 0, 1.0), impulse(frames, 0, 1.0)];
    let mut node = corpus(&all, block.max(64));
    let extras = Extras {
        notes,
        ..Extras::default()
    };
    stream(node.as_mut(), &input, block, &extras).swap_remove(0)
}

// Spec: corpus R9
#[test]
fn a_note_retunes_the_body_on_its_frame_at_any_block_size() {
    // Key 60 is C3, 261.63 Hz; before it the body rings at Tune.
    let note_at = 24_001;
    let render = |block: usize| played(&[], &[on(note_at, 60)], 2.0, block);
    let reference = render(512);
    let untouched = played(&[], &[], 2.0, 512);
    let first = (0..untouched.len())
        .find(|&i| reference[i] != untouched[i])
        .expect("the note changes the sound");
    assert_eq!(first, note_at as usize, "the first frame the note changes");
    for block in [1, 64, 4096] {
        assert_eq!(render(block), reference, "blocks of {block}");
    }
    let before = peak(&reference[4_800..24_000], 48_000.0, 330.0, 0.05);
    assert!(
        cents(before, 330.0).abs() < 2.0,
        "Tune before the note: {before}"
    );
    let after = peak(&reference[28_800..76_800], 48_000.0, key_hz(60.0), 0.05);
    assert!(cents(after, key_hz(60.0)).abs() < 2.0, "{after} Hz");
    // Tune read as semitones, and Fine in cents.
    let up = played(
        &[(TUNE_SEMITONES, 12.0), (FINE, 25.0)],
        &[on(1_000, 60)],
        1.5,
        512,
    );
    let wanted = key_hz(72.0) * (25.0f64 / 1_200.0).exp2();
    assert!(cents(peak(&up[9_600..60_000], 48_000.0, wanted, 0.05), wanted).abs() < 2.0);
    // Without Frequency the notes change nothing.
    let plain = played(&[(FREQUENCY, 0.0)], &[on(1_000, 60)], 1.0, 512);
    assert_eq!(plain, played(&[(FREQUENCY, 0.0)], &[], 1.0, 512));
}

// Spec: corpus R9, R10
#[test]
fn last_and_low_choose_among_held_notes_and_a_release_moves_to_the_next() {
    // A chord of C3, E3 and G3, then G3 released, then the rest.
    let chord = [
        on(0, 60),
        on(100, 64),
        on(200, 67),
        off(48_000, 67),
        off(96_000, 64),
    ];
    for (low, expected) in [(0.0, [67.0, 64.0, 60.0]), (1.0, [60.0, 60.0, 60.0])] {
        let out = played(&[(PRIORITY, low)], &chord, 3.0, 512);
        for (n, key) in expected.iter().enumerate() {
            let from = n * 48_000 + 9_600;
            let heard = peak(&out[from..from + 28_800], 48_000.0, key_hz(*key), 0.05);
            assert!(
                cents(heard, key_hz(*key)).abs() < 3.0,
                "{} priority, stretch {n}: {heard} Hz, key {key}",
                if low == 1.0 { "Low" } else { "Last" }
            );
        }
    }
    // With the last note released, a body rings on at its pitch.
    let out = played(&[], &[on(0, 64), off(24_000, 64)], 2.0, 512);
    let heard = peak(&out[48_000..96_000], 48_000.0, key_hz(64.0), 0.05);
    assert!(cents(heard, key_hz(64.0)).abs() < 3.0, "{heard}");
}

// Spec: corpus R9
#[test]
fn pitch_bend_moves_the_tuning_over_its_range_at_once() {
    let out = played(
        &[(PB_RANGE, 7.0)],
        &[on(0, 60), bend(48_000, 1.0), bend(96_000, -0.5)],
        3.0,
        512,
    );
    for (n, semitones) in [0.0, 7.0, -3.5].into_iter().enumerate() {
        let from = n * 48_000 + 9_600;
        let wanted = key_hz(60.0 + semitones);
        let heard = peak(&out[from..from + 28_800], 48_000.0, wanted, 0.05);
        assert!(
            cents(heard, wanted).abs() < 3.0,
            "bend {semitones}: {heard} against {wanted}"
        );
    }
    // A range of 0 turns the bend off.
    let still = played(
        &[(PB_RANGE, 0.0)],
        &[on(0, 60), bend(24_000, 1.0)],
        1.0,
        512,
    );
    assert_eq!(still, played(&[(PB_RANGE, 0.0)], &[on(0, 60)], 1.0, 512));
}

// Spec: corpus R10
#[test]
fn off_decay_damps_the_body_on_a_note_off_and_without_it_the_body_rings_on() {
    let level = |x: &[f32], from: f64, to: f64| {
        rms(&x[(from * 48_000.0) as usize..(to * 48_000.0) as usize])
    };
    let run = |on_off: f64, amount: f64| {
        played(
            &[(OFF_DECAY, on_off), (OFF_AMOUNT, amount)],
            &[on(0, 60), off(48_000, 60)],
            3.0,
            512,
        )
    };
    let ringing = run(0.0, 100.0);
    let before = level(&ringing, 0.5, 1.0);
    let after = level(&ringing, 1.5, 2.0);
    assert!(after > 0.7 * before, "rings on: {after} against {before}");
    // 100 % stops it within 50 ms.
    let stopped = run(1.0, 100.0);
    assert!(
        level(&stopped, 1.1, 1.2) < 0.01 * before,
        "{}",
        level(&stopped, 1.1, 1.2)
    );
    // 0 % leaves the decay alone.
    let left = run(1.0, 0.0);
    assert!(level(&left, 1.5, 2.0) > 0.7 * before);
    // 50 % sits between.
    let half = run(1.0, 50.0);
    let middle = level(&half, 1.5, 2.0) / before;
    assert!((0.005..0.5).contains(&middle), "{middle}");
    // The next note undoes the damping: struck again, the body rings long.
    let mut input = vec![impulse(144_000, 0, 1.0), impulse(144_000, 0, 1.0)];
    for channel in &mut input {
        channel[48_001] = 1.0;
    }
    let mut node = corpus(
        &[
            (FREQUENCY, 1.0),
            (DECAY, 100.0),
            (MATERIAL, 100.0),
            (HIT, 30.0),
            (POS_L, 30.0),
            (OFF_DECAY, 1.0),
            (OFF_AMOUNT, 100.0),
        ],
        512,
    );
    let notes = [on(0, 60), off(24_000, 60), on(48_000, 64)];
    let again = stream(
        node.as_mut(),
        &input,
        512,
        &Extras {
            notes: &notes,
            ..Extras::default()
        },
    );
    assert!(
        level(&again[0], 0.9, 0.99) < 1e-4 * level(&again[0], 0.2, 0.45),
        "damped"
    );
    let struck = peak(&again[0][52_800..100_000], 48_000.0, key_hz(64.0), 0.05);
    assert!(cents(struck, key_hz(64.0)).abs() < 3.0, "{struck}");
    assert!(
        level(&again[0], 2.5, 2.9) > 0.1 * level(&again[0], 1.05, 1.15),
        "rings long again"
    );
}

// Spec: corpus R12, R13
#[test]
fn changing_the_type_ratio_inharm_or_quality_while_it_runs_allocates_nothing_and_stays_finite() {
    let frames = 24_000;
    let input = vec![common::noise(frames), common::noise(frames)];
    let changes = [
        (1_000, TYPE, 4.0),
        (2_000, RATIO, 20.0),
        (3_000, INHARM, 60.0),
        (4_000, QUALITY, 0.0),
        (5_000, TYPE, 2.0),
        (6_000, TYPE, 5.0),
        (7_000, OPENING, 20.0),
        (8_000, TYPE, 6.0),
        (9_000, TYPE, 3.0),
        (10_000, RATIO, 80.0),
        (11_000, TYPE, 0.0),
    ];
    let events: Vec<NodeEvent> = changes
        .iter()
        .map(|&(at, param, value)| common::set(at, param, value))
        .collect();
    let mut node = corpus(&[(DECAY, 70.0)], 512);
    let out = stream(
        node.as_mut(),
        &input,
        512,
        &Extras {
            events: &events,
            ..Extras::default()
        },
    );
    assert!(
        out.iter()
            .flatten()
            .all(|v| v.is_finite() && v.abs() <= 1.0)
    );
    assert!(rms(&out[0][12_000..]) > 1e-4, "it still rings");
}

// Spec: corpus R14
#[test]
fn renders_are_the_same_at_every_block_size_and_every_run() {
    let input = vec![common::noise(24_000), common::noise(24_000)];
    let notes = [
        on(1_001, 60),
        on(5_003, 67),
        off(9_001, 60),
        bend(11_111, 0.5),
        off(15_000, 67),
    ];
    for kind in 0..7 {
        let params = [
            (TYPE, f64::from(kind)),
            (FREQUENCY, 1.0),
            (SPREAD, 40.0),
            (LFO_AMOUNT, 30.0),
            (LFO_WAVE, 5.0),
            (FILTER_ON, 1.0),
            (BLEED, 20.0),
            (OFF_DECAY, 1.0),
            (INHARM, 30.0),
            (RATIO, 63.0),
        ];
        let mut first: Option<Vec<Vec<f32>>> = None;
        for block in BLOCKS.into_iter().chain([512]) {
            let mut node = corpus(&params, 4096);
            let out = stream(
                node.as_mut(),
                &input,
                block,
                &Extras {
                    notes: &notes,
                    ..Extras::default()
                },
            );
            match &first {
                None => first = Some(out),
                Some(first) => assert_eq!(&out, first, "type {kind}, blocks of {block}"),
            }
        }
    }
}

// Spec: corpus R17
#[test]
fn a_set_with_corpus_saves_and_reopens_with_every_parameter_and_its_midi_source() {
    use bl_model::{Command, Document, Editor, Tap, TrackKind};
    let mut editor = Editor::new();
    let mut track = |kind| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        id
    };
    let (audio, keys) = (track(TrackKind::Audio), track(TrackKind::Midi));
    let device_type = corpus_type();
    let id = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track: audio,
            index: 0,
            device: device_type.device(id),
        })
        .unwrap();
    let mut moved = Vec::new();
    for p in &device_type.description.params {
        let value = p.clamp(p.min() + 0.3 * (p.max() - p.min()));
        editor
            .apply(Command::SetDeviceParam {
                device: id,
                param: p.id,
                value,
            })
            .unwrap();
        moved.push((p.id, value));
    }
    editor
        .apply(Command::SetMidiSidechainSource {
            device: id,
            track: Some(keys),
            tap: Some(Tap::PostMixer),
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let reopened: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(&reopened, editor.document());
    let device = reopened.device(id).unwrap();
    for (param, value) in moved {
        assert_eq!(device.params.get(&param), Some(&value), "{param:?}");
    }
    let sidechain = device.midi_sidechain.as_ref().unwrap();
    assert_eq!(
        (sidechain.source, sidechain.tap),
        (Some(keys), Some(Tap::PostMixer))
    );
}

/// A song of a MIDI track playing `key` from beat 1 (frame 24,000 at 120
/// BPM) and an audio track of a constant into Corpus, whose MIDI source is
/// that track with `tap`; the audio track's left channel at Main's output for
/// `frames` frames in blocks of `block`.
fn song(block: usize, with_note: bool, tap: &str, up: Option<i64>, frames: usize) -> Vec<f32> {
    use bl_engine::testing::{DC, SEMITONES, TRANSPOSE, VALUE};
    use bl_model::{Command, Device, DeviceKind, DeviceRole, TrackKind};
    let mut chain = common::chain::Chain::new(block.max(64));
    if let Some(up) = up {
        let id = chain.editor.new_id();
        let mut transpose = Device::new(
            id,
            "Up",
            DeviceKind::Native {
                device_type: TRANSPOSE.to_owned(),
            },
        )
        .with_role(DeviceRole::MidiEffect);
        transpose.params.insert(SEMITONES, up as f64);
        chain
            .editor
            .apply(Command::InsertDevice {
                track: chain.track,
                index: 0,
                device: transpose,
            })
            .unwrap();
    }
    if with_note {
        chain.clip(4, [common::chain::note(60, 1, 2)]);
    }
    let audio = chain.editor.new_id();
    chain
        .editor
        .apply(Command::CreateTrack {
            id: audio,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 1,
        })
        .unwrap();
    let make = |chain: &mut common::chain::Chain, device_type: &str, params: &[(ParamId, f64)]| {
        let id = chain.editor.new_id();
        let mut device = Device::new(
            id,
            "",
            DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        );
        for &(param, value) in params {
            device.params.insert(param, value);
        }
        (id, device)
    };
    let (_, dc) = make(&mut chain, DC, &[(VALUE, 0.5)]);
    let (corpus, device) = make(
        &mut chain,
        corpus::CORPUS,
        &[
            (FREQUENCY, 1.0),
            (DECAY, 100.0),
            (MATERIAL, 100.0),
            (HIT, 30.0),
            (POS_L, 30.0),
            (TUNE, 330.0),
        ],
    );
    let device = Device {
        midi_sidechain: corpus_type().device(corpus).midi_sidechain,
        ..device
    };
    for (index, device) in [dc, device].into_iter().enumerate() {
        chain
            .editor
            .apply(Command::InsertDevice {
                track: audio,
                index,
                device,
            })
            .unwrap();
    }
    chain
        .editor
        .apply(Command::SetMidiSidechainSource {
            device: corpus,
            track: Some(chain.track),
            tap: Some(match tap {
                "pre_fx" => bl_model::Tap::PreFx,
                "post_fx" => bl_model::Tap::PostFx,
                _ => bl_model::Tap::PostMixer,
            }),
        })
        .unwrap();
    chain.sync();
    chain.play();
    let mut out = Vec::new();
    while out.len() < frames {
        let n = block.min(frames - out.len());
        let processor = &mut chain.processor;
        bl_rt::rt_test!({ processor.process(n) });
        out.extend_from_slice(chain.processor.output().channel(0));
    }
    out
}

// Spec: routing R32, R39; corpus R9, R11
#[test]
fn a_midi_tracks_note_retunes_corpus_through_its_sidechain_on_its_frame_at_any_block_size() {
    let frames = 96_000;
    let silent = song(512, false, "pre_fx", None, frames);
    let mut reference: Option<Vec<f32>> = None;
    for block in [1, 64, 512] {
        let out = song(block, true, "pre_fx", None, frames);
        let first = (0..frames)
            .find(|&i| out[i] != silent[i])
            .expect("the note changes the sound");
        assert_eq!(first, 24_000, "blocks of {block}: the note's frame");
        match &reference {
            None => reference = Some(out),
            Some(reference) => assert_eq!(&out, reference, "blocks of {block}"),
        }
    }
    let out = reference.unwrap();
    let before = peak(&out[4_800..24_000], 48_000.0, 330.0, 0.05);
    assert!(
        cents(before, 330.0).abs() < 3.0,
        "Tune before the note: {before}"
    );
    let after = peak(&out[30_000..70_000], 48_000.0, key_hz(60.0), 0.05);
    assert!(cents(after, key_hz(60.0)).abs() < 3.0, "{after} Hz");
}

// Spec: routing R39; corpus R11
#[test]
fn a_post_fx_tap_hears_the_notes_after_the_midi_effects_and_pre_fx_before_them() {
    let frames = 96_000;
    // A MIDI effect that moves the note up an octave: Pre FX hears key 60,
    // Post FX and Post Mixer key 72.
    for (tap, key) in [("pre_fx", 60.0), ("post_fx", 72.0), ("post_mixer", 72.0)] {
        let out = song(512, true, tap, Some(12), frames);
        let heard = peak(&out[30_000..70_000], 48_000.0, key_hz(key), 0.05);
        assert!(
            cents(heard, key_hz(key)).abs() < 3.0,
            "{tap}: {heard} Hz, wanted key {key}"
        );
    }
}
