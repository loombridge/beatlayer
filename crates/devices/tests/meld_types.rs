//! Meld, part 2 (WP-8.18; the meld spec, R24 to R39): the other fifteen
//! oscillator types, each with its two macros, measured by FFT.
//!
//! Every block runs as a real-time section under the allocation trap, as
//! part 1's tests do.

use crate::common::node;
use crate::meld::{
    BLOCKS, HZ, Params, Setting, a, aliasing_db, amp, assert_near, assert_same, b, c_major,
    centroid, crossing_hz, hz, meld_with, off, on, phrase, play, play_with, power_outside,
    registry, render_node, rms, steady,
};
use bl_core::id::Id;
use bl_devices::meld::{MACRO_1, MACRO_2, OSC_SCALE, TYPE, USE_SCALE};
use bl_engine::NoteEvent;
use bl_model::DeviceId;
use bl_model::{Scale, ScaleName};

const HARMONIC_FM: f64 = 10.0;
const FOLD_FM: f64 = 11.0;
const SQUELCH: f64 = 12.0;
const SIMPLE_FM: f64 = 13.0;
const CHIP: f64 = 14.0;
const SHEPARDS_PI: f64 = 15.0;
const TARP: f64 = 16.0;
const EXTRATONE: f64 = 17.0;
const NOISE_LOOP: f64 = 18.0;
const FILTERED_NOISE: f64 = 19.0;
const BITGRUNGE: f64 = 20.0;
const CRACKLE: f64 = 21.0;
const RAIN: f64 = 22.0;
const BUBBLE: f64 = 23.0;
const CHORD: f64 = 24.0;

fn typed(kind: f64, m1: f64, m2: f64) -> Params {
    vec![(a(TYPE), kind), (a(MACRO_1), m1), (a(MACRO_2), m2)]
}

/// The Bessel function of the first kind J_n(x), by its series.
fn bessel(n: i32, x: f64) -> f64 {
    let n = n.unsigned_abs() as usize;
    let mut term = (x / 2.0).powi(n as i32) / (1..=n).map(|k| k as f64).product::<f64>();
    let mut sum = term;
    for k in 1..40 {
        term *= -(x * x / 4.0) / (k as f64 * (k + n) as f64);
        sum += term;
    }
    sum
}

// Spec: meld R24 (Harmonic FM), R26 (Squelch), R27 (Simple FM)
#[test]
fn the_fm_types_put_the_bessel_sidebands_where_their_ratios_say() {
    let f0 = hz(48.0);
    let near = |measured: f64, expected: f64, what: &str| {
        assert!(
            (measured - expected).abs() < 0.015,
            "{what}: {measured:.4}, not {expected:.4}"
        );
    };
    // Amount 25 % is an index of 2.
    let beta = 2.0;
    // Harmonic FM: Ratio over its travel is a whole number from 1 to 16; at
    // 9 the sidebands sit at 9n above and below the carrier.
    let nine = steady(&typed(HARMONIC_FM, 25.0, 50.0), 48);
    near(amp(&nine, f0), bessel(0, beta).abs(), "Harmonic FM carrier");
    near(
        amp(&nine, 10.0 * f0),
        bessel(1, beta).abs(),
        "Harmonic FM upper sideband",
    );
    near(
        amp(&nine, 8.0 * f0),
        bessel(1, beta).abs(),
        "Harmonic FM lower sideband",
    );
    near(
        amp(&nine, 19.0 * f0),
        bessel(2, beta).abs(),
        "Harmonic FM second sideband",
    );
    let sixteen = steady(&typed(HARMONIC_FM, 25.0, 100.0), 48);
    near(
        amp(&sixteen, 17.0 * f0),
        bessel(1, beta).abs(),
        "Harmonic FM at 16",
    );
    let one = steady(&typed(HARMONIC_FM, 0.0, 100.0), 48);
    near(amp(&one, f0), 1.0, "Amount 0 is the carrier alone");
    // Simple FM: a ratio of 0.5 to 16, logarithmically: 2.83 in the middle,
    // so the sidebands fall between whole multiples of the note.
    let ratio = 0.5 * 32f64.powf(0.5);
    let simple = steady(&typed(SIMPLE_FM, 25.0, 50.0), 48);
    near(amp(&simple, f0), bessel(0, beta).abs(), "Simple FM carrier");
    near(
        amp(&simple, f0 * (1.0 + ratio)),
        bessel(1, beta).abs(),
        "Simple FM upper sideband",
    );
    near(
        amp(&simple, f0 * (ratio - 1.0)),
        bessel(1, beta).abs(),
        "Simple FM lower sideband",
    );
    let half = steady(&typed(SIMPLE_FM, 25.0, 0.0), 48);
    near(
        amp(&half, f0 * 1.5),
        bessel(1, beta).abs(),
        "Simple FM at 0.5",
    );
    // Squelch: the modulator at twice the note, so the carrier's amplitude
    // is J0 plus the reflected first sideband, and Feedback changes the
    // higher sidebands.
    let plain = steady(&typed(SQUELCH, 25.0, 0.0), 48);
    near(
        amp(&plain, f0),
        bessel(0, beta) + bessel(1, beta),
        "Squelch carrier",
    );
    let fed = steady(&typed(SQUELCH, 25.0, 100.0), 48);
    assert!(
        (amp(&fed, 5.0 * f0) - amp(&plain, 5.0 * f0)).abs() > 0.1,
        "Feedback changes the fifth harmonic from {:.3} to {:.3}",
        amp(&plain, 5.0 * f0),
        amp(&fed, 5.0 * f0)
    );
    assert!(fed.iter().all(|x| x.abs() < 2.0), "Feedback stays bounded");
}

