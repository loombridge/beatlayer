//! Random, the random-pitch MIDI effect (WP-5.30; the MIDI random pitch spec):
//! chance, choices, interval, the two modes and the three signs, seeded so a
//! render repeats. Every block is a real-time section under the allocation
//! trap.

use crate::common;
use bl_devices::dice::{CHANCE, CHOICES, DICE, INTERVAL, MODE, SIGN, USE_SCALE};
use bl_engine::NoteKind;
use bl_engine::scales::ScaleRuns;
use bl_model::{Scale, ScaleName};
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};
use common::{bend, bends, device, keys_on, node, off, on, registry, scaled_through};

/// Eight notes of middle C, a beat each, played through Random.
fn render(params: &[(bl_model::ParamId, f64)], size: usize) -> Vec<(i64, NoteKind, u8, f32)> {
    let mut chain = Chain::new(4096);
    chain.effect(DICE, params);
    chain.clip(16, (0..16).map(|b| note(60, b, 1)));
    chain.sync();
    chain.play();
    chain.run(17 * BEAT_FRAMES as usize, size);
    chain.heard()
}

fn ons(heard: &[(i64, NoteKind, u8, f32)]) -> Vec<i32> {
    heard
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| i32::from(h.2) - 60)
        .collect()
}

// Spec: midi-random-pitch R1. At 0 % Chance every note passes unchanged.
#[test]
fn zero_chance_leaves_every_note_alone() {
    let heard = render(&[(CHANCE, 0.0)], 512);
    assert_eq!(ons(&heard), [0; 16]);
}

// Spec: midi-random-pitch R2. Add moves a note up by a whole number of
// intervals, from one to Choices of them.
#[test]
fn add_moves_notes_up_by_choices_of_the_interval() {
    for size in [1, 100, 512, 4096] {
        let moves = ons(&render(
            &[
                (CHANCE, 100.0),
                (CHOICES, 4.0),
                (INTERVAL, 3.0),
                (SIGN, 0.0),
            ],
            size,
        ));
        assert!(moves.iter().all(|m| [3, 6, 9, 12].contains(m)), "{moves:?}");
        assert!(moves.iter().any(|m| *m != moves[0]), "they vary: {moves:?}");
    }
}

// Spec: midi-random-pitch R3. Sub moves notes down; Bi either way.
#[test]
fn sub_moves_down_and_bi_either_way() {
    let down = ons(&render(
        &[(CHOICES, 2.0), (INTERVAL, 5.0), (SIGN, 1.0)],
        512,
    ));
    assert!(down.iter().all(|m| [-5, -10].contains(m)), "{down:?}");
    let both = ons(&render(
        &[(CHOICES, 2.0), (INTERVAL, 5.0), (SIGN, 2.0)],
        512,
    ));
    assert!(
        both.iter().all(|m| [-10, -5, 5, 10].contains(m)),
        "{both:?}"
    );
    assert!(
        both.iter().any(|m| *m < 0) && both.iter().any(|m| *m > 0),
        "{both:?}"
    );
}

// Spec: midi-random-pitch R4. Alt goes through the distances in a fixed
// order; at 100 % each note takes the next one.
#[test]
fn alt_goes_through_the_choices_in_turn() {
    let moves = ons(&render(
        &[(CHOICES, 3.0), (INTERVAL, 2.0), (MODE, 1.0)],
        512,
    ));
    assert_eq!(&moves[..7], [2, 4, 6, 2, 4, 6, 2]);
    // Bi alternates up and down.
    let bi = ons(&render(
        &[(CHOICES, 2.0), (INTERVAL, 1.0), (MODE, 1.0), (SIGN, 2.0)],
        512,
    ));
    assert_eq!(&bi[..5], [1, -1, 2, -2, 1]);
}

// Spec: midi-random-pitch R5. Chance is how often a note moves.
#[test]
fn chance_sets_how_often_notes_move() {
    let moves = ons(&render(
        &[(CHANCE, 50.0), (CHOICES, 1.0), (INTERVAL, 7.0)],
        512,
    ));
    let moved = moves.iter().filter(|m| **m != 0).count();
    assert!((2..=14).contains(&moved), "{moved} of 16 moved: {moves:?}");
}

// Spec: midi-random-pitch R6. Two renders are bit for bit the same.
#[test]
fn two_renders_are_identical() {
    let params = [
        (CHANCE, 70.0),
        (CHOICES, 12.0),
        (INTERVAL, 2.0),
        (SIGN, 2.0),
    ];
    assert_eq!(render(&params, 512), render(&params, 100));
}

// Spec: midi-random-pitch R7. Each note ends on the key it moved to, and a
// note near the top of the keyboard folds back onto it.
#[test]
fn every_note_ends_on_its_own_key() {
    let heard = render(&[(CHOICES, 24.0), (INTERVAL, 4.0), (SIGN, 2.0)], 512);
    assert!(nothing_hangs(&heard));
    assert_eq!(
        heard.iter().filter(|h| h.1 == NoteKind::Off).count(),
        16,
        "{heard:?}"
    );
    let mut chain = Chain::new(512);
    chain.effect(DICE, &[(CHOICES, 24.0), (INTERVAL, 24.0)]);
    chain.clip(4, [note(125, 0, 1)]);
    chain.sync();
    chain.play();
    chain.run(2 * BEAT_FRAMES as usize, 512);
    assert!(chain.heard().iter().all(|h| h.2 <= 127));
}

