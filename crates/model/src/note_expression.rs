//! A note's expression envelopes (WP-6.17; the MPE spec, R1 to R4): Pitch,
//! Slide and Pressure, each a list of breakpoints counted from the note's
//! start, as a clip envelope's are.
//!
//! Every note edit that moves, copies or reshapes a note keeps its envelopes
//! with it: the functions here are what the edits that cut or stretch a
//! note share.

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{Breakpoint, envelope_value};
use crate::clip::{MidiData, Note};

/// Which of a note's three expression envelopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Expression {
    /// The note's own pitch bend, in semitones from its key, −48 to +48.
    Pitch,
    /// The finger's movement along the key, 0 to 127.
    Slide,
    /// The note's own pressure, 0 to 127.
    Pressure,
}

impl Expression {
    /// All three, in the order the clip query lists them.
    pub const ALL: [Self; 3] = [Self::Pitch, Self::Slide, Self::Pressure];

    /// The least and the greatest value its envelope holds (MPE R1).
    #[must_use]
    pub const fn range(self) -> (f64, f64) {
        match self {
            Self::Pitch => (-48.0, 48.0),
            Self::Slide | Self::Pressure => (0.0, 127.0),
        }
    }
}

impl Note {
    /// The note's `expression` envelope.
    #[must_use]
    pub fn envelope(&self, expression: Expression) -> &[Breakpoint] {
        match expression {
            Expression::Pitch => &self.pitch,
            Expression::Slide => &self.slide,
            Expression::Pressure => &self.pressure,
        }
    }

    /// The note's `expression` envelope, to change.
    pub fn envelope_mut(&mut self, expression: Expression) -> &mut Vec<Breakpoint> {
        match expression {
            Expression::Pitch => &mut self.pitch,
            Expression::Slide => &mut self.slide,
            Expression::Pressure => &mut self.pressure,
        }
    }

    /// Whether the note carries any expression envelope.
    #[must_use]
    pub fn has_expression(&self) -> bool {
        !(self.pitch.is_empty() && self.slide.is_empty() && self.pressure.is_empty())
    }

    /// Whether each of the note's envelopes obeys MPE R1: positions from 0 to
    /// the note's length, in order with none twice, values in the envelope's
    /// range, bends from −1 to 1.
    #[must_use]
    pub fn expressions_fit(&self) -> bool {
        Expression::ALL
            .into_iter()
            .all(|expression| points_fit(self.envelope(expression), self.length, expression))
    }

    /// The note made `length` long (MPE R4): a shorter note loses the
    /// breakpoints past its new end and gets one at the end holding the value
    /// there, and a longer one lets each envelope's last value hold.
    #[must_use]
    pub fn resized(&self, length: Ticks) -> Self {
        let mut note = self.clone();
        note.set_length(length);
        note
    }

    /// Makes the note `length` long, as [`resized`](Self::resized) does.
    pub fn set_length(&mut self, length: Ticks) {
        if length < self.length {
            for expression in Expression::ALL {
                truncate(self.envelope_mut(expression), length);
            }
        }
        self.length = length;
    }
}

impl MidiData {
    /// Clears every note envelope that breaks MPE R1, leaving the note as it
    /// is (R9): what a loaded set does with one.
    pub(crate) fn drop_broken_expressions(&mut self) {
        for note in self.notes.iter_mut() {
            let length = note.length;
            for expression in Expression::ALL {
                if !points_fit(note.envelope(expression), length, expression) {
                    note.envelope_mut(expression).clear();
                }
            }
        }
    }
}

/// Whether `points` obey MPE R1 for a note `length` long.
#[must_use]
pub fn points_fit(points: &[Breakpoint], length: Ticks, expression: Expression) -> bool {
    let (low, high) = expression.range();
    points.iter().all(|point| {
        point.at >= Ticks::ZERO
            && point.at <= length
            && point.value.is_finite()
            && (low..=high).contains(&point.value)
            && (-1.0..=1.0).contains(&point.bend)
    }) && points.windows(2).all(|pair| pair[0].at < pair[1].at)
}

/// `points` cut off at `length`: those past it go, and a breakpoint at
/// `length` holds the value the envelope had there.
fn truncate(points: &mut Vec<Breakpoint>, length: Ticks) {
    let Some(value) = envelope_value(points, length.0 as f64) else {
        return;
    };
    points.retain(|point| point.at <= length);
    if points.last().is_none_or(|last| last.at < length) {
        points.push(Breakpoint {
            at: length,
            value,
            bend: 0.0,
        });
    }
}

