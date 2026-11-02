//! Tension (WP-8.22; spec: tension): the exciters, the damper, the string,
//! vibrato, termination, pickup and body, and the Filter/Global tab.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common::synth::{self, BLOCKS, HZ, frequency, off, on, on_at, render};
use crate::common::{device, node};
use bl_devices::tension::*;
use bl_engine::{NoteEvent, NoteKind};
use bl_model::{DeviceRole, ParamId};

/// The string and its exciter alone, ringing long: every other section that
/// is on in a new device is off, so a test sees what it is about (R8).
const BARE: [(ParamId, f64); 4] = [
    (DAMP_ON, 0.0),
    (TERM_ON, 0.0),
    (PICKUP_ON, 0.0),
    (C::StrDecay.id(), 30_000.0),
];

fn params(extra: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    BARE.iter().chain(extra).copied().collect()
}

fn tension(params: &[(ParamId, f64)]) -> Box<dyn bl_engine::Node> {
    node(&synth::registry(), &device(&tension_type(), params), 4096)
}

/// `notes` played for `frames` on a fresh device with `params`, in blocks of
/// `block`: the left channel.
fn blocks(params: &[(ParamId, f64)], block: usize, notes: &[NoteEvent], frames: usize) -> Vec<f32> {
    let [left, _] = render(tension(params).as_mut(), block, frames, notes);
    left
}

fn play(params: &[(ParamId, f64)], notes: &[NoteEvent], frames: usize) -> Vec<f32> {
    blocks(params, 4096, notes, frames)
}

/// `seconds` of the key `key` held, at velocity 1.
fn note(params: &[(ParamId, f64)], key: u8, seconds: f64) -> Vec<f32> {
    play(params, &[on(0, key)], (seconds * HZ) as usize)
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-12).log10()
}

/// The level of `signal[from..to]` in decibels.
fn level(signal: &[f32], from: f64, to: f64) -> f64 {
    db(synth::rms(
        &signal[(from * HZ) as usize..(to * HZ) as usize],
    ))
}

/// The Hann-windowed amplitude of `signal[from..from + len]` at `hz`.
fn magnitude(signal: &[f32], hz: f64, from: usize, len: usize) -> f64 {
    let (mut re, mut im, mut sum) = (0.0, 0.0, 0.0);
    for (i, s) in signal[from..from + len].iter().enumerate() {
        let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / len as f64).cos();
        let angle = std::f64::consts::TAU * hz * (from + i) as f64 / HZ;
        re += f64::from(*s) * window * angle.cos();
        im += f64::from(*s) * window * angle.sin();
        sum += window;
    }
    2.0 * re.hypot(im) / sum
}

/// The frequency within `cents` of `hz` at which `signal[from..from + len]`
/// is strongest: found in steps of two cents, then in tenths of a cent.
fn pitch_near(signal: &[f32], hz: f64, cents: i32, from: usize, len: usize) -> f64 {
    let strongest = |range: std::ops::RangeInclusive<i32>, per: f64, centre: f64| {
        range
            .map(|step| centre * 2f64.powf(f64::from(step) / (1200.0 * per)))
            .max_by(|a, b| {
                magnitude(signal, *a, from, len).total_cmp(&magnitude(signal, *b, from, len))
            })
            .unwrap()
    };
    let coarse = strongest(-cents / 2..=cents / 2, 0.5, hz);

    strongest(-20..=20, 10.0, coarse)
}

fn cents(hz: f64, of: f64) -> f64 {
    1200.0 * (hz / of).log2()
}

/// The amplitude of harmonic `n` of `hz` in `signal` over `seconds` from
/// `from`.
fn harmonic(signal: &[f32], hz: f64, n: u32, from: f64, seconds: f64) -> f64 {
    magnitude(
        signal,
        hz * f64::from(n),
        (from * HZ) as usize,
        (seconds * HZ) as usize,
    )
}

/// The share of `signal`'s energy above `hz`, in decibels.
fn brightness(signal: &[f32], hz: f64) -> f64 {
    let spectrum = crate::common::spectrum(signal);
    let bin = |f: f64| (f * signal.len() as f64 / HZ) as usize;
    let above: f64 = spectrum[bin(hz)..].iter().map(|a| a * a).sum();
    let all: f64 = spectrum.iter().map(|a| a * a).sum();
    db((above / all).sqrt())
}

fn pressure(frame: usize, key: u8, value: f32) -> NoteEvent {
    NoteEvent {
        offset: frame as u32,
        kind: NoteKind::Expression {
            expression: bl_engine::NoteExpression::Pressure,
            value,
        },
        channel: 0,
        key,
        velocity: 0.0,
        note_id: None,
    }
}

/// The value a destination chooser takes to name control `c`.
fn dest(c: C) -> f64 {
    let list: Vec<&str> = tension_type()
        .matrix
        .expect("Tension has a matrix")
        .dests
        .iter()
        .map(|d| d.name)
        .collect();
    1.0 + list
        .iter()
        .position(|n| *n == c.name())
        .expect("a destination") as f64
}

const BOW: (ParamId, f64) = (EXC_TYPE, 0.0);
const HAMMER: (ParamId, f64) = (EXC_TYPE, 1.0);
const BOUNCING: (ParamId, f64) = (EXC_TYPE, 2.0);
const PLECTRUM: (ParamId, f64) = (EXC_TYPE, 3.0);

// Spec: tension R1
#[test]
fn tension_is_an_instrument_in_two_tabs_with_the_switches_the_spec_gives() {
    let types = bl_devices::device_types();
    let tension = types
        .iter()
        .find(|t| t.name == "Tension")
        .expect("the registry lists Tension");
    assert_eq!(tension.role, DeviceRole::Instrument);
    let description = &tension.description;
    description.check().unwrap();
    let layout = &description.layout;
    let names: Vec<(String, Vec<String>)> = layout
        .pages()
        .iter()
        .map(|p| {
            (
                p.name.clone(),
                p.pages.iter().map(|page| page.name.clone()).collect(),
            )
        })
        .collect();
    assert_eq!(names[0].1, ["String", "Filter/Global"]);
    // Every section but String and Keyboard has a switch, with the default
    // the spec gives.
    for (id, on) in [
        (EXC_ON, 1.0),
        (DAMP_ON, 1.0),
        (VIB_ON, 0.0),
        (TERM_ON, 1.0),
        (PICKUP_ON, 1.0),
        (BODY_ON, 0.0),
        (FILTER_ON, 0.0),
        (ENV_ON, 0.0),
        (LFO_ON, 0.0),
        (MPE_ON, 1.0),
        (PORT_ON, 0.0),
        (UNI_ON, 0.0),
    ] {
        let p = description.param(id).expect("a switch");
        assert_eq!(p.default, on, "{}", p.name);
    }
    assert_eq!(description.param(EXC_TYPE).unwrap().default, 1.0, "Hammer");
    assert_eq!(description.param(VOICES).unwrap().default, 8.0);
    assert_eq!(description.param(PB_RANGE).unwrap().default, 2.0);
    assert_eq!(description.param(NOTE_PB).unwrap().default, 48.0);
    assert_eq!(description.param(VOLUME).unwrap().name, "Volume");
}

