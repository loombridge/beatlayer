//! Collision (WP-8.21; the collision spec, R1 to R20): the exciters, the
//! seven resonator types at their pitches and controls, the mixers, the two
//! structures, the LFOs and the MIDI/MPE sources through the matrix, the Key
//! and Vel amounts, the global section, the voice budget and block-size
//! independence.
//!
//! Every block runs as a real-time section under the allocation trap.

use std::time::Instant;

use crate::common::synth::{self, BLOCKS, HZ, amplitude_at, off, on, on_at, peak_frequency, rms};
use crate::common::{CHANNELS, RATE, device, node};
use bl_devices::collision::*;
use bl_engine::{Block, DeviceType, Node, NoteBuffer, NoteEvent, NoteExpression, NoteKind, TransportInfo};
use bl_model::{DeviceKind, ParamId};

/// A device's parameters, each with its value.
type Params<'a> = &'a [(ParamId, f64)];

fn collision() -> DeviceType {
    bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == COLLISION))
        .expect("Collision is registered")
}

/// A Collision with `params` set, rendered at block size `block`.
fn play_with(params: Params<'_>, block: usize, frames: usize, notes: &[NoteEvent]) -> [Vec<f32>; 2] {
    let mut node = node(&synth::registry(), &device(&collision(), params), 4_096);
    synth::render(node.as_mut(), block, frames, notes)
}

/// As [`play_with`] at 512 frames a block.
fn play(params: Params<'_>, frames: usize, notes: &[NoteEvent]) -> [Vec<f32>; 2] {
    play_with(params, 512, frames, notes)
}

/// Key 60's frequency: C3.
fn c3() -> f64 {
    synth::frequency(60)
}

/// The slice of `signal` from `from` to `to` seconds.
fn between(signal: &[f32], from: f64, to: f64) -> &[f32] {
    &signal[(from * HZ) as usize..(to * HZ) as usize]
}

/// A window of the left channel of `params` playing `key` for a second, from
/// `from` to `to` seconds.
fn window(params: Params<'_>, key: u8, from: f64, to: f64) -> Vec<f32> {
    between(&play(params, 48_000, &[on(0, key)])[0], from, to).to_vec()
}

/// The strongest frequency near `hz` (within 5 %) of `signal`.
fn pitch_near(signal: &[f32], hz: f64) -> f64 {
    peak_frequency(signal, hz * 0.95, hz * 1.05)
}

fn cents(a: f64, b: f64) -> f64 {
    1_200.0 * (a / b).log2()
}

fn type_param(r: usize, ty: usize) -> (ParamId, f64) {
    (res(r, R_TYPE), ty as f64)
}

const BEAM: usize = 0;
const MARIMBA: usize = 1;
const STRING: usize = 2;
const MEMBRANE: usize = 3;
const PLATE: usize = 4;
const PIPE: usize = 5;
const TUBE: usize = 6;

fn assert_same(actual: &[f32], expected: &[f32], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length");
    if let Some(i) = (0..actual.len()).find(|&i| actual[i].to_bits() != expected[i].to_bits()) {
        panic!("{what}: sample {i} is {} against {}", actual[i], expected[i]);
    }
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |peak, x| peak.max(x.abs()))
}

// Spec: collision R1, R17
#[test]
fn collision_is_an_instrument_with_its_tabs_its_matrix_and_every_parameter_banked() {
    let t = collision();
    assert_eq!(t.name, "Collision");
    assert_eq!(t.role, bl_model::DeviceRole::Instrument);
    t.description.check().expect("the description holds together");
    let layout = &t.description.layout;
    let pages: Vec<Vec<String>> = layout
        .pages()
        .iter()
        .map(|node| node.pages.iter().map(|p| p.name.clone()).collect())
        .collect();
    assert_eq!(
        pages,
        [["Mallet", "Noise", "Resonator 1", "Resonator 2", "LFO", "MIDI/MPE"]],
        "the tabs of R1"
    );
    // Every control is a parameter placed once and held by a bank.
    let banked: Vec<ParamId> = t.description.banks().into_iter().flat_map(|b| b.params).collect();
    for p in &t.description.params {
        assert_eq!(
            banked.iter().filter(|&&id| id == p.id).count(),
            1,
            "{} is in one bank",
            p.name
        );
    }
    // Twelve chosen slots: two for each LFO and each MIDI/MPE source.
    let matrix = t.matrix.as_ref().expect("it has a matrix");
    assert_eq!(matrix.slots.len(), 12);
    assert_eq!(
        matrix.sources.iter().map(|s| s.name).collect::<Vec<_>>(),
        ["LFO 1", "LFO 2", "Pitch Bend", "Mod Wheel", "Pressure", "Slide"]
    );
    for (n, dest) in matrix.dests.iter().enumerate() {
        let id = dest.param.expect("every destination is a parameter");
        assert_eq!(destination(id), (n + 1) as f64);
        assert_eq!(dest.name, t.description.param(id).unwrap().name);
    }
}

