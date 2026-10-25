//! The Clip View's Envelope Editor's editing (WP-6.25c): a time selection in
//! the editor, moving several breakpoints at once, the edit commands, the
//! automation shapes, stretching, skewing and Simplify Envelope, on a clip's
//! envelope.
//!
//! They are the Arrangement's automation editing ([`crate::arrangement::automation`])
//! on a clip's envelope, working on the same model functions: positions are
//! ticks from the beginning of the clip's contents, and values sit on the
//! envelope's scale, the modulation scale for a Modulation envelope. Copied
//! shapes go on the clipboard automation editing keeps, with their values as
//! positions on the travel, so a shape pastes onto another parameter or into
//! an automation lane.
//!
//! The behaviour is in the clip envelopes spec (R12, R13).

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Breakpoint, ChangeSet, ClipId, Command, Document, Editor, Envelope,
    EnvelopeKind,
};
use bl_shell_spec::arrangement::TimeRange;
use bl_shell_spec::automation::SkewEdge;
use bl_shell_spec::clip_envelope_editing::{
    self as ids, ClipEnvelopeResult, InsertShapeArgs, MoveBreakpointsArgs, PasteArgs, SkewArgs,
    SpanArgs, StretchTimeArgs, StretchValuesArgs,
};
use bl_shell_spec::editing::Position;
use serde::Serialize;
use serde_json::Value;

use crate::arrangement::automation::{
    EnvelopeClipboard, EnvelopeShape, Travel, moved_points, rest_of, share_scale, travel,
};
use crate::arrangement::editing::{EditFailure, transaction};
use crate::arrangement::{ArrangementView, parse};
use crate::clip_view::ClipView;
use crate::devices::DeviceView;

/// The envelope a command acts on.
#[derive(Clone, Copy, Debug)]
struct Which {
    clip: ClipId,
    target: AutomationTarget,
    kind: EnvelopeKind,
}

/// The envelope the arguments name, or the shown clip's shown one.
fn which(
    doc: &Document,
    clip_view: &ClipView,
    arrangement: &ArrangementView,
    clip: Option<ClipId>,
    target: Option<AutomationTarget>,
    kind: EnvelopeKind,
) -> Result<Which, EditFailure> {
    let clip = clip
        .or_else(|| clip_view.shown_any_clip(doc, arrangement))
        .ok_or_else(|| EditFailure::Unavailable("no clip is shown".to_owned()))?;
    if doc.clip(clip).is_none() {
        return Err(EditFailure::InvalidArgs(format!(
            "the set has no clip {clip:?}"
        )));
    }
    let target = target.or_else(|| clip_view.envelope()).ok_or_else(|| {
        EditFailure::Unavailable("the Envelopes tab shows no parameter".to_owned())
    })?;
    Ok(Which { clip, target, kind })
}

/// The scale the envelope sits on, and the value that modulates nothing (or,
/// for a Session clip's automation, the control's own value), which a ramp
/// joins where the envelope has none.
fn scale_of(doc: &Document, devices: &DeviceView, which: Which) -> (Travel, f64) {
    match which.kind {
        EnvelopeKind::Modulation => (share_scale(which.target), rest_of(which.target)),
        EnvelopeKind::Automation => {
            let scale = travel(doc, devices, which.target);
            let rest = doc
                .clip_track(which.clip)
                .and_then(|track| {
                    crate::modulation::control_value(doc, devices, track, &which.target)
                })
                .unwrap_or_else(|| scale.value(0.5));
            (scale, rest)
        }
    }
}

fn envelope_of(doc: &Document, which: Which) -> Option<&Envelope> {
    doc.clip(which.clip)?
        .envelope_of(&which.target, which.kind)
        .map(|clip_envelope| &clip_envelope.envelope)
}

/// The parameter the Envelopes tab shows for `clip`, while its track still
/// has it.
#[must_use]
pub fn shown(
    doc: &Document,
    clip_view: &ClipView,
    arrangement: &ArrangementView,
) -> Option<(ClipId, AutomationTarget)> {
    let clip = clip_view.shown_any_clip(doc, arrangement)?;
    let target = clip_view.envelope()?;
    let track = doc.clip_track(clip)?;
    doc.check_clip_target(track, doc.clip(clip)?, target).ok()?;
    Some((clip, target))
}

