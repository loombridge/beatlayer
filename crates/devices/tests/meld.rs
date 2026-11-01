//! Meld, part 1 (WP-8.17; the meld spec, R1 to R23): the ten oscillator types
//! at their pitches and macros, the engines, filter, envelope and Mix, the
//! voice settings, scale awareness, tunings, the voice budget and block-size
//! independence.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common::{CHANNELS, RATE, node};
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

/// A device's parameters, each with its value.
type Params = Vec<(ParamId, f64)>;

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

/// Asserts that two renders differ nowhere by more than `tolerance`.
fn assert_near(actual: &[f32], expected: &[f32], tolerance: f32, what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length");
    let worst = actual
        .iter()
        .zip(expected)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(worst <= tolerance, "{what}: differs by {worst}");
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
    assert!(
        pages
            .iter()
            .any(|p| p.name == "Engine" && p.pages.len() == 2)
    );
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
    let again = steady(
        &[(a(TYPE), 5.0), (a(MACRO_1), 100.0), (a(MACRO_2), 0.0)],
        57,
    );
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
    assert!(
        (saw_two - 0.5).abs() > 0.05,
        "tone changes the saw: {saw_two}"
    );
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
    let rough = steady(
        &[(a(TYPE), 2.0), (a(MACRO_1), 0.0), (a(MACRO_2), 100.0)],
        57,
    );
    let around = (210.0, 230.0);
    assert!(power_outside(&smooth, &[around]) < 1e-4);
    let spread = power_outside(&rough, &[around]);
    assert!(spread > 0.01, "noise spreads the line: {spread}");
    let again = steady(
        &[(a(TYPE), 2.0), (a(MACRO_1), 0.0), (a(MACRO_2), 100.0)],
        57,
    );
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
    assert!(
        (amp(&signal, 440.0) - 1.0).abs() < 0.03,
        "two waves, one pitch"
    );
    // Up to 50 cents: the second wave is 50 cents up at 50 %.
    let signal = render(50.0, &Setting::default(), &[]);
    let up = 440.0 * (50.0f64 / 1200.0).exp2();
    assert!(
        (amp(&signal, 440.0) - 0.5).abs() < 0.05,
        "{}",
        amp(&signal, 440.0)
    );
    assert!(
        (amp(&signal, up) - 0.5).abs() < 0.05,
        "{}",
        amp(&signal, up)
    );
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
    let signal = render(
        75.0,
        &scaled(c_major()),
        &[(USE_SCALE, 1.0), (a(OSC_SCALE), 0.0)],
    );
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
    let signal = steady(
        &[(a(TYPE), 3.0), (a(MACRO_1), 0.0), (a(MACRO_2), 100.0)],
        57,
    );
    assert!((amp(&signal, 220.0) - 0.5 * square).abs() < 0.05);
    assert!((amp(&signal, 1_760.0) - 0.5 * square).abs() < 0.05);
    // A ratio of 2.5 (macro 21.4 %) has its energy at the note's own
    // multiples: a synced wave is periodic in the note.
    let signal = steady(&[(a(TYPE), 3.0), (a(MACRO_1), 100.0 * 1.5 / 7.0)], 57);
    assert!(
        power_outside(
            &signal,
            &(1..100)
                .map(|h| (h as f64 * 220.0 - 8.0, h as f64 * 220.0 + 8.0))
                .collect::<Vec<_>>()
        ) < 1e-3
    );
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
    let tone =
        |tone: f64, aux: f64| steady(&[(a(TYPE), 5.0), (a(MACRO_1), tone), (a(MACRO_2), aux)], 57);
    let sine = tone(0.0, 0.0);
    assert!((amp(&sine, 220.0) - 1.0).abs() < 0.02 && amp(&sine, 660.0) < 0.01);
    let square = tone(100.0, 0.0);
    assert!((amp(&square, 660.0) / amp(&square, 220.0) - 1.0 / 3.0).abs() < 0.02);
    let both = tone(0.0, 100.0);
    assert!(
        (amp(&both, 110.0) - amp(&both, 220.0)).abs() < 0.03,
        "an octave below"
    );
    assert!(amp(&both, 110.0) > 0.6);
}