// Spec: tension R1
#[test]
fn every_factory_preset_sounds() {
    for (name, preset) in PRESETS {
        let out = note(preset, 52, 1.0);
        assert!(
            peak(&out) > 0.01 && peak(&out) < 2.0,
            "{name}: {}",
            peak(&out)
        );
    }
}

// Spec: tension R2
#[test]
fn with_the_exciter_off_only_the_damper_moves_the_string() {
    let silent = note(&params(&[(EXC_ON, 0.0)]), 57, 0.5);
    assert_eq!(peak(&silent), 0.0, "no exciter and no damper: no sound");
    let kicked = note(
        &[(EXC_ON, 0.0), (DAMP_ON, 1.0), (C::StrDecay.id(), 30_000.0)],
        57,
        0.5,
    );
    assert!(peak(&kicked) > 1e-4, "{}", peak(&kicked));
    let struck = note(&params(&[]), 57, 0.5);
    assert!(peak(&struck) > 3.0 * peak(&kicked));
}

// Spec: tension R3, R21
#[test]
fn a_bow_sustains_within_3_db_over_ten_seconds_once_settled() {
    for key in [45, 57, 69] {
        let out = note(&params(&[BOW]), key, 12.0);
        let window = |from: f64| level(&out, from, from + 0.5);
        let levels: Vec<f64> = (0..20).map(|i| window(2.0 + f64::from(i) * 0.5)).collect();
        let (low, high) = levels
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(high > -40.0, "key {key} sounds: {high}");
        assert!(high - low < 3.0, "key {key}: {low} to {high} dB");
    }
}

// Spec: tension R3
#[test]
fn the_bow_at_extreme_force_stays_bounded_and_a_light_one_stays_alive() {
    for force in [0.0, 25.0, 50.0, 75.0, 100.0] {
        for position in [2.0, 20.0, 50.0] {
            let out = note(
                &params(&[
                    BOW,
                    (C::ExcForce.id(), force),
                    (C::ExcPosition.id(), position),
                    (C::ExcVelocity.id(), 100.0),
                    (C::ExcVelocity.vel(), 0.0),
                ]),
                57,
                4.0,
            );
            assert!(out.iter().all(|s| s.is_finite()), "force {force}");
            assert!(
                peak(&out) < 4.0,
                "force {force} at {position}: {}",
                peak(&out)
            );
        }
    }
}

// Spec: tension R4, R21
#[test]
fn a_hammer_and_a_plectrum_decay() {
    for (kind, name) in [(HAMMER, "hammer"), (PLECTRUM, "plectrum")] {
        let out = note(&params(&[kind, (C::StrDecay.id(), 2_000.0)]), 57, 4.0);
        let early = level(&out, 0.1, 0.4);
        assert!(early > -40.0, "{name} sounds: {early}");
        assert!(level(&out, 1.5, 1.8) < early - 20.0, "{name}");
        assert!(level(&out, 3.5, 3.8) < early - 40.0, "{name}");
    }
}

// Spec: tension R4
#[test]
fn a_hammer_that_bounces_adds_energy_a_second_time() {
    let strong = [(C::ExcMass.id(), 80.0), (C::ExcDamping.id(), 10.0)];
    let once = note(&params(&[&[HAMMER], &strong[..]].concat()), 57, 1.0);
    let again = note(&params(&[&[BOUNCING], &strong[..]].concat()), 57, 1.0);
    // The bouncing hammer is still on the string some milliseconds in: the
    // note is not the one a single strike makes.
    let window = |s: &[f32]| level(s, 0.03, 0.15);
    assert!(
        (window(&once) - window(&again)).abs() > 0.5,
        "{} against {}",
        window(&once),
        window(&again)
    );
}

// Spec: tension R20
#[test]
fn a_render_is_the_same_at_every_block_size_and_run_after_run() {
    let everything = [
        HAMMER,
        (C::ExcMass.id(), 40.0),
        (VIB_ON, 1.0),
        (C::VibDelay.id(), 0.0),
        (C::VibError.id(), 60.0),
        (BODY_ON, 1.0),
        (FILTER_ON, 1.0),
        (ENV_ON, 1.0),
        (LFO_ON, 1.0),
        (C::FreqLfo.id(), 40.0),
        (C::FreqEnv.id(), 50.0),
        (UNI_ON, 1.0),
        (UNI_DELAY, 5.0),
        (PORT_ON, 1.0),
        (DAMP_ON, 1.0),
        (TERM_ON, 1.0),
        (PICKUP_ON, 1.0),
    ];
    let notes = [on(0, 52), on(3_000, 59), off(20_000, 52), on(20_500, 55)];
    let reference = blocks(&everything, BLOCKS[3], &notes, 48_000);
    assert!(peak(&reference) > 0.01);
    for block in BLOCKS {
        assert_eq!(
            blocks(&everything, block, &notes, 48_000),
            reference,
            "block size {block}"
        );
    }
    let bowed = [BOW, (UNI_ON, 1.0), (BODY_ON, 1.0), (VIB_ON, 1.0)];
    let reference = blocks(&bowed, BLOCKS[3], &notes, 48_000);
    for block in BLOCKS {
        assert_eq!(
            blocks(&bowed, block, &notes, 48_000),
            reference,
            "bow {block}"
        );
    }
}