/// The breakpoints of the shown envelope that are selected: the two of the
/// segment that was clicked, or those the time selection covers, from its
/// start up to, not including, its end (the automation editing spec, R2, R4).
#[must_use]
pub fn selected_breakpoints(
    doc: &Document,
    clip_view: &ClipView,
    clip: ClipId,
    target: AutomationTarget,
) -> Vec<Ticks> {
    let Some(envelope) = doc
        .clip(clip)
        .and_then(|clip| clip.envelope(&target))
        .map(|clip_envelope| &clip_envelope.envelope)
    else {
        return Vec::new();
    };
    if let Some(at) = clip_view.envelope_segment(clip, target) {
        let mut out = vec![at];
        out.extend(envelope.points().find(|point| point.at > at).map(|p| p.at));
        return out;
    }
    let Some((start, end)) = clip_view.envelope_time(clip) else {
        return Vec::new();
    };
    envelope
        .points()
        .filter(|point| point.at >= start && point.at < end)
        .map(|point| point.at)
        .collect()
}

/// A span from positions given, or the editor's time selection.
fn span(
    doc: &Document,
    clip_view: &ClipView,
    clip: ClipId,
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
        (None, None) => clip_view
            .envelope_time(clip)
            .ok_or_else(|| EditFailure::Unavailable("no time is selected".to_owned()))?,
        _ => {
            return Err(EditFailure::InvalidArgs(
                "give `start` and `end`, or neither".to_owned(),
            ));
        }
    };
    if start < Ticks::ZERO || end <= start {
        return Err(EditFailure::InvalidArgs(
            "the span's `end` must be after its `start`, which is 0 or later".to_owned(),
        ));
    }
    Ok((start, end))
}

/// Writes `envelope` as the envelope the arguments name.
fn write(editor: &mut Editor, which: Which, envelope: &Envelope) -> Result<(), EditFailure> {
    editor
        .apply(Command::SetClipEnvelope {
            clip: which.clip,
            target: which.target,
            kind: which.kind,
            points: envelope.points().collect(),
        })
        .map(|_| ())?;
    Ok(())
}

/// Edits the envelope with `edit`, which gets its scale and the value a ramp
/// joins where it has none, and writes the result; an edit that leaves it as
/// it was writes nothing.
fn edit_envelope(
    editor: &mut Editor,
    devices: &DeviceView,
    which: Which,
    edit: impl FnOnce(&mut Envelope, Travel, f64),
) -> Result<(), EditFailure> {
    let doc = editor.document();
    let (scale, rest) = scale_of(doc, devices, which);
    let before = envelope_of(doc, which).cloned();
    let mut envelope = before.clone().unwrap_or_default();
    edit(&mut envelope, scale, rest);
    if before.as_ref() == Some(&envelope) || (before.is_none() && envelope.is_empty()) {
        return Ok(());
    }
    write(editor, which, &envelope)
}

