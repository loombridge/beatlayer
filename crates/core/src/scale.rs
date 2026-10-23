//! The arithmetic that moves a key by scale degrees (the scales spec, R2),
//! over a root and a scale's steps, so the model's `Scale` and the MIDI
//! Tools share one copy.

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
    let notes = extended_notes(root, steps);
    let below = notes
        .iter()
        .rposition(|note| *note <= i32::from(key))
        .expect("the extended scale reaches below the keyboard");
    let target = below.checked_add_signed(isize::try_from(degrees).ok()?)?;
    let moved = notes.get(target)?;
    let key = i32::from(key) + (moved - notes[below]);
    // A move that leaves the keyboard is off it (the scales spec, R2).
    if !(0..=127).contains(&key) {
        return None;
    }
    Some(key as u8)
}

/// The scale note nearest `key`: the one fewest semitones away, the lower
/// of two equally near.
#[must_use]
pub fn nearest(root: u8, steps: &[u8], key: u8) -> u8 {
    let key = i32::from(key);
    extended_notes(root, steps)
        .into_iter()
        .filter(|note| (0..=127).contains(note))
        .min_by_key(|note| ((note - key).abs(), *note))
        .unwrap_or(0) as u8
}

/// The scale's notes from below the keyboard to above it, in order, so a
/// move that leaves the scale's range still has a note to land on and can be
/// refused for leaving the keyboard.
fn extended_notes(root: u8, steps: &[u8]) -> Vec<i32> {
    let root = i32::from(root % 12);
    (-36i32..=156)
        .filter(|key| {
            let step = ((*key).rem_euclid(12) - root).rem_euclid(12);
            steps.contains(&(step as u8))
        })
        .collect()
}
