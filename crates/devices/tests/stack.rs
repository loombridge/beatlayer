//! Stack, the chord MIDI effect (WP-5.30; the MIDI chord spec): added notes with
//! their velocity and chance, the strum with tension and crescendo, notes
//! that end with their source, and each added note's own ID. Every block is a
//! real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_devices::stack::{
    CRESCENDO, DUPLICATES, PER_NOTE, STACK, STRUM, TENSION, chance, shift, strum_position, velocity,
};
use bl_engine::{NoteExpression, NoteKind};
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};
use common::{node, note_off, note_on, notes_through, registry};

const SIZES: [usize; 4] = [1, 100, 512, 4096];

fn play(params: &[(bl_model::ParamId, f64)], size: usize) -> Vec<(i64, NoteKind, u8, f32)> {
    let mut chain = Chain::new(4096);
    chain.effect(STACK, params);
    chain.clip(4, [note(60, 0, 1)]);
    chain.sync();
    chain.play();
    chain.run(3 * BEAT_FRAMES as usize, size);
    chain.heard()
}

// Spec: midi-chord R1. Each note brings its shifts with it on the same frame,
// the note itself first, then the shifts in order; each ends with the note.
#[test]
fn a_note_brings_its_shifts_on_the_same_frame() {
    for size in SIZES {
        let heard = play(&[(shift(2), 11.0)], size);
        let ons: Vec<(i64, u8)> = heard
            .iter()
            .filter(|h| h.1 == NoteKind::On)
            .map(|h| (h.0, h.2))
            .collect();
        assert_eq!(ons, [(0, 60), (0, 64), (0, 67), (0, 71)], "size {size}");
        let offs: Vec<(i64, u8)> = heard
            .iter()
            .filter(|h| h.1 == NoteKind::Off)
            .map(|h| (h.0, h.2))
            .collect();
        assert_eq!(offs.len(), 4);
        assert!(offs.iter().all(|o| o.0 == BEAT_FRAMES));
    }
}

// Spec: midi-chord R2. A shift of 0, or one an earlier shift already has,
// adds nothing.
#[test]
fn a_repeated_or_zero_shift_adds_nothing() {
    let heard = play(&[(shift(0), 4.0), (shift(1), 4.0), (shift(2), 0.0)], 512);
    let ons: Vec<u8> = heard
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.2)
        .collect();
    assert_eq!(ons, [60, 64]);
}

// Spec: midi-chord R3. Each shift's velocity is a share of the note's.
#[test]
fn each_shift_has_its_own_velocity() {
    let heard = play(&[(velocity(0), 50.0), (velocity(1), 200.0)], 512);
    let ons: Vec<f32> = heard
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.3)
        .collect();
    assert!((ons[0] - 100.0 / 127.0).abs() < 1e-6);
    assert!((ons[1] - 50.0 / 127.0).abs() < 1e-6);
    assert!(
        (ons[2] - 1.0).abs() < 1e-6,
        "200 % of 100 is held at the top"
    );
}

// Spec: midi-chord R4. Each shift's chance is how often its note sounds.
#[test]
fn each_shift_has_its_own_chance() {
    let heard = play(&[(chance(0), 0.0)], 512);
    let ons: Vec<u8> = heard
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.2)
        .collect();
    assert_eq!(ons, [60, 67]);
}

// Spec: midi-chord R5. A strum spreads the chord over its time on exact
// frames, the note first; a negative strum plays it the other way.
#[test]
fn a_strum_spreads_the_chord_over_its_time() {
    for size in SIZES {
        let heard = play(&[(STRUM, 100.0)], size);
        let ons: Vec<(i64, u8)> = heard
            .iter()
            .filter(|h| h.1 == NoteKind::On)
            .map(|h| (h.0, h.2))
            .collect();
        // 100 ms at 48 kHz is 4,800 frames, over three notes.
        assert_eq!(ons, [(0, 60), (2400, 64), (4800, 67)], "size {size}");
        let reversed = play(&[(STRUM, -100.0)], size);
        let ons: Vec<(i64, u8)> = reversed
            .iter()
            .filter(|h| h.1 == NoteKind::On)
            .map(|h| (h.0, h.2))
            .collect();
        assert_eq!(ons, [(0, 67), (2400, 64), (4800, 60)], "size {size}");
    }
}

