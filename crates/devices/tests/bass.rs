//! Bass (WP-8.16; the bass and Poli spec, R1 to R8, R15, R17, R18): one
//! voice with a legato glide, the oscillator with its sub and noise, the
//! 24 dB filter with its circuits and envelope, the amplifier and the
//! distortion.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common::synth::{
    BLOCKS, HZ, aliasing_db, amplitude_at, assert_near, bend, envelope, frequency, off, on, on_at,
    peak_frequency, registry, render, rms, wheel,
};
use crate::common::{device, node};
use bl_devices::bass::*;
use bl_engine::NoteEvent;
use bl_model::param::db_to_gain;
use bl_model::{DeviceKind, DeviceRole, ParamId};

/// Parameters every test starts from: a saw through an open Clean filter, the
/// amplifier at full for the length of a note, nothing gliding.
const PLAIN: [(ParamId, f64); 8] = [
    (OCTAVE, 0.0),
    (GLIDE, 0.0),
    (FREQ, 20_000.0),
    (ENV_AMOUNT, 0.0),
    (ATTACK, 1.0),
    (SUSTAIN, 100.0),
    (RELEASE, 20.0),
    (VOLUME, 1.0),
];

/// `PLAIN` with `extra` laid over it.
fn params(extra: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut all: Vec<(ParamId, f64)> = PLAIN
        .iter()
        .copied()
        .filter(|(id, _)| !extra.iter().any(|(set, _)| set == id))
        .collect();
    all.extend_from_slice(extra);
    all
}

/// The left channel of `notes` played for `frames` on a Bass with `extra`
/// set over `PLAIN`, at `block` frames a block.
fn play_at(extra: &[(ParamId, f64)], block: usize, frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
    let mut node = node(&registry(), &device(&bass_type(), &params(extra)), 4096);
    render(node.as_mut(), block, frames, notes)[0].clone()
}