// Spec: collision R2
#[test]
fn a_stiffer_mallet_excites_the_upper_partials_and_noise_adds_to_them() {
    // The Beam's fifth sounding partial: 13.34 times the fundamental.
    let high = |params: Params<'_>| {
        let tone = window(params, 60, 0.0, 0.4);
        amplitude_at(&tone, c3() * 13.34) / amplitude_at(&tone, c3())
    };
    let soft = high(&[(MALLET_STIFFNESS, 70.0)]);
    let middle = high(&[(MALLET_STIFFNESS, 85.0)]);
    let hard = high(&[(MALLET_STIFFNESS, 100.0)]);
    assert!(soft < middle && middle < hard, "{soft} {middle} {hard}");
    assert!(hard > 8.0 * soft, "harder is a lot brighter: {soft} against {hard}");
    // Noise in the strike lifts the upper partials of a soft mallet, at
    // Color's frequency.
    let quiet = high(&[(MALLET_STIFFNESS, 0.0), (MALLET_NOISE, 0.0), (MALLET_COLOR, 3_500.0)]);
    let noisy = high(&[(MALLET_STIFFNESS, 0.0), (MALLET_NOISE, 100.0), (MALLET_COLOR, 3_500.0)]);
    let elsewhere = high(&[(MALLET_STIFFNESS, 0.0), (MALLET_NOISE, 100.0), (MALLET_COLOR, 250.0)]);
    assert!(noisy > 3.0 * quiet, "{quiet} against {noisy}");
    assert!(noisy > 3.0 * elsewhere, "Color picks the band: {elsewhere} against {noisy}");
    // Color does nothing with Noise at 0.
    let one = play(&[(MALLET_NOISE, 0.0), (MALLET_COLOR, 500.0)], 12_000, &[on(0, 60)]);
    let other = play(&[(MALLET_NOISE, 0.0), (MALLET_COLOR, 9_000.0)], 12_000, &[on(0, 60)]);
    assert_same(&one[0], &other[0], "Color with no noise");
    // Volume sets the strike's level.
    let loud = peak(&play(&[(MALLET_VOLUME, 1.0)], 12_000, &[on(0, 60)])[0]);
    let low = peak(&play(&[(MALLET_VOLUME, 0.25)], 12_000, &[on(0, 60)])[0]);
    assert!((loud / low - 4.0).abs() < 0.05, "{loud} {low}");
}

// Spec: collision R2 to R4
#[test]
fn with_both_exciters_off_or_both_resonators_off_it_is_silent() {
    let params = [(MALLET_ON, 0.0), (NOISE_ON, 0.0)];
    let [left, right] = play(&params, 12_000, &[on(0, 60)]);
    assert!(left.iter().chain(&right).all(|&x| x == 0.0), "no exciter");
    let params = [(res(0, R_ON), 0.0), (res(1, R_ON), 0.0)];
    let [left, right] = play(&params, 12_000, &[on(0, 60)]);
    assert!(left.iter().chain(&right).all(|&x| x == 0.0), "no resonator");
    let [left, _] = play(&[], 12_000, &[on(0, 60)]);
    assert!(peak(&left) > 0.01, "a new device sounds");
}

// Spec: collision R3
#[test]
fn the_noise_filters_its_types_and_envelope_shape_what_it_blows() {
    let blown = |params: &[(ParamId, f64)]| {
        let mut all = vec![
            (MALLET_ON, 0.0),
            (NOISE_ON, 1.0),
            (NOISE_SUSTAIN, 100.0),
            (NOISE_ATTACK, 0.0),
            (res(0, R_DECAY), 0.05),
            (res(0, R_MATERIAL), 100.0),
        ];
        all.extend_from_slice(params);
        play(&all, 48_000, &[on(0, 60)])[0][12_000..].to_vec()
    };
    // A Beam's partials 1 and 13.34 (the fifth sounding) show the filter.
    let tilt = |tone: &[f32]| amplitude_at(tone, c3() * 13.34) / amplitude_at(tone, c3());
    let lp_low = blown(&[(NOISE_FILTER, 0.0), (NOISE_FREQ, 400.0)]);
    let lp_high = blown(&[(NOISE_FILTER, 0.0), (NOISE_FREQ, 12_000.0)]);
    let hp = blown(&[(NOISE_FILTER, 1.0), (NOISE_FREQ, 6_000.0)]);
    assert!(tilt(&lp_low) < 0.1 * tilt(&lp_high), "LP cuts above its frequency");
    assert!(tilt(&hp) > 10.0 * tilt(&lp_low), "HP cuts below its frequency");
    // A band-pass at the upper partial leaves the fundamental alone.
    let bp = blown(&[(NOISE_FILTER, 2.0), (NOISE_FREQ, 3_490.0), (NOISE_RES, 80.0)]);
    assert!(tilt(&bp) > 10.0 * tilt(&lp_low), "BP passes its band");
    // LP+HP is a band: Res narrows it.
    let wide = blown(&[(NOISE_FILTER, 3.0), (NOISE_FREQ, 3_490.0), (NOISE_RES, 0.0)]);
    let narrow = blown(&[(NOISE_FILTER, 3.0), (NOISE_FREQ, 3_490.0), (NOISE_RES, 100.0)]);
    assert!(amplitude_at(&wide, c3()) > 3.0 * amplitude_at(&narrow, c3()));
    // The envelope: a slow Attack builds the noise, and with no Sustain it
    // dies away while the key is held.
    let attack = play(
        &[(MALLET_ON, 0.0), (NOISE_ON, 1.0), (NOISE_ATTACK, 500.0), (NOISE_SUSTAIN, 100.0), (res(0, R_DECAY), 0.05)],
        48_000,
        &[on(0, 60)],
    )[0]
    .clone();
    assert!(rms(between(&attack, 0.6, 0.8)) > 10.0 * rms(between(&attack, 0.0, 0.05)));
    let decaying = play(
        &[(MALLET_ON, 0.0), (NOISE_ON, 1.0), (NOISE_DECAY, 100.0), (NOISE_SUSTAIN, 0.0), (res(0, R_DECAY), 0.05)],
        48_000,
        &[on(0, 60)],
    )[0]
    .clone();
    assert!(rms(between(&decaying, 0.0, 0.1)) > 100.0 * rms(between(&decaying, 0.5, 0.9)));
    // Env Amt opens the filter with the envelope, up to six octaves.
    let swept = |amount: f64| {
        let tone = play(
            &[
                (MALLET_ON, 0.0), (NOISE_ON, 1.0), (NOISE_FILTER, 0.0), (NOISE_FREQ, 300.0),
                (NOISE_ENV_AMT, amount), (NOISE_SUSTAIN, 100.0), (NOISE_ATTACK, 0.0),
                (res(0, R_DECAY), 0.05), (res(0, R_MATERIAL), 100.0),
            ],
            24_000,
            &[on(0, 60)],
        )[0][4_800..].to_vec();
        tilt(&tone)
    };
    assert!(swept(100.0) > 10.0 * swept(0.0), "a positive amount opens it");
}

