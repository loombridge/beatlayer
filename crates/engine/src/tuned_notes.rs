//! A tuning played through per-note pitch bend (WP-6.9b; the tunings spec,
//! R20 and R21).
//!
//! A plug-in with MPE Mode and its tuning switch on gets each note on the
//! twelve-step key nearest the note's tuned pitch, with a per-note bend
//! carrying the rest; the plug-in formats' nodes run a block's notes through
//! [`TunedNotes`] before they lay them out. The rewritten notes go in a
//! buffer and the held notes in a table, both made with the node, so nothing
//! on the audio thread allocates; a note past their room is passed on as it
//! came.

use crate::node::{NoteBuffer, NoteEvent, NoteExpression, NoteKind};
use crate::tuned::KeyPitches;

/// The most notes held at once that are retuned.
const HELD: usize = 256;

/// The notes a block can hold once retuned: its own, and a bend after each
/// note-on.
const BLOCK_NOTES: usize = 2048;

/// The per-note bend range the plug-in formats are set to, in semitones.
const BEND_RANGE: f64 = 48.0;

/// A note that was sent on another key.
#[derive(Clone, Copy, Debug)]
struct Held {
    channel: u8,
    key: u8,
    id: Option<u32>,
    /// The key it was sent on, or none while the tuning has no pitch for
    /// the key and the note plays nothing.
    to: Option<u8>,
}

/// Retunes a block's notes into nearest keys and per-note bends.
#[derive(Debug)]
pub struct TunedNotes {
    keys: KeyPitches,
    out: NoteBuffer,
    held: Vec<Held>,
}

impl Default for TunedNotes {
    fn default() -> Self {
        Self::new()
    }
}

impl TunedNotes {
    /// A rewriter that passes notes on as they are, with room made.
    #[must_use]
    pub fn new() -> Self {
        Self {
            keys: KeyPitches::twelve(),
            out: NoteBuffer::with_capacity(BLOCK_NOTES),
            held: Vec::with_capacity(HELD),
        }
    }

    /// Sets the pitches the next notes are retuned to; twelve equal steps
    /// pass them on as they are. A note already sounding keeps its key.
    pub fn set(&mut self, keys: KeyPitches) {
        self.keys = keys;
    }

    /// The bend that carries `pitch`, in semitones from twelve-step key
    /// `to`, as a fraction of the bend range.
    fn bend(pitch: f64, to: u8) -> f32 {
        ((pitch - f64::from(to)) / BEND_RANGE).clamp(-1.0, 1.0) as f32
    }

    fn find(&self, note: &NoteEvent) -> Option<usize> {
        self.held.iter().position(|held| {
            held.channel == note.channel
                && held.key == note.key
                && (note.note_id.is_none() || held.id == note.note_id)
        })
    }

    /// `notes`, retuned. With no tuning and no note held on another key, the
    /// notes as they are.
    pub fn retune<'a>(&'a mut self, notes: &'a [NoteEvent]) -> &'a [NoteEvent] {
        if !self.keys.is_tuned() && self.held.is_empty() {
            return notes;
        }
        self.out.clear();
        for note in notes {
            self.one(note);
        }
        self.out.notes()
    }

    fn one(&mut self, note: &NoteEvent) {
        match note.kind {
            NoteKind::On => self.on(note),
            NoteKind::Off | NoteKind::Choke => match self.find(note) {
                Some(at) => {
                    let held = self.held.swap_remove(at);
                    if let Some(to) = held.to {
                        self.out.push(NoteEvent { key: to, ..*note });
                    }
                }
                None => self.out.push(*note),
            },
            NoteKind::Expression { expression, value } => {
                let Some(at) = self.find(note) else {
                    return self.out.push(*note);
                };
                let Held { key, to, .. } = self.held[at];
                let Some(to) = to else { return };
                let value = match (expression, self.keys.is_tuned()) {
                    // Spec: tunings R13. A bend of b semitones is b steps of
                    // the tuning.
                    (NoteExpression::Bend, true) => self
                        .keys
                        .bent(key, f64::from(value) * BEND_RANGE)
                        .map_or(value, |pitch| Self::bend(pitch, to)),
                    _ => value,
                };
                self.out.push(NoteEvent {
                    key: to,
                    kind: NoteKind::Expression { expression, value },
                    ..*note
                });
            }
            _ => self.out.push(*note),
        }
    }