fn play(extra: &[(ParamId, f64)], frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
    play_at(extra, 512, frames, notes)
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

// Spec: bass and Poli R1, R17
#[test]
fn bass_is_an_instrument_that_describes_every_control_with_its_unit() {
    let kind = bass_type();
    assert_eq!(kind.name, "Bass");
    assert_eq!(kind.role, DeviceRole::Instrument);
    assert_eq!(
        kind.kind,
        DeviceKind::Native {
            device_type: BASS.to_owned()
        }
    );
    assert!(
        bl_devices::device_types()
            .iter()
            .any(|t| t.name == "Bass" && t.role == DeviceRole::Instrument),
        "it is registered"
    );
    kind.description.check().unwrap();
    for param in &kind.description.params {
        let shown = param.format(param.default);
        assert!(!shown.is_empty(), "{} shows its value", param.name);
        assert!(
            param.parse(&shown).is_some(),
            "{} reads {shown:?} back",
            param.name
        );
    }
    // The spec's ranges and defaults.
    let get = |id: ParamId| {
        kind.description
            .param(id)
            .unwrap_or_else(|| panic!("{id:?}"))
    };
    assert_eq!((get(GLIDE).min(), get(GLIDE).max()), (0.0, 2_000.0));
    assert_eq!(get(GLIDE).default, 60.0);
    assert_eq!((get(OCTAVE).min(), get(OCTAVE).max()), (-2.0, 1.0));
    assert_eq!(get(OCTAVE).default, -1.0);
    assert_eq!((get(TUNE).min(), get(TUNE).max()), (-12.0, 12.0));
    assert_eq!(get(SUB).default, 0.0);
    assert_eq!(get(NOISE).default, 0.0);
    assert_eq!((get(FREQ).min(), get(FREQ).max()), (20.0, 20_000.0));
    assert_eq!(get(FREQ).default, 400.0);
    assert_eq!(get(DISTORTION).default, 0.0);
    assert_eq!(
        (get(ENV_AMOUNT).min(), get(ENV_AMOUNT).max()),
        (-100.0, 100.0)
    );
    assert_eq!(
        (get(FILTER_DECAY).min(), get(FILTER_DECAY).max()),
        (0.0, 10_000.0)
    );
}

// Spec: bass and Poli R3
#[test]
fn the_oscillator_aliases_below_minus_80_db_at_the_top_key() {
    // Octave +1 on key 115 sounds key 127, where the band-limiting leaves the
    // fundamental alone; key 60 keeps many harmonics below Nyquist.
    for shape in [0.0, 100.0] {
        for (key, octave) in [(115u8, 1.0), (60, 0.0)] {
            let signal = play(&[(SHAPE, shape), (OCTAVE, octave)], 1 << 15, &[on(0, key)]);
            let sounding = key + 12 * octave as u8;
            let worst = aliasing_db(&signal[4_096..], frequency(sounding));
            assert!(
                worst < -80.0,
                "shape {shape} at key {sounding}: {worst:.1} dB"
            );
        }
    }
}

/// The frequency `signal[from..to]` plays, searched within a fifth of
/// `near`.
fn pitch_in(signal: &[f32], from: usize, to: usize, near: f64) -> f64 {
    peak_frequency(&signal[from..to], near * 0.8, near * 1.25)
}

// Spec: bass and Poli R4
#[test]
fn one_voice_plays_the_newest_key_and_returns_to_the_one_held() {
    let notes = [on(0, 60), on(12_000, 67), off(24_000, 67), off(36_000, 60)];
    let signal = play(&[], 40_000, &notes);
    let f60 = frequency(60);
    let f67 = frequency(67);
    assert!((pitch_in(&signal, 4_000, 12_000, f60) / f60 - 1.0).abs() < 0.01);
    let both = &signal[16_000..24_000];
    assert!((pitch_in(both, 0, both.len(), f67) / f67 - 1.0).abs() < 0.01);
    assert!(
        amplitude_at(both, f60) < 0.05 * amplitude_at(both, f67),
        "the held key's pitch is silent while the newest sounds"
    );
    let back = &signal[28_000..36_000];
    assert!((pitch_in(back, 0, back.len(), f60) / f60 - 1.0).abs() < 0.01);
}

// Spec: bass and Poli R5
#[test]
fn a_legato_note_glides_in_a_straight_line_without_retriggering_the_envelope() {
    // Key 60 to 72 over a second: halfway, six semitones up.
    let glide = [(GLIDE, 1_000.0)];
    let signal = play(&glide, 100_000, &[on(0, 60), on(24_000, 72)]);
    let halfway = &signal[24_000 + 22_000..24_000 + 26_000];
    let f66 = frequency(66);
    assert!(
        (pitch_in(halfway, 0, halfway.len(), f66) / f66 - 1.0).abs() < 0.04,
        "halfway through the glide the pitch is the middle of the interval"
    );
    let landed = &signal[24_000 + 56_000..24_000 + 64_000];
    let f72 = frequency(72);
    assert!((pitch_in(landed, 0, landed.len(), f72) / f72 - 1.0).abs() < 0.01);

    // The envelope has finished a 400 ms attack when the legato note comes: it
    // doesn't start over.
    let slow = [(ATTACK, 400.0), (GLIDE, 0.0)];
    let signal = play(&slow, 110_000, &[on(0, 60), on(96_000, 67)]);
    let before = rms(&signal[90_000..94_000]);
    let after = rms(&signal[96_000..100_000]);
    assert!(
        (after / before - 1.0).abs() < 0.05,
        "the level carries on: {before} then {after}"
    );
}

// Spec: bass and Poli R5
#[test]
fn a_note_after_silence_starts_at_its_pitch_and_restarts_the_envelopes() {
    let params = [(ATTACK, 400.0), (GLIDE, 1_000.0), (RELEASE, 20.0)];
    let notes = [on(0, 60), off(30_000, 60), on(60_000, 72)];
    let signal = play(&params, 100_000, &notes);
    let steady = rms(&signal[25_000..29_000]);
    assert!(
        rms(&signal[60_000..62_000]) < 0.3 * steady,
        "the attack starts over"
    );
    let f72 = frequency(72);
    let early = &signal[64_000..72_000];
    assert!(
        (pitch_in(early, 0, early.len(), f72) / f72 - 1.0).abs() < 0.02,
        "no glide from the note before after silence"
    );
}

/// Spec: bass and Poli R6. The amplitude of harmonic `n` over the
/// fundamental's, for a note of key 69.
fn harmonic(extra: &[(ParamId, f64)], n: f64) -> f64 {
    let signal = play(extra, 24_000, &[on(0, 69)]);
    let steady = &signal[6_000..22_000];
    amplitude_at(steady, 440.0 * n) / amplitude_at(steady, 440.0)
}

// Spec: bass and Poli R6
#[test]
fn shape_morphs_from_a_saw_to_a_square() {
    // A saw has every harmonic at 1/n; a square has only the odd ones.
    assert!((harmonic(&[(SHAPE, 0.0)], 2.0) - 0.5).abs() < 0.05);
    assert!(harmonic(&[(SHAPE, 100.0)], 2.0) < 0.02);
    assert!((harmonic(&[(SHAPE, 100.0)], 3.0) - 1.0 / 3.0).abs() < 0.04);
    let half = harmonic(&[(SHAPE, 50.0)], 2.0);
    assert!(
        half > 0.1 && half < 0.4,
        "halfway the second harmonic is {half}"
    );
}

// Spec: bass and Poli R6
#[test]
fn octave_and_tune_move_the_pitch_and_sub_adds_a_sine_an_octave_below() {
    let hz = |extra: &[(ParamId, f64)]| {
        let signal = play(extra, 24_000, &[on(0, 69)]);
        peak_frequency(&signal[6_000..22_000], 60.0, 1_800.0)
    };
    for (octave, expected) in [(-2.0, 110.0), (-1.0, 220.0), (0.0, 440.0), (1.0, 880.0)] {
        assert!(
            (hz(&[(OCTAVE, octave)]) / expected - 1.0).abs() < 0.01,
            "octave {octave}"
        );
    }
    for semitones in [-12.0, -5.0, 7.0, 12.0] {
        let expected = 440.0 * 2f64.powf(semitones / 12.0);
        assert!(
            (hz(&[(TUNE, semitones)]) / expected - 1.0).abs() < 0.01,
            "tune {semitones}"
        );
    }
    let sub = |gain: f64| {
        let signal = play(&[(SUB, gain)], 24_000, &[on(0, 69)]);
        amplitude_at(&signal[6_000..22_000], 220.0)
    };
    assert!(sub(0.0) < 0.01, "no sub by default");
    // A Hann-windowed sine of amplitude 1 reads 0.5.
    assert!(
        (sub(1.0) - 0.5).abs() < 0.04,
        "a full sub is a sine of amplitude 1: {}",
        sub(1.0)
    );
    assert!((sub(db_to_gain(-6.0)) - 0.25).abs() < 0.03);
    // The sub follows Octave: an octave below the oscillator, not the key.
    let signal = play(&[(SUB, 1.0), (OCTAVE, 1.0)], 24_000, &[on(0, 69)]);
    assert!((amplitude_at(&signal[6_000..22_000], 440.0) - 0.5).abs() < 0.04);
}

// Spec: bass and Poli R6
#[test]
fn noise_comes_from_the_devices_seed_and_fills_between_the_harmonics() {
    let quiet = play(&[], 24_000, &[on(0, 69)]);
    let noisy = play(&[(NOISE, 0.5)], 24_000, &[on(0, 69)]);
    assert_eq!(
        noisy,
        play(&[(NOISE, 0.5)], 24_000, &[on(0, 69)]),
        "the same run after run"
    );
    // Between the harmonics a saw has nothing and the noise has its level,
    // averaged over twenty frequencies off the harmonics.
    let between = |signal: &[f32]| {
        (0..20)
            .map(|k| amplitude_at(&signal[6_000..22_000], 661.0 + 97.0 * f64::from(k)))
            .sum::<f64>()
            / 20.0
    };
    assert!(between(&quiet) < 0.0005);
    assert!(between(&noisy) > 0.0015);
    assert!(rms(&noisy) > rms(&quiet));
}

/// The level of harmonic `n` of key `key` through the filter set as `extra`,
/// in dB.
fn harmonic_db(extra: &[(ParamId, f64)], key: u8, n: f64) -> f64 {
    let signal = play(extra, 24_000, &[on(0, key)]);
    db(amplitude_at(&signal[6_000..22_000], frequency(key) * n))
}

// Spec: bass and Poli R7
#[test]
fn the_filter_falls_24_db_an_octave_and_key_moves_the_cutoff() {
    // Key 43 sounds 98 Hz; its 16th harmonic, 1,568 Hz, is two octaves
    // short of one above a 400 Hz cutoff: 10 log(1 + (f / fc)^8) down.
    let open = harmonic_db(&[], 43, 16.0);
    let shut = harmonic_db(&[(FREQ, 400.0)], 43, 16.0);
    let fall = open - shut;
    let expected = 10.0 * (1.0 + (frequency(43) * 16.0 / 400.0).powi(8)).log10();
    assert!(
        (fall - expected).abs() < 2.0,
        "{fall:.1} dB against {expected:.1}"
    );

    // The 6th harmonic against the fundamental, an octave apart: with Key at
    // 100 % the cutoff follows, so the two notes sound alike.
    let tilt = |key: u8, tracking: f64| {
        let extra = [(FREQ, 1_500.0), (KEY, tracking)];
        harmonic_db(&extra, key, 6.0) - harmonic_db(&extra, key, 1.0)
    };
    assert!((tilt(60, 100.0) - tilt(72, 100.0)).abs() < 1.5);
    assert!((tilt(60, 0.0) - tilt(72, 0.0)).abs() > 6.0);
}

// Spec: bass and Poli R7
#[test]
fn the_filter_envelope_moves_the_cutoff_over_its_decay() {
    let bright = |extra: &[(ParamId, f64)], from: usize| {
        let mut all = vec![(FREQ, 200.0), (FILTER_ATTACK, 0.0), (FILTER_DECAY, 1_000.0)];
        all.extend_from_slice(extra);
        let signal = play(&all, 80_000, &[on(0, 48)]);
        amplitude_at(&signal[from..from + 4_000], frequency(48) * 8.0)
    };
    // At full the cutoff starts six octaves up and falls to the knob's value.
    let up = [(ENV_AMOUNT, 100.0)];
    assert!(
        bright(&up, 2_000) > 10.0 * bright(&up, 70_000),
        "falls over the decay"
    );
    // Negative moves it down, so the note opens as the envelope ends.
    let down = [(ENV_AMOUNT, -100.0)];
    assert!(bright(&down, 2_000) < bright(&down, 70_000));
    // At zero the envelope does nothing.
    let none = [(ENV_AMOUNT, 0.0)];
    let (a, b) = (bright(&none, 2_000), bright(&none, 70_000));
    assert!((a / b - 1.0).abs() < 0.02, "{a} and {b}");
}

// Spec: bass and Poli R7
#[test]
fn every_circuit_plays_bounded_and_drive_acts_on_all_but_clean() {
    let notes = [on(0, 40), on(12_000, 52)];
    let mut renders = Vec::new();
    for circuit in 0..5 {
        let push = [
            (CIRCUIT, f64::from(circuit)),
            (FREQ, 500.0),
            (RES, 90.0),
            (DRIVE, 50.0),
        ];
        let signal = play(&push, 24_000, &notes);
        let peak = signal.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));
        assert!(signal.iter().all(|x| x.is_finite()), "circuit {circuit}");
        assert!(
            peak > 0.05 && peak < 8.0,
            "circuit {circuit} peaks at {peak}"
        );
        renders.push(signal);
    }
    for pair in 0..4 {
        assert_ne!(
            renders[pair],
            renders[pair + 1],
            "circuits {pair} and {} sound alike",
            pair + 1
        );
    }
    let clean = |drive: f64| {
        play(
            &[(CIRCUIT, 0.0), (FREQ, 500.0), (DRIVE, drive)],
            12_000,
            &notes,
        )
    };
    assert_eq!(clean(0.0), clean(100.0), "Clean has no drive");
    let ms2 = |drive: f64| {
        play(
            &[(CIRCUIT, 2.0), (FREQ, 500.0), (DRIVE, drive)],
            12_000,
            &notes,
        )
    };
    assert_ne!(ms2(0.0), ms2(100.0));
}

