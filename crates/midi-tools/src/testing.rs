//! One transformation and one generator for the framework's tests
//! (WP-6.11); the shell's, the interface's and the agent's tests use them
//! through the `testing` feature.

use bl_core::time::Ticks;

use crate::descriptor::{
    ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind, division_ticks,
};
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::random::Rng;
use crate::registry::{MidiTool, ToolError};

/// The test tools.
pub(crate) fn tools() -> Vec<Box<dyn MidiTool>> {
    vec![Box::new(Transpose::new()), Box::new(Fill::new())]
}

fn whole(values: &[ParamValue], at: usize) -> i64 {
    match values.get(at) {
        Some(ParamValue::Whole(n)) => *n,
        _ => 0,
    }
}

/// `test_transpose`: moves every input note by `degrees` degrees, cuts it
/// into `pieces` pieces, and spreads its velocity by up to `spread` from
/// the seed.
struct Transpose(ToolDescriptor);

impl Transpose {
    fn new() -> Self {
        Self(ToolDescriptor {
            id: "test_transpose".to_owned(),
            name: "Test Transpose".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "degrees",
                    "Degrees",
                    ParamKind::Whole {
                        min: -127,
                        max: 127,
                    },
                    ParamValue::Whole(0),
                    "How many scale degrees each note moves.",
                )
                .scale_aware(),
                ParamSpec::new(
                    "pieces",
                    "Pieces",
                    ParamKind::Whole { min: 1, max: 4 },
                    ParamValue::Whole(1),
                    "How many equal pieces each note is cut into.",
                ),
                ParamSpec::new(
                    "spread",
                    "Spread",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(0),
                    "How far a velocity may move at random.",
                ),
            ],
        })
    }
}

impl MidiTool for Transpose {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let degrees = i32::try_from(whole(values, 0)).unwrap_or(0);
        let pieces = whole(values, 1).max(1);
        let spread = u32::try_from(whole(values, 2)).unwrap_or(0);
        let mut rng = Rng::new(input.seed);
        let mut notes = Vec::new();
        for note in &input.notes {
            let key = u8::try_from(note.key).unwrap_or(0);
            // A move off the keyboard is made in semitones and folded.
            let moved = input.step_key(key, degrees).map_or(
                i16::try_from(i32::from(note.key) + degrees).unwrap_or(0),
                i16::from,
            );
            for piece in 0..pieces {
                let from = Ticks(note.length.0 * piece / pieces);
                let to = Ticks(note.length.0 * (piece + 1) / pieces);
                let mut out = note.part(from, to);
                out.key = moved;
                if spread > 0 {
                    let drawn = i32::try_from(rng.below(2 * spread + 1)).unwrap_or(0);
                    let velocity =
                        i32::from(note.velocity) + drawn - i32::try_from(spread).unwrap_or(0);
                    out.velocity = u8::try_from(velocity.clamp(1, 127)).unwrap_or(1);
                }
                notes.push(out);
            }
        }
        Ok(ToolOutput { notes })
    }
}

/// `test_fill`: a note of `key` at every `division` step of the range, as
/// long as `length` percent of the step.
struct Fill(ToolDescriptor);

impl Fill {
    fn new() -> Self {
        Self(ToolDescriptor {
            id: "test_fill".to_owned(),
            name: "Test Fill".to_owned(),
            kind: ToolKind::Generator,
            parameters: vec![
                ParamSpec::new(
                    "key",
                    "Key",
                    ParamKind::Pitch,
                    ParamValue::Pitch(60),
                    "The key the notes are written on.",
                ),
                ParamSpec::new(
                    "division",
                    "Division",
                    ParamKind::Division,
                    ParamValue::Division(7),
                    "The distance between the notes.",
                ),
                ParamSpec::new(
                    "length",
                    "Length",
                    ParamKind::Whole { min: 1, max: 200 },
                    ParamValue::Whole(50),
                    "How long each note lasts, as a share of the division.",
                )
                .unit("%"),
                ParamSpec::new(
                    "spread",
                    "Spread",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(0),
                    "How far a velocity may move at random.",
                ),
            ],
        })
    }
}

impl MidiTool for Fill {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let (ParamValue::Pitch(key), ParamValue::Division(division)) = (&values[0], &values[1])
        else {
            return Err(ToolError::Values(
                "the key and division are wrong".to_owned(),
            ));
        };
        let step = division_ticks(*division);
        let length = Ticks(step.0 * whole(values, 2) / 100);
        let spread = u32::try_from(whole(values, 3)).unwrap_or(0);
        let key = input.nearest_key(*key);
        let mut rng = Rng::new(input.seed);
        let mut notes = Vec::new();
        let mut at = input.range.0;
        while at < input.range.1 {
            let drawn = i32::try_from(rng.below(2 * spread + 1)).unwrap_or(0);
            let velocity = (100 + drawn - i32::try_from(spread).unwrap_or(0)).clamp(1, 127);
            notes.push(ToolNote::new(
                i16::from(key),
                at,
                length,
                u8::try_from(velocity).unwrap_or(100),
            ));
            at += step;
            if notes.len() > crate::registry::MAX_NOTES {
                break;
            }
        }
        Ok(ToolOutput { notes })
    }
}