// Spec: collision R5, R6
#[test]
fn each_type_rings_at_the_notes_pitch() {
    for (ty, name) in [BEAM, MARIMBA, STRING, MEMBRANE, PLATE, PIPE, TUBE]
        .into_iter()
        .zip(["Beam", "Marimba", "String", "Membrane", "Plate", "Pipe", "Tube"])
    {
        for key in [48u8, 60, 72] {
            let tone = window(&[type_param(0, ty)], key, 0.05, 0.55);
            let found = pitch_near(&tone, synth::frequency(key));
            assert!(
                cents(found, synth::frequency(key)).abs() < 25.0,
                "{name} at key {key} rings at {found} Hz"
            );
        }
    }
}

// Spec: collision R6
#[test]
fn decay_material_inharm_the_positions_and_radius_move_what_rings() {
    // Decay.
    let tail = |params: Params<'_>| rms(&window(params, 60, 0.5, 0.9));
    assert!(tail(&[(res(0, R_DECAY), 4.0)]) > 30.0 * tail(&[(res(0, R_DECAY), 0.2)]));
    // Material: the upper partials of a metal bar outlast a wooden one's.
    let upper = |material: f64| {
        let tone = window(&[(res(0, R_MATERIAL), material), (res(0, R_DECAY), 1.0), (MALLET_STIFFNESS, 100.0)], 60, 0.3, 0.6);
        amplitude_at(&tone, c3() * 13.34) / amplitude_at(&tone, c3())
    };
    assert!(upper(100.0) > 30.0 * upper(0.0), "{} {}", upper(0.0), upper(100.0));
    // Inharm pushes the third sounding partial (5.404 times the fundamental)
    // up.
    let third = |inharm: f64| {
        let tone = window(&[(res(0, R_INHARM), inharm), (MALLET_STIFFNESS, 100.0)], 60, 0.0, 0.4);
        amplitude_at(&tone, c3() * 5.404) / amplitude_at(&tone, c3())
    };
    assert!(third(0.0) > 20.0 * third(100.0));
    assert!(third(0.0) > 20.0 * third(-100.0), "negative values bring it down");
    // Hit: a string struck at its middle leaves its even harmonics silent,
    // and struck a quarter in plays them.
    let even = |hit: f64| {
        let tone = window(&[type_param(0, STRING), (res(0, R_HIT), hit), (res(0, R_DECAY), 2.0)], 60, 0.1, 0.5);
        amplitude_at(&tone, c3() * 2.0) / amplitude_at(&tone, c3())
    };
    assert!(even(50.0) > 100.0 * even(0.0), "{} {}", even(0.0), even(50.0));
    // Pos. L and Pos. R listen at their own places, one to a channel.
    let [left, right] = play(
        &[type_param(0, STRING), (res(0, R_POS_L), 0.0), (res(0, R_POS_R), 50.0), (res(0, R_DECAY), 2.0)],
        24_000,
        &[on(0, 60)],
    );
    let second = |signal: &[f32]| amplitude_at(&signal[4_800..], c3() * 2.0) / amplitude_at(&signal[4_800..], c3());
    assert!(second(&right) > 100.0 * second(&left));
    // Radius lengthens a tube's decay.
    let wide = rms(&window(&[type_param(0, TUBE), (res(0, R_DECAY), 0.4), (res(0, R_RADIUS), 100.0)], 60, 0.6, 0.9));
    let thin = rms(&window(&[type_param(0, TUBE), (res(0, R_DECAY), 0.4), (res(0, R_RADIUS), 0.0)], 60, 0.6, 0.9));
    assert!(wide > 3.0 * thin, "{thin} against {wide}");
    // Opening: a closed pipe plays the odd harmonics of a note an octave
    // below, an open one every harmonic of the note. Struck and heard a
    // quarter in, where no low harmonic has a node.
    let pipe = |opening: f64| {
        let params = [
            type_param(0, PIPE), (res(0, R_OPENING), opening), (res(0, R_DECAY), 2.0),
            (res(0, R_HIT), 50.0), (res(0, R_POS_L), 50.0), (res(0, R_POS_R), 50.0),
        ];
        window(&params, 60, 0.1, 0.6)
    };
    let closed = pipe(0.0);
    assert!(amplitude_at(&closed, c3() / 2.0) > 1e-3, "it plays an octave below");
    assert!(amplitude_at(&closed, c3()) < amplitude_at(&closed, c3() / 2.0) / 100.0, "no even harmonic");
    assert!(amplitude_at(&closed, 1.5 * c3()) > 1e-3, "the third harmonic is there");
    let open = pipe(100.0);
    assert!(amplitude_at(&open, 2.0 * c3()) > 0.5 * amplitude_at(&open, c3()), "every harmonic");
    // Ratio changes a membrane's and a plate's partials.
    for ty in [MEMBRANE, PLATE] {
        let square = play(&[type_param(0, ty), (res(0, R_RATIO), 1.0)], 12_000, &[on(0, 60)]);
        let long = play(&[type_param(0, ty), (res(0, R_RATIO), 2.0)], 12_000, &[on(0, 60)]);
        assert_ne!(square[0], long[0], "Ratio moves type {ty}");
    }
}

