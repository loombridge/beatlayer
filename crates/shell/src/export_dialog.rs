//! The Export Audio/Video dialog's Render Start and Render Length fields
//! (the export spec, R31): a position in bars, beats, and sixteenths, and a
//! length in bars, beats, and sixteenths counted from 0, each stepped and
//! typed as the Control Bar's Loop Start and Loop Length fields are
//! (control bar transport spec, R5, R8, R9). The dialog runs
//! `export.set_choices` with what these work out.

use bl_core::time::{BarBeatSixteenth, Ticks, TimeSignatureMap};

use crate::transport::length_parts;

/// One of the dialog's two fields; each has three parts, for bars, beats,
/// and sixteenths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// Render Start.
    Start,
    /// Render Length.
    Length,
}

impl Field {
    /// What the field is called.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Start => "Render Start",
            Self::Length => "Render Length",
        }
    }
}

/// The range the fields show: where it starts and how long it is, in ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    /// Render Start.
    pub start: Ticks,
    /// Render Length.
    pub length: Ticks,
}

/// The three numbers a field shows: a position is a bar, a beat, and a
/// sixteenth counted from 1; a length is counted from 0, in the time
/// signature at the range's start.
#[must_use]
pub fn parts(signatures: &TimeSignatureMap, field: Field, range: Range) -> [i64; 3] {
    match field {
        Field::Start => {
            let shown = signatures.to_bbs(range.start.max(Ticks::ZERO));
            [shown.bar, i64::from(shown.beat), i64::from(shown.sixteenth)]
        }
        Field::Length => length_parts(signatures, range.start, range.length),
    }
}

/// The length of one step of a bar, beat, or sixteenth part, in the time
/// signature at `at`.
fn unit(signatures: &TimeSignatureMap, at: Ticks, part: usize) -> Ticks {
    let signature = signatures.signature_at(at);
    match part {
        0 => signature.bar_length(),
        1 => signature.beat_length(),
        _ => Ticks::SIXTEENTH,
    }
}

/// The range after `steps` steps of a part (negative steps go down): a bar,
/// a beat, or a sixteenth. The start doesn't go before 1.1.1 and the length
/// not below 0.
#[must_use]
pub fn step(
    signatures: &TimeSignatureMap,
    field: Field,
    range: Range,
    part: usize,
    steps: i32,
) -> Range {
    let by = unit(signatures, range.start, part) * i64::from(steps);
    match field {
        Field::Start => Range {
            start: (range.start + by).max(Ticks::ZERO),
            ..range
        },
        Field::Length => Range {
            length: (range.length + by).max(Ticks::ZERO),
            ..range
        },
    }
}

/// The range after a typed value: `typed` holds what was typed into each
/// part, and a part not typed into keeps the value shown. A bar of 1 or
/// more, a beat and a sixteenth that exist in that bar, and a length of 0
/// or more. `None` if the text isn't a number.
#[must_use]
pub fn typed(
    signatures: &TimeSignatureMap,
    field: Field,
    range: Range,
    typed: &[Option<String>; 3],
) -> Option<Range> {
    let shown = parts(signatures, field, range);
    let mut whole = [0i64; 3];
    for (part, value) in whole.iter_mut().enumerate() {
        *value = match typed[part].as_deref().map(str::trim) {
            None | Some("") => shown[part],
            Some(text) => text.parse::<i64>().ok()?,
        };
    }
    match field {
        Field::Start => {
            let bar = whole[0].max(1);
            let signature = signatures.signature_at(signatures.bar_start(bar - 1));
            let beat = whole[1].clamp(1, i64::from(signature.numerator()));
            let sixteenth = whole[2].clamp(1, i64::from(signature.sixteenths_per_beat()));
            let start = signatures
                .from_bbs(BarBeatSixteenth::new(bar, beat as u32, sixteenth as u32))
                .ok()?;
            Some(Range { start, ..range })
        }
        Field::Length => {
            let signature = signatures.signature_at(range.start);
            let length = signature
                .bar_length()
                .get()
                .checked_mul(whole[0].max(0))?
                .checked_add(signature.beat_length().get().checked_mul(whole[1].max(0))?)?
                .checked_add(Ticks::SIXTEENTH.get().checked_mul(whole[2].max(0))?)?;
            Some(Range {
                length: Ticks(length),
                ..range
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fields_step_and_take_typed_values_as_the_control_bars_do() {
        // Spec: export R31.
        let signatures = TimeSignatureMap::default();
        let range = Range {
            start: Ticks::ZERO,
            length: Ticks::WHOLE * 4,
        };
        assert_eq!(parts(&signatures, Field::Start, range), [1, 1, 1]);
        assert_eq!(parts(&signatures, Field::Length, range), [4, 0, 0]);
        // A bar up from 1.1.1 is 2.1.1; a length steps by sixteenths.
        let up = step(&signatures, Field::Start, range, 0, 1);
        assert_eq!(parts(&signatures, Field::Start, up), [2, 1, 1]);
        let shorter = step(&signatures, Field::Length, range, 2, -1);
        assert_eq!(parts(&signatures, Field::Length, shorter), [3, 3, 3]);
        // Neither goes below its floor.
        assert_eq!(
            step(&signatures, Field::Start, range, 0, -3).start,
            Ticks::ZERO
        );
        assert_eq!(
            step(&signatures, Field::Length, range, 0, -9).length,
            Ticks::ZERO
        );
        // Typed: 3.2.1 for the start, and 2.0.2 for the length; a part not
        // typed keeps what is shown.
        let typed_start = typed(
            &signatures,
            Field::Start,
            range,
            &[Some("3".into()), Some("2".into()), None],
        )
        .unwrap();
        assert_eq!(parts(&signatures, Field::Start, typed_start), [3, 2, 1]);
        let typed_length = typed(
            &signatures,
            Field::Length,
            range,
            &[Some("2".into()), None, Some("2".into())],
        )
        .unwrap();
        assert_eq!(parts(&signatures, Field::Length, typed_length), [2, 0, 2]);
        assert!(
            typed(
                &signatures,
                Field::Start,
                range,
                &[Some("x".into()), None, None]
            )
            .is_none()
        );
    }
}
