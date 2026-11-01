//! Meld, part 1 (WP-8.17; the meld spec, R1 to R23): the ten oscillator types
//! at their pitches and macros, the engines, filter, envelope and Mix, the
//! voice settings, scale awareness, tunings, the voice budget and block-size
//! independence.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common::{CHANNELS, RATE, device, node};
use std::time::Instant;

use bl_devices::meld::{
    ATTACK, CENTS, DECAY, DRIVE, FILTER_FREQ, FILTER_MACRO_1, FILTER_MACRO_2, FILTER_ON,
    FILTER_TYPE, GLIDE_MODE, GLIDE_TIME, KEY_TRACKING, LEGATO, LIMIT, MACRO_1, MACRO_2, MELD, MODE,
    OCTAVES, ON, OSC_SCALE, OUTPUT, PAN, PHASE_RESET, PHASE_SPREAD, RELEASE, SEMITONES, SPREAD,
    STACK, SUSTAIN, TONE, TYPE, USE_SCALE, VOICES, VOLUME, param,
};
use bl_engine::scales::ScaleRuns;
use bl_engine::{
    Block, Collector, DeviceType, KeyPitches, Node, NoteBuffer, NoteEvent, NoteKind, Registry,
    TransportInfo,
};
use bl_model::{DeviceKind, ParamId, Scale, ScaleName, Tuning};
use realfft::RealFftPlanner;

/// The sample rate the tests run at, in hertz.
const HZ: f64 = 48_000.0;

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

fn meld_type() -> DeviceType {
    bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == MELD))
        .expect("Meld is registered")
}

/// Engine A's parameter `offset`.
fn a(offset: u32) -> ParamId {
    param(0, offset)
}

/// Engine B's parameter `offset`.
fn b(offset: u32) -> ParamId {
    param(1, offset)
}

/// A device with Meld's filters open, the limiter off and the levels at
/// 0 dB, so a test reads the oscillator itself, with `params` on top.
fn meld_with(params: &[(ParamId, f64)]) -> bl_model::Device {
    let mut all = vec![
        (a(FILTER_ON), 0.0),
        (b(FILTER_ON), 0.0),
        (LIMIT, 0.0),
        (a(VOLUME), 1.0),
        (b(VOLUME), 1.0),
        (OUTPUT, 1.0),
        (a(ATTACK), 0.0),
        (b(ATTACK), 0.0),
    ];
    all.extend_from_slice(params);
    crate::common::device(&meld_type(), &all)
}

fn event(offset: usize, kind: NoteKind, key: u8) -> NoteEvent {
    NoteEvent {
        offset: offset as u32,
        kind,
        channel: 0,
        key,
        velocity: 1.0,
        note_id: None,
    }
}

fn on(offset: usize, key: u8) -> NoteEvent {
    event(offset, NoteKind::On, key)
}

fn off(offset: usize, key: u8) -> NoteEvent {
    event(offset, NoteKind::Off, key)
}

/// What a render plays through: the set's tuning, and its scale from
/// frame 0 with the changes that follow it.
#[derive(Clone, Default)]
struct Setting {
    tuning: Option<Tuning>,
    scales: Vec<(usize, Scale)>,
}

fn scaled(scale: Scale) -> Setting {
    Setting {
        scales: vec![(0, scale)],
        ..Setting::default()
    }
}

fn c_major() -> Scale {
    Scale::new(0, ScaleName::Major)
}

