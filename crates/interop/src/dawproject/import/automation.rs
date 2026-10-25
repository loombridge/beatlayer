//! Automation (the DAWproject spec, R56 to R62): the track envelopes for
//! mixer controls, sends and device parameters, the clips' own automation,
//! MIDI controllers and note expressions, and an audio clip's one-value
//! gain and transpose.

use std::collections::BTreeMap;
use std::sync::Arc;

use bl_model::note_expression::Expression as NoteExpression;
use bl_model::param::db_to_gain;
use bl_model::{
    AutomationTarget, Breakpoint, ClipEnvelope, Envelope, EnvelopeKind, MixerParam,
    ParamDescriptor, ParamId, ParamKind,
};

use super::devices::native_value;
use super::*;
use crate::dawproject::generic::Quantity;
use crate::dawproject::model::{Expression, Points};
use crate::dawproject::plugins::Installed;

/// What a DAWproject parameter drives in the set, recorded as the parameter
/// is made so that the points aimed at it find their target (R56).
#[derive(Clone, Debug)]
pub(super) enum Driven {
    /// A control of a track's mixer.
    Mixer {
        track: TrackId,
        param: MixerParam,
        unit: Option<Unit>,
        min: Option<f64>,
        max: Option<f64>,
    },
    /// A track's send to a return.
    Send {
        track: TrackId,
        to: TrackId,
        unit: Option<Unit>,
        min: Option<f64>,
        max: Option<f64>,
    },
    /// A parameter of one of Beatlayer's own devices.
    Device {
        track: TrackId,
        device: bl_model::DeviceId,
        param: ParamId,
        quantity: Quantity,
        device_type: String,
        unit: Option<Unit>,
        min: Option<f64>,
        max: Option<f64>,
        /// A decibel offset the file's value takes (R53).
        offset_db: f64,
    },
    /// A parameter of a plug-in, by the plug-in's own number for it (R57).
    Plugin {
        track: TrackId,
        device: bl_model::DeviceId,
        native: ParamId,
        installed: Option<Installed>,
        unit: Option<Unit>,
    },
    /// Something Beatlayer can't automate (R56).
    Unsupported(&'static str),
}

/// The furthest tick from zero a point is placed at.
const TICK_LIMIT: i64 = i64::MAX / 4;

/// One automation point in ticks: where, what and whether it holds.
pub(super) type Step = (Ticks, f64, bool);

/// The envelopes the Arrangement's points make, by track and parameter,
/// written when the import is done.
pub(super) type Pending = BTreeMap<(TrackId, AutomationTarget), Vec<Breakpoint>>;

/// Points that drive a MIDI controller, an expression or a note, found
/// outside any clip in a track's lanes (R60, R61).
#[derive(Clone, Debug)]
pub(super) struct Expressions {
    pub points: Points,
    pub unit: TimeUnit,
}

/// The pieces of a series: its steps, each as the breakpoints R58 makes.
pub(super) fn breakpoints(steps: &[Step]) -> Vec<Breakpoint> {
    let mut made: Vec<Breakpoint> = Vec::new();
    for (index, (at, value, hold)) in steps.iter().enumerate() {
        made.push(Breakpoint {
            at: *at,
            value: *value,
            bend: 0.0,
        });
        // A holding point steps one tick before the next point's.
        if *hold
            && let Some((next, _, _)) = steps.get(index + 1)
            && *next - Ticks(1) > *at
        {
            made.push(Breakpoint {
                at: *next - Ticks(1),
                value: *value,
                bend: 0.0,
            });
        }
    }
    made
}

/// The value in force at `at` among `steps`, and whether it holds there.
fn value_at(steps: &[Step], at: Ticks) -> Option<(f64, bool)> {
    let before = steps.iter().rposition(|(t, _, _)| *t <= at);
    let first_after = steps.iter().position(|(t, _, _)| *t > at);
    match (before, first_after) {
        (Some(b), Some(a)) if !steps[b].2 => {
            let (t0, v0, _) = steps[b];
            let (t1, v1, _) = steps[a];
            let along = (at - t0).get() as f64 / (t1 - t0).get() as f64;
            Some((v0 + (v1 - v0) * along, false))
        }
        (Some(b), _) => Some((steps[b].1, steps[b].2)),
        // Before the first point the first point's value holds.
        (None, Some(a)) => Some((steps[a].1, true)),
        (None, None) => None,
    }
}

/// The steps between `from` and `to`, with a first step at `from` holding the
/// value in force there, so a clip's share of a longer series starts right.
fn window(steps: &[Step], from: Ticks, to: Ticks) -> Vec<Step> {
    let mut inside: Vec<Step> = Vec::new();
    if !steps.iter().any(|(at, _, _)| *at == from)
        && let Some((value, hold)) = value_at(steps, from)
    {
        inside.push((from, value, hold));
    }
    inside.extend(
        steps
            .iter()
            .filter(|(at, _, _)| *at >= from && *at <= to)
            .copied(),
    );
    inside
}

impl Importer<'_, '_> {
    /// The steps a set of points makes, in ticks of the song: `zero` is the
    /// tick where the points' time zero plays (R9, R58).
    pub(super) fn steps(&self, points: &Points, unit: TimeUnit, zero: Ticks) -> Vec<Step> {
        let unit = points.info.time_unit.unwrap_or(unit);
        let mut steps: Vec<Step> = Vec::new();
        for point in &points.points {
            let (time, value, hold) = match point {
                PointValue::Real {
                    time,
                    value,
                    interpolation,
                } => (
                    *time,
                    *value,
                    interpolation.unwrap_or(Interpolation::Hold) == Interpolation::Hold,
                ),
                PointValue::Bool { time, value } => {
                    (*time, value.map(|v| f64::from(u8::from(v))), true)
                }
                PointValue::Integer { time, value } | PointValue::Enumerated { time, value } => {
                    (*time, value.map(|v| v as f64), true)
                }
                PointValue::Signature { .. } => continue,
            };
            let (Some(time), Some(value)) = (time, value) else {
                continue;
            };
            if !time.is_finite() || !value.is_finite() {
                continue;
            }
            // Kept well inside a tick count, so later sums can't overflow.
            let at = Ticks(
                zero.get()
                    .saturating_add(self.span(unit, time, zero).get())
                    .clamp(-TICK_LIMIT, TICK_LIMIT),
            );
            steps.push((at, value, hold));
        }
        // Of two points at one tick the later in the file wins (R58).
        steps.sort_by_key(|(at, _, _)| *at);
        let mut unique: Vec<Step> = Vec::new();
        for step in steps {
            match unique.last_mut() {
                Some(last) if last.0 == step.0 => *last = step,
                _ => unique.push(step),
            }
        }
        unique
    }