// Spec: midi-chord R6. Tension makes the first gaps of a strum longer
// (positive) or shorter (negative).
#[test]
fn tension_shapes_the_strum() {
    let gaps = |t| {
        let at: Vec<f64> = (0..4).map(|j| strum_position(j, 4, t)).collect();
        (at[1] - at[0], at[3] - at[2])
    };
    let (first, last) = gaps(0.8);
    assert!(first > last, "positive starts slowly");
    let (first, last) = gaps(-0.8);
    assert!(first < last, "negative starts quickly");
    let (first, last) = gaps(0.0);
    assert!((first - last).abs() < 1e-12);
    assert_eq!(strum_position(0, 4, 0.5), 0.0);
    assert_eq!(strum_position(3, 4, 0.5), 1.0);
    // Through the device: the middle note comes later with positive tension.
    let at = |t| {
        play(&[(STRUM, 200.0), (TENSION, t)], 512)
            .iter()
            .filter(|h| h.1 == NoteKind::On)
            .map(|h| h.0)
            .nth(1)
            .unwrap()
    };
    assert!(at(80.0) > at(0.0) && at(0.0) > at(-80.0));
}

// Spec: midi-chord R7. Crescendo ramps velocity along the strum.
#[test]
fn crescendo_ramps_the_velocity() {
    let up = play(&[(STRUM, 100.0), (CRESCENDO, 100.0)], 512);
    let v: Vec<f32> = up
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.3)
        .collect();
    assert!(v[0] < v[1] && v[1] < v[2], "{v:?}");
    let down = play(&[(STRUM, 100.0), (CRESCENDO, -100.0)], 512);
    let v: Vec<f32> = down
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.3)
        .collect();
    assert!(v[0] > v[1] && v[1] > v[2], "{v:?}");
}

// Spec: midi-chord R8. A note released before its strum finishes takes the
// notes that haven't sounded with it, and nothing hangs.
#[test]
fn notes_that_have_not_sounded_when_the_key_comes_up_never_do() {
    let mut chain = Chain::new(4096);
    chain.effect(STACK, &[(STRUM, 400.0)]);
    // A sixteenth long, 1,500 frames: the strum would last 19,200.
    chain.clip(
        4,
        [bl_model::Note::new(
            60,
            bl_core::time::Ticks::ZERO,
            bl_core::time::Ticks(bl_core::time::TICKS_PER_QUARTER / 16),
            100,
        )],
    );
    chain.sync();
    chain.play();
    chain.run(2 * BEAT_FRAMES as usize, 512);
    let heard = chain.heard();
    assert_eq!(heard.len(), 2, "{heard:?}");
    assert!(nothing_hangs(&heard));
}

// Spec: midi-chord R9. Without Play Duplicate Notes, a strum doesn't sound a
// key that is already sounding or waiting.
#[test]
fn a_strum_skips_a_key_already_sounding() {
    let mut chain = Chain::new(4096);
    chain.effect(STACK, &[(STRUM, 100.0)]);
    // E is played, and the chord on C would add another E.
    chain.clip(4, [note(64, 0, 2), note(60, 0, 2)]);
    chain.sync();
    chain.play();
    chain.run(3 * BEAT_FRAMES as usize, 512);
    let e = chain
        .heard()
        .iter()
        .filter(|h| h.1 == NoteKind::On && h.2 == 64)
        .count();
    assert_eq!(e, 1);
    let mut chain = Chain::new(4096);
    chain.effect(STACK, &[(STRUM, 100.0), (DUPLICATES, 1.0)]);
    chain.clip(4, [note(64, 0, 2), note(60, 0, 2)]);
    chain.sync();
    chain.play();
    chain.run(3 * BEAT_FRAMES as usize, 512);
    let e = chain
        .heard()
        .iter()
        .filter(|h| h.1 == NoteKind::On && h.2 == 64)
        .count();
    assert_eq!(e, 2);
    assert!(nothing_hangs(&chain.heard()));
}

