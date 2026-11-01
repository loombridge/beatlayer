//! Poli (WP-8.16; the bass and Poli spec, R1 to R3, R9 to R15, R17, R18):
//! up to sixteen voices of two detunable oscillators, the filter with its
//! envelope, the amplifier, the LFO with the mod wheel, and the chorus.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common::synth::{
    BLOCKS, aliasing_db, amplitude_at, assert_near, bend, frequency, mod_wheel, off, on, on_at,
    peak_frequency, registry, render, rms, wheel,
};
use crate::common::{device, node, run};
use bl_devices::chorus_ensemble::{self, chorus_ensemble_type};
use bl_devices::poli::*;
use bl_engine::NoteEvent;
use bl_model::param::db_to_gain;
use bl_model::{DeviceKind, DeviceRole, ParamId};

/// Parameters every test starts from: one saw through an open Clean filter,
/// the amplifier at full for the length of a note, no chorus.
const PLAIN: [(ParamId, f64); 10] = [
    (OSC2_LEVEL, 0.0),
    (OSC1_LEVEL, 1.0),
    (FREQ, 20_000.0),
    (ENV_AMOUNT, 0.0),
    (ATTACK, 1.0),
    (SUSTAIN, 100.0),
    (RELEASE, 20.0),
    (VOLUME, 1.0),
    (CHORUS, 0.0),
    (OSC2_DETUNE, 0.0),
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

/// Both channels of `notes` played for `frames` on a Poli with `extra` set
/// over `PLAIN`, at `block` frames a block.
fn play_both(
    extra: &[(ParamId, f64)],
    block: usize,
    frames: usize,
    notes: &[NoteEvent],
) -> [Vec<f32>; 2] {
    let mut node = node(&registry(), &device(&poli_type(), &params(extra)), 4096);
    render(node.as_mut(), block, frames, notes)
}

fn play(extra: &[(ParamId, f64)], frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
    play_both(extra, 512, frames, notes)[0].clone()
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

/// The frequency `signal` plays around frame `centre`, within 150 to 1,800 Hz.
fn pitch_at(signal: &[f32], centre: usize) -> f64 {
    peak_frequency(&signal[centre - 1_000..centre + 1_000], 150.0, 1_800.0)
}

// Spec: bass and Poli R1, R17
#[test]
fn poli_is_an_instrument_that_describes_every_control_with_its_unit() {
    let kind = poli_type();
    assert_eq!(kind.name, "Poli");
    assert_eq!(kind.role, DeviceRole::Instrument);
    assert_eq!(
        kind.kind,
        DeviceKind::Native {
            device_type: POLI.to_owned()
        }
    );
    assert!(
        bl_devices::device_types()
            .iter()
            .any(|t| t.name == "Poli" && t.role == DeviceRole::Instrument),
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
    let range = |id: ParamId| (get(id).min(), get(id).max(), get(id).default);
    assert_eq!(range(VOICES), (1.0, 16.0, 8.0));
    assert_eq!(range(GLIDE), (0.0, 2_000.0, 0.0));
    assert_eq!(range(PB_RANGE), (0.0, 24.0, 2.0));
    assert_eq!(range(OSC1_OCTAVE), (-2.0, 2.0, 0.0));
    assert_eq!(range(OSC2_OCTAVE), (-2.0, 2.0, 0.0));
    assert_eq!(range(OSC2_DETUNE), (-50.0, 50.0, 7.0));
    assert_eq!(range(PW), (5.0, 95.0, 50.0));
    assert_eq!(range(LFO_RATE).0, 0.05);
    assert_eq!(range(LFO_RATE).1, 20.0);
    assert_eq!((range(LFO_DELAY).0, range(LFO_DELAY).1), (0.0, 5_000.0));
    assert_eq!(range(WHEEL).0, 0.0);
    assert_eq!(range(WHEEL).1, 100.0);
    for amount in [LFO_PITCH, LFO_PW, LFO_FILTER, ENV_AMOUNT] {
        assert_eq!((range(amount).0, range(amount).1), (-100.0, 100.0));
    }
    assert_eq!(range(CHORUS).2, 1.0, "the chorus is on in a new device");
    assert_eq!(SHAPES, ["Saw", "Square", "Pulse"]);
    assert_eq!(LFO_SHAPES, ["Sine", "Triangle", "Square", "Saw", "S&H"]);
    assert_eq!(CIRCUITS, ["Clean", "MS2", "PRD"]);
}

// Spec: bass and Poli R3
#[test]
fn every_shape_aliases_below_minus_80_db_at_the_top_key() {
    for shape in 0..3 {
        for key in [127u8, 60] {
            let signal = play(
                &[(OSC1_SHAPE, f64::from(shape)), (PW, 25.0)],
                1 << 15,
                &[on(0, key)],
            );
            let worst = aliasing_db(&signal[4_096..], frequency(key));
            assert!(worst < -80.0, "shape {shape} at key {key}: {worst:.1} dB");
        }
    }
}

// Spec: bass and Poli R9
#[test]
fn it_plays_up_to_voices_notes_and_the_oldest_makes_way_for_a_new_one() {
    let level = |signal: &[f32], key: u8| amplitude_at(&signal[8_000..20_000], frequency(key));
    let chord = [on(0, 60), on(1_000, 64), on(2_000, 67), on(3_000, 72)];
    let all = play(&[], 24_000, &chord);
    for key in [60, 64, 67, 72] {
        assert!(
            level(&all, key) > 0.1,
            "key {key} sounds with room for four"
        );
    }
    let two = play(&[(VOICES, 2.0)], 24_000, &chord);
    for key in [60, 64] {
        assert!(
            level(&two, key) < 0.02,
            "key {key} was taken for a newer one"
        );
    }
    for key in [67, 72] {
        assert!(level(&two, key) > 0.1, "key {key} sounds");
    }
    // Sixteen voices at once, and a seventeenth.
    let big: Vec<NoteEvent> = (0..17).map(|i| on(i * 100, 36 + i as u8 * 3)).collect();
    let sixteen = play(&[(VOICES, 16.0), (OSC2_LEVEL, 1.0)], 24_000, &big);
    assert!(sixteen.iter().all(|x| x.is_finite()) && rms(&sixteen) > 0.1);
}

// Spec: bass and Poli R9
#[test]
fn glide_is_polyphonic_from_the_note_before_and_the_wheel_has_its_range() {
    // Key 66 over a held key 60: at the start of a 1 s glide it sounds near
    // 60, at its end at 66.
    let at_66 = |glide: f64, from: usize| {
        let signal = play(&[(GLIDE, glide)], 80_000, &[on(0, 60), on(10_000, 66)]);
        amplitude_at(&signal[from..from + 4_000], frequency(66))
    };
    assert!(
        at_66(1_000.0, 10_000) < 0.05,
        "gliding from the note before"
    );
    assert!(at_66(1_000.0, 10_000 + 60_000) > 0.2, "landed on its pitch");
    assert!(
        at_66(0.0, 10_000 + 2_000) > 0.2,
        "without Glide it starts there"
    );

    let f60 = frequency(60);
    let wheeled = play(&[(PB_RANGE, 12.0)], 24_000, &[on(0, 60), wheel(4_000, 1.0)]);
    assert!((pitch_at(&wheeled, 18_000) / (f60 * 2.0) - 1.0).abs() < 0.02);
    let still = play(&[(PB_RANGE, 0.0)], 24_000, &[on(0, 60), wheel(4_000, 1.0)]);
    assert!((pitch_at(&still, 18_000) / f60 - 1.0).abs() < 0.02);
    let two = play(&[], 24_000, &[on(0, 60), wheel(4_000, 1.0)]);
    let f62 = f60 * 2f64.powf(2.0 / 12.0);
    assert!(
        (pitch_at(&two, 18_000) / f62 - 1.0).abs() < 0.02,
        "2 semitones by default"
    );
}

// Spec: bass and Poli R10
#[test]
fn two_oscillators_beat_by_their_detune_and_have_their_own_shape_octave_and_level() {
    let both = |extra: &[(ParamId, f64)]| {
        let mut all = vec![(OSC2_LEVEL, 1.0)];
        all.extend_from_slice(extra);
        let signal = play(&all, 40_000, &[on(0, 69)]);
        signal[8_000..40_000].to_vec()
    };
    let detuned = both(&[(OSC2_DETUNE, 50.0)]);
    let f2 = 440.0 * 2f64.powf(50.0 / 1200.0);
    assert!(
        (amplitude_at(&detuned, 440.0) - std::f64::consts::FRAC_1_PI).abs() < 0.03,
        "first oscillator"
    );
    assert!(
        (amplitude_at(&detuned, f2) - std::f64::consts::FRAC_1_PI).abs() < 0.03,
        "the second, 50 cents up"
    );
    // 7 cents is the default: a beat of about 1.8 Hz in the fundamental.
    let default = {
        let mut all = vec![(OSC2_LEVEL, 1.0)];
        all.push((OSC2_DETUNE, 7.0));
        play(&all, 48_000, &[on(0, 69)])
    };
    let lows: Vec<f64> = (0..4)
        .map(|k| {
            amplitude_at(
                &default[6_000 + k * 9_000..6_000 + k * 9_000 + 6_000],
                440.0,
            )
        })
        .collect();
    let (min, max) = lows
        .iter()
        .fold((f64::MAX, 0.0f64), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(
        max > 1.3 * min,
        "the level of the fundamental beats: {lows:?}"
    );
    // Octave and level of the second.
    let up = both(&[(OSC2_OCTAVE, 1.0)]);
    assert!(amplitude_at(&up, 880.0) > 0.5 * amplitude_at(&up, 440.0));
    let down = both(&[(OSC2_OCTAVE, -2.0), (OSC1_LEVEL, 0.0)]);
    assert!((peak_frequency(&down, 60.0, 1_800.0) / 110.0 - 1.0).abs() < 0.02);
    let quiet = both(&[(OSC2_LEVEL, 0.0)]);
    assert!(amplitude_at(&quiet, f2) < 0.01 && amplitude_at(&quiet, 440.0) > 0.2);
    let half = both(&[(OSC2_LEVEL, db_to_gain(-6.0)), (OSC2_DETUNE, 50.0)]);
    assert!((amplitude_at(&half, f2) / amplitude_at(&half, 440.0) - 0.5).abs() < 0.05);
    // Shapes: a square has no even harmonics; a pulse of width 25 % none at
    // the fourth; a saw has them all.
    let harmonic = |shape: f64, width: f64, n: f64| {
        let signal = play(&[(OSC1_SHAPE, shape), (PW, width)], 24_000, &[on(0, 69)]);
        amplitude_at(&signal[6_000..22_000], 440.0 * n)
            / amplitude_at(&signal[6_000..22_000], 440.0)
    };
    assert!((harmonic(0.0, 50.0, 2.0) - 0.5).abs() < 0.03, "saw");
    assert!(harmonic(1.0, 50.0, 2.0) < 0.02, "square");
    assert!((harmonic(1.0, 50.0, 3.0) - 1.0 / 3.0).abs() < 0.03);
    assert!(harmonic(2.0, 25.0, 4.0) < 0.02, "a quarter pulse's fourth");
    assert!(harmonic(2.0, 25.0, 2.0) > 0.4);
    assert!(
        harmonic(2.0, 50.0, 2.0) < 0.02,
        "a pulse of half width is a square"
    );
}

/// The level of harmonic `n` of key `key` through the filter set as `extra`,
/// in dB.
fn harmonic_db(extra: &[(ParamId, f64)], key: u8, n: f64) -> f64 {
    let signal = play(extra, 24_000, &[on(0, key)]);
    db(amplitude_at(&signal[6_000..22_000], frequency(key) * n))
}

// Spec: bass and Poli R11
#[test]
fn the_filter_is_low_or_high_pass_at_12_or_24_db_and_key_follows_the_note() {
    // Key 43's 16th harmonic, 1,568 Hz, against a 400 Hz cutoff.
    let f = frequency(43) * 16.0;
    let open = harmonic_db(&[], 43, 16.0);
    for (slope, order) in [(0.0, 4), (1.0, 8)] {
        let shut = harmonic_db(&[(FREQ, 400.0), (SLOPE, slope)], 43, 16.0);
        let expected = 10.0 * (1.0 + (f / 400.0).powi(order)).log10();
        assert!(
            ((open - shut) - expected).abs() < 2.0,
            "slope {slope}: {:.1} dB against {expected:.1}",
            open - shut
        );
    }
    // High-pass: the fundamental of a low note falls, a high harmonic stays.
    let high = [(FILTER_TYPE, 1.0), (FREQ, 1_000.0)];
    assert!(harmonic_db(&high, 43, 1.0) < harmonic_db(&[], 43, 1.0) - 30.0);
    assert!((harmonic_db(&high, 43, 40.0) - harmonic_db(&[], 43, 40.0)).abs() < 1.0);
    // Key at 100 % moves the cutoff with the note.
    let tilt = |key: u8, tracking: f64| {
        let extra = [(FREQ, 1_500.0), (KEY, tracking)];
        harmonic_db(&extra, key, 6.0) - harmonic_db(&extra, key, 1.0)
    };
    assert!((tilt(60, 100.0) - tilt(72, 100.0)).abs() < 1.5);
    assert!((tilt(60, 0.0) - tilt(72, 0.0)).abs() > 6.0);
    // The three circuits all play, bounded, and sound apart.
    let notes = [on(0, 40), on(6_000, 52)];
    let mut renders = Vec::new();
    for circuit in 0..3 {
        let signal = play(
            &[
                (CIRCUIT, f64::from(circuit)),
                (FREQ, 500.0),
                (RES, 90.0),
                (OSC2_LEVEL, 1.0),
            ],
            24_000,
            &notes,
        );
        let peak = signal.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));
        assert!(signal.iter().all(|x| x.is_finite()), "circuit {circuit}");
        assert!(
            peak > 0.05 && peak < 8.0,
            "circuit {circuit} peaks at {peak}"
        );
        renders.push(signal);
    }
    assert_ne!(renders[0], renders[1]);
    assert_ne!(renders[1], renders[2]);
}

// Spec: bass and Poli R11
#[test]
fn the_filter_envelope_has_four_stages_and_moves_the_cutoff() {
    // Brightness is the 8th harmonic of key 48.
    let bright = |extra: &[(ParamId, f64)], from: usize| {
        let mut all = vec![
            (FREQ, 200.0),
            (FILTER_ATTACK, 0.0),
            (FILTER_DECAY, 500.0),
            (FILTER_RELEASE, 300.0),
        ];
        all.extend_from_slice(extra);
        let signal = play(&all, 100_000, &[on(0, 48), off(70_000, 48)]);
        amplitude_at(&signal[from..from + 4_000], frequency(48) * 8.0)
    };
    // Decaying to a sustain of 0: bright early, dark in the hold.
    let falling = [(ENV_AMOUNT, 100.0), (FILTER_SUSTAIN, 0.0)];
    assert!(bright(&falling, 1_000) > 10.0 * bright(&falling, 50_000));
    // A sustain of 100 % holds the cutoff up.
    let held = [(ENV_AMOUNT, 100.0), (FILTER_SUSTAIN, 100.0)];
    assert!(bright(&held, 50_000) > 5.0 * bright(&falling, 50_000));
    // The release closes it again after the key.
    assert!(bright(&held, 66_000) > 5.0 * bright(&held, 90_000));
    // Negative amounts darken.
    let down = [(ENV_AMOUNT, -100.0), (FILTER_SUSTAIN, 0.0)];
    assert!(bright(&down, 1_000) < bright(&down, 50_000));
}

/// The time `signal`'s envelope first reaches `fraction` of its peak, in
/// milliseconds.
fn rise_ms(signal: &[f32], fraction: f32) -> f64 {
    let envelope = crate::common::synth::envelope(signal);
    let peak = envelope.iter().copied().fold(0.0f32, f32::max);
    let at = envelope.iter().position(|&x| x >= fraction * peak).unwrap();
    at as f64 * 128.0 / 48_000.0 * 1000.0
}

// Spec: bass and Poli R12
#[test]
fn the_amplifier_is_an_adsr_with_vel_vol_and_volume() {
    let rise = play(&[(ATTACK, 200.0)], 24_000, &[on(0, 57)]);
    let at_90 = rise_ms(&rise, 0.9);
    assert!(
        at_90 > 150.0 && at_90 < 215.0,
        "90 % of a 200 ms attack at {at_90} ms"
    );
    let sustained = |sustain: f64| {
        rms(&play(&[(DECAY, 50.0), (SUSTAIN, sustain)], 24_000, &[on(0, 57)])[12_000..])
    };
    assert!((sustained(25.0) / sustained(100.0) - 0.25).abs() < 0.01);
    let released = play(&[(RELEASE, 100.0)], 24_000, &[on(0, 57), off(8_000, 57)]);
    assert!(rms(&released[8_100..8_300]) > 0.3 * rms(&released[7_000..7_800]));
    assert!(rms(&released[16_000..]) < 1e-4, "silent after the release");
    let level = |extra: &[(ParamId, f64)], velocity: f32| {
        rms(&play(extra, 12_000, &[on_at(0, 57, velocity)])[2_000..])
    };
    let full = level(&[(VEL_VOL, 100.0)], 1.0);
    assert!((level(&[(VEL_VOL, 100.0)], 0.5) / full - 0.5).abs() < 0.02);
    assert!((level(&[(VEL_VOL, 50.0)], 0.5) / full - 0.75).abs() < 0.02);
    assert!((level(&[(VOLUME, 0.5)], 1.0) / level(&[], 1.0) - 0.5).abs() < 0.01);
    assert_eq!(level(&[(VOLUME, 0.0)], 1.0), 0.0, "Volume reaches silence");
}

/// The LFO's own settings for a vibrato: a sine at 1 Hz sending `amount`
/// to the pitch.
fn vibrato(amount: f64) -> Vec<(ParamId, f64)> {
    vec![
        (LFO_SHAPE, 0.0),
        (LFO_SYNC, 0.0),
        (LFO_RATE, 1.0),
        (LFO_PITCH, amount),
    ]
}

// Spec: bass and Poli R13
#[test]
fn the_lfo_moves_the_pitch_up_to_an_octave_and_restarts_with_each_note() {
    // A sine at 1 Hz is at its top at 250 ms and its bottom at 750 ms.
    let signal = play(&vibrato(100.0), 60_000, &[on(0, 69)]);
    assert!(
        (pitch_at(&signal, 12_000) / 880.0 - 1.0).abs() < 0.04,
        "an octave up"
    );
    assert!(
        (pitch_at(&signal, 36_000) / 220.0 - 1.0).abs() < 0.04,
        "an octave down"
    );
    let half = play(&vibrato(-50.0), 60_000, &[on(0, 69)]);
    let up = 440.0 * 2f64.powf(-6.0 / 12.0);
    assert!(
        (pitch_at(&half, 12_000) / up - 1.0).abs() < 0.04,
        "negative flips it"
    );
    // A second note, long after the first, starts its LFO over.
    let two = play(
        &vibrato(100.0),
        120_000,
        &[on(0, 69), off(2_000, 69), on(60_777, 69)],
    );
    assert!((pitch_at(&two, 60_777 + 12_000) / 880.0 - 1.0).abs() < 0.04);
    // Sync by the tempo's division: a quarter note at 120 bpm is 2 Hz.
    let synced = play(
        &[(LFO_SYNC, 1.0), (LFO_DIVISION, 2.0), (LFO_PITCH, 100.0)],
        30_000,
        &[on(0, 69)],
    );
    let free = play(&[(LFO_RATE, 2.0), (LFO_PITCH, 100.0)], 30_000, &[on(0, 69)]);
    assert_near(
        &synced,
        &free,
        1e-3,
        "synced to a quarter note, and at 2 Hz",
    );
}

// Spec: bass and Poli R13
#[test]
fn the_lfo_fades_in_over_its_delay_and_the_wheel_scales_its_depth() {
    let mut delayed = vibrato(100.0);
    delayed.push((LFO_DELAY, 1_000.0));
    let signal = play(&delayed, 70_000, &[on(0, 69)]);
    // A quarter of the way in, a quarter of the depth: 3 semitones.
    let quarter = 440.0 * 2f64.powf(3.0 / 12.0);
    assert!((pitch_at(&signal, 12_000) / quarter - 1.0).abs() < 0.04);
    assert!(
        (pitch_at(&signal, 60_000) / 880.0 - 1.0).abs() < 0.04,
        "the whole of it later"
    );

    let mut wheeled = vibrato(100.0);
    wheeled.push((WHEEL, 100.0));
    let down = play(&wheeled, 24_000, &[mod_wheel(0, 0.0), on(0, 69)]);
    assert!(
        (pitch_at(&down, 12_000) / 440.0 - 1.0).abs() < 0.02,
        "wheel down, no LFO"
    );
    let up = play(&wheeled, 24_000, &[mod_wheel(0, 1.0), on(0, 69)]);
    assert!(
        (pitch_at(&up, 12_000) / 880.0 - 1.0).abs() < 0.04,
        "wheel up, all of it"
    );
    let mid = play(&wheeled, 24_000, &[mod_wheel(0, 0.5), on(0, 69)]);
    let fifth = 440.0 * 2f64.powf(6.0 / 12.0);
    assert!(
        (pitch_at(&mid, 12_000) / fifth - 1.0).abs() < 0.04,
        "half the wheel, half the depth"
    );
    // At 0 % the wheel does nothing.
    let mut free = vibrato(100.0);
    free.push((WHEEL, 0.0));
    let none = play(&free, 24_000, &[mod_wheel(0, 0.0), on(0, 69)]);
    assert!((pitch_at(&none, 12_000) / 880.0 - 1.0).abs() < 0.04);
}

// Spec: bass and Poli R13
#[test]
fn the_lfo_moves_a_pulses_width_and_the_cutoff() {
    // A pulse of half width has no 2nd harmonic; at the top of the LFO its
    // width is 95 % and the 2nd is as strong as the 5 % pulse's.
    let width = |from: usize| {
        let signal = play(
            &[
                (OSC1_SHAPE, 2.0),
                (LFO_SHAPE, 0.0),
                (LFO_SYNC, 0.0),
                (LFO_RATE, 1.0),
                (LFO_PW, 100.0),
            ],
            24_000,
            &[on(0, 57)],
        );
        let window = &signal[from..from + 2_000];
        amplitude_at(window, 440.0) / amplitude_at(window, 220.0)
    };
    assert!(
        width(11_000) > 5.0 * width(0),
        "the width swings the harmonics"
    );
    // The cutoff, four octaves either way.
    let bright = |from: usize| {
        let signal = play(
            &[(FREQ, 1_000.0), (LFO_RATE, 1.0), (LFO_FILTER, 100.0)],
            48_000,
            &[on(0, 48)],
        );
        amplitude_at(&signal[from..from + 2_000], frequency(48) * 8.0)
    };
    assert!(bright(11_000) > 20.0 * bright(35_000));
    // Every shape plays.
    for shape in 0..5 {
        let signal = play(
            &[
                (LFO_SHAPE, f64::from(shape)),
                (LFO_RATE, 6.0),
                (LFO_PITCH, 30.0),
                (LFO_PW, 30.0),
                (LFO_FILTER, 30.0),
                (OSC1_SHAPE, 2.0),
            ],
            24_000,
            &[on(0, 57)],
        );
        assert!(
            signal.iter().all(|x| x.is_finite()) && rms(&signal) > 0.05,
            "shape {shape}"
        );
    }
}

// Spec: bass and Poli R14
#[test]
fn the_chorus_is_chorus_ensembles_in_stereo_after_the_amplifier() {
    let notes = [on(0, 57), on(0, 64)];
    // Off, both channels carry the dry voices.
    let [left, right] = play_both(&[(CHORUS, 0.0)], 512, 24_000, &notes);
    assert_eq!(left, right);
    assert!(rms(&left) > 0.1);
    // On, the channels differ, and at Dry/Wet 0 they are the dry signal.
    let [wet_left, wet_right] = play_both(&[(CHORUS, 1.0)], 512, 24_000, &notes);
    assert_ne!(wet_left, wet_right);
    let [dry_left, dry_right] =
        play_both(&[(CHORUS, 1.0), (CHORUS_DRY_WET, 0.0)], 512, 24_000, &notes);
    assert_eq!(dry_left, left);
    assert_eq!(dry_right, left);
    // It is Chorus-Ensemble's chorus mode with two taps, sample for sample.
    let settings = [
        (CHORUS, 1.0),
        (CHORUS_RATE, 1.7),
        (CHORUS_AMOUNT, 60.0),
        (CHORUS_DRY_WET, 100.0),
    ];
    let [poli_left, poli_right] = play_both(&settings, 512, 24_000, &notes);
    let ensemble = {
        let mut registry = bl_engine::Registry::empty();
        chorus_ensemble::register(&mut registry);
        let mut node = node(
            &registry,
            &device(
                &chorus_ensemble_type(),
                &[
                    (chorus_ensemble::MODE, 0.0),
                    (chorus_ensemble::CHORUS_TAPS, 1.0),
                    (chorus_ensemble::RATE, 1.7),
                    (chorus_ensemble::AMOUNT, 60.0),
                    (chorus_ensemble::DRY_WET, 100.0),
                ],
            ),
            left.len(),
        );
        run(node.as_mut(), &left, &[])
    };
    assert_near(&poli_left, &ensemble[0], 1e-6, "left");
    assert_near(&poli_right, &ensemble[1], 1e-6, "right");
}

/// A phrase that moves no pitch: chords, steals, releases, the envelopes, an
/// LFO on the width and the cutoff, the mod wheel, the chorus.
fn still_phrase() -> ([(ParamId, f64); 19], Vec<NoteEvent>) {
    (
        [
            (CHORUS, 1.0),
            (OSC2_LEVEL, db_to_gain(-6.0)),
            (OSC2_DETUNE, 12.0),
            (OSC1_SHAPE, 2.0),
            (OSC2_SHAPE, 1.0),
            (CIRCUIT, 1.0),
            (FREQ, 900.0),
            (RES, 50.0),
            (ENV_AMOUNT, 60.0),
            (FILTER_DECAY, 400.0),
            (LFO_RATE, 5.0),
            (LFO_SHAPE, 4.0),
            (LFO_PW, 40.0),
            (LFO_FILTER, 20.0),
            (LFO_DELAY, 200.0),
            (WHEEL, 50.0),
            (VOICES, 3.0),
            (VEL_VOL, 50.0),
            (PW, 35.0),
        ],
        vec![
            on(0, 48),
            on_at(0, 55, 0.6),
            on(1_500, 60),
            mod_wheel(2_000, 0.5),
            on(3_000, 64),
            off(8_000, 48),
            on(9_000, 52),
            off(16_000, 55),
        ],
    )
}

// Spec: bass and Poli R15
#[test]
fn a_phrase_renders_the_same_at_every_block_size_and_every_run() {
    let (extra, notes) = still_phrase();
    let reference = play_both(&extra, 4_096, 30_000, &notes);
    assert!(rms(&reference[0]) > 0.01);
    for block in BLOCKS {
        let rendered = play_both(&extra, block, 30_000, &notes);
        for channel in 0..2 {
            assert_near(
                &rendered[channel],
                &reference[channel],
                1e-5,
                &format!("block size {block}"),
            );
        }
    }
    assert!(play_both(&extra, 4_096, 30_000, &notes) == reference);
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
    extra.extend([
        (GLIDE, 120.0),
        (LFO_PITCH, 10.0),
        (LFO_SHAPE, 0.0),
        (LFO_RATE, 5.0),
    ]);
    notes.extend([bend(2_500, 55, 0.1), wheel(4_000, 0.5)]);
    let reference = play_both(&extra, 4_096, 30_000, &notes);
    assert!(rms(&reference[0]) > 0.01);
    for block in [64, 512] {
        assert!(
            play_both(&extra, block, 30_000, &notes) == reference,
            "block size {block} differs"
        );
    }
}

// Spec: bass and Poli R18
#[test]
fn a_per_note_bend_moves_only_its_note_by_48_semitones_a_unit() {
    let level = |signal: &[f32], hz: f64| amplitude_at(&signal[8_000..24_000], hz);
    let f60 = frequency(60);
    let f67 = frequency(67);
    let bent = play(&[], 24_000, &[on(0, 60), on(0, 67), bend(4_000, 60, 0.25)]);
    assert!(
        level(&bent, f60 * 2.0) > 0.2,
        "the bent note is an octave up"
    );
    assert!(level(&bent, f60) < 0.02);
    assert!(level(&bent, f67) > 0.2, "the other stays");
    let down = play(&[], 24_000, &[on(0, 60), bend(4_000, 60, -0.125)]);
    let f54 = f60 * 2f64.powf(-6.0 / 12.0);
    assert!(level(&down, f54) > 0.2);
}

// Spec: bass and Poli R1 (the factory presets)
#[test]
fn every_factory_preset_sets_parameters_in_range_and_plays() {
    let kind = poli_type();
    let all = presets();
    assert!(all.len() >= 3);
    for preset in all {
        for (id, value) in &preset.values {
            let param = kind.description.param(*id).expect("a parameter of Poli");
            assert!(
                (param.min()..=param.max()).contains(value),
                "{}: {} is {value}",
                preset.name,
                param.name
            );
        }
        let mut node = node(&registry(), &device(&kind, &preset.values), 512);
        let chord = [on(0, 48), on(0, 55), on(0, 60), off(12_000, 48)];
        let [left, right] = render(node.as_mut(), 512, 24_000, &chord);
        assert!(rms(&left) > 0.005, "{} is silent", preset.name);
        assert!(
            left.iter()
                .chain(&right)
                .all(|x| x.is_finite() && x.abs() < 4.0),
            "{}",
            preset.name
        );
    }
}

/// The windows of the golden render, each [`GOLDEN_WINDOW`] frames from its
/// start, and the RMS of Poli's still phrase over them in each channel,
/// rendered when the device was written. A window's level, not a sample,
/// is compared: the platforms' maths libraries differ in the last bit, and
/// the resonant filter carries that into single samples.
const GOLDEN_AT: [usize; 7] = [0, 500, 4_000, 7_000, 12_000, 20_000, 26_000];
const GOLDEN_WINDOW: usize = 512;
const GOLDEN_LEFT: [f64; 7] = [
    0.74824584, 0.81367847, 1.2468131, 0.96082344, 0.81128887, 0.7859888, 1.3153281,
];
const GOLDEN_RIGHT: [f64; 7] = [
    0.74824584, 1.0764424, 1.1629638, 0.96594234, 1.351002, 0.8291039, 1.0599068,
];

// Spec: bass and Poli R9 to R14 (the golden render)
#[test]
fn a_phrase_renders_as_it_did_when_the_device_was_written() {
    let (extra, notes) = still_phrase();
    let [left, right] = play_both(&extra, 512, 30_000, &notes);
    for (n, &at) in GOLDEN_AT.iter().enumerate() {
        for (channel, signal, want) in [
            ("left", &left, GOLDEN_LEFT[n]),
            ("right", &right, GOLDEN_RIGHT[n]),
        ] {
            let level = rms(&signal[at..at + GOLDEN_WINDOW]);
            assert!(
                (level / want - 1.0).abs() < 1e-3,
                "{channel} frames {at}+: RMS {level} isn't {want}"
            );
        }
    }
}
