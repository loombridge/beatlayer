//! Rhythm (the MIDI generators spec, R4 to R7): a rhythm on one pitch from a
//! pattern of steps, with accents and split steps.

use bl_core::time::Ticks;

use crate::descriptor::{
    ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind, division_ticks,
};
use crate::euclidean::euclid;
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::random::Rng;
use crate::registry::{MAX_NOTES, MidiTool, ToolError};
use crate::value::{division, hold, pitch, whole};

/// The most steps a pattern has.
pub const MAX_STEPS: usize = 16;

pub(crate) struct Rhythm(ToolDescriptor);

/// How many ways there are to choose `k` of `n` steps.
fn choose(n: usize, k: usize) -> u64 {
    (0..k).fold(1, |c, i| c * (n - i) as u64 / (i as u64 + 1))
}

/// How many patterns `steps` and `density` give (R5).
#[must_use]
pub fn pattern_count(steps: usize, density: usize) -> usize {
    choose(steps, density.min(steps)) as usize
}

/// Which steps of pattern `pattern` hold a note, moved `shift` steps later
/// (R5): pattern 0 is Euclidean's even spread, and pattern p from 1 the p-th
/// of the other choices of `density` steps, in order of their step numbers
/// compared from the first. A pattern past the last is the last.
#[must_use]
pub fn rhythm_steps(steps: usize, density: usize, pattern: usize, shift: i64) -> Vec<bool> {
    let steps = steps.clamp(1, MAX_STEPS);
    let density = density.min(steps);
    // A pattern as bits, step 0 the highest: the choices in order of their
    // step numbers are the masks from the greatest down.
    let mask = |hits: &[bool]| hits.iter().fold(0u32, |m, hit| (m << 1) | u32::from(*hit));
    let even = euclid(steps, density, 0);
    let even_mask = mask(&even);
    let last = pattern_count(steps, density) - 1;
    let wanted = pattern.min(last);
    let chosen = if wanted == 0 {
        even_mask
    } else {
        (0..1u32 << steps)
            .rev()
            .filter(|m| m.count_ones() as usize == density && *m != even_mask)
            .nth(wanted - 1)
            .unwrap_or(even_mask)
    };
    let mut hits = vec![false; steps];
    for (i, hit) in hits.iter_mut().enumerate() {
        let from = (i as i64 - shift).rem_euclid(steps as i64) as usize;
        *hit = chosen >> (steps - 1 - from) & 1 == 1;
    }
    hits
}

impl Rhythm {
    pub(crate) fn new() -> Self {
        Self(ToolDescriptor {
            id: "rhythm".to_owned(),
            name: "Rhythm".to_owned(),
            kind: ToolKind::Generator,
            parameters: vec![
                ParamSpec::new(
                    "pitch",
                    "Pitch",
                    ParamKind::Pitch,
                    ParamValue::Pitch(60),
                    "The key the notes are written on, or the Drum Rack pad.",
                ),
                ParamSpec::new(
                    "steps",
                    "Steps",
                    ParamKind::Whole {
                        min: 1,
                        max: MAX_STEPS as i64,
                    },
                    ParamValue::Whole(16),
                    "How many steps the pattern has.",
                ),
                ParamSpec::new(
                    "density",
                    "Density",
                    ParamKind::Whole {
                        min: 0,
                        max: MAX_STEPS as i64,
                    },
                    ParamValue::Whole(4),
                    "How many of the steps hold a note.",
                ),
                ParamSpec::new(
                    "pattern",
                    "Pattern",
                    ParamKind::Whole {
                        min: 0,
                        max: choose(MAX_STEPS, MAX_STEPS / 2) as i64 - 1,
                    },
                    ParamValue::Whole(0),
                    "Which steps hold the notes; pattern 0 spreads them evenly.",
                ),
                ParamSpec::new(
                    "step_duration",
                    "Step Duration",
                    ParamKind::Division,
                    ParamValue::Division(7),
                    "How long a step is, so how many times the pattern fits.",
                ),
                ParamSpec::new(
                    "split",
                    "Split",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(0),
                    "The chance that a step is halved into two notes.",
                )
                .unit("%"),
                ParamSpec::new(
                    "shift",
                    "Shift",
                    ParamKind::Whole {
                        min: 1 - MAX_STEPS as i64,
                        max: MAX_STEPS as i64 - 1,
                    },
                    ParamValue::Whole(0),
                    "Moves the notes by steps, later for a positive number.",
                ),
                ParamSpec::new(
                    "velocity",
                    "Velocity",
                    ParamKind::Whole { min: 1, max: 127 },
                    ParamValue::Whole(100),
                    "The velocity of the notes.",
                ),
                ParamSpec::new(
                    "accent",
                    "Accent",
                    ParamKind::Whole { min: 1, max: 127 },
                    ParamValue::Whole(127),
                    "The velocity of the accented notes.",
                ),
                ParamSpec::new(
                    "accent_frequency",
                    "Accent Frequency",
                    ParamKind::Whole {
                        min: 0,
                        max: MAX_STEPS as i64,
                    },
                    ParamValue::Whole(0),
                    "How often a note is accented; 1 accents every note, 0 none.",
                ),
                ParamSpec::new(
                    "accent_offset",
                    "Accent Offset",
                    ParamKind::Whole {
                        min: 0,
                        max: MAX_STEPS as i64 - 1,
                    },
                    ParamValue::Whole(0),
                    "Which notes take the accent.",
                ),
            ],
        })
    }
}