/// The time `signal`'s envelope first reaches `fraction` of its peak, in
/// milliseconds.
fn rise_ms(signal: &[f32], fraction: f32) -> f64 {
    let envelope = envelope(signal);
    let peak = envelope.iter().copied().fold(0.0f32, f32::max);
    let at = envelope.iter().position(|&x| x >= fraction * peak).unwrap();
    at as f64 * 128.0 / HZ * 1000.0
}

// Spec: bass and Poli R8
#[test]
fn the_amplifier_envelope_rises_falls_holds_and_releases() {
    let rise = play(&[(ATTACK, 200.0)], 24_000, &[on(0, 57)]);
    let at_90 = rise_ms(&rise, 0.9);
    assert!(
        at_90 > 150.0 && at_90 < 215.0,
        "90 % of a 200 ms attack at {at_90} ms"
    );

    // Decay to a sustain of 25 %, against a sustain of 100 %.
    let sustained = |sustain: f64| {
        rms(&play(&[(DECAY, 50.0), (SUSTAIN, sustain)], 24_000, &[on(0, 57)])[12_000..])
    };
    assert!((sustained(25.0) / sustained(100.0) - 0.25).abs() < 0.01);

    // Release: still sounding 5 ms after the key, silent after 150 ms.
    let released = play(&[(RELEASE, 100.0)], 24_000, &[on(0, 57), off(8_000, 57)]);
    assert!(rms(&released[8_100..8_300]) > 0.3 * rms(&released[7_000..7_800]));
    assert!(rms(&released[16_000..]) < 1e-4, "silent after the release");
}

