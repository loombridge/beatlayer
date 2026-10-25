//! The Arrangement's automation editing (WP-6.25): the shape of a track's
//! envelope over a span of time, the clipboard of copied shapes, and the
//! commands that move an envelope's breakpoints and edit its shape.
//!
//! A shape over a span is the envelope's breakpoints inside it, with the
//! values at the span's edges, so that a copy of it can be pasted onto
//! another parameter: the values travel as positions on the parameter's
//! travel, and land at the same position on the target's.
//!
//! The behaviour is in the automation editing spec.

use std::collections::BTreeSet;

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Breakpoint, ChangeSet, Command, Document, Editor, Envelope, MixerParam,
    ParamScale, TrackId,
};
use bl_shell_spec::arrangement::Lane;
use bl_shell_spec::automation::{
    self as ids, AutomationResult, EnvelopeClipboardState, EnvelopeSpanArgs, InsertShapeArgs,
    MoveBreakpointsArgs, PasteEnvelopeArgs, SelectSegmentArgs, SkewArgs, SkewEdge, StretchTimeArgs,
    StretchValuesArgs,
};
use bl_shell_spec::editing::Position;
use serde::Serialize;
use serde_json::Value;

use super::editing::{EditFailure, transaction};
use super::{ArrangementView, parse};
use crate::devices::DeviceView;

/// How a parameter's values map onto its control's travel: the axis its
/// lane draws, from 0 at the bottom to 1 at the top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Travel {
    /// A track's volume fader, which is dB-like.
    Volume,
    /// A send level, whose travel matches the faders'.
    Send,
    /// Pan, from −1 to 1.
    Pan,
    /// A switch or radio button, whose axis is discrete (0 or 1).
    Switch,
    /// A stepped macro, whose axis is discrete, its steps (macros spec, R22).
    Steps {
        /// The number of steps.
        steps: u8,
    },
    /// A device parameter, from its descriptor's low value to its high one.
    Linear {
        /// The parameter's lowest value.
        min: f64,
        /// Its highest.
        max: f64,
    },
}

impl Travel {
    /// Where a value sits on the travel, from 0 (the bottom) to 1 (the top).
    #[must_use]
    pub fn position(self, value: f64) -> f64 {
        let t = match self {
            Self::Volume => bl_model::fader::fader_position(value),
            Self::Send => bl_model::fader::send_position(value),
            Self::Pan => (value + 1.0) / 2.0,
            Self::Switch => value,
            Self::Steps { .. } => value,
            Self::Linear { min, max } => {
                if max > min {
                    (value - min) / (max - min)
                } else {
                    0.5
                }
            }
        };
        t.clamp(0.0, 1.0)
    }

    /// The value at a place on the travel, from 0 (the bottom) to 1 (the
    /// top).
    #[must_use]
    pub fn value(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Volume => bl_model::fader::fader_gain(t),
            Self::Send => bl_model::fader::send_level(t),
            Self::Pan => t * 2.0 - 1.0,
            Self::Switch => t.round(),
            // Spec: macros R22. A stepped macro's lane has a stepped axis.
            Self::Steps { steps } => bl_model::macro_step_position(steps, t),
            Self::Linear { min, max } => min + (max - min) * t,
        }
    }
}

impl ParamScale for Travel {
    fn position(&self, value: f64) -> f64 {
        Self::position(*self, value)
    }

    fn value(&self, position: f64) -> f64 {
        Self::value(*self, position)
    }
}

/// The scale a clip's modulation envelope is drawn and edited on: shares of
/// the parameter's travel, 0 to 100 % for a level (Volume and sends) and
/// −100 % to 100 % for the rest, not the parameter's own units; a MIDI
/// controller's own range, pitch bend centred, and a clip setting's own.
// Spec: modulation R9, midi-controller-data R8, warp-modes R29
#[must_use]
pub fn share_scale(target: AutomationTarget) -> Travel {
    if let Some((min, max)) = target.midi_range().or_else(|| target.clip_range()) {
        return Travel::Linear { min, max };
    }
    let travel = if is_level(target) {
        bl_model::modulation::Travel::Fader
    } else {
        bl_model::modulation::Travel::Pan
    };
    let (min, max) = travel.clip_range();
    Travel::Linear { min, max }
}