// Spec: collision R6
#[test]
fn controls_that_dont_apply_to_a_type_change_nothing() {
    let same = |ty: usize, others: &[ParamId]| {
        let reference = play(&[type_param(0, ty)], 12_000, &[on(0, 60)]);
        for &id in others {
            let max = collision().description.param(id).unwrap().max();
            for value in [max, collision().description.param(id).unwrap().min()] {
                let got = play(&[type_param(0, ty), (id, value)], 12_000, &[on(0, 60)]);
                assert_same(&got[0], &reference[0], &format!("type {ty}, parameter {id:?} at {value}"));
            }
        }
    };
    same(BEAM, &[res(0, R_RADIUS), res(0, R_OPENING), res(0, R_RATIO)]);
    same(MARIMBA, &[res(0, R_RADIUS), res(0, R_OPENING), res(0, R_RATIO)]);
    same(MEMBRANE, &[res(0, R_RADIUS), res(0, R_OPENING)]);
    same(
        STRING,
        &[res(0, R_RADIUS), res(0, R_OPENING), res(0, R_RATIO), res(0, R_QUALITY), res(0, R_BRIGHTNESS)],
    );
    same(
        PIPE,
        &[res(0, R_MATERIAL), res(0, R_BRIGHTNESS), res(0, R_RATIO), res(0, R_QUALITY), res(0, R_INHARM)],
    );
    same(
        TUBE,
        &[res(0, R_MATERIAL), res(0, R_BRIGHTNESS), res(0, R_RATIO), res(0, R_QUALITY), res(0, R_OPENING), res(0, R_INHARM)],
    );
    // A switched-off resonator's controls change nothing either.
    let reference = play(&[(res(1, R_ON), 0.0)], 12_000, &[on(0, 60)]);
    let moved = play(&[(res(1, R_ON), 0.0), (res(1, R_DECAY), 30.0), (res(1, R_TUNE), 12.0)], 12_000, &[on(0, 60)]);
    assert_same(&moved[0], &reference[0], "Resonator 2 off");
}

// Spec: collision R7
#[test]
fn note_off_mutes_a_resonator_when_its_note_ends() {
    let after = |note_off: f64| {
        let notes = [on(0, 60), off(14_400, 60)];
        let [left, _] = play(&[(res(0, R_DECAY), 8.0), (res(0, R_NOTE_OFF), note_off)], 48_000, &notes);
        rms(&left[19_200..24_000])
    };
    let kept = after(0.0);
    let half = after(50.0);
    let muted = after(100.0);
    assert!(kept > 0.001, "at 0 % the note's end changes nothing");
    assert!(muted < kept / 1_000.0, "at 100 % it falls silent: {kept} against {muted}");
    assert!(muted < half && half < kept, "{kept} {half} {muted}");
    // And it falls within a few milliseconds.
    let notes = [on(0, 60), off(14_400, 60)];
    let [left, _] = play(&[(res(0, R_DECAY), 0.5), (res(0, R_NOTE_OFF), 100.0)], 48_000, &notes);
    assert!(rms(&left[14_400 + 1_440..14_400 + 2_400]) < rms(&left[13_000..14_400]) / 100.0);
}

// Spec: collision R8
#[test]
fn pitch_is_the_notes_times_key_plus_tune_fine_and_a_pitch_envelope() {
    let at = |params: Params<'_>, key: u8| {
        let mut all = vec![type_param(0, TUBE)];
        all.extend_from_slice(params);
        let tone = window(&all, key, 0.1, 0.5);
        peak_frequency(&tone, 40.0, 4_000.0)
    };
    let near = |found: f64, expected: f64, what: &str| {
        assert!(cents(found, expected).abs() < 20.0, "{what}: {found} against {expected}");
    };
    near(at(&[], 60), c3(), "C3");
    near(at(&[(res(0, R_KEY), 200.0)], 72), synth::frequency(84), "Key 200 % doubles the interval");
    near(at(&[(res(0, R_KEY), 0.0)], 72), c3(), "Key 0 % ignores the key");
    near(at(&[(res(0, R_KEY), -100.0)], 72), synth::frequency(48), "Key -100 % reverses it");
    near(at(&[(res(0, R_TUNE), 12.0)], 60), 2.0 * c3(), "Tune");
    near(at(&[(res(0, R_FINE), 50.0)], 60), c3() * 2f64.powf(50.0 / 1_200.0), "Fine");
    // The pitch envelope starts away and reaches the pitch in a straight
    // line in semitones.
    let start = {
        let mut all = vec![type_param(0, TUBE), (res(0, R_PITCH_ENV), 12.0), (res(0, R_PITCH_TIME), 10_000.0)];
        all.push((res(0, R_DECAY), 3.0));
        let tone = window(&all, 60, 0.0, 0.2);
        peak_frequency(&tone, 40.0, 4_000.0)
    };
    near(start, 2.0 * c3(), "a note starts at Pitch Env");
    near(
        at(&[(res(0, R_PITCH_ENV), 12.0), (res(0, R_PITCH_TIME), 100.0)], 60),
        c3(),
        "and has reached its pitch",
    );
}

// Spec: collision R9
#[test]
fn gain_pan_and_bleed_mix_each_resonator() {
    let [left, right] = play(&[(res(0, R_GAIN), 0.0)], 12_000, &[on(0, 60)]);
    assert!(left.iter().chain(&right).all(|&x| x == 0.0), "Gain at -inf");
    let level = |gain: f64| peak(&play(&[(res(0, R_GAIN), gain)], 12_000, &[on(0, 60)])[0]);
    assert!((level(2.0) / level(1.0) - 2.0).abs() < 0.01);
    let [left, right] = play(&[(res(0, R_PAN), -100.0)], 12_000, &[on(0, 60)]);
    assert!(peak(&left) > 0.01 && peak(&right) == 0.0, "hard left");
    let [left, right] = play(&[(res(0, R_PAN), 100.0)], 12_000, &[on(0, 60)]);
    assert!(peak(&right) > 0.01 && peak(&left) == 0.0, "hard right");
    // Bleed mixes the exciter's own signal into the output.
    let first = |bleed: f64| {
        let params = [(res(0, R_BLEED), bleed), (res(0, R_DECAY), 0.001), (MALLET_STIFFNESS, 100.0)];
        rms(&play(&params, 4_800, &[on(0, 60)])[0][..8])
    };
    assert!(first(100.0) > 3.0 * first(0.0), "{} {}", first(0.0), first(100.0));
}