// Spec: meld R25
#[test]
fn fold_fm_folds_a_sine_into_odd_harmonics_and_amount_brightens_it() {
    let f0 = hz(48.0);
    // Amount 0, a sine carrier through the sine fold: sin(pi sin t) has
    // odd harmonics 2 J_n(pi).
    let sine = steady(&typed(FOLD_FM, 0.0, 0.0), 48);
    for n in [1, 3, 5] {
        let want = 2.0 * bessel(n, std::f64::consts::PI);
        assert!(
            (amp(&sine, f64::from(n) * f0) - want).abs() < 0.01,
            "harmonic {n}: {:.4}, not {want:.4}",
            amp(&sine, f64::from(n) * f0)
        );
    }
    assert!(
        amp(&sine, 2.0 * f0) < 0.005,
        "a sine folds to odd harmonics"
    );
    // Shape moves the carrier to a triangle, which folds differently.
    let triangle = steady(&typed(FOLD_FM, 0.0, 100.0), 48);
    assert!((amp(&triangle, f0) - amp(&sine, f0)).abs() > 0.1);
    // Amount adds modulation index, and with it a brighter, busier sound.
    let bright = steady(&typed(FOLD_FM, 100.0, 0.0), 48);
    let (lo, hi) = (f0 * 0.5, 9_000.0);
    assert!(centroid(&bright, lo, hi) > 1.5 * centroid(&sine, lo, hi));
    assert!(bright.iter().all(|x| x.abs() <= 1.0 + 1e-3));
}

/// The ratio of a pulse of width `duty`'s second harmonic to its first.
fn pulse_second(duty: f64) -> f64 {
    (std::f64::consts::PI * duty).cos().abs()
}

/// Seconds `from` to `to` of `signal`.
fn window(signal: &[f32], from: f64, to: f64) -> &[f32] {
    &signal[(from * HZ) as usize..(to * HZ) as usize]
}

// Spec: meld R28
#[test]
fn chip_steps_its_pulse_width_and_octave_with_tone_and_arpeggiates_with_rate() {
    let key = 60u8;
    let f0 = hz(f64::from(key));
    // Tone in sixteenths of its travel: the width falls from 50 % in steps
    // of a fortieth, which the second harmonic's share of the first shows.
    // The first third of the travel stays at the note's octave.
    for step in [0u32, 1, 2, 3, 4] {
        let tone = f64::from(step) / 15.0 * 100.0;
        let width = 0.5 - 0.375 * f64::from(step) / 15.0;
        // At Rate 0 the arpeggio is the note's for its first third of a
        // second.
        let [left, _] = play(&typed(CHIP, tone, 0.0), 12_000, &[on(0, key)]);
        let early = &left[1_000..8_000];
        let ratio = amp(early, 2.0 * f0) / amp(early, f0);
        assert!(
            (ratio - pulse_second(width)).abs() < 0.02,
            "step {step}: second over first is {ratio:.3}, not {:.3}",
            pulse_second(width)
        );
    }
    // Between steps the width holds: 8 % and 12 % of the travel are the
    // same step.
    let [one, _] = play(&typed(CHIP, 8.0, 0.0), 12_000, &[on(0, key)]);
    let [two, _] = play(&typed(CHIP, 9.0, 0.0), 12_000, &[on(0, key)]);
    assert_eq!(one[1_000..8_000], two[1_000..8_000], "one step, one wave");
    // Tone climbs through 0, 12 and 24 semitones in thirds of its travel.
    for (tone, octaves) in [(10.0, 0.0), (40.0, 1.0), (90.0, 2.0)] {
        let [left, _] = play(&typed(CHIP, tone, 0.0), 12_000, &[on(0, key)]);
        let expected = f0 * 2f64.powf(octaves);
        let measured = crossing_hz(&left[2_000..10_000]);
        assert!(
            (measured / expected - 1.0).abs() < 0.02,
            "tone {tone}: {measured:.1} Hz, not {expected:.1}"
        );
    }
    // Rate 0 is one cycle a second: the note, a third above, a fifth
    // above, each for a third of a second.
    let [left, _] = play(&typed(CHIP, 0.0, 0.0), 48_000, &[on(0, key)]);
    for (from, semitones) in [(0.05, 0.0), (0.4, 4.0), (0.72, 7.0)] {
        let expected = f0 * (semitones / 12.0f64).exp2();
        let measured = centroid(
            window(&left, from, from + 0.2),
            expected * 0.9,
            expected * 1.1,
        );
        assert!(
            (measured / expected - 1.0).abs() < 0.01,
            "from {from} s: {measured:.1} Hz, not {expected:.1}"
        );
    }
    // Rate 100 % is 50 cycles a second: twenty times the steps.
    let [fast, _] = play(&typed(CHIP, 0.0, 100.0), 48_000, &[on(0, key)]);
    let steps = |signal: &[f32]| {
        let third = f0 * (4.0f64 / 12.0).exp2();
        let (mut last, mut changes) = (false, 0);
        for chunk in signal.chunks(480) {
            let now = amp(chunk, third) > amp(chunk, f0);
            changes += usize::from(now != last);
            last = now;
        }
        changes
    };
    assert!(
        steps(&fast) > 5 * steps(&left),
        "{} against {}",
        steps(&fast),
        steps(&left)
    );
}