/// Whether a parameter is a level, whose modulation is a share of its base.
#[must_use]
pub fn is_level(target: AutomationTarget) -> bool {
    matches!(
        target,
        AutomationTarget::Mixer {
            param: MixerParam::Volume
        } | AutomationTarget::Send { .. }
    )
}

/// Where a clip envelope for `target` modulates nothing: the top of a share
/// (a level, or a clip's Gain), and zero for the rest.
#[must_use]
pub fn rest_of(target: AutomationTarget) -> f64 {
    match target {
        _ if is_level(target) => 1.0,
        AutomationTarget::Clip {
            param: bl_model::ClipParam::Gain,
        } => 100.0,
        _ => 0.0,
    }
}

/// The travel of a parameter: where its values sit on its control. A
/// device parameter's travel is the range its descriptor gives.
#[must_use]
pub fn travel(doc: &Document, devices: &DeviceView, target: AutomationTarget) -> Travel {
    match target {
        AutomationTarget::Mixer {
            param: MixerParam::Volume,
        }
        | AutomationTarget::Chain {
            param: MixerParam::Volume,
            ..
        } => Travel::Volume,
        AutomationTarget::Mixer {
            param: MixerParam::Pan | MixerParam::SplitLeft | MixerParam::SplitRight,
        }
        | AutomationTarget::Chain {
            param: MixerParam::Pan | MixerParam::SplitLeft | MixerParam::SplitRight,
            ..
        } => Travel::Pan,
        AutomationTarget::Mixer {
            param: MixerParam::Mute,
        }
        | AutomationTarget::Chain {
            param: MixerParam::Mute,
            ..
        } => Travel::Switch,
        AutomationTarget::Send { .. } | AutomationTarget::ChainSend { .. } => Travel::Send,
        AutomationTarget::Device { device, param } => {
            // Spec: macros R22. A stepped macro's lane has a stepped axis.
            if let Some(steps) = macro_steps(doc, device, param) {
                return Travel::Steps { steps };
            }
            let range = doc
                .device(device)
                .and_then(|device| devices.description(&device.kind))
                .and_then(|description| description.param(param))
                .map(|descriptor| (descriptor.min(), descriptor.max()));
            match range {
                Some((min, max)) => Travel::Linear { min, max },
                None => Travel::Linear { min: 0.0, max: 1.0 },
            }
        }
        // A clip setting's lane runs over the setting's own range.
        AutomationTarget::Clip { param } => {
            let (min, max) = param.limits();
            Travel::Linear { min, max }
        }
        AutomationTarget::MidiCc { .. }
        | AutomationTarget::MidiPitchBend
        | AutomationTarget::MidiChannelPressure => {
            let (min, max) = target.midi_range().unwrap_or((0.0, 127.0));
            Travel::Linear { min, max }
        }
    }
}

/// The number of steps a stepped macro of `device` takes, if `param` is one
/// (macros spec, R22).
fn macro_steps(doc: &Document, device: bl_model::DeviceId, param: bl_model::ParamId) -> Option<u8> {
    let number = bl_model::macro_number(param)?;
    let rack = doc.device(device)?;
    let setting = rack.macros.setting(number);
    setting.is_stepped().then_some(setting.steps)
}

/// A copied shape of envelope: one lane's shape over a span, with its
/// positions from the span's start, its values as positions on the
/// parameter's travel, and its bends.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvelopeShape {
    /// The track the envelope is on.
    pub track: TrackId,
    /// The parameter the envelope drives.
    pub target: AutomationTarget,
    /// The shape: the breakpoints, with the values as positions on the
    /// travel.
    pub points: Vec<Breakpoint>,
}

/// The clipboard of copied shapes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvelopeClipboard {
    /// The lanes' shapes, in the order they were copied.
    pub lanes: Vec<EnvelopeShape>,
    /// How long the shape is.
    pub length: Ticks,
}