    fn on(&mut self, note: &NoteEvent) {
        let Some(pitch) = self.keys.is_tuned().then(|| self.keys.pitch(note.key)) else {
            return self.out.push(*note);
        };
        if self.held.len() == HELD || self.out.notes().len() + 2 > BLOCK_NOTES {
            return self.out.push(*note);
        }
        let held = |to| Held {
            channel: note.channel,
            key: note.key,
            id: note.note_id,
            to,
        };
        // A note on a key the same note holds is a new one.
        if let Some(at) = self.find(note) {
            self.held.swap_remove(at);
        }
        let Some(pitch) = pitch else {
            // Spec: tunings R8. The tuning has no pitch for the key.
            self.held.push(held(None));
            return;
        };
        let to = pitch.round().clamp(0.0, 127.0) as u8;
        self.held.push(held(Some(to)));
        self.out.push(NoteEvent { key: to, ..*note });
        let bend = Self::bend(pitch, to);
        if bend != 0.0 {
            self.out.push(NoteEvent {
                key: to,
                kind: NoteKind::Expression {
                    expression: NoteExpression::Bend,
                    value: bend,
                },
                ..*note
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_model::Tuning;

    fn equal(steps: usize) -> KeyPitches {
        let cents = (1..=steps)
            .map(|i| 1200.0 * i as f64 / steps as f64)
            .collect();
        let tuning = Tuning::new("equal".into(), String::new(), cents).unwrap();
        KeyPitches::of(Some(&tuning))
    }

    fn note(kind: NoteKind, key: u8) -> NoteEvent {
        NoteEvent {
            offset: 3,
            kind,
            channel: 0,
            key,
            velocity: 0.8,
            note_id: Some(7),
        }
    }

    // Spec: tunings R20
    #[test]
    fn a_note_goes_on_the_nearest_key_with_a_bend_for_the_rest() {
        let mut tuned = TunedNotes::new();
        tuned.set(equal(19));
        let on = note(NoteKind::On, 61);
        let sent = tuned.retune(&[on, note(NoteKind::Off, 61)]).to_vec();
        // Key 61 is 19-EDO step 1 above key 60's pitch, 1200/19 cents up.
        let pitch = equal(19).pitch(61).unwrap();
        let to = pitch.round() as u8;
        assert_eq!(sent.len(), 3);
        assert_eq!(
            (sent[0].key, sent[0].kind, sent[0].offset),
            (to, NoteKind::On, 3)
        );
        let NoteKind::Expression { expression, value } = sent[1].kind else {
            panic!("a bend follows the note-on");
        };
        assert_eq!(expression, NoteExpression::Bend);
        assert!((f64::from(value) * 48.0 - (pitch - f64::from(to))).abs() < 1e-5);
        assert_eq!((sent[2].key, sent[2].kind), (to, NoteKind::Off));
    }

    // Spec: tunings R13
    #[test]
    fn a_clips_bend_moves_the_note_by_steps_of_the_tuning() {
        let mut tuned = TunedNotes::new();
        let keys = equal(19);
        tuned.set(keys);
        let one_step = note(
            NoteKind::Expression {
                expression: NoteExpression::Bend,
                value: 1.0 / 48.0,
            },
            62,
        );
        let sent = tuned.retune(&[note(NoteKind::On, 62), one_step]).to_vec();
        let to = keys.pitch(62).unwrap().round() as u8;
        let NoteKind::Expression { value, .. } = sent.last().unwrap().kind else {
            panic!("the bend is passed on");
        };
        let wanted = keys.pitch(63).unwrap() - f64::from(to);
        assert!((f64::from(value) * 48.0 - wanted).abs() < 1e-5);
    }

    // Spec: tunings R21
    #[test]
    fn twelve_equal_steps_pass_notes_on_as_they_are() {
        let mut tuned = TunedNotes::new();
        let notes = [note(NoteKind::On, 61), note(NoteKind::Off, 61)];
        assert_eq!(tuned.retune(&notes), notes);
    }

    // Spec: tunings R18
    #[test]
    fn a_note_sounding_keeps_its_key_when_the_tuning_goes() {
        let mut tuned = TunedNotes::new();
        tuned.set(equal(19));
        let sent = tuned.retune(&[note(NoteKind::On, 61)]).to_vec();
        tuned.set(KeyPitches::twelve());
        let off = tuned.retune(&[note(NoteKind::Off, 61)]).to_vec();
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].key, sent[0].key);
        assert_eq!(off[0].kind, NoteKind::Off);
    }

    // Spec: tunings R8
    #[test]
    fn a_key_the_tuning_has_no_pitch_for_plays_nothing() {
        let mut tuned = TunedNotes::new();
        let full = equal(19);
        let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
        let tuning = Tuning::new("equal".into(), String::new(), cents).unwrap();
        let short = tuning
            .with_range(tuning.key_note(0), tuning.key_note(99))
            .unwrap();
        let keys = KeyPitches::of(Some(&short));
        assert!(full.pitch(100).is_some() && keys.pitch(100).is_none());
        tuned.set(keys);
        let notes = [note(NoteKind::On, 100), note(NoteKind::Off, 100)];
        assert!(tuned.retune(&notes).is_empty());
    }
}