/// The stretch of `points` from `from` to `to` (both counted from the note's
/// start), counted from `from`: a breakpoint at its start holding the value
/// there, those inside it, and one at its end holding the value there. None
/// for an envelope with no breakpoints (MPE R4, a note split or chopped).
#[must_use]
pub fn window(points: &[Breakpoint], from: Ticks, to: Ticks) -> Vec<Breakpoint> {
    let (Some(start), Some(end)) = (
        envelope_value(points, from.0 as f64),
        envelope_value(points, to.0 as f64),
    ) else {
        return Vec::new();
    };
    // The bend of the segment the part starts in carries on into the part.
    let bend = points
        .iter()
        .rev()
        .find(|point| point.at <= from)
        .map_or(0.0, |point| point.bend);
    let mut part = vec![Breakpoint {
        at: Ticks::ZERO,
        value: start,
        bend,
    }];
    part.extend(
        points
            .iter()
            .filter(|point| point.at > from && point.at < to)
            .map(|point| Breakpoint {
                at: point.at - from,
                ..*point
            }),
    );
    part.push(Breakpoint {
        at: to - from,
        value: end,
        bend: 0.0,
    });
    part
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(at: i64, value: f64) -> Breakpoint {
        Breakpoint {
            at: Ticks(at),
            value,
            bend: 0.0,
        }
    }

    // Spec: mpe R1
    #[test]
    fn an_envelope_fits_while_it_keeps_inside_the_note_and_its_range() {
        let mut note = Note::new(60, Ticks::ZERO, Ticks(480), 100);
        assert!(note.expressions_fit());
        note.pitch = vec![point(0, -48.0), point(480, 48.0)];
        note.slide = vec![point(0, 0.0), point(100, 127.0)];
        note.pressure = vec![point(240, 64.0)];
        assert!(note.expressions_fit(), "the ends of every range are in it");
        for bad in [
            |n: &mut Note| n.pitch = vec![point(0, 48.5)],
            |n: &mut Note| n.slide = vec![point(0, -1.0)],
            |n: &mut Note| n.pressure = vec![point(0, 127.5)],
            |n: &mut Note| n.pressure = vec![point(481, 1.0)],
            |n: &mut Note| n.pressure = vec![point(-1, 1.0)],
            |n: &mut Note| n.pressure = vec![point(5, 1.0), point(5, 2.0)],
            |n: &mut Note| n.pressure = vec![point(9, 1.0), point(5, 2.0)],
            |n: &mut Note| {
                n.pressure = vec![Breakpoint {
                    bend: 1.5,
                    ..point(0, 1.0)
                }]
            },
            |n: &mut Note| n.slide = vec![point(0, f64::NAN)],
        ] {
            let mut broken = note.clone();
            bad(&mut broken);
            assert!(!broken.expressions_fit());
        }
    }

    // Spec: mpe R4. A shorter note loses what is past its end and holds the
    // value it had there; a longer one changes nothing.
    #[test]
    fn a_note_made_shorter_ends_each_envelope_at_its_new_end() {
        let mut note = Note::new(60, Ticks::ZERO, Ticks(400), 100);
        note.pitch = vec![point(0, 0.0), point(200, 10.0), point(400, 20.0)];
        note.slide = vec![point(100, 50.0)];
        let shorter = note.resized(Ticks(300));
        assert_eq!(shorter.length, Ticks(300));
        assert_eq!(
            shorter.pitch,
            [point(0, 0.0), point(200, 10.0), point(300, 15.0)]
        );
        assert_eq!(shorter.slide, [point(100, 50.0), point(300, 50.0)]);
        assert!(shorter.pressure.is_empty(), "none stays none");
        assert!(shorter.expressions_fit());
        let longer = note.resized(Ticks(900));
        assert_eq!(longer.pitch, note.pitch);
        assert_eq!(longer.slide, note.slide);
        // A breakpoint exactly at the new end is already the one there.
        assert_eq!(
            note.resized(Ticks(200)).pitch,
            [point(0, 0.0), point(200, 10.0)]
        );
    }

    // Spec: mpe R4. A part has the stretch of each envelope over its span.
    #[test]
    fn a_window_has_the_stretch_over_its_span_and_the_values_at_its_ends() {
        let points = [point(0, 0.0), point(200, 20.0), point(400, 0.0)];
        assert_eq!(
            window(&points, Ticks(100), Ticks(300)),
            [point(0, 10.0), point(100, 20.0), point(200, 10.0)]
        );
        assert!(window(&[], Ticks(0), Ticks(5)).is_empty());
        assert_eq!(
            window(&[point(50, 7.0)], Ticks(100), Ticks(200)),
            [point(0, 7.0), point(100, 7.0)]
        );
    }
}
