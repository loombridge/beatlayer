//! Connect (the MIDI transformations spec, R11 to R13): fills the gaps
//! between notes with new notes that wander from one pitch to the next.

use bl_core::time::Ticks;

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind, division_ticks};
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::random::Rng;
use crate::registry::{MAX_NOTES, MidiTool, ToolError};
use crate::value::{division, whole};

pub(crate) struct Connect(ToolDescriptor);

impl Connect {
    pub(crate) fn new() -> Self {
        Self(ToolDescriptor {
            id: "connect".to_owned(),
            name: "Connect".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "spread",
                    "Spread",
                    ParamKind::Whole { min: 0, max: 12 },
                    ParamValue::Whole(0),
                    "The most the new notes' pitches move at random from the line between the notes, in scale degrees where the clip has a scale, else in semitones.",
                )
                .scale_aware(),
                ParamSpec::new(
                    "density",
                    "Density",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(100),
                    "The share of the time between notes that new notes fill; every gap at 100 %.",
                )
                .unit("%"),
                ParamSpec::new(
                    "rate",
                    "Rate",
                    ParamKind::Division,
                    ParamValue::Division(7),
                    "The length of the new notes.",
                ),
                ParamSpec::new(
                    "tie",
                    "Tie",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(0),
                    "The chance that a new note lasts to the next original note.",
                )
                .unit("%"),
            ],
        })
    }
}

impl MidiTool for Connect {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let spread = i32::try_from(whole(values, 0)).unwrap_or(0);
        let density = whole(values, 1);
        let rate = division_ticks(division(values, 2));
        let tie = whole(values, 3) as f64 / 100.0;
        let mut rng = Rng::new(input.seed);
        // R12: the different starts in order; for each, the latest end and the highest note.
        let mut starts: Vec<(Ticks, Ticks, &ToolNote)> = Vec::new();
        for note in &input.notes {
            match starts.iter_mut().find(|(start, ..)| *start == note.start) {
                Some((_, end, top)) => {
                    *end = (*end).max(note.end());
                    if note.key > top.key {
                        *top = note;
                    }
                }
                None => starts.push((note.start, note.end(), note)),
            }
        }
        starts.sort_by_key(|(start, ..)| *start);
        let mut notes = input.notes.clone();
        for pair in starts.windows(2) {
            let ((start, end, from), (next, _, to)) = (pair[0], pair[1]);
            let room = (next - end).0;
            if room <= 0 || notes.len() > MAX_NOTES {
                continue;
            }
            let slots = (room / rate.0) as usize;
            let fill = ((slots as i64 * density + 50) / 100) as usize;
            // Density picks that many slots at random to fill.
            let mut free: Vec<usize> = (0..slots).collect();
            let mut picked = Vec::with_capacity(fill);
            for _ in 0..fill {
                picked.push(free.swap_remove(rng.below(free.len() as u32) as usize));
            }
            picked.sort_unstable();
            let (a, b) = (
                u8::try_from(from.key.clamp(0, 127)).unwrap_or(0),
                u8::try_from(to.key.clamp(0, 127)).unwrap_or(0),
            );
            let distance = f64::from(input.steps_between(a, b));
            for slot in picked {
                let at = end + Ticks(rate.0 * slot as i64);
                let along = ((at + Ticks(rate.0 / 2)) - start).0 as f64 / (next - start).0 as f64;
                let mut moved = (distance * along).round() as i32;
                if spread > 0 {
                    moved += rng.below(2 * spread as u32 + 1) as i32 - spread;
                }
                let tied = tie > 0.0 && rng.unit() < tie;
                let mut note = ToolNote::new(
                    input.stepped(a, moved),
                    at,
                    if tied { next - at } else { rate },
                    from.velocity,
                );
                note.release_velocity = from.release_velocity;
                note.chance = from.chance;
                note.velocity_deviation = from.velocity_deviation;
                note.deactivated = from.deactivated;
                notes.push(note);
                if tied {
                    break;
                }
            }
        }
        Ok(ToolOutput { notes })
    }
}
