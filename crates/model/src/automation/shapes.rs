//! Editing an envelope over a span of time (WP-6.25b): inserting a shape,
//! stretching the values or the time of the breakpoints in a span, skewing
//! them, and simplifying them to fewer breakpoints.
//!
//! The edits work on a parameter's travel, from 0 at the bottom of its
//! control to 1 at the top, so that a shape or a stretch looks the same
//! on a fader, a knob and a modulation range alike: a [`ParamScale`] maps the
//! parameter's values to and from the travel. The rules are in the
//! automation editing spec, R11 to R19.

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Breakpoint, Envelope, curve_value, envelope_value};

/// How closely an inserted shape's breakpoints follow it, as a share of the
/// travel. The spec promises 1% (R12); this keeps half of that in reserve
/// for what lies between the ticks that are checked.
const SHAPE_TOLERANCE: f64 = 0.005;

/// How closely a simplified envelope follows the original, as a share of the
/// travel. The spec promises 0.5% (R18); this keeps half of that in reserve.
const SIMPLIFY_TOLERANCE: f64 = 0.0025;

/// How many times a shape's segment is halved at most, so a parameter that
/// no breakpoints can follow (a switch) gets a bounded number of them.
const MAX_SPLITS: u32 = 10;

/// How a parameter's values sit on its control's travel.
pub trait ParamScale {
    /// Where a value sits on the travel, from 0 (the bottom) to 1 (the top).
    fn position(&self, value: f64) -> f64;

    /// The value at a place on the travel, from 0 (the bottom) to 1 (the
    /// top).
    fn value(&self, position: f64) -> f64;
}

/// A travel that runs straight from `min` to `max`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearRange {
    /// The value at the bottom.
    pub min: f64,
    /// The value at the top.
    pub max: f64,
}

impl ParamScale for LinearRange {
    fn position(&self, value: f64) -> f64 {
        if self.max > self.min {
            ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.5
        }
    }

    fn value(&self, position: f64) -> f64 {
        self.min + (self.max - self.min) * position.clamp(0.0, 1.0)
    }
}

/// One of the ready-made shapes a time selection's menu inserts (the
/// automation editing spec, R11 to R13).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "An automation shape: the waveforms `sine`, `triangle`, `sawtooth`, \
                   `inverse_sawtooth` and `square`, which run one cycle across the span \
                   between the bottom and top of the parameter's range; the ramps `rise` \
                   (from the value at the span's start to the top), `fall` (to the bottom), \
                   `rise_to` (from the bottom to the value at the span's end) and `fall_to` \
                   (from the top); and `adsr`."
)]
pub enum Shape {
    /// One sine cycle, from the middle, rising first.
    Sine,
    /// Bottom to top at the span's middle and back.
    Triangle,
    /// Bottom rising to top.
    Sawtooth,
    /// Top falling to bottom.
    InverseSawtooth,
    /// Top for the first half, bottom for the second.
    Square,
    /// The value at the span's start, rising to the top.
    Rise,
    /// The value at the span's start, falling to the bottom.
    Fall,
    /// The bottom, rising to the value at the span's end.
    RiseTo,
    /// The top, falling to the value at the span's end.
    FallTo,
    /// Attack, decay, sustain and release.
    Adsr,
}

impl Shape {
    /// The shapes in the order the menu shows them: the waveforms, then the
    /// ramps and the ADSR.
    pub const ALL: [Self; 10] = [
        Self::Sine,
        Self::Triangle,
        Self::Sawtooth,
        Self::InverseSawtooth,
        Self::Square,
        Self::Rise,
        Self::Fall,
        Self::RiseTo,
        Self::FallTo,
        Self::Adsr,
    ];

