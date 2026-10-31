//! Tuning systems (WP-6.9; the tunings spec, R1 to R9): the set's tuning,
//! and the arithmetic that says which pitch each key plays.
//!
//! A tuning divides a pseudo-octave into `n` degrees. Its notes are the
//! degrees in every octave; the keys 0 up to 127 of a MIDI clip lie on a
//! run of at most 128 consecutive notes, the tuning's range, so key 0 is
//! the lowest note and each key above it is the next note. The reference
//! pitch (an octave, a degree and a frequency) fixes the frequencies.
//!
//! Everything here is plain functions over a [`Tuning`], so the engine, the
//! note editor and the MIDI Tools can all use it without a UI crate.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;

/// The most keys the tuning's range holds: the MIDI keys.
pub const MAX_KEYS: u32 = 128;

/// The most degrees a tuning may divide its pseudo-octave into. A tuning
/// with more is refused, so a damaged file can't make an enormous one.
pub const MAX_DEGREES: usize = 1024;

/// The most cents a degree may lie above the unison, in either direction
/// (a hundred octaves), so no frequency overflows.
pub const MAX_CENTS: f64 = 120_000.0;

/// The frequency of the reference pitch a tuning without an `.ascl` file's
/// own has (the tunings spec, R7): C3 of twelve equal steps at A3 = 440 Hz.
pub const DEFAULT_REFERENCE_HZ: f64 = 261.626;

/// The octave of a default reference pitch (the tunings spec, R7).
pub const DEFAULT_REFERENCE_OCTAVE: i32 = 3;

/// The key a default range puts degree 0 of the reference's octave on (the
/// tunings spec, R8).
const DEFAULT_REFERENCE_KEY: i64 = 60;

/// A note of a tuning: a degree in an octave. Octave 0 starts at degree 0
/// of the unison; a note below it has a negative octave.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct TuningNote {
    /// The octave (a pseudo-octave of the tuning), negative below the first.
    pub octave: i32,
    /// The degree within it, 0 to the tuning's degrees − 1.
    pub degree: u32,
}

impl TuningNote {
    /// The note's place in the run of all notes, counting from octave 0,
    /// degree 0, in a tuning of `degrees` degrees.
    #[must_use]
    pub fn index(self, degrees: usize) -> i64 {
        i64::from(self.octave) * degrees as i64 + i64::from(self.degree)
    }

    /// The note at `index` in a tuning of `degrees` degrees, if it lies
    /// within the range of octaves a note can name.
    #[must_use]
    pub fn from_index(index: i64, degrees: usize) -> Option<Self> {
        let degrees = degrees as i64;
        let octave = i32::try_from(index.div_euclid(degrees)).ok()?;
        let degree = u32::try_from(index.rem_euclid(degrees)).ok()?;
        Some(Self { octave, degree })
    }
}

/// A tuning's reference pitch (the tunings spec, R7): a note, and the
/// frequency in Hz it sounds at, which tunes the whole set.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Reference {
    /// The reference note's octave.
    pub octave: i32,
    /// The reference note's degree.
    pub degree: u32,
    /// The reference note's frequency in Hz.
    pub frequency: f64,
}

/// A tuning's note range (the tunings spec, R8): the notes the keys play,
/// from the lowest, on key 0, to the highest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TuningRange {
    /// The note key 0 plays.
    pub lowest: TuningNote,
    /// The last note a key plays.
    pub highest: TuningNote,
}

/// The set's tuning system (the tunings spec, R1 to R9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A tuning: its `name`, `description`, the `cents` of degrees 1 to n above \
                   the unison (the last is the pseudo-octave), optional note `names` (one per \
                   degree) and `link`, its `reference` pitch (octave, degree and frequency in \
                   Hz) and its `range` of notes (the `lowest` on key 0 and the `highest`, at \
                   most 128 notes)."
)]
pub struct Tuning {
    /// The tuning's name: its file's name without the extension.
    pub name: String,
    /// The tuning's description, from its file.
    pub description: String,
    /// The pitch of degrees 1 to n above degree 0 in cents; the last is the
    /// pseudo-octave. The pitches needn't rise in order.
    pub cents: Vec<f64>,
    /// The name of each degree, 0 to n − 1, or none at all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// A link to a page about the tuning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The reference pitch.
    pub reference: Reference,
    /// The note range.
    pub range: TuningRange,
}

