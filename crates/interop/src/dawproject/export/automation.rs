//! Automation, tempo and time signatures (the DAWproject spec, R75, R76):
//! each envelope as the points of its track's lane, curves broken into
//! straight segments, steps as holds, and every value in the unit the
//! parameter is written in.

use bl_core::time::{TempoSegment, Ticks};
use bl_model::automation::curve_value;
use bl_model::param::db_to_gain;
use bl_model::{Breakpoint, ParamKind};

use super::*;
use crate::dawproject::generic::{self, Quantity};
use crate::dawproject::model::{
    Expression, Interpolation, Marker, Markers, PointValue, Points, Target, Timeline,
};

/// How far a straight segment may stray from the curve it stands for, as a
/// share of the parameter's range (R76).
const TOLERANCE: f64 = 0.01;

/// The most points a curved segment breaks into at sixteenth notes; a longer
/// one is broken at wider steps.
const MAX_POINTS: i64 = 2048;

/// How a parameter's value is written (R76).
#[derive(Clone, Debug)]
pub(super) enum Conversion {
    /// A track's volume, as a linear gain.
    Volume,
    /// A track's pan, normalised: 0 hard left, 1 hard right.
    Pan,
    /// A track's Track Activator, as a boolean.
    Mute,
    /// A send's level, as a linear gain.
    Send,
    /// A parameter of one of Beatlayer's own devices, as its generic device
    /// writes it, or in its own units. `range` is the parameter's.
    Native {
        quantity: Quantity,
        range: Option<(f64, f64)>,
    },
    /// A plug-in's parameter, normalised through the plug-in's own mapping.
    Plugin(ParamKind),
}

impl Conversion {
    /// Whether the points are booleans.
    pub(super) fn is_bool(&self) -> bool {
        matches!(
            self,
            Self::Mute
                | Self::Native {
                    quantity: Quantity::Toggle,
                    ..
                }
                | Self::Plugin(ParamKind::Toggle)
        )
    }

    /// The unit the points are in, or `None` for booleans.
    fn unit(&self) -> Option<Unit> {
        if self.is_bool() {
            return None;
        }
        Some(match self {
            Self::Volume | Self::Send => Unit::Linear,
            Self::Pan | Self::Plugin(_) => Unit::Normalized,
            Self::Native { quantity, range } => {
                generic::to_file(*quantity, 0.0, range.unwrap_or((0.0, 1.0))).1
            }
            Self::Mute => return None,
        })
    }

    /// The value as the file writes it.
    fn value(&self, value: f64) -> f64 {
        match self {
            Self::Volume | Self::Send | Self::Mute => value,
            Self::Pan => (value + 1.0) / 2.0,
            Self::Native { quantity, range } => {
                generic::to_file(*quantity, value, range.unwrap_or((f64::MIN, f64::MAX))).0
            }
            Self::Plugin(kind) => normalised(kind, value),
        }
    }

    /// The width of the parameter's range, which a segment's error is
    /// measured against, or `None` where it isn't known.
    fn span(&self) -> Option<f64> {
        match self {
            Self::Volume => Some(bl_model::fader::db_gain(bl_model::fader::TOP_DB)),
            Self::Pan => Some(2.0),
            Self::Send => Some(1.0),
            Self::Mute => Some(1.0),
            Self::Native { range, .. } => range.map(|(low, high)| high - low),
            Self::Plugin(kind) => match kind {
                ParamKind::Continuous { min, max, .. } => Some(max - min),
                ParamKind::Integer { min, max, .. } => Some((*max - *min) as f64),
                ParamKind::Gain { max_db, .. } => Some(db_to_gain(*max_db)),
                ParamKind::Choice { choices } => Some(choices.len().saturating_sub(1) as f64),
                ParamKind::Toggle => Some(1.0),
            },
        }
    }
}