impl EnvelopeClipboard {
    /// What it holds, as `automation.copy` reports it.
    #[must_use]
    pub fn state(&self) -> EnvelopeClipboardState {
        EnvelopeClipboardState {
            lanes: self.lanes.len(),
            length: self.length,
        }
    }
}

/// The lanes a command acts on: the automation lanes among the selection's
/// lanes, or the ones given.
fn lanes_of(
    view: &ArrangementView,
    given: Option<Vec<Lane>>,
) -> Result<Vec<(TrackId, AutomationTarget)>, EditFailure> {
    let lanes = given.unwrap_or_else(|| view.time_selection_lanes().to_vec());
    let lanes: Vec<(TrackId, AutomationTarget)> = lanes
        .into_iter()
        .filter_map(|lane| lane.target().map(|target| (lane.track(), target)))
        .collect();
    if lanes.is_empty() {
        return Err(EditFailure::Unavailable(
            "no automation lane is selected".to_owned(),
        ));
    }
    Ok(lanes)
}

/// Whether the time selection covers an automation lane.
fn has_automation_lane(view: &ArrangementView) -> bool {
    view.time_selection_lanes()
        .iter()
        .any(|lane| lane.target().is_some())
}

/// The automation lanes the selected tracks show: the main lane and the
/// lanes of their own (the automation spec, R30).
fn shown_lanes(view: &ArrangementView) -> Vec<(TrackId, AutomationTarget)> {
    view.selected_tracks()
        .iter()
        .flat_map(|track| {
            view.shown_lane_targets(*track)
                .into_iter()
                .map(|(_, target)| (*track, target))
        })
        .collect()
}

/// Edits the envelope of each lane with `edit`, which gets the parameter's
/// travel and the value its control holds now, and writes the result as one
/// command; a lane the edit leaves as it was writes nothing.
fn edit_lanes(
    editor: &mut Editor,
    devices: &DeviceView,
    lanes: &[(TrackId, AutomationTarget)],
    edit: impl Fn(&mut Envelope, Travel, f64),
) -> Result<(), EditFailure> {
    for (track, target) in lanes {
        let doc = editor.document();
        let travel = travel(doc, devices, *target);
        let rest = crate::modulation::control_value(doc, devices, *track, target)
            .unwrap_or_else(|| travel.value(0.5));
        let before = envelope_of(doc, *track, *target).cloned();
        let mut envelope = before.clone().unwrap_or_default();
        edit(&mut envelope, travel, rest);
        if before.as_ref() == Some(&envelope) {
            continue;
        }
        apply(
            editor,
            Command::SetAutomationEnvelope {
                track: *track,
                target: *target,
                points: envelope.points().collect(),
            },
        )?;
    }
    Ok(())
}

/// The span a command acts on: the one given, or the time selection's.
fn span(
    doc: &Document,
    view: &ArrangementView,
    start: Option<Position>,
    end: Option<Position>,
) -> Result<(Ticks, Ticks), EditFailure> {
    let signatures = &doc.settings().time_signatures;
    let (start, end) = match (start, end) {
        (Some(start), Some(end)) => (
            start
                .resolve(signatures)
                .map_err(EditFailure::InvalidArgs)?,
            end.resolve(signatures).map_err(EditFailure::InvalidArgs)?,
        ),
        (None, None) => view
            .time_selection()
            .ok_or_else(|| EditFailure::Unavailable("no time is selected".to_owned()))?,
        _ => {
            return Err(EditFailure::InvalidArgs(
                "give `start` and `end`, or neither".to_owned(),
            ));
        }
    };
    if end <= start {
        return Err(EditFailure::InvalidArgs(
            "the span's `end` must be after its `start`".to_owned(),
        ));
    }
    Ok((start, end))
}

/// The envelope of a track's parameter, if it has one.
pub fn envelope_of(doc: &Document, track: TrackId, target: AutomationTarget) -> Option<&Envelope> {
    doc.track(track)?
        .automation_for(&target)
        .map(|automation| &automation.envelope)
}

