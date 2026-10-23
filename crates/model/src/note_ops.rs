//! The note operations that change the notes themselves (WP-6.3): Split,
//! Chop, Join, Fit to Time Range and the stretch markers' mapping. Each is
//! pure, over notes and ticks, so the shell's commands only choose the
//! notes and the transaction.
//!
//! Spec: note-editing R34 (what parts keep), R36 (the chop's counts), R39
//! (Join), R43 to R46 (stretching).

use bl_core::time::Ticks;

use crate::note_expression::{Expression, window};
use crate::{Breakpoint, Note};

/// The most parts a chop makes of a note (note-editing R36).
pub const MOST_PARTS: u32 = 64;

fn end(note: &Note) -> Ticks {
    note.start + note.length
}

/// The part of `note` from `from` to `to`, which lie inside it: the note's
/// values, and the stretch of each expression envelope over the part, counted
/// from the part's start (note-editing R34; MPE R4).
fn part(note: &Note, from: Ticks, to: Ticks) -> Note {
    let into = from - note.start;
    let mut part = note.clone();
    part.start = from;
    part.length = to - from;
    for expression in Expression::ALL {
        *part.envelope_mut(expression) =
            window(note.envelope(expression), into, into + part.length);
    }
    part
}

/// `note` cut at each of `cuts` that falls strictly inside it, in parts that
/// together cover it. A note nothing cuts comes back whole.
#[must_use]
pub fn split(note: &Note, cuts: &[Ticks]) -> Vec<Note> {
    let mut inside: Vec<Ticks> = cuts
        .iter()
        .copied()
        .filter(|at| *at > note.start && *at < end(note))
        .collect();
    inside.sort_unstable();
    inside.dedup();
    if inside.is_empty() {
        return vec![note.clone()];
    }
    let mut edges = vec![note.start];
    edges.extend(inside);
    edges.push(end(note));
    edges
        .windows(2)
        .map(|edge| part(note, edge[0], edge[1]))
        .collect()
}

/// `note` cut into `parts` equal parts, at most as many as it has ticks.
#[must_use]
pub fn chop_equal(note: &Note, parts: u32) -> Vec<Note> {
    let parts = i64::from(parts).clamp(1, note.length.get().max(1));
    let cuts: Vec<Ticks> = (1..parts)
        .map(|k| note.start + Ticks(note.length.get() * k / parts))
        .collect();
    split(note, &cuts)
}

/// The notes of one pitch, earliest first, as one note: from the earliest
/// start to the latest end, with the earliest note's values, and every
/// note's expression breakpoints at their places in time, the later note's
/// where two overlap (note-editing R39; MPE R4).
///
/// # Panics
///
/// If `notes` is empty.
#[must_use]
pub fn join(notes: &[Note]) -> Note {
    let first = notes.first().expect("a note to join");
    let to = notes.iter().map(end).max().unwrap_or(end(first));
    let mut joined = first.clone();
    joined.length = to - first.start;
    for expression in Expression::ALL {
        let mut points: Vec<Breakpoint> = Vec::new();
        for note in notes {
            let offset = note.start - first.start;
            let source = note.envelope(expression);
            if !source.is_empty() {
                points.retain(|kept| kept.at < offset || kept.at > offset + note.length);
            }
            points.extend(source.iter().map(|point| Breakpoint {
                at: offset + point.at,
                ..*point
            }));
        }
        points.sort_by_key(|point| point.at);
        *joined.envelope_mut(expression) = points;
    }
    joined
}

/// A stretch's mapping of time: `from` goes to `to`, linearly, and with a
/// point, what lies either side of `point.0` goes to either side of
/// `point.1` (note-editing R43, R44). Outside `from` it carries on with the
/// slope of the nearest piece.
#[derive(Clone, Copy, Debug)]
pub struct Mapping {
    pieces: [(Ticks, Ticks, Ticks, Ticks); 2],
    split: Ticks,
}

