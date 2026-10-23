//! The arithmetic that moves a key by scale degrees (the scales spec, R2),
//! over a root and a scale's steps, so the model's `Scale` and the MIDI
//! Tools share one copy. It allocates nothing but [`notes`], so a device can
//! call the rest on the audio thread.

/// The chromatic scale's steps: every semitone, so a degree of it is a
/// semitone (the scales spec, R2).
pub const CHROMATIC: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

/// Whether the scale of `root` and `steps` holds `key`.
#[must_use]
pub fn holds(root: u8, steps: &[u8], key: u8) -> bool {
    let step = (key % 12 + 12 - root % 12) % 12;
    steps.contains(&step)
}

/// The scale's notes from key 0 to key 127, in order: its degrees.
#[must_use]
pub fn notes(root: u8, steps: &[u8]) -> Vec<u8> {
    (0..=127u8).filter(|key| holds(root, steps, *key)).collect()
}

/// The key `degrees` degrees from `key`: a key in the scale lands on the
/// scale note that many degrees above or below it, and a key outside it
/// moves by as many semitones as the scale note below it does, so it keeps
/// its distance above that note. `None` when the move would leave the
/// keyboard.
#[must_use]
pub fn move_by(root: u8, steps: &[u8], key: u8, degrees: i32) -> Option<u8> {
    u8::try_from(move_off(root, steps, key, degrees))
        .ok()
        .filter(|key| *key <= 127)
}

/// As [`move_by`], but the key a move off the keyboard would reach, below 0
/// or above 127, instead of `None`.
#[must_use]
pub fn move_off(root: u8, steps: &[u8], key: u8, degrees: i32) -> i32 {
    let below = index_at_or_below(root, steps, i32::from(key));
    let moved = i64::from(key) + note_at(root, steps, below + i64::from(degrees))
        - note_at(root, steps, below);
    saturate(moved)
}

/// The scale note nearest `key`: the one fewest semitones away, the lower
/// of two equally near.
#[must_use]
pub fn nearest(root: u8, steps: &[u8], key: u8) -> u8 {
    let below = index_at_or_below(root, steps, i32::from(key));
    let (lower, upper) = (note_at(root, steps, below), note_at(root, steps, below + 1));
    let key = i64::from(key);
    let picked = if lower < 0 || (upper <= 127 && upper - key < key - lower) {
        upper
    } else {
        lower
    };
    picked.clamp(0, 127) as u8
}

/// The note of the scale's `index`th place: indexes run over all the
/// integers, 0 the root in the octave of key 0 to 11, and the notes rise
/// with the index, below the keyboard and above it.
#[must_use]
pub fn note_at(root: u8, steps: &[u8], index: i64) -> i64 {
    let count = steps.len() as i64;
    i64::from(root % 12)
        + 12 * index.div_euclid(count)
        + i64::from(steps[index.rem_euclid(count) as usize])
}

/// The index of the highest scale note at or below `key`.
#[must_use]
pub fn index_at_or_below(root: u8, steps: &[u8], key: i32) -> i64 {
    let above = key - i32::from(root % 12);
    let (octave, within) = (above.div_euclid(12), above.rem_euclid(12));
    // Every list starts at the root, so a step is always found.
    let place = steps
        .iter()
        .rposition(|step| i32::from(*step) <= within)
        .unwrap_or(0);
    i64::from(octave) * steps.len() as i64 + place as i64
}

/// `value` as an `i32`, held at its ends.
#[must_use]
pub fn saturate(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}