// Spec: meld R28, R38 (the scale's first, third and fifth degrees)
#[test]
fn chips_arpeggio_follows_the_current_scale_with_both_switches_on() {
    let key = 60u8;
    let f0 = hz(f64::from(key));
    let minor = Setting {
        scales: vec![(0, Scale::new(0, ScaleName::Minor))],
        ..Setting::default()
    };
    let play_minor = |extra: &[(bl_model::ParamId, f64)]| {
        let mut params = typed(CHIP, 0.0, 0.0);
        params.extend_from_slice(extra);
        play_with(&params, 512, 48_000, &[on(0, key)], &minor)[0].clone()
    };
    // C minor's third and fifth degrees from C are E flat and G.
    let scaled = play_minor(&[(USE_SCALE, 1.0)]);
    for (from, semitones) in [(0.05, 0.0), (0.4, 3.0), (0.72, 7.0)] {
        let expected = f0 * (semitones / 12.0f64).exp2();
        let measured = centroid(
            window(&scaled, from, from + 0.2),
            expected * 0.9,
            expected * 1.1,
        );
        assert!(
            (measured / expected - 1.0).abs() < 0.01,
            "scaled, from {from} s: {measured:.1} Hz, not {expected:.1}"
        );
    }
    // With Oscillator Scale Awareness off, or Use Current Scale off, it is
    // the major third.
    for extra in [vec![(USE_SCALE, 1.0), (a(OSC_SCALE), 0.0)], vec![]] {
        let plain = play_minor(&extra);
        let expected = f0 * (4.0f64 / 12.0).exp2();
        let measured = centroid(window(&plain, 0.4, 0.6), expected * 0.9, expected * 1.1);
        assert!(
            (measured / expected - 1.0).abs() < 0.01,
            "{extra:?}: {measured:.1} Hz"
        );
    }
}

/// The partials of Shepard's Pi's levels as the law gives them.
fn shepard_levels(width: f32, slide: f32) -> Vec<(f32, f32)> {
    let mut all: Vec<(f32, f32)> = (0..10)
        .map(|partial| bl_devices::meld::shepard(partial, slide, width))
        .collect();
    all.sort_by(|x, y| x.0.total_cmp(&y.0));
    all
}

// Spec: meld R29
#[test]
fn shepards_pi_slides_without_a_step_at_the_loop_point() {
    // The partials' levels, by their octave, move smoothly through the
    // slide's whole run and past the point where one leaves the bell's top
    // and another enters at its foot.
    for width in [1.0, 2.5, 6.0, 10.0] {
        let steps = 20_000;
        let mut before = shepard_levels(width, 0.0);
        for k in 1..=steps {
            let slide = 12.0 * k as f32 / steps as f32;
            let now = shepard_levels(width, slide);
            for (x, y) in before.iter().zip(&now) {
                // A partial moves 12/steps of an octave a step; the bell's
                // slope bounds how far its level moves with it, except
                // where the sort orders two partials differently, which a
                // wrap makes.
                if (x.0 - y.0).abs() < 0.01 {
                    assert!(
                        (x.1 - y.1).abs() < 0.01,
                        "width {width} slide {slide}: level {} to {}",
                        x.1,
                        y.1
                    );
                }
            }
            // The levels as a whole barely move: every partial that leaves
            // or enters does so at zero.
            let sum_before: f32 = before.iter().map(|p| p.1 * p.1).sum();
            let sum_now: f32 = now.iter().map(|p| p.1 * p.1).sum();
            assert!(
                (sum_before - sum_now).abs() < 0.02,
                "width {width} slide {slide}"
            );
            before = now;
        }
    }
}

// Spec: meld R29
#[test]
fn shepards_pi_rises_above_fifty_percent_falls_below_and_stands_still_at_it() {
    // A Width of 1 octave plays one partial sweeping up (or down) a bell's
    // octave a second: its frequency at a quarter and at three quarters of
    // the second.
    let key = 60u8;
    let sweep = |rate: f64| {
        let [left, _] = play(&typed(SHEPARDS_PI, rate, 0.0), 48_000, &[on(0, key)]);
        let early = crossing_hz(window(&left, 0.15, 0.3));
        let late = crossing_hz(window(&left, 0.7, 0.85));
        (early, late)
    };
    let (up_early, up_late) = sweep(100.0);
    assert!(up_late > 1.2 * up_early, "{up_early:.1} to {up_late:.1} Hz");
    let (down_early, down_late) = sweep(0.0);
    assert!(
        down_early > 1.2 * down_late,
        "{down_early:.1} to {down_late:.1} Hz"
    );
    // At 50 % the partials stand still.
    let [left, _] = play(&typed(SHEPARDS_PI, 50.0, 100.0), 48_000, &[on(0, key)]);
    let a = &left[24_000..36_000];
    let b = &left[36_000..48_000];
    assert!((rms(a) / rms(b) - 1.0).abs() < 0.02);
    // A wide bell holds all its partials an octave apart, as the note and
    // the octaves around it.
    let f0 = hz(f64::from(key));
    for octave in [-3, -1, 0, 2] {
        let f = f0 * 2f64.powi(octave);
        assert!(
            amp(&left[24_000..], f) > 0.05,
            "a partial at octave {octave}"
        );
    }
}

/// The notes in semitones above the key a Chord of `shape` and `inversion`
/// plays in the major scale.
fn chord_in_c_major(m1: f64, m2: f64, extra: &[(bl_model::ParamId, f64)]) -> Vec<f32> {
    let mut params = typed(CHORD, m1, m2);
    params.extend_from_slice(extra);
    steady(&params, 60)
}