// Spec: collision R11
#[test]
fn series_and_parallel_link_the_resonators_and_the_worst_case_stays_bounded() {
    // Resonator 2 is a Tube a fifth up. In parallel the exciters strike it;
    // in series it only hears Resonator 1, which has little at its pitch.
    let fifth = c3() * 2f64.powf(7.0 / 12.0);
    let linked = |structure: f64| {
        let tone = window(
            &[
                (res(1, R_ON), 1.0), type_param(1, TUBE), (res(1, R_TUNE), 7.0), (res(1, R_DECAY), 3.0),
                (STRUCTURE, structure), (res(0, R_DECAY), 3.0),
            ],
            60,
            0.1,
            0.6,
        );
        amplitude_at(&tone, fifth) / amplitude_at(&tone, c3())
    };
    let serial = linked(0.0);
    let parallel = linked(1.0);
    assert!(parallel > 3.0 * serial, "serial {serial}, parallel {parallel}");
    // Both are heard in series: Resonator 2 alone is quieter than both.
    let both = rms(&window(&[(res(1, R_ON), 1.0), type_param(1, TUBE)], 60, 0.0, 0.5));
    let one = rms(&window(&[(res(1, R_ON), 0.0)], 60, 0.0, 0.5));
    assert!(both > one, "{one} against {both}");
    // The worst case: both at full Decay and Material, High quality, tuned
    // the same, played loud in series for ten seconds. Output stays bounded
    // and, once the keys are released, doesn't grow.
    let worst = [
        (res(0, R_DECAY), 60.0), (res(0, R_MATERIAL), 100.0), (res(0, R_QUALITY), 3.0),
        (res(1, R_ON), 1.0), (res(1, R_DECAY), 60.0), (res(1, R_MATERIAL), 100.0), (res(1, R_QUALITY), 3.0),
        (NOISE_ON, 1.0), (NOISE_SUSTAIN, 100.0), (NOISE_FREQ, 6_000.0), (NOISE_VOLUME, 1.0),
        (MALLET_VOLUME, 1.0), (MALLET_STIFFNESS, 100.0), (STRUCTURE, 0.0), (VOLUME, 1.0),
    ];
    let notes: Vec<NoteEvent> = (0..6).map(|i| on(0, 48 + 5 * i)).collect();
    let released: Vec<NoteEvent> = notes.iter().map(|n| off(240_000, n.key)).collect();
    let all: Vec<NoteEvent> = notes.into_iter().chain(released).collect();
    let [left, right] = play_with(&worst, 512, 480_000, &all);
    let loudest = peak(&left).max(peak(&right));
    assert!(loudest.is_finite() && loudest < 16.0, "peak {loudest} is under +24 dBFS");
    assert!(left.iter().chain(&right).all(|x| x.is_finite()));
    let held = rms(&left[200_000..240_000]);
    let after = rms(&left[440_000..480_000]);
    assert!(after < held * 4.0, "{held} then {after}");
    // Every waveguide type in series too.
    for ty in [STRING, PIPE, TUBE] {
        let params = [
            type_param(0, ty), (res(0, R_DECAY), 60.0), type_param(1, ty), (res(1, R_ON), 1.0), (res(1, R_DECAY), 60.0),
            (NOISE_ON, 1.0), (NOISE_SUSTAIN, 100.0), (MALLET_VOLUME, 1.0), (VOLUME, 1.0),
        ];
        let [left, _] = play(&params, 240_000, &[on(0, 40), on(0, 52)]);
        assert!(peak(&left).is_finite() && peak(&left) < 16.0, "type {ty}: {}", peak(&left));
    }
}

/// An LFO's first destination, with its amount.
fn lfo_to(i: usize, dest: ParamId, amount: f64) -> Vec<(ParamId, f64)> {
    vec![
        (lfo(i, L_ON), 1.0),
        (lfo(i, L_SHAPE), 1.0),
        (lfo(i, L_RATE), 4.0),
        (lfo(i, L_DEST_A), destination(dest)),
        (lfo(i, L_AMOUNT_A), amount),
    ]
}

/// Whether, in the stretch of `signal` between `from` and `to` seconds, the
/// left channel is louder than the right.
fn leans_left(left: &[f32], right: &[f32], from: f64, to: f64) -> bool {
    rms(between(left, from, to)) > rms(between(right, from, to))
}

