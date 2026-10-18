//! Scale, the scale-map MIDI effect (WP-5.30; the MIDI scale map spec): the grid
//! from the notes that come in to the notes that go out, Base, Transpose,
//! Fold and the window. Every block is a real-time section under the
//! allocation trap.

bl_rt::install_trap!();

mod common;

use bl_devices::remap::{BASE, FOLD, LOWEST, RANGE, REMAP, TRANSPOSE, map};
use bl_engine::NoteKind;
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};

/// Plays one note per beat of `keys` through Scale and returns the keys of
/// the note-ons.
fn through(params: &[(bl_model::ParamId, f64)], keys: &[u8]) -> (Vec<u8>, usize) {
    let mut chain = Chain::new(4096);
    chain.effect(REMAP, params);
    chain.clip(
        2 * keys.len() as i64,
        keys.iter()
            .enumerate()
            .map(|(i, k)| note(*k, 2 * i as i64, 1)),
    );
    chain.sync();
    chain.play();
    chain.run(2 * keys.len() * BEAT_FRAMES as usize, 512);
    let heard = chain.heard();
    assert!(nothing_hangs(&heard), "{heard:?}");
    let offs = heard.iter().filter(|h| h.1 == NoteKind::Off).count();
    (
        heard
            .iter()
            .filter(|h| h.1 == NoteKind::On)
            .map(|h| h.2)
            .collect(),
        offs,
    )
}

// Spec: midi-scale-map R1. The default grid sends every note to itself.
#[test]
fn the_default_grid_changes_nothing() {
    let (keys, _) = through(&[], &[60, 61, 62, 71, 84]);
    assert_eq!(keys, [60, 61, 62, 71, 84]);
}

// Spec: midi-scale-map R2. A column sends its note to its row, in every octave.
#[test]
fn a_column_sends_its_note_to_its_row() {
    let (keys, _) = through(&[(map(0), 2.0)], &[60, 72, 48, 62]);
    assert_eq!(keys, [62, 74, 50, 62]);
}

// Spec: midi-scale-map R3. A note mapped to nothing plays nothing, and its
// note-off with it.
#[test]
fn a_removed_note_plays_nothing() {
    let (keys, offs) = through(&[(map(1), -1.0)], &[60, 61, 62]);
    assert_eq!(keys, [60, 62]);
    assert_eq!(offs, 2);
}

// Spec: midi-scale-map R4. Base moves the grid's first column to another note.
#[test]
fn base_counts_the_columns_from_another_note() {
    // With Base D, column 0 is D: it goes to two steps above D.
    let (keys, _) = through(&[(BASE, 2.0), (map(0), 2.0)], &[62, 60]);
    assert_eq!(keys, [64, 60]);
}

// Spec: midi-scale-map R5. Transpose moves every note after it is mapped.
#[test]
fn transpose_moves_the_mapped_notes() {
    let (keys, _) = through(&[(map(0), 2.0), (TRANSPOSE, 7.0)], &[60, 61]);
    assert_eq!(keys, [69, 68]);
}

// Spec: midi-scale-map R6. Fold keeps a mapped note within half an octave of
// the note that came in.
#[test]
fn fold_keeps_the_note_near_where_it_was() {
    let (plain, _) = through(&[(map(0), 11.0)], &[60]);
    assert_eq!(plain, [71]);
    let (folded, _) = through(&[(map(0), 11.0), (FOLD, 1.0)], &[60]);
    assert_eq!(folded, [59]);
}

// Spec: midi-scale-map R7. Only notes inside Lowest and Range are mapped.
#[test]
fn notes_outside_the_window_pass_through() {
    let (keys, _) = through(
        &[(map(0), 2.0), (LOWEST, 60.0), (RANGE, 11.0)],
        &[48, 60, 72],
    );
    assert_eq!(keys, [48, 62, 72]);
}
