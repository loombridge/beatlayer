//! Record Quantization: moving the notes a recording makes onto a grid
//! (Spec: recording R17 to R23).
//!
//! The grid is `note.quantize`'s: [`quantize_step`] lays each value's
//! spacings out from every bar's start. What differs is the tie (the earlier
//! line wins), the "and" values (two grids' lines together) and what happens
//! at a clip's edges.

use std::collections::BTreeMap;

use bl_core::time::{Ticks, TimeSignatureMap};
use bl_model::{ClipData, ClipId, Command, EditError, Editor, MidiData, Note, RecordQuantization};

use crate::arrangement::grid::lines;
use crate::clip_view::quantize_step;

/// The undo step that quantises the notes an Arrangement take added
/// (Spec: recording R19).
pub const UNDO_LABEL: &str = "Record Quantization";

/// Where a note whose nearest line is at or past the end of its span goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PastEnd {
    /// The last line inside the span: an Arrangement clip's rule (R19).
    LastLine,
    /// To this place: a looping Session clip's start (R20, R22).
    To(Ticks),
}

/// Every line of `value`'s grids from `from` to `to`, in order, with each
/// bar's start among them.
fn grid_lines(
    signatures: &TimeSignatureMap,
    value: RecordQuantization,
    from: Ticks,
    to: Ticks,
) -> Vec<Ticks> {
    let mut found = Vec::new();
    for &spacing in value.spacings() {
        let step = quantize_step(spacing, signatures.signature_at(from));
        lines(signatures, step, from, to, usize::MAX, |line, _| {
            found.push(line);
        });
    }
    found.sort_unstable();
    found.dedup();
    found
}

/// The line of `value`'s grid nearest `at`, the earlier of two as near
/// (Spec: recording R18). `None` for No Quantization.
#[must_use]
pub fn nearest_line(
    signatures: &TimeSignatureMap,
    value: RecordQuantization,
    at: Ticks,
) -> Option<Ticks> {
    let (bar, _) = signatures.bar_at(at);
    // The next bar's start is a line of every grid.
    grid_lines(
        signatures,
        value,
        signatures.bar_start(bar),
        signatures.bar_start(bar + 1),
    )
    .into_iter()
    .min_by_key(|line| ((*line - at).abs(), *line))
}

/// Where a note starting at `at` goes with `value` on, keeping inside
/// `(from, to)`: the nearest line, or when that is outside, `past_end`'s
/// place after the end and the first line inside before the start. A span
/// with no line in it leaves the note where it is.
#[must_use]
pub fn quantized_start(
    signatures: &TimeSignatureMap,
    value: RecordQuantization,
    at: Ticks,
    (from, to): (Ticks, Ticks),
    past_end: PastEnd,
) -> Ticks {
    let Some(line) = nearest_line(signatures, value, at) else {
        return at;
    };
    if line >= from && line < to {
        return line;
    }
    let inside = if line >= to {
        match past_end {
            PastEnd::To(place) => return place,
            PastEnd::LastLine => {
                let (bar, _) = signatures.bar_at(to - Ticks(1));
                grid_lines(signatures, value, signatures.bar_start(bar), to - Ticks(1)).pop()
            }
        }
    } else {
        let (bar, _) = signatures.bar_at(from);
        grid_lines(signatures, value, from, signatures.bar_start(bar + 1))
            .into_iter()
            .next()
    };
    inside
        .filter(|line| *line >= from && *line < to)
        .unwrap_or(at)
}

/// `note` with its start moved onto `value`'s grid and everything else, its
/// length, velocities and expression, kept (R18). `origin` is where the
/// note's contents begin on the bars `signatures` count, and `span` the part
/// of the contents that plays.
#[must_use]
pub fn quantized_note(
    note: Note,
    signatures: &TimeSignatureMap,
    value: RecordQuantization,
    origin: Ticks,
    span: (Ticks, Ticks),
    past_end: PastEnd,
) -> Note {
    let start = quantized_start(
        signatures,
        value,
        note.start + origin,
        (span.0 + origin, span.1 + origin),
        match past_end {
            PastEnd::To(place) => PastEnd::To(place + origin),
            last => last,
        },
    ) - origin;
    Note { start, ..note }
}

