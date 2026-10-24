//! Shape (the MIDI generators spec, R11 to R13): a melody drawn from a curve
//! between two pitches, with jitter and ties.

use std::f64::consts::PI;

use bl_core::time::Ticks;

use crate::descriptor::{
    ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind, division_ticks,
};
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::random::Rng;
use crate::registry::{MAX_NOTES, MidiTool, ToolError};
use crate::value::{division, levels, options, pitch_range, whole};

/// How many levels the Shape Levels display has.
pub const LEVELS: usize = 16;
/// The presets' IDs and names; after them comes User, which drawing selects.
const PRESETS: [(&str, &str); 6] = [
    ("rise", "Rise"),
    ("fall", "Fall"),
    ("arch", "Arch"),
    ("valley", "Valley"),
    ("wave", "Wave"),
    ("steps", "Steps"),
];
const USER: usize = PRESETS.len();

pub(crate) struct Shape(ToolDescriptor);

/// The levels preset `preset` writes, for level k from 0 to 15 (R11).
///
/// # Panics
///
/// If `preset` isn't one of the six presets.
#[must_use]
pub fn preset_levels(preset: usize) -> Vec<f64> {
    (0..LEVELS)
        .map(|k| {
            let k = k as f64;
            let along = k / (LEVELS - 1) as f64;
            match preset {
                0 => along,
                1 => 1.0 - along,
                2 => (PI * along).sin(),
                3 => 1.0 - (PI * along).sin(),
                4 => 0.5 + 0.5 * (2.0 * PI * k / LEVELS as f64).sin(),
                5 => (k % 4.0) / 3.0,
                _ => panic!("preset {preset} isn't one of the six"),
            }
        })
        .collect()
}

/// The level at `x` from 0 to 1 along the range: the levels spread evenly
/// with straight lines between them (R12).
fn level_at(levels: &[f64], x: f64) -> f64 {
    let at = x.clamp(0.0, 1.0) * (levels.len() - 1) as f64;
    let below = at.floor() as usize;
    let above = (below + 1).min(levels.len() - 1);
    levels[below] + (levels[above] - levels[below]) * (at - below as f64)
}

impl Shape {
    pub(crate) fn new() -> Self {
        let mut names = PRESETS.to_vec();
        names.push(("user", "User"));
        Self(ToolDescriptor {
            id: "shape".to_owned(),
            name: "Shape".to_owned(),
            kind: ToolKind::Generator,
            parameters: vec![
                ParamSpec::new(
                    "preset",
                    "Shape Presets",
                    ParamKind::Choice {
                        options: options(&names),
                    },
                    ParamValue::Choice(0),
                    "Chooses a shape and writes its levels. Drawing in Shape Levels makes it User.",
                ),
                ParamSpec::new(
                    "levels",
                    "Shape Levels",
                    ParamKind::Levels {
                        count: LEVELS,
                        min: 0.0,
                        max: 1.0,
                    },
                    ParamValue::Levels(preset_levels(0)),
                    "The curve between the two pitches, drawn in 16 levels.",
                ),
                ParamSpec::new(
                    "pitch_range",
                    "Pitch Range",
                    ParamKind::PitchRange,
                    ParamValue::PitchRange { low: 48, high: 72 },
                    "The lowest and highest key the notes are written on.",
                ),
                ParamSpec::new(
                    "rate",
                    "Rate",
                    ParamKind::Division,
                    ParamValue::Division(7),
                    "How long the shortest note is.",
                ),
                ParamSpec::new(
                    "tie",
                    "Tie",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(0),
                    "The chance that a note lasts into the next.",
                )
                .unit("%"),
                ParamSpec::new(
                    "density",
                    "Density",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(100),
                    "How much of the shape gets notes.",
                )
                .unit("%"),
                ParamSpec::new(
                    "jitter",
                    "Jitter",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(0),
                    "How far a pitch moves at random from the shape, within the range.",
                )
                .unit("%"),
            ],
        })
    }
}

impl MidiTool for Shape {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    /// Choosing a preset writes its levels, and drawing levels makes the
    /// preset User (R11).
    fn settle(
        &self,
        _before: &[ParamValue],
        after: &mut [ParamValue],
        changed: &[usize],
    ) -> Result<(), String> {
        match (changed.contains(&0), changed.contains(&1)) {
            (true, false) => {
                if let ParamValue::Choice(preset) = after[0]
                    && preset < USER
                {
                    after[1] = ParamValue::Levels(preset_levels(preset));
                }
            }
            (false, true) => after[0] = ParamValue::Choice(USER),
            _ => {}
        }
        Ok(())
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let (from, to) = (input.range.0.0, input.range.1.0);
        let step = division_ticks(division(values, 3)).0;
        let slots = ((to - from) / step).max(0);
        let take = (slots * whole(values, 5) + 50) / 100;
        if take > MAX_NOTES as i64 {
            return Err(ToolError::TooManyNotes(take as usize));
        }
        let mut rng = Rng::new(input.seed);
        // Density picks that many slots at random: a partial shuffle.
        let mut order: Vec<i64> = (0..if take > 0 { slots } else { 0 }).collect();
        for i in 0..take as usize {
            let j = i + rng.below((order.len() - i) as u32) as usize;
            order.swap(i, j);
        }
        let mut filled = order[..take as usize].to_vec();
        filled.sort_unstable();
        let (low, high) = pitch_range(values, 2);
        let span = f64::from(high - low);
        let levels = levels(values, 1);
        let (tie, jitter) = (whole(values, 4), whole(values, 6) as f64 / 100.0);
        let mut notes = Vec::new();
        let mut tied = None;
        for slot in filled {
            if tied == Some(slot) {
                continue;
            }
            let level = level_at(levels, (slot * step) as f64 / (to - from) as f64);
            let moved = (rng.unit() * 2.0 - 1.0) * jitter * span;
            let key = (f64::from(low) + level * span + moved)
                .clamp(f64::from(low), f64::from(high))
                .round() as u8;
            let ties = i64::from(rng.below(100)) < tie && slot + 1 < slots;
            tied = ties.then_some(slot + 1);
            notes.push(ToolNote::new(
                i16::from(input.nearest_key(key)),
                Ticks(from + slot * step),
                Ticks(if ties { 2 * step } else { step }),
                100,
            ));
        }
        Ok(ToolOutput { notes })
    }
}