    /// The shape's name, which assistive technology reads (R11).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sine => "Sine",
            Self::Triangle => "Triangle",
            Self::Sawtooth => "Sawtooth",
            Self::InverseSawtooth => "Inverse Sawtooth",
            Self::Square => "Square",
            Self::Rise => "Rise",
            Self::Fall => "Fall",
            Self::RiseTo => "Rise To",
            Self::FallTo => "Fall To",
            Self::Adsr => "ADSR",
        }
    }

    /// Where the shape sits on the travel, `along` of the way through the
    /// span, 0 to 1, given the positions of the envelope's value at the
    /// span's start (`before`) and end (`after`). A square isn't continuous
    /// and is laid out by tick instead.
    #[must_use]
    pub fn level(self, along: f64, before: f64, after: f64) -> f64 {
        let line = |from: f64, to: f64, a: f64, b: f64| {
            from + (to - from) * ((along - a) / (b - a)).clamp(0.0, 1.0)
        };
        match self {
            Self::Sine => 0.5 + 0.5 * (std::f64::consts::TAU * along).sin(),
            Self::Triangle => 1.0 - (2.0 * along - 1.0).abs(),
            Self::Sawtooth => along,
            Self::InverseSawtooth => 1.0 - along,
            Self::Square => {
                if along < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Rise => line(before, 1.0, 0.0, 1.0),
            Self::Fall => line(before, 0.0, 0.0, 1.0),
            Self::RiseTo => line(0.0, after, 0.0, 1.0),
            Self::FallTo => line(1.0, after, 0.0, 1.0),
            Self::Adsr => {
                if along < 0.125 {
                    line(before, 1.0, 0.0, 0.125)
                } else if along < 0.25 {
                    line(1.0, 0.5, 0.125, 0.25)
                } else if along <= 0.75 {
                    0.5
                } else {
                    line(0.5, after, 0.75, 1.0)
                }
            }
        }
    }

    /// The places along the span, 0 to 1, where a continuous shape bends,
    /// so the fit starts a segment on each side of them.
    fn knees(self) -> &'static [f64] {
        match self {
            Self::Sine => &[0.0, 0.25, 0.5, 0.75, 1.0],
            Self::Triangle => &[0.0, 0.5, 1.0],
            Self::Adsr => &[0.0, 0.125, 0.25, 0.75, 1.0],
            _ => &[0.0, 1.0],
        }
    }
}

/// The breakpoints that follow `level`, a function of the tick from `from`
/// to `to` giving the shape's place on the travel, within
/// [`SHAPE_TOLERANCE`] of it at the ticks between: the segment is halved
/// until its straight line between two values follows the shape.
fn fit_curve(
    level: &impl Fn(i64) -> f64,
    scale: &(impl ParamScale + ?Sized),
    from: i64,
    to: i64,
    splits: u32,
    out: &mut Vec<Breakpoint>,
) {
    if to - from >= 2 && splits < MAX_SPLITS {
        let (a, b) = (scale.value(level(from)), scale.value(level(to)));
        let samples = (to - from - 1).min(31);
        let off = (1..=samples).any(|k| {
            let at = from + (to - from) * k / (samples + 1);
            let along = (at - from) as f64 / (to - from) as f64;
            (scale.position(curve_value(a, b, along, 0.0)) - level(at)).abs() > SHAPE_TOLERANCE
        });
        if off {
            let middle = from + (to - from) / 2;
            fit_curve(level, scale, from, middle, splits + 1, out);
            fit_curve(level, scale, middle, to, splits + 1, out);
            return;
        }
    }
    out.push(Breakpoint {
        at: Ticks(to),
        value: scale.value(level(to)),
        bend: 0.0,
    });
}