/// Quantises the notes a take added to the clips it made or merged into
/// (R19), as one undo step. `added` holds each such clip with the place in
/// its notes of the first one the take added; the notes already there stay.
/// A clip that has gone since, or isn't a MIDI clip, is passed over. No step
/// is made when no note moves.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step.
pub fn quantize_added(
    editor: &mut Editor,
    added: &BTreeMap<ClipId, usize>,
    value: RecordQuantization,
) -> Result<bool, EditError> {
    let doc = editor.document();
    let signatures = &doc.settings().time_signatures;
    let mut moved = Vec::new();
    for (&id, &first) in added {
        let Some(clip) = doc.clip(id) else { continue };
        let ClipData::Midi(data) = &clip.data else {
            continue;
        };
        let origin = clip.content_start();
        let span = (clip.start - origin, clip.end() - origin);
        let notes: bl_model::Vector<Note> = data
            .notes
            .iter()
            .enumerate()
            .map(|(place, note)| {
                if place < first {
                    note.clone()
                } else {
                    quantized_note(
                        note.clone(),
                        signatures,
                        value,
                        origin,
                        span,
                        PastEnd::LastLine,
                    )
                }
            })
            .collect();
        if notes != data.notes {
            moved.push((
                id,
                MidiData {
                    notes,
                    ..data.clone()
                },
            ));
        }
    }
    if moved.is_empty() {
        return Ok(false);
    }
    editor.transact(UNDO_LABEL, |editor| {
        for (clip, data) in moved {
            editor.apply(Command::SetClipData {
                clip,
                data: ClipData::Midi(data),
            })?;
        }
        Ok::<(), EditError>(())
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use RecordQuantization as Q;
    use bl_core::time::{TimeSignature, TimeSignatureMap};

    fn four_four() -> TimeSignatureMap {
        TimeSignatureMap::constant(TimeSignature::FOUR_FOUR)
    }

    fn seven_eight() -> TimeSignatureMap {
        TimeSignatureMap::constant(TimeSignature::new(7, 8).unwrap())
    }

    /// The first line at or after `at` and the one before it, to show the
    /// lines around a place.
    fn around(map: &TimeSignatureMap, value: Q, at: Ticks) -> Option<Ticks> {
        nearest_line(map, value, at)
    }

    // Spec: recording R18. Each value's lines fall where the grid says, from
    // each bar's start.
    #[test]
    fn every_values_lines_fall_on_its_grid_in_four_four() {
        let map = four_four();
        let sixteenth = Ticks::QUARTER.get() / 4;
        let quarter = Ticks::QUARTER.get();
        let cases = [
            (Q::Quarter, quarter, 3 * quarter),
            (Q::Eighth, quarter / 2, 3 * quarter / 2),
            (Q::EighthTriplet, quarter / 3, 4 * quarter / 3),
            (Q::Sixteenth, sixteenth, 5 * sixteenth),
            (Q::SixteenthTriplet, quarter / 6, 5 * quarter / 6),
            (Q::ThirtySecond, quarter / 8, 3 * quarter / 8),
        ];
        for (value, spacing, line) in cases {
            // A hair after a line is pulled back to it.
            assert_eq!(
                around(&map, value, Ticks(line + 3)),
                Some(Ticks(line)),
                "{value:?}"
            );
            // And a hair before.
            assert_eq!(
                around(&map, value, Ticks(line - 3)),
                Some(Ticks(line)),
                "{value:?}"
            );
            // The next bar's start is a line of every grid.
            let bar = Ticks::WHOLE.get();
            assert_eq!(
                around(&map, value, Ticks(bar - spacing / 4)),
                Some(Ticks(bar)),
                "{value:?}"
            );
        }
        assert_eq!(around(&map, Q::None, Ticks(5)), None);
    }

    // Spec: recording R18. How many lines each value lays out in one bar,
    // with the next bar's start counted: triplets as thirds, and an "and"
    // value's two grids together with the lines they share counted once.
    #[test]
    fn each_value_lays_out_its_lines_from_the_bars_start() {
        // (value, lines in a bar of 4/4, lines in a bar of 7/8)
        let table = [
            (Q::Quarter, 5, 5),
            (Q::Eighth, 9, 8),
            (Q::EighthTriplet, 13, 12),
            (Q::EighthAndTriplet, 17, 15),
            (Q::Sixteenth, 17, 15),
            (Q::SixteenthTriplet, 25, 22),
            (Q::SixteenthAndTriplet, 33, 29),
            (Q::ThirtySecond, 33, 29),
        ];
        for (value, four_four_lines, seven_eight_lines) in table {
            for (map, expected) in [
                (four_four(), four_four_lines),
                (seven_eight(), seven_eight_lines),
            ] {
                let end = map.bar_start(1);
                let found = grid_lines(&map, value, Ticks::ZERO, end);
                assert_eq!(found.len(), expected, "{value:?} to {end:?}");
                assert_eq!(found.first(), Some(&Ticks::ZERO));
                assert_eq!(found.last(), Some(&end));
            }
        }
    }

    // Spec: recording R18. A bar of 7/8 holds seven eighths, and its lines
    // count from its own start.
    #[test]
    fn lines_count_from_each_bars_start_in_seven_eight() {
        let map = seven_eight();
        let quarter = Ticks::QUARTER.get();
        let bar = 7 * quarter / 2;
        // The second bar starts at 3.5 quarters: its quarter lines are
        // there, 4.5, 5.5 and 6.5, not on the song's quarters.
        assert_eq!(
            around(&map, Q::Quarter, Ticks(bar + quarter + 5)),
            Some(Ticks(bar + quarter))
        );
        // Three quarters into the first bar, the last quarter line is at 3;
        // 3.4 quarters is nearer the next bar's start at 3.5.
        assert_eq!(
            around(&map, Q::Quarter, Ticks(34 * quarter / 10)),
            Some(Ticks(bar))
        );
        assert_eq!(
            around(&map, Q::Quarter, Ticks(31 * quarter / 10)),
            Some(Ticks(3 * quarter))
        );
    }

    // Spec: recording R18. A note midway between two lines goes to the
    // earlier.
    #[test]
    fn a_tie_goes_to_the_earlier_line() {
        let map = four_four();
        let sixteenth = Ticks::QUARTER.get() / 4;
        for value in [Q::Sixteenth, Q::Eighth] {
            let spacing = value.spacings()[0].get();
            assert_eq!(
                around(&map, value, Ticks(spacing + spacing / 2)),
                Some(Ticks(spacing)),
                "{value:?}"
            );
        }
        // Across a bar's end too.
        let bar = Ticks::WHOLE.get();
        assert_eq!(
            around(&map, Q::Sixteenth, Ticks(bar - sixteenth / 2)),
            Some(Ticks(bar - sixteenth))
        );
    }

    // Spec: recording R18. An "and" value picks the nearer of a straight and
    // a triplet line, the earlier when both are as near.
    #[test]
    fn an_and_value_takes_the_nearer_of_its_two_grids() {
        let map = four_four();
        let quarter = Ticks::QUARTER.get();
        let (sixteenth, triplet) = (quarter / 4, quarter / 6);
        // Just before the 1/16T line at 1/6 of a quarter: nearer than the
        // straight 1/16 line at 1/4.
        assert_eq!(
            around(&map, Q::SixteenthAndTriplet, Ticks(triplet + 100)),
            Some(Ticks(triplet))
        );
        // Between the triplet line at 2/6 and the straight one at 1/4,
        // nearer the straight one.
        assert_eq!(
            around(&map, Q::SixteenthAndTriplet, Ticks(sixteenth + 100)),
            Some(Ticks(sixteenth))
        );
        // Midway between 1/16 (1/4) and 1/16T (1/3): the earlier.
        let middle = (sixteenth + 2 * triplet) / 2;
        assert_eq!((middle - sixteenth) * 2, 2 * triplet - sixteenth);
        assert_eq!(
            around(&map, Q::SixteenthAndTriplet, Ticks(middle)),
            Some(Ticks(sixteenth))
        );
        // The eighth and its triplet.
        assert_eq!(
            around(&map, Q::EighthAndTriplet, Ticks(quarter / 3 + 50)),
            Some(Ticks(quarter / 3))
        );
    }

    // Spec: recording R18. The note keeps its length, velocities and
    // expression.
    #[test]
    fn a_moved_note_keeps_its_length_velocities_and_expression() {
        let map = four_four();
        let sixteenth = Ticks::QUARTER.get() / 4;
        let mut note = Note::new(60, Ticks(sixteenth + 40), Ticks(1000), 90);
        note.release_velocity = 33;
        note.pressure = vec![bl_model::Breakpoint {
            at: Ticks(10),
            value: 50.0,
            bend: 0.0,
        }];
        let moved = quantized_note(
            note.clone(),
            &map,
            Q::Sixteenth,
            Ticks::ZERO,
            (Ticks::ZERO, Ticks::WHOLE),
            PastEnd::LastLine,
        );
        assert_eq!(moved.start, Ticks(sixteenth));
        assert_eq!(
            Note {
                start: note.start,
                ..moved
            },
            note
        );
    }

    // Spec: recording R19. A note that would land at the clip's end goes to
    // the last line inside it.
    #[test]
    fn a_note_past_the_end_goes_to_the_last_line_inside() {
        let map = four_four();
        let sixteenth = Ticks::QUARTER.get() / 4;
        let end = Ticks::WHOLE;
        let at = end.get() - 10;
        assert_eq!(
            quantized_start(
                &map,
                Q::Sixteenth,
                Ticks(at),
                (Ticks::ZERO, end),
                PastEnd::LastLine
            ),
            Ticks(end.get() - sixteenth)
        );
        // R20: or to the start of the loop.
        assert_eq!(
            quantized_start(
                &map,
                Q::Sixteenth,
                Ticks(at),
                (Ticks::ZERO, end),
                PastEnd::To(Ticks::ZERO)
            ),
            Ticks::ZERO
        );
        // R19: before the clip's start goes to the first line inside.
        assert_eq!(
            quantized_start(
                &map,
                Q::Sixteenth,
                Ticks(sixteenth + 5),
                (Ticks(sixteenth + 100), end),
                PastEnd::LastLine
            ),
            Ticks(2 * sixteenth)
        );
    }
}