// Spec: collision R12, R14
#[test]
fn an_lfo_moves_its_destinations_by_its_shape_rate_and_amount() {
    // A square LFO at 4 Hz swings Pan from one side to the other every
    // 125 ms.
    let mut params = lfo_to(0, res(0, R_PAN), 100.0);
    params.push((res(0, R_DECAY), 4.0));
    let [left, right] = play(&params, 48_000, &[on(0, 60)]);
    assert!(!leans_left(&left, &right, 0.01, 0.12), "the first half of the cycle is on the right");
    assert!(leans_left(&left, &right, 0.14, 0.24), "the second on the left");
    assert!(!leans_left(&left, &right, 0.27, 0.37), "and round again");
    // Amount scales it, and 0 % of Amount stills it.
    let mut stilled = lfo_to(0, res(0, R_PAN), 100.0);
    stilled.push((lfo(0, L_AMOUNT), 0.0));
    let [left, right] = play(&stilled, 24_000, &[on(0, 60)]);
    assert!((rms(&left[4_800..]) / rms(&right[4_800..]) - 1.0).abs() < 1e-3, "no movement");
    // The switch: off, the routes do nothing.
    let mut off_lfo = lfo_to(0, res(0, R_PAN), 100.0);
    off_lfo.push((lfo(0, L_ON), 0.0));
    let [left, right] = play(&off_lfo, 24_000, &[on(0, 60)]);
    assert!((rms(&left[4_800..]) / rms(&right[4_800..]) - 1.0).abs() < 1e-3);
    // The second LFO and a negative amount swing the other way.
    let mut second = lfo_to(1, res(0, R_PAN), -100.0);
    second.push((res(0, R_DECAY), 4.0));
    let [left, right] = play(&second, 48_000, &[on(0, 60)]);
    assert!(leans_left(&left, &right, 0.01, 0.12), "negated");
    // Every shape stays inside its range and moves its destination.
    for shape in 0..7 {
        let mut params = lfo_to(0, res(0, R_FINE), 100.0);
        params.push((lfo(0, L_SHAPE), f64::from(shape)));
        params.push(type_param(0, TUBE));
        let [left, _] = play(&params, 24_000, &[on(0, 60)]);
        assert!(left.iter().all(|x| x.is_finite()) && peak(&left) > 0.005, "shape {shape}");
    }
    // Retrigger restarts it with a note; Offs. starts it later in the cycle.
    let phase = |offset: f64| {
        let mut params = lfo_to(0, res(0, R_PAN), 100.0);
        params.push((lfo(0, L_OFFSET), offset));
        params.push((res(0, R_DECAY), 4.0));
        let [left, right] = play(&params, 24_000, &[on(0, 60)]);
        leans_left(&left, &right, 0.01, 0.05)
    };
    assert!(!phase(0.0) && phase(180.0), "180 degrees starts on the other half");
    // The other LFO's Rate is a destination: LFO 2 moves LFO 1's.
    let mut routed = lfo_to(0, res(0, R_PAN), 100.0);
    routed.extend(lfo_to(1, lfo(0, L_RATE), 100.0));
    routed.push((res(0, R_DECAY), 4.0));
    let [left, right] = play(&routed, 48_000, &[on(0, 60)]);
    assert!(left.iter().chain(&right).all(|x| x.is_finite()));
}

