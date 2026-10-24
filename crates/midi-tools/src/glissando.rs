//! Glissando (the MIDI transformations spec, R36 to R38): bends each note
//! of a chord toward the note it pairs with in the next chord.

use bl_core::time::Ticks;

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolNote, ToolOutput, ToolPoint};
use crate::registry::{MidiTool, ToolError};
use crate::value::{percent, whole};

/// Breakpoints after Start, evenly spaced to the note's end (R38).
const STEPS: i64 = 16;

/// The furthest a note's Pitch envelope bends, in semitones (the MPE spec, R1).
const RANGE: f64 = 48.0;

pub(crate) struct Glissando(ToolDescriptor);

impl Glissando {
    pub(crate) fn new() -> Self {
        Self(ToolDescriptor {
            id: "glissando".to_owned(),
            name: "Glissando".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "start",
                    "Start",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(50),
                    "Where in each note the bend begins; the pitch holds until then.",
                )
                .unit("%"),
                ParamSpec::new(
                    "curve",
                    "Curve",
                    ParamKind::Whole {
                        min: -100,
                        max: 100,
                    },
                    ParamValue::Whole(0),
                    "The bend's shape; a positive curve reaches the next note sooner.",
                )
                .unit("%"),
            ],
        })
    }
}

impl MidiTool for Glissando {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        if input.notes.len() < 2 {
            return Err(ToolError::Input(
                "Glissando needs at least two notes to bend from one to the next".to_owned(),
            ));
        }
        let (start, curve) = (whole(values, 0), whole(values, 1));
        // R37: the chords in order of their starts, each from its lowest note up.
        let mut order: Vec<usize> = (0..input.notes.len()).collect();
        order.sort_by_key(|&at| (input.notes[at].start, input.notes[at].key));
        let chords: Vec<&[usize]> = order
            .chunk_by(|a, b| input.notes[*a].start == input.notes[*b].start)
            .collect();
        let mut notes = input.notes.clone();
        for pair in chords.windows(2) {
            for (rank, &at) in pair[0].iter().enumerate() {
                let partner = pair[1][rank.min(pair[1].len() - 1)];
                let jump = f64::from(input.notes[partner].key - input.notes[at].key);
                notes[at].pitch = bend(&input.notes[at], jump, start, curve);
            }
        }
        Ok(ToolOutput { notes })
    }
}

/// The Pitch envelope of `note` bending by `jump` semitones (R38).
fn bend(note: &ToolNote, jump: f64, start: i64, curve: i64) -> Vec<ToolPoint> {
    let begin = Ticks(percent(note.length.0, start));
    let power = 2f64.powf(-2.0 * curve as f64 / 100.0);
    let point = |at: Ticks, share: f64| ToolPoint {
        at,
        value: (jump * share).clamp(-RANGE, RANGE),
        bend: 0.0,
    };
    let mut points = vec![point(Ticks::ZERO, 0.0), point(begin, 0.0)];
    for step in 1..=STEPS {
        let at = begin + Ticks((note.length - begin).0 * step / STEPS);
        points.push(point(at, (step as f64 / STEPS as f64).powf(power)));
    }
    // Positions are whole ticks, so a short note or a Start of 100 % lands
    // points on one tick; the later one, nearer the bend's end, stays.
    let mut kept: Vec<ToolPoint> = Vec::with_capacity(points.len());
    for point in points {
        match kept.last_mut() {
            Some(last) if last.at == point.at => *last = point,
            _ => kept.push(point),
        }
    }
    kept
}