// Spec: meld R36
#[test]
fn chord_plays_four_saws_by_shape_and_inversion() {
    let f0 = hz(60.0);
    let share = |signal: &[f32], semitones: f64| amp(signal, f0 * (semitones / 12.0).exp2());
    // Shape 0 in a major scale on the note is the third, fifth and sixth
    // degrees: E, G and A.
    let first = chord_in_c_major(0.0, 0.0, &[]);
    for semitones in [0.0, 4.0, 7.0, 9.0] {
        assert!(share(&first, semitones) > 0.1, "{semitones} st sounds");
    }
    for semitones in [1.0, 3.0, 6.0, 10.0] {
        assert!(share(&first, semitones) < 0.03, "{semitones} st is silent");
    }
    // The last of the twelve shapes: the fifth, the seventh and the tenth
    // degree, a third above the octave.
    let last = chord_in_c_major(100.0, 0.0, &[]);
    for semitones in [0.0, 7.0, 11.0, 16.0] {
        assert!(share(&last, semitones) > 0.1, "{semitones} st sounds");
    }
    // Inversion moves the lowest notes up an octave in turn: at 33 % the
    // note itself leaves.
    let inverted = chord_in_c_major(0.0, 100.0 / 3.0, &[]);
    assert!(share(&first, 0.0) > 5.0 * share(&inverted, 0.0));
    assert!(share(&inverted, 12.0) > 1.5 * share(&first, 12.0));
    let twice = chord_in_c_major(0.0, 200.0 / 3.0, &[]);
    assert!(
        share(&twice, 4.0) < share(&first, 4.0) / 5.0,
        "the third has gone up too"
    );
    // Four saws at the same level.
    assert!(rms(&first) > 0.4 && rms(&first) < 0.8);
}

// Spec: meld R36, R38
#[test]
fn chords_intervals_are_degrees_of_the_current_scale_when_it_is_on() {
    let f0 = hz(60.0);
    let setting = Setting {
        scales: vec![(0, Scale::new(0, ScaleName::Minor))],
        ..Setting::default()
    };
    let render = |extra: &[(bl_model::ParamId, f64)]| {
        let mut params = typed(CHORD, 0.0, 0.0);
        params.extend_from_slice(extra);
        play_with(&params, 512, 48_000, &[on(0, 60)], &setting)[0][24_000..].to_vec()
    };
    let share = |signal: &[f32], semitones: f64| amp(signal, f0 * (semitones / 12.0).exp2());
    // C minor: the third degree is E flat, the fifth G, the sixth A flat.
    let minor = render(&[(USE_SCALE, 1.0)]);
    for semitones in [0.0, 3.0, 7.0, 8.0] {
        assert!(
            share(&minor, semitones) > 0.1,
            "{semitones} st sounds in C minor"
        );
    }
    assert!(share(&minor, 4.0) < 0.03 && share(&minor, 9.0) < 0.03);
    // With Oscillator Scale Awareness off, or Use Current Scale off, a
    // major scale on the note.
    for extra in [vec![(USE_SCALE, 1.0), (a(OSC_SCALE), 0.0)], vec![]] {
        let major = render(&extra);
        for semitones in [0.0, 4.0, 7.0, 9.0] {
            assert!(
                share(&major, semitones) > 0.1,
                "{extra:?}: {semitones} st sounds"
            );
        }
        assert!(share(&major, 3.0) < 0.03, "{extra:?}");
    }
    let _ = c_major();
}

// Spec: meld R31
#[test]
fn extratone_repeats_a_falling_hit_at_the_notes_frequency() {
    let key = 60u8;
    let f0 = hz(f64::from(key));
    // Pitch 1 is a sine whatever Env Amount says; so is any Pitch with
    // Env Amount 0.
    let sine = steady(&typed(EXTRATONE, 0.0, 100.0), key);
    assert!((amp(&sine, f0) - 1.0).abs() < 0.01);
    let flat = steady(&typed(EXTRATONE, 100.0, 0.0), key);
    assert!((amp(&flat, f0) - 1.0).abs() < 0.01);
    // Each hit repeats at the note, so every sound is at a harmonic of it.
    for (pitch, depth) in [(30.0, 60.0), (100.0, 100.0), (60.0, 30.0)] {
        let hits = steady(&typed(EXTRATONE, pitch, depth), key);
        let alias = aliasing_db(&hits, &[f0]);
        assert!(
            alias < -60.0,
            "pitch {pitch} depth {depth}: {alias:.1} dB off the harmonics"
        );
    }
    // Pitch and Env Amount together brighten the hit: more of it above the
    // note.
    let share = |signal: &[f32]| 1.0 - amp(signal, f0).powi(2) / (2.0 * rms(signal).powi(2));
    let (low, mid, high) = (
        steady(&typed(EXTRATONE, 100.0, 15.0), key),
        steady(&typed(EXTRATONE, 100.0, 50.0), key),
        steady(&typed(EXTRATONE, 100.0, 100.0), key),
    );
    assert!(share(&low) < share(&mid) && share(&mid) < share(&high));
    let (small, big) = (
        steady(&typed(EXTRATONE, 15.0, 100.0), key),
        steady(&typed(EXTRATONE, 100.0, 100.0), key),
    );
    assert!(share(&small) < share(&big));
}

// Spec: meld R34
#[test]
fn bitgrunge_xors_a_square_with_squares_octaves_below_it() {
    let key = 60u8;
    let f0 = hz(f64::from(key));
    // Freq a third of the way is a ratio of 1; Mult at its maximum is no
    // squares below: a square at the note.
    let square = steady(&typed(BITGRUNGE, 100.0 / 3.0, 100.0), key);
    let ratio = amp(&square, 3.0 * f0) / amp(&square, f0);
    assert!(
        (ratio - 1.0 / 3.0).abs() < 0.02,
        "a square's third harmonic is {ratio:.3}"
    );
    assert!(amp(&square, 2.0 * f0) < 0.01);
    // One square below: the product has period two cycles, so it sits on
    // the half-multiples of the note and not on the note.
    let one = steady(&typed(BITGRUNGE, 100.0 / 3.0, 100.0 * 6.0 / 7.0), key);
    assert!(amp(&one, 0.5 * f0) > 0.2, "{:.3}", amp(&one, 0.5 * f0));
    assert!(amp(&one, f0) < 0.01, "{:.3}", amp(&one, f0));
    // Freq moves the square from half the note to four times it.
    for (freq, ratio) in [
        (0.0, 0.5),
        (100.0 / 3.0, 1.0),
        (200.0 / 3.0, 2.0),
        (100.0, 4.0),
    ] {
        let moved = steady(&typed(BITGRUNGE, freq, 100.0), key);
        let measured = crossing_hz(&moved);
        assert!(
            (measured / (f0 * ratio) - 1.0).abs() < 0.01,
            "Freq {freq}: {measured:.1} Hz, not {:.1}",
            f0 * ratio
        );
    }
}