/// The breakpoints of a lane's envelope that are selected: the two of the
/// segment that was clicked, or those the time selection covers, from its
/// start up to, not including, its end (the automation editing spec, R2,
/// R4).
#[must_use]
pub fn selected_breakpoints(
    doc: &Document,
    view: &ArrangementView,
    track: TrackId,
    target: AutomationTarget,
) -> Vec<Ticks> {
    if let Some((selected, at)) = view.selected_segment(track)
        && selected == target
    {
        return segment_breakpoints(doc, track, target, at);
    }
    let Some((start, end)) = view.time_selection() else {
        return Vec::new();
    };
    let covered = view
        .time_selection_lanes()
        .iter()
        .any(|lane| lane.track() == track && lane.target() == Some(target));
    if !covered {
        return Vec::new();
    }
    envelope_of(doc, track, target)
        .map(|envelope| {
            envelope
                .points()
                .filter(|point| point.at >= start && point.at < end)
                .map(|point| point.at)
                .collect()
        })
        .unwrap_or_default()
}

/// The two breakpoints of the segment starting at `at`.
fn segment_breakpoints(
    doc: &Document,
    track: TrackId,
    target: AutomationTarget,
    at: Ticks,
) -> Vec<Ticks> {
    let Some(envelope) = envelope_of(doc, track, target) else {
        return Vec::new();
    };
    let mut out = vec![at];
    if let Some(next) = envelope.points().find(|point| point.at > at) {
        out.push(next.at);
    }
    out
}

/// The shape of a lane's envelope over a span, with the values as positions
/// on the parameter's travel.
fn shape_of(
    doc: &Document,
    devices: &DeviceView,
    track: TrackId,
    target: AutomationTarget,
    start: Ticks,
    end: Ticks,
) -> Option<EnvelopeShape> {
    let envelope = envelope_of(doc, track, target)?;
    let travel = travel(doc, devices, target);
    let points = envelope
        .shape_in(start, end)
        .into_iter()
        .map(|point| Breakpoint {
            value: travel.position(point.value),
            ..point
        })
        .collect();
    Some(EnvelopeShape {
        track,
        target,
        points,
    })
}

/// The shape of a lane's envelope over a span, with the values in the
/// parameter's own units: what a clip carries with it.
pub fn shape_in(
    doc: &Document,
    track: TrackId,
    target: AutomationTarget,
    start: Ticks,
    end: Ticks,
) -> Option<Vec<Breakpoint>> {
    envelope_of(doc, track, target).map(|envelope| envelope.shape_in(start, end))
}

/// Writes a shape into a lane's envelope over a span, with the values in
/// the parameter's own units.
pub fn write_shape(
    editor: &mut Editor,
    track: TrackId,
    target: AutomationTarget,
    start: Ticks,
    end: Ticks,
    points: &[Breakpoint],
) -> Result<(), EditFailure> {
    let mut envelope = envelope_of(editor.document(), track, target)
        .cloned()
        .unwrap_or_default();
    envelope.write_span(start, end, points);
    apply(
        editor,
        Command::SetAutomationEnvelope {
            track,
            target,
            points: envelope.points().collect(),
        },
    )
}

/// Removes the shape of a lane's envelope over a span: the breakpoints
/// inside it go, and the envelope's values at the span's edges are kept
/// with a breakpoint at each, so it runs straight across the span.
pub fn remove_shape(
    editor: &mut Editor,
    track: TrackId,
    target: AutomationTarget,
    start: Ticks,
    end: Ticks,
) -> Result<(), EditFailure> {
    let Some(mut envelope) = envelope_of(editor.document(), track, target).cloned() else {
        return Ok(());
    };
    envelope.remove_span(start, end);
    apply(
        editor,
        Command::SetAutomationEnvelope {
            track,
            target,
            points: envelope.points().collect(),
        },
    )
}

fn apply(editor: &mut Editor, command: Command) -> Result<(), EditFailure> {
    editor.apply(command).map(|_| ())?;
    Ok(())
}

