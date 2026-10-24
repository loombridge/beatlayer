//! Span (the MIDI transformations spec, R26 to R28).

use crate::descriptor::{ChoiceOption, ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolOutput};
use crate::random::Rng;
use crate::registry::{MidiTool, ToolError};
use crate::values::{choice, percent_of, round_ticks, whole};
use bl_core::time::Ticks;

/// `span`: sets the notes' lengths by articulation.
pub(crate) struct Span(ToolDescriptor);

impl Span {
    pub(crate) fn new() -> Self {
        let percent = |id: &str, name: &str, description: &str| {
            ParamSpec::new(
                id,
                name,
                ParamKind::Whole {
                    min: -100,
                    max: 100,
                },
                ParamValue::Whole(0),
                description,
            )
            .unit("%")
        };
        Self(ToolDescriptor {
            id: "span".to_owned(),
            name: "Span".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "mode",
                    "Mode",
                    ParamKind::Choice {
                        options: vec![
                            ChoiceOption::new("legato", "Legato"),
                            ChoiceOption::new("tenuto", "Tenuto"),
                            ChoiceOption::new("staccato", "Staccato"),
                        ],
                    },
                    ParamValue::Choice(0),
                    "Legato lasts each note to the next note's start, Tenuto keeps the lengths, Staccato gives every note half the smallest gap between starts.",
                ),
                percent(
                    "offset",
                    "Offset",
                    "Moves every note's end by up to a grid step, as a share of it. Positive lengthens.",
                ),
                percent(
                    "variation",
                    "Variation",
                    "Changes the lengths at random. Positive shortens and negative lengthens.",
                ),
            ],
        })
    }
}

impl MidiTool for Span {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let mode = choice(values, 0);
        let offset = percent_of(input.grid_step, whole(values, 1));
        let variation = whole(values, 2) as f64 / 100.0;
        let mut starts: Vec<Ticks> = input.notes.iter().map(|note| note.start).collect();
        starts.sort_unstable();
        starts.dedup();
        // The smallest gap between two of the different starts next to each
        // other, halved: Staccato's length (R27).
        let staccato = starts
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .min()
            .map(|gap| round_ticks(gap.0 as f64 / 2.0));
        let mut rng = Rng::new(input.seed);
        let mut notes = input.notes.clone();
        for note in &mut notes {
            let mut length = match (mode, staccato) {
                (0, _) => match starts.get(starts.partition_point(|start| *start <= note.start)) {
                    Some(next) => *next - note.start,
                    // The latest notes last to the range's end, or keep
                    // their length where they start at or after it.
                    None if note.start < input.range.1 => input.range.1 - note.start,
                    None => note.length,
                },
                (2, Some(length)) => length,
                _ => note.length,
            };
            length += offset;
            if variation != 0.0 {
                length = round_ticks(length.0 as f64 * (1.0 - variation * rng.unit()));
            }
            *note = note.with_length(length.max(Ticks(1)));
        }
        Ok(ToolOutput { notes })
    }
}
