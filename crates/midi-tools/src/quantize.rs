//! Quantize (the MIDI transformations spec, R18 and R19): moves notes onto
//! the grid. The Clip View's Quantize settings are this tool's settings
//! (R20).

use bl_core::time::Ticks;

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolOutput};
use crate::registry::{MidiTool, ToolError};
use crate::value::{choice, options, switch, whole};

/// The Grid choices after Current Grid, in whole notes divided by these.
pub const GRIDS: [i64; 6] = [1, 2, 4, 8, 16, 32];

pub(crate) struct Quantize(ToolDescriptor);

impl Quantize {
    pub(crate) fn new() -> Self {
        Self(ToolDescriptor {
            id: "quantize".to_owned(),
            name: "Quantize".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "start",
                    "Start",
                    ParamKind::Switch,
                    ParamValue::Switch(true),
                    "Moves the notes' starts toward the grid, keeping their lengths.",
                ),
                ParamSpec::new(
                    "end",
                    "End",
                    ParamKind::Switch,
                    ParamValue::Switch(false),
                    "Moves the notes' ends toward the grid, after their starts have moved.",
                ),
                ParamSpec::new(
                    "grid",
                    "Grid",
                    ParamKind::Choice {
                        options: options(&[
                            ("current_grid", "Current Grid"),
                            ("1", "1"),
                            ("1_2", "1/2"),
                            ("1_4", "1/4"),
                            ("1_8", "1/8"),
                            ("1_16", "1/16"),
                            ("1_32", "1/32"),
                        ]),
                    },
                    ParamValue::Choice(0),
                    "The spacing of the grid lines the notes move toward: the note editor's grid, or a note division.",
                ),
                ParamSpec::new(
                    "triplets",
                    "Triplets",
                    ParamKind::Switch,
                    ParamValue::Switch(false),
                    "Makes the grid's lines two-thirds as far apart; not available with Current Grid.",
                ),
                ParamSpec::new(
                    "amount",
                    "Amount",
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(100),
                    "How far the notes move toward the grid; 100 % puts them on it.",
                )
                .unit("%"),
            ],
        })
    }
}

/// The spacing of the lines of Grid choice `grid` (0 for Current Grid, which
/// is `current`), two-thirds as long with `triplets` (R19).
#[must_use]
pub fn grid_ticks(grid: usize, triplets: bool, current: Ticks) -> Ticks {
    match grid.checked_sub(1).and_then(|at| GRIDS.get(at)) {
        None => current,
        Some(n) if triplets => Ticks(Ticks::WHOLE.0 / n * 2 / 3),
        Some(n) => Ticks(Ticks::WHOLE.0 / n),
    }
}

/// The grid line nearest `at` for lines `step` apart, laid out from each
/// bar's start (`bar` long): a spacing of whole bars counts bars from the
/// contents' start.
fn nearest(at: i64, step: i64, bar: i64) -> i64 {
    if step >= bar && step % bar == 0 {
        let start = at.div_euclid(step) * step;
        return if at - start <= start + step - at { start } else { start + step };
    }
    let start = at.div_euclid(bar) * bar;
    let end = start + bar;
    let snapped = (start + ((at - start) as f64 / step as f64).round() as i64 * step).min(end);
    if end - at < (at - snapped).abs() { end } else { snapped }
}

/// The first grid line after `at`.
fn after(at: i64, step: i64, bar: i64) -> i64 {
    if step >= bar && step % bar == 0 {
        return at.div_euclid(step) * step + step;
    }
    let start = at.div_euclid(bar) * bar;
    (start + ((at - start) / step + 1) * step).min(start + bar)
}

/// `at` moved `amount` percent of the way to `to`, rounded to the nearest
/// tick, half up.
fn toward(at: i64, to: i64, amount: i64) -> i64 {
    at + ((to - at) * amount + 50).div_euclid(100)
}

impl MidiTool for Quantize {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let (start, end, amount) = (switch(values, 0), switch(values, 1), whole(values, 4));
        let step = grid_ticks(choice(values, 2), switch(values, 3), input.grid_step).0.max(1);
        let bar = input.signature.bar_length().0.max(1);
        let notes = input
            .notes
            .iter()
            .map(|note| {
                let mut note = note.clone();
                if start {
                    note.start = Ticks(toward(note.start.0, nearest(note.start.0, step, bar), amount));
                }
                if end {
                    let at = note.end().0;
                    let mut line = nearest(at, step, bar);
                    if line <= note.start.0 {
                        line = after(note.start.0, step, bar);
                    }
                    note = note.with_length(Ticks(toward(at, line, amount) - note.start.0));
                }
                note
            })
            .collect();
        Ok(ToolOutput { notes })
    }
}