/// Meld with device number `id`, so its seed is its own, rendered mono for
/// `frames`.
fn play_as(id: u64, params: &Params, frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
    let mut device = meld_with(params);
    device.id = DeviceId(Id::new(id).expect("an ID"));
    let mut node = node(&registry(), &device, 4_096);
    render_node(node.as_mut(), 512, frames, notes, &Setting::default())[0].clone()
}

/// How alike two renders are, from −1 to 1.
fn correlation(x: &[f32], y: &[f32]) -> f64 {
    let dot: f64 = x
        .iter()
        .zip(y)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    dot / (rms(x) * rms(y) * x.len() as f64)
}

/// Asserts that a random type renders the same from run to run and
/// differently on another device.
fn assert_seeded(params: &Params, key: u8) {
    let first = play_as(1, params, 24_000, &[on(0, key)]);
    let again = play_as(1, params, 24_000, &[on(0, key)]);
    assert_same(&again, &first, "run after run");
    let other = play_as(2, params, 24_000, &[on(0, key)]);
    assert!(
        correlation(&first, &other).abs() < 0.5,
        "two devices differ: correlation {:.3}",
        correlation(&first, &other)
    );
    assert!(rms(&first) > 1e-4 && rms(&other) > 1e-4);
}

// Spec: meld R30
#[test]
fn tarp_rings_at_the_note_for_as_long_as_decay_says_and_tone_brightens_it() {
    let key = 60u8;
    let f0 = hz(f64::from(key));
    let ring =
        |decay: f64, tone: f64| play(&typed(TARP, decay, tone), 48_000, &[on(0, key)])[0].clone();
    let short = ring(0.0, 100.0);
    let long = ring(100.0, 100.0);
    assert!(rms(window(&long, 0.5, 0.6)) > 30.0 * rms(window(&short, 0.5, 0.6)));
    assert!(rms(window(&short, 0.0, 0.01)) > 0.01, "the burst sounds");
    // The comb rings at the note.
    let measured = centroid(window(&long, 0.02, 0.2), f0 * 0.9, f0 * 1.1);
    assert!(
        (measured / f0 - 1.0).abs() < 0.01,
        "{measured:.1} Hz, not {f0:.1}"
    );
    // Decay 50 % is 0.2 s to fall 60 dB: the note's own sine falls a
    // thousandfold in the 0.2 s between two windows.
    let middle = ring(50.0, 100.0);
    let fall = amp(window(&middle, 0.1, 0.15), f0) / amp(window(&middle, 0.3, 0.35), f0);
    assert!((300.0..3_000.0).contains(&fall), "fell {fall:.0} times");
    // Tone is the loop's low-pass: a dark one rings lower.
    let dark = ring(100.0, 0.0);
    let bright_centroid = centroid(window(&long, 0.0, 0.1), 50.0, 10_000.0);
    let dark_centroid = centroid(window(&dark, 0.0, 0.1), 50.0, 10_000.0);
    assert!(
        bright_centroid > 1.5 * dark_centroid,
        "{bright_centroid:.0} against {dark_centroid:.0}"
    );
    assert_seeded(&typed(TARP, 50.0, 50.0), key);
}

// Spec: meld R32
#[test]
fn noise_loop_repeats_a_period_of_noise_until_rate_replaces_it() {
    let key = 60u8;
    let f0 = hz(f64::from(key));
    // Rate 0: the loop never changes, so every sound is at a multiple of
    // the note, and there is a good deal of them: noise, pitched.
    let still = steady(&typed(NOISE_LOOP, 0.0, 0.0), key);
    let alias = aliasing_db(&still, &[f0]);
    assert!(alias < -60.0, "{alias:.1} dB between the harmonics");
    assert!(
        power_outside(&still, &[(0.5 * f0, 1.5 * f0)]) > 0.8,
        "noise, not a sine"
    );
    // Rate 100 %: a fresh loop every period, so it is noise all the way.
    let fresh = steady(&typed(NOISE_LOOP, 100.0, 0.0), key);
    assert!(aliasing_db(&fresh, &[f0]) > -25.0);
    // Fade crossfades one loop into the next: the render differs and is no
    // louder.
    let faded = steady(&typed(NOISE_LOOP, 100.0, 100.0), key);
    assert!(faded != fresh);
    assert!(rms(&faded) < 1.5 * rms(&fresh) && rms(&faded) > 0.3 * rms(&fresh));
    assert_seeded(&typed(NOISE_LOOP, 0.0, 0.0), key);
    assert_seeded(&typed(NOISE_LOOP, 50.0, 50.0), key);
}