/// The shape of the envelope over a span, with the values as positions on its
/// scale.
fn copy(
    doc: &Document,
    devices: &DeviceView,
    which: Which,
    start: Ticks,
    end: Ticks,
) -> Result<EnvelopeClipboard, EditFailure> {
    let track = doc
        .clip_track(which.clip)
        .ok_or_else(|| EditFailure::Unavailable("the clip is on no track".to_owned()))?;
    let (scale, _) = scale_of(doc, devices, which);
    let points = envelope_of(doc, which)
        .map(|envelope| {
            envelope
                .shape_in(start, end)
                .into_iter()
                .map(|point| Breakpoint {
                    value: scale.position(point.value),
                    ..point
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(EnvelopeClipboard {
        lanes: vec![EnvelopeShape {
            track,
            target: which.target,
            points,
        }],
        length: end - start,
    })
}

/// Writes the clipboard's first shape into the envelope from `at`, each value
/// at the same position on the envelope's scale.
fn paste(
    editor: &mut Editor,
    devices: &DeviceView,
    which: Which,
    copied: &EnvelopeClipboard,
    at: Ticks,
) -> Result<(), EditFailure> {
    let Some(shape) = copied.lanes.first() else {
        return Err(EditFailure::Unavailable(
            "there's nothing on the clipboard to paste".to_owned(),
        ));
    };
    edit_envelope(editor, devices, which, |envelope, scale, _| {
        let points: Vec<Breakpoint> = shape
            .points
            .iter()
            .map(|point| Breakpoint {
                value: scale.value(point.value),
                ..*point
            })
            .collect();
        envelope.write_span(at, at + copied.length, &points);
    })
}

/// The name of the undo step an Envelope Editor command makes.
#[must_use]
pub fn label(id: &str) -> &'static str {
    match id {
        ids::MOVE_BREAKPOINTS => "Move Envelope Breakpoints",
        ids::CUT => "Cut Envelope",
        ids::DELETE_RANGE => "Delete Envelope",
        ids::PASTE => "Paste Envelope",
        ids::DUPLICATE => "Duplicate Envelope",
        ids::INSERT_SHAPE => "Insert Shape",
        ids::STRETCH_VALUES => "Stretch Envelope Values",
        ids::STRETCH_TIME => "Stretch Envelope in Time",
        ids::SKEW => "Skew Envelope",
        ids::SIMPLIFY => "Simplify Envelope",
        _ => "Envelope",
    }
}

/// Whether a person can run Envelope Editor command `id` now. The edit
/// commands the Edit menu and keys reach (`keyed`) need the editor to have
/// the focus, with a time selection to act on or, for Paste, something on the
/// clipboard.
#[must_use]
pub fn can_run(
    id: &str,
    doc: &Document,
    clip_view: &ClipView,
    arrangement: &ArrangementView,
    clipboard: Option<&EnvelopeClipboard>,
    keyed: bool,
) -> bool {
    let Some((clip, _)) = shown(doc, clip_view, arrangement) else {
        return false;
    };
    let selected = clip_view.envelope_time(clip).is_some();
    match id {
        ids::COPY | ids::CUT | ids::DELETE_RANGE | ids::DUPLICATE => keyed && selected,
        ids::PASTE => keyed && clipboard.is_some_and(|copied| !copied.lanes.is_empty()),
        // Spec: automation-editing R18. Simplify Envelope needs a time
        // selection.
        ids::SIMPLIFY => selected,
        _ => true,
    }
}

/// Runs Envelope Editor command `id` with `args`, as `Shell::run` does.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    clip_view: &mut ClipView,
    arrangement: &ArrangementView,
    clipboard: &mut Option<EnvelopeClipboard>,
    devices: &DeviceView,
) -> Result<Value, EditFailure> {
    let doc = editor.document().clone();
    match id {
        ids::SELECT_TIME => {
            let ids::SelectTimeArgs { clip, start, end } = parse(args)?;
            let clip = clip
                .or_else(|| clip_view.shown_any_clip(&doc, arrangement))
                .ok_or_else(|| EditFailure::Unavailable("no clip is shown".to_owned()))?;
            let span = match (start, end) {
                (None, None) => None,
                (start, end) => Some(span(&doc, clip_view, clip, start, end)?),
            };
            clip_view.select_envelope_time(clip, span);
            Ok(to_value(&result(&doc, editor, clip_view, clip, false)))
        }
        ids::SELECT_SEGMENT => {
            let ids::SelectSegmentArgs { clip, at } = parse(args)?;
            let which = which(
                &doc,
                clip_view,
                arrangement,
                clip,
                None,
                EnvelopeKind::Modulation,
            )?;
            clip_view.select_envelope_segment(which.clip, which.target, at);
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, false,
            )))
        }
        ids::MOVE_BREAKPOINTS => {
            let MoveBreakpointsArgs {
                at,
                by,
                share,
                clip,
                target,
                kind,
            } = parse(args)?;
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                let Some(envelope) = envelope_of(editor.document(), which).cloned() else {
                    return Err(EditFailure::Refused(
                        "the parameter has no envelope to move breakpoints on".to_owned(),
                    ));
                };
                let (scale, _) = scale_of(editor.document(), devices, which);
                let points = moved_points(&envelope, &scale, &at, by, share)?;
                let moved = Envelope::from_points(points)
                    .map_err(|error| EditFailure::Refused(error.to_string()))?;
                write(editor, which, &moved)
            })?;
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        ids::COPY => {
            let SpanArgs {
                clip,
                target,
                kind,
                start,
                end,
            } = parse(args)?;
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (start, end) = span(&doc, clip_view, which.clip, start, end)?;
            let copied = copy(&doc, devices, which, start, end)?;
            let state = copied.state();
            *clipboard = Some(copied);
            Ok(to_value(&state))
        }
        ids::CUT | ids::DELETE_RANGE => {
            let SpanArgs {
                clip,
                target,
                kind,
                start,
                end,
            } = parse(args)?;
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (start, end) = span(&doc, clip_view, which.clip, start, end)?;
            let copied = (id == ids::CUT)
                .then(|| copy(&doc, devices, which, start, end))
                .transpose()?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_envelope(editor, devices, which, |envelope, _, _| {
                    envelope.remove_span(start, end);
                })
            })?;
            if copied.is_some() {
                *clipboard = copied;
            }
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        ids::PASTE => {
            let PasteArgs {
                clip,
                target,
                kind,
                at,
            } = parse(args)?;
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let Some(copied) = clipboard.as_ref().filter(|c| !c.lanes.is_empty()) else {
                return Err(EditFailure::Unavailable(
                    "there's nothing on the clipboard to paste".to_owned(),
                ));
            };
            let at = match at {
                Some(at) => at
                    .resolve(&doc.settings().time_signatures)
                    .map_err(EditFailure::InvalidArgs)?,
                None => clip_view
                    .envelope_time(which.clip)
                    .map_or_else(|| clip_view.envelope_marker(which.clip), |(start, _)| start),
            };
            let (_, recorded) = transaction(editor, label(id), |editor| {
                paste(editor, devices, which, copied, at)
            })?;
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        ids::DUPLICATE => {
            let SpanArgs {
                clip,
                target,
                kind,
                start,
                end,
            } = parse(args)?;
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (start, end) = span(&doc, clip_view, which.clip, start, end)?;
            let copied = copy(&doc, devices, which, start, end)?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                paste(editor, devices, which, &copied, end)
            })?;
            // The copy is the time selection.
            clip_view.select_envelope_time(which.clip, Some((end, end + copied.length)));
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        ids::INSERT_SHAPE => {
            let InsertShapeArgs {
                shape,
                clip,
                target,
                kind,
                start,
                end,
            } = parse(args)?;
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (start, end) = match (start, end, clip_view.envelope_time(which.clip)) {
                // Spec: automation-editing R11. With no time selection, one
                // grid step from the insert marker.
                (None, None, None) => {
                    let at = clip_view.envelope_marker(which.clip);
                    (at, at + arrangement.grid_step(&doc).ticks)
                }
                (start, end, _) => span(&doc, clip_view, which.clip, start, end)?,
            };
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_envelope(editor, devices, which, |envelope, scale, rest| {
                    envelope.insert_shape(shape, start, end, rest, &scale);
                })
            })?;
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        ids::STRETCH_VALUES => {
            let StretchValuesArgs {
                clip,
                target,
                kind,
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
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (start, end) = span(&doc, clip_view, which.clip, start, end)?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_envelope(editor, devices, which, |envelope, scale, _| {
                    envelope.remap_span(start, end, (bottom, bottom), (top, top), &scale);
                })
            })?;
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        ids::SKEW => {
            let SkewArgs {
                edge,
                at_start,
                at_end,
                clip,
                target,
                kind,
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
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (start, end) = span(&doc, clip_view, which.clip, start, end)?;
            let (bottom, top) = match edge {
                SkewEdge::Top => ((0.0, 0.0), (at_start, at_end)),
                SkewEdge::Bottom => ((at_start, at_end), (1.0, 1.0)),
            };
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_envelope(editor, devices, which, |envelope, scale, _| {
                    envelope.remap_span(start, end, bottom, top, &scale);
                })
            })?;
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        ids::STRETCH_TIME => {
            let StretchTimeArgs {
                clip,
                target,
                kind,
                start,
                end,
                to_start,
                to_end,
                carry,
            } = parse(args)?;
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (start, end) = span(&doc, clip_view, which.clip, start, end)?;
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
                edit_envelope(editor, devices, which, |envelope, _, _| {
                    envelope.stretch_span(start, end, to_start, to_end, carry);
                })
            })?;
            // Spec: automation-editing R16. The time selection follows.
            if clip_view.envelope_time(which.clip) == Some((start, end)) {
                clip_view.select_envelope_time(which.clip, Some((to_start, to_end)));
            }
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        ids::SIMPLIFY => {
            let SpanArgs {
                clip,
                target,
                kind,
                start,
                end,
            } = parse(args)?;
            let which = which(&doc, clip_view, arrangement, clip, target, kind)?;
            let (start, end) = span(&doc, clip_view, which.clip, start, end)?;
            let (_, recorded) = transaction(editor, label(id), |editor| {
                edit_envelope(editor, devices, which, |envelope, scale, _| {
                    envelope.simplify_span(start, end, &scale);
                })
            })?;
            Ok(to_value(&result(
                &doc, editor, clip_view, which.clip, recorded,
            )))
        }
        other => Err(EditFailure::InvalidArgs(format!(
            "`{other}` isn't an Envelope Editor command"
        ))),
    }
}

/// What a finished edit reports: what changed, and the selection.
fn result(
    before: &Document,
    editor: &Editor,
    clip_view: &ClipView,
    clip: ClipId,
    undo_step: bool,
) -> ClipEnvelopeResult {
    ClipEnvelopeResult {
        changes: ChangeSet::between(before, editor.document()),
        time_selection: clip_view
            .envelope_time(clip)
            .map(|(start, end)| TimeRange { start, end }),
        undo_step,
    }
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
