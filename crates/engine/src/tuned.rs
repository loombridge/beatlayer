//! The pitch of every key under the set's tuning, as node data (WP-6.9; the
//! tunings spec, R12, R13 and R18).
//!
//! The compiler sends it to each native instrument on a track that plays the
//! tuning whenever the tuning, its reference pitch or range, or the track's
//! Bypass Tuning changes, so the change reaches the next notes without a new
//! program. The table is of a fixed size, so nothing on the audio thread
//! allocates; an instrument keeps a copy and reads it where it used the key.

use bl_model::{LayoutKind, Tuning, TuningLayout};

/// How many keys the table holds: the MIDI keys.
const KEYS: usize = 128;

/// The key of the tuning each incoming key plays, or none (the tunings spec,
/// R22).
pub type KeyMap = [Option<u8>; KEYS];

/// The pitch each key plays, in semitones as a twelve-step key counts them
/// from A3 = 440 Hz at key 69 (the tunings spec, R12): with no tuning, the
/// key's own number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyPitches {
    pitches: [f64; KEYS],
    /// How many keys from key 0 play; the keys above play nothing (R8).
    keys: usize,
    tuned: bool,
}

impl Default for KeyPitches {
    fn default() -> Self {
        Self::twelve()
    }
}

impl KeyPitches {
    /// Twelve equal steps: every key plays its own number (R1).
    #[must_use]
    pub fn twelve() -> Self {
        Self {
            pitches: std::array::from_fn(|key| key as f64),
            keys: KEYS,
            tuned: false,
        }
    }

    /// The pitches `tuning` gives the keys, or twelve equal steps' without
    /// one.
    #[must_use]
    pub fn of(tuning: Option<&Tuning>) -> Self {
        let Some(tuning) = tuning else {
            return Self::twelve();
        };
        let mut table = Self {
            pitches: [0.0; KEYS],
            keys: 0,
            tuned: true,
        };
        for key in 0..KEYS {
            match tuning.key_pitch(key as u8) {
                Some(pitch) if table.keys == key => {
                    table.pitches[key] = pitch;
                    table.keys += 1;
                }
                _ => break,
            }
        }
        table
    }

    /// Whether a tuning gives the pitches.
    #[must_use]
    pub fn is_tuned(&self) -> bool {
        self.tuned
    }

    /// The pitch key `key` plays, or none if it plays nothing.
    #[must_use]
    pub fn pitch(&self, key: u8) -> Option<f64> {
        let key = usize::from(key);
        (key < self.keys).then(|| self.pitches[key])
    }

    /// The key of the tuning each incoming key plays under `layout` (the
    /// tunings spec, R22, R23), or none for All Keys, which maps each key to
    /// itself. A key the tuning has none for plays nothing.
    #[must_use]
    pub fn layout_map(&self, layout: &TuningLayout) -> Option<Box<KeyMap>> {
        // The black keys' and the white keys' positions along their own
        // rows, from key 0; the centre key plays key 60.
        const BLACK: [bool; 12] = [
            false, true, false, true, false, false, true, false, true, false, true, false,
        ];
        let row = |key: usize, black: bool| {
            let per_octave = if black { 5 } else { 7 };
            (key / 12) * per_octave + (0..key % 12).filter(|&step| BLACK[step] == black).count()
        };
        let by_row = |black: bool, centre: usize, key: usize| {
            (BLACK[key % 12] == black)
                .then(|| (row(key, black) + 60).checked_sub(row(centre, black)))
                .flatten()
                .and_then(|to| u8::try_from(to).ok())
        };
        let to = |key: usize| -> Option<u8> {
            match layout.layout {
                LayoutKind::AllKeys => Some(key as u8),
                LayoutKind::BlackKeysOnly => by_row(true, 61, key),
                LayoutKind::WhiteKeysOnly => by_row(false, 60, key),
                LayoutKind::ClosestInPitch => self.nearest(key as f64).map(|(near, _)| near),
                LayoutKind::Custom => layout.custom(key as u8),
            }
        };
        (layout.layout != LayoutKind::AllKeys).then(|| {
            Box::new(std::array::from_fn(|key| {
                to(key).filter(|&to| usize::from(to) < self.keys)
            }))
        })
    }

    /// The pitch of a key moved by `steps` keys, a fraction of a key
    /// included (the tunings spec, R13): between two keys' pitches in
    /// proportion, and at the first or last key's pitch beyond them. With no
    /// tuning, the key plus `steps` semitones. None if the key plays
    /// nothing.
    #[must_use]
    pub fn bent(&self, key: u8, steps: f64) -> Option<f64> {
        let at = self.pitch(key)?;
        if !self.tuned {
            return Some(at + steps);
        }
        if steps == 0.0 {
            return Some(at);
        }
        let position = (f64::from(key) + steps).clamp(0.0, (self.keys - 1) as f64);
        let below = position.floor();
        let lower = self.pitches[below as usize];
        let upper = self.pitches[(below as usize + 1).min(self.keys - 1)];
        Some(lower + (upper - lower) * (position - below))
    }

    /// The key whose pitch is nearest `pitch` (in the same semitones), the
    /// lower on a tie, and how far above that key's pitch `pitch` is, in
    /// cents (the tunings spec, R33). None if no key plays.
    #[must_use]
    pub fn nearest(&self, pitch: f64) -> Option<(u8, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (key, own) in self.pitches[..self.keys].iter().enumerate() {
            let distance = (pitch - own).abs();
            if best.is_none_or(|(_, least)| distance < least) {
                best = Some((key, distance));
            }
        }
        let (key, _) = best?;
        Some((key as u8, (pitch - self.pitches[key]) * 100.0))
    }