/// Plays `notes` (frames from the start) through `node` for `frames`, in
/// blocks of `block`, and returns both channels.
fn render_node(
    node: &mut dyn Node,
    block: usize,
    frames: usize,
    notes: &[NoteEvent],
    setting: &Setting,
) -> [Vec<f32>; 2] {
    let collector = Collector::new();
    node.receive_data(
        collector
            .handle()
            .data(KeyPitches::of(setting.tuning.as_ref())),
    );
    let scale_at = |frame: usize| {
        setting
            .scales
            .iter()
            .rev()
            .find(|(from, _)| *from <= frame)
            .map_or_else(Scale::default, |(_, scale)| *scale)
    };
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut start = 0;
    while start < frames {
        let len = block.min(frames - start);
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= start && (n.offset as usize) < start + len)
            .map(|n| NoteEvent {
                offset: n.offset - start as u32,
                ..*n
            })
            .collect();
        let mut runs = ScaleRuns::of(scale_at(start));
        for (at, scale) in &setting.scales {
            if *at > start && *at < start + len {
                runs.push((*at - start) as u32, *scale);
            }
        }
        let mut data = vec![0.0f32; CHANNELS * len];
        let mut sink = NoteBuffer::with_capacity(block_notes.len().max(1));
        {
            let mut audio = Block::new(
                &mut data,
                CHANNELS,
                len,
                len,
                &[],
                TransportInfo::default(),
                RATE,
            )
            .with_notes(&block_notes, &mut sink)
            .with_set_scale(scale_at(start))
            .with_scale(runs);
            if setting.tuning.is_some() {
                audio = audio.with_scales_hidden(false);
            }
            bl_rt::rt_test!({ node.process(&mut audio) });
        }
        out[0].extend_from_slice(&data[..len]);
        out[1].extend_from_slice(&data[len..]);
        start += len;
    }
    out
}

/// A Meld with `params` set, rendered at block size `block`.
fn play_with(
    params: &[(ParamId, f64)],
    block: usize,
    frames: usize,
    notes: &[NoteEvent],
    setting: &Setting,
) -> [Vec<f32>; 2] {
    let mut node = node(&registry(), &meld_with(params), 4_096);
    render_node(node.as_mut(), block, frames, notes, setting)
}

/// As [`play_with`] at 512 frames a block, with nothing special in the set.
fn play(params: &[(ParamId, f64)], frames: usize, notes: &[NoteEvent]) -> [Vec<f32>; 2] {
    play_with(params, 512, frames, notes, &Setting::default())
}

/// A second of `key` with `params`, and its left channel's last half
/// second: a steady tone.
fn steady(params: &[(ParamId, f64)], key: u8) -> Vec<f32> {
    let [left, _] = play(params, 48_000, &[on(0, key)]);
    left[24_000..].to_vec()
}

/// The frequency of a pitch in semitones, A4 at 440 Hz.
fn hz(pitch: f64) -> f64 {
    440.0 * ((pitch - 69.0) / 12.0).exp2()
}

/// The amplitude of a sine at `frequency` in `signal`, by a Hann-windowed
/// DFT over the whole slice.
fn amp(signal: &[f32], frequency: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n).cos();
        let phase = core::f64::consts::TAU * frequency * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
    }
    4.0 * re.hypot(im) / n
}

/// The root mean square of `signal`.
fn rms(signal: &[f32]) -> f64 {
    (signal
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / signal.len() as f64)
        .sqrt()
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, x| m.max(x.abs()))
}

/// The power-weighted mean frequency of `signal` between `low` and `high`,
/// in hertz: where a tone, or a tight swarm of them, sits.
fn centroid(signal: &[f32], low: f64, high: f64) -> f64 {
    let n = signal.len().next_power_of_two() / 2;
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    for (i, slot) in input.iter_mut().enumerate() {
        let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n as f64).cos();
        *slot = f64::from(signal[i]) * w;
    }
    fft.process(&mut input, &mut spectrum).unwrap();
    let bin = HZ / n as f64;
    let (mut sum, mut weight) = (0.0, 0.0);
    for (k, value) in spectrum.iter().enumerate() {
        let f = k as f64 * bin;
        if (low..high).contains(&f) {
            sum += f * value.norm_sqr();
            weight += value.norm_sqr();
        }
    }
    sum / weight
}

/// The share of `signal`'s power outside `bands` (pairs of frequencies).
fn power_outside(signal: &[f32], bands: &[(f64, f64)]) -> f64 {
    let n = signal.len().next_power_of_two() / 2;
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    for (i, slot) in input.iter_mut().enumerate() {
        let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n as f64).cos();
        *slot = f64::from(signal[i]) * w;
    }
    fft.process(&mut input, &mut spectrum).unwrap();
    let bin = HZ / n as f64;
    let (mut outside, mut total) = (0.0, 0.0);
    for (k, value) in spectrum.iter().enumerate().skip(1) {
        let f = k as f64 * bin;
        total += value.norm_sqr();
        if !bands.iter().any(|(low, high)| (*low..*high).contains(&f)) {
            outside += value.norm_sqr();
        }
    }
    outside / total
}