impl Tuning {
    /// A tuning with the default reference pitch and the default range
    /// (the tunings spec, R7 and R8): octave 3, degree 0 at 261.626 Hz, and
    /// the 128 notes that put that degree on key 60.
    ///
    /// # Errors
    ///
    /// [`ModelError`] if the tuning is one [`Tuning::check`] refuses.
    pub fn new(name: String, description: String, cents: Vec<f64>) -> Result<Self, ModelError> {
        let degrees = cents.len().max(1);
        let reference = Reference {
            octave: DEFAULT_REFERENCE_OCTAVE,
            degree: 0,
            frequency: DEFAULT_REFERENCE_HZ,
        };
        let first = i64::from(reference.octave) * degrees as i64 - DEFAULT_REFERENCE_KEY;
        let range = TuningRange {
            lowest: TuningNote::from_index(first, degrees).ok_or(ModelError::InvalidInput(
                "the tuning's range is out of bounds",
            ))?,
            highest: TuningNote::from_index(first + i64::from(MAX_KEYS) - 1, degrees).ok_or(
                ModelError::InvalidInput("the tuning's range is out of bounds"),
            )?,
        };
        let tuning = Self {
            name,
            description,
            cents,
            names: Vec::new(),
            link: None,
            reference,
            range,
        };
        tuning.check()?;
        Ok(tuning)
    }

    /// The number of degrees to the pseudo-octave.
    #[must_use]
    pub fn degrees(&self) -> usize {
        self.cents.len()
    }

    /// The pseudo-octave's size in cents.
    #[must_use]
    pub fn pseudo_octave(&self) -> f64 {
        self.cents.last().copied().unwrap_or(1200.0)
    }

    /// Degree `degree`'s pitch above degree 0, in cents (degree 0 is 0).
    fn degree_cents(&self, degree: u32) -> f64 {
        match degree {
            0 => 0.0,
            d => self.cents[d as usize - 1],
        }
    }

    /// How many keys the range holds: 1 to 128.
    #[must_use]
    pub fn key_count(&self) -> u32 {
        let n = self.degrees();
        let count = self.range.highest.index(n) - self.range.lowest.index(n) + 1;
        u32::try_from(count).unwrap_or(0)
    }

    /// The note key `key` plays, or none for a key above the range
    /// (the tunings spec, R8).
    #[must_use]
    pub fn key_note(&self, key: u8) -> Option<TuningNote> {
        if u32::from(key) >= self.key_count() {
            return None;
        }
        let n = self.degrees();
        TuningNote::from_index(self.range.lowest.index(n) + i64::from(key), n)
    }

    /// Whether the tuning hides the scales (the tunings spec, R16): it holds
    /// other than twelve notes to its pseudo-octave.
    #[must_use]
    pub fn hides_scales(&self) -> bool {
        self.degrees() != 12
    }

    /// The name of `note` as the note editor, the Tuning section and the Tuner
    /// show it (the tunings spec, R29): its degree's name from the file with
    /// its octave number, or, where the file names no degrees, its degree's
    /// number, a colon and its octave number.
    #[must_use]
    pub fn note_name(&self, note: TuningNote) -> String {
        match self.names.get(note.degree as usize) {
            Some(name) => format!("{name}{}", note.octave),
            None => format!("{}:{}", note.degree, note.octave),
        }
    }

    /// The name of degree `degree` alone, as the Note chooser lists it: its
    /// name from the file, or its number.
    #[must_use]
    pub fn degree_name(&self, degree: u32) -> String {
        self.names
            .get(degree as usize)
            .cloned()
            .unwrap_or_else(|| degree.to_string())
    }

    /// The frequency in Hz of a note (the tunings spec, R6):
    /// f × P^(o − o₀) × r(d) / r(d₀), worked in cents so the pitch is exact
    /// to the precision of the double. None where the result isn't a finite
    /// frequency above 0.
    #[must_use]
    pub fn note_frequency(&self, note: TuningNote) -> Option<f64> {
        let reference = self.reference;
        let cents = (i64::from(note.octave) - i64::from(reference.octave)) as f64
            * self.pseudo_octave()
            + self.degree_cents(note.degree)
            - self.degree_cents(reference.degree);
        let frequency = reference.frequency * (cents / 1200.0).exp2();
        (frequency.is_finite() && frequency > 0.0).then_some(frequency)
    }

    /// The frequency in Hz that key `key` plays (the tunings spec, R6 and
    /// R8), or none for a key above the range.
    #[must_use]
    pub fn key_frequency(&self, key: u8) -> Option<f64> {
        self.note_frequency(self.key_note(key)?)
    }

