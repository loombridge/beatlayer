//! Scale, the scale-map MIDI effect (WP-5.30; the MIDI scale map spec): the grid
//! from the notes that come in to the notes that go out, Base, Transpose,
//! Fold and the window. Every block is a real-time section under the
//! allocation trap.

use crate::common;
use bl_devices::remap::{BASE, FOLD, LOWEST, RANGE, REMAP, SCALE_NAME, TRANSPOSE, USE_SCALE, map};
use bl_engine::NoteKind;
use bl_engine::scales::ScaleRuns;
use bl_model::{Scale, ScaleName};
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};
use common::{device, keys_on, node, off, on, registry, scaled_through};

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

/// The chooser's entry for `name`: User is 0 and the list follows.
fn entry(name: ScaleName) -> f64 {
    1.0 + ScaleName::ALL.iter().position(|n| *n == name).unwrap() as f64
}

/// The keys the twelve notes from C3 up leave Scale as, set by `params`
/// under `scale`.
fn twelve(params: &[(bl_model::ParamId, f64)], scale: Scale) -> Vec<u8> {
    let mut remap = node(&registry(), &device(&bl_devices::remap_type(), params), 256);
    let notes: Vec<_> = (0..12).map(|k| on(k, 60 + k as u8, 1 + k)).collect();
    keys_on(&scaled_through(
        remap.as_mut(),
        &notes,
        256,
        ScaleRuns::of(scale),
    ))
}

// Spec: midi-scale-map R10. A named scale sends each note to the scale note
// nearest it, counted from Base, the lower of two: in-scale notes stay,
// out-of-scale ones move.
#[test]
fn a_named_scale_sends_each_note_to_the_nearest_scale_note() {
    let c_major = [(SCALE_NAME, entry(ScaleName::Major))];
    assert_eq!(
        twelve(&c_major, Scale::default()),
        [60, 60, 62, 62, 64, 65, 65, 67, 67, 69, 69, 71]
    );
    // Counted from Base D, C3 is between B2 and C#3 and goes to the lower.
    let d_major = [(SCALE_NAME, entry(ScaleName::Major)), (BASE, 2.0)];
    let sent = twelve(&d_major, Scale::default());
    assert_eq!(sent[0], 59);
    assert!(
        sent.iter()
            .all(|k| Scale::new(2, ScaleName::Major).holds(*k))
    );
    // User keeps its Map columns, which a named scale leaves alone.
    let mut remap = node(
        &registry(),
        &device(
            &bl_devices::remap_type(),
            &[(map(0), 5.0), (SCALE_NAME, entry(ScaleName::Major))],
        ),
        256,
    );
    let sent = scaled_through(
        remap.as_mut(),
        &[on(0, 60, 1)],
        256,
        ScaleRuns::of(Scale::default()),
    );
    assert_eq!(keys_on(&sent), [60], "the named scale, not the column");
    let mut remap = node(
        &registry(),
        &device(&bl_devices::remap_type(), &[(map(0), 5.0)]),
        256,
    );
    let sent = scaled_through(
        remap.as_mut(),
        &[on(0, 60, 1)],
        256,
        ScaleRuns::of(Scale::default()),
    );
    assert_eq!(keys_on(&sent), [65], "User: the column is back");
}

// Spec: midi-scale-map R11. Fold works only for User: with a named scale it
// acts as off.
#[test]
fn fold_acts_as_off_with_a_named_scale() {
    let named = [(SCALE_NAME, entry(ScaleName::MinorPentatonic))];
    let folded = [(SCALE_NAME, entry(ScaleName::MinorPentatonic)), (FOLD, 1.0)];
    assert_eq!(
        twelve(&named, Scale::default()),
        twelve(&folded, Scale::default())
    );
    // User still folds: column 0 sent to row 12 lands an octave above the
    // note, which Fold brings back down.
    let user = |fold| {
        let mut remap = node(
            &registry(),
            &device(&bl_devices::remap_type(), &[(map(0), 12.0), (FOLD, fold)]),
            256,
        );
        keys_on(&scaled_through(
            remap.as_mut(),
            &[on(0, 60, 1)],
            256,
            ScaleRuns::of(Scale::default()),
        ))
    };
    assert_eq!(user(0.0), [72]);
    assert_eq!(user(1.0), [60]);
}

// Spec: midi-scale-map R12. With Use Current Scale on, Base and Scale Name
// follow the device's current scale; with its Scale Mode off every note stays
// as it came, Transpose too.
#[test]
fn use_current_scale_takes_base_and_scale_name_from_the_current_scale() {
    let follow = [(USE_SCALE, 1.0), (BASE, 5.0), (SCALE_NAME, 0.0)];
    let d_dorian = Scale::new(2, ScaleName::Dorian);
    let sent = twelve(&follow, d_dorian);
    assert!(sent.iter().all(|k| d_dorian.holds(*k)), "{sent:?}");
    assert_eq!(sent[2], 62, "D3 is in D Dorian and stays");
    let transposing = [(USE_SCALE, 1.0), (TRANSPOSE, 5.0)];
    assert_eq!(
        twelve(&transposing, Scale::default()),
        (60..72).collect::<Vec<u8>>()
    );
}

// Spec: midi-scale-map R12, scales R17. A change of the current scale while a
// note is held doesn't move it.
#[test]
fn a_scale_change_while_a_note_is_held_does_not_move_it() {
    let mut remap = node(
        &registry(),
        &device(&bl_devices::remap_type(), &[(USE_SCALE, 1.0)]),
        256,
    );
    let mut scale = ScaleRuns::of(Scale::new(0, ScaleName::Major));
    scale.push(100, Scale::default());
    let sent = scaled_through(remap.as_mut(), &[on(0, 61, 1), off(200, 61, 1)], 256, scale);
    // C#3 went to C3 under C Major, and its note-off follows it there.
    assert_eq!(
        sent.iter().map(|n| (n.kind, n.key)).collect::<Vec<_>>(),
        [(NoteKind::On, 60), (NoteKind::Off, 60)]
    );
}
