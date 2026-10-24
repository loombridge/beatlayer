//! Recombine (the MIDI transformations spec, R23 to R25).

use crate::descriptor::{ChoiceOption, ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolOutput};
use crate::random::Rng;
use crate::registry::{MidiTool, ToolError};
use crate::value::{choice, switch, whole};
use bl_core::time::Ticks;

/// How many steps Rotation Steps reaches either way. A rotation counts
/// round the notes, so one past what the input has turns as its remainder.
const MOST_STEPS: i64 = 255;

/// `recombine`: moves one property among the notes.
pub(crate) struct Recombine(ToolDescriptor);

impl Recombine {
    pub(crate) fn new() -> Self {
        Self(ToolDescriptor {
            id: "recombine".to_owned(),
            name: "Recombine".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "dimension",
                    "Dimension",
                    ParamKind::Choice {
                        options: vec![
                            ChoiceOption::new("position", "Position"),
                            ChoiceOption::new("pitch", "Pitch"),
                            ChoiceOption::new("duration", "Duration"),
                            ChoiceOption::new("velocity", "Velocity"),
                        ],
                    },
                    ParamValue::Choice(0),
                    "The property that is rearranged among the notes.",
                ),
                ParamSpec::new(
                    "shuffle",
                    "Shuffle",
                    ParamKind::Switch,
                    ParamValue::Switch(false),
                    "Puts the values in a random order, anew each time the tool is applied.",
                ),
                ParamSpec::new(
                    "mirror",
                    "Mirror",
                    ParamKind::Switch,
                    ParamValue::Switch(false),
                    "Reverses the order of the values.",
                ),
                ParamSpec::new(
                    "rotation_steps",
                    "Rotation Steps",
                    ParamKind::Whole {
                        min: -MOST_STEPS,
                        max: MOST_STEPS,
                    },
                    ParamValue::Whole(0),
                    "Moves the values round: positive later, negative earlier.",
                ),
                ParamSpec::new(
                    "rotate_on_grid",
                    "Rotate on Grid",
                    ParamKind::Switch,
                    ParamValue::Switch(false),
                    "With Position, rotates by grid steps of the range instead of by notes.",
                ),
            ],
        })
    }
}

impl MidiTool for Recombine {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let dimension = choice(values, 0);
        let steps = whole(values, 3);
        // Only Position can rotate on the grid (R23).
        let on_grid = switch(values, 4) && dimension == 0;
        let mut notes = input.notes.clone();
        let count = notes.len();
        let mut list: Vec<i64> = notes
            .iter()
            .map(|note| match dimension {
                0 => note.start.0,
                1 => i64::from(note.key),
                2 => note.length.0,
                _ => i64::from(note.velocity),
            })
            .collect();
        if switch(values, 1) {
            let mut rng = Rng::new(input.seed);
            for at in (1..count).rev() {
                list.swap(at, rng.below(at as u32 + 1) as usize);
            }
        }
        if switch(values, 2) {
            list.reverse();
        }
        if !on_grid && count > 0 {
            list.rotate_right(steps.rem_euclid(count as i64) as usize);
        }
        let (from, to) = input.range;
        for (note, value) in notes.iter_mut().zip(list) {
            match dimension {
                0 => note.start = Ticks(value),
                1 => note.key = value as i16,
                2 => *note = note.with_length(Ticks(value)),
                _ => note.velocity = value as u8,
            }
            if on_grid && to > from {
                // A start that passes either end of the range goes round (R25).
                let moved = note.start - from + Ticks(steps * input.grid_step.0);
                note.start = from + moved.rem_euclid(to - from);
            }
        }
        Ok(ToolOutput { notes })
    }
}