    /// The pitch key `key` plays, in semitones as a twelve-step key counts
    /// them from A3 = 440 Hz at key 69 (the tunings spec, R12): the number a
    /// native instrument puts where the key was. None for a key that plays
    /// nothing.
    #[must_use]
    pub fn key_pitch(&self, key: u8) -> Option<f64> {
        self.key_frequency(key).map(pitch_of)
    }

    /// The key whose pitch is nearest `pitch` (in the same semitones as
    /// [`key_pitch`](Self::key_pitch)), the lower on a tie (the tunings spec,
    /// R15). None if no key plays.
    #[must_use]
    pub fn nearest_key(&self, pitch: f64) -> Option<u8> {
        nearest(&key_pitches(Some(self)), pitch)
    }

    /// The tuning with the reference's note changed to `octave` and
    /// `degree`: its frequency becomes that note's present frequency, so
    /// nothing sounds different (the tunings spec, R7), and then to
    /// `frequency` if one is given, which moves every pitch by one ratio.
    ///
    /// # Errors
    ///
    /// [`ModelError`] for a degree the tuning doesn't have, or a frequency
    /// that isn't finite and above 0.
    pub fn with_reference(
        &self,
        octave: Option<i32>,
        degree: Option<u32>,
        frequency: Option<f64>,
    ) -> Result<Self, ModelError> {
        let mut next = self.clone();
        let note = TuningNote {
            octave: octave.unwrap_or(self.reference.octave),
            degree: degree.unwrap_or(self.reference.degree),
        };
        if note.degree as usize >= self.degrees() {
            return Err(ModelError::InvalidValue {
                what: "reference degree",
                value: f64::from(note.degree),
            });
        }
        let kept = self.note_frequency(note).ok_or(ModelError::InvalidInput(
            "the reference note has no frequency in this tuning",
        ))?;
        next.reference = Reference {
            octave: note.octave,
            degree: note.degree,
            frequency: frequency.unwrap_or(kept),
        };
        next.check()?;
        Ok(next)
    }

    /// The tuning with its range changed (the tunings spec, R8): where only
    /// one end is given, the other moves by as many notes, so the count
    /// stays; where both are, they are the range.
    ///
    /// # Errors
    ///
    /// [`ModelError`] for a range of more than 128 notes, one that starts
    /// above its end, or a note the tuning doesn't have.
    pub fn with_range(
        &self,
        lowest: Option<TuningNote>,
        highest: Option<TuningNote>,
    ) -> Result<Self, ModelError> {
        let n = self.degrees();
        let (start, end) = match (lowest, highest) {
            (Some(l), Some(h)) => (l.index(n), h.index(n)),
            (Some(l), None) => (l.index(n), l.index(n) + i64::from(self.key_count()) - 1),
            (None, Some(h)) => (h.index(n) - i64::from(self.key_count()) + 1, h.index(n)),
            (None, None) => return Ok(self.clone()),
        };
        let range = TuningRange {
            lowest: TuningNote::from_index(start, n).ok_or(ModelError::InvalidInput(
                "the tuning's range is out of bounds",
            ))?,
            highest: TuningNote::from_index(end, n).ok_or(ModelError::InvalidInput(
                "the tuning's range is out of bounds",
            ))?,
        };
        let mut next = self.clone();
        next.range = range;
        next.check()?;
        Ok(next)
    }

    /// Whether the tuning is one the document may hold: 1 to
    /// [`MAX_DEGREES`] degrees whose pitches are finite, a pseudo-octave
    /// above the unison, names (if any) one to a degree, a reference
    /// frequency above 0, and a range of 1 to 128 notes whose ends are
    /// degrees the tuning has (the tunings spec, R3, R7 and R8).
    ///
    /// # Errors
    ///
    /// [`ModelError`] naming what is wrong.
    pub fn check(&self) -> Result<(), ModelError> {
        let n = self.degrees();
        if n == 0 || n > MAX_DEGREES {
            return Err(ModelError::InvalidInput(
                "a tuning holds 1 to 1,024 pitches",
            ));
        }
        if let Some(&cents) = self
            .cents
            .iter()
            .find(|c| !c.is_finite() || c.abs() > MAX_CENTS)
        {
            return Err(ModelError::InvalidValue {
                what: "tuning pitch in cents",
                value: cents,
            });
        }
        if self.pseudo_octave() <= 0.0 {
            return Err(ModelError::InvalidInput(
                "a tuning's pseudo-octave must lie above the unison",
            ));
        }
        if !self.names.is_empty() && self.names.len() != n {
            return Err(ModelError::InvalidInput(
                "a tuning names every degree or none",
            ));
        }
        let reference = self.reference;
        if reference.degree as usize >= n {
            return Err(ModelError::InvalidValue {
                what: "reference degree",
                value: f64::from(reference.degree),
            });
        }
        if !reference.frequency.is_finite() || reference.frequency <= 0.0 {
            return Err(ModelError::InvalidValue {
                what: "reference frequency",
                value: reference.frequency,
            });
        }
        for end in [self.range.lowest, self.range.highest] {
            if end.degree as usize >= n {
                return Err(ModelError::InvalidValue {
                    what: "range degree",
                    value: f64::from(end.degree),
                });
            }
        }
        let count = self.range.highest.index(n) - self.range.lowest.index(n) + 1;
        if !(1..=i64::from(MAX_KEYS)).contains(&count) {
            return Err(ModelError::InvalidInput(
                "a tuning's range holds 1 to 128 notes, lowest first",
            ));
        }
        Ok(())
    }
}

