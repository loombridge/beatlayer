//! Time Warp (the MIDI transformations spec, R31 to R33).

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::registry::{MidiTool, ToolError};
use crate::values::{round_ticks, switch, whole};
use bl_core::time::Ticks;

/// The breakpoints' places in the parameter list: each has a switch, a Time
/// and a Speed, then come Quantize, Preserve Time Range and Include Note End.
const BREAKPOINTS: usize = 3;
const QUANTIZE: usize = 3 * BREAKPOINTS;

/// `time_warp`: moves the notes along a speed curve.
pub(crate) struct TimeWarp(ToolDescriptor);

impl TimeWarp {
    pub(crate) fn new() -> Self {
        let mut parameters = Vec::new();
        for (n, (on, time)) in [(true, 0), (true, 100), (false, 50)]
            .into_iter()
            .enumerate()
        {
            let n = n + 1;
            parameters.push(ParamSpec::new(
                &format!("breakpoint_{n}"),
                &format!("Breakpoint {n}"),
                ParamKind::Switch,
                ParamValue::Switch(on),
                "Whether this breakpoint shapes the speed curve.",
            ));
            parameters.push(
                ParamSpec::new(
                    &format!("breakpoint_{n}_time"),
                    &format!("Breakpoint {n} Time"),
                    ParamKind::Whole { min: 0, max: 100 },
                    ParamValue::Whole(time),
                    "Where the breakpoint sits in the range.",
                )
                .unit("%"),
            );
            parameters.push(
                ParamSpec::new(
                    &format!("breakpoint_{n}_speed"),
                    &format!("Breakpoint {n} Speed"),
                    ParamKind::Whole { min: 25, max: 400 },
                    ParamValue::Whole(100),
                    "How fast time runs at the breakpoint: above 100 % the notes draw closer together.",
                )
                .unit("%"),
            );
        }
        for (id, name, default, description) in [
            (
                "quantize",
                "Quantize",
                false,
                "Puts the warped notes on the grid.",
            ),
            (
                "preserve_time_range",
                "Preserve Time Range",
                true,
                "Keeps the warped notes within the range's time.",
            ),
            (
                "include_note_end",
                "Include Note End",
                false,
                "Warps the notes' ends too, which changes their lengths.",
            ),
        ] {
            parameters.push(ParamSpec::new(
                id,
                name,
                ParamKind::Switch,
                ParamValue::Switch(default),
                description,
            ));
        }
        Self(ToolDescriptor {
            id: "time_warp".to_owned(),
            name: "Time Warp".to_owned(),
            kind: ToolKind::Transformation,
            parameters,
        })
    }
}

/// The switched-on breakpoints as a share of the range and a speed, in
/// order of time.
fn curve(values: &[ParamValue]) -> Vec<(f64, f64)> {
    let mut points: Vec<(f64, f64)> = (0..BREAKPOINTS)
        .filter(|n| switch(values, 3 * n))
        .map(|n| {
            (
                whole(values, 3 * n + 1) as f64 / 100.0,
                whole(values, 3 * n + 2) as f64 / 100.0,
            )
        })
        .collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    points
}

/// The integral from the first breakpoint to `x` of 100 % divided by the
/// speed: the speed is level before the first breakpoint and after the
/// last, and straight between them (R32).
fn integral(points: &[(f64, f64)], x: f64) -> f64 {
    let (first, last) = (points[0], points[points.len() - 1]);
    if x <= first.0 {
        return (x - first.0) / first.1;
    }
    let mut total = 0.0;
    for pair in points.windows(2) {
        let ((a, sa), (b, sb)) = (pair[0], pair[1]);
        // Two breakpoints at one time make a step, which takes no time.
        if b <= a {
            continue;
        }
        if x <= a {
            break;
        }
        let to = x.min(b);
        // The speed at `to` along the straight line from `sa` to `sb`.
        let s_to = sa + (sb - sa) * (to - a) / (b - a);
        total += if (s_to - sa).abs() < 1e-12 {
            (to - a) / sa
        } else {
            (to - a) * (s_to / sa).ln() / (s_to - sa)
        };
    }
    if x > last.0 {
        total += (x - last.0) / last.1;
    }
    total
}