    /// The target and the value of each step of points aimed at the
    /// parameter `parameter` (R56, R57), or why there are none.
    pub(super) fn drive(
        &mut self,
        parameter: &str,
        points: &Points,
        steps: &[Step],
        place: &str,
    ) -> Option<(TrackId, AutomationTarget, Vec<Step>)> {
        let Some(driven) = self.targets.get(parameter).cloned() else {
            self.report.add(
                "automation of a parameter the import left out",
                place,
                "left out",
            );
            return None;
        };
        let is_bool = points
            .points
            .iter()
            .all(|point| matches!(point, PointValue::Bool { .. }));
        let unit = points.unit;
        match driven {
            Driven::Unsupported(what) => {
                self.report
                    .add(&format!("automation of {what}"), place, "left out");
                None
            }
            Driven::Mixer {
                track,
                param,
                unit: own,
                min,
                max,
            } => {
                let unit = unit.or(own);
                let converted = convert_steps(steps, |value| match param {
                    MixerParam::Volume => volume_from(unit, value, min, max),
                    MixerParam::Mute => Some(f64::from(u8::from(value > 0.5))),
                    _ => pan_from(unit, value, min, max),
                });
                Some((track, AutomationTarget::Mixer { param }, converted))
            }
            Driven::Send {
                track,
                to,
                unit: own,
                min,
                max,
            } => {
                let unit = unit.or(own);
                let converted = convert_steps(steps, |value| {
                    volume_from(unit, value, min, max).map(|gain| gain.min(1.0))
                });
                Some((track, AutomationTarget::Send { to }, converted))
            }
            Driven::Device {
                track,
                device,
                param,
                quantity,
                device_type,
                unit: own,
                min,
                max,
                offset_db,
            } => {
                let unit = unit.or(own);
                let types = self.native_types();
                let range = crate::dawproject::generic::range_of(&types, &device_type, param)
                    .unwrap_or((f64::MIN, f64::MAX));
                let converted = convert_steps(steps, |value| {
                    native_value(quantity, unit, value, min, max, range, offset_db)
                });
                if converted.len() < steps.len() {
                    self.report.add(
                        "automation in a unit Beatlayer can't convert",
                        place,
                        "left out",
                    );
                }
                Some((track, AutomationTarget::Device { device, param }, converted))
            }
            Driven::Plugin {
                track,
                device,
                native,
                installed,
                unit: own,
            } => {
                let Some(installed) = installed else {
                    self.report.add(
                        "automation of a plug-in that isn't installed",
                        place,
                        "left out",
                    );
                    return None;
                };
                let descriptors = match self.described(&installed) {
                    Ok(descriptors) => descriptors,
                    Err(_) => {
                        self.report.add(
                            "automation of a plug-in that didn't load",
                            place,
                            "left out",
                        );
                        return None;
                    }
                };
                let Some(descriptor) = descriptors.iter().find(|d| d.id == native).cloned() else {
                    self.report.add(
                        "automation of a parameter the plug-in doesn't have",
                        place,
                        "left out",
                    );
                    return None;
                };
                let unit = unit.or(own).unwrap_or(Unit::Normalized);
                if unit != Unit::Normalized && !is_bool {
                    self.report.add(
                        "automation of a plug-in in a unit that isn't normalized",
                        place,
                        "left out",
                    );
                    return None;
                }
                let converted = convert_steps(steps, |value| {
                    Some(if is_bool {
                        value
                    } else {
                        plain_value(&descriptor, value)
                    })
                });
                Some((
                    track,
                    AutomationTarget::Device {
                        device,
                        param: native,
                    },
                    converted,
                ))
            }
        }
    }