/// A tuning as the agent API reads it back (the tunings spec, R19), by
/// `tuning.state` and the query tool's `song`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TuningReadout {
    /// The tuning's name.
    pub name: String,
    /// The tuning's description.
    pub description: String,
    /// How many notes it holds to the pseudo-octave.
    pub notes_per_octave: usize,
    /// The pitch of degrees 1 to n above the unison, in cents.
    pub pitches: Vec<f64>,
    /// The pseudo-octave's size in cents.
    pub pseudo_octave: f64,
    /// The name of each degree; empty when the tuning names none.
    pub names: Vec<String>,
    /// A link to a page about the tuning.
    pub link: Option<String>,
    /// The reference pitch.
    pub reference: Reference,
    /// The note range.
    pub range: TuningRange,
    /// How many keys play: 1 to 128.
    pub key_count: u32,
    /// The frequency in Hz of each key 0 to 127; null for a key that plays
    /// nothing.
    pub frequencies: Vec<Option<f64>>,
}

impl Tuning {
    /// The tuning as the agent API reads it back.
    #[must_use]
    pub fn readout(&self) -> TuningReadout {
        TuningReadout {
            name: self.name.clone(),
            description: self.description.clone(),
            notes_per_octave: self.degrees(),
            pitches: self.cents.clone(),
            pseudo_octave: self.pseudo_octave(),
            names: self.names.clone(),
            link: self.link.clone(),
            reference: self.reference,
            range: self.range,
            key_count: self.key_count(),
            frequencies: (0..=127u8).map(|key| self.key_frequency(key)).collect(),
        }
    }
}

/// The pitch each key plays under `tuning`, or twelve equal steps' under
/// none: None for a key that plays nothing.
#[must_use]
pub fn key_pitches(tuning: Option<&Tuning>) -> [Option<f64>; 128] {
    std::array::from_fn(|key| match tuning {
        Some(tuning) => tuning.key_pitch(key as u8),
        None => Some(key as f64),
    })
}

/// The key of `pitches` whose pitch is nearest `pitch`, the lower on a tie.
/// Two distances within a billionth of a semitone are a tie, so rounding in
/// the pitches doesn't pick the higher key.
fn nearest(pitches: &[Option<f64>; 128], pitch: f64) -> Option<u8> {
    let mut best: Option<(u8, f64)> = None;
    for (key, p) in pitches.iter().enumerate() {
        if let Some(p) = p {
            let distance = (p - pitch).abs();
            if best.is_none_or(|(_, d)| distance < d - 1e-9) {
                best = Some((key as u8, distance));
            }
        }
    }
    best.map(|(key, _)| key)
}