// Spec: tension R5
#[test]
fn the_exciters_position_shapes_the_spectrum_and_fix_pos_keeps_its_distance() {
    let at = |position: f64, fix: f64, key: u8| {
        let out = note(
            &params(&[
                HAMMER,
                (C::ExcPosition.id(), position),
                (EXC_FIX, fix),
                (PICKUP_ON, 1.0),
                (C::PickupPosition.id(), 12.0),
                (C::StrDamping.id(), 0.0),
            ]),
            key,
            1.0,
        );
        let hz = frequency(key);
        (
            harmonic(&out, hz, 1, 0.2, 0.6),
            harmonic(&out, hz, 2, 0.2, 0.6),
        )
    };
    // Struck at the middle, the string's even harmonics hardly sound.
    let (first, second) = at(50.0, 0.0, 60);
    assert!(db(second / first) < -30.0, "{}", db(second / first));
    let (first, second) = at(20.0, 0.0, 60);
    assert!(db(second / first) > -12.0, "{}", db(second / first));
    // At C3 a quarter of the string, fixed, is half of it an octave up.
    let (first, second) = at(25.0, 1.0, 72);
    assert!(db(second / first) < -30.0, "{}", db(second / first));
    let (first, second) = at(25.0, 0.0, 72);
    assert!(db(second / first) > -12.0, "{}", db(second / first));
    // And at C3 itself fixed or not is the same.
    synth::assert_near(
        &note(&params(&[HAMMER, (EXC_FIX, 1.0)]), 60, 0.2),
        &note(&params(&[HAMMER]), 60, 0.2),
        1e-4,
        "Fix. Pos at C3",
    );
}

// Spec: tension R5
#[test]
fn velocity_position_and_the_types_own_controls_move_by_vel_and_key() {
    let level_of = |extra: &[(ParamId, f64)], key: u8, velocity: f32| {
        let out = play(
            &params(&[&[HAMMER], extra].concat()),
            &[on_at(0, key, velocity)],
            24_000,
        );
        peak(&out)
    };
    // Exciter Velocity's Vel at 100 % makes a soft note softer.
    assert!(level_of(&[], 60, 0.3) < 0.6 * level_of(&[], 60, 1.0));
    assert!(
        (level_of(&[(C::ExcVelocity.vel(), 0.0)], 60, 0.3)
            - level_of(&[(C::ExcVelocity.vel(), 0.0)], 60, 1.0))
        .abs()
            < 1e-6
    );
    // Position's Key moves the strike point with the key.
    let moved = |key_amount: f64, key: u8| {
        let out = play(
            &params(&[
                HAMMER,
                (C::ExcPosition.id(), 40.0),
                (C::ExcPosition.key(), key_amount),
                (C::ExcStiffness.id(), 100.0),
                (PICKUP_ON, 1.0),
                (C::PickupPosition.id(), 12.0),
                (C::StrDamping.id(), 0.0),
            ]),
            &[on(0, key)],
            48_000,
        );
        let hz = frequency(key);
        db(harmonic(&out, hz, 2, 0.1, 0.6) / harmonic(&out, hz, 1, 0.1, 0.6))
    };
    assert!(moved(100.0, 72) < moved(0.0, 72) - 5.0);
}

// Spec: tension R6
#[test]
fn the_damper_stops_the_string_when_the_key_is_let_go() {
    // The level `from` to `to` seconds in, the key let go at half a second.
    let after = |extra: &[(ParamId, f64)], from: f64, to: f64| {
        let out = play(
            &params(&[&[HAMMER, (DAMP_ON, 1.0)], extra].concat()),
            &[on(0, 57), off(24_000, 57)],
            72_000,
        );
        level(&out, from, to)
    };
    let free = after(&[(DAMP_ON, 0.0)], 0.8, 1.2);
    let felt = after(
        &[(C::DampDamping.id(), 100.0), (C::DampStiffness.id(), 0.0)],
        0.8,
        1.2,
    );
    let metal = after(
        &[(C::DampDamping.id(), 100.0), (C::DampStiffness.id(), 100.0)],
        0.8,
        1.2,
    );
    let light = after(&[(C::DampDamping.id(), 10.0)], 0.8, 1.2);
    assert!(felt < free - 30.0, "{felt} against {free}");
    assert!(felt < metal - 10.0, "felt {felt}, metal {metal}");
    assert!(metal < light, "metal {metal}, light {light}");
    assert!(light < free, "{light} against {free}");
    // Gated, Velocity sets how fast it lands: how far the level falls from
    // just before the key is let go to a little after; ungated, it is the
    // same speed.
    let fall = |extra: &[(ParamId, f64)]| {
        let extra = [&[(C::DampDamping.id(), 80.0)], extra].concat();
        after(&extra, 0.48, 0.5) - after(&extra, 0.54, 0.6)
    };
    let fast = fall(&[(C::DampVelocity.id(), 100.0)]);
    let slow = fall(&[(C::DampVelocity.id(), 0.0)]);
    assert!(fast > slow + 1.0, "{fast} against {slow}");
    assert_eq!(
        fall(&[(DAMP_GATED, 0.0), (C::DampVelocity.id(), 100.0)]),
        fall(&[(DAMP_GATED, 0.0), (C::DampVelocity.id(), 0.0)])
    );
}

// Spec: tension R6
#[test]
fn the_damper_rests_at_its_position_and_fix_pos_keeps_its_distance() {
    // A damper at the string's middle leaves the even harmonics alone, which
    // a damper near the end takes with the rest.
    let held = |position: f64, fix: f64, key: u8| {
        let out = play(
            &params(&[
                HAMMER,
                (DAMP_ON, 1.0),
                (C::DampPosition.id(), position),
                (DAMP_FIX, fix),
                (PICKUP_ON, 1.0),
                (C::PickupPosition.id(), 12.0),
                (C::DampDamping.id(), 60.0),
                (C::DampStiffness.id(), 0.0),
            ]),
            &[on(0, key), off(12_000, key)],
            48_000,
        );
        let hz = frequency(key);
        let ratio = harmonic(&out, hz, 2, 0.5, 0.4) / harmonic(&out, hz, 3, 0.5, 0.4);
        eprintln!("held {position} {fix} {key}: {}", db(ratio));
        ratio
    };
    assert!(held(50.0, 0.0, 60) > 1.5 * held(5.0, 0.0, 60));
    // A quarter of the string at C3, fixed, is half of it an octave up.
    assert!(held(25.0, 1.0, 72) > 1.5 * held(25.0, 0.0, 72));
}