/// A plug-in parameter's plain value as its normalised one, over the range
/// the host maps it across (R57 turned round).
fn normalised(kind: &ParamKind, plain: f64) -> f64 {
    let share = |low: f64, high: f64| {
        if high > low {
            ((plain - low) / (high - low)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    match kind {
        ParamKind::Continuous { min, max, .. } => share(*min, *max),
        ParamKind::Integer { min, max, .. } => share(*min as f64, *max as f64),
        ParamKind::Gain { max_db, .. } => share(0.0, db_to_gain(*max_db)),
        ParamKind::Toggle => f64::from(u8::from(plain > 0.5)),
        ParamKind::Choice { choices } => share(0.0, choices.len().saturating_sub(1) as f64),
    }
}

/// The beats a tick count is.
pub(super) fn beats(ticks: Ticks) -> f64 {
    ticks.get() as f64 / Ticks::QUARTER.get() as f64
}

/// One point to write: where, the value in the parameter's own units, and
/// whether the value holds to the next point.
type Step = (Ticks, f64, bool);

/// An envelope's breakpoints as the points that stand for it (R76): a
/// straight segment as it is, a curved one as straight segments at every
/// sixteenth note and more often where the curve moves more than
/// `tolerance` between two of them, and a step, two breakpoints a tick
/// apart, as a point that holds.
pub(super) fn straighten(points: &[Breakpoint], tolerance: f64) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::new();
    for (index, a) in points.iter().enumerate() {
        let Some(b) = points.get(index + 1) else {
            out.push((a.at, a.value, true));
            break;
        };
        if b.at - a.at <= Ticks(1) && b.value != a.value {
            out.push((a.at, a.value, true));
            continue;
        }
        out.push((a.at, a.value, false));
        if a.bend == 0.0 || a.value == b.value {
            continue;
        }
        let value_at = |at: Ticks| {
            let along = (at - a.at).get() as f64 / (b.at - a.at).get() as f64;
            curve_value(a.value, b.value, along, a.bend)
        };
        // A sixteenth note apart, or wider for a segment so long that it
        // would make more than MAX_POINTS of them.
        let step = Ticks::SIXTEENTH.max(Ticks((b.at - a.at).get() / MAX_POINTS));
        let mut from = (a.at, a.value);
        let mut at = a.at + step;
        while at < b.at {
            refine(from, (at, value_at(at)), tolerance, &value_at, &mut out);
            out.push((at, value_at(at), false));
            from = (at, value_at(at));
            at += step;
        }
        refine(from, (b.at, b.value), tolerance, &value_at, &mut out);
    }
    out
}

/// Adds the points between two that a curve moves more than `tolerance`
/// between, halving until it moves no more.
fn refine(
    from: (Ticks, f64),
    to: (Ticks, f64),
    tolerance: f64,
    value_at: &dyn Fn(Ticks) -> f64,
    out: &mut Vec<Step>,
) {
    if (to.1 - from.1).abs() <= tolerance || to.0 - from.0 < Ticks(2) {
        return;
    }
    let middle = from.0 + Ticks((to.0 - from.0).get() / 2);
    let point = (middle, value_at(middle));
    refine(from, point, tolerance, value_at, out);
    out.push((point.0, point.1, false));
    refine(point, to, tolerance, value_at, out);
}

impl Exporter<'_> {
    /// An envelope's points in the unit its parameter is written in (R76).
    pub(super) fn points_of(
        &self,
        envelope: &[Breakpoint],
        conversion: &Conversion,
    ) -> Vec<PointValue> {
        if conversion.is_bool() {
            let mut last = None;
            return envelope
                .iter()
                .filter_map(|point| {
                    let on = point.value > 0.5;
                    (last != Some(on)).then(|| {
                        last = Some(on);
                        PointValue::Bool {
                            time: Some(beats(point.at)),
                            value: Some(on),
                        }
                    })
                })
                .collect();
        }
        let span = conversion.span().unwrap_or_else(|| {
            let (low, high) = envelope
                .iter()
                .fold((f64::MAX, f64::MIN), |(low, high), p| {
                    (low.min(p.value), high.max(p.value))
                });
            if high > low { high - low } else { 1.0 }
        });
        straighten(envelope, span * TOLERANCE)
            .into_iter()
            .map(|(at, value, hold)| PointValue::Real {
                time: Some(beats(at)),
                value: Some(conversion.value(value)),
                interpolation: Some(if hold {
                    Interpolation::Hold
                } else {
                    Interpolation::Linear
                }),
            })
            .collect()
    }

    /// A parameter's automation as the points that target it (R76): the
    /// whole envelope for a track's, or a Session clip's, in ticks from the
    /// clip's contents.
    pub(super) fn parameter_points(
        &mut self,
        track: &Track,
        target: AutomationTarget,
        envelope: &[Breakpoint],
    ) -> Option<Timeline> {
        let key = (track.id(), target);
        if self.dropped.contains(&key) {
            return None;
        }
        let (Some(id), Some(conversion)) = (
            self.ids.parameters.get(&key).cloned(),
            self.conversions.get(&key).cloned(),
        ) else {
            self.dropped.insert(key);
            self.report.add(
                "an envelope of a parameter the format can't carry",
                &Self::place(track),
                "not written",
            );
            return None;
        };
        if envelope.is_empty() {
            return None;
        }
        Some(Timeline::Points(Points {
            info: TimelineInfo {
                time_unit: Some(super::super::model::TimeUnit::Beats),
                ..TimelineInfo::default()
            },
            unit: conversion.unit(),
            target: Target {
                parameter: Some(id),
                ..Target::default()
            },
            points: self.points_of(envelope, &conversion),
        }))
    }

    /// The tempo automation: a point at each segment's start, a hold for a
    /// steady one and a straight line for a ramp, with a point holding a
    /// ramp's end value where the tempo jumps after it (R75).
    pub(super) fn tempo_automation(&self) -> Option<Points> {
        let segments: Vec<TempoSegment> = self.tempo.segments().collect();
        if segments.len() < 2 {
            return None;
        }
        let mut points = Vec::new();
        for (index, segment) in segments.iter().enumerate() {
            match *segment {
                TempoSegment::Constant { start, bpm } => points.push((start, bpm.get(), true)),
                TempoSegment::Ramp { start, from, to } => {
                    points.push((start, from.get(), false));
                    // The jump after a ramp: the value it reaches holds up
                    // to the tick before the next segment.
                    if let Some(next) = segments.get(index + 1)
                        && next.start_bpm() != to
                        && next.start() - start > Ticks(1)
                    {
                        points.push((next.start() - Ticks(1), to.get(), true));
                    }
                }
            }
        }
        Some(Points {
            info: TimelineInfo {
                time_unit: Some(super::super::model::TimeUnit::Beats),
                ..TimelineInfo::default()
            },
            unit: Some(Unit::Bpm),
            target: Target {
                parameter: Some(self.ids.tempo.clone()),
                ..Target::default()
            },
            points: points
                .into_iter()
                .map(|(at, bpm, hold)| PointValue::Real {
                    time: Some(beats(at)),
                    value: Some(bpm),
                    interpolation: Some(if hold {
                        Interpolation::Hold
                    } else {
                        Interpolation::Linear
                    }),
                })
                .collect(),
        })
    }

    /// The time-signature changes after the first (R75).
    pub(super) fn signature_automation(&self) -> Option<Points> {
        let changes: Vec<_> = self.signatures.changes().collect();
        if changes.len() < 2 {
            return None;
        }
        Some(Points {
            info: TimelineInfo {
                time_unit: Some(super::super::model::TimeUnit::Beats),
                ..TimelineInfo::default()
            },
            unit: None,
            target: Target {
                parameter: Some(self.ids.signature.clone()),
                ..Target::default()
            },
            points: changes
                .into_iter()
                .map(|change| PointValue::Signature {
                    time: Some(beats(change.at)),
                    numerator: Some(i64::from(change.signature.numerator())),
                    denominator: Some(i64::from(change.signature.denominator())),
                })
                .collect(),
        })
    }

    /// The locators as the Arrangement's markers (R79).
    pub(super) fn markers(&self) -> Option<Markers> {
        let locators = self.document.locators();
        if locators.is_empty() {
            return None;
        }
        Some(Markers {
            info: TimelineInfo::default(),
            markers: locators
                .iter()
                .map(|locator| Marker {
                    info: TimelineInfo {
                        name: Some(locator.name.clone()),
                        ..TimelineInfo::default()
                    },
                    time: Some(beats(locator.position)),
                })
                .collect(),
        })
    }

    /// One MIDI controller envelope of a clip as the points that stand for
    /// it, in beats from the clip's contents (R76).
    pub(super) fn controller_points(
        &self,
        target: AutomationTarget,
        envelope: &[Breakpoint],
    ) -> Option<Timeline> {
        let (expression, controller, scale): (_, _, fn(f64) -> f64) = match target {
            AutomationTarget::MidiCc { number } => (
                Expression::ChannelController,
                Some(i64::from(number)),
                |value| value / 127.0,
            ),
            AutomationTarget::MidiPitchBend => {
                (Expression::PitchBend, None, |value| value / 16_384.0 + 0.5)
            }
            AutomationTarget::MidiChannelPressure => {
                (Expression::ChannelPressure, None, |value| value / 127.0)
            }
            _ => return None,
        };
        let (low, high) = target.midi_range()?;
        let points = straighten(envelope, (high - low) * TOLERANCE)
            .into_iter()
            .map(|(at, value, hold)| PointValue::Real {
                time: Some(beats(at)),
                value: Some(scale(value)),
                interpolation: Some(if hold {
                    Interpolation::Hold
                } else {
                    Interpolation::Linear
                }),
            })
            .collect();
        Some(Timeline::Points(Points {
            info: TimelineInfo {
                time_unit: Some(super::super::model::TimeUnit::Beats),
                ..TimelineInfo::default()
            },
            unit: Some(Unit::Normalized),
            target: Target {
                expression: Some(expression),
                channel: Some(0),
                controller,
                ..Target::default()
            },
            points,
        }))
    }

    /// A note's own pitch bend, slide and pressure as the expressions inside
    /// the note (R76).
    pub(super) fn note_expressions(&self, note: &bl_model::Note) -> Option<Timeline> {
        let mut children = Vec::new();
        for (expression, unit, envelope, scale, span) in [
            (
                Expression::Transpose,
                Unit::Semitones,
                &note.pitch,
                1.0,
                96.0,
            ),
            (
                Expression::Timbre,
                Unit::Normalized,
                &note.slide,
                127.0,
                127.0,
            ),
            (
                Expression::Pressure,
                Unit::Normalized,
                &note.pressure,
                127.0,
                127.0,
            ),
        ] {
            if envelope.is_empty() {
                continue;
            }
            children.push(Timeline::Points(Points {
                info: TimelineInfo {
                    time_unit: Some(super::super::model::TimeUnit::Beats),
                    ..TimelineInfo::default()
                },
                unit: Some(unit),
                target: Target {
                    expression: Some(expression),
                    ..Target::default()
                },
                points: straighten(envelope, span * TOLERANCE)
                    .into_iter()
                    .map(|(at, value, hold)| PointValue::Real {
                        time: Some(beats(at)),
                        value: Some(value / scale),
                        interpolation: Some(if hold {
                            Interpolation::Hold
                        } else {
                            Interpolation::Linear
                        }),
                    })
                    .collect(),
            }));
        }
        (!children.is_empty()).then(|| {
            Timeline::Lanes(super::super::model::Lanes {
                info: TimelineInfo::default(),
                children,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(at: i64, value: f64, bend: f64) -> Breakpoint {
        Breakpoint {
            at: Ticks(at),
            value,
            bend,
        }
    }

    // Spec: dawproject R76. A curve breaks into segments that stay within
    // the tolerance of it at every tick.
    #[test]
    fn a_curved_segment_is_broken_into_segments_within_the_tolerance() {
        let bar = Ticks::QUARTER.get() * 4;
        let envelope = [point(0, 0.0, 1.0), point(bar, 1.0, 0.0)];
        let steps = straighten(&envelope, 0.01);
        assert!(steps.len() > 17, "{} points", steps.len());
        for tick in (0..bar).step_by(997) {
            let along = tick as f64 / bar as f64;
            let want = curve_value(0.0, 1.0, along, 1.0);
            let after = steps.partition_point(|(at, _, _)| at.get() <= tick);
            let (t0, v0, _) = steps[after - 1];
            let (t1, v1, _) = steps[after];
            let got = v0 + (v1 - v0) * (tick - t0.get()) as f64 / (t1.get() - t0.get()) as f64;
            assert!(
                (got - want).abs() <= 0.01,
                "at {tick}: {got} against {want}"
            );
        }
    }

    // Spec: dawproject R76. A segment far longer than a song makes a bounded
    // number of points.
    #[test]
    fn a_very_long_curved_segment_makes_a_bounded_number_of_points() {
        let envelope = [point(0, 0.0, 1.0), point(i64::MAX / 8, 1.0, 0.0)];
        let steps = straighten(&envelope, 0.01);
        assert!(steps.len() < 20_000, "{} points", steps.len());
    }

    // Spec: dawproject R76. Two breakpoints a tick apart whose value steps
    // are one point that holds.
    #[test]
    fn a_step_is_a_point_that_holds() {
        let envelope = [
            point(0, 0.0, 0.0),
            point(100, 0.0, 0.0),
            point(101, 1.0, 0.0),
        ];
        let steps = straighten(&envelope, 0.01);
        assert_eq!(
            steps,
            vec![
                (Ticks(0), 0.0, false),
                (Ticks(100), 0.0, true),
                (Ticks(101), 1.0, true)
            ]
        );
    }
}