// Spec: bass and Poli R8
#[test]
fn vel_vol_and_volume_scale_the_level_as_drifts_law_does() {
    let level = |extra: &[(ParamId, f64)], velocity: f32| {
        rms(&play(extra, 12_000, &[on_at(0, 57, velocity)])[2_000..])
    };
    let full = level(&[(VEL_VOL, 100.0)], 1.0);
    assert!((level(&[(VEL_VOL, 100.0)], 0.5) / full - 0.5).abs() < 0.02);
    assert!((level(&[(VEL_VOL, 50.0)], 0.5) / full - 0.75).abs() < 0.02);
    assert!((level(&[(VEL_VOL, 0.0)], 0.5) / full - 1.0).abs() < 0.02);
    assert!((level(&[(VOLUME, 0.5)], 1.0) / level(&[], 1.0) - 0.5).abs() < 0.01);
    assert_eq!(level(&[(VOLUME, 0.0)], 1.0), 0.0, "Volume reaches silence");
}

// Spec: bass and Poli R8
#[test]
fn distortion_drives_a_soft_clipper_by_up_to_36_db_and_scales_back_by_half() {
    let notes = [on(0, 57)];
    let clean = play(&[], 12_000, &notes);
    assert_eq!(
        clean,
        play(&[(DISTORTION, 0.0)], 12_000, &notes),
        "off passes the signal as it is"
    );
    let peak = |signal: &[f32]| signal.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));

    // A small signal sits in the clipper's straight part: at 100 % it gains
    // 36 dB and gives back 18, 18 dB net.
    let quiet = [(VOLUME, db_to_gain(-60.0))];
    let mut driven = quiet.to_vec();
    driven.push((DISTORTION, 100.0));
    let gain = peak(&play(&driven, 12_000, &notes)) / peak(&play(&quiet, 12_000, &notes));
    assert!(
        (db(f64::from(gain)) - 18.0).abs() < 0.3,
        "{:.2} dB",
        db(f64::from(gain))
    );

    // A loud one is held under the clipper's ceiling, 1 over the root of the
    // drive's gain.
    let loud = play(&[(DISTORTION, 100.0)], 12_000, &notes);
    let ceiling = 1.0 / 10f64.powf(18.0 / 20.0);
    assert!(f64::from(peak(&loud)) <= ceiling + 1e-6);
    assert!(f64::from(peak(&loud)) > 0.8 * ceiling);
    // And the waveform it makes: a saw has its 2nd harmonic at one half of the
    // 1st; clipped hard it flattens towards a square, which has none.
    let second = |signal: &[f32]| {
        amplitude_at(&signal[4_000..], 440.0) / amplitude_at(&signal[4_000..], 220.0)
    };
    assert!((second(&clean) - 0.5).abs() < 0.05);
    assert!(second(&loud) < 0.2, "{}", second(&loud));
}