/// Asserts that two renders are the same bits, naming the first sample
/// that differs.
fn assert_same(actual: &[f32], expected: &[f32], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length");
    if let Some(i) = (0..actual.len()).find(|&i| actual[i].to_bits() != expected[i].to_bits()) {
        panic!(
            "{what}: sample {i} is {} against {}",
            actual[i], expected[i]
        );
    }
}

/// 19 equal steps to the octave, with the default reference and range.
fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".into(), String::new(), cents).unwrap()
}

// Spec: meld R1, R2, R4
#[test]
fn meld_is_an_instrument_with_two_engines_and_twenty_five_types() {
    let device_type = meld_type();
    assert_eq!(device_type.name, "Meld");
    let description = &device_type.description;
    description.check().expect("a sound description");
    for engine in 0..2 {
        for offset in [ON, TYPE, MACRO_1, MACRO_2, FILTER_TYPE, ATTACK, VOLUME] {
            assert!(description.param(param(engine, offset)).is_some());
        }
    }
    // A is on and B off in a new device; Basic Shapes in both.
    assert_eq!(description.param(a(ON)).unwrap().default, 1.0);
    assert_eq!(description.param(b(ON)).unwrap().default, 0.0);
    assert_eq!(description.param(a(TYPE)).unwrap().default, 0.0);
    // The chooser holds all twenty-five types in the manual's order, and
    // the layout holds the engine selector's two pages.
    let bl_model::ParamKind::Choice { choices } = &description.param(a(TYPE)).unwrap().kind else {
        panic!("Type is a chooser");
    };
    assert_eq!(choices.len(), 25);
    assert_eq!(choices[0], "Basic Shapes");
    assert_eq!(choices[24], "Chord");
    let pages = description.layout.pages();
    assert!(pages.iter().any(|p| p.name == "Engine" && p.pages.len() == 2));
    assert_eq!(description.title_switch, Some(USE_SCALE));
}

// Spec: meld R5
#[test]
fn the_macros_keep_their_ids_and_values_whatever_the_type() {
    let description = meld_type().description;
    // The IDs are the engine's base and a fixed offset: they never depend on
    // a type.
    assert_eq!(a(MACRO_1), ParamId(5));
    assert_eq!(b(MACRO_2), ParamId(106));
    // A macro keeps its value across a type change.
    let first = steady(&[(a(TYPE), 0.0), (a(MACRO_1), 100.0)], 57);
    let again = steady(&[(a(TYPE), 5.0), (a(MACRO_1), 100.0), (a(MACRO_2), 0.0)], 57);
    assert!(rms(&first) > 0.1 && rms(&again) > 0.1);
    assert!(description.param(a(MACRO_1)).unwrap().default == 0.0);
}

// Spec: meld R7 to R11 (each type plays at its pitch)
#[test]
fn each_of_the_ten_types_plays_at_its_pitch() {
    for kind in 0..10u32 {
        // Square 5th at a P Width of 50 % has a strong fundamental; the
        // others play at their macros' zeros.
        let mut params = vec![(a(TYPE), f64::from(kind))];
        if kind == 4 {
            params.push((a(MACRO_2), 50.0));
        }
        for key in [45u8, 69, 93] {
            let signal = steady(&params, key);
            assert!(rms(&signal) > 0.01, "type {kind} sounds at key {key}");
            let expected = hz(f64::from(key));
            let measured = centroid(&signal, expected * 0.9, expected * 1.1);
            assert!(
                (measured / expected - 1.0).abs() < 0.002,
                "type {kind} at key {key} plays {measured:.2} Hz, not {expected:.2}"
            );
        }
    }
}