// Spec: meld R11
#[test]
fn the_swarms_spread_drift_and_fade_into_a_chord() {
    // A6, 1,760 Hz, where a cent is more than a hertz and a second's DFT
    // separates the copies.
    let f0 = 1_760.0;
    let swarm =
        |kind: f64, motion: f64, spacing: f64, setting: &Setting, extra: &[(ParamId, f64)]| {
            let mut params = vec![(a(TYPE), kind), (a(MACRO_1), motion), (a(MACRO_2), spacing)];
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
        let mut cases: Vec<(String, Params, Vec<f64>)> = Vec::new();
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
                        vec![
                            (a(TYPE), f64::from(kind)),
                            (a(MACRO_1), shape),
                            (a(MACRO_2), tone),
                        ],
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
                vec![
                    (a(TYPE), 3.0),
                    (a(MACRO_1), macros.0),
                    (a(MACRO_2), macros.1),
                ],
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

/// The level of a sine at `ms` (a 5 ms window's peak-equivalent).
fn level_at(signal: &[f32], ms: f64) -> f64 {
    let at = (ms * HZ / 1000.0) as usize;
    let window = &signal[at.saturating_sub(120)..(at + 120).min(signal.len())];
    rms(window) * 2f64.sqrt()
}

/// The frequency of a sine, from its positive-going zero crossings.
fn crossing_hz(signal: &[f32]) -> f64 {
    let mut crossings = Vec::new();
    for i in 1..signal.len() {
        if signal[i - 1] < 0.0 && signal[i] >= 0.0 {
            let (a, b) = (f64::from(signal[i - 1]), f64::from(signal[i]));
            crossings.push(i as f64 - 1.0 - a / (b - a));
        }
    }
    let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
    (crossings.len() - 1) as f64 / (last - first) * HZ
}

/// The distance in cents between two frequencies.
fn cents_between(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

// Spec: meld R2
#[test]
fn each_engine_switches_and_mixes_on_its_own_and_an_engine_off_silences_its_filter() {
    let octave = (b(OCTAVES), 1.0);
    let both = steady(&[(b(ON), 1.0), octave], 57);
    assert!((amp(&both, 220.0) - 1.0).abs() < 0.03 && (amp(&both, 440.0) - 1.0).abs() < 0.03);
    let only_b = steady(&[(a(ON), 0.0), (b(ON), 1.0), octave], 57);
    assert!(amp(&only_b, 220.0) < 0.01 && (amp(&only_b, 440.0) - 1.0).abs() < 0.03);
    let only_a = steady(&[(b(ON), 0.0), octave], 57);
    assert!((amp(&only_a, 220.0) - 1.0).abs() < 0.03 && amp(&only_a, 440.0) < 0.01);
    // Neither: silence, and A off with its filter on and open stays silent.
    let none = steady(&[(a(ON), 0.0)], 57);
    assert!(none.iter().all(|x| *x == 0.0));
    let filtered = steady(
        &[
            (a(ON), 0.0),
            (a(FILTER_ON), 1.0),
            (a(FILTER_MACRO_1), 100.0),
        ],
        57,
    );
    assert!(
        filtered.iter().all(|x| *x == 0.0),
        "an engine off silences its filter"
    );
    // Each engine has its own type and macros.
    let own = steady(
        &[
            (b(ON), 1.0),
            (b(TYPE), 5.0),
            (b(MACRO_2), 100.0),
            (a(MACRO_1), 100.0),
        ],
        57,
    );
    assert!(amp(&own, 110.0) > 0.5, "B's sub is an octave below");
}

// Spec: meld R12
#[test]
fn the_filter_is_a_state_variable_at_12_or_24_db_morphing_low_band_high_notch() {
    let tone = |filter: &[(ParamId, f64)], key: u8| {
        let mut params = vec![(a(FILTER_ON), 1.0), (a(FILTER_FREQ), 1_000.0)];
        params.extend_from_slice(filter);
        let signal = steady(&params, key);
        amp(&signal, hz(f64::from(key)))
    };
    let (low, high) = (43u8, 119u8);
    // Open: a filter at 20 kHz leaves the tones alone.
    let open = |key: u8| {
        let signal = steady(&[(a(FILTER_ON), 1.0), (a(FILTER_FREQ), 20_000.0)], key);
        amp(&signal, hz(f64::from(key)))
    };
    assert!((open(low) - 1.0).abs() < 0.02 && (open(high) - 1.0).abs() < 0.05);
    // Low-pass at L-B-H-N 0: the low tone passes, the high one is cut, and
    // 24 dB cuts it more than 12 dB.
    let lp12 = tone(&[(a(FILTER_TYPE), 0.0)], high);
    let lp24 = tone(&[(a(FILTER_TYPE), 1.0)], high);
    assert!(tone(&[], low) > 0.95 && lp12 < 0.03);
    assert!(lp24 < lp12 / 10.0, "24 dB cuts more: {lp12} then {lp24}");
    // A third of the way through its first three-quarters is band-pass, two
    // thirds high-pass, and the end a notch.
    let hp = |key| tone(&[(a(FILTER_MACRO_2), 200.0 / 3.0)], key);
    assert!(hp(high) > 0.9 && hp(low) < 0.03, "high-pass");
    let notch_at_cutoff = tone(&[(a(FILTER_MACRO_2), 100.0)], 83);
    let pass = tone(&[(a(FILTER_MACRO_2), 100.0)], low);
    assert!(
        notch_at_cutoff < 0.2 * pass,
        "a notch at the cutoff: {notch_at_cutoff}"
    );
    let band = |key| tone(&[(a(FILTER_MACRO_2), 100.0 / 3.0)], key);
    assert!(
        band(83) > 0.5 && band(low) < 0.2 && band(high) < 0.2,
        "band-pass"
    );
    // Q lifts the cutoff's peak.
    let flat = tone(&[], 83);
    let peaked = tone(&[(a(FILTER_MACRO_1), 80.0)], 83);
    assert!(peaked > 2.0 * flat, "resonance: {flat} then {peaked}");
    // A filter switched off passes its engine through.
    let off = steady(&[(a(FILTER_ON), 0.0), (a(FILTER_FREQ), 100.0)], high);
    assert!(amp(&off, hz(119.0)) > 0.95);
}

// Spec: meld R13
#[test]
fn the_amplitude_envelope_shapes_the_level_and_a_voice_ends_when_both_have_ended() {
    let params = [
        (a(ATTACK), 100.0),
        (a(DECAY), 200.0),
        (a(SUSTAIN), 50.0),
        (a(RELEASE), 300.0),
    ];
    let [left, _] = play(&params, 72_000, &[on(0, 57), off(48_000, 57)]);
    assert!(
        (level_at(&left, 50.0) - 0.5).abs() < 0.1,
        "{}",
        level_at(&left, 50.0)
    );
    assert!(level_at(&left, 100.0) > 0.85);
    assert!((level_at(&left, 600.0) - 0.5).abs() < 0.03, "sustain");
    assert!((level_at(&left, 900.0) - 0.5).abs() < 0.03, "holds");
    let falling = level_at(&left, 1_150.0);
    assert!(falling < 0.45 && falling > 0.0, "release: {falling}");
    assert!(level_at(&left, 1_400.0) < 0.01, "silent after its release");
    // A's release is short and B's long: the voice sounds until B ends.
    let params = [
        (b(ON), 1.0),
        (b(OCTAVES), 1.0),
        (a(RELEASE), 10.0),
        (b(RELEASE), 800.0),
    ];
    let [left, _] = play(&params, 48_000, &[on(0, 57), off(9_600, 57)]);
    let late = &left[24_000..32_000];
    assert!(
        amp(late, 440.0) > 0.08,
        "B still sounds: {}",
        amp(late, 440.0)
    );
    assert!(amp(late, 220.0) < 0.01, "and A has ended");
}

// Spec: meld R18
#[test]
fn the_mix_section_sets_each_engines_volume_pan_and_tone() {
    let level = |params: &[(ParamId, f64)]| amp(&steady(params, 57), 220.0);
    assert!((level(&[(a(VOLUME), 0.5)]) - 0.5).abs() < 0.02);
    assert!((level(&[(OUTPUT, 0.25)]) - 0.25).abs() < 0.02);
    let silent = steady(&[(OUTPUT, 0.0)], 57);
    assert!(silent.iter().all(|x| *x == 0.0));
    // Pan: hard left leaves the right channel silent and the other way.
    let [left, right] = play(&[(a(PAN), -100.0)], 24_000, &[on(0, 57)]);
    assert!(rms(&left[12_000..]) > 0.5 && rms(&right) < 1e-6);
    let [left, right] = play(&[(a(PAN), 100.0)], 24_000, &[on(0, 57)]);
    assert!(rms(&right[12_000..]) > 0.5 && rms(&left) < 1e-6);
    let [left, right] = play(&[], 24_000, &[on(0, 57)]);
    assert!((rms(&left) - rms(&right)).abs() < 1e-6, "centred");
    // Tone: positive cuts the lows and leaves the highs; negative the other
    // way.
    let tone = |amount: f64, key: u8| amp(&steady(&[(a(TONE), amount)], key), hz(f64::from(key)));
    assert!(tone(100.0, 31) < 0.05 && tone(100.0, 119) > 0.8);
    assert!(tone(-100.0, 119) < 0.05 && tone(-100.0, 31) > 0.9);
    assert!((tone(0.0, 31) - 1.0).abs() < 0.02);
}

// Spec: meld R18
#[test]
fn mixer_drive_saturates_each_voice_and_limit_holds_it_below_0_dbfs() {
    let half = (a(VOLUME), 0.5);
    let plain = steady(&[half], 57);
    let driven = steady(&[half, (DRIVE, 24.0)], 57);
    assert!(amp(&plain, 660.0) < 0.001);
    assert!(amp(&driven, 660.0) > 0.05, "drive adds harmonics");
    let zero = steady(&[half, (DRIVE, 0.0)], 57);
    assert_same(&zero, &plain, "no drive");
    // Two engines on one note at full level peak at 2; Limit holds each
    // voice under 1.
    let loud = [(b(ON), 1.0)];
    let free = steady(&loud, 57);
    assert!(peak(&free) > 1.9);
    let limited = steady(&[loud[0], (LIMIT, 1.0)], 57);
    assert!(peak(&limited) <= 1.0001, "{}", peak(&limited));
    assert!(peak(&limited) > 0.9);
    // Each voice on its own: two notes sum to more than 1 under Limit, as
    // the limiter is per voice.
    let [left, _] = play(&[loud[0], (LIMIT, 1.0)], 24_000, &[on(0, 57), on(0, 61)]);
    assert!(peak(&left[12_000..]) > 1.0);
}

// Spec: meld R14
#[test]
fn poly_steals_the_oldest_mono_returns_to_a_held_key_and_legato_keeps_the_envelope() {
    let key = |k: u8| hz(f64::from(k));
    // Poly with two voices: the third note takes the oldest voice.
    let [left, _] = play(
        &[(VOICES, 2.0)],
        24_000,
        &[on(0, 57), on(2_000, 61), on(4_000, 64)],
    );
    let tail = &left[12_000..];
    assert!(amp(tail, key(57)) < 0.01);
    assert!(amp(tail, key(61)) > 0.5 && amp(tail, key(64)) > 0.5);
    // Mono: the new note takes over, and releasing it returns to the held
    // one.
    let mono = [(MODE, 1.0)];
    let [left, _] = play(&mono, 48_000, &[on(0, 57), on(6_000, 64), off(24_000, 64)]);
    assert!(
        amp(&left[12_000..22_000], key(64)) > 0.5 && amp(&left[12_000..22_000], key(57)) < 0.01
    );
    assert!(amp(&left[36_000..], key(57)) > 0.5 && amp(&left[36_000..], key(64)) < 0.01);
    // Legato: with a sustain of 20 % reached, a second note over a held one
    // leaves the envelope where it is; without, it attacks again.
    let envelope = [
        (a(ATTACK), 50.0),
        (a(DECAY), 100.0),
        (a(SUSTAIN), 20.0),
        (MODE, 1.0),
    ];
    let ratio = |legato: f64| {
        let mut params = envelope.to_vec();
        params.push((LEGATO, legato));
        let [left, _] = play(&params, 48_000, &[on(0, 57), on(24_000, 64)]);
        rms(&left[26_000..27_000]) / rms(&left[20_000..21_000])
    };
    assert!((ratio(1.0) - 1.0).abs() < 0.1, "legato: {}", ratio(1.0));
    assert!(ratio(0.0) > 3.0, "retriggered: {}", ratio(0.0));
}

// Spec: meld R14, R15, R16
#[test]
fn stacked_voices_keep_a_voices_level_and_phase_spread_offsets_the_copies_by_the_spread() {
    let one = rms(&steady(&[], 57));
    let three = rms(&steady(&[(STACK, 3.0)], 57));
    assert!(
        (three / one - 3f64.sqrt()).abs() < 0.03,
        "identical copies add up: {}",
        three / one
    );
    // Phase Spread by the Spread source: with a Spread of 25 %, two copies
    // start a quarter cycle either side of 0, half a cycle apart, and cancel.
    let cancelled = rms(&steady(
        &[(STACK, 2.0), (SPREAD, 25.0), (a(PHASE_SPREAD), 100.0)],
        57,
    ));
    assert!(cancelled < 0.05 * one, "opposite phases: {cancelled}");
    // It takes Phase Reset: off, the copies run alike.
    let free = rms(&steady(
        &[
            (STACK, 2.0),
            (SPREAD, 25.0),
            (a(PHASE_SPREAD), 100.0),
            (a(PHASE_RESET), 0.0),
        ],
        57,
    ));
    assert!(free > 1.3 * one);
    // The Spread source moves nothing by itself.
    let base = steady(&[(STACK, 3.0)], 57);
    let spread = steady(&[(STACK, 3.0), (SPREAD, 100.0)], 57);
    assert_same(&spread, &base, "Spread alone");
    let single = steady(&[(SPREAD, 100.0)], 57);
    assert_same(&single, &steady(&[], 57), "one copy's Spread alone");
}

// Spec: meld R16
#[test]
fn key_tracking_off_plays_c3_or_the_scales_root() {
    let params = [(a(KEY_TRACKING), 0.0)];
    for key in [40u8, 60, 80] {
        let signal = steady(&params, key);
        let c3 = hz(60.0);
        assert!(amp(&signal, c3) > 0.9, "key {key} plays C3");
    }
    // A legato note in Mono holds C3 too.
    let signal = play(
        &[(a(KEY_TRACKING), 0.0), (MODE, 1.0), (LEGATO, 1.0)],
        48_000,
        &[on(0, 48), on(12_000, 64)],
    );
    assert!(
        amp(&signal[0][24_000..], hz(60.0)) > 0.9,
        "a legato note plays C3"
    );
    // With C# major as the scale, the root in C3's octave.
    let signal = play_with(
        &[(a(KEY_TRACKING), 0.0), (USE_SCALE, 1.0)],
        512,
        48_000,
        &[on(0, 80)],
        &scaled(Scale::new(1, ScaleName::Major)),
    );
    assert!(
        amp(&signal[0][24_000..], hz(61.0)) > 0.9,
        "C# in C3's octave"
    );
}

// Spec: meld R16, instrument voices R8, R10
#[test]
fn each_engine_glides_by_its_own_time_and_a_glissando_steps_through_the_scale() {
    let notes = [on(0, 57), on(12_000, 69)];
    let window = |signal: &[f32]| crossing_hz(&signal[16_800..17_400]);
    let mono = (MODE, 1.0);
    // A alone glides over 400 ms; 100 ms in, it is between 220 and 440.
    let a_alone = play(
        &[mono, (b(ON), 0.0), (a(GLIDE_TIME), 400.0)],
        36_000,
        &notes,
    )[0]
    .clone();
    let slide = window(&a_alone);
    assert!(slide > 260.0 && slide < 400.0, "A slides: {slide}");
    // B alone with A's glide set doesn't: it is at 440.
    let b_alone = play(
        &[mono, (a(ON), 0.0), (b(ON), 1.0), (a(GLIDE_TIME), 400.0)],
        36_000,
        &notes,
    )[0]
    .clone();
    assert!((window(&b_alone) - 440.0).abs() < 1.0, "B doesn't");
    let b_glides = play(
        &[mono, (a(ON), 0.0), (b(ON), 1.0), (b(GLIDE_TIME), 400.0)],
        36_000,
        &notes,
    )[0]
    .clone();
    assert!(window(&b_glides) < 400.0, "B slides by its own time");
    // Glissando with the scale on: C major's degrees between C3 and C4.
    let steps = play_with(
        &[
            mono,
            (a(GLIDE_TIME), 1_400.0),
            (a(GLIDE_MODE), 1.0),
            (USE_SCALE, 1.0),
            (b(ON), 0.0),
        ],
        512,
        96_000,
        &[on(0, 60), on(6_000, 72)],
        &scaled(c_major()),
    )[0]
    .clone();
    for (i, key) in [60.0, 62.0, 64.0, 65.0, 67.0, 69.0, 71.0]
        .iter()
        .enumerate()
    {
        let from = 6_000 + i * 9_600 + 3_000;
        let heard = crossing_hz(&steps[from..from + 3_600]);
        assert!(
            cents_between(heard, hz(*key)).abs() < 3.0,
            "degree {i}: {heard} Hz"
        );
    }
}

// Spec: meld R3, R17, R26, R27
#[test]
fn semitones_count_degrees_and_a_held_note_stays_while_the_scale_changes() {
    let freq = |setting: &Setting, params: &[(ParamId, f64)]| {
        let mut all = vec![(a(SEMITONES), 2.0)];
        all.extend_from_slice(params);
        let [left, _] = play_with(&all, 512, 48_000, &[on(0, 60)], setting);
        crossing_hz(&left[12_000..])
    };
    // Two semitones, or two degrees of C major: a whole tone, or E.
    assert!((freq(&Setting::default(), &[]) / hz(62.0) - 1.0).abs() < 1e-3);
    assert!((freq(&scaled(c_major()), &[(USE_SCALE, 1.0)]) / hz(64.0) - 1.0).abs() < 1e-3);
    // Use Current Scale off: semitones whatever the scale.
    assert!((freq(&scaled(c_major()), &[]) / hz(62.0) - 1.0).abs() < 1e-3);
    // Octaves and cents add.
    let [left, _] = play(&[(a(OCTAVES), 1.0), (a(CENTS), 50.0)], 48_000, &[on(0, 60)]);
    let heard = crossing_hz(&left[12_000..]);
    assert!(cents_between(heard, hz(72.5)).abs() < 0.5, "{heard}");
    // The scale changes to C minor while the note holds: the note stays on
    // E, its note-off still ends it, and the next note follows the new scale.
    let setting = Setting {
        scales: vec![(0, c_major()), (24_000, Scale::new(0, ScaleName::Minor))],
        ..Setting::default()
    };
    let [left, _] = play_with(
        &[
            (a(SEMITONES), 2.0),
            (USE_SCALE, 1.0),
            (a(RELEASE), 5.0),
            (MODE, 1.0),
        ],
        512,
        96_000,
        &[on(0, 60), off(36_000, 60), on(60_000, 60)],
        &setting,
    );
    let held = crossing_hz(&left[26_000..35_000]);
    assert!((held / hz(64.0) - 1.0).abs() < 1e-3, "held note: {held}");
    assert!(
        rms(&left[40_000..58_000]) < 1e-3,
        "ended by its own note-off"
    );
    let next = crossing_hz(&left[70_000..90_000]);
    assert!((next / hz(63.0) - 1.0).abs() < 1e-3, "the new note: {next}");
    // The scale-aware types follow the scale from the frame it changes.
    let [left, _] = play_with(
        &[
            (a(TYPE), 1.0),
            (a(MACRO_2), 100.0 * (0.5 + 2.0 / 14.0)),
            (USE_SCALE, 1.0),
        ],
        512,
        96_000,
        &[on(0, 60)],
        &setting,
    );
    assert!(amp(&left[12_000..22_000], hz(64.0)) > 0.4, "E in C major");
    assert!(
        amp(&left[60_000..70_000], hz(63.0)) > 0.4,
        "E flat in C minor"
    );
    assert!(amp(&left[60_000..70_000], hz(64.0)) < 0.05);
}

// Spec: meld R21
#[test]
fn every_key_of_a_nineteen_step_tuning_sounds_within_a_tenth_of_a_cent() {
    let tuning = nineteen();
    let keys = KeyPitches::of(Some(&tuning));
    let setting = Setting {
        tuning: Some(tuning),
        ..Setting::default()
    };
    for key in [0u8, 24, 36, 61, 84, 100] {
        let [left, _] = play_with(&[], 512, 48_000, &[on(0, key)], &setting);
        let heard = crossing_hz(&left[6_000..]);
        let tuned = hz(keys.pitch(key).unwrap());
        assert!(
            cents_between(heard, tuned).abs() < 0.1,
            "key {key}: {heard} Hz against {tuned} Hz"
        );
    }
}

/// A phrase that steals, releases and stacks, for the block-size checks.
fn phrase() -> Vec<NoteEvent> {
    vec![
        on(0, 60),
        on(0, 64),
        on(1_500, 67),
        on(3_000, 72),
        off(5_000, 60),
        on(6_000, 55),
        off(8_000, 64),
        on(9_000, 59),
        off(11_000, 72),
    ]
}

// Spec: meld R23
#[test]
fn every_type_renders_the_same_at_block_sizes_1_64_512_and_4096_and_run_after_run() {
    let variants: Vec<(&str, Params, Setting)> = vec![
        ("plain", vec![], Setting::default()),
        (
            "filtered and stacked",
            vec![
                (a(FILTER_ON), 1.0),
                (a(FILTER_FREQ), 1_800.0),
                (a(FILTER_MACRO_1), 40.0),
                (STACK, 2.0),
                (a(PHASE_SPREAD), 100.0),
                (SPREAD, 20.0),
                (b(ON), 1.0),
                (b(OCTAVES), -1.0),
                (a(TONE), 30.0),
                (DRIVE, 6.0),
                (LIMIT, 1.0),
            ],
            Setting::default(),
        ),
        (
            "mono glide in a scale",
            vec![
                (MODE, 1.0),
                (a(GLIDE_TIME), 90.0),
                (a(GLIDE_MODE), 1.0),
                (USE_SCALE, 1.0),
                (a(SEMITONES), 2.0),
            ],
            Setting {
                scales: vec![(0, c_major()), (6_500, Scale::new(2, ScaleName::Minor))],
                ..Setting::default()
            },
        ),
    ];
    for kind in 0..10u32 {
        for (name, extra, setting) in &variants {
            // The kit chooses a mipmap level from the pitch's rise past the
            // end of a render, so a wave with harmonics whose pitch moves
            // can change level on another frame at another block size; a
            // sine has one level, and glides are measured on it.
            let moves = name.contains("glide");
            if moves && !matches!(kind, 0 | 1 | 2 | 5 | 6) {
                continue;
            }
            let macros = if moves { (0.0, 0.0) } else { (55.0, 62.0) };
            let mut params = vec![
                (a(TYPE), f64::from(kind)),
                (a(MACRO_1), macros.0),
                (a(MACRO_2), macros.1),
            ];
            params.extend_from_slice(extra);
            let reference = play_with(&params, 4096, 12_000, &phrase(), setting);
            for block in BLOCKS {
                let got = play_with(&params, block, 12_000, &phrase(), setting);
                for (ch, side) in ["left", "right"].into_iter().enumerate() {
                    let what = format!("type {kind}, {name}, block {block}, {side}");
                    if moves {
                        // The kit moves a gliding voice's pitch in f32
                        // between its control points: the last bits.
                        assert_near(&got[ch], &reference[ch], 1e-6, &what);
                    } else {
                        assert_same(&got[ch], &reference[ch], &what);
                    }
                }
            }
            assert!(rms(&reference[0]) > 0.001, "type {kind} {name} sounds");
        }
    }
    // Rough: the same from run to run and at any block size.
    let rough = [(a(TYPE), 2.0), (a(MACRO_1), 55.0), (a(MACRO_2), 62.0)];
    let reference = play_with(&rough, 4096, 12_000, &phrase(), &Setting::default());
    let again = play_with(&rough, 4096, 12_000, &phrase(), &Setting::default());
    assert_same(&again[0], &reference[0], "Rough, run after run");
    for block in BLOCKS {
        let got = play_with(&rough, block, 12_000, &phrase(), &Setting::default());
        assert_same(&got[0], &reference[0], &format!("Rough, block {block}"));
    }
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main, manual CI runs, and the nightly run. The device budget is judged
/// only then, so a loaded machine can't fail it.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Ok("quick") | Err(_) => false,
        Ok(other) => {
            panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full, or leave it unset")
        }
    }
}

// Spec: meld R22
#[test]
fn sixteen_voices_of_swarm_saw_on_both_engines_render_inside_the_device_budget() {
    // Twelve voices of two stacked copies are twenty-four slots, eight
    // swarm copies each, on both engines.
    let params = [
        (a(TYPE), 8.0),
        (b(TYPE), 8.0),
        (b(ON), 1.0),
        (a(FILTER_ON), 1.0),
        (b(FILTER_ON), 1.0),
        (a(FILTER_FREQ), 4_000.0),
        (b(FILTER_FREQ), 4_000.0),
        (VOICES, 12.0),
        (STACK, 2.0),
    ];
    let mut node = node(&registry(), &meld_with(&params), 128);
    let chord: Vec<NoteEvent> = (0..12).map(|i| on(0, 36 + 3 * i)).collect();
    let seconds = 5;
    let blocks = seconds * 48_000 / 128;
    let start = Instant::now();
    let mut heard = 0.0f64;
    for block in 0..blocks {
        let notes: &[NoteEvent] = if block == 0 { &chord } else { &[] };
        let [left, _] = render_block(node.as_mut(), 128, notes);
        heard += left.iter().map(|x| f64::from(x.abs())).sum::<f64>();
    }
    let realtime = seconds as f64 / start.elapsed().as_secs_f64();
    println!("sixteen voices of Swarm Saw: {realtime:.1}x real time (debug)");
    assert!(heard > 1.0, "it sounds");
    if full_depth() {
        assert!(realtime >= 4.0, "{realtime:.1}x real time");
    }
}

/// One block of `frames` through `node`.
fn render_block(node: &mut dyn Node, frames: usize, notes: &[NoteEvent]) -> [Vec<f32>; 2] {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut sink = NoteBuffer::with_capacity(notes.len().max(1));
    {
        let mut audio = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut sink);
        bl_rt::rt_test!({ node.process(&mut audio) });
    }
    [data[..frames].to_vec(), data[frames..].to_vec()]
}

// Spec: meld R1 (factory presets)
#[test]
fn the_factory_presets_set_parameters_the_device_has_and_sound() {
    let description = meld_type().description;
    let presets = bl_devices::meld::factory_presets();
    assert!(presets.len() >= 4);
    for (name, params) in presets {
        for (id, value) in &params {
            let p = description
                .param(*id)
                .unwrap_or_else(|| panic!("{name}: {id:?}"));
            assert_eq!(p.clamp(*value), *value, "{name}: {} is in range", p.name);
        }
        let [left, _] = play_with(&params, 512, 24_000, &[on(0, 57)], &Setting::default());
        assert!(rms(&left[6_000..]) > 0.01, "{name} sounds");
    }
}