// Spec: collision R13, R14, R16
#[test]
fn the_midi_sources_reach_their_destinations() {
    let wheel = |value: f32| NoteEvent {
        offset: 0,
        kind: NoteKind::PitchBend { value },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let expression = |expression, value: f32| NoteEvent {
        offset: 2_000,
        kind: NoteKind::Expression { expression, value },
        channel: 0,
        key: 60,
        velocity: 0.0,
        note_id: None,
    };
    let tube = [type_param(0, TUBE), (res(0, R_DECAY), 3.0)];
    let pitch = |notes: &[NoteEvent], extra: Params<'_>| {
        let mut all = tube.to_vec();
        all.extend_from_slice(extra);
        let [left, _] = play(&all, 48_000, notes);
        peak_frequency(&left[9_600..33_600], 40.0, 4_000.0)
    };
    // The wheel moves nothing by itself.
    let still = pitch(&[wheel(1.0), on(0, 60)], &[]);
    assert!(cents(still, c3()).abs() < 20.0, "{still}");
    // Routed to Tune, a quarter of its travel is 24 semitones.
    let routed = [(midi(PITCH_BEND, 0), destination(res(0, R_TUNE))), (midi(PITCH_BEND, 1), 25.0)];
    let bent = pitch(&[wheel(1.0), on(0, 60)], &routed);
    assert!(cents(bent, 4.0 * c3()).abs() < 25.0, "{bent}");
    let other = [(midi(PITCH_BEND, 2), destination(res(0, R_TUNE))), (midi(PITCH_BEND, 3), 25.0)];
    assert!(cents(pitch(&[wheel(1.0), on(0, 60)], &other), 4.0 * c3()).abs() < 25.0, "the second destination");
    // A per-note bend moves its note by 48 semitones per ±1.
    let note_bent = pitch(&[on(0, 60), expression(NoteExpression::Bend, 0.25)], &[]);
    let at_start = {
        let [left, _] = play(&tube, 48_000, &[on(0, 60), expression(NoteExpression::Bend, 0.25)]);
        peak_frequency(&left[9_600..33_600], 40.0, 4_000.0)
    };
    assert!((note_bent - at_start).abs() < 1.0);
    assert!(cents(note_bent, 2.0 * c3()).abs() < 25.0, "a quarter is an octave: {note_bent}");
    // The mod wheel, pressure and slide move Pan.
    let pan = |source: usize, notes: &[NoteEvent]| {
        let params = [
            (midi(source, 0), destination(res(0, R_PAN))),
            (midi(source, 1), 100.0),
            (res(0, R_DECAY), 4.0),
        ];
        let [left, right] = play(&params, 24_000, notes);
        (rms(&left[4_800..]), rms(&right[4_800..]))
    };
    let control = |value: f32| NoteEvent {
        offset: 0,
        kind: NoteKind::Control { controller: 1, value },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let (left, right) = pan(MOD_WHEEL, &[control(1.0), on(0, 60)]);
    assert!(right > 100.0 * left, "mod wheel up pans right: {left} {right}");
    let (left, right) = pan(MOD_WHEEL, &[control(0.0), on(0, 60)]);
    assert!((left / right - 1.0).abs() < 0.01, "mod wheel down centres it");
    let (left, right) = pan(PRESSURE, &[on(0, 60), expression(NoteExpression::Pressure, 1.0)]);
    assert!(right > 3.0 * left, "pressure: {left} {right}");
    let (left, right) = pan(SLIDE, &[on(0, 60), expression(NoteExpression::Slide, 1.0)]);
    assert!(right > 3.0 * left, "slide: {left} {right}");
}

// Spec: collision R16
#[test]
fn key_and_vel_move_the_controls_the_manual_names() {
    let n = |id: ParamId| {
        let t = collision();
        let p = t.description.params.iter().find(|p| p.id == id).unwrap().name.clone();
        let key = t.description.params.iter().find(|q| q.name == format!("{p} Key")).unwrap().id;
        let vel = t.description.params.iter().find(|q| q.name == format!("{p} Vel")).unwrap().id;
        (key, vel)
    };
    // Velocity into the mallet's Volume.
    let (_, vel) = n(MALLET_VOLUME);
    let level = |velocity: f32| peak(&play(&[(vel, 100.0), (MALLET_VOLUME, 0.002)], 12_000, &[on_at(0, 60, velocity)])[0]);
    assert!(level(1.0) > 1.5 * level(0.5), "{} {}", level(0.5), level(1.0));
    let none = |velocity: f32| peak(&play(&[], 12_000, &[on_at(0, 60, velocity)])[0]);
    assert_eq!(none(1.0), none(0.5), "with 0 % velocity does nothing");
    // The key into the mallet's Stiffness: Key at C3 is 0, so only other
    // keys move it. Resonators keep to C3 so only the mallet differs.
    let (key, _) = n(MALLET_STIFFNESS);
    let bright = |k: u8| {
        let tone = window(&[(key, 100.0), (res(0, R_KEY), 0.0), (MALLET_STIFFNESS, 80.0)], k, 0.0, 0.4);
        amplitude_at(&tone, c3() * 13.34) / amplitude_at(&tone, c3())
    };
    assert!(bright(84) > 10.0 * bright(36), "{} {} {}", bright(36), bright(60), bright(84));
    // The key into Pan, and velocity into Pitch Env.
    let (key, _) = n(res(0, R_PAN));
    let [left, right] = play(&[(key, 100.0)], 12_000, &[on(0, 84)]);
    assert!(rms(&right) > 100.0 * rms(&left), "a high key pans right");
    let (_, vel) = n(res(0, R_PITCH_ENV));
    let start = |velocity: f32| {
        let tone = window_with_velocity(&[type_param(0, TUBE), (vel, 25.0), (res(0, R_PITCH_TIME), 10_000.0), (res(0, R_DECAY), 3.0)], velocity);
        peak_frequency(&tone, 40.0, 4_000.0)
    };
    assert!(start(1.0) > 1.2 * start(0.0), "{} {}", start(0.0), start(1.0));
    // Every keyed control has both amounts.
    let t = collision();
    for id in [MALLET_VOLUME, MALLET_NOISE, NOISE_FREQ, lfo(0, L_RATE), lfo(1, L_AMOUNT), res(1, R_PAN)] {
        let (key, vel) = n(id);
        assert!(t.description.param(key).is_some() && t.description.param(vel).is_some());
    }
}

fn window_with_velocity(params: Params<'_>, velocity: f32) -> Vec<f32> {
    between(&play(params, 48_000, &[on_at(0, 60, velocity)])[0], 0.0, 0.2).to_vec()
}

// Spec: collision R15
#[test]
fn voices_retrigger_and_volume_follow_the_global_section() {
    // Voices 1: a second key takes the voice, so the first stops ringing.
    let notes = [on(0, 60), on(12_000, 67)];
    let ring = |voices: f64| {
        let [left, _] = play(&[(VOICES, voices), (res(0, R_DECAY), 4.0)], 36_000, &notes);
        amplitude_at(&left[24_000..], c3())
    };
    assert!(ring(8.0) > 100.0 * ring(1.0), "{} {}", ring(1.0), ring(8.0));
    // Retrig.: the same key again restarts its voice, so what follows is a
    // fresh note alone; without it a second voice joins the first.
    let again = [on(0, 60), on(12_000, 60)];
    let alone = play(&[], 24_000, &[on(12_000, 60)]);
    let restarted = play(&[(RETRIGGER, 1.0)], 24_000, &again);
    assert_same(&restarted[0][12_000..], &alone[0][12_000..], "a retriggered note");
    let joined = play(&[(RETRIGGER, 0.0)], 24_000, &again);
    assert_ne!(joined[0][12_000..], alone[0][12_000..], "a second voice sounds with the first");
    // Volume.
    let volume = |gain: f64| peak(&play(&[(VOLUME, gain)], 12_000, &[on(0, 60)])[0]);
    assert!((volume(1.0) / volume(0.5) - 2.0).abs() < 0.01);
    assert_eq!(peak(&play(&[(VOLUME, 0.0)], 12_000, &[on(0, 60)])[0]), 0.0);
}

// Spec: collision R20
#[test]
fn a_held_pipe_or_tube_blown_with_noise_keeps_sounding_until_the_key_ends() {
    for ty in [PIPE, TUBE] {
        let base = [
            (MALLET_ON, 0.0),
            (NOISE_ON, 1.0),
            (NOISE_FILTER, 2.0),
            (NOISE_FREQ, 900.0),
            (NOISE_RES, 40.0),
            type_param(0, ty),
            (res(0, R_DECAY), 0.5),
        ];
        let mut sustained = base.to_vec();
        sustained.push((NOISE_SUSTAIN, 100.0));
        let notes = [on(0, 60), off(96_000, 60)];
        let [left, _] = play(&sustained, 144_000, &notes);
        let held = rms(&left[72_000..96_000]);
        assert!(held > 0.005, "type {ty} keeps sounding: {held}");
        assert!(rms(&left[132_000..144_000]) < held / 20.0, "and stops after the key");
        let mut removed = base.to_vec();
        removed.push((NOISE_SUSTAIN, 0.0));
        let [left, _] = play(&removed, 144_000, &notes);
        assert!(rms(&left[72_000..96_000]) < held / 50.0, "with no Sustain it dies away");
    }
}

// Spec: collision R19
#[test]
fn the_output_is_the_same_at_every_block_size_and_run_after_run() {
    let params = [
        (res(0, R_DECAY), 2.0),
        type_param(0, MARIMBA),
        (res(1, R_ON), 1.0),
        type_param(1, PIPE),
        (res(1, R_OPENING), 40.0),
        (res(1, R_RADIUS), 30.0),
        (MALLET_NOISE, 40.0),
        (NOISE_ON, 1.0),
        (NOISE_FILTER, 3.0),
        (NOISE_ENV_AMT, 60.0),
        (NOISE_SUSTAIN, 50.0),
        (res(0, R_RND), 60.0),
        (res(1, R_PITCH_ENV), 5.0),
        (res(1, R_PITCH_TIME), 300.0),
        (lfo(0, L_ON), 1.0),
        (lfo(0, L_SHAPE), 6.0),
        (lfo(0, L_RATE), 7.0),
        (lfo(0, L_DEST_A), destination(res(0, R_PAN))),
        (lfo(0, L_AMOUNT_A), 80.0),
        (lfo(0, L_DEST_B), destination(res(1, R_FINE))),
        (lfo(0, L_AMOUNT_B), 40.0),
        (lfo(1, L_ON), 1.0),
        (lfo(1, L_SHAPE), 5.0),
        (lfo(1, L_RETRIGGER), 0.0),
        (lfo(1, L_DEST_A), destination(lfo(0, L_RATE))),
        (lfo(1, L_AMOUNT_A), 30.0),
        (lfo(1, L_DEST_B), destination(res(0, R_INHARM))),
        (lfo(1, L_AMOUNT_B), 30.0),
        (midi(MOD_WHEEL, 0), destination(res(0, R_DECAY))),
        (midi(MOD_WHEEL, 1), -40.0),
        (midi(PRESSURE, 0), destination(NOISE_FREQ)),
        (midi(PRESSURE, 1), 50.0),
        (VOICES, 4.0),
    ];
    let phrase = || {
        let wheel = NoteEvent {
            offset: 3_333,
            kind: NoteKind::Control { controller: 1, value: 0.7 },
            channel: 0,
            key: 0,
            velocity: 0.0,
            note_id: None,
        };
        let pressure = NoteEvent {
            offset: 6_001,
            kind: NoteKind::Expression { expression: NoteExpression::Pressure, value: 0.6 },
            channel: 0,
            key: 64,
            velocity: 0.0,
            note_id: None,
        };
        let bend = NoteEvent {
            offset: 4_099,
            kind: NoteKind::Expression { expression: NoteExpression::Bend, value: 0.1 },
            channel: 0,
            key: 60,
            velocity: 0.0,
            note_id: None,
        };
        vec![
            on(0, 60),
            on_at(1_000, 64, 0.5),
            on(2_049, 67),
            wheel,
            bend,
            pressure,
            off(9_000, 64),
            on_at(10_001, 72, 0.3),
            on(12_345, 48),
            on(14_000, 55),
            off(20_000, 60),
        ]
    };
    let reference = play_with(&params, 512, 36_000, &phrase());
    assert!(rms(&reference[0]) > 0.001, "it sounds");
    let again = play_with(&params, 4_096, 36_000, &phrase());
    for block in BLOCKS {
        let got = play_with(&params, block, 36_000, &phrase());
        assert_same(&got[0], &reference[0], &format!("left, block {block}"));
        assert_same(&got[1], &reference[1], &format!("right, block {block}"));
    }
    assert_same(&again[0], &reference[0], "run after run");
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes to
/// main, manual CI runs, and the nightly run. The device budget is judged only
/// then, so a loaded machine can't fail it.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Ok("quick") | Err(_) => false,
        Ok(other) => {
            panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full, or leave it unset")
        }
    }
}