    /// How far the key's tuned pitch is from its twelve-step pitch, in
    /// semitones: what an instrument adds to a voice's key-based pitch.
    /// None if the key plays nothing.
    #[must_use]
    pub fn offset(&self, key: u8) -> Option<f64> {
        self.pitch(key).map(|pitch| pitch - f64::from(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equal(steps: usize) -> Tuning {
        let cents = (1..=steps)
            .map(|i| 1200.0 * i as f64 / steps as f64)
            .collect();
        Tuning::new("equal".into(), String::new(), cents).unwrap()
    }

    #[test]
    fn without_a_tuning_a_key_plays_its_number() {
        let table = KeyPitches::of(None);
        assert!(!table.is_tuned());
        assert_eq!(table.pitch(60), Some(60.0));
        assert_eq!(table.bent(60, 2.5), Some(62.5));
        assert_eq!(table.offset(60), Some(0.0));
    }

    #[test]
    fn a_tuning_gives_each_key_its_pitch_and_silences_keys_past_the_range() {
        let tuning = equal(19);
        let table = KeyPitches::of(Some(&tuning));
        assert!(table.is_tuned());
        assert_eq!(table.pitch(61), tuning.key_pitch(61));
        let short = tuning
            .with_range(tuning.key_note(0), tuning.key_note(99))
            .unwrap();
        let table = KeyPitches::of(Some(&short));
        assert!(table.pitch(99).is_some());
        assert_eq!(table.pitch(100), None);
        assert_eq!(table.bent(100, 0.0), None);
    }

    #[test]
    fn a_bend_moves_by_steps_of_the_tuning() {
        let tuning = equal(19);
        let table = KeyPitches::of(Some(&tuning));
        let (a, b) = (table.pitch(60).unwrap(), table.pitch(61).unwrap());
        assert_eq!(table.bent(60, 1.0), Some(b));
        assert!((table.bent(60, 0.5).unwrap() - (a + b) / 2.0).abs() < 1e-12);
        assert!((table.bent(60, -1.0).unwrap() - table.pitch(59).unwrap()).abs() < 1e-12);
        // Beyond the first key the bend stops at its pitch.
        assert_eq!(table.bent(0, -3.0), table.pitch(0));
        assert_eq!(table.bent(127, 3.0), table.pitch(127));
    }

    fn layout(kind: LayoutKind) -> TuningLayout {
        TuningLayout {
            layout: kind,
            keys: Vec::new(),
        }
    }

    // Spec: tunings R22
    #[test]
    fn all_keys_needs_no_map() {
        let table = KeyPitches::of(Some(&equal(19)));
        assert!(table.layout_map(&layout(LayoutKind::AllKeys)).is_none());
    }

    // Spec: tunings R22
    #[test]
    fn black_keys_only_centres_c_sharp_3_on_key_60_and_silences_the_white_keys() {
        let table = KeyPitches::of(Some(&equal(19)));
        let map = table
            .layout_map(&layout(LayoutKind::BlackKeysOnly))
            .unwrap();
        // C#3 is key 61: key 60 of the tuning; D#3 (63) the next up; F#3
        // (66) the next; the black key below, A#2 (58), key 59.
        assert_eq!(
            (map[61], map[63], map[66], map[58]),
            (Some(60), Some(61), Some(62), Some(59))
        );
        // The white keys play nothing.
        assert_eq!((map[60], map[62], map[64]), (None, None, None));
    }

    // Spec: tunings R22
    #[test]
    fn white_keys_only_centres_c3_on_key_60_and_silences_the_black_keys() {
        let table = KeyPitches::of(Some(&equal(19)));
        let map = table
            .layout_map(&layout(LayoutKind::WhiteKeysOnly))
            .unwrap();
        assert_eq!(
            (map[60], map[62], map[64], map[65], map[59]),
            (Some(60), Some(61), Some(62), Some(63), Some(59))
        );
        assert_eq!((map[61], map[63]), (None, None));
    }

    // Spec: tunings R22
    #[test]
    fn closest_in_pitch_plays_the_key_whose_pitch_is_nearest() {
        let table = KeyPitches::of(Some(&equal(19)));
        let map = table
            .layout_map(&layout(LayoutKind::ClosestInPitch))
            .unwrap();
        assert_eq!(map[60], Some(60));
        // 19 steps to the octave put key 62's pitch 1.26 semitones above
        // key 60's, nearest to a twelve-step key 61.
        assert_eq!(map[61], Some(62));
    }

    // Spec: tunings R23
    #[test]
    fn a_custom_layout_follows_its_table_and_a_value_the_tuning_cant_hold_plays_nothing() {
        let tuning = equal(19);
        let short = tuning
            .with_range(tuning.key_note(0), tuning.key_note(99))
            .unwrap();
        let mut keys: Vec<Option<u8>> = (0..128).map(Some).collect();
        keys[61] = Some(70);
        keys[62] = Some(120);
        keys[63] = None;
        let custom = TuningLayout {
            layout: LayoutKind::Custom,
            keys,
        };
        let map = KeyPitches::of(Some(&short)).layout_map(&custom).unwrap();
        assert_eq!(
            (map[61], map[62], map[63], map[64]),
            (Some(70), None, None, Some(64))
        );
        // Keys past the tuning's range play nothing.
        assert_eq!(map[100], None);
    }
}