/// Moves each note from the key it had under `from` to the key nearest its
/// pitch under `to` (the tunings spec, R15). A note whose key played nothing
/// before, or when no key plays now, stays. Where moved notes overlap on one
/// key, the earlier is shortened to end where the later starts, and of two
/// that start together the shorter is deleted (the longer, or the first
/// when they are alike, stays).
pub(crate) fn retune(
    notes: &mut imbl::Vector<crate::Note>,
    from: &[Option<f64>; 128],
    to: &[Option<f64>; 128],
) {
    let mut moved = vec![false; notes.len()];
    let mut all: Vec<crate::Note> = notes.iter().cloned().collect();
    for (note, moved) in all.iter_mut().zip(&mut moved) {
        if let Some(key) = from[usize::from(note.key)].and_then(|p| nearest(to, p))
            && key != note.key
        {
            note.key = key;
            *moved = true;
        }
    }
    let mut order: Vec<usize> = (0..all.len()).collect();
    order.sort_by_key(|&i| (all[i].key, all[i].start));
    let mut dead = vec![false; all.len()];
    // The notes still alive on the key at hand, in the order above.
    let mut kept: Vec<usize> = Vec::new();
    for i in order {
        if kept.last().is_some_and(|&a| all[a].key != all[i].key) {
            kept.clear();
        }
        while let Some(&a) = kept.last() {
            let overlap = all[a].start + all[a].length > all[i].start;
            if !overlap || !(moved[a] || moved[i]) {
                break;
            }
            if all[a].start == all[i].start {
                if all[i].length > all[a].length {
                    dead[a] = true;
                    kept.pop();
                    continue;
                }
                dead[i] = true;
            } else {
                let length = all[i].start - all[a].start;
                all[a].set_length(length);
            }
            break;
        }
        if !dead[i] {
            kept.push(i);
        }
    }
    *notes = all
        .into_iter()
        .zip(dead)
        .filter(|(_, dead)| !dead)
        .map(|(note, _)| note)
        .collect();
}

