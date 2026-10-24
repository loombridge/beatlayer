//! Strum (the MIDI transformations spec, R29 and R30), and the strum law it
//! shares with the Chord device (the MIDI chord spec).

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolOutput};
use crate::registry::{MidiTool, ToolError};
use crate::values::{round_ticks, whole};
use bl_core::time::Ticks;

/// How far into a strum of `n` notes note `j` sounds, from 0 to 1, under
/// `tension` (−1 to 1): positive starts slowly and speeds up.
#[must_use]
pub fn strum_position(j: usize, n: usize, tension: f64) -> f64 {
    if n < 2 {
        return 0.0;
    }
    let u = j as f64 / (n - 1) as f64;
    1.0 - (1.0 - u).powf(2f64.powf(2.0 * tension))
}

/// `strum`: moves the starts of each chord's notes.
pub(crate) struct Strum(ToolDescriptor);

impl Strum {
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
            id: "strum".to_owned(),
            name: "Strum".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                percent(
                    "strum_low",
                    "Strum Low",
                    "How far the lowest note of each chord moves, as a share of the grid step. Positive is later.",
                ),
                percent(
                    "strum_high",
                    "Strum High",
                    "How far the highest note of each chord moves, as a share of the grid step. Positive is later.",
                ),
                percent(
                    "tension",
                    "Tension",
                    "Positive makes the gaps between the notes widest at the start and shrinking; negative does the opposite.",
                ),
            ],
        })
    }
}

impl MidiTool for Strum {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let low = whole(values, 0) as f64 / 100.0;
        let high = whole(values, 1) as f64 / 100.0;
        let tension = whole(values, 2) as f64 / 100.0;
        let mut notes = input.notes.clone();
        // A chord is the notes that start on one tick, lowest key first (R2).
        let given = &input.notes;
        let mut order: Vec<usize> = (0..given.len()).collect();
        order.sort_by_key(|at| (given[*at].start, given[*at].key));
        for chord in order.chunk_by(|a, b| given[*a].start == given[*b].start) {
            // A note alone at its start doesn't move (R30).
            if chord.len() < 2 {
                continue;
            }
            for (j, at) in chord.iter().enumerate() {
                let along = strum_position(j, chord.len(), tension);
                let moved = round_ticks(input.grid_step.0 as f64 * (low + (high - low) * along));
                let note = &mut notes[*at];
                let end = note.end();
                // The end stays put, and the note keeps a tick (R30).
                let start = (note.start + moved).max(Ticks::ZERO).min(end - Ticks(1));
                note.start = start;
                *note = note.with_length(end - start);
            }
        }
        Ok(ToolOutput { notes })
    }
}
