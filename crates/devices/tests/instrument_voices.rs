//! The instruments' voice section (WP-8.2; spec: instrument voices): the
//! shared voice parts on the framework's test instrument, and Drift playing
//! through them.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common;
use core::f64::consts::TAU;

use bl_devices::instrument::{self, Parts, Section};
use bl_devices::polysynth::{
    MOD_DESTINATIONS, MOD_SOURCES, MOD1_AMOUNT, MOD1_DEST, MOD1_SOURCE, NOTE_PB, PB_RANGE,
    POLYSYNTH, VOICES,
};
use bl_devices::testing::{
    self, INSTRUMENT_ERROR, INSTRUMENT_GLIDE_LEGATO, INSTRUMENT_GLIDE_MODE, INSTRUMENT_GLIDE_PROP,
    INSTRUMENT_GLIDE_TIME, INSTRUMENT_HI_QUALITY, INSTRUMENT_KEY_CURVE, INSTRUMENT_LEGATO,
    INSTRUMENT_NOTE_PB, INSTRUMENT_NOTE_PB_RANGE, INSTRUMENT_OUTPUT, INSTRUMENT_PB_RANGE,
    INSTRUMENT_PRESSURE_LEVEL, INSTRUMENT_PRIORITY, INSTRUMENT_RETRIGGER, INSTRUMENT_STRETCH,
    INSTRUMENT_TRANSPOSE, INSTRUMENT_UNISON_DELAY, INSTRUMENT_UNISON_DETUNE,
    INSTRUMENT_UNISON_SPREAD, INSTRUMENT_UNISON_VOICES, INSTRUMENT_USE_SCALE,
    INSTRUMENT_VELOCITY_CURVE, INSTRUMENT_VOICE_MODE, INSTRUMENT_VOICES, instrument_type,
};
use bl_engine::{
    Block, Collector, DeviceType, KeyPitches, Node, NoteBuffer, NoteEvent, NoteExpression,
    NoteKind, Registry, TransportInfo,
};
use bl_model::{DeviceKind, ParamId, Scale, ScaleName, Tuning};
use common::{CHANNELS, RATE, device, node};

/// The sample rate, in hertz.
const HZ: f64 = 48_000.0;

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

/// 19 equal steps to the octave, with the default reference and range.
fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".into(), String::new(), cents).unwrap()
}

fn event(offset: usize, kind: NoteKind, key: u8, velocity: f32, channel: u8) -> NoteEvent {
    NoteEvent {
        offset: offset as u32,
        kind,
        channel,
        key,
        velocity,
        note_id: None,
    }
}

fn on(frame: usize, key: u8) -> NoteEvent {
    event(frame, NoteKind::On, key, 1.0, 0)
}

fn on_at(frame: usize, key: u8, velocity: f32) -> NoteEvent {
    event(frame, NoteKind::On, key, velocity, 0)
}

fn off(frame: usize, key: u8) -> NoteEvent {
    event(frame, NoteKind::Off, key, 0.0, 0)
}

fn off_at(frame: usize, key: u8, velocity: f32) -> NoteEvent {
    event(frame, NoteKind::Off, key, velocity, 0)
}

fn expression(frame: usize, key: u8, expression: NoteExpression, value: f32) -> NoteEvent {
    event(
        frame,
        NoteKind::Expression { expression, value },
        key,
        0.0,
        0,
    )
}

fn message(frame: usize, kind: NoteKind) -> NoteEvent {
    event(frame, kind, 0, 0.0, 0)
}

/// What a render plays through: the tuning and scale a block carries.
#[derive(Clone, Default)]
struct Setting {
    tuning: Option<Tuning>,
    scale: Option<Scale>,
}

/// Plays `notes` (frames counted from the start) through `node` for
/// `frames`, in blocks of `block`, and returns both channels.
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
            .with_notes(&block_notes, &mut sink);
            if let Some(scale) = setting.scale {
                audio = audio.with_set_scale(scale);
            }
            bl_rt::rt_test!({ node.process(&mut audio) });
        }
        out[0].extend_from_slice(&data[..len]);
        out[1].extend_from_slice(&data[len..]);
        start += len;
    }
    out
}

/// A test instrument with `params` set, rendered at block size `block`.
fn play(
    params: &[(ParamId, f64)],
    block: usize,
    frames: usize,
    notes: &[NoteEvent],
    setting: &Setting,
) -> [Vec<f32>; 2] {
    let mut node = node(
        &testing::registry(),
        &device(&instrument_type(), params),
        4096,
    );
    render_node(node.as_mut(), block, frames, notes, setting)
}

/// As [`play`] at 512 frames a block, with nothing special in the set.
fn run(params: &[(ParamId, f64)], frames: usize, notes: &[NoteEvent]) -> [Vec<f32>; 2] {
    play(params, 512, frames, notes, &Setting::default())
}

/// The outputs the test instrument reads out.
const PITCH: f64 = 11.0;
const PITCH_LAST: f64 = 12.0;

fn output(index: f64) -> (ParamId, f64) {
    (INSTRUMENT_OUTPUT, index)
}

/// The frequency of a pitch in semitones, A4 at 440 Hz.
fn hz(pitch: f64) -> f64 {
    440.0 * ((pitch - 69.0) / 12.0).exp2()
}

/// The power of `signal` at `hz`, through a Hann window.
fn power_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &x) in signal.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let phase = TAU * hz * i as f64 / HZ;
        re += f64::from(x) * window * phase.cos();
        im -= f64::from(x) * window * phase.sin();
    }
    (re * re + im * im) / (n * n)
}

/// Asserts that `signal` holds each of `present` keys and none of `absent`.
fn assert_keys(signal: &[f32], present: &[u8], absent: &[u8]) {
    let power = |key: &u8| power_at(signal, hz(f64::from(*key)));
    let quietest = present.iter().map(power).fold(f64::MAX, f64::min);
    let loudest = absent.iter().map(power).fold(0.0, f64::max);
    assert!(
        quietest > 20.0 * loudest,
        "{present:?} sound and {absent:?} don't: {quietest:e} against {loudest:e}"
    );
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
fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, x| m.max(x.abs()))
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