/// A phrase that moves no pitch: a legato note without glide, the envelopes,
/// the circuit with drive, noise and sub, the distortion.
fn still_phrase() -> ([(ParamId, f64); 11], Vec<NoteEvent>) {
    (
        [
            (NOISE, db_to_gain(-20.0)),
            (SUB, db_to_gain(-6.0)),
            (CIRCUIT, 2.0),
            (FREQ, 700.0),
            (RES, 60.0),
            (DRIVE, 40.0),
            (ENV_AMOUNT, 60.0),
            (FILTER_DECAY, 400.0),
            (DISTORTION, 30.0),
            (SHAPE, 40.0),
            (VEL_VOL, 50.0),
        ],
        vec![
            on(0, 40),
            on(6_000, 47),
            off(14_000, 47),
            off(16_000, 40),
            on(24_000, 52),
            off(30_000, 52),
        ],
    )
}

/// `base` with `over` laid over it.
fn params_over(base: &[(ParamId, f64)], over: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut all: Vec<(ParamId, f64)> = base
        .iter()
        .copied()
        .filter(|(id, _)| !over.iter().any(|(set, _)| set == id))
        .collect();
    all.extend_from_slice(over);
    all
}

// Spec: bass and Poli R15
#[test]
fn a_phrase_renders_the_same_at_every_block_size_and_every_run() {
    let (extra, notes) = still_phrase();
    let reference = play_at(&extra, 4_096, 40_000, &notes);
    assert!(rms(&reference) > 0.01);
    for block in BLOCKS {
        assert_near(
            &play_at(&extra, block, 40_000, &notes),
            &reference,
            1e-5,
            &format!("block size {block}"),
        );
    }
    assert!(play_at(&extra, 4_096, 40_000, &notes) == reference);
}