// Spec: meld R7
#[test]
fn shape_morphs_sine_triangle_saw_and_square_and_tone_narrows_the_duty_cycle() {
    // 220 Hz: a whole number of cycles in the window.
    let harmonics = |shape: f64, tone: f64| {
        let signal = steady(&[(a(MACRO_1), shape), (a(MACRO_2), tone)], 57);
        let one = amp(&signal, 220.0);
        (one, amp(&signal, 440.0) / one, amp(&signal, 660.0) / one)
    };
    let (sine, two, three) = harmonics(0.0, 0.0);
    assert!((sine - 1.0).abs() < 0.02, "a sine at full level: {sine}");
    assert!(two < 0.01 && three < 0.01);
    let (_, two, three) = harmonics(100.0 / 3.0, 0.0);
    assert!(two < 0.01, "a triangle has no even harmonics: {two}");
    assert!((three - 1.0 / 9.0).abs() < 0.01, "its third: {three}");
    let (_, two, three) = harmonics(200.0 / 3.0, 0.0);
    assert!((two - 0.5).abs() < 0.03, "a saw's second: {two}");
    assert!((three - 1.0 / 3.0).abs() < 0.03, "its third: {three}");
    let (square, two, three) = harmonics(100.0, 0.0);
    assert!((square - 4.0 / core::f64::consts::PI).abs() < 0.03);
    assert!(two < 0.01, "a square has no even harmonics: {two}");
    assert!((three - 1.0 / 3.0).abs() < 0.02);
    // Between sine and triangle, the neighbours crossfade.
    let (half, _, _) = harmonics(100.0 / 6.0, 0.0);
    let triangle = 8.0 / core::f64::consts::PI.powi(2);
    assert!((half - 0.5 * (1.0 + triangle)).abs() < 0.02, "{half}");
    // Tone narrows the square to a 5 % pulse: its second harmonic is cos(π d)
    // of its first.
    let (_, two, _) = harmonics(100.0, 100.0);
    assert!(
        (two - (core::f64::consts::PI * 0.05).cos()).abs() < 0.03,
        "a 5 % pulse: {two}"
    );
    // And it narrows a saw's duty cycle too; a sine and a triangle it leaves.
    let (_, saw_two, _) = harmonics(200.0 / 3.0, 100.0);
    assert!((saw_two - 0.5).abs() > 0.05, "tone changes the saw: {saw_two}");
    let (_, two, three) = harmonics(0.0, 100.0);
    assert!(two < 0.01 && three < 0.01, "tone leaves the sine");
}

// Spec: meld R7 (Noisy Shapes)
#[test]
fn rough_adds_phase_noise_from_the_seed_and_is_the_same_run_after_run() {
    let smooth = steady(&[(a(TYPE), 2.0), (a(MACRO_1), 0.0), (a(MACRO_2), 0.0)], 57);
    let plain = steady(&[(a(TYPE), 0.0), (a(MACRO_1), 0.0)], 57);
    let worst = smooth
        .iter()
        .zip(&plain)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1e-4, "no Rough is Basic Shapes: {worst}");
    let rough = steady(&[(a(TYPE), 2.0), (a(MACRO_1), 0.0), (a(MACRO_2), 100.0)], 57);
    let around = (210.0, 230.0);
    assert!(power_outside(&smooth, &[around]) < 1e-4);
    let spread = power_outside(&rough, &[around]);
    assert!(spread > 0.01, "noise spreads the line: {spread}");
    let again = steady(&[(a(TYPE), 2.0), (a(MACRO_1), 0.0), (a(MACRO_2), 100.0)], 57);
    assert_same(&rough, &again, "from its seed");
    // The phase wanders by up to a quarter cycle, not more: the tone stays.
    assert!(amp(&rough, 220.0) > 0.5, "the note stays");
}

// Spec: meld R8
#[test]
fn dual_basic_shapes_detunes_a_copy_by_cents_then_by_intervals() {
    let frames = 96_000;
    let render = |detune: f64, setting: &Setting, extra: &[(ParamId, f64)]| {
        let mut params = vec![(a(TYPE), 1.0), (a(MACRO_2), detune)];
        params.extend_from_slice(extra);
        let [left, _] = play_with(&params, 512, frames, &[on(0, 69)], setting);
        left[48_000..].to_vec()
    };
    let signal = render(0.0, &Setting::default(), &[]);
    assert!((amp(&signal, 440.0) - 1.0).abs() < 0.03, "two waves, one pitch");
    // Up to 50 cents: the second wave is 50 cents up at 50 %.
    let signal = render(50.0, &Setting::default(), &[]);
    let up = 440.0 * (50.0f64 / 1200.0).exp2();
    assert!((amp(&signal, 440.0) - 0.5).abs() < 0.05, "{}", amp(&signal, 440.0));
    assert!((amp(&signal, up) - 0.5).abs() < 0.05, "{}", amp(&signal, up));
    // Past 50 % it lands on semitones: 75 % is half an octave.
    let signal = render(75.0, &Setting::default(), &[]);
    assert!(amp(&signal, hz(75.0)) > 0.4, "six semitones up");
    let signal = render(100.0, &Setting::default(), &[]);
    assert!(amp(&signal, 880.0) > 0.4, "an octave up");
    // With the scale on, 75 % is four degrees of C major above A: E.
    let on_scale = [(USE_SCALE, 1.0)];
    let signal = render(75.0, &scaled(c_major()), &on_scale);
    assert!(amp(&signal, hz(76.0)) > 0.4, "four degrees up is E");
    assert!(amp(&signal, hz(75.0)) < 0.05);
    // Oscillator Scale Awareness off puts it back to semitones.
    let signal = render(75.0, &scaled(c_major()), &[(USE_SCALE, 1.0), (a(OSC_SCALE), 0.0)]);
    assert!(amp(&signal, hz(75.0)) > 0.4, "six semitones again");
}