// Spec: meld R33
#[test]
fn filtered_noise_is_a_band_at_the_note_times_freq_as_wide_as_narrow_says() {
    let key = 72u8;
    let f0 = hz(f64::from(key));
    let band = |freq: f64, narrow: f64| steady(&typed(FILTERED_NOISE, freq, narrow), key);
    // Freq 50 % is a ratio of 1; 0 is a quarter of the note and 100 % four
    // times it.
    for (freq, ratio) in [(0.0, 0.25), (50.0, 1.0), (100.0, 4.0)] {
        let centre = f0 * ratio;
        let measured = centroid(&band(freq, 100.0), centre * 0.5, centre * 2.0);
        assert!(
            (measured / centre - 1.0).abs() < 0.03,
            "Freq {freq}: {measured:.1} Hz, not {centre:.1}"
        );
    }
    // Narrow: a twentieth of an octave keeps the sound within a few per
    // cent of the centre; four octaves spreads it far past.
    let around = [(0.95 * f0, 1.05 * f0)];
    assert!(power_outside(&band(50.0, 100.0), &around) < 0.5);
    assert!(power_outside(&band(50.0, 0.0), &around) > 0.9);
    // Whatever the width, it plays at about the same level.
    let (wide, narrow) = (rms(&band(50.0, 0.0)), rms(&band(50.0, 100.0)));
    assert!(
        wide / narrow > 0.3 && wide / narrow < 3.0,
        "{wide:.3} against {narrow:.3}"
    );
    assert_seeded(&typed(FILTERED_NOISE, 50.0, 50.0), key);
}

/// The clicks in `signal`: the times it rises past `threshold`.
fn clicks(signal: &[f32], threshold: f32) -> usize {
    let mut above = false;
    let mut count = 0;
    for x in signal {
        let now = x.abs() > threshold;
        count += usize::from(now && !above);
        above = now;
    }
    count
}

// Spec: meld R35
#[test]
fn crackle_clicks_at_density_and_intensity_widens_them() {
    let key = 60u8;
    let long = |density: f64, intensity: f64| {
        play(&typed(CRACKLE, density, intensity), 96_000, &[on(0, key)])[0].clone()
    };
    // 100 clicks a second on average is 60.6 % of a travel from 1 to
    // 2,000: about 200 in two seconds.
    let hundred = clicks(&long(60.59, 0.0), 0.1);
    assert!((140..260).contains(&hundred), "{hundred} clicks");
    let sparse = clicks(&long(30.0, 0.0), 0.1);
    assert!(sparse < hundred / 4, "{sparse} against {hundred}");
    // Intensity draws loudness from a wider distribution: the clicks' peaks
    // spread more about their mean.
    let spread = |signal: &[f32]| {
        let (mut peaks, mut top, mut inside) = (Vec::new(), 0.0f32, false);
        for x in signal {
            if x.abs() > 0.02 {
                top = top.max(x.abs());
                inside = true;
            } else if inside && x.abs() < 0.005 {
                peaks.push(f64::from(top));
                (top, inside) = (0.0, false);
            }
        }
        let mean = peaks.iter().sum::<f64>() / peaks.len() as f64;
        let variance = peaks.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / peaks.len() as f64;
        variance.sqrt() / mean
    };
    assert!(spread(&long(40.0, 100.0)) > 2.0 * spread(&long(40.0, 0.0)));
    assert_seeded(&typed(CRACKLE, 60.0, 50.0), key);
}

// Spec: meld R35
#[test]
fn rain_drops_at_rate_over_wind_that_rings_at_the_note_with_tone() {
    let key = 72u8;
    let f0 = hz(f64::from(key));
    let long =
        |tone: f64, rate: f64| play(&typed(RAIN, tone, rate), 96_000, &[on(0, key)])[0].clone();
    assert!(rms(&long(50.0, 100.0)) > 1.5 * rms(&long(50.0, 20.0)));
    // Tone: drops and wind resonate at the note.
    let near = [(0.8 * f0, 1.6 * f0)];
    let tuned = 1.0 - power_outside(&long(100.0, 60.0), &near);
    let loose = 1.0 - power_outside(&long(0.0, 60.0), &near);
    assert!(tuned > 2.0 * loose, "{tuned:.3} against {loose:.3}");
    assert_seeded(&typed(RAIN, 50.0, 60.0), key);
}

// Spec: meld R35
#[test]
fn bubbles_chirp_up_and_spread_varies_their_size() {
    let key = 72u8;
    let f0 = hz(f64::from(key));
    let long = |density: f64, spread: f64| {
        play(&typed(BUBBLE, density, spread), 192_000, &[on(0, key)])[0].clone()
    };
    assert!(rms(&long(100.0, 0.0)) > 1.5 * rms(&long(30.0, 0.0)));
    // Spread 0: every bubble near the note and above it, chirping up;
    // 100 %: an octave either way.
    let near = [(0.9 * f0, 1.9 * f0)];
    let steady_size = power_outside(&long(70.0, 0.0), &near);
    let varied = power_outside(&long(70.0, 100.0), &near);
    assert!(
        varied > 2.5 * steady_size && varied > 0.3,
        "{steady_size:.3} and {varied:.3}"
    );
    // One bubble: its pitch rises as it decays.
    let one = long(0.0, 0.0);
    let onset = one
        .iter()
        .position(|x| x.abs() > 0.05)
        .expect("a bubble in four seconds");
    let early = crossing_hz(&one[onset + 100..onset + 500]);
    let late = crossing_hz(&one[onset + 500..onset + 900]);
    assert!(late > 1.1 * early, "{early:.0} to {late:.0} Hz");
    assert_seeded(&typed(BUBBLE, 50.0, 50.0), key);
}