/// One block of `frames` through `node`.
fn render_block(node: &mut dyn Node, frames: usize, notes: &[NoteEvent]) -> [Vec<f32>; 2] {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut sink = NoteBuffer::with_capacity(notes.len().max(1));
    {
        let mut audio = Block::new(&mut data, CHANNELS, frames, frames, &[], TransportInfo::default(), RATE)
            .with_notes(notes, &mut sink);
        bl_rt::rt_test!({ node.process(&mut audio) });
    }
    [data[..frames].to_vec(), data[frames..].to_vec()]
}

// Spec: collision R18
#[test]
fn sixteen_voices_with_both_resonators_at_medium_render_inside_the_device_budget() {
    let params = [
        (VOICES, 16.0),
        (res(0, R_ON), 1.0),
        (res(1, R_ON), 1.0),
        type_param(1, MARIMBA),
        (res(0, R_QUALITY), 2.0),
        (res(1, R_QUALITY), 2.0),
        (res(0, R_DECAY), 4.0),
        (res(1, R_DECAY), 4.0),
        (STRUCTURE, 1.0),
    ];
    let mut node = node(&synth::registry(), &device(&collision(), &params), 128);
    let chord: Vec<NoteEvent> = (0..16).map(|i| on(0, 36 + 3 * i)).collect();
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
    println!("sixteen voices of Collision: {realtime:.1}x real time");
    assert!(heard > 1.0, "it sounds");
    if full_depth() {
        assert!(realtime >= 4.0, "{realtime:.1}x real time");
    }
}

// Spec: collision R1 (factory presets)
#[test]
fn the_factory_presets_set_parameters_the_device_has_and_sound() {
    let description = collision().description;
    let presets = factory_presets();
    assert!(presets.len() >= 4);
    for (name, params) in presets {
        for (id, value) in &params {
            let p = description.param(*id).unwrap_or_else(|| panic!("{name}: {id:?}"));
            assert_eq!(p.clamp(*value), *value, "{name}: {} is in range", p.name);
        }
        let [left, _] = play(&params, 48_000, &[on(0, 57)]);
        assert!(peak(&left) > 0.02, "{name} sounds: {}", peak(&left));
    }
}