    /// A plug-in's parameters, loading it the first time they are asked for
    /// (R57).
    fn described(&mut self, plugin: &Installed) -> Result<Arc<Vec<ParamDescriptor>>, String> {
        let key = (plugin.format, plugin.plugin_id.clone());
        if let Some(found) = self.described.get(&key) {
            return found.clone();
        }
        let loaded = self.context.plugins.describe(plugin).map(Arc::new);
        self.described.insert(key, loaded.clone());
        loaded
    }

    /// Adds the envelope of one set of points to the Arrangement's
    /// (R56 to R58), over `span` only when it lies inside a clip (R59).
    pub(super) fn add_envelope(
        &mut self,
        parameter: &str,
        points: &Points,
        unit: TimeUnit,
        zero: Ticks,
        span: Option<(Ticks, Ticks)>,
        place: &str,
    ) {
        let steps = self.steps(points, unit, zero);
        let steps = match span {
            Some((from, to)) => {
                let mut inside = window(&steps, from, to);
                // A segment the clip's end cuts ends at the value it reaches
                // there, which holds after the clip (R59).
                if !steps.iter().any(|(at, _, _)| *at == to)
                    && let Some((value, _)) = value_at(&steps, to)
                {
                    inside.push((to, value, true));
                }
                inside
            }
            None => steps,
        };
        if steps.is_empty() {
            return;
        }
        let Some((track, target, converted)) = self.drive(parameter, points, &steps, place) else {
            return;
        };
        let made = breakpoints(&converted);
        if let Some((_, to)) = span
            && made.last().is_some_and(|last| last.at == to)
        {
            self.held_ends.insert((track, target, to));
        }
        self.pending
            .entry((track, target))
            .or_default()
            .extend(made);
    }