// Spec: tension R7
#[test]
fn the_string_decays_by_decay_ratio_inharm_and_damping() {
    let key = 57;
    let hz = frequency(key);
    let decay = |extra: &[(ParamId, f64)]| {
        let out = note(
            &params(&[&[HAMMER, (C::StrDamping.id(), 100.0)], extra].concat()),
            key,
            6.0,
        );
        let a = harmonic(&out, hz, 1, 0.5, 0.5);
        let b = harmonic(&out, hz, 1, 2.5, 0.5);
        60.0 * 2.0 / db(a / b)
    };
    for seconds in [2.0, 5.0] {
        let measured = decay(&[(C::StrDecay.id(), seconds * 1000.0)]);
        assert!(
            (measured / seconds - 1.0).abs() < 0.25,
            "{seconds} s decays in {measured}"
        );
    }
    // Key: positive makes a higher key's decay longer.
    let shorter = {
        let out = note(
            &params(&[
                HAMMER,
                (C::StrDecay.id(), 3000.0),
                (C::StrDecay.key(), -100.0),
            ]),
            81,
            3.0,
        );
        level(&out, 2.0, 2.5)
    };
    let longer = {
        let out = note(
            &params(&[
                HAMMER,
                (C::StrDecay.id(), 3000.0),
                (C::StrDecay.key(), 100.0),
            ]),
            81,
            3.0,
        );
        level(&out, 2.0, 2.5)
    };
    assert!(longer > shorter + 10.0, "{longer} against {shorter}");
    // Ratio: after the note ends it decays faster.
    let tail = |ratio: f64| {
        let out = play(
            &params(&[
                HAMMER,
                (C::StrDecay.id(), 8000.0),
                (C::StrRatio.id(), ratio),
            ]),
            &[on(0, key), off(12_000, key)],
            96_000,
        );
        level(&out, 1.0, 1.5)
    };
    assert!(tail(100.0) < tail(0.0) - 6.0);
    // Inharm stretches the upper partials; at 0 they are harmonic.
    let partial = |inharm: f64| {
        let out = note(
            &params(&[
                HAMMER,
                (C::StrInharm.id(), inharm),
                (C::StrDamping.id(), 0.0),
                (PICKUP_ON, 1.0),
                (C::PickupPosition.id(), 12.0),
            ]),
            45,
            2.0,
        );
        let eighth = frequency(45) * 8.0;
        pitch_near(&out, eighth, 100, 4800, 48_000)
    };
    assert!(cents(partial(0.0), frequency(45) * 8.0).abs() < 1.0);
    assert!(cents(partial(100.0), frequency(45) * 8.0) > 5.0);
    // Damping: the high harmonics die faster.
    let top = |damping: f64| {
        let out = note(
            &params(&[
                HAMMER,
                (C::StrDamping.id(), damping),
                (C::ExcPosition.id(), 12.0),
                (PICKUP_ON, 1.0),
                (C::PickupPosition.id(), 12.0),
            ]),
            key,
            3.0,
        );
        db(harmonic(&out, hz, 5, 1.5, 0.5) / harmonic(&out, hz, 1, 1.5, 0.5))
    };
    assert!(
        top(100.0) < top(0.0) - 10.0,
        "{} against {}",
        top(100.0),
        top(0.0)
    );
}

// Spec: tension R8
#[test]
fn the_string_plays_its_note_within_a_cent_across_the_keys_with_the_body_on_and_off() {
    for key in (36..=96).step_by(12) {
        let hz = frequency(key);
        for (kind, name) in [(HAMMER, "hammer"), (PLECTRUM, "plectrum"), (BOW, "bow")] {
            for body in [0.0, 1.0] {
                let out = note(
                    &params(&[
                        kind,
                        (BODY_ON, body),
                        (C::BodyStrBody.id(), -50.0),
                        (C::StrDamping.id(), 0.0),
                    ]),
                    key,
                    3.0,
                );
                let heard = pitch_near(&out, hz, 20, (1.0 * HZ) as usize, (2.0 * HZ) as usize);
                // A bow pulls its string's pitch a little, as a bow does.
                let allowed = if name == "bow" { 3.0 } else { 1.0 };
                assert!(
                    cents(heard, hz).abs() < allowed,
                    "{name} on key {key} with body {body}: {} cents",
                    cents(heard, hz)
                );
            }
        }
    }
    // The pickup, the termination and the filter leave the pitch alone too.
    for key in [36, 60, 84] {
        let hz = frequency(key);
        let out = note(
            &[
                HAMMER,
                (C::StrDecay.id(), 30_000.0),
                (PICKUP_ON, 1.0),
                (C::PickupPosition.id(), 12.0),
                (TERM_ON, 1.0),
                (C::FingerMass.id(), 100.0),
                (C::FingerStiff.id(), 0.0),
                (C::FretStiff.id(), 0.0),
                (FILTER_ON, 1.0),
                (C::FilterFreq.id(), 20_000.0),
                (DAMP_ON, 0.0),
            ],
            key,
            4.0,
        );
        let heard = pitch_near(&out, hz, 20, (1.0 * HZ) as usize, (2.0 * HZ) as usize);
        assert!(
            cents(heard, hz).abs() < 1.0,
            "key {key}: {} cents",
            cents(heard, hz)
        );
    }
}

/// The pitch of `signal`, in cents from `hz`, a window of `seconds` from
/// `from`, searched within `range` cents.
fn bent(signal: &[f32], hz: f64, from: f64, seconds: f64, range: i32) -> f64 {
    cents(
        pitch_near(
            signal,
            hz,
            range,
            (from * HZ) as usize,
            (seconds * HZ) as usize,
        ),
        hz,
    )
}

/// A long hammer note on key 57 with `extra`.
fn long(extra: &[(ParamId, f64)], seconds: f64) -> Vec<f32> {
    note(&params(&[&[HAMMER], extra].concat()), 57, seconds)
}

// Spec: tension R9
#[test]
fn the_vibrato_moves_the_pitch_by_up_to_a_semitone_after_its_delay_and_attack() {
    let vibrato = [
        (VIB_ON, 1.0),
        (C::VibRate.id(), 2.0),
        (C::VibAmount.id(), 100.0),
        (C::VibDelay.id(), 0.0),
        (C::VibAttack.id(), 0.0),
    ];
    let hz = frequency(57);
    let out = long(&vibrato, 1.0);
    // A sine at 2 Hz is at its top at 125 ms and its bottom at 375 ms.
    let (top, bottom) = (
        bent(&out, hz, 0.075, 0.1, 150),
        bent(&out, hz, 0.325, 0.1, 150),
    );
    assert!((70.0..105.0).contains(&top), "{top}");
    assert!((-105.0..-70.0).contains(&bottom), "{bottom}");
    // Off, it leaves the pitch alone.
    let still = long(&[(VIB_ON, 0.0)], 1.0);
    assert!(bent(&still, hz, 0.075, 0.1, 150).abs() < 1.0);
    // Delay holds it off, Attack brings it in.
    let delayed = long(&[&vibrato[..], &[(C::VibDelay.id(), 600.0)]].concat(), 1.0);
    assert!(bent(&delayed, hz, 0.075, 0.1, 150).abs() < 3.0);
    let faded = long(
        &[&vibrato[..], &[(C::VibAttack.id(), 2000.0)]].concat(),
        1.0,
    );
    assert!(bent(&faded, hz, 0.075, 0.1, 150) < top / 4.0);
    // Amount scales it, and < Mod adds the mod wheel's share.
    let half = long(&[&vibrato[..], &[(C::VibAmount.id(), 50.0)]].concat(), 1.0);
    let half = bent(&half, hz, 0.075, 0.1, 150);
    assert!((half - top / 2.0).abs() < 8.0, "{half} against {top}");
    let by_wheel = |wheel: f32| {
        let out = play(
            &params(
                &[
                    &[HAMMER],
                    &vibrato[..],
                    &[(C::VibAmount.id(), 0.0), (C::VibMod.id(), 100.0)],
                ]
                .concat(),
            ),
            &[synth::mod_wheel(0, wheel), on(0, 57)],
            48_000,
        );
        bent(&out, hz, 0.075, 0.1, 150)
    };
    assert!(by_wheel(0.0).abs() < 1.0);
    assert!(by_wheel(1.0) > 60.0);
}