// Spec: bass and Poli R15
#[test]
fn a_phrase_that_moves_pitch_renders_the_same_at_block_sizes_64_512_and_4096() {
    // The kit's oscillator picks its mipmap from the pitch's rise over the
    // render call, so while a pitch moves a render in one-frame calls differs
    // from the others in a few samples, as Drift's does; from 64 up, the
    // calls are the same pieces and so are the renders.
    let (still, mut notes) = still_phrase();
    let mut extra = still.to_vec();
    extra.push((GLIDE, 120.0));
    notes.extend([bend(9_000, 47, 0.1), wheel(10_000, 0.5)]);
    let reference = play_at(&extra, 4_096, 40_000, &notes);
    assert!(rms(&reference) > 0.01);
    for block in [64, 512] {
        assert!(
            play_at(&extra, block, 40_000, &notes) == reference,
            "block size {block} differs"
        );
    }
}

// Spec: bass and Poli R18
#[test]
fn a_per_note_bend_moves_its_note_by_48_semitones_a_unit_and_the_wheel_by_pb_range() {
    let f60 = frequency(60);
    let bent = play(&[], 24_000, &[on(0, 60), bend(4_000, 60, 0.25)]);
    assert!(
        (pitch_in(&bent[8_000..24_000], 0, 16_000, f60 * 2.0) / (f60 * 2.0) - 1.0).abs() < 0.01
    );
    let down = play(&[], 24_000, &[on(0, 60), bend(4_000, 60, -0.125)]);
    let f54 = f60 * 2f64.powf(-6.0 / 12.0);
    assert!((pitch_in(&down[8_000..24_000], 0, 16_000, f54) / f54 - 1.0).abs() < 0.01);
    let wheeled = play(&[(PB_RANGE, 12.0)], 24_000, &[on(0, 60), wheel(4_000, 1.0)]);
    assert!(
        (pitch_in(&wheeled[8_000..24_000], 0, 16_000, f60 * 2.0) / (f60 * 2.0) - 1.0).abs() < 0.01
    );
    let still = play(&[(PB_RANGE, 0.0)], 24_000, &[on(0, 60), wheel(4_000, 1.0)]);
    assert!((pitch_in(&still[8_000..24_000], 0, 16_000, f60) / f60 - 1.0).abs() < 0.01);
}