/// A shape's breakpoints from tick `start` to tick `last`, the shape's first
/// and last ticks, given the positions on the travel of the envelope's value
/// before the span and after it.
fn shape_points(
    shape: Shape,
    start: i64,
    last: i64,
    before: f64,
    after: f64,
    scale: &(impl ParamScale + ?Sized),
) -> Vec<Breakpoint> {
    let at = |tick: i64, position: f64| Breakpoint {
        at: Ticks(tick),
        value: scale.value(position),
        bend: 0.0,
    };
    if shape == Shape::Square {
        // The top for the first half of the span's ticks, the bottom for the
        // second.
        let half = (last - start + 1) / 2;
        let mut points = vec![at(start, 1.0)];
        if half > 1 {
            points.push(at(start + half - 1, 1.0));
        }
        points.push(at(start + half, 0.0));
        if start + half < last {
            points.push(at(last, 0.0));
        }
        points.dedup_by_key(|point| point.at);
        return points;
    }
    let length = (last - start).max(1);
    let level = |tick: i64| shape.level((tick - start) as f64 / length as f64, before, after);
    let mut points = vec![at(start, level(start))];
    if last > start {
        let knees: Vec<i64> = shape
            .knees()
            .iter()
            .map(|knee| start + (knee * length as f64).round() as i64)
            .collect();
        for pair in knees.windows(2) {
            fit_curve(&level, scale, pair[0], pair[1], 0, &mut points);
        }
    }
    points
}

impl Envelope {
    /// Puts `point` in its place, replacing the breakpoint at its position.
    fn put(&mut self, point: Breakpoint) {
        match self.points.binary_search_by_key(&point.at, |p| p.at) {
            Ok(index) => self.points[index] = point,
            Err(index) => self.points.insert(index, point),
        }
    }

    /// Writes `shape` over the span from `start` to `end`, replacing the
    /// envelope there (the automation editing spec, R11 to R13). The shape
    /// runs from tick `start` to the tick before `end`, and the envelope
    /// outside the span stays as it was: where the shape's value at an edge
    /// differs from the envelope's, a breakpoint one tick away holds the
    /// envelope's value, so the step takes a single tick. `rest` is the
    /// value the ramps join where the envelope has none.
    pub fn insert_shape(
        &mut self,
        shape: Shape,
        start: Ticks,
        end: Ticks,
        rest: f64,
        scale: &(impl ParamScale + ?Sized),
    ) {
        if end <= start {
            return;
        }
        let value_at = |envelope: &Self, at: Ticks| envelope.value_at(at).unwrap_or(rest);
        let (before, after) = (value_at(self, start), value_at(self, end));
        let points = shape_points(
            shape,
            start.0,
            end.0 - 1,
            scale.position(before),
            scale.position(after),
            scale,
        );
        // What holds the envelope in place outside the span, worked out
        // before the span goes. An empty envelope has nothing to hold.
        let holds = !self.is_empty();
        let before_start = start - Ticks(1);
        let held_before = Breakpoint {
            at: before_start,
            value: value_at(self, before_start),
            bend: 0.0,
        };
        // The segment that crosses `end` keeps its bend from there on.
        let held_after = Breakpoint {
            at: end,
            value: after,
            bend: self
                .points
                .iter()
                .rfind(|point| point.at < end)
                .map_or(0.0, |point| point.bend),
        };
        let first = points.first().map_or(0.0, |point| point.value);
        let keep_end = self.point(end).is_none();
        self.points
            .retain(|point| point.at < start || point.at >= end);
        if holds
            && start > Ticks::ZERO
            && (scale.position(held_before.value) - scale.position(first)).abs() > f64::EPSILON
            && self.point(before_start).is_none()
        {
            self.put(held_before);
        }
        for point in points {
            self.put(point);
        }
        if holds && keep_end {
            self.put(held_after);
        }
    }