/// The name of the undo step an automation editing command makes.
#[must_use]
pub fn label(id: &str) -> &'static str {
    match id {
        ids::MOVE_BREAKPOINTS => "Move Automation Breakpoints",
        ids::SELECT_SEGMENT => "Select Automation Segment",
        ids::COPY => "Copy Automation",
        ids::CUT => "Cut Automation",
        ids::DELETE_RANGE => "Delete Automation",
        ids::PASTE => "Paste Automation",
        ids::DUPLICATE => "Duplicate Automation",
        ids::INSERT_SHAPE => "Insert Shape",
        ids::STRETCH_VALUES => "Stretch Automation Values",
        ids::STRETCH_TIME => "Stretch Automation in Time",
        ids::SKEW => "Skew Automation",
        ids::SIMPLIFY => "Simplify Envelope",
        _ => "Automation",
    }
}

/// Whether a person can run automation editing command `id` now: there's
/// an automation lane to act on, or something on the clipboard to paste.
#[must_use]
pub fn can_run(id: &str, view: &ArrangementView, clipboard: Option<&EnvelopeClipboard>) -> bool {
    let lanes = view.time_selection_lanes();
    let has_lane = lanes.iter().any(|lane| lane.target().is_some());
    match id {
        ids::MOVE_BREAKPOINTS => has_lane,
        ids::SELECT_SEGMENT => true,
        ids::COPY
        | ids::CUT
        | ids::DELETE_RANGE
        | ids::DUPLICATE
        | ids::INSERT_SHAPE
        | ids::STRETCH_VALUES
        | ids::STRETCH_TIME
        | ids::SKEW => has_lane,
        // Spec: automation-editing R18. Simplify Envelope needs a time
        // selection in an automation lane.
        ids::SIMPLIFY => has_lane && view.time_selection().is_some(),
        ids::PASTE => clipboard.is_some_and(|c| !c.lanes.is_empty()) && has_lane,
        _ => false,
    }
}