// Spec: meld R6, R37 (band-limited at the top key; the noise types are
// filtered, and Tarp, Filtered Noise, Crackle, Rain and Bubble sound at
// no note's harmonics)
#[test]
fn the_tonal_types_of_part_two_fold_back_less_than_minus_80_db_at_the_top_key() {
    let semitone = |n: f64| (n / 12.0).exp2();
    for key in [127u8, 108, 96, 84, 72, 60, 48] {
        let f0 = hz(f64::from(key));
        let mut cases: Vec<(String, Params, Vec<f64>)> = Vec::new();
        let mut add = |what: &str, params: Params, fundamentals: Vec<f64>| {
            cases.push((format!("{what} {params:?}"), params, fundamentals));
        };
        for amount in [0.0, 25.0, 100.0] {
            for ratio in [0.0, 50.0, 100.0] {
                add("Harmonic FM", typed(HARMONIC_FM, amount, ratio), vec![f0]);
            }
            // Simple FM at ratios of 2, 4, 8 and 16 stays on the note's
            // harmonics; Squelch is a ratio of 2.
            for ratio in [40.0, 60.0, 80.0, 100.0] {
                add("Simple FM", typed(SIMPLE_FM, amount, ratio), vec![f0]);
            }
            for feedback in [0.0, 100.0] {
                add("Squelch", typed(SQUELCH, amount, feedback), vec![f0]);
            }
            for shape in [0.0, 100.0] {
                add("Fold FM", typed(FOLD_FM, amount, shape), vec![f0]);
            }
        }
        for (pitch, depth) in [(100.0, 100.0), (50.0, 50.0), (100.0, 15.0)] {
            add("Extratone", typed(EXTRATONE, pitch, depth), vec![f0]);
        }
        // Noise Loop plays a table of noise with partials up to Nyquist, and
        // the kit's reads leave about −69 dB of image of that at middle C:
        // as with Rough, noise is R6's exception below the top octaves.
        if key >= 72 {
            add("Noise Loop", typed(NOISE_LOOP, 0.0, 0.0), vec![f0]);
        }
        for octaves in 0..3 {
            let freq = 100.0 / 3.0;
            let mult = 100.0 - 100.0 * f64::from(octaves) / 7.0;
            add(
                "Bitgrunge",
                typed(BITGRUNGE, freq, mult),
                vec![f0 / f64::from(1 << octaves)],
            );
        }
        for width in [100.0, 55.0, 0.0] {
            let w = 1.0 + 9.0 * width / 100.0;
            let partials = (0..10)
                .map(f64::from)
                .filter(|o| *o < w)
                .map(|o| f0 * (o - w / 2.0).exp2())
                .collect();
            add("Shepard's Pi", typed(SHEPARDS_PI, 50.0, width), partials);
        }
        for inversion in [0.0, 33.3, 66.7, 100.0] {
            add(
                "Chord",
                typed(CHORD, 0.0, inversion),
                [0.0, 4.0, 7.0, 9.0, 12.0, 16.0, 19.0, 21.0]
                    .iter()
                    .map(|n| f0 * semitone(*n))
                    .collect(),
            );
            add(
                "Chord",
                typed(CHORD, 100.0, inversion),
                [0.0, 7.0, 11.0, 12.0, 16.0, 19.0, 23.0, 28.0]
                    .iter()
                    .map(|n| f0 * semitone(*n))
                    .collect(),
            );
        }
        for (params, fundamentals) in cases.iter().map(|(_, p, f)| (p, f)) {
            let [left, _] = play(params, 48_000, &[on(0, key)]);
            let alias = aliasing_db(&left[16_384..], fundamentals);
            let loud = rms(&left[16_384..]);
            assert!(
                loud < 1e-6 || alias < -80.0,
                "{params:?} at key {key}: {alias:.1} dB (level {loud:.4})"
            );
        }
        // Chip's arpeggio, in its second step, a third above the note.
        for tone in [0.0, 10.0, 20.0] {
            let [left, _] = play(&typed(CHIP, tone, 0.0), 32_000, &[on(0, key)]);
            let third = f0 * semitone(4.0);
            let alias = aliasing_db(&left[16_384..], &[third]);
            assert!(
                rms(&left[16_384..]) < 1e-6 || alias < -80.0,
                "Chip tone {tone} at key {key}: {alias:.1} dB"
            );
        }
    }
}

// Spec: meld R23, R39
#[test]
fn every_type_of_part_two_renders_the_same_at_block_sizes_1_64_512_and_4096_and_run_after_run() {
    use bl_devices::meld::{
        DRIVE, FILTER_FREQ, FILTER_MACRO_1, FILTER_ON, GLIDE_MODE, GLIDE_TIME, LIMIT, MODE,
        OCTAVES, ON, PHASE_SPREAD, SEMITONES, SPREAD, STACK, TONE,
    };
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
                (USE_SCALE, 1.0),
                (a(SEMITONES), 2.0),
            ],
            Setting {
                scales: vec![(0, c_major()), (6_500, Scale::new(2, ScaleName::Minor))],
                ..Setting::default()
            },
        ),
        (
            "mono glissando in a scale",
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
    for kind in 10..25u32 {
        for (name, extra, setting) in &variants {
            // The kit chooses a mipmap level from the pitch's rise past the
            // end of a render, so a table whose pitch moves can change level
            // on another frame at another block size, as part 1 notes: the
            // types that play from the FM core and the generators don't.
            let gliding = name.contains("gli");
            if gliding && !matches!(kind, 10..=13 | 15 | 16 | 19 | 21..=23) {
                continue;
            }
            // Shepard's Pi's partials slide, in f32, between a block's cuts.
            let moves = gliding || kind == 15;
            let mut params = typed(f64::from(kind), 55.0, 62.0);
            params.extend_from_slice(extra);
            let reference = play_with(&params, 4096, 12_000, &phrase(), setting);
            let again = play_with(&params, 4096, 12_000, &phrase(), setting);
            for ch in 0..2 {
                assert_same(
                    &again[ch],
                    &reference[ch],
                    &format!("type {kind}, {name}, run after run"),
                );
            }
            for block in BLOCKS {
                let got = play_with(&params, block, 12_000, &phrase(), setting);
                for (ch, side) in ["left", "right"].into_iter().enumerate() {
                    let what = format!("type {kind}, {name}, block {block}, {side}");
                    if moves {
                        // A glide's pitch is a line over the frames a block
                        // leaves of a chunk, and the glide curves a little
                        // within one: a fold, a feedback or a comb of noise
                        // turns the difference into a few parts in ten
                        // thousand.
                        assert_near(&got[ch], &reference[ch], 1e-3, &what);
                    } else {
                        assert_same(&got[ch], &reference[ch], &what);
                    }
                }
            }
            assert!(rms(&reference[0]) > 0.001, "type {kind} {name} sounds");
        }
    }
}