fn c_major() -> Scale {
    Scale::new(0, ScaleName::Major)
}

/// Sixteen notes of `key`, a beat each, played through Random counting
/// degrees of the clip's C Major, and the keys that sounded.
fn degrees(params: &[(bl_model::ParamId, f64)], key: u8, size: usize) -> Vec<u8> {
    let mut chain = Chain::new(4096);
    chain.effect(DICE, params);
    chain.clip(16, (0..16).map(|b| note(key, b, 1)));
    chain.scale_clips(c_major());
    chain.sync();
    chain.play();
    chain.run(17 * BEAT_FRAMES as usize, size);
    chain
        .heard()
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.2)
        .collect()
}

// Spec: midi-random-pitch R10. With Use Current Scale on, a note that moves goes
// k times Interval degrees: a note in the scale lands on a scale note, and
// so does one outside it, counted from the scale note below.
#[test]
fn use_current_scale_moves_notes_by_degrees_and_lands_in_the_scale() {
    let params = [
        (USE_SCALE, 1.0),
        (CHANCE, 100.0),
        (CHOICES, 4.0),
        (INTERVAL, 2.0),
        (SIGN, 2.0),
    ];
    for size in [1, 512, 4096] {
        for key in [60, 61, 66] {
            let keys = degrees(&params, key, size);
            assert_eq!(keys.len(), 16);
            assert!(keys.iter().all(|k| c_major().holds(*k)), "{key}: {keys:?}");
            // Two, four, six or eight degrees away from the scale note at or
            // below the key: C3 (60) for 60 and 61, F3 (65) for 66.
            let base = [(60, 60), (61, 60), (66, 65)]
                .iter()
                .find(|b| b.0 == key)
                .unwrap()
                .1;
            let allowed: Vec<u8> = [-8, -6, -4, -2, 2, 4, 6, 8]
                .iter()
                .map(|d| c_major().land_off(base, *d) as u8)
                .collect();
            assert!(
                keys.iter().all(|k| allowed.contains(k)),
                "{keys:?} {allowed:?}"
            );
        }
    }
    // A note Chance leaves alone stays as it came, in the scale or not.
    let still = [(USE_SCALE, 1.0), (CHANCE, 0.0)];
    assert_eq!(degrees(&still, 61, 512), [61; 16]);
}

// Spec: midi-random-pitch R10. With the switch off, or a current scale with
// Scale Mode off, distances are semitones.
#[test]
fn without_a_scale_the_distances_are_semitones() {
    let params = [
        (USE_SCALE, 1.0),
        (CHANCE, 100.0),
        (CHOICES, 1.0),
        (INTERVAL, 3.0),
        (SIGN, 0.0),
    ];
    let mut chain = Chain::new(512);
    chain.effect(DICE, &params);
    chain.clip(2, [note(61, 0, 1)]);
    chain.sync();
    chain.play();
    chain.run(2 * BEAT_FRAMES as usize, 512);
    // The clip's scale is off, so the chromatic scale: three semitones.
    assert_eq!(ons(&chain.heard()), [64 - 60]);
}

// Spec: midi-random-pitch R10, R6. Two renders with the scale are bit for
// bit the same, at any block size.
#[test]
fn two_renders_with_the_scale_are_identical() {
    let params = [
        (USE_SCALE, 1.0),
        (CHANCE, 70.0),
        (CHOICES, 12.0),
        (INTERVAL, 2.0),
        (SIGN, 2.0),
    ];
    assert_eq!(degrees(&params, 62, 512), degrees(&params, 62, 100));
}

// Spec: midi-random-pitch R11. A moved note's bend is carried in degrees, and
// a scale change while it is held doesn't move it.
#[test]
fn a_moved_note_s_bend_is_carried_in_degrees() {
    let params = [
        (USE_SCALE, 1.0),
        (CHANCE, 100.0),
        (CHOICES, 1.0),
        (INTERVAL, 2.0),
        (SIGN, 0.0),
    ];
    let device = device(&bl_devices::dice_type(), &params);
    let mut dice = node(&registry(), &device, 256);
    // C3 goes two degrees up to E3; a bend of two semitones, a degree up
    // from C3, becomes a degree up from E3: one semitone.
    let notes = [on(0, 60, 1), bend(5, 60, 1, 2.0 / 48.0)];
    let sent = scaled_through(dice.as_mut(), &notes, 256, ScaleRuns::of(c_major()));
    assert_eq!(keys_on(&sent), [64]);
    let carried = bends(&sent);
    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0].0, 64);
    assert!((carried[0].1 - 1.0 / 48.0).abs() < 1e-6, "{carried:?}");
    // A scale change before the note-off leaves it on E3.
    let mut scale = ScaleRuns::of(c_major());
    scale.push(100, Scale::new(1, ScaleName::Minor));
    let sent = scaled_through(
        node(&registry(), &device, 256).as_mut(),
        &[on(0, 60, 1), off(200, 60, 1)],
        256,
        scale,
    );
    assert_eq!(sent.iter().map(|n| n.key).collect::<Vec<_>>(), [64, 64]);
}