impl Mapping {
    /// The mapping from `from` to `to`, with `point` moved inside it.
    ///
    /// # Errors
    ///
    /// A message when `from` is empty or backwards, or `point` isn't
    /// strictly inside it.
    pub fn new(
        from: (Ticks, Ticks),
        to: (Ticks, Ticks),
        point: Option<(Ticks, Ticks)>,
    ) -> Result<Self, &'static str> {
        if from.0 >= from.1 {
            return Err("the range to stretch must be longer than a tick");
        }
        let (p, q) = match point {
            Some((p, q)) if p > from.0 && p < from.1 => (p, q),
            Some(_) => return Err("the point must be inside the range to stretch"),
            None => (from.1, to.1),
        };
        Ok(Self {
            pieces: [(from.0, p, to.0, q), (p, from.1, q, to.1)],
            split: p,
        })
    }

    /// Where `at` goes.
    #[must_use]
    pub fn at(&self, at: Ticks) -> Ticks {
        let (x0, x1, y0, y1) = if at <= self.split || self.pieces[1].0 == self.pieces[1].1 {
            self.pieces[0]
        } else {
            self.pieces[1]
        };
        let (run, rise, along) = (
            i128::from((x1 - x0).get()),
            i128::from((y1 - y0).get()),
            i128::from((at - x0).get()),
        );
        // Rounded to the nearest tick, halves up.
        let scaled = (2 * along * rise + run).div_euclid(2 * run);
        Ticks(
            y0.get()
                .saturating_add(i64::try_from(scaled).unwrap_or(i64::MAX)),
        )
    }
}

