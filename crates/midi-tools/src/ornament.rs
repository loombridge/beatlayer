//! Ornament (the MIDI transformations spec, R14 to R17): puts a flam or
//! grace notes before each note.

use bl_core::time::Ticks;

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::registry::{MidiTool, ToolError};
use crate::value::{choice, options, percent, whole};

pub(crate) struct Ornament(ToolDescriptor);

fn position(id: &str, name: &str, what: &str) -> ParamSpec {
    ParamSpec::new(
        id,
        name,
        ParamKind::Whole { min: -100, max: 100 },
        ParamValue::Whole(-50),
        &format!("Where the {what} sit, as a share of the grid step: negative puts them before the note, positive takes over its start."),
    )
    .unit("%")
}

impl Ornament {
    pub(crate) fn new() -> Self {
        Self(ToolDescriptor {
            id: "ornament".to_owned(),
            name: "Ornament".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "mode",
                    "Mode",
                    ParamKind::Choice {
                        options: options(&[("flam", "Flam"), ("grace_notes", "Grace Notes")]),
                    },
                    ParamValue::Choice(0),
                    "Whether each note gets a flam or grace notes.",
                ),
                position("flam_position", "Flam Position", "flam"),
                ParamSpec::new(
                    "flam_velocity",
                    "Flam Velocity",
                    ParamKind::Whole { min: 1, max: 100 },
                    ParamValue::Whole(50),
                    "The flam's velocity, as a share of the note's.",
                )
                .unit("%"),
                ParamSpec::new(
                    "grace_notes_pitch",
                    "Grace Notes Pitch",
                    ParamKind::Choice {
                        options: options(&[("high", "High"), ("low", "Low"), ("same", "Same")]),
                    },
                    ParamValue::Choice(0),
                    "Whether every other grace note sits a step above the note, a step below it, or all sit at its pitch.",
                ),
                position("grace_notes_position", "Grace Notes Position", "grace notes"),
                ParamSpec::new(
                    "grace_notes_velocity",
                    "Grace Notes Velocity",
                    ParamKind::Whole { min: 1, max: 100 },
                    ParamValue::Whole(50),
                    "The grace notes' velocity, as a share of the note's.",
                )
                .unit("%"),
                ParamSpec::new(
                    "grace_notes_chance",
                    "Grace Notes Chance",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(100),
                    "The grace notes' chance, as a share of the note's.",
                )
                .unit("%"),
                ParamSpec::new(
                    "grace_notes_amount",
                    "Grace Notes Amount",
                    ParamKind::Whole { min: 1, max: 8 },
                    ParamValue::Whole(2),
                    "How many grace notes each note gets.",
                ),
            ],
        })
    }
}

impl MidiTool for Ornament {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let grace = choice(values, 0) == 1;
        let (position, velocity, amount) = if grace {
            (whole(values, 4), whole(values, 5), whole(values, 7))
        } else {
            (whole(values, 1), whole(values, 2), 1)
        };
        let (chance, pitch) = (whole(values, 6), choice(values, 3));
        let span = percent(input.grid_step.0, position.abs());
        let mut notes = Vec::with_capacity(input.notes.len() * (1 + amount as usize));
        for note in &input.notes {
            // R15: before the note, or over its start with the note moved later.
            let over = position > 0;
            let room = if over { span.min(note.length.0 - 1) } else { span.min(note.start.0) };
            if room <= 0 {
                notes.push(note.clone());
                continue;
            }
            let from = if over { note.start.0 } else { note.start.0 - room };
            let key = u8::try_from(note.key.clamp(0, 127)).unwrap_or(0);
            let velocity = u8::try_from(percent(i64::from(note.velocity), velocity).clamp(1, 127))
                .unwrap_or(1);
            for at in 0..amount {
                let edge = |n: i64| from + (room * n * 2 + amount) / (2 * amount);
                let (a, b) = (edge(at), edge(at + 1));
                if b <= a {
                    continue;
                }
                // Counting back from the note, the one next to it is 0.
                let back = amount - 1 - at;
                let mut ornament = if over {
                    note.part(Ticks(a - note.start.0), Ticks(b - note.start.0))
                } else {
                    ToolNote { pitch: Vec::new(), slide: Vec::new(), pressure: Vec::new(), ..note.clone() }
                };
                ornament.start = Ticks(a);
                ornament.length = Ticks(b - a);
                ornament.velocity = velocity;
                if grace {
                    ornament.chance = note.chance * chance as f64 / 100.0;
                    if pitch < 2 && back % 2 == 0 {
                        ornament.key = input.stepped(key, if pitch == 0 { 1 } else { -1 });
                    }
                }
                notes.push(ornament);
            }
            notes.push(if over { note.part(Ticks(room), note.length) } else { note.clone() });
        }
        Ok(ToolOutput { notes })
    }
}