// Spec: meld R9
#[test]
fn square_sync_and_square_5th_play_their_ratios() {
    // Both syncs at a ratio of 8: a square at eight times the note.
    let signal = steady(
        &[(a(TYPE), 3.0), (a(MACRO_1), 100.0), (a(MACRO_2), 100.0)],
        57,
    );
    let square = 4.0 / core::f64::consts::PI;
    assert!((amp(&signal, 8.0 * 220.0) - square).abs() < 0.05);
    assert!(amp(&signal, 220.0) < 0.05, "the note itself is gone");
    // One at 1 and one at 8, mixed equally.
    let signal = steady(&[(a(TYPE), 3.0), (a(MACRO_1), 0.0), (a(MACRO_2), 100.0)], 57);
    assert!((amp(&signal, 220.0) - 0.5 * square).abs() < 0.05);
    assert!((amp(&signal, 1_760.0) - 0.5 * square).abs() < 0.05);
    // A ratio of 2.5 (macro 21.4 %) has its energy at the note's own
    // multiples: a synced wave is periodic in the note.
    let signal = steady(&[(a(TYPE), 3.0), (a(MACRO_1), 100.0 * 1.5 / 7.0)], 57);
    assert!(power_outside(&signal, &(1..100).map(|h| (h as f64 * 220.0 - 8.0, h as f64 * 220.0 + 8.0)).collect::<Vec<_>>()) < 1e-3);
    // Square 5th: 5th Amt mixes in a square a fifth up.
    let level = |amount: f64| {
        let signal = steady(
            &[(a(TYPE), 4.0), (a(MACRO_1), amount), (a(MACRO_2), 50.0)],
            57,
        );
        (amp(&signal, 220.0), amp(&signal, 330.0))
    };
    let (note, fifth) = level(0.0);
    assert!(fifth < 0.01 && note > 1.0);
    let (note, fifth) = level(50.0);
    assert!((fifth / note - 0.5).abs() < 0.03, "{}", fifth / note);
    let (note, fifth) = level(100.0);
    assert!((fifth / note - 1.0).abs() < 0.03);
    // P Width: at 50 % no even harmonics, narrower pulses have them.
    let wide = steady(&[(a(TYPE), 4.0), (a(MACRO_2), 50.0)], 57);
    let narrow = steady(&[(a(TYPE), 4.0), (a(MACRO_2), 0.0)], 57);
    assert!(amp(&wide, 440.0) < 0.01);
    assert!(amp(&narrow, 440.0) > 0.5 * amp(&narrow, 220.0));
}

// Spec: meld R10
#[test]
fn sub_morphs_a_sine_into_a_square_and_adds_a_sine_below() {
    let tone = |tone: f64, aux: f64| {
        steady(&[(a(TYPE), 5.0), (a(MACRO_1), tone), (a(MACRO_2), aux)], 57)
    };
    let sine = tone(0.0, 0.0);
    assert!((amp(&sine, 220.0) - 1.0).abs() < 0.02 && amp(&sine, 660.0) < 0.01);
    let square = tone(100.0, 0.0);
    assert!((amp(&square, 660.0) / amp(&square, 220.0) - 1.0 / 3.0).abs() < 0.02);
    let both = tone(0.0, 100.0);
    assert!((amp(&both, 110.0) - amp(&both, 220.0)).abs() < 0.03, "an octave below");
    assert!(amp(&both, 110.0) > 0.6);
}