    /// Moves the breakpoints from `start` up to, not including, `end` to
    /// the same place between two edges, which run from `bottom.0` to
    /// `bottom.1` and from `top.0` to `top.1` as shares of the travel at the
    /// span's start and end: the stretch of values when the edges are
    /// level, the skew when they tilt (the automation editing spec, R15,
    /// R17). A value the edges carry past the travel is clipped to it.
    pub fn remap_span(
        &mut self,
        start: Ticks,
        end: Ticks,
        bottom: (f64, f64),
        top: (f64, f64),
        scale: &(impl ParamScale + ?Sized),
    ) {
        if end <= start {
            return;
        }
        let length = (end - start).0 as f64;
        for point in &mut self.points {
            if point.at < start || point.at >= end {
                continue;
            }
            let along = (point.at - start).0 as f64 / length;
            let low = bottom.0 + (bottom.1 - bottom.0) * along;
            let high = top.0 + (top.1 - top.0) * along;
            let position = scale.position(point.value);
            let moved = (low + position * (high - low)).clamp(0.0, 1.0);
            // A breakpoint that doesn't move keeps its own value: the way
            // back from a position isn't quite the value it came from.
            if (moved - position).abs() > 1e-12 {
                point.value = scale.value(moved);
            }
        }
    }

    /// Stretches the breakpoints from `start` to `end`, both included, into
    /// the span from `to_start` to `to_end` (the automation editing spec,
    /// R16). A breakpoint outside the span that a moving end passes over is
    /// removed; with `carry`, the breakpoints beyond an end move with it
    /// instead, keeping their distance from it, and those that would pass
    /// time zero are removed.
    pub fn stretch_span(
        &mut self,
        start: Ticks,
        end: Ticks,
        to_start: Ticks,
        to_end: Ticks,
        carry: bool,
    ) {
        if end <= start || to_end <= to_start {
            return;
        }
        let (length, to_length) = (
            i128::from((end - start).0),
            i128::from((to_end - to_start).0),
        );
        let mut kept: std::collections::BTreeMap<Ticks, Breakpoint> =
            std::collections::BTreeMap::new();
        for point in &self.points {
            let at = if point.at < start {
                if carry {
                    point.at + (to_start - start)
                } else if point.at >= to_start {
                    continue;
                } else {
                    point.at
                }
            } else if point.at > end {
                if carry {
                    point.at + (to_end - end)
                } else if point.at <= to_end {
                    continue;
                } else {
                    point.at
                }
            } else {
                let offset = i128::from((point.at - start).0) * to_length;
                to_start + Ticks(((offset + length / 2) / length) as i64)
            };
            if at >= Ticks::ZERO {
                kept.insert(at, Breakpoint { at, ..*point });
            }
        }
        self.points = kept.into_values().collect();
    }

    /// Replaces the breakpoints from `start` up to, not including, `end`
    /// with as few as keep the envelope within [`SIMPLIFY_TOLERANCE`] of
    /// what it was, using curved segments where one follows the shape (the
    /// automation editing spec, R18). The first and last of them stay, with
    /// the last one's bend, which shapes the segment leaving the span; no
    /// breakpoint is added, and the envelope outside the span is unchanged.
    pub fn simplify_span(&mut self, start: Ticks, end: Ticks, scale: &(impl ParamScale + ?Sized)) {
        let first = self.points.partition_point(|point| point.at < start);
        let last = self.points.partition_point(|point| point.at < end);
        if last < first + 3 {
            return;
        }
        let points = &self.points[first..last];
        let mut keep = vec![false; points.len()];
        let mut bends = vec![0.0; points.len()];
        keep[0] = true;
        keep[points.len() - 1] = true;
        simplify_between(
            &self.points,
            first,
            0,
            points.len() - 1,
            scale,
            &mut keep,
            &mut bends,
        );
        let simplified: Vec<Breakpoint> = points
            .iter()
            .zip(keep.iter().zip(&bends))
            .enumerate()
            .filter(|(_, (_, (keep, _)))| **keep)
            .map(|(index, (point, (_, bend)))| Breakpoint {
                bend: if index == points.len() - 1 {
                    point.bend
                } else {
                    *bend
                },
                ..*point
            })
            .collect();
        self.points.splice(first..last, simplified);
    }
}