/// The nearest line of the grid, laid out from each bar's start (the piano
/// roll spec, R18), half up; the last line of a bar is the next bar's start.
fn nearest_line(at: Ticks, step: Ticks, bar: Ticks) -> Ticks {
    let bar_start = Ticks(at.0.div_euclid(bar.0) * bar.0);
    let line = |n: i64| (bar_start + Ticks(n * step.0)).min(bar_start + bar);
    let n = (at - bar_start).0 / step.0;
    let (before, after) = (line(n), line(n + 1));
    if after - at <= at - before {
        after
    } else {
        before
    }
}

/// The last grid line before `at`.
fn line_before(at: Ticks, step: Ticks, bar: Ticks) -> Ticks {
    let bar_start = Ticks((at.0 - 1).div_euclid(bar.0) * bar.0);
    bar_start + Ticks((at - Ticks(1) - bar_start).0 / step.0 * step.0)
}

/// The first grid line after `at`.
fn line_after(at: Ticks, step: Ticks, bar: Ticks) -> Ticks {
    let bar_start = Ticks(at.0.div_euclid(bar.0) * bar.0);
    let n = (at - bar_start).0 / step.0 + 1;
    (bar_start + Ticks(n * step.0)).min(bar_start + bar)
}

impl MidiTool for TimeWarp {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn validate(&self, values: &[ParamValue]) -> Result<(), ToolError> {
        if curve(values).is_empty() {
            return Err(ToolError::Values(
                "Time Warp keeps at least one breakpoint on".to_owned(),
            ));
        }
        Ok(())
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let points = curve(values);
        if points.is_empty() {
            return self.validate(values).map(|()| ToolOutput::default());
        }
        let (quantize, preserve, include_end) = (
            switch(values, QUANTIZE),
            switch(values, QUANTIZE + 1),
            switch(values, QUANTIZE + 2),
        );
        let (from, to) = input.range;
        let span = (to - from).0 as f64;
        let origin = integral(&points, 0.0);
        let whole_range = integral(&points, 1.0) - origin;
        let step = input.grid_step.max(Ticks(1));
        let bar = input.signature.bar_length();
        // Where a point of the contents goes: a fraction U of the way through
        // the range, then on to the grid where Quantize asks (R32, R33).
        let warp = |at: Ticks| {
            let u = (at - from).0 as f64 / span;
            let mut moved = integral(&points, u) - origin;
            if preserve {
                moved /= whole_range;
            }
            from + round_ticks(moved * span)
        };
        let mut notes = Vec::with_capacity(input.notes.len());
        for note in &input.notes {
            let mut out: ToolNote = note.clone();
            if span > 0.0 {
                let mut start = warp(note.start);
                // A note that started in the range stays in it (R32), on the
                // grid where it can.
                let inside = preserve && note.start >= from && note.start < to;
                if inside {
                    start = start.min(to - Ticks(1));
                }
                if quantize {
                    start = nearest_line(start, step, bar);
                    if inside && start >= to {
                        start = line_before(to, step, bar);
                    }
                    if inside && start < from {
                        start = line_after(from - Ticks(1), step, bar);
                    }
                    if inside {
                        start = start.clamp(from, to - Ticks(1));
                    }
                }
                let mut end = if include_end {
                    warp(note.end())
                } else {
                    start + note.length
                };
                if include_end && quantize {
                    end = nearest_line(end, step, bar);
                    if end <= start {
                        end = line_after(start, step, bar);
                    }
                }
                out.start = start;
                out = out.stretched((end - start).max(Ticks(1)));
            }
            notes.push(out);
        }
        Ok(ToolOutput { notes })
    }
}