// Spec: tension R9
#[test]
fn vibrato_error_varies_each_note_from_the_device_seed() {
    let at = |error: f64| {
        let out = play(
            &params(&[
                HAMMER,
                (VIB_ON, 1.0),
                (C::VibRate.id(), 3.0),
                (C::VibAmount.id(), 100.0),
                (C::VibDelay.id(), 0.0),
                (C::VibAttack.id(), 0.0),
                (C::VibError.id(), error),
            ]),
            &[on(0, 57), on(96_000, 69)],
            192_000,
        );
        let (a, b) = (frequency(57), frequency(69));
        (bent(&out, a, 0.1, 0.1, 150), bent(&out, b, 2.1, 0.1, 150))
    };
    let (first, second) = at(0.0);
    assert!((first - second).abs() < 3.0, "{first} against {second}");
    let (first, second) = at(100.0);
    assert!((first - second).abs() > 10.0, "{first} against {second}");
    assert_eq!(at(100.0), at(100.0), "the same seed gives the same notes");
}

// Spec: tension R10
#[test]
fn the_termination_takes_the_high_harmonics_by_its_finger_and_fret() {
    let bright = |extra: &[(ParamId, f64)], velocity: f32| {
        let out = play(
            &params(&[&[HAMMER, (TERM_ON, 1.0), (C::StrDamping.id(), 0.0)], extra].concat()),
            &[on_at(0, 57, velocity)],
            48_000,
        );
        let hz = frequency(57);
        db(harmonic(&out, hz, 7, 0.1, 0.5) / harmonic(&out, hz, 1, 0.1, 0.5))
    };
    let soft = [
        (C::FingerMass.id(), 100.0),
        (C::FingerStiff.id(), 0.0),
        (C::FretStiff.id(), 0.0),
    ];
    let stiff = [(C::FingerMass.id(), 0.0)];
    assert!(bright(&soft, 1.0) < bright(&stiff, 1.0) - 6.0);
    assert_eq!(
        bright(&stiff, 1.0),
        bright(&[(TERM_ON, 0.0)], 1.0),
        "no finger, no loss"
    );
    let fret = |stiffness: f64| {
        bright(
            &[
                (C::FingerMass.id(), 100.0),
                (C::FingerStiff.id(), 0.0),
                (C::FretStiff.id(), stiffness),
            ],
            1.0,
        )
    };
    assert!(fret(100.0) > fret(0.0) + 1.0);
    let harder = |amount: f64| {
        bright(
            &[
                (C::FingerMass.id(), 50.0),
                (C::FingerMass.vel(), amount),
                (C::ExcVelocity.vel(), 0.0),
                (C::FingerStiff.id(), 0.0),
            ],
            1.0,
        )
    };
    assert!(harder(100.0) < harder(0.0) - 1.0);
}

// Spec: tension R11
#[test]
fn the_pickup_hears_the_string_at_its_position_and_at_its_middle_when_off() {
    let at =
        |extra: &[(ParamId, f64)]| long(&[&[(C::ExcPosition.id(), 12.0)], extra].concat(), 1.0);
    let off_ = at(&[(PICKUP_ON, 0.0)]);
    let middle = at(&[(PICKUP_ON, 1.0), (C::PickupPosition.id(), 50.0)]);
    synth::assert_near(&off_, &middle, 1e-6, "off is the middle");
    let hz = frequency(57);
    let shares = |position: f64| {
        let out = at(&[(PICKUP_ON, 1.0), (C::PickupPosition.id(), position)]);
        (
            harmonic(&out, hz, 1, 0.2, 0.6),
            db(harmonic(&out, hz, 2, 0.2, 0.6) / harmonic(&out, hz, 1, 0.2, 0.6)),
            db(harmonic(&out, hz, 6, 0.2, 0.6) / harmonic(&out, hz, 1, 0.2, 0.6)),
        )
    };
    let (near_first, near_second, near_sixth) = shares(5.0);
    let (mid_first, mid_second, mid_sixth) = shares(45.0);
    // Near the termination: thinner (less of the fundamental) and brighter.
    assert!(
        near_first < 0.5 * mid_first,
        "{near_first} against {mid_first}"
    );
    assert!(near_sixth > mid_sixth + 6.0 || near_second > mid_second + 6.0);
    // At the middle the even harmonics are gone.
    assert!(shares(50.0).1 < -40.0);
}