impl MidiTool for Rhythm {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    /// Density, Pattern, Shift and the accents' numbers depend on Steps and
    /// Density (R4).
    fn settle(
        &self,
        _before: &[ParamValue],
        after: &mut [ParamValue],
        changed: &[usize],
    ) -> Result<(), String> {
        let steps = whole(after, 1);
        hold(after, 2, (0, steps), changed, "density")?;
        let density = whole(after, 2);
        let patterns = pattern_count(steps as usize, density as usize) as i64;
        hold(after, 3, (0, patterns - 1), changed, "pattern")?;
        hold(after, 6, (1 - steps, steps - 1), changed, "shift")?;
        hold(after, 9, (0, density), changed, "accent_frequency")?;
        hold(
            after,
            10,
            (0, (density - 1).max(0)),
            changed,
            "accent_offset",
        )
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let step = division_ticks(division(values, 4)).0;
        let (from, to) = (input.range.0.0, input.range.1.0);
        // More steps than the range holds act as that many (R6).
        let held = usize::try_from(((to - from) / step).max(1)).unwrap_or(1);
        let steps = usize::try_from(whole(values, 1))
            .unwrap_or(1)
            .clamp(1, MAX_STEPS.min(held));
        let density = usize::try_from(whole(values, 2)).unwrap_or(0).min(steps);
        let pattern = usize::try_from(whole(values, 3)).unwrap_or(0);
        let hits: Vec<usize> = rhythm_steps(steps, density, pattern, whole(values, 6))
            .iter()
            .enumerate()
            .filter(|(_, hit)| **hit)
            .map(|(at, _)| at)
            .collect();
        let split = whole(values, 5);
        let (velocity, accent) = (whole(values, 7), whole(values, 8));
        let frequency = whole(values, 9).min(density as i64);
        let offset = whole(values, 10);
        let key = i16::from(input.nearest_key(pitch(values, 0)));
        let length = steps as i64 * step;
        let patterns = (to - from + length - 1).max(0) / length;
        if patterns * hits.len() as i64 > MAX_NOTES as i64 {
            return Err(ToolError::TooManyNotes(
                (patterns * hits.len() as i64) as usize,
            ));
        }
        let mut rng = Rng::new(input.seed);
        let mut notes = Vec::new();
        let mut counted = 0i64;
        for pattern in 0..patterns {
            for at in &hits {
                let start = from + pattern * length + *at as i64 * step;
                let accented = frequency > 0 && (counted - offset).rem_euclid(frequency) == 0;
                counted += 1;
                let velocity =
                    u8::try_from(if accented { accent } else { velocity }).unwrap_or(100);
                if i64::from(rng.below(100)) < split {
                    let half = step / 2;
                    notes.push(ToolNote::new(key, Ticks(start), Ticks(half), velocity));
                    notes.push(ToolNote::new(
                        key,
                        Ticks(start + half),
                        Ticks(step - half),
                        velocity,
                    ));
                } else {
                    notes.push(ToolNote::new(key, Ticks(start), Ticks(step), velocity));
                }
            }
        }
        Ok(ToolOutput { notes })
    }
}