    /// Writes the envelopes the points made: of two breakpoints at one tick
    /// the later wins (R58).
    pub(super) fn write_envelopes(&mut self) {
        for ((track, target), mut points) in std::mem::take(&mut self.pending) {
            points.retain(|point| point.at >= Ticks::ZERO);
            let mut unique: Vec<Breakpoint> = Vec::new();
            points.sort_by_key(|point| point.at);
            for point in points {
                match unique.last_mut() {
                    Some(last) if last.at == point.at => *last = point,
                    _ => unique.push(point),
                }
            }
            if unique.is_empty() {
                continue;
            }
            // A clip's last value holds until the envelope's next point (R59).
            for index in (0..unique.len().saturating_sub(1)).rev() {
                let (at, next) = (unique[index].at, unique[index + 1].at);
                if next - Ticks(1) > at && self.held_ends.contains(&(track, target, at)) {
                    let value = unique[index].value;
                    unique.insert(
                        index + 1,
                        Breakpoint {
                            at: next - Ticks(1),
                            value,
                            bend: 0.0,
                        },
                    );
                }
            }
            let place = self.scope_place_of(track);
            self.try_apply(
                bl_model::Command::SetAutomationEnvelope {
                    track,
                    target,
                    points: unique,
                },
                "an envelope",
                &place,
            );
        }
    }

    /// The points outside clips in the Arrangement's lanes (R56, R60): the
    /// ones that drive a parameter become envelopes, the ones that drive a
    /// MIDI controller wait for the clips they are copied into.
    pub(super) fn collect_lane_points(&mut self, project: &DawProject) {
        let Some(arrangement) = &project.arrangement else {
            return;
        };
        let transport = project.transport.as_ref();
        let own = [
            transport
                .and_then(|transport| transport.tempo.as_ref())
                .and_then(|tempo| tempo.id.clone()),
            transport
                .and_then(|transport| transport.time_signature.as_ref())
                .and_then(|signature| signature.id.clone()),
        ];
        let mut found: Vec<(Option<String>, Points, TimeUnit)> = Vec::new();
        for lane in &arrangement.lanes {
            walk_points(lane, None, TimeUnit::Beats, &mut found);
        }
        for (scope, points, unit) in found {
            let place = self.scope_place(scope.as_deref());
            if let Some(parameter) = &points.target.parameter {
                if own.iter().any(|id| id.as_deref() == Some(parameter)) {
                    continue;
                }
                self.add_envelope(parameter, &points, unit, Ticks::ZERO, None, &place);
            } else if points.target.expression.is_some() {
                let Some(track) = scope.as_deref().and_then(|id| self.tracks.get(id).copied())
                else {
                    self.report.add(
                        "automation of a MIDI controller outside a track",
                        &place,
                        "left out",
                    );
                    continue;
                };
                self.track_expressions
                    .entry(track)
                    .or_default()
                    .push(Expressions { points, unit });
            } else {
                self.report.add(
                    "a timeline of automation with no target",
                    &place,
                    "left out",
                );
            }
        }
    }

    /// Where a track is, for the report.
    fn scope_place_of(&self, track: TrackId) -> String {
        self.editor.document().track(track).map_or_else(
            || "the Arrangement".to_owned(),
            |track| format!("the track \"{}\"", track.name()),
        )
    }

    /// Where a track the lanes are scoped to is, for the report.
    fn scope_place(&self, scope: Option<&str>) -> String {
        scope
            .and_then(|id| self.tracks.get(id).copied())
            .and_then(|id| self.editor.document().track(id))
            .map_or_else(
                || "the Arrangement".to_owned(),
                |track| format!("the track \"{}\"", track.name()),
            )
    }