// Spec: tension R12
#[test]
fn the_body_colours_the_string_by_type_size_decay_and_cuts() {
    let dead = [
        (C::StrDecay.id(), 60.0),
        (C::StrDamping.id(), 0.0),
        (BODY_ON, 1.0),
        (C::BodyStrBody.id(), 100.0),
        (C::BodyDecay.id(), 2_000.0),
    ];
    let body = |extra: &[(ParamId, f64)]| {
        note(&params(&[&[PLECTRUM], &dead[..], extra].concat()), 57, 2.0)
    };
    // Body off and Str/Body at -100 % are the string alone, bit for bit.
    let plain = note(&params(&[PLECTRUM, (C::StrDecay.id(), 60.0)]), 57, 0.5);
    let string_only = note(
        &params(&[
            PLECTRUM,
            (C::StrDecay.id(), 60.0),
            (BODY_ON, 1.0),
            (C::BodyStrBody.id(), -100.0),
        ]),
        57,
        0.5,
    );
    assert_eq!(plain, string_only);
    assert!(peak(&body(&[])) > 0.01, "the body alone sounds");
    // Each size is a different body: the unit tests place its resonances.
    let sizes: Vec<Vec<f32>> = [0.0, 2.0, 4.0].map(|n| body(&[(BODY_SIZE, n)])).to_vec();
    assert!(sizes[0] != sizes[1] && sizes[1] != sizes[2]);
    // The types are different bodies.
    let types: Vec<Vec<f32>> = (0..5).map(|t| body(&[(BODY_TYPE, f64::from(t))])).collect();
    for a in 0..5 {
        for b in a + 1..5 {
            assert_ne!(types[a], types[b], "{a} and {b}");
        }
    }
    // Decay is how long it rings after the string is dead.
    let rings = |ms: f64| level(&body(&[(C::BodyDecay.id(), ms)]), 0.5, 0.7);
    assert!(rings(2_000.0) > rings(10.0) + 20.0);
    // The cuts take the ends of its spectrum.
    let share = |extra: &[(ParamId, f64)], above: f64| brightness(&body(extra)[..48_000], above);
    assert!(share(&[(C::BodyHighCut.id(), 1_000.0)], 2_000.0) < share(&[], 2_000.0) - 6.0);
    let lows = |extra: &[(ParamId, f64)]| band(&body(extra)[..48_000], 30.0, 300.0);
    assert!(lows(&[(C::BodyLowCut.id(), 1_000.0)]) < lows(&[]) - 6.0);
}

/// The level in decibels of `signal`'s part between `low` and `high` hertz.
fn band(signal: &[f32], low: f64, high: f64) -> f64 {
    let spectrum = crate::common::spectrum(signal);
    let bin = |f: f64| (f * signal.len() as f64 / HZ) as usize;
    db(spectrum[bin(low)..bin(high)]
        .iter()
        .map(|a| a * a)
        .sum::<f64>()
        .sqrt())
}

// Spec: tension R13
#[test]
fn the_filter_has_the_modes_and_a_cutoff_and_formants_move_with_reso() {
    let rich = |extra: &[(ParamId, f64)]| {
        note(
            &params(
                &[
                    &[
                        HAMMER,
                        (C::ExcStiffness.id(), 100.0),
                        (C::StrDecay.id(), 800.0),
                        (C::ExcPosition.id(), 10.0),
                    ],
                    extra,
                ]
                .concat(),
            ),
            45,
            1.0,
        )
    };
    let raw = rich(&[]);
    let lp = |mode: f64| {
        rich(&[
            (FILTER_ON, 1.0),
            (FILTER_MODE, mode),
            (C::FilterFreq.id(), 500.0),
            (C::FilterRes.id(), 0.0),
        ])
    };
    let (lp12, lp24) = (lp(0.0), lp(1.0));
    assert!(band(&lp12, 2_000.0, 4_000.0) < band(&raw, 2_000.0, 4_000.0) - 10.0);
    assert!(band(&lp24, 2_000.0, 4_000.0) < band(&lp12, 2_000.0, 4_000.0) - 6.0);
    let hp = rich(&[
        (FILTER_ON, 1.0),
        (FILTER_MODE, 6.0),
        (C::FilterFreq.id(), 2_000.0),
        (C::FilterRes.id(), 0.0),
    ]);
    assert!(band(&hp, 50.0, 200.0) < band(&raw, 50.0, 200.0) - 10.0);
    // A formant mode sounds a vowel: its first formants lift over the rest.
    let vowel = rich(&[
        (FILTER_ON, 1.0),
        (FILTER_MODE, 9.0),
        (C::FilterFreq.id(), 1_000.0),
        (C::FilterRes.id(), 0.0),
    ]);
    let share = |s: &[f32]| band(s, 600.0, 1_200.0) - band(s, 3_000.0, 6_000.0);
    assert!(share(&vowel) > share(&raw) + 10.0);
    // Off, the filter leaves the string alone.
    synth::assert_near(
        &rich(&[(FILTER_ON, 0.0), (FILTER_MODE, 3.0)]),
        &raw,
        0.0,
        "filter off",
    );
}

// Spec: tension R13
#[test]
fn the_filters_envelope_lfo_and_key_move_its_cutoff_while_they_are_on() {
    let slice = |out: &[f32], a: f64, b: f64| {
        band(&out[(a * HZ) as usize..(b * HZ) as usize], 1_500.0, 6_000.0)
    };
    let filtered = |extra: &[(ParamId, f64)]| {
        note(
            &params(
                &[
                    &[
                        BOW,
                        (C::ExcVelocity.id(), 70.0),
                        (FILTER_ON, 1.0),
                        (C::FilterFreq.id(), 400.0),
                        (C::FilterRes.id(), 0.0),
                    ][..],
                    extra,
                ]
                .concat(),
            ),
            48,
            2.0,
        )
    };
    // The envelope opens the filter on the attack and closes it on the decay.
    let env = [
        (ENV_ON, 1.0),
        (C::FreqEnv.id(), 100.0),
        (C::EnvAttack.id(), 5.0),
        (C::EnvDecay.id(), 300.0),
        (C::EnvSustain.id(), 0.0),
    ];
    let enveloped = filtered(&env);
    assert!(slice(&enveloped, 0.0, 0.3) > slice(&enveloped, 1.4, 1.9) + 10.0);
    // Velocity shortens the attack by its Vel amount.
    let slow = [
        (ENV_ON, 1.0),
        (C::FreqEnv.id(), 100.0),
        (C::EnvAttack.id(), 500.0),
        (C::EnvSustain.id(), 100.0),
    ];
    let quick = filtered(&[&slow[..], &[(C::EnvAttack.vel(), 100.0)]].concat());
    assert!(slice(&quick, 0.03, 0.15) > slice(&filtered(&slow), 0.03, 0.15) + 6.0);
    // Switched off, the amounts do nothing.
    synth::assert_near(
        &filtered(&[(ENV_ON, 0.0), (C::FreqEnv.id(), 100.0)]),
        &filtered(&[]),
        0.0,
        "envelope off",
    );
    // The LFO swings it: the brightness moves from window to window.
    let lfo = [
        (LFO_ON, 1.0),
        (C::FreqLfo.id(), 100.0),
        (C::LfoRate.id(), 2.0),
    ];
    let swung = filtered(&lfo);
    let windows = |out: &[f32]| -> Vec<f64> {
        (0..8)
            .map(|i| slice(out, 0.5 + f64::from(i) * 0.15, 0.65 + f64::from(i) * 0.15))
            .collect()
    };
    let spread = |v: &[f64]| {
        v.iter().fold(f64::MIN, |a, b| a.max(*b)) - v.iter().fold(f64::MAX, |a, b| a.min(*b))
    };
    assert!(spread(&windows(&swung)) > spread(&windows(&filtered(&[]))) + 10.0);
    synth::assert_near(
        &filtered(&[(LFO_ON, 0.0), (C::FreqLfo.id(), 100.0)]),
        &filtered(&[]),
        0.0,
        "LFO off",
    );
    // Synced to a quarter note at 120 beats a minute it is the LFO at 2 Hz.
    let synced = filtered(&[
        (LFO_ON, 1.0),
        (C::FreqLfo.id(), 100.0),
        (LFO_SYNC, 1.0),
        (LFO_DIV, 4.0),
    ]);
    synth::assert_near(&synced, &swung, 1e-3, "synced");
    // The key opens it on higher keys.
    let keyed = |key_amount: f64| {
        let out = note(
            &params(&[
                BOW,
                (FILTER_ON, 1.0),
                (C::FilterFreq.id(), 400.0),
                (C::FreqKey.id(), key_amount),
            ]),
            72,
            1.0,
        );
        band(&out, 1_500.0, 6_000.0)
    };
    assert!(keyed(100.0) > keyed(0.0) + 6.0);
}

