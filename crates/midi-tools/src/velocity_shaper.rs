//! Velocity Shaper (the MIDI transformations spec, R34 and R35).

use crate::descriptor::{
    Breakpoint, ChoiceOption, DIVISION_NAMES, ParamKind, ParamSpec, ParamValue, ToolDescriptor,
    ToolKind, division_ticks,
};
use crate::input::{ToolInput, ToolOutput};
use crate::registry::{MidiTool, ToolError};
use crate::values::{choice, whole};

/// The fewest and the most breakpoints the envelope has (R34).
pub const ENVELOPE_POINTS: (usize, usize) = (2, 32);

/// `velocity_shaper`: sets the notes' velocities from an envelope.
pub(crate) struct VelocityShaper(ToolDescriptor);

impl VelocityShaper {
    pub(crate) fn new() -> Self {
        let mut divisions = vec![ChoiceOption::new("grid", "Grid")];
        divisions.extend(DIVISION_NAMES.iter().map(|n| ChoiceOption::new(n, n)));
        Self(ToolDescriptor {
            id: "velocity_shaper".to_owned(),
            name: "Velocity Shaper".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "envelope",
                    "Envelope",
                    ParamKind::Breakpoints { min: 0.0, max: 1.0 },
                    ParamValue::Breakpoints(vec![
                        Breakpoint { x: 0.0, y: 0.0 },
                        Breakpoint { x: 1.0, y: 1.0 },
                    ]),
                    "The velocities' shape across the range, from 2 to 32 breakpoints: the first at the start, the last at the end.",
                ),
                ParamSpec::new(
                    "minimum_velocity",
                    "Minimum Velocity",
                    ParamKind::Whole { min: 1, max: 127 },
                    ParamValue::Whole(1),
                    "The velocity at the bottom of the envelope.",
                ),
                ParamSpec::new(
                    "maximum_velocity",
                    "Maximum Velocity",
                    ParamKind::Whole { min: 1, max: 127 },
                    ParamValue::Whole(127),
                    "The velocity at the top of the envelope.",
                ),
                ParamSpec::new(
                    "loop",
                    "Loop",
                    ParamKind::Whole { min: 1, max: 16 },
                    ParamValue::Whole(1),
                    "How many times the envelope repeats across the range.",
                ),
                ParamSpec::new(
                    "rotate",
                    "Rotate",
                    ParamKind::Whole { min: -16, max: 16 },
                    ParamValue::Whole(0),
                    "Shifts the envelope by steps of Division from the range's start.",
                ),
                ParamSpec::new(
                    "division",
                    "Division",
                    ParamKind::Choice { options: divisions },
                    ParamValue::Choice(0),
                    "How long one step of Rotate is: the grid step or a note division.",
                ),
            ],
        })
    }
}

/// The envelope's value `phase` of the way through it, along straight lines
/// between its breakpoints.
fn value_at(points: &[Breakpoint], phase: f64) -> f64 {
    let after = points.partition_point(|point| point.x <= phase);
    match (after.checked_sub(1).map(|at| points[at]), points.get(after)) {
        (Some(a), Some(b)) => a.y + (b.y - a.y) * (phase - a.x) / (b.x - a.x),
        (Some(a), None) => a.y,
        (None, Some(b)) => b.y,
        (None, None) => 0.0,
    }
}

impl MidiTool for VelocityShaper {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn validate(&self, values: &[ParamValue]) -> Result<(), ToolError> {
        let Some(ParamValue::Breakpoints(points)) = values.first() else {
            return Ok(());
        };
        let (fewest, most) = ENVELOPE_POINTS;
        let pinned =
            points.first().is_some_and(|p| p.x == 0.0) && points.last().is_some_and(|p| p.x == 1.0);
        if (fewest..=most).contains(&points.len()) && pinned {
            Ok(())
        } else {
            Err(ToolError::Values(format!(
                "`envelope` takes {fewest} to {most} breakpoints, the first at time 0 and the last at time 1"
            )))
        }
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let Some(ParamValue::Breakpoints(points)) = values.first() else {
            return Err(ToolError::Values("`envelope` is missing".to_owned()));
        };
        let (low, high) = (whole(values, 1), whole(values, 2));
        let looped = whole(values, 3).max(1);
        let rotate = whole(values, 4);
        let division = match choice(values, 5) {
            0 => input.grid_step,
            at => division_ticks(at - 1),
        };
        let (from, to) = input.range;
        let period = (to - from).0 as f64 / looped as f64;
        let mut notes = input.notes.clone();
        for note in &mut notes {
            let along = (note.start - from).0 - rotate * division.0;
            let phase = if period > 0.0 {
                let turns = along as f64 / period;
                turns - turns.floor()
            } else {
                0.0
            };
            let velocity = low as f64 + value_at(points, phase) * (high - low) as f64;
            note.velocity = (velocity + 0.5).floor().clamp(1.0, 127.0) as u8;
        }
        Ok(ToolOutput { notes })
    }
}
