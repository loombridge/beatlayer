//! Chop (the MIDI transformations spec, R7 to R10, and R43 for its toggles):
//! cuts each note into parts, some of them gaps, some stretched.

use bl_core::time::Ticks;

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolOutput};
use crate::random::Rng;
use crate::registry::{MidiTool, ToolError};
use crate::value::{toggles, whole};

/// How many elements the pattern and the emphasis have: the pattern
/// repeats past 16 parts.
pub const TOGGLES: usize = 16;

pub(crate) struct Chop(ToolDescriptor);

/// The pattern `gaps` writes (R8): every element a part for 0; for a
/// positive g every (g + 1)th a gap; for a negative −g one part and g gaps,
/// repeated.
#[must_use]
pub fn gaps_pattern(gaps: i64) -> Vec<bool> {
    (0..TOGGLES as i64)
        .map(|i| match gaps {
            0 => true,
            g if g > 0 => (i + 1) % (g + 1) != 0,
            g => i % (1 - g) == 0,
        })
        .collect()
}

/// Whether the first `parts` (at most 16) elements of `pattern` are what
/// `gaps` writes, so Gaps still shows its number (R8).
#[must_use]
pub fn pattern_follows_gaps(gaps: i64, parts: i64, pattern: &[bool]) -> bool {
    let p = parts.clamp(1, TOGGLES as i64) as usize;
    gaps_pattern(gaps)[..p] == pattern[..p.min(pattern.len())]
}

impl Chop {
    pub(crate) fn new() -> Self {
        Self(ToolDescriptor {
            id: "chop".to_owned(),
            name: "Chop".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "parts",
                    "Parts",
                    ParamKind::Whole { min: 2, max: 64 },
                    ParamValue::Whole(2),
                    "How many parts each note is cut into.",
                ),
                ParamSpec::new(
                    "gaps",
                    "Gaps",
                    ParamKind::Whole { min: -15, max: 15 },
                    ParamValue::Whole(0),
                    "Silences among the parts: a positive number puts a gap after that many parts, a negative one that many gaps after each part. Setting it writes the Pattern.",
                ),
                ParamSpec::new(
                    "pattern",
                    "Pattern",
                    ParamKind::Toggles { count: TOGGLES },
                    ParamValue::Toggles(vec![true; TOGGLES]),
                    "Which parts sound (on) and which are gaps (off), repeating past 16 parts.",
                ),
                ParamSpec::new(
                    "emphasis",
                    "Emphasis",
                    ParamKind::Toggles { count: TOGGLES },
                    ParamValue::Toggles(vec![false; TOGGLES]),
                    "Which parts or gaps Stretch Chunk(s) makes longer.",
                ),
                ParamSpec::new(
                    "stretch_chunks",
                    "Stretch Chunk(s)",
                    ParamKind::Whole { min: 2, max: 8 },
                    ParamValue::Whole(2),
                    "How many times as long the emphasised parts and gaps are.",
                ),
                ParamSpec::new(
                    "variation",
                    "Variation",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(0),
                    "How far the parts' starts and ends move at random.",
                )
                .unit("%"),
            ],
        })
    }
}

impl MidiTool for Chop {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    /// Setting Gaps writes the Pattern afresh, unless the same change sets
    /// the Pattern; a pattern with no part is refused (R8).
    fn settle(
        &self,
        before: &[ParamValue],
        after: &mut [ParamValue],
        changed: &[usize],
    ) -> Result<(), String> {
        let p = whole(after, 0).clamp(1, TOGGLES as i64);
        let gaps = whole(after, 1);
        if gaps.abs() >= p {
            if changed.contains(&1) {
                return Err(format!("`gaps` goes from {} to {} with {p} parts", 1 - p, p - 1));
            }
            after[1] = ParamValue::Whole(gaps.clamp(1 - p, p - 1));
        }
        if (changed.contains(&1) || gaps.abs() >= p) && !changed.contains(&2) {
            // An element with Emphasis on keeps its state.
            let mut pattern = gaps_pattern(whole(after, 1));
            for (i, on) in toggles(after, 3).iter().enumerate() {
                if *on {
                    pattern[i] = toggles(before, 2)[i];
                }
            }
            after[2] = ParamValue::Toggles(pattern);
        }
        if !toggles(after, 2)[..p as usize].contains(&true) {
            return Err("the pattern needs at least one part".to_owned());
        }
        Ok(())
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let parts = whole(values, 0) as usize;
        let p = parts.min(TOGGLES);
        let (pattern, emphasis) = (toggles(values, 2), toggles(values, 3));
        if !pattern[..p].contains(&true) {
            return Err(ToolError::Input("the pattern needs at least one part".to_owned()));
        }
        let stretch = whole(values, 4);
        let variation = whole(values, 5);
        let weight = |j: usize| if emphasis[j % p] { stretch } else { 1 };
        let total: i64 = (0..parts).map(weight).sum();
        let mut rng = Rng::new(input.seed);
        let mut notes = Vec::new();
        for note in &input.notes {
            let length = note.length.0;
            let mut cut = 0;
            let mut before = 0i64;
            for j in 0..parts {
                before += weight(j);
                // Each boundary is rounded from the note's start, so the pieces tile the note.
                let next = (length * before * 2 + total).div_euclid(2 * total);
                let (from, to) = (cut, next);
                cut = next;
                if to <= from || !pattern[j % p] {
                    continue;
                }
                let (mut from, mut to) = (from, to);
                if variation > 0 {
                    let half = (to - from) as f64 / 2.0 * variation as f64 / 100.0;
                    let mut shift = || ((rng.unit() * 2.0 - 1.0) * half).round() as i64;
                    from = (from + shift()).clamp(0, length - 1);
                    to = (to + shift()).clamp(from + 1, length);
                }
                notes.push(note.part(Ticks(from), Ticks(to)));
            }
        }
        Ok(ToolOutput { notes })
    }
}