    /// Copies a MIDI track's points into the clips made for it, each clip
    /// starting with the value in force at its start; what falls outside
    /// every clip is listed (R60, R61).
    pub(super) fn copy_expressions(&mut self, lane: &Lane, pieces: &mut [Piece]) {
        let Some(lists) = self.track_expressions.get(&lane.id).cloned() else {
            return;
        };
        for Expressions { points, unit } in lists {
            let steps = self.steps(&points, unit, Ticks::ZERO);
            if steps.is_empty() {
                continue;
            }
            let mut used = false;
            for piece in pieces.iter_mut() {
                let clip = &mut piece.clip;
                let (from, to) = (clip.start, clip.end());
                // A series that starts before the clip ends reaches it.
                if steps.first().is_none_or(|(at, _, _)| *at >= to) {
                    continue;
                }
                let origin = clip.content_start();
                let relative: Vec<Step> = window(&steps, from, to)
                    .into_iter()
                    .map(|(at, value, hold)| (at - origin, value, hold))
                    .collect();
                // The clip reports what it cannot keep.
                self.expression_into(clip, &points, &relative, &lane.place);
                used = true;
            }
            if !used {
                self.report.add(
                    "points of a MIDI controller outside every clip",
                    &lane.place,
                    "left out",
                );
            }
        }
    }

    /// Puts one expression's steps, in ticks from the clip's contents, into
    /// a MIDI clip (R60, R61). Whether anything was kept.
    pub(super) fn expression_into(
        &mut self,
        clip: &mut Clip,
        points: &Points,
        steps: &[Step],
        place: &str,
    ) {
        let target = &points.target;
        let Some(expression) = target.expression else {
            return;
        };
        let unit = points.unit.unwrap_or(Unit::Normalized);
        let share = |value: f64| match unit {
            Unit::Percent => value / 100.0,
            _ => value,
        };
        let (auto, scale): (Option<AutomationTarget>, fn(f64) -> f64) = match expression {
            Expression::ChannelController => {
                let number = target.controller.and_then(|n| u8::try_from(n).ok());
                match number.filter(|n| AutomationTarget::is_clip_controller(*n)) {
                    Some(number) => (Some(AutomationTarget::MidiCc { number }), |v| v * 127.0),
                    None => (None, |v| v),
                }
            }
            Expression::PitchBend if unit == Unit::Semitones => {
                // The bend's range isn't in the file, so semitones can't be
                // placed on it.
                self.report
                    .add("a pitch bend in semitones", place, "left out");
                return;
            }
            Expression::PitchBend => (Some(AutomationTarget::MidiPitchBend), |v| {
                ((v - 0.5) * 2.0 * 8192.0).clamp(-8192.0, 8191.0)
            }),
            Expression::ChannelPressure => {
                (Some(AutomationTarget::MidiChannelPressure), |v| v * 127.0)
            }
            Expression::PolyPressure => {
                self.poly_pressure(clip, points, steps, place);
                return;
            }
            _ => (None, |v| v),
        };
        let Some(auto) = auto else {
            self.report.add(
                "a MIDI controller or expression Beatlayer can't hold",
                place,
                "left out",
            );
            return;
        };
        // Values for one controller on a second channel are left out.
        let code = match auto {
            AutomationTarget::MidiCc { number } => u16::from(number),
            AutomationTarget::MidiPitchBend => 128,
            _ => 129,
        };
        let channel = target.channel.unwrap_or(0);
        let held = *self
            .controller_channels
            .entry((clip.id, code))
            .or_insert(channel);
        if held != channel {
            self.report
                .add("a controller on a second channel", place, "left out");
            return;
        }
        let (low, high) = auto.midi_range().unwrap_or((0.0, 127.0));
        let converted: Vec<Step> = steps
            .iter()
            .filter(|(at, _, _)| *at >= Ticks::ZERO)
            .map(|(at, value, hold)| (*at, scale(share(*value)).clamp(low, high), *hold))
            .collect();
        let Ok(envelope) = Envelope::from_points(breakpoints(&converted)) else {
            return;
        };
        if envelope.is_empty() {
            return;
        }
        clip.envelopes.retain(|held| held.target != auto);
        clip.envelopes.push(ClipEnvelope::new(auto, envelope));
        clip.envelopes.sort_by_key(|held| held.target);
    }

