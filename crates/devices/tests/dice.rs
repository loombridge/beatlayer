//! Dice, the random-pitch MIDI effect (WP-5.30; the MIDI random pitch spec):
//! chance, choices, interval, the two modes and the three signs, seeded so a
//! render repeats. Every block is a real-time section under the allocation
//! trap.

bl_rt::install_trap!();

mod common;

use bl_devices::dice::{CHANCE, CHOICES, DICE, INTERVAL, MODE, SIGN};
use bl_engine::NoteKind;
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};

/// Eight notes of middle C, a beat each, played through Dice.
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