// Spec: meld R11
#[test]
fn the_swarms_spread_drift_and_fade_into_a_chord() {
    // A6, 1,760 Hz, where a cent is more than a hertz and a second's DFT
    // separates the copies.
    let f0 = 1_760.0;
    let swarm = |kind: f64, motion: f64, spacing: f64, setting: &Setting, extra: &[(ParamId, f64)]| {
        let mut params = vec![
            (a(TYPE), kind),
            (a(MACRO_1), motion),
            (a(MACRO_2), spacing),
        ];
        params.extend_from_slice(extra);
        let [left, _] = play_with(&params, 512, 96_000, &[on(0, 93)], setting);
        left[48_000..].to_vec()
    };
    let cents = |c: f64| f0 * (c / 1200.0).exp2();
    // Spacing 0, Motion 0: eight copies within ±10 cents.
    let tight = swarm(6.0, 0.0, 0.0, &Setting::default(), &[]);
    let outside = power_outside(&tight, &[(cents(-14.0), cents(14.0))]);
    assert!(outside < 1e-2, "a unison spread: {outside:e} outside");
    assert!(
        (rms(&tight) - 0.5f64.sqrt()).abs() < 0.12,
        "eight copies at an eighth of the power each: {}",
        rms(&tight)
    );
    // Spacing 100 %: a chord of octaves and fifths above the note.
    let chord = swarm(6.0, 0.0, 100.0, &Setting::default(), &[]);
    for semitones in [0.0, 7.0, 12.0, 19.0, 24.0] {
        let f = f0 * (semitones / 12.0f64).exp2();
        assert!(
            (amp(&chord, f) - 1.0 / 8.0f64.sqrt()).abs() < 0.06,
            "{semitones} semitones: {}",
            amp(&chord, f)
        );
    }
    // With the scale on, every second degree: A, C, E, G, B above A in C
    // major.
    let scale = swarm(6.0, 0.0, 100.0, &scaled(c_major()), &[(USE_SCALE, 1.0)]);
    for semitones in [0.0, 3.0, 7.0, 10.0, 14.0, 17.0, 20.0] {
        let f = f0 * (semitones / 12.0f64).exp2();
        assert!(
            amp(&scale, f) > 0.25,
            "{semitones} semitones in the scale: {}",
            amp(&scale, f)
        );
    }
    // Motion moves the copies beyond their spread, up to ±30 cents.
    let wandering = swarm(6.0, 100.0, 0.0, &Setting::default(), &[]);
    assert!(
        power_outside(&wandering, &[(cents(-44.0), cents(44.0))]) < 1e-2,
        "no more than ±30 cents from a copy's own place"
    );
    assert!(
        power_outside(&wandering, &[(cents(-12.0), cents(12.0))]) > 0.05,
        "the copies drift"
    );
    // The waves: a triangle has no second harmonic and a saw does.
    let band = |signal: &[f32], harmonic: f64| {
        (-14..=14)
            .map(|c| amp(signal, f0 * harmonic * (f64::from(c) / 1200.0).exp2()).powi(2))
            .sum::<f64>()
    };
    let wave = |kind: f64| swarm(kind, 0.0, 0.0, &Setting::default(), &[]);
    let (sine, triangle, saw, square) = (wave(6.0), wave(7.0), wave(8.0), wave(9.0));
    assert!(band(&sine, 3.0) < 1e-4);
    assert!(band(&triangle, 3.0) > 1e-3 && band(&triangle, 2.0) < 1e-4);
    assert!(band(&saw, 2.0) > 1e-2);
    assert!(band(&square, 3.0) > 1e-2 && band(&square, 2.0) < 1e-4);
}