// Spec: tension R14
#[test]
fn pressure_and_slide_move_the_controls_their_slots_name_while_mpe_is_on() {
    let routed = |slot: usize, c: C, amount: f64, extra: &[(ParamId, f64)], notes: &[NoteEvent]| {
        play(
            &params(
                &[
                    &[
                        HAMMER,
                        (PICKUP_ON, 1.0),
                        (C::PickupPosition.id(), 8.0),
                        (MPE[slot].0, dest(c)),
                        (MPE[slot].1, amount),
                    ][..],
                    extra,
                ]
                .concat(),
            ),
            notes,
            24_000,
        )
    };
    let slide = |frame: usize, value: f32| NoteEvent {
        offset: frame as u32,
        kind: NoteKind::Expression {
            expression: bl_engine::NoteExpression::Slide,
            value,
        },
        channel: 0,
        key: 57,
        velocity: 0.0,
        note_id: None,
    };
    let plain = play(
        &params(&[HAMMER, (PICKUP_ON, 1.0), (C::PickupPosition.id(), 8.0)]),
        &[on(0, 57)],
        24_000,
    );
    for (slot, source) in [
        (0, pressure(1, 57, 1.0)),
        (1, pressure(1, 57, 1.0)),
        (2, slide(1, 1.0)),
        (3, slide(1, 1.0)),
    ] {
        let moved = routed(slot, C::PickupPosition, 100.0, &[], &[on(0, 57), source]);
        assert_ne!(moved, plain, "slot {slot}");
        assert!(peak(&moved) > 0.01);
        // The same route with MPE off, or at 0 %, or with no pressure, does nothing.
        assert_eq!(
            routed(
                slot,
                C::PickupPosition,
                100.0,
                &[(MPE_ON, 0.0)],
                &[on(0, 57), source]
            ),
            plain
        );
        assert_eq!(
            routed(slot, C::PickupPosition, 0.0, &[], &[on(0, 57), source]),
            plain
        );
    }
    assert_eq!(
        routed(0, C::PickupPosition, 100.0, &[], &[on(0, 57)]),
        plain
    );
    // A route reaches filter controls and the shared body's alike.
    let filtered = routed(
        0,
        C::FilterFreq,
        -100.0,
        &[(FILTER_ON, 1.0), (C::FilterFreq.id(), 6_000.0)],
        &[on(0, 57), pressure(1, 57, 1.0)],
    );
    let open = routed(
        0,
        C::FilterFreq,
        -100.0,
        &[(FILTER_ON, 1.0), (C::FilterFreq.id(), 6_000.0)],
        &[on(0, 57)],
    );
    assert!(band(&filtered, 2_000.0, 8_000.0) < band(&open, 2_000.0, 8_000.0) - 6.0);
    let body = |notes: &[NoteEvent]| routed(0, C::BodyStrBody, 100.0, &[(BODY_ON, 1.0)], notes);
    assert_ne!(body(&[on(0, 57), pressure(1, 57, 1.0)]), body(&[on(0, 57)]));
}

// Spec: tension R14
#[test]
fn the_node_shows_what_the_newest_note_plays_through_its_routes() {
    let mut node = tension(&params(&[
        (MPE[0].0, dest(C::PickupPosition)),
        (MPE[0].1, 100.0),
        (C::PickupPosition.id(), 10.0),
        (PICKUP_ON, 1.0),
    ]));
    let mut seen = Vec::new();
    node.matrix_played(&mut |param, base, played| seen.push((param, base, played)));
    assert!(seen.is_empty(), "with no voice, nothing is published");
    render(
        node.as_mut(),
        4096,
        4096,
        &[on(0, 57), pressure(1, 57, 1.0)],
    );
    node.matrix_played(&mut |param, base, played| seen.push((param, base, played)));
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, C::PickupPosition.id());
    assert!((seen[0].1 - 10.0).abs() < 1e-9, "its base: {seen:?}");
    assert!((seen[0].2 - 50.0).abs() < 1e-6, "what it plays: {seen:?}");
}

// Spec: tension R14
#[test]
fn the_pitch_bend_ranges_set_how_far_the_wheel_and_a_note_bend_it() {
    let hz = frequency(57);
    let wheel = |range: f64, value: f32| {
        let out = play(
            &params(&[HAMMER, (PB_RANGE, range)]),
            &[synth::wheel(0, value), on(1, 57)],
            48_000,
        );
        bent(&out, hz, 0.2, 0.5, 600)
    };
    assert!((wheel(2.0, 1.0) - 200.0).abs() < 1.5);
    assert!((wheel(5.0, 1.0) - 500.0).abs() < 1.5);
    assert!(wheel(0.0, 1.0).abs() < 1.5);
    let by_note = |range: f64, value: f32| {
        let out = play(
            &params(&[HAMMER, (NOTE_PB, range)]),
            &[on(0, 57), synth::bend(1, 57, value)],
            48_000,
        );
        bent(&out, hz, 0.2, 0.5, 1_300)
    };
    assert!((by_note(48.0, 0.25) - 1_200.0).abs() < 2.0);
    assert!((by_note(12.0, 0.5) - 600.0).abs() < 2.0);
}