/// `note` with its start and end carried through `map`, which may reverse
/// time, and its expression envelopes with them (MPE R4).
///
/// # Errors
///
/// A message when the note would be shorter than a tick or start before the
/// clip's contents (note-editing R45).
pub fn remap(note: &Note, map: impl Fn(Ticks) -> Ticks) -> Result<Note, String> {
    let (a, b) = (map(note.start), map(end(note)));
    let (from, to) = (a.min(b), a.max(b));
    if to - from < Ticks(1) {
        return Err("a note would be shorter than a tick".to_owned());
    }
    if from.is_negative() {
        return Err(format!(
            "a note would start at {from}, before the clip's contents begin"
        ));
    }
    let mut mapped = note.clone();
    mapped.start = from;
    mapped.length = to - from;
    for expression in Expression::ALL {
        let mut points: Vec<Breakpoint> = note
            .envelope(expression)
            .iter()
            .map(|point| Breakpoint {
                at: map(note.start + point.at),
                ..*point
            })
            .collect();
        points.sort_by_key(|point| point.at);
        if b < a {
            // Time runs backwards, so a segment's bend bows the other way
            // and belongs to the breakpoint that now starts it.
            let bends: Vec<f64> = points.iter().map(|point| point.bend).collect();
            let last = points.len().saturating_sub(1);
            for (i, point) in points.iter_mut().enumerate() {
                point.bend = if i < last { -bends[i + 1] } else { 0.0 };
            }
        }
        let mut kept: Vec<Breakpoint> = Vec::with_capacity(points.len());
        for point in points {
            if point.at < from || point.at > to {
                continue;
            }
            let point = Breakpoint {
                at: point.at - from,
                ..point
            };
            match kept.last_mut() {
                Some(last) if last.at == point.at => *last = point,
                _ => kept.push(point),
            }
        }
        *mapped.envelope_mut(expression) = kept;
    }
    Ok(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(at: i64, value: u8) -> Breakpoint {
        Breakpoint {
            at: Ticks(at),
            value: f64::from(value),
            bend: 0.0,
        }
    }

    fn note(start: i64, length: i64) -> Note {
        Note::new(60, Ticks(start), Ticks(length), 100)
    }

    // Spec: note-editing R34; mpe R4. Each part has the stretch of the
    // envelope over its span, with the value there at both its ends.
    #[test]
    fn a_split_covers_the_note_and_puts_pressure_in_its_part() {
        let held = Note {
            pressure: vec![point(0, 10), point(300, 20), point(700, 30)],
            release_velocity: 40,
            ..note(1000, 1000)
        };
        let parts = split(&held, &[Ticks(1500), Ticks(1500), Ticks(1000), Ticks(2000)]);
        assert_eq!(parts.len(), 2, "cuts at the ends and repeats split nothing");
        assert_eq!((parts[0].start, parts[0].length), (Ticks(1000), Ticks(500)));
        assert_eq!((parts[1].start, parts[1].length), (Ticks(1500), Ticks(500)));
        assert_eq!(
            parts[0].pressure,
            [point(0, 10), point(300, 20), point(500, 25)]
        );
        assert_eq!(
            parts[1].pressure,
            [point(0, 25), point(200, 30), point(500, 30)],
            "counted from the part"
        );
        assert!(parts.iter().all(|part| part.release_velocity == 40));
    }

    // Spec: note-editing R36.
    #[test]
    fn equal_parts_cover_the_note() {
        let parts = chop_equal(&note(0, 1000), 3);
        let spans: Vec<(i64, i64)> = parts
            .iter()
            .map(|part| (part.start.get(), part.length.get()))
            .collect();
        assert_eq!(spans, [(0, 333), (333, 333), (666, 334)]);
        assert_eq!(chop_equal(&note(0, 5), 64).len(), 5, "a part is a tick");
    }

    // Spec: note-editing R39; mpe R4.
    #[test]
    fn a_join_keeps_the_earliest_values_and_every_pressure_point() {
        let a = Note {
            velocity: 90,
            pressure: vec![point(0, 1), point(100, 2), point(200, 4)],
            ..note(0, 300)
        };
        let b = Note {
            velocity: 50,
            pressure: vec![point(0, 3)],
            ..note(100, 500)
        };
        let joined = join(&[a, b]);
        assert_eq!((joined.start, joined.length), (Ticks(0), Ticks(600)));
        assert_eq!(joined.velocity, 90);
        assert_eq!(
            joined.pressure,
            [point(0, 1), point(100, 3)],
            "where they overlap the later note's points win"
        );
    }

    // Spec: mpe R1, R4. Two notes whose envelopes both end on one tick join
    // with one breakpoint there, the later note's.
    #[test]
    fn a_join_keeps_one_breakpoint_where_two_ends_meet() {
        let a = Note {
            pressure: vec![point(0, 1), point(1000, 5)],
            ..note(0, 1000)
        };
        let b = Note {
            pressure: vec![point(0, 2), point(800, 9)],
            ..note(200, 800)
        };
        let joined = join(&[a, b]);
        assert_eq!(
            joined.pressure,
            [point(0, 1), point(200, 2), point(1000, 9)]
        );
        assert!(joined.expressions_fit());
    }

    // Spec: note-editing R43, R44, R45.
    #[test]
    fn a_mapping_scales_pivots_and_mirrors() {
        let by = Mapping::new((Ticks(0), Ticks(100)), (Ticks(0), Ticks(200)), None).unwrap();
        assert_eq!(by.at(Ticks(50)), Ticks(100));
        let pivot = Mapping::new(
            (Ticks(0), Ticks(100)),
            (Ticks(0), Ticks(100)),
            Some((Ticks(50), Ticks(80))),
        )
        .unwrap();
        assert_eq!(pivot.at(Ticks(25)), Ticks(40));
        assert_eq!(pivot.at(Ticks(75)), Ticks(90));
        let mirror = Mapping::new((Ticks(0), Ticks(100)), (Ticks(100), Ticks(0)), None).unwrap();
        let mirrored = remap(&note(10, 20), |at| mirror.at(at)).unwrap();
        assert_eq!((mirrored.start, mirrored.length), (Ticks(70), Ticks(20)));
        assert!(
            Mapping::new((Ticks(0), Ticks(100)), (Ticks(0), Ticks(0)), None)
                .map(|map| remap(&note(10, 20), |at| map.at(at)))
                .unwrap()
                .is_err(),
            "a note can't shrink to nothing"
        );
        assert!(Mapping::new((Ticks(5), Ticks(5)), (Ticks(0), Ticks(1)), None).is_err());
    }
}