/// Runs automation editing command `id` with `args`, as `Shell::run` does.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &mut ArrangementView,
    clipboard: &mut Option<EnvelopeClipboard>,
    devices: &DeviceView,
) -> Result<Value, EditFailure> {
    let doc = editor.document().clone();
    match id {
        ids::SELECT_SEGMENT => {
            let SelectSegmentArgs { track, target, at } = parse(args)?;
            view.select_segment(track, target, at);
            Ok(to_value(&view.state(&doc)))
        }
        ids::MOVE_BREAKPOINTS => {
            let MoveBreakpointsArgs {
                track,
                target,
                at,
                by,
                share,
            } = parse(args)?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                move_breakpoints(editor, devices, track, target, &at, by, share)
            })?;
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::COPY => {
            let EnvelopeSpanArgs { lanes, start, end } = parse(args)?;
            let lanes = lanes_of(view, lanes)?;
            let (start, end) = span(&doc, view, start, end)?;
            let copied = copy(&doc, devices, &lanes, start, end);
            let state = copied.state();
            *clipboard = Some(copied);
            Ok(to_value(&state))
        }
        ids::CUT => {
            let EnvelopeSpanArgs { lanes, start, end } = parse(args)?;
            let lanes = lanes_of(view, lanes)?;
            let (start, end) = span(&doc, view, start, end)?;
            let copied = copy(&doc, devices, &lanes, start, end);
            let (_, recorded) = transaction(editor, label(id), |editor| {
                delete_range(editor, &lanes, start, end)
            })?;
            *clipboard = Some(copied);
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::DELETE_RANGE => {
            let EnvelopeSpanArgs { lanes, start, end } = parse(args)?;
            let lanes = lanes_of(view, lanes)?;
            let (start, end) = span(&doc, view, start, end)?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                delete_range(editor, &lanes, start, end)
            })?;
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::PASTE => {
            let PasteEnvelopeArgs { lanes, at } = parse(args)?;
            let lanes = lanes_of(view, lanes)?;
            let Some(copied) = clipboard.as_ref().filter(|c| !c.lanes.is_empty()) else {
                return Err(EditFailure::Unavailable(
                    "there's nothing on the clipboard to paste".to_owned(),
                ));
            };
            let at = match at {
                Some(at) => at
                    .resolve(&doc.settings().time_signatures)
                    .map_err(EditFailure::InvalidArgs)?,
                None => view
                    .time_selection()
                    .map_or(view.insert_marker(), |(start, _)| start),
            };
            let (_, recorded) = transaction(editor, label(id), |editor| {
                paste(editor, devices, &lanes, copied, at)
            })?;
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::DUPLICATE => {
            let EnvelopeSpanArgs { lanes, start, end } = parse(args)?;
            let lanes = lanes_of(view, lanes)?;
            let (start, end) = span(&doc, view, start, end)?;
            let copied = copy(&doc, devices, &lanes, start, end);
            let (_, recorded) = transaction(editor, label(id), |editor| {
                paste(editor, devices, &lanes, &copied, end)
            })?;
            // The copy is the time selection, on the same lanes.
            let lanes: Vec<Lane> = lanes
                .iter()
                .map(|(track, target)| Lane::Automation {
                    track: *track,
                    target: *target,
                })
                .collect();
            view.select_span_on(lanes, end, end + copied.length);
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::INSERT_SHAPE => {
            let InsertShapeArgs {
                shape,
                lanes,
                start,
                end,
            } = parse(args)?;
            let lanes = match lanes {
                None if !has_automation_lane(view) => {
                    let shown = shown_lanes(view);
                    if shown.is_empty() {
                        return Err(EditFailure::Unavailable(
                            "no automation lane is selected".to_owned(),
                        ));
                    }
                    shown
                }
                lanes => lanes_of(view, lanes)?,
            };
            let (start, end) = match (start, end, view.time_selection()) {
                // Spec: automation-editing R11. With no time selection, one
                // grid step from the insert marker.
                (None, None, None) => {
                    let at = view.insert_marker();
                    (at, at + view.grid_step(&doc).ticks)
                }
                (start, end, _) => span(&doc, view, start, end)?,
            };
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_lanes(editor, devices, &lanes, |envelope, travel, rest| {
                    envelope.insert_shape(shape, start, end, rest, &travel);
                })
            })?;
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::STRETCH_VALUES => {
            let StretchValuesArgs {
                lanes,
                start,
                end,
                bottom,
                top,
            } = parse(args)?;
            if !(bottom.is_finite() && top.is_finite() && bottom <= top) {
                return Err(EditFailure::InvalidArgs(
                    "`bottom` must not be above `top`".to_owned(),
                ));
            }
            let lanes = lanes_of(view, lanes)?;
            let (start, end) = span(&doc, view, start, end)?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_lanes(editor, devices, &lanes, |envelope, travel, _| {
                    envelope.remap_span(start, end, (bottom, bottom), (top, top), &travel);
                })
            })?;
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::SKEW => {
            let SkewArgs {
                edge,
                at_start,
                at_end,
                lanes,
                start,
                end,
            } = parse(args)?;
            // The edge can't cross the other one, which stays level.
            let crosses = match edge {
                SkewEdge::Top => at_start < 0.0 || at_end < 0.0,
                SkewEdge::Bottom => at_start > 1.0 || at_end > 1.0,
            };
            if !(at_start.is_finite() && at_end.is_finite()) || crosses {
                return Err(EditFailure::InvalidArgs(
                    "the edge can't pass the opposite one".to_owned(),
                ));
            }
            let lanes = lanes_of(view, lanes)?;
            let (start, end) = span(&doc, view, start, end)?;
            let (bottom, top) = match edge {
                SkewEdge::Top => ((0.0, 0.0), (at_start, at_end)),
                SkewEdge::Bottom => ((at_start, at_end), (1.0, 1.0)),
            };
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_lanes(editor, devices, &lanes, |envelope, travel, _| {
                    envelope.remap_span(start, end, bottom, top, &travel);
                })
            })?;
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::STRETCH_TIME => {
            let StretchTimeArgs {
                lanes,
                start,
                end,
                to_start,
                to_end,
                carry,
            } = parse(args)?;
            let lanes = lanes_of(view, lanes)?;
            let (start, end) = span(&doc, view, start, end)?;
            let signatures = &doc.settings().time_signatures;
            let resolve = |at: Option<Position>, default: Ticks| match at {
                Some(at) => at.resolve(signatures).map_err(EditFailure::InvalidArgs),
                None => Ok(default),
            };
            let (to_start, to_end) = (resolve(to_start, start)?, resolve(to_end, end)?);
            if to_end <= to_start || to_start < Ticks::ZERO {
                return Err(EditFailure::InvalidArgs(
                    "the new span must be after time zero and `to_end` after `to_start`".to_owned(),
                ));
            }
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_lanes(editor, devices, &lanes, |envelope, _, _| {
                    envelope.stretch_span(start, end, to_start, to_end, carry);
                })
            })?;
            // Spec: automation-editing R16. The time selection follows.
            if view.time_selection() == Some((start, end)) {
                view.select_span_on(view.time_selection_lanes().to_vec(), to_start, to_end);
            }
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        ids::SIMPLIFY => {
            let EnvelopeSpanArgs { lanes, start, end } = parse(args)?;
            let lanes = lanes_of(view, lanes)?;
            let (start, end) = span(&doc, view, start, end)?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_lanes(editor, devices, &lanes, |envelope, travel, _| {
                    envelope.simplify_span(start, end, &travel);
                })
            })?;
            Ok(to_value(&result(&doc, editor, view, recorded)))
        }
        other => Err(EditFailure::InvalidArgs(format!(
            "`{other}` isn't an automation editing command"
        ))),
    }
}