// Spec: midi-chord R10. With Per-Note Events on, each added note has its own
// ID and takes its source's expressions; with it off, only the note does.
#[test]
fn per_note_events_give_each_added_note_its_own_id_and_expressions() {
    let registry = registry();
    for per_note in [true, false] {
        let device = common::device(
            &bl_devices::stack_type(),
            &[(PER_NOTE, if per_note { 1.0 } else { 0.0 })],
        );
        let mut stack = node(&registry, &device, 256);
        let on = note_on(0, 60, 0.8, Some(7));
        let bend = bl_engine::NoteEvent {
            offset: 10,
            kind: NoteKind::Expression {
                expression: NoteExpression::Bend,
                value: 0.25,
            },
            ..on
        };
        let sent = notes_through(stack.as_mut(), &[on, bend, note_off(20, &on, 0.5)], 256);
        let ons: Vec<_> = sent.iter().filter(|n| n.kind == NoteKind::On).collect();
        assert_eq!(ons.len(), 3);
        let bends: Vec<_> = sent
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Expression { .. }))
            .collect();
        if per_note {
            assert_eq!(ons[0].note_id, Some(7));
            assert!(ons[1].note_id.is_some() && ons[2].note_id.is_some());
            assert_ne!(ons[1].note_id, ons[2].note_id);
            assert_ne!(ons[1].note_id, Some(7));
            assert_eq!(bends.len(), 3);
            let ids: Vec<_> = bends.iter().map(|b| b.note_id).collect();
            assert!(ons.iter().all(|o| ids.contains(&o.note_id)));
        } else {
            assert!(ons[1].note_id.is_none());
            assert_eq!(bends.len(), 1);
            assert_eq!(bends[0].key, 60);
        }
        assert_eq!(sent.iter().filter(|n| n.kind == NoteKind::Off).count(), 3);
    }
}

// Spec: midi-chord R11. Two renders with Chance are bit for bit the same.
#[test]
fn two_renders_are_identical() {
    let params: Vec<_> = (0..6).map(|n| (chance(n), 50.0)).collect();
    let render = || {
        let mut chain = Chain::new(4096);
        let mut params = params.clone();
        params.extend((0..6).map(|n| (shift(n), [3.0, 4.0, 7.0, 10.0, 12.0, 14.0][n as usize])));
        chain.effect(STACK, &params);
        chain.clip(16, (0..16).map(|b| note(60, b, 1)));
        chain.sync();
        chain.play();
        chain.run(17 * BEAT_FRAMES as usize, 512);
        chain.heard()
    };
    let (first, second) = (render(), render());
    assert_eq!(first, second);
    let ons = first.iter().filter(|h| h.1 == NoteKind::On).count();
    assert!(ons > 16 && ons < 16 * 7, "{ons}");
}

// Spec: midi-chord R12. Several hundred notes in one block go through without
// allocating, and every one ends.
#[test]
fn several_hundred_notes_in_a_block_allocate_nothing() {
    let registry = registry();
    let params: Vec<_> = (0..6)
        .map(|n| (shift(n), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0][n as usize]))
        .collect();
    let device = common::device(&bl_devices::stack_type(), &params);
    let mut stack = node(&registry, &device, 4096);
    let mut notes = Vec::new();
    for i in 0..300u32 {
        let on = note_on(i % 100, (i % 100) as u8, 0.7, Some(i));
        notes.push(on);
        notes.push(note_off(200 + i % 100, &on, 0.5));
    }
    notes.sort_by_key(|n| n.offset);
    let sent = notes_through(stack.as_mut(), &notes, 4096);
    let ons = sent.iter().filter(|n| n.kind == NoteKind::On).count();
    let offs = sent.iter().filter(|n| n.kind == NoteKind::Off).count();
    // The table holds 1,024 notes: past it, notes are dropped, and their
    // note-offs pass through as they came.
    assert!(ons > 300, "{ons}");
    assert!(
        offs >= ons,
        "every note that sounded ends: {ons} on, {offs} off"
    );
}

// Spec: midi-chord R9. A strum never drops the note that came in, even when
// an earlier chord already added its key.
#[test]
fn a_strum_never_drops_the_note_that_came_in() {
    let mut chain = Chain::new(4096);
    chain.effect(STACK, &[(STRUM, 100.0)]);
    // C's chord adds an E; then E itself comes in.
    chain.clip(4, [note(60, 0, 2), note(64, 0, 2)]);
    chain.sync();
    chain.play();
    chain.run(3 * BEAT_FRAMES as usize, 512);
    let heard = chain.heard();
    let on = |k| {
        heard
            .iter()
            .filter(|h| h.1 == NoteKind::On && h.2 == k)
            .count()
    };
    let off = |k| {
        heard
            .iter()
            .filter(|h| h.1 == NoteKind::Off && h.2 == k)
            .count()
    };
    assert!(on(64) >= 1, "E came in and sounds: {heard:?}");
    assert_eq!(on(64), off(64), "and ends: {heard:?}");
    assert!(nothing_hangs(&heard));
}