// Spec: meld R38 (from the frame the current scale changes)
#[test]
fn a_held_chord_follows_the_current_scale_from_the_frame_it_changes() {
    let f0 = hz(60.0);
    let setting = Setting {
        scales: vec![(0, c_major()), (24_000, Scale::new(0, ScaleName::Minor))],
        ..Setting::default()
    };
    let mut params = typed(CHORD, 0.0, 0.0);
    params.extend_from_slice(&[(USE_SCALE, 1.0)]);
    let left = play_with(&params, 512, 48_000, &[on(0, 60)], &setting)[0].clone();
    let share = |signal: &[f32], semitones: f64| amp(signal, f0 * (semitones / 12.0).exp2());
    let (before, after) = (&left[8_000..24_000], &left[32_000..]);
    // C major's third degree is E, C minor's E flat; the fifth is G in both.
    assert!(share(before, 4.0) > 0.1 && share(before, 3.0) < 0.03);
    assert!(share(after, 3.0) > 0.1 && share(after, 4.0) < 0.03);
    assert!(share(before, 7.0) > 0.1 && share(after, 7.0) > 0.1);
    // The note itself, its envelope and its level never moved.
    assert!((rms(before) / rms(after) - 1.0).abs() < 0.1);
}

/// A phrase for the golden renders: a note, a second over it, and the
/// first leaving.
fn golden_phrase() -> Vec<NoteEvent> {
    vec![on(0, 60), on(6_000, 67), off(12_000, 60), off(16_000, 67)]
}

/// The macros each type's golden render sets: mid-travel, with the
/// textures dense enough to sound in every window.
fn golden_macros(kind: u32) -> (f64, f64) {
    match kind {
        16 => (70.0, 50.0),
        21 => (70.0, 70.0),
        23 => (80.0, 60.0),
        _ => (40.0, 70.0),
    }
}
const GOLDEN_AT: [usize; 7] = [0, 500, 2_000, 6_000, 8_000, 13_000, 17_000];
const GOLDEN_WINDOW: usize = 512;

/// The RMS of each window of the golden render of types 10 to 24, in the
/// left channel, rendered when the types were written. A window's level,
/// not a sample, is compared: the platforms' maths libraries differ in the
/// last bit, and the feedback, the comb and the textures carry that into
/// single samples.
const GOLDEN: [[f64; 7]; 15] = [
    [
        0.7083014, 0.7072158, 0.6945392, 0.8951285, 1.0917574, 0.7069217, 0.1957748,
    ],
    [
        0.5826939, 0.5983218, 0.5968339, 0.9506886, 0.9533918, 0.7142843, 0.2035645,
    ],
    [
        0.5994770, 0.5954255, 0.5873406, 0.8608306, 0.8309414, 0.6513869, 0.1761075,
    ],
    [
        0.7124706, 0.7075422, 0.7014998, 1.0317281, 0.9895533, 0.7445034, 0.1997031,
    ],
    [
        0.7410142, 0.7134485, 0.7228198, 0.9164738, 1.0169995, 0.7437809, 0.2037699,
    ],
    [
        0.6770331, 0.7170571, 0.7353234, 1.1097794, 0.9360906, 0.7438800, 0.1844448,
    ],
    [
        0.3807693, 0.1354924, 0.0741502, 0.3349841, 0.0600905, 0.0122298, 0.0012828,
    ],
    [
        0.6212572, 0.6340965, 0.6348204, 0.8148401, 0.9599847, 0.6075026, 0.1772983,
    ],
    [
        0.1290591, 0.1490303, 0.1361681, 0.1797635, 0.1856009, 0.1267548, 0.0320190,
    ],
    [
        0.1376517, 0.1141741, 0.1365177, 0.3036938, 0.1878788, 0.2590112, 0.0502195,
    ],
    [
        0.9975032, 0.9976519, 0.9982085, 1.5735775, 1.2221899, 0.9932634, 0.2800882,
    ],
    [
        0.2247842, 0.1690918, 0.0562274, 0.2443868, 0.0209150, 0.1675793, 0.0000001,
    ],
    [
        0.1063401, 0.1513156, 0.2773401, 0.0315346, 0.1863344, 0.1682038, 0.0592885,
    ],
    [
        0.2410020, 0.2949089, 0.1674401, 0.1563030, 0.2702042, 0.1747851, 0.0422156,
    ],
    [
        0.7311925, 0.7002089, 0.7147047, 1.1617720, 1.0532398, 0.7205776, 0.1913228,
    ],
];

// Spec: meld R24 to R36 (the golden renders)
#[test]
fn each_type_renders_a_phrase_as_it_did_when_it_was_written() {
    for (n, golden) in GOLDEN.iter().enumerate() {
        let kind = 10 + n as u32;
        let (m1, m2) = golden_macros(kind);
        let [left, _] = play(&typed(f64::from(kind), m1, m2), 20_000, &golden_phrase());
        for (&at, &want) in GOLDEN_AT.iter().zip(golden) {
            let level = rms(&left[at..at + GOLDEN_WINDOW]);
            assert!(
                (level - want).abs() <= 2e-3 * want + 1e-5,
                "type {kind}, frames {at}+: RMS {level} isn't {want}"
            );
        }
    }
}