/// What a finished edit reports: what changed, and the selection.
fn result(
    before: &Document,
    editor: &Editor,
    view: &ArrangementView,
    undo_step: bool,
) -> AutomationResult {
    AutomationResult {
        changes: ChangeSet::between(before, editor.document()),
        time_selection: view
            .time_selection()
            .map(|(start, end)| bl_shell_spec::arrangement::TimeRange { start, end }),
        undo_step,
    }
}

/// The shape of each lane's envelope over a span, with the values as
/// positions on the parameter's travel.
///
/// Every lane gets an entry, empty where it has no envelope, so a paste
/// writes each shape into the lane it was copied from (the automation
/// editing spec, R5).
fn copy(
    doc: &Document,
    devices: &DeviceView,
    lanes: &[(TrackId, AutomationTarget)],
    start: Ticks,
    end: Ticks,
) -> EnvelopeClipboard {
    let shapes = lanes
        .iter()
        .map(|(track, target)| {
            shape_of(doc, devices, *track, *target, start, end).unwrap_or(EnvelopeShape {
                track: *track,
                target: *target,
                points: Vec::new(),
            })
        })
        .collect();
    EnvelopeClipboard {
        lanes: shapes,
        length: end - start,
    }
}

/// Removes the shape of each lane's envelope over a span.
fn delete_range(
    editor: &mut Editor,
    lanes: &[(TrackId, AutomationTarget)],
    start: Ticks,
    end: Ticks,
) -> Result<(), EditFailure> {
    for (track, target) in lanes {
        remove_shape(editor, *track, *target, start, end)?;
    }
    Ok(())
}

/// Writes a copied shape into each lane's envelope, in the order the
/// shapes were copied. Each value lands at the same position on the
/// target's travel.
fn paste(
    editor: &mut Editor,
    devices: &DeviceView,
    lanes: &[(TrackId, AutomationTarget)],
    copied: &EnvelopeClipboard,
    at: Ticks,
) -> Result<(), EditFailure> {
    let doc = editor.document().clone();
    for (shape, (track, target)) in copied.lanes.iter().zip(lanes) {
        let travel = travel(&doc, devices, *target);
        let points: Vec<Breakpoint> = shape
            .points
            .iter()
            .map(|point| Breakpoint {
                value: travel.value(point.value),
                ..*point
            })
            .collect();
        let mut envelope = envelope_of(editor.document(), *track, *target)
            .cloned()
            .unwrap_or_default();
        envelope.write_span(at, at + copied.length, &points);
        apply(
            editor,
            Command::SetAutomationEnvelope {
                track: *track,
                target: *target,
                points: envelope.points().collect(),
            },
        )?;
    }
    Ok(())
}