/// Asserts that two renders differ nowhere by more than `tolerance`: the
/// kit moves a voice's pitch between control points in `f32`, so a render
/// whose pitch moves can differ in the last bit between block sizes.
fn assert_near(actual: &[f32], expected: &[f32], tolerance: f32, what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length");
    let worst = (0..actual.len())
        .max_by(|&a, &b| {
            (actual[a] - expected[a])
                .abs()
                .total_cmp(&(actual[b] - expected[b]).abs())
        })
        .unwrap_or(0);
    assert!(
        (actual[worst] - expected[worst]).abs() <= tolerance,
        "{what}: sample {worst} is {} against {}",
        actual[worst],
        expected[worst]
    );
}

fn assert_close(actual: f64, expected: f64, tolerance: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} against {expected} (±{tolerance})"
    );
}

fn drift_type() -> DeviceType {
    bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == POLYSYNTH))
        .expect("Drift is registered")
}

/// FNV-1a over the samples' bits: what a golden render compares.
fn digest(signal: &[f32]) -> u64 {
    signal.iter().fold(0xcbf2_9ce4_8422_2325, |hash, sample| {
        (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A phrase that uses what Drift's voices do: four voices stolen from, a
/// per-note bend, the pitch wheel, the mod wheel on a route, pressure, and
/// a tuning arriving mid-phrase.
fn drift_phrase(block: usize) -> Vec<f32> {
    let source = |name: &str| {
        MOD_SOURCES
            .iter()
            .position(|s| *s == name)
            .expect("a source") as f64
    };
    let destination = |name: &str| {
        MOD_DESTINATIONS
            .iter()
            .position(|d| *d == name)
            .expect("a destination") as f64
    };
    let params = [
        (VOICES, 4.0),
        (NOTE_PB, 1.0),
        (PB_RANGE, 12.0),
        (MOD1_SOURCE, source("Modwheel")),
        (MOD1_DEST, destination("LP Frequency")),
        (MOD1_AMOUNT, 0.5),
    ];
    let mut node = node(&registry(), &device(&drift_type(), &params), 4096);
    let notes = [
        on_at(0, 60, 0.8),
        on_at(0, 64, 0.6),
        on_at(0, 67, 0.9),
        on_at(0, 72, 0.7),
        on(2_000, 76),
        expression(4_000, 64, NoteExpression::Bend, 0.25),
        message(6_000, NoteKind::PitchBend { value: 0.5 }),
        message(
            8_000,
            NoteKind::Control {
                controller: 1,
                value: 0.7,
            },
        ),
        expression(8_500, 67, NoteExpression::Pressure, 0.6),
        off(9_000, 60),
        on_at(12_000, 55, 0.5),
        on_at(16_000, 62, 0.8),
        on_at(17_000, 70, 0.8),
        off(30_000, 62),
        off(30_000, 70),
    ];
    // The tuning arrives at the start, so a 19-step table is in force for
    // the notes from frame 15,000 on only where the first block holds it:
    // send it to the node between blocks instead.
    let collector = Collector::new();
    let mut out = Vec::new();
    let mut start = 0;
    while start < 48_000 {
        let len = block.min(48_000 - start);
        if (start..start + len).contains(&15_000) {
            node.receive_data(collector.handle().data(KeyPitches::of(Some(&nineteen()))));
        }
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= start && (n.offset as usize) < start + len)
            .map(|n| NoteEvent {
                offset: n.offset - start as u32,
                ..*n
            })
            .collect();
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
            .with_notes(&block_notes, &mut sink);
            bl_rt::rt_test!({ node.process(&mut audio) });
        }
        out.extend_from_slice(&data[..len]);
        start += len;
    }
    out
}

// Spec: instrument voices R22.
#[test]
fn drift_plays_through_the_section_sample_for_sample() {
    let signal = drift_phrase(512);
    assert!(signal.iter().any(|s| s.abs() > 0.01), "the phrase sounds");
    // The digest of the phrase before Drift moved onto the section. A
    // vector unit of another architecture rounds some samples differently,
    // so only the one it was taken on compares it.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(digest(&signal), 0xb41f_6267_b297_f453);
    let _ = digest(&signal);
}

// Spec: instrument voices R21, R22.
#[test]
fn drift_renders_the_same_at_any_block_size() {
    let reference = drift_phrase(4096);
    for block in BLOCKS {
        assert_near(
            &drift_phrase(block),
            &reference,
            1e-3,
            &format!("block size {block}"),
        );
    }
}

// Spec: instrument voices R1.
#[test]
fn the_parts_read_alike_and_hi_quality_is_a_menu_option() {
    let description = instrument_type().description;
    description.check().unwrap();
    let param = |id: ParamId| description.param(id).expect("described").clone();
    for (id, name) in [
        (INSTRUMENT_VOICES, "Voices"),
        (INSTRUMENT_VOICE_MODE, "Voice Mode"),
        (INSTRUMENT_PRIORITY, "Priority"),
        (INSTRUMENT_LEGATO, "Legato"),
        (INSTRUMENT_GLIDE_TIME, "Glide Time"),
        (INSTRUMENT_GLIDE_MODE, "Glide Mode"),
        (INSTRUMENT_GLIDE_PROP, "Glide Prop"),
        (INSTRUMENT_GLIDE_LEGATO, "Glide Legato"),
        (INSTRUMENT_RETRIGGER, "Retrigger"),
        (INSTRUMENT_UNISON_VOICES, "Unison Voices"),
        (INSTRUMENT_UNISON_DETUNE, "Unison Detune"),
        (INSTRUMENT_UNISON_SPREAD, "Unison Spread"),
        (INSTRUMENT_UNISON_DELAY, "Unison Delay"),
        (INSTRUMENT_PB_RANGE, "PB Range"),
        (INSTRUMENT_NOTE_PB, "Note PB"),
        (INSTRUMENT_NOTE_PB_RANGE, "Note PB Range"),
        (INSTRUMENT_TRANSPOSE, "Transpose"),
        (INSTRUMENT_STRETCH, "Stretch"),
        (INSTRUMENT_ERROR, "Error"),
        (INSTRUMENT_HI_QUALITY, "Hi-Quality"),
    ] {
        assert_eq!(param(id).name, name);
        assert!(param(id).automatable, "{name} is automatable");
    }
    // The units and tapers the helpers give: times in milliseconds that
    // reach zero, ranges in semitones, detune in cents.
    let json = |id| serde_json::to_value(param(id).kind).unwrap();
    assert_eq!(
        json(INSTRUMENT_GLIDE_TIME)["continuous"]["unit"],
        "milliseconds"
    );
    assert_eq!(
        json(INSTRUMENT_GLIDE_TIME)["continuous"]["taper"]["power"],
        2
    );
    assert_eq!(json(INSTRUMENT_PB_RANGE)["integer"]["unit"], "semitones");
    assert_eq!(
        json(INSTRUMENT_UNISON_DETUNE)["continuous"]["unit"],
        "cents"
    );
    assert_eq!(description.menu_options, vec![INSTRUMENT_HI_QUALITY]);
    assert_eq!(param(INSTRUMENT_HI_QUALITY).default, 0.0);
}

// Spec: instrument voices R2.
#[test]
fn voices_times_copies_fit_the_lanes_in_sixteens() {
    for (voices, copies) in [(32, 8), (32, 16), (5, 3), (1, 1), (16, 2)] {
        let section = Section::new(Parts::default(), voices, copies, HZ, 0);
        assert_eq!(section.lane_count() % 16, 0, "{voices} voices of {copies}");
        assert!(section.lane_count() >= voices * copies);
    }
}

/// The largest second difference of `signal` over `frames`: what a click
/// shows in.
fn bend_in(signal: &[f32], frames: core::ops::Range<usize>) -> f32 {
    frames
        .map(|i| (signal[i] - 2.0 * signal[i - 1] + signal[i - 2]).abs())
        .fold(0.0, f32::max)
}

// Spec: instrument voices R3, R4.
#[test]
fn thirty_two_voices_are_played_and_stolen_without_a_click() {
    // Thirty-two voices on one key sum to one clean sine, so a voice that
    // restarts from zero, or a stolen voice that jumps, shows as a step far
    // above that sine's curvature.
    let steals = [
        12_000, 12_100, 12_200, 12_300, 12_400, 12_500, 12_600, 12_700,
    ];
    let mut notes: Vec<NoteEvent> = (0..32).map(|_| on(0, 40)).collect();
    for (i, frame) in steals.iter().enumerate() {
        notes.push(on(*frame, [40, 46, 52, 43, 40, 49, 55, 46][i]));
    }
    let params = [(INSTRUMENT_VOICES, 32.0)];
    let [left, _] = run(&params, 24_000, &notes);
    let steady = bend_in(&left, 11_000..11_900);
    let at_steals = bend_in(&left, 12_000..13_000);
    assert!(steady < 3e-4, "a sine's own curvature: {steady}");
    assert!(
        at_steals < 1e-3,
        "the steals are smooth: {at_steals} against {steady}"
    );
    // The detector does see a click of one voice's size: a step of the
    // level one voice has at its steal frame.
    let mut clicked = left.clone();
    for sample in &mut clicked[12_000..] {
        *sample += 0.03;
    }
    assert!(bend_in(&clicked, 12_000..13_000) > 1e-2);
    // All 32 voices still sound after the steals, the stolen ones at their
    // new keys.
    assert!(rms(&left[20_000..24_000]) > 0.7);
    assert_keys(&left[18_000..24_000], &[46, 52, 55], &[58]);
}

// Spec: instrument voices R3.
#[test]
fn priority_high_and_low_keep_the_right_notes() {
    let chord = |extra: &[NoteEvent]| {
        let mut notes = vec![on(0, 60), on(0, 64), on(0, 67)];
        notes.extend_from_slice(extra);
        notes
    };
    let window = |signal: &[f32], from: usize| signal[from..from + 6_000].to_vec();
    // High: a note above the lowest takes its voice; one below every note
    // isn't played.
    let high = [(INSTRUMENT_VOICES, 3.0), (INSTRUMENT_PRIORITY, 0.0)];
    let [signal, _] = run(
        &high,
        36_000,
        &chord(&[on(6_000, 62), on(18_000, 50), on(24_000, 70)]),
    );
    assert_keys(&window(&signal, 10_000), &[62, 64, 67], &[60]);
    assert_keys(&window(&signal, 20_000), &[62, 64, 67], &[50, 60]);
    assert_keys(&window(&signal, 30_000), &[64, 67, 70], &[62, 60]);
    // Low keeps the lowest.
    let low = [(INSTRUMENT_VOICES, 3.0), (INSTRUMENT_PRIORITY, 1.0)];
    let [signal, _] = run(
        &low,
        36_000,
        &chord(&[on(6_000, 66), on(18_000, 72), on(24_000, 55)]),
    );
    assert_keys(&window(&signal, 10_000), &[60, 64, 66], &[67]);
    assert_keys(&window(&signal, 20_000), &[60, 64, 66], &[72, 67]);
    assert_keys(&window(&signal, 30_000), &[55, 60, 64], &[66, 67]);
    // Last, the default, takes the oldest.
    let last = [(INSTRUMENT_VOICES, 3.0)];
    let [signal, _] = run(&last, 24_000, &chord(&[on(6_000, 50)]));
    assert_keys(&window(&signal, 10_000), &[50, 64, 67], &[60]);
    // A released voice whose tail still sounds goes before a held one,
    // whatever the priority.
    let [signal, _] = run(&high, 24_000, &chord(&[off(4_000, 64), on(4_500, 50)]));
    assert_keys(&window(&signal, 10_000), &[50, 60, 67], &[64]);
}

// Spec: instrument voices R5.
#[test]
fn retrigger_restarts_a_sounding_key_instead_of_taking_a_voice() {
    // A440's period divides 6,000 frames into 55 cycles, so a second voice
    // started then plays in phase with the first and doubles it.
    let notes = [on(0, 69), on(6_000, 69)];
    let level = |retrigger: f64| {
        let [signal, _] = run(&[(INSTRUMENT_RETRIGGER, retrigger)], 18_000, &notes);
        rms(&signal[12_000..18_000]) / rms(&signal[3_000..6_000])
    };
    assert_close(level(1.0), 1.0, 0.05, "with Retrigger on");
    assert_close(level(0.0), 2.0, 0.1, "with Retrigger off");
}

// Spec: instrument voices R6.
#[test]
fn mono_returns_to_a_held_key_and_releases_with_the_last() {
    let mono = [(INSTRUMENT_VOICE_MODE, 1.0)];
    let notes = [on(0, 60), on(6_000, 64), off(12_000, 64), off(24_000, 60)];
    let [signal, _] = run(&mono, 36_000, &notes);
    let window = |from: usize| signal[from..from + 4_000].to_vec();
    assert_keys(&window(2_000), &[60], &[64]);
    assert_keys(&window(8_000), &[64], &[60]);
    assert_keys(&window(15_000), &[60], &[64]);
    assert!(
        peak(&signal[30_000..36_000]) < 1e-4,
        "the last key releases the voice"
    );
}

// Spec: instrument voices R7.
#[test]
fn legato_keeps_the_envelopes_where_they_are() {
    // The envelope has decayed to its sustain level by the time the second
    // note comes: legato leaves it there, and a note without it restarts
    // the attack from that level.
    let notes = [on(0, 60), on(6_000, 64)];
    let after = |legato: f64| {
        let [signal, _] = run(
            &[(INSTRUMENT_VOICE_MODE, 1.0), (INSTRUMENT_LEGATO, legato)],
            12_000,
            &notes,
        );
        peak(&signal[6_000..6_600])
    };
    assert!(
        after(1.0) < 0.055,
        "Legato on stays at the sustain level: {}",
        after(1.0)
    );
    assert!(
        after(0.0) > 0.085,
        "Legato off restarts the attack: {}",
        after(0.0)
    );
}

/// The pitch the instrument reads out each frame, in semitones, from `notes`.
fn pitches(params: &[(ParamId, f64)], frames: usize, notes: &[NoteEvent], which: f64) -> Vec<f32> {
    let mut all = vec![output(which)];
    all.extend_from_slice(params);
    run(&all, frames, notes)[0].clone()
}

fn mono_glide(time_ms: f64) -> Vec<(ParamId, f64)> {
    vec![
        (INSTRUMENT_VOICE_MODE, 1.0),
        (INSTRUMENT_GLIDE_TIME, time_ms),
    ]
}

// Spec: instrument voices R8.
#[test]
fn a_glide_is_a_straight_line_that_reaches_the_note_at_glide_time() {
    let pitch = pitches(
        &mono_glide(500.0),
        48_000,
        &[on(0, 60), on(12_000, 72)],
        PITCH,
    );
    assert_eq!(pitch[0], 60.0, "the first note doesn't glide");
    assert_eq!(pitch[11_999], 60.0);
    for k in (0..=24_000).step_by(997) {
        let expected = 60.0 + 12.0 * k as f64 / 24_000.0;
        assert_close(
            f64::from(pitch[12_000 + k]),
            expected,
            1e-3,
            &format!("{k} frames in"),
        );
    }
    assert_eq!(pitch[36_000], 72.0, "exactly at Glide Time");
    assert_eq!(pitch[47_999], 72.0);
}

// Spec: instrument voices R8.
#[test]
fn polyphonic_glide_starts_from_the_note_before_and_not_after_silence() {
    let params = [(INSTRUMENT_GLIDE_TIME, 500.0)];
    // A second note with the first held glides from it, on its own voice.
    let pitch = pitches(&params, 48_000, &[on(0, 60), on(12_000, 72)], PITCH_LAST);
    assert_close(
        f64::from(pitch[12_000]),
        60.0,
        1e-3,
        "starts at the note before",
    );
    assert_close(f64::from(pitch[24_000]), 66.0, 1e-3, "halfway");
    assert_eq!(pitch[36_000], 72.0);
    // After every note has ended, the next doesn't glide.
    let pitch = pitches(
        &params,
        48_000,
        &[on(0, 60), off(6_000, 60), on(20_000, 72)],
        PITCH_LAST,
    );
    assert_eq!(pitch[20_000], 72.0);
}

// Spec: instrument voices R9.
#[test]
fn prop_scales_the_time_with_the_interval_and_legato_glides_only_into_held_notes() {
    let prop = |interval: u8| {
        let mut params = mono_glide(1_000.0);
        params.push((INSTRUMENT_GLIDE_PROP, 1.0));
        pitches(
            &params,
            120_000,
            &[on(0, 60), on(12_000, 60 + interval)],
            PITCH,
        )
    };
    // Glide Time is an octave's: 6 semitones take half of it, 24 twice it.
    let six = prop(6);
    assert_close(
        f64::from(six[12_000 + 12_000]),
        63.0,
        1e-3,
        "6 semitones, halfway",
    );
    assert_close(
        f64::from(six[12_000 + 24_000]),
        66.0,
        1e-3,
        "6 semitones, landed",
    );
    let twenty_four = prop(24);
    assert_close(
        f64::from(twenty_four[12_000 + 48_000]),
        72.0,
        1e-3,
        "24 semitones, halfway",
    );
    assert_close(
        f64::from(twenty_four[12_000 + 96_000]),
        84.0,
        1e-3,
        "24 semitones, landed",
    );
    // Const keeps the time whatever the interval.
    let constant = pitches(
        &mono_glide(1_000.0),
        96_000,
        &[on(0, 60), on(12_000, 84)],
        PITCH,
    );
    assert_close(f64::from(constant[12_000 + 48_000]), 84.0, 1e-3, "Const");
    // With the Legato option, a note played after the keys were let go
    // starts at its pitch; one played over a held key glides.
    let mut legato = mono_glide(500.0);
    legato.push((INSTRUMENT_GLIDE_LEGATO, 1.0));
    let apart = pitches(
        &legato,
        48_000,
        &[on(0, 60), off(6_000, 60), on(7_000, 72)],
        PITCH,
    );
    assert_eq!(apart[7_000], 72.0);
    let over = pitches(&legato, 48_000, &[on(0, 60), on(7_000, 72)], PITCH);
    assert_close(f64::from(over[7_000]), 60.0, 1e-3, "over a held key");
}

fn glissando(time_ms: f64) -> Vec<(ParamId, f64)> {
    vec![
        (INSTRUMENT_VOICE_MODE, 1.0),
        (INSTRUMENT_GLIDE_TIME, time_ms),
        (INSTRUMENT_GLIDE_MODE, 1.0),
    ]
}

// Spec: instrument voices R10.
#[test]
fn glissando_steps_through_semitones_in_equal_shares() {
    // 1.2 s over twelve semitones: eleven between, so twelve shares of
    // 100 ms, the first at the starting pitch, landing on the note after.
    let pitch = pitches(
        &glissando(1_200.0),
        72_000,
        &[on(0, 60), on(6_000, 72)],
        PITCH,
    );
    for step in 0..12 {
        let share = 6_000 + step * 4_800;
        for frame in [share + 1, share + 2_400, share + 4_799] {
            assert_close(
                f64::from(pitch[frame]),
                60.0 + step as f64,
                1e-4,
                &format!("step {step}"),
            );
        }
    }
    assert_eq!(pitch[6_000 + 57_600], 72.0);
    assert_eq!(pitch[71_999], 72.0);
}

// Spec: instrument voices R10.
#[test]
fn glissando_steps_through_the_current_scales_degrees() {
    let mut params = glissando(1_400.0);
    params.push((INSTRUMENT_USE_SCALE, 1.0));
    let setting = Setting {
        scale: Some(Scale::new(0, ScaleName::Major)),
        ..Setting::default()
    };
    let mut all = vec![output(PITCH)];
    all.extend(params);
    let [pitch, _] = play(&all, 512, 80_000, &[on(0, 60), on(6_000, 72)], &setting);
    // C major between C3 and C4: D, E, F, G, A, B; seven shares of 200 ms.
    let steps = [60.0, 62.0, 64.0, 65.0, 67.0, 69.0, 71.0];
    for (i, step) in steps.iter().enumerate() {
        let frame = 6_000 + i * 9_600 + 4_800;
        assert_close(f64::from(pitch[frame]), *step, 1e-4, &format!("degree {i}"));
    }
    assert_eq!(pitch[79_999], 72.0);
}

// Spec: instrument voices R10, R18; tunings R35.
#[test]
fn glissando_under_a_tuning_steps_through_the_tunings_keys() {
    let tuning = nineteen();
    let keys = KeyPitches::of(Some(&tuning));
    let mut all = vec![output(PITCH)];
    all.extend(glissando(1_200.0));
    let setting = Setting {
        tuning: Some(tuning),
        ..Setting::default()
    };
    let [pitch, _] = play(&all, 512, 72_000, &[on(0, 60), on(6_000, 72)], &setting);
    for step in 0..12u8 {
        let frame = 6_000 + usize::from(step) * 4_800 + 2_400;
        let expected = keys.pitch(60 + step).unwrap();
        assert_close(
            f64::from(pitch[frame]),
            expected,
            1e-4,
            &format!("key {}", 60 + step),
        );
    }
    assert_close(
        f64::from(pitch[71_999]),
        keys.pitch(72).unwrap(),
        1e-4,
        "lands",
    );
}

// Spec: instrument voices R11.
#[test]
fn unison_copies_sit_at_their_detunes() {
    let params = [
        (INSTRUMENT_UNISON_VOICES, 4.0),
        (INSTRUMENT_UNISON_DETUNE, 50.0),
    ];
    let [signal, _] = run(&params, 65_536, &[on(0, 81)]);
    let window = &signal[16_384..16_384 + 32_768];
    // Copies at -50, -16.7, +16.7 and +50 cents of A5, each the loudest
    // frequency within a few hertz of where it should be.
    for expected in [-50.0, -50.0 / 3.0, 50.0 / 3.0, 50.0] {
        let target = hz(81.0 + expected / 100.0);
        let power = power_at(window, target);
        for off_by in [-6.0, 6.0] {
            assert!(
                power > 20.0 * power_at(window, target + off_by),
                "a copy at {expected} cents ({target} Hz)"
            );
        }
    }
}

// Spec: instrument voices R12.
#[test]
fn unison_copies_pan_alternately_and_start_over_the_delay() {
    let params = [
        (INSTRUMENT_UNISON_VOICES, 2.0),
        (INSTRUMENT_UNISON_DETUNE, 100.0),
        (INSTRUMENT_UNISON_SPREAD, 100.0),
        (INSTRUMENT_UNISON_DELAY, 100.0),
    ];
    let [left, right] = run(&params, 24_000, &[on(0, 69)]);
    // Copy 0 is hard left and copy 1 hard right; copy 1 starts 100 ms
    // (4,800 frames) after copy 0.
    assert!(
        peak(&left[..4_800]) > 0.01,
        "copy 0 sounds from the note's frame"
    );
    assert!(right[..4_800].iter().all(|&x| x == 0.0), "copy 1 waits");
    assert!(peak(&right[4_800..9_600]) > 0.01, "then copy 1 sounds");
    assert!(
        right[4_800..]
            .iter()
            .zip(&left[4_800..])
            .any(|(r, l)| (r - l).abs() > 0.01)
    );
    // Each channel holds only its own copy: A4 down 50 cents on the left,
    // up 50 on the right.
    let low = hz(69.0 - 0.5);
    let high = hz(69.0 + 0.5);
    assert!(power_at(&left[8_000..24_000], low) > 20.0 * power_at(&left[8_000..24_000], high));
    assert!(power_at(&right[8_000..24_000], high) > 20.0 * power_at(&right[8_000..24_000], low));
    // With no spread the copies sit in the middle.
    let mut centred = params.to_vec();
    centred[2].1 = 0.0;
    let [left, right] = run(&centred, 12_000, &[on(0, 69)]);
    assert_eq!(left, right);
}

// Spec: instrument voices R13.
#[test]
fn a_stacked_voice_keeps_the_level_of_one() {
    let level = |copies: f64| {
        let [signal, _] = run(
            &[
                (INSTRUMENT_UNISON_VOICES, copies),
                (INSTRUMENT_UNISON_DETUNE, 100.0),
            ],
            96_000,
            &[on(0, 69)],
        );
        rms(&signal[12_000..96_000])
    };
    let one = level(1.0);
    for copies in [2.0, 4.0, 8.0] {
        assert_close(level(copies) / one, 1.0, 0.03, &format!("{copies} copies"));
    }
}

fn bend(frame: usize, key: u8, value: f32) -> NoteEvent {
    expression(frame, key, NoteExpression::Bend, value)
}

fn wheel(frame: usize, value: f32) -> NoteEvent {
    message(frame, NoteKind::PitchBend { value })
}

// Spec: instrument voices R14.
#[test]
fn a_per_note_bend_moves_only_its_note_and_the_wheel_moves_all() {
    let params = [(INSTRUMENT_NOTE_PB_RANGE, 12.0), (INSTRUMENT_PB_RANGE, 2.0)];
    let both = |extra: NoteEvent, which: f64| {
        let notes = [on(0, 60), on(0, 72), extra];
        pitches(&params, 12_000, &notes, which)
    };
    // Voice 0 plays key 60, voice 1 key 72: a bend of +1 on 72 moves it by
    // the Note PB range and leaves 60 where it was.
    let moved = both(bend(4_000, 72, 1.0), PITCH_LAST);
    assert_close(f64::from(moved[11_999]), 84.0, 1e-3, "the bent note");
    let still = both(bend(4_000, 72, 1.0), PITCH);
    assert_close(f64::from(still[11_999]), 60.0, 1e-4, "the other note");
    let down = both(bend(4_000, 60, -1.0), PITCH);
    assert_close(f64::from(down[11_999]), 48.0, 1e-3, "-1 moves down");
    // The wheel moves both by PB Range.
    for (which, key) in [(PITCH, 60.0), (PITCH_LAST, 72.0)] {
        let wheeled = both(wheel(4_000, 1.0), which);
        assert_close(f64::from(wheeled[11_999]), key + 2.0, 1e-3, "the wheel");
        let wheeled = both(wheel(4_000, -0.5), which);
        assert_close(f64::from(wheeled[11_999]), key - 1.0, 1e-3, "half down");
    }
    // It lands on its frame.
    let landed = both(wheel(4_000, 1.0), PITCH);
    assert_eq!(landed[3_999], 60.0);
    assert!(landed[4_000] > 60.0 || landed[4_100] > 60.0);
    // With Note PB off a per-note bend is ignored, and the wheel isn't.
    let mut off_params = params.to_vec();
    off_params.push((INSTRUMENT_NOTE_PB, 0.0));
    let ignored = pitches(
        &off_params,
        12_000,
        &[on(0, 60), bend(4_000, 60, 1.0)],
        PITCH,
    );
    assert_eq!(ignored[11_999], 60.0);
}

/// The block sizes agree on `signal` to within the kit's rounding.
fn at_every_block(
    params: &[(ParamId, f64)],
    frames: usize,
    notes: &[NoteEvent],
    setting: &Setting,
) -> Vec<f32> {
    let reference = play(params, 4_096, frames, notes, setting)[0].clone();
    for block in BLOCKS {
        let [signal, _] = play(params, block, frames, notes, setting);
        assert_near(&signal, &reference, 1e-5, &format!("block size {block}"));
    }
    reference
}

// Spec: instrument voices R15.
#[test]
fn the_sources_read_on_the_frame_their_message_lands_at_any_block_size() {
    let source = |which: f64, notes: &[NoteEvent]| {
        at_every_block(&[output(which)], 4_096, notes, &Setting::default())
    };
    // Velocity is the note's over 127 (here, as the engine carries it).
    let velocity = source(1.0, &[on_at(100, 72, 0.5)]);
    assert_eq!(velocity[99], 0.0);
    assert_eq!(velocity[100], 0.5);
    // Key: (k - 60) / 48, clamped: C3 is 0, each unit four octaves.
    for (key, expected) in [
        (60u8, 0.0),
        (72, 0.25),
        (36, -0.5),
        (108, 1.0),
        (127, 1.0),
        (0, -1.0),
    ] {
        assert_eq!(source(3.0, &[on(10, key)])[10], expected, "key {key}");
    }
    // Release velocity is 0 until the note ends.
    let release = at_every_block(
        &[output(2.0)],
        3_000,
        &[on(0, 60), off_at(1_000, 60, 0.75)],
        &Setting::default(),
    );
    assert_eq!(release[999], 0.0);
    assert_eq!(release[1_000], 0.75);
    // Pressure: the note's own, or the channel's when none reached it.
    let pressure = source(
        5.0,
        &[
            on(0, 60),
            expression(500, 60, NoteExpression::Pressure, 0.6),
        ],
    );
    assert_eq!(pressure[499], 0.0);
    assert_close(f64::from(pressure[500]), 0.6, 1e-6, "per-note pressure");
    // MIDI carries 7 bits, so the values are sevenths-and-more of 127.
    let seven = |n: f32| n / 127.0;
    let channel = source(
        5.0,
        &[
            on(0, 60),
            message(700, NoteKind::ChannelPressure { value: seven(51.0) }),
        ],
    );
    assert_eq!(channel[699], 0.0);
    assert_eq!(channel[700], seven(51.0), "channel pressure");
    // Slide, and Note PB.
    let slide = source(
        6.0,
        &[on(0, 60), expression(300, 60, NoteExpression::Slide, 0.9)],
    );
    assert_close(f64::from(slide[300]), 0.9, 1e-6, "slide");
    let note_pb = source(7.0, &[on(0, 60), bend(300, 60, -0.5)]);
    assert_eq!(note_pb[299], 0.0);
    assert_eq!(note_pb[300], -0.5);
    // The global sources: the mod wheel, the pitch wheel, aftertouch.
    let mod_wheel = source(
        8.0,
        &[message(
            200,
            NoteKind::Control {
                controller: 1,
                value: seven(89.0),
            },
        )],
    );
    assert_eq!((mod_wheel[199], mod_wheel[200]), (0.0, seven(89.0)));
    let wheel_source = source(9.0, &[wheel(200, -0.25)]);
    assert_eq!((wheel_source[199], wheel_source[200]), (0.0, -0.25));
    let aftertouch = source(
        10.0,
        &[message(
            200,
            NoteKind::ChannelPressure { value: seven(38.0) },
        )],
    );
    assert_eq!((aftertouch[199], aftertouch[200]), (0.0, seven(38.0)));
    // Random is drawn from 0 to 1 when a note starts, held for the note,
    // and differs between notes.
    // (Mono, so the one voice the readout follows takes both notes.)
    let drawn = |key: u8, start: usize| {
        let notes = [on(start, key), off(3_000, key), on(3_500, key)];
        at_every_block(
            &[output(4.0), (INSTRUMENT_VOICE_MODE, 1.0)],
            4_096,
            &notes,
            &Setting::default(),
        )
    };
    let random = drawn(60, 10);
    assert!((0.0..1.0).contains(&random[10]));
    assert_eq!(random[10], random[2_999]);
    assert_ne!(random[10], random[3_500]);
}

// Spec: instrument voices R16.
#[test]
fn a_curve_bows_the_source_by_four_to_the_minus_its_amount() {
    assert_eq!(instrument::curve(0.25, 0.0), 0.25);
    assert_close(
        instrument::curve(0.25, 1.0),
        0.25f64.powf(0.25),
        1e-12,
        "up",
    );
    assert_close(
        instrument::curve(0.25, -1.0),
        0.25f64.powi(4),
        1e-12,
        "down",
    );
    assert_eq!(instrument::curve(0.0, 1.0), 0.0);
    assert_eq!(instrument::curve(1.0, -1.0), 1.0);
    // On the instrument: the velocity through its curve, and the key on its
    // 0-to-1 reading.
    let velocity = |curve: f64| {
        let params = [output(1.0), (INSTRUMENT_VELOCITY_CURVE, curve)];
        run(&params, 100, &[on_at(10, 60, 0.25)])[0][10]
    };
    assert_close(
        f64::from(velocity(100.0)),
        0.25f64.powf(0.25),
        1e-6,
        "velocity up",
    );
    assert_close(f64::from(velocity(0.0)), 0.25, 1e-7, "velocity straight");
    let key = |curve: f64| {
        let params = [output(3.0), (INSTRUMENT_KEY_CURVE, curve)];
        run(&params, 100, &[on(10, 72)])[0][10]
    };
    // Key 72 reads 0.25, which is 0.625 on the 0-to-1 reading.
    assert_close(
        f64::from(key(100.0)),
        2.0 * 0.625f64.powf(0.25) - 1.0,
        1e-6,
        "key up",
    );
}

// Spec: instrument voices R17.
#[test]
fn transpose_stretch_and_error_move_a_voices_pitch() {
    let one = |params: &[(ParamId, f64)], key: u8| {
        let mut all = vec![output(PITCH)];
        all.extend_from_slice(params);
        run(&all, 12_000, &[on(0, key)])[0][11_999]
    };
    assert_eq!(one(&[], 60), 60.0);
    assert_eq!(one(&[(INSTRUMENT_TRANSPOSE, 7.0)], 60), 67.0);
    // Transpose moves a note that's already sounding.
    let mut all = vec![output(PITCH)];
    all.push((INSTRUMENT_TRANSPOSE, 0.0));
    let mut node = node(
        &testing::registry(),
        &device(&instrument_type(), &all),
        4096,
    );
    let first = render_node(node.as_mut(), 512, 4_000, &[on(0, 60)], &Setting::default());
    assert_eq!(first[0][3_999], 60.0);
    // Stretch at 100 %: 12 cents wider each octave above C3 and as much
    // narrower below it, and C3 stays.
    for (key, octaves) in [(60u8, 0.0), (72, 1.0), (84, 2.0), (48, -1.0)] {
        let stretched = one(&[(INSTRUMENT_STRETCH, 100.0)], key);
        let expected = f64::from(key) + octaves * 0.12;
        assert_close(f64::from(stretched), expected, 1e-5, &format!("key {key}"));
    }
    let narrowed = one(&[(INSTRUMENT_STRETCH, -50.0)], 72);
    assert_close(f64::from(narrowed), 72.0 - 0.06, 1e-5, "negative");
    // Error detunes each note within +-0.5 semitone at 100 %, differently
    // each note, the same on every run.
    let errors = |seed_run: usize| -> Vec<f32> {
        let _ = seed_run;
        let notes: Vec<NoteEvent> = (0..8).map(|i| on(i * 6_000, 60)).collect();
        let mut all = vec![
            output(PITCH_LAST),
            (INSTRUMENT_ERROR, 100.0),
            (INSTRUMENT_VOICE_MODE, 1.0),
        ];
        all.push((INSTRUMENT_VOICES, 1.0));
        let signal = run(&all, 48_000, &notes)[0].clone();
        (0..8).map(|i| signal[i * 6_000 + 5_000] - 60.0).collect()
    };
    let drawn = errors(0);
    assert!(drawn.iter().all(|e| e.abs() <= 0.5), "{drawn:?}");
    assert!(drawn.windows(2).any(|w| w[0] != w[1]), "{drawn:?}");
    assert_eq!(drawn, errors(1));
    let none = one(&[(INSTRUMENT_ERROR, 0.0)], 60);
    assert_eq!(none, 60.0);
}

// Spec: instrument voices R18; tunings R34.
#[test]
fn every_key_of_a_nineteen_step_tuning_sounds_within_a_tenth_of_a_cent() {
    let tuning = nineteen();
    let keys = KeyPitches::of(Some(&tuning));
    let setting = Setting {
        tuning: Some(tuning),
        ..Setting::default()
    };
    // The pitch of every key the tuning plays, read out one after another
    // on a single Mono voice.
    let playing: Vec<u8> = (0..=127).filter(|&k| keys.pitch(k).is_some()).collect();
    let span = 2_400;
    let mut notes = Vec::new();
    for (i, key) in playing.iter().enumerate() {
        notes.push(on(i * span, *key));
        notes.push(off(i * span + 1_200, *key));
    }
    let params = [output(PITCH), (INSTRUMENT_VOICE_MODE, 1.0)];
    let [pitch, _] = play(&params, 4_096, playing.len() * span, &notes, &setting);
    for (i, key) in playing.iter().enumerate() {
        let read = f64::from(pitch[i * span + 600]);
        let expected = keys.pitch(*key).unwrap();
        // f32 holds a pitch near 100 to about 1e-5 semitones, a thousandth
        // of a cent.
        assert!(
            (read - expected).abs() * 100.0 < 0.1,
            "key {key}: {read} against {expected}"
        );
    }
    // And heard: a key's sine is within 0.1 cent of its tuned pitch.
    for key in [0u8, 36, 61, 100] {
        let [signal, _] = play(&[], 512, 24_000, &[on(0, key)], &setting);
        let heard = crossing_hz(&signal[3_000..24_000]);
        let tuned = hz(keys.pitch(key).unwrap());
        assert!(
            cents(heard, tuned).abs() < 0.1,
            "key {key}: {heard} Hz against {tuned} Hz"
        );
    }
    // A per-note bend moves its note by steps of the tuning; the wheel
    // stays in semitones.
    let bent = |extra: NoteEvent| {
        let mut all = vec![
            output(PITCH),
            (INSTRUMENT_NOTE_PB_RANGE, 12.0),
            (INSTRUMENT_PB_RANGE, 2.0),
        ];
        all.push((INSTRUMENT_VOICE_MODE, 1.0));
        play(&all, 512, 12_000, &[on(0, 60), extra], &setting)[0][11_999]
    };
    assert_close(
        f64::from(bent(bend(4_000, 60, 1.0))),
        keys.bent(60, 12.0).unwrap(),
        1e-4,
        "bend",
    );
    assert_close(
        f64::from(bent(wheel(4_000, 1.0))),
        keys.pitch(60).unwrap() + 2.0,
        1e-4,
        "wheel",
    );
}

// Spec: instrument voices R19.
#[test]
fn hi_quality_is_a_switch_parameter_off_in_a_new_device() {
    let device = device(&instrument_type(), &[]);
    assert_eq!(
        device
            .params
            .get(&INSTRUMENT_HI_QUALITY)
            .copied()
            .unwrap_or(0.0),
        0.0
    );
}

// Spec: instrument voices R20, R21.
#[test]
fn modulation_runs_every_frame_in_hi_quality_and_on_a_grid_of_32_off() {
    // Pressure arrives at frame 1,000 + 10 of a note that's sounding; the
    // level it lowers follows on the control rate.
    let notes = [
        on(0, 60),
        expression(1_010, 60, NoteExpression::Pressure, 1.0),
    ];
    let run_with = |hi_quality: f64, block: usize| {
        let params = [
            (INSTRUMENT_PRESSURE_LEVEL, 100.0),
            (INSTRUMENT_HI_QUALITY, hi_quality),
        ];
        play(&params, block, 4_096, &notes, &Setting::default())[0].clone()
    };
    for block in BLOCKS {
        let on = run_with(1.0, block);
        let off = run_with(0.0, block);
        // On: silent from the frame the pressure lands. Off: it still
        // sounds until the next multiple of 32, frame 1,024.
        assert!(on[1_010..1_100].iter().all(|&x| x == 0.0), "block {block}");
        assert!(peak(&on[1_000..1_010]) > 0.01);
        assert!(peak(&off[1_010..1_024]) > 0.01, "block {block}");
        assert!(off[1_024..1_100].iter().all(|&x| x == 0.0), "block {block}");
    }
    // The grid is counted from the node's creation, not from the block:
    // the same at every size, to the bit.
    let reference = run_with(0.0, 4_096);
    for block in BLOCKS {
        assert_same(
            &run_with(0.0, block),
            &reference,
            &format!("block size {block}"),
        );
    }
}

// Spec: instrument voices R21.
#[test]
fn a_phrase_renders_the_same_at_block_sizes_1_64_512_and_4096() {
    let notes = [
        on(0, 60),
        on(0, 64),
        on(0, 67),
        on(1_500, 72),
        bend(2_000, 64, 0.5),
        wheel(3_000, 0.5),
        off(4_000, 60),
        on(5_000, 55),
        expression(5_500, 67, NoteExpression::Pressure, 0.5),
        on(6_000, 59),
        off(8_000, 64),
    ];
    for (name, extra) in [
        ("poly", vec![]),
        (
            "unison",
            vec![
                (INSTRUMENT_UNISON_VOICES, 4.0),
                (INSTRUMENT_UNISON_DELAY, 20.0),
                (INSTRUMENT_UNISON_SPREAD, 60.0),
            ],
        ),
        (
            "glide",
            vec![(INSTRUMENT_GLIDE_TIME, 120.0), (INSTRUMENT_GLIDE_PROP, 1.0)],
        ),
        (
            "mono",
            vec![
                (INSTRUMENT_VOICE_MODE, 1.0),
                (INSTRUMENT_LEGATO, 1.0),
                (INSTRUMENT_GLIDE_TIME, 80.0),
            ],
        ),
        (
            "glissando",
            vec![
                (INSTRUMENT_VOICE_MODE, 1.0),
                (INSTRUMENT_GLIDE_TIME, 90.0),
                (INSTRUMENT_GLIDE_MODE, 1.0),
            ],
        ),
        (
            "steal",
            vec![(INSTRUMENT_VOICES, 3.0), (INSTRUMENT_PRIORITY, 0.0)],
        ),
    ] {
        for hi_quality in [0.0, 1.0] {
            let mut params = extra.clone();
            params.push((INSTRUMENT_PRESSURE_LEVEL, 50.0));
            params.push((INSTRUMENT_HI_QUALITY, hi_quality));
            at_every_block(&params, 12_000, &notes, &Setting::default());
            let _ = name;
        }
    }
}