/// Decides which of the original breakpoints `from` to `to` (indices into
/// the span that starts at `base` in `all`) stay: if one segment between
/// the two, straight or bent, keeps within tolerance, none of those between
/// them do; otherwise the one furthest from the chord stays, and each half
/// is decided the same way.
fn simplify_between(
    all: &[Breakpoint],
    base: usize,
    from: usize,
    to: usize,
    scale: &(impl ParamScale + ?Sized),
    keep: &mut [bool],
    bends: &mut [f64],
) {
    let points = &all[base..];
    if to == from + 1 {
        bends[from] = points[from].bend;
        return;
    }
    let (a, b) = (points[from], points[to]);
    // The ticks checked: each original breakpoint, and eight more inside
    // each of the original segments.
    let mut ticks: Vec<i64> = Vec::new();
    for k in from..to {
        let (x, y) = (points[k].at.0, points[k + 1].at.0);
        ticks.extend((0..9).map(|i| x + (y - x) * i / 9));
    }
    let along = |tick: i64| (tick - a.at.0) as f64 / (b.at.0 - a.at.0) as f64;
    let original = |tick: i64| envelope_value(all, tick as f64).unwrap_or(0.0);
    let error = |bend: f64| {
        ticks
            .iter()
            .map(|&tick| {
                let line = curve_value(a.value, b.value, along(tick), bend);
                (scale.position(line) - scale.position(original(tick))).abs()
            })
            .fold(0.0, f64::max)
    };
    if error(0.0) <= SIMPLIFY_TOLERANCE {
        bends[from] = 0.0;
        return;
    }
    // The bend that takes the curve through the original where the straight
    // line misses it most: `along^(2^bend)` is that share of the way up.
    let worst = ticks.iter().copied().max_by(|&x, &y| {
        let miss = |tick: i64| {
            let line = curve_value(a.value, b.value, along(tick), 0.0);
            (scale.position(line) - scale.position(original(tick))).abs()
        };
        miss(x).total_cmp(&miss(y))
    });
    if let Some(tick) = worst
        && b.value != a.value
    {
        let share = (original(tick) - a.value) / (b.value - a.value);
        let along = along(tick);
        if share > 0.0 && share < 1.0 && along > 0.0 && along < 1.0 {
            let bend = (share.ln() / along.ln()).log2().clamp(-1.0, 1.0);
            if error(bend) <= SIMPLIFY_TOLERANCE {
                bends[from] = bend;
                return;
            }
        }
    }
    // Split at the breakpoint furthest from the chord.
    let split = (from + 1..to)
        .max_by(|&x, &y| {
            let miss = |k: usize| {
                let line = curve_value(a.value, b.value, along(points[k].at.0), 0.0);
                (scale.position(line) - scale.position(points[k].value)).abs()
            };
            miss(x).total_cmp(&miss(y))
        })
        .unwrap_or(from + 1);
    keep[split] = true;
    simplify_between(all, base, from, split, scale, keep, bends);
    simplify_between(all, base, split, to, scale, keep, bends);
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RANGE: LinearRange = LinearRange { min: 0.0, max: 1.0 };

    /// A fader-like travel, which isn't straight.
    struct Curved;

    impl ParamScale for Curved {
        fn position(&self, value: f64) -> f64 {
            value.clamp(0.0, 1.0).sqrt()
        }
        fn value(&self, position: f64) -> f64 {
            position.clamp(0.0, 1.0).powi(2)
        }
    }

    fn envelope(points: &[(i64, f64)]) -> Envelope {
        let mut envelope = Envelope::default();
        for &(at, value) in points {
            envelope.set(Ticks(at), value);
        }
        envelope
    }

    /// The ticks to check in a span: a few hundred spread across it, with
    /// its edges.
    fn ticks(start: i64, end: i64) -> impl Iterator<Item = i64> {
        let step = ((end - start) / 997).max(1);
        (start..end).step_by(step as usize).chain([start, end - 1])
    }

    /// The position the shape has at a tick, by its definition.
    fn definition(shape: Shape, tick: i64, start: i64, end: i64, before: f64, after: f64) -> f64 {
        if shape == Shape::Square {
            return if tick - start < (end - start) / 2 {
                1.0
            } else {
                0.0
            };
        }
        shape.level(
            (tick - start) as f64 / (end - 1 - start) as f64,
            before,
            after,
        )
    }

    fn any_shape() -> impl Strategy<Value = Shape> {
        prop::sample::select(Shape::ALL.to_vec())
    }

    // Spec: automation-editing R12, R13.
    proptest! {
        #[test]
        fn a_shape_stays_within_one_percent_of_its_definition_and_leaves_the_rest(
            shape in any_shape(),
            start in 1_000i64..5_000_000,
            length in 2_000i64..4_000_000,
            low in -100.0f64..100.0,
            width in 0.5f64..500.0,
            curved in any::<bool>(),
            points in prop::collection::vec((0i64..12_000_000, 0.0f64..1.0), 0..8),
        ) {
            let end = start + length;
            let linear = LinearRange { min: low, max: low + width };
            let scale: &dyn ParamScale = if curved { &Curved } else { &linear };
            // Values on the scale's own range: the curved travel runs 0 to 1.
            let (min, max) = if curved { (0.0, 1.0) } else { (low, low + width) };
            let mut original = Envelope::default();
            for (at, share) in points {
                original.set(Ticks(at), min + (max - min) * share);
            }
            let mut edited = original.clone();
            edited.insert_shape(shape, Ticks(start), Ticks(end), min, scale);
            let before = original.value_at(Ticks(start)).map_or(0.0, |v| scale.position(v));
            let after = original.value_at(Ticks(end)).map_or(0.0, |v| scale.position(v));
            for tick in ticks(start, end) {
                let value = edited.value_at(Ticks(tick)).unwrap();
                let want = definition(shape, tick, start, end, before, after);
                prop_assert!(
                    (scale.position(value) - want).abs() <= 0.01,
                    "{shape:?} at {tick}: {} against {want}", scale.position(value)
                );
            }
            // Outside the span, within the tick that holds each edge; an empty
            // envelope has nothing outside to keep.
            for tick in ticks(0, start - 1).chain(ticks(end + 1, 14_000_000)).filter(|_| !original.is_empty()) {
                let (now, was) = (edited.value_at(Ticks(tick)).unwrap(), original.value_at(Ticks(tick)).unwrap());
                prop_assert!(
                    (scale.position(now) - scale.position(was)).abs() < 1e-9,
                    "{shape:?} changed {tick}: {now} against {was}"
                );
            }
        }
    }

    // Spec: automation-editing R18.
    proptest! {
        #[test]
        fn simplifying_stays_within_half_a_percent_adds_nothing_and_keeps_the_edges(
            gaps in prop::collection::vec(1i64..400_000, 3..120),
            shares in prop::collection::vec(0.0f64..1.0, 120),
            bends in prop::collection::vec(-1.0f64..1.0, 120),
            curved in any::<bool>(),
        ) {
            let scale: &dyn ParamScale = if curved { &Curved } else { &RANGE };
            let mut at = 1_000;
            let mut original = Envelope::default();
            for (index, gap) in gaps.iter().enumerate() {
                at += gap;
                original.set(Ticks(at), shares[index]);
            }
            let ats: Vec<Ticks> = original.points().map(|p| p.at).collect();
            for (index, tick) in ats.iter().enumerate() {
                original.set_bend(*tick, if index + 1 < ats.len() { bends[index] } else { 0.0 }).ok();
            }
            let (start, end) = (ats[0], *ats.last().unwrap() + Ticks(1));
            let mut simplified = original.clone();
            simplified.simplify_span(start, end, scale);
            prop_assert!(simplified.points().len() <= original.points().len());
            prop_assert_eq!(simplified.point(start).map(|p| p.value), original.point(start).map(|p| p.value));
            let last = *ats.last().unwrap();
            prop_assert_eq!(simplified.point(last).map(|p| p.value), original.point(last).map(|p| p.value));
            for tick in ticks(0, end.0 + 1_000) {
                let (a, b) = (
                    simplified.value_at(Ticks(tick)).unwrap(),
                    original.value_at(Ticks(tick)).unwrap(),
                );
                prop_assert!(
                    (scale.position(a) - scale.position(b)).abs() <= 0.005,
                    "at {tick}: {a} against {b}"
                );
            }
        }
    }

    // Spec: automation-editing R15, R17.
    proptest! {
        #[test]
        fn stretching_and_skewing_map_each_breakpoint_between_the_edges(
            shares in prop::collection::vec((0.0f64..1.0, 0.0f64..1.0), 1..12),
            bottom in (-0.5f64..1.0, -0.5f64..1.0),
            gap in (0.0f64..1.5, 0.0f64..1.5),
        ) {
            let (start, end) = (Ticks(10_000), Ticks(10_000_000));
            let top = (bottom.0 + gap.0, bottom.1 + gap.1);
            let mut original = Envelope::default();
            for (along, share) in &shares {
                original.set(start + Ticks(((end - start).0 as f64 * along * 0.999) as i64), *share);
            }
            let mut edited = original.clone();
            edited.remap_span(start, end, bottom, top, &RANGE);
            for (was, now) in original.points().zip(edited.points()) {
                let along = (was.at - start).0 as f64 / (end - start).0 as f64;
                let low = bottom.0 + (bottom.1 - bottom.0) * along;
                let high = top.0 + (top.1 - top.0) * along;
                let want = (low + was.value * (high - low)).clamp(0.0, 1.0);
                prop_assert!((now.value - want).abs() < 1e-9, "{} against {want}", now.value);
                prop_assert_eq!(now.at, was.at);
            }
        }
    }

    // Spec: automation-editing R16.
    #[test]
    fn stretching_in_time_maps_the_span_and_removes_what_the_end_passes() {
        let mut stretched = envelope(&[(0, 0.1), (100, 0.2), (200, 0.3), (300, 0.4), (400, 0.5)]);
        // The right end goes from 300 to 450, past the point at 400.
        stretched.stretch_span(Ticks(100), Ticks(300), Ticks(100), Ticks(450), false);
        assert_eq!(
            stretched
                .points()
                .map(|p| (p.at.0, p.value))
                .collect::<Vec<_>>(),
            vec![(0, 0.1), (100, 0.2), (275, 0.3), (450, 0.4)]
        );
        // With Shift the point beyond keeps its distance from the end.
        let mut carried = envelope(&[(0, 0.1), (100, 0.2), (200, 0.3), (300, 0.4), (400, 0.5)]);
        carried.stretch_span(Ticks(100), Ticks(300), Ticks(100), Ticks(450), true);
        assert_eq!(
            carried
                .points()
                .map(|p| (p.at.0, p.value))
                .collect::<Vec<_>>(),
            vec![(0, 0.1), (100, 0.2), (275, 0.3), (450, 0.4), (550, 0.5)]
        );
        // The left end goes in: the point before it stays.
        let mut shrunk = envelope(&[(0, 0.1), (100, 0.2), (200, 0.3), (300, 0.4)]);
        shrunk.stretch_span(Ticks(100), Ticks(300), Ticks(200), Ticks(300), false);
        assert_eq!(
            shrunk
                .points()
                .map(|p| (p.at.0, p.value))
                .collect::<Vec<_>>(),
            vec![(0, 0.1), (200, 0.2), (250, 0.3), (300, 0.4)]
        );
    }

    // Spec: automation-editing R12.
    #[test]
    fn a_sawtooth_on_a_straight_range_is_two_breakpoints_and_a_sine_follows_its_cycle() {
        let mut saw = Envelope::default();
        saw.insert_shape(Shape::Sawtooth, Ticks(1_000), Ticks(2_000), 0.0, &RANGE);
        let points: Vec<_> = saw.points().collect();
        assert_eq!(points.len(), 2);
        assert_eq!((points[0].at.0, points[0].value), (1_000, 0.0));
        assert_eq!((points[1].at.0, points[1].value), (1_999, 1.0));
        let mut sine = Envelope::default();
        sine.insert_shape(Shape::Sine, Ticks(0), Ticks(40_001), 0.5, &RANGE);
        assert!((sine.value_at(Ticks(0)).unwrap() - 0.5).abs() < 1e-9);
        assert!(sine.value_at(Ticks(10_000)).unwrap() > 0.99);
        assert!(sine.value_at(Ticks(30_000)).unwrap() < 0.01);
    }

    // Spec: automation-editing R13.
    #[test]
    fn the_ramps_join_the_envelope_outside_the_span() {
        let mut rise = envelope(&[(0, 0.2), (10_000, 0.6)]);
        rise.insert_shape(Shape::Rise, Ticks(2_000), Ticks(4_000), 0.0, &RANGE);
        let at = |tick: i64| rise.value_at(Ticks(tick)).unwrap();
        // From the envelope's value at the span's start to the top at its
        // end, and the envelope's own value one tick later.
        assert!((at(2_000) - 0.2 - 0.4 * 0.2).abs() < 1e-9);
        assert!((at(3_999) - 1.0).abs() < 1e-9);
        assert!((at(4_000) - (0.2 + 0.4 * 0.4)).abs() < 1e-9);
        let mut adsr = envelope(&[(0, 0.2), (10_000, 0.2)]);
        adsr.insert_shape(Shape::Adsr, Ticks(1_000), Ticks(9_001), 0.0, &RANGE);
        assert!((adsr.value_at(Ticks(2_000)).unwrap() - 1.0).abs() < 1e-9);
        assert!((adsr.value_at(Ticks(5_000)).unwrap() - 0.5).abs() < 1e-9);
        assert!((adsr.value_at(Ticks(9_000)).unwrap() - 0.2).abs() < 1e-9);
    }

    // Spec: automation-editing R18.
    #[test]
    fn a_recorded_sweep_of_hundreds_of_breakpoints_needs_a_few_dozen() {
        let mut sweep = Envelope::default();
        for k in 0..400 {
            let along = f64::from(k) / 399.0;
            sweep.set(Ticks(k * 1_000), 0.5 + 0.4 * (along * 5.0).sin());
        }
        let original = sweep.clone();
        sweep.simplify_span(Ticks(0), Ticks(400_000), &RANGE);
        assert!(sweep.points().len() < 40, "{}", sweep.points().len());
        for tick in (0..399_000).step_by(37) {
            let (a, b) = (
                sweep.value_at(Ticks(tick)).unwrap(),
                original.value_at(Ticks(tick)).unwrap(),
            );
            assert!((a - b).abs() <= 0.005, "at {tick}: {a} against {b}");
        }
    }

    // Spec: automation-editing R18.
    #[test]
    fn simplifying_a_straight_run_leaves_its_ends_and_a_bent_one_gets_a_bend() {
        let mut line = envelope(&[(0, 0.0), (100, 0.25), (200, 0.5), (300, 0.75), (400, 1.0)]);
        line.simplify_span(Ticks(0), Ticks(401), &RANGE);
        assert_eq!(line.points().len(), 2);
        // Points on a power curve become one bent segment.
        let mut bent = Envelope::default();
        for k in 0..=20 {
            bent.set(
                Ticks(k * 1_000),
                (f64::from(k as i32) / 20.0).powf(2f64.powf(0.5)),
            );
        }
        bent.simplify_span(Ticks(0), Ticks(20_001), &RANGE);
        let points: Vec<_> = bent.points().collect();
        assert_eq!(points.len(), 2);
        assert!((points[0].bend - 0.5).abs() < 0.05, "{}", points[0].bend);
    }
}