/// A frequency in Hz as the pitch a twelve-step key counts: semitones from
/// A3 = 440 Hz at key 69.
#[must_use]
pub fn pitch_of(frequency: f64) -> f64 {
    69.0 + 12.0 * (frequency / 440.0).log2()
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

    // Octaves at the ends of i32 neither overflow nor sound.
    #[test]
    fn an_octave_at_the_end_of_i32_has_no_frequency() {
        let tuning = equal(12);
        assert!(tuning.with_reference(Some(i32::MIN), None, None).is_err());
        let low = TuningNote {
            octave: i32::MIN,
            degree: 0,
        };
        let ranged = tuning.with_range(Some(low), None).unwrap();
        assert!((0..128u8).all(|key| ranged.key_frequency(key).is_none()));
        let moved = tuning.with_reference(Some(-5), None, None).unwrap();
        let high = TuningNote {
            octave: i32::MAX,
            degree: 0,
        };
        assert_eq!(moved.note_frequency(high), None);
    }

    #[test]
    fn twelve_equal_steps_play_the_keys_they_always_did() {
        let tuning = equal(12);
        for key in 0..=127u8 {
            let pitch = tuning.key_pitch(key).unwrap();
            // The default reference is 0.003 cent from A3 = 440 Hz's.
            assert!((pitch - f64::from(key)).abs() < 0.0001, "{key}: {pitch}");
        }
    }

    #[test]
    fn the_default_range_puts_degree_0_of_the_reference_octave_on_key_60() {
        for steps in [5, 19, 31] {
            let tuning = equal(steps);
            assert_eq!(tuning.key_count(), 128);
            assert_eq!(
                tuning.key_note(60),
                Some(TuningNote {
                    octave: 3,
                    degree: 0
                })
            );
            assert!((tuning.key_frequency(60).unwrap() - 261.626).abs() < 1e-9);
        }
    }

    #[test]
    fn nineteen_steps_follow_r6() {
        let tuning = equal(19);
        // Key 61 is degree 1 of octave 3: one step above the reference.
        let want = 261.626 * 2f64.powf(1.0 / 19.0);
        assert!((tuning.key_frequency(61).unwrap() - want).abs() < 1e-9);
        // Key 79 is degree 0 of octave 4: the pseudo-octave above.
        assert!((tuning.key_frequency(79).unwrap() - 523.252).abs() < 1e-9);
    }

    #[test]
    fn a_new_reference_note_keeps_the_sound_and_a_new_frequency_moves_it() {
        let tuning = equal(19);
        let moved = tuning.with_reference(Some(4), Some(3), None).unwrap();
        for key in 0..128u8 {
            let a = tuning.key_frequency(key).unwrap();
            let b = moved.key_frequency(key).unwrap();
            assert!((a / b - 1.0).abs() < 1e-12, "{key}");
        }
        let higher = tuning.with_reference(None, None, Some(271.626)).unwrap();
        let ratio = 271.626 / 261.626;
        for key in 0..128u8 {
            let a = tuning.key_frequency(key).unwrap();
            let b = higher.key_frequency(key).unwrap();
            assert!((b / a - ratio).abs() < 1e-12, "{key}");
        }
        assert!(tuning.with_reference(None, Some(19), None).is_err());
        assert!(tuning.with_reference(None, None, Some(0.0)).is_err());
        assert!(tuning.with_reference(None, None, Some(f64::NAN)).is_err());
    }

    #[test]
    fn moving_one_end_of_the_range_moves_the_other_by_as_many_notes() {
        let tuning = equal(19);
        // The default range starts a step below octave 0's end: one step up
        // moves key 0 and key 127 up a step.
        let one_up = TuningNote {
            octave: -1,
            degree: 17,
        };
        let moved = tuning.with_range(Some(one_up), None).unwrap();
        assert_eq!(moved.key_count(), 128);
        assert_eq!(moved.key_note(0), Some(one_up));
        // Key 60 now plays the note one step higher than it did.
        assert_eq!(
            moved.key_note(60),
            Some(TuningNote {
                octave: 3,
                degree: 1
            })
        );
        let by_the_top = tuning.with_range(None, tuning.key_note(126)).unwrap();
        assert_eq!(by_the_top.key_count(), 128);
        assert_eq!(by_the_top.key_note(127), tuning.key_note(126));
    }

    #[test]
    fn a_range_of_fewer_notes_silences_the_keys_above_it() {
        let tuning = equal(19);
        let short = tuning
            .with_range(tuning.key_note(40), tuning.key_note(99))
            .unwrap();
        assert_eq!(short.key_count(), 60);
        assert!(short.key_pitch(59).is_some());
        assert!(short.key_pitch(60).is_none());
        assert!(short.key_pitch(127).is_none());
    }

    #[test]
    fn a_range_of_more_than_128_notes_or_backwards_is_refused() {
        let tuning = equal(19);
        let low = tuning.key_note(0).unwrap();
        let high = TuningNote {
            octave: 100,
            degree: 0,
        };
        assert!(tuning.with_range(Some(low), Some(high)).is_err());
        assert!(tuning.with_range(Some(high), Some(low)).is_err());
    }

    #[test]
    fn steps_that_do_not_rise_in_order_use_every_key() {
        let tuning = Tuning::new("odd".into(), String::new(), vec![300.0, 100.0, 1200.0]).unwrap();
        assert_eq!(tuning.key_count(), 128);
        let up = tuning.key_pitch(61).unwrap();
        let down = tuning.key_pitch(62).unwrap();
        assert!(up > down);
        assert_eq!(tuning.nearest_key(tuning.key_pitch(64).unwrap()), Some(64));
    }

    #[test]
    fn the_nearest_key_is_the_lower_on_a_tie() {
        let tuning = equal(12);
        assert_eq!(tuning.nearest_key(60.0), Some(60));
        let a = tuning.key_pitch(60).unwrap();
        let b = tuning.key_pitch(61).unwrap();
        assert_eq!(tuning.nearest_key((a + b) / 2.0 - 1e-9), Some(60));
    }

    #[test]
    fn notes_that_land_on_one_key_out_of_order_never_get_a_negative_length() {
        use bl_core::time::Ticks;
        let q = Ticks::QUARTER;
        let from = key_pitches(None);
        // Every key moves to key 50.
        let mut to = [None; 128];
        to[50] = Some(61.0);
        // Stored out of order: the later note first.
        let mut notes: imbl::Vector<crate::Note> = [
            crate::Note::new(62, q, q * 2, 100),
            crate::Note::new(60, Ticks::ZERO, q, 100),
            crate::Note::new(61, Ticks::ZERO, q * 2, 100),
        ]
        .into_iter()
        .collect();
        retune(&mut notes, &from, &to);
        let got: Vec<_> = notes.iter().map(|n| (n.key, n.start, n.length)).collect();
        // The shorter of the two at 0 is deleted, and the longer is
        // shortened to end where the next starts.
        assert_eq!(got, [(50, q, q * 2), (50, Ticks::ZERO, q)]);
    }

    #[test]
    fn a_tuning_that_breaks_the_rules_is_refused() {
        assert!(Tuning::new("x".into(), String::new(), vec![]).is_err());
        assert!(Tuning::new("x".into(), String::new(), vec![-5.0]).is_err());
        assert!(Tuning::new("x".into(), String::new(), vec![f64::NAN]).is_err());
        assert!(Tuning::new("x".into(), String::new(), vec![0.0]).is_err());
        assert!(Tuning::new("x".into(), String::new(), vec![1e9]).is_err());
        let mut tuning = equal(5);
        tuning.names = vec!["a".into()];
        assert!(tuning.check().is_err());
    }
}