    /// A polyphonic pressure becomes the pressure of the notes of its key
    /// that it overlaps (R61).
    fn poly_pressure(&mut self, clip: &mut Clip, points: &Points, steps: &[Step], place: &str) {
        let Some(key) = points.target.key.and_then(|k| u8::try_from(k).ok()) else {
            self.report
                .add("polyphonic pressure with no key", place, "left out");
            return;
        };
        let bl_model::ClipData::Midi(data) = &mut clip.data else {
            return;
        };
        let unit = points.unit;
        let notes: Vec<bl_model::Note> = data.notes.iter().cloned().collect();
        let mut changed = Vec::new();
        for mut note in notes {
            if note.key == key {
                let (from, to) = (note.start, note.start + note.length);
                let inside = window(steps, from, to);
                if !inside.is_empty() {
                    let relative: Vec<Step> = inside
                        .into_iter()
                        .map(|(at, value, hold)| {
                            let share = if unit == Some(Unit::Percent) {
                                value / 100.0
                            } else {
                                value
                            };
                            (at - note.start, (share * 127.0).clamp(0.0, 127.0), hold)
                        })
                        .collect();
                    note.pressure = breakpoints(&relative);
                }
            }
            changed.push(note);
        }
        data.notes = changed.into_iter().collect();
    }

    /// The expressions inside a note: pressure, its own pitch bend and its
    /// slide (R61).
    pub(super) fn note_expressions(
        &mut self,
        note: &mut bl_model::Note,
        content: &Timeline,
        unit: TimeUnit,
        place: &str,
    ) {
        let mut found: Vec<(Option<String>, Points, TimeUnit)> = Vec::new();
        walk_points(content, None, unit, &mut found);
        for (_, points, unit) in found {
            let steps = self.steps(&points, unit, Ticks::ZERO);
            let which = match points.target.expression {
                Some(Expression::Pressure) => Some((NoteExpression::Pressure, 127.0)),
                Some(Expression::Timbre) => Some((NoteExpression::Slide, 127.0)),
                Some(Expression::Transpose) => Some((NoteExpression::Pitch, 1.0)),
                _ => None,
            };
            let Some((expression, scale)) = which else {
                self.report.add(
                    "an expression of a note Beatlayer can't hold",
                    place,
                    "left out",
                );
                continue;
            };
            let (low, high) = expression.range();
            let percent = points.unit == Some(Unit::Percent);
            let inside: Vec<Step> = window(&steps, Ticks::ZERO, note.length)
                .into_iter()
                .map(|(at, value, hold)| {
                    let value = if percent { value / 100.0 } else { value };
                    (at, (value * scale).clamp(low, high), hold)
                })
                .collect();
            *note.envelope_mut(expression) = breakpoints(&inside);
        }
    }

    /// A Session clip's automation of its track's parameters becomes its
    /// automation envelopes (R59).
    pub(super) fn session_automation(
        &mut self,
        points: &[&Points],
        clip: &mut Clip,
        unit: TimeUnit,
        lane: &Lane,
        place: &str,
    ) {
        for set in points {
            let Some(parameter) = set.target.parameter.clone() else {
                continue;
            };
            let steps = self.steps(set, unit, Ticks::ZERO);
            if steps.is_empty() {
                continue;
            }
            let Some((track, target, converted)) = self.drive(&parameter, set, &steps, place)
            else {
                continue;
            };
            if track != lane.id {
                self.report.add(
                    "automation in a Session clip of another track's parameter",
                    place,
                    "left out",
                );
                continue;
            }
            let Ok(envelope) = Envelope::from_points(breakpoints(&converted)) else {
                continue;
            };
            let mut made = ClipEnvelope::new(target, envelope);
            made.kind = EnvelopeKind::Automation;
            clip.envelopes
                .retain(|held| !(held.target == target && held.kind == made.kind));
            clip.envelopes.push(made);
            clip.envelopes.sort_by_key(|held| (held.target, held.kind));
        }
    }