// Spec: bass and Poli R1 (the factory presets)
#[test]
fn every_factory_preset_sets_parameters_in_range_and_plays() {
    let kind = bass_type();
    let all = presets();
    assert!(all.len() >= 3);
    for preset in all {
        for (id, value) in &preset.values {
            let param = kind.description.param(*id).expect("a parameter of Bass");
            assert!(
                (param.min()..=param.max()).contains(value),
                "{}: {} is {value}",
                preset.name,
                param.name
            );
        }
        let mut node = node(&registry(), &device(&kind, &preset.values), 512);
        let signal = render(node.as_mut(), 512, 24_000, &[on(0, 45), off(12_000, 45)])[0].clone();
        assert!(rms(&signal) > 0.005, "{} is silent", preset.name);
        assert!(
            signal.iter().all(|x| x.is_finite() && x.abs() < 4.0),
            "{}",
            preset.name
        );
    }
}

/// The frames of the golden render, and Bass's samples at them: its phrase
/// with Distortion at 3 %, rendered when the device was written.
const GOLDEN_AT: [usize; 12] = [
    1, 37, 101, 777, 4_001, 7_013, 12_345, 17_001, 26_001, 26_333, 27_777, 30_100,
];
const GOLDEN: [f32; 12] = [
    3.4496227e-6,
    0.871125,
    0.9135369,
    0.547285,
    -0.88623667,
    -0.8829688,
    0.8534715,
    0.0,
    -0.88761806,
    -0.9121562,
    -0.90679675,
    -0.7830151,
];

// Spec: bass and Poli R6 to R8 (the golden render)
#[test]
fn a_phrase_renders_as_it_did_when_the_device_was_written() {
    let (extra, notes) = still_phrase();
    let extra = params_over(&extra, &[(DISTORTION, 3.0)]);
    let signal = play_at(&extra, 512, 40_000, &notes);
    for (&at, &want) in GOLDEN_AT.iter().zip(&GOLDEN) {
        assert!(
            (signal[at] - want).abs() < 1e-4,
            "frame {at}: {} isn't {want}",
            signal[at]
        );
    }
}