/// The energy outside a note's harmonics over the energy in them, in dB:
/// the aliasing level. A 7-term Blackman–Harris window, 8,192-frame spectra
/// overlapping by half, and 9 bins either side of each harmonic of each of
/// `fundamentals`, as the kit's own measurement does.
fn aliasing_db(signal: &[f32], fundamentals: &[f64]) -> f64 {
    const N: usize = 8_192;
    const A: [f64; 7] = [
        0.271_051_400_693_42,
        0.433_297_939_234_48,
        0.218_122_999_543_11,
        0.065_925_446_388_03,
        0.010_811_742_098_37,
        0.000_776_584_825_22,
        0.000_013_887_217_35,
    ];
    let window: Vec<f64> = (0..N)
        .map(|i| {
            let x = core::f64::consts::TAU * i as f64 / (N - 1) as f64;
            A.iter()
                .enumerate()
                .map(|(k, a)| a * (k as f64 * x).cos() * if k % 2 == 0 { 1.0 } else { -1.0 })
                .sum()
        })
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let bin = HZ / N as f64;
    let margin = 9.0 * bin;
    let (mut harmonic, mut other) = (0.0, 0.0);
    for start in (0..=signal.len() - N).step_by(N / 2) {
        for ((slot, &x), &w) in input.iter_mut().zip(&signal[start..start + N]).zip(&window) {
            *slot = f64::from(x) * w;
        }
        fft.process(&mut input, &mut spectrum).unwrap();
        for (k, value) in spectrum.iter().enumerate() {
            let f = k as f64 * bin;
            if f < margin || f >= HZ / 2.0 {
                continue;
            }
            let near = fundamentals.iter().any(|&fundamental| {
                let lowest = ((f - margin) / fundamental).ceil().max(1.0);
                lowest <= ((f + margin) / fundamental).floor() && lowest * fundamental < HZ / 2.0
            });
            if near {
                harmonic += value.norm_sqr();
            } else {
                other += value.norm_sqr();
            }
        }
    }
    10.0 * (other / harmonic).log10()
}

// Spec: meld R6, R37 (band-limited at the top key)
#[test]
fn no_type_folds_back_more_than_minus_80_db_at_the_top_key() {
    for key in [127u8, 96, 84, 72, 60] {
        let f0 = hz(f64::from(key));
        let mut cases: Vec<(String, Vec<(ParamId, f64)>, Vec<f64>)> = Vec::new();
        for kind in 0..3u32 {
            for shape in [0.0, 20.0, 50.0, 80.0, 100.0] {
                // Dual Basic Shapes' second macro past 50 % goes up the
                // intervals, which the top key can't hold.
                for tone in match kind {
                    1 => [0.0, 30.0],
                    // Rough is noise, which R6 leaves aside.
                    2 => [0.0, 0.0],
                    _ => [0.0, 100.0],
                } {
                    cases.push((
                        format!("type {kind} shape {shape} tone {tone}"),
                        vec![(a(TYPE), f64::from(kind)), (a(MACRO_1), shape), (a(MACRO_2), tone)],
                        // Dual Basic Shapes' second wave is tone cents up
                        // from tone per cent of a semitone.
                        if kind == 1 {
                            vec![f0, f0 * (tone / 1200.0).exp2()]
                        } else {
                            vec![f0]
                        },
                    ));
                }
            }
        }
        for macros in [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (37.0, 61.0)] {
            cases.push((
                format!("Square Sync {macros:?}"),
                vec![(a(TYPE), 3.0), (a(MACRO_1), macros.0), (a(MACRO_2), macros.1)],
                vec![f0],
            ));
        }
        for width in [0.0, 30.0, 50.0, 100.0] {
            cases.push((
                format!("Square 5th, width {width}"),
                vec![(a(TYPE), 4.0), (a(MACRO_1), 100.0), (a(MACRO_2), width)],
                vec![f0, f0 * 1.5],
            ));
        }
        for tone in [0.0, 50.0, 100.0] {
            cases.push((
                format!("Sub, tone {tone}"),
                vec![(a(TYPE), 5.0), (a(MACRO_1), tone), (a(MACRO_2), 100.0)],
                vec![f0, f0 / 2.0],
            ));
        }
        for kind in 6..10u32 {
            let copies = (0..8)
                .map(|i| f0 * ((2.0 * f64::from(i) / 7.0 - 1.0) * 0.1 / 12.0).exp2())
                .collect();
            cases.push((
                format!("swarm {kind}"),
                vec![(a(TYPE), f64::from(kind))],
                copies,
            ));
        }
        for (what, params, fundamentals) in cases {
            // A 5 % pulse keeps its harmonics to Nyquist, and the kit's
            // table reads leave about -77 dB of image at middle C; the
            // rule is the top key's.
            if key < 72 && what.ends_with("tone 100") {
                continue;
            }
            let [left, _] = play(&params, 48_000, &[on(0, key)]);
            let alias = aliasing_db(&left[16_384..], &fundamentals);
            assert!(alias < -80.0, "{what} at key {key}: {alias:.1} dB");
        }
    }
}