    /// An audio clip's one-value gain and transpose (R62).
    pub(super) fn audio_expressions(&mut self, points: &[&Points], clip: &mut Clip, place: &str) {
        let bl_model::ClipData::Audio(data) = &mut clip.data else {
            return;
        };
        for set in points {
            let value = match set.points.as_slice() {
                [
                    PointValue::Real {
                        value: Some(value), ..
                    },
                ] if value.is_finite() => *value,
                _ => {
                    self.report.add(
                        "an expression that changes over an audio clip",
                        place,
                        "left out",
                    );
                    continue;
                }
            };
            match set.target.expression {
                Some(Expression::Gain) => {
                    let (low, high) = bl_model::ClipParam::Gain.limits();
                    data.gain = match set.unit {
                        Some(Unit::Decibel) => db_to_gain(value),
                        _ => value,
                    }
                    .clamp(low, high);
                }
                Some(Expression::Transpose) => {
                    // To the nearest cent: whole semitones and a detune.
                    let (low, high) = bl_model::ClipParam::Transpose.limits();
                    let cents = (value.clamp(low, high) * 100.0).round();
                    let transpose = (cents / 100.0).round();
                    data.transpose = transpose as i32;
                    data.detune = cents - transpose * 100.0;
                }
                _ => self.report.add(
                    "an expression of an audio clip Beatlayer can't hold",
                    place,
                    "left out",
                ),
            }
        }
    }
}

/// Converts each step's value, dropping the steps that don't convert.
fn convert_steps(steps: &[Step], mut convert: impl FnMut(f64) -> Option<f64>) -> Vec<Step> {
    steps
        .iter()
        .filter_map(|(at, value, hold)| convert(*value).map(|value| (*at, value, *hold)))
        .collect()
}

/// A plug-in parameter's plain value from its normalized one, over the range
/// the host maps it across (R57).
pub(super) fn plain_value(descriptor: &ParamDescriptor, normalized: f64) -> f64 {
    let share = normalized.clamp(0.0, 1.0);
    match &descriptor.kind {
        ParamKind::Continuous { min, max, .. } => min + share * (max - min),
        ParamKind::Integer { min, max, .. } => (*min as f64 + share * (*max - *min) as f64).round(),
        ParamKind::Gain { max_db, .. } => db_to_gain(*max_db) * share,
        ParamKind::Toggle => f64::from(u8::from(share > 0.5)),
        ParamKind::Choice { choices } => (share * choices.len().saturating_sub(1) as f64).round(),
    }
}

/// A volume's linear gain from a value in `unit` (R21), as the file gives it.
pub(super) fn volume_raw(
    unit: Option<Unit>,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
) -> Option<f64> {
    Some(match unit? {
        Unit::Linear => value,
        Unit::Decibel => db_to_gain(value),
        Unit::Normalized => {
            let (min, max) = (min.unwrap_or(0.0), max.unwrap_or(1.0));
            min + value * (max - min)
        }
        _ => return None,
    })
}

/// A volume's gain kept within the fader's range.
fn volume_from(unit: Option<Unit>, value: f64, min: Option<f64>, max: Option<f64>) -> Option<f64> {
    volume_raw(unit, value, min, max)
        .filter(|gain| gain.is_finite())
        .map(|gain| gain.clamp(0.0, 2.0))
}

/// A pan from −1 to 1 from a value in `unit` (R21).
pub(super) fn pan_from(
    unit: Option<Unit>,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
) -> Option<f64> {
    let (low, high) = match unit {
        Some(Unit::Percent) => (min.unwrap_or(0.0), max.unwrap_or(100.0)),
        Some(Unit::Normalized | Unit::Linear) | None => (min.unwrap_or(0.0), max.unwrap_or(1.0)),
        Some(_) => return None,
    };
    let pan = (value - low) / (high - low) * 2.0 - 1.0;
    pan.is_finite().then(|| pan.clamp(-1.0, 1.0))
}

/// Every set of points in a timeline, with the track its lanes are scoped to
/// and the unit of its times.
fn walk_points(
    timeline: &Timeline,
    scope: Option<String>,
    unit: TimeUnit,
    found: &mut Vec<(Option<String>, Points, TimeUnit)>,
) {
    let scope = timeline.info().track.clone().or(scope);
    let unit = timeline.info().time_unit.unwrap_or(unit);
    match timeline {
        Timeline::Points(points) => found.push((scope, points.clone(), unit)),
        Timeline::Lanes(lanes) => {
            for child in &lanes.children {
                walk_points(child, scope.clone(), unit, found);
            }
        }
        _ => {}
    }
}