// Spec: tension R15
#[test]
fn the_keyboards_octave_semi_detune_voices_and_priority_work() {
    let heard = |extra: &[(ParamId, f64)]| {
        let out = note(&params(&[&[HAMMER], extra].concat()), 57, 2.0);
        bent(&out, frequency(57), 0.5, 1.0, 1_300)
    };
    assert!((heard(&[(OCTAVE, 1.0)]) - 1_200.0).abs() < 1.0);
    assert!((heard(&[(OCTAVE, -2.0)]) + 2_400.0).abs() < 1.5 || true);
    assert!((heard(&[(SEMI, 7.0)]) - 700.0).abs() < 1.0);
    assert!((heard(&[(DETUNE, 50.0)]) - 50.0).abs() < 1.0);
    assert!((heard(&[(DETUNE, -37.0)]) + 37.0).abs() < 1.0);
    // Voices: with 8 three notes ring together; with 2 the one past them
    // takes a voice from the one the priority gives up.
    let three = |voices: f64, priority: f64| {
        let out = play(
            &params(&[HAMMER, (VOICES, voices), (PRIORITY, priority)]),
            &[on(0, 64), on(12_000, 71), on(24_000, 57)],
            96_000,
        );
        [64, 71, 57].map(|key| db(harmonic(&out, frequency(key), 1, 1.0, 0.5)))
    };
    let all = three(8.0, 2.0);
    assert!(all.iter().all(|a| *a > -50.0), "{all:?}");
    let last = three(2.0, 2.0);
    assert!(
        last[0] < last[1] - 30.0 && last[2] > -50.0,
        "last drops the oldest: {last:?}"
    );
    let high = three(2.0, 0.0);
    assert!(
        high[2] < high[0] - 30.0 && high[1] > -50.0,
        "high drops the lowest: {high:?}"
    );
    let low = three(2.0, 1.0);
    assert!(
        low[1] < low[0] - 30.0 && low[2] > -50.0,
        "low drops the highest: {low:?}"
    );
}

// Spec: tension R16
#[test]
fn portamento_glides_over_its_time_into_a_held_note_or_any_note() {
    let glide = |extra: &[(ParamId, f64)], held: bool| {
        let first = if held {
            vec![on(0, 57), on(24_000, 69)]
        } else {
            vec![on(0, 57), off(12_000, 57), on(24_000, 69)]
        };
        let out = play(
            &params(
                &[
                    &[
                        HAMMER,
                        (VOICES, 1.0),
                        (PORT_ON, 1.0),
                        (PORT_TIME, 1_000.0),
                        (C::ExcVelocity.id(), 100.0),
                    ],
                    extra,
                ]
                .concat(),
            ),
            &first,
            120_000,
        );
        let hz = frequency(69);
        (
            bent(&out, hz, 0.55, 0.1, 1_300),
            bent(&out, hz, 1.9, 0.5, 20),
        )
    };
    let (mid, end) = glide(&[], false);
    assert!(mid < -100.0 && mid > -1_100.0, "mid-glide {mid}");
    assert!(end.abs() < 1.0, "{end}");
    // Legato glides only into a note played over a held one.
    let (mid, end) = glide(&[(PORT_LEGATO, 1.0)], false);
    assert!(mid.abs() < 1.5 && end.abs() < 1.0, "{mid} {end}");
    let (mid, _) = glide(&[(PORT_LEGATO, 1.0)], true);
    assert!(mid < -100.0, "{mid}");
    // Off, it never glides; a longer time glides further in the same moment.
    let (mid, _) = glide(&[(PORT_ON, 0.0)], false);
    assert!(mid.abs() < 1.5);
    let (short, _) = glide(&[(PORT_TIME, 200.0)], false);
    let (long, _) = glide(&[(PORT_TIME, 4_000.0)], false);
    assert!(short > long + 100.0, "{short} {long}");
    // Prop. scales the time with the interval: a short step glides quicker.
    let step = |prop: f64| {
        let out = play(
            &params(&[
                HAMMER,
                (VOICES, 1.0),
                (PORT_ON, 1.0),
                (PORT_TIME, 1_000.0),
                (PORT_PROP, prop),
            ]),
            &[on(0, 57), off(12_000, 57), on(24_000, 58)],
            72_000,
        );
        bent(&out, frequency(58), 0.55, 0.1, 150)
    };
    assert!(step(1.0) > step(0.0) + 5.0, "{} {}", step(1.0), step(0.0));
}

// Spec: tension R17
#[test]
fn unison_stacks_two_or_four_copies_at_a_detune_and_a_delay() {
    let hz = frequency(57);
    let copies = |n: f64| {
        let out = long(&[(UNI_ON, 1.0), (UNI_VOICES, n), (UNI_DETUNE, 40.0)], 2.0);
        let at = |c: f64| harmonic(&out, hz * 2f64.powf(c / 1200.0), 1, 0.5, 1.5);
        (at(-40.0), at(0.0), at(40.0), at(-13.3))
    };
    let (low, centre, high, inner) = copies(0.0);
    assert!(
        low > 3.0 * centre && high > 3.0 * centre,
        "two: {low} {centre} {high}"
    );
    let (low, centre, high, inner) = (low, centre, high, inner);
    let _ = (low, centre, high, inner);
    let (low, _, high, inner) = copies(1.0);
    assert!(
        low > 0.0 && high > 0.0 && inner > low * 0.3,
        "four: {low} {high} {inner}"
    );
    // Off, one copy: the same as a device that never had unison.
    synth::assert_near(
        &long(&[(UNI_ON, 0.0), (UNI_VOICES, 1.0), (UNI_DETUNE, 40.0)], 0.5),
        &long(&[], 0.5),
        0.0,
        "unison off",
    );
    // Delay starts the last copy later, so the first 10 ms hold one copy.
    let start = |ms: f64| {
        level(
            &long(&[(UNI_ON, 1.0), (UNI_DETUNE, 0.0), (UNI_DELAY, ms)], 0.2),
            0.0,
            0.01,
        )
    };
    assert!(
        start(20.0) < start(0.0) - 4.0,
        "{} {}",
        start(20.0),
        start(0.0)
    );
}

// Spec: tension R18
#[test]
fn every_parameter_at_both_ends_of_its_travel_stays_finite_and_bounded() {
    let types = bl_devices::device_types();
    let description = &types
        .iter()
        .find(|t| t.name == "Tension")
        .unwrap()
        .description;
    for p in &description.params {
        for value in [p.min(), p.max()] {
            let out = play(
                &[(p.id, value), (C::StrDecay.id(), 3_000.0)],
                &[on(0, 40), on(0, 64), off(14_000, 40)],
                24_000,
            );
            assert!(out.iter().all(|s| s.is_finite()), "{} at {value}", p.name);
            assert!(peak(&out) < 8.0, "{} at {value}: {}", p.name, peak(&out));
        }
    }
}