/// Moves several breakpoints of one envelope at once: each moves `by` ticks
/// in time and `share` of the parameter's travel in value, and a breakpoint
/// the moved ones pass over is removed.
fn move_breakpoints(
    editor: &mut Editor,
    devices: &DeviceView,
    track: TrackId,
    target: AutomationTarget,
    at: &[Ticks],
    by: Ticks,
    share: f64,
) -> Result<(), EditFailure> {
    let Some(envelope) = envelope_of(editor.document(), track, target).cloned() else {
        return Err(EditFailure::Refused(
            "the parameter has no envelope to move breakpoints on".to_owned(),
        ));
    };
    let travel = travel(editor.document(), devices, target);
    let points = moved_points(&envelope, &travel, at, by, share)?;
    apply(
        editor,
        Command::SetAutomationEnvelope {
            track,
            target,
            points,
        },
    )
}

/// The breakpoints of `envelope` once those at `at` have moved `by` ticks in
/// time and `share` of the travel in value (the automation editing spec, R2,
/// R4); a clip's envelope moves its breakpoints the same way.
pub(crate) fn moved_points(
    envelope: &Envelope,
    travel: &impl ParamScale,
    at: &[Ticks],
    by: Ticks,
    share: f64,
) -> Result<Vec<Breakpoint>, EditFailure> {
    let mut points: Vec<Breakpoint> = envelope.points().collect();
    // A position with no breakpoint gets one at the envelope's value there:
    // the edges of a time selection, when a stretch inside it moves (the
    // automation editing spec, R4).
    for from in at {
        if !points.iter().any(|point| point.at == *from)
            && let Some(value) = envelope.value_at(*from)
        {
            let index = points.partition_point(|point| point.at < *from);
            points.insert(
                index,
                Breakpoint {
                    at: *from,
                    value,
                    bend: 0.0,
                },
            );
        }
    }
    // Where each breakpoint goes.
    let moves: Vec<(Ticks, Ticks, f64)> = at
        .iter()
        .filter_map(|from| {
            let point = points.iter().find(|point| point.at == *from)?;
            let to = (*from + by).max(Ticks::ZERO);
            // A value that doesn't move keeps its own: the travel's way
            // back from a position isn't quite the value it came from.
            let value = if share == 0.0 {
                point.value
            } else {
                travel.value(travel.position(point.value) + share)
            };
            Some((*from, to, value))
        })
        .collect();
    if moves.is_empty() {
        return Err(EditFailure::Refused(
            "there's no breakpoint at the position given".to_owned(),
        ));
    }
    // A breakpoint the moved ones pass over goes.
    let moved: BTreeSet<Ticks> = moves.iter().map(|(from, _, _)| *from).collect();
    points.retain(|point| {
        if moved.contains(&point.at) {
            return true;
        }
        !moves
            .iter()
            .any(|(from, to, _)| point.at >= *from.min(to) && point.at <= *from.max(to))
    });
    // The moved breakpoints go out first, so that one of them landing where
    // another was doesn't lose either.
    let mut landed: Vec<Breakpoint> = Vec::new();
    points.retain(
        |point| match moves.iter().find(|(from, _, _)| *from == point.at) {
            Some((_, to, value)) => {
                landed.push(Breakpoint {
                    at: *to,
                    value: *value,
                    bend: point.bend,
                });
                false
            }
            None => true,
        },
    );
    for point in landed {
        let at = points.partition_point(|other| other.at < point.at);
        // Two moved breakpoints can land on one position when a drag snaps
        // them together: the one moved last keeps the position.
        if points.get(at).is_some_and(|other| other.at == point.at) {
            points[at] = point;
        } else {
            points.insert(at, point);
        }
    }
    Ok(points)
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
