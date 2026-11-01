//! Automation on Push 2: the dots beside a parameter, deleting a
//! parameter's automation with Delete held, and step automation
//! (`docs/specs/push-2-notes.md`, R62, R64 and R66).

use bl_control::{ReadModel, SlotState};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Clip, EnvelopeKind, TrackId};
use serde_json::json;

use crate::content::{Ctx, HeldStep};
use crate::screen::Dot;

/// The clip playing in the Session View on `track`, if any.
fn playing_clip(model: &ReadModel, track: TrackId) -> Option<&Clip> {
    let slot = model.slots.iter().find(|slot| {
        slot.track == track && matches!(slot.state, SlotState::Playing | SlotState::QueuedToStop)
    })?;
    model.document.slot(track, slot.scene)
}

/// The clip the steps edit: the selected slot's.
pub(crate) fn edited_clip(model: &ReadModel) -> Option<&Clip> {
    let &(track, scene) = model.selection.slots.first()?;
    model.document.slot(track, scene)
}

/// The dot for `target` on `track` (R64): white while an enabled Automation
/// envelope of the clip playing on the track, or an Arrangement envelope,
/// drives it, grey while either is overridden.
pub(crate) fn dot(model: &ReadModel, track: TrackId, target: &AutomationTarget) -> Dot {
    let mut enabled = Vec::new();
    if let Some(lane) = model
        .document
        .track(track)
        .and_then(|track| track.automation_for(target))
    {
        enabled.push((lane.enabled, !lane.envelope.is_empty()));
    }
    if let Some(clip) = playing_clip(model, track)
        && let Some(envelope) = clip
            .envelopes
            .iter()
            .find(|e| e.kind == EnvelopeKind::Automation && e.target == *target)
    {
        enabled.push((envelope.enabled, !envelope.envelope.is_empty()));
    }
    if enabled.iter().any(|&(enabled, _)| !enabled) {
        Dot::Overridden
    } else if enabled.iter().any(|&(_, filled)| filled) {
        Dot::Automated
    } else {
        Dot::None
    }
}

/// Delete and a touch (R62): deletes the parameter's automation, in the
/// clip the steps edit, or with the Arrangement in focus the Arrangement's
/// envelope. False where there was none, so the parameter goes back to its
/// default instead.
pub(crate) fn delete(
    ctx: &mut Ctx<'_>,
    model: &ReadModel,
    track: TrackId,
    target: &AutomationTarget,
) -> bool {
    if model.session_view {
        let Some(clip) = edited_clip(model) else {
            return false;
        };
        if !clip
            .envelopes
            .iter()
            .any(|e| e.kind == EnvelopeKind::Automation && e.target == *target)
        {
            return false;
        }
        ctx.cx.run(
            "clip.delete_envelope",
            json!({ "clip": clip.id, "target": target, "kind": "automation" }),
        );
        true
    } else if model
        .document
        .track(track)
        .is_some_and(|t| t.automation_for(target).is_some())
    {
        ctx.cx.run(
            "automation.delete",
            json!({ "track": track, "target": target }),
        );
        true
    } else {
        false
    }
}

/// The value the clip's Automation envelope for `target` holds at the first
/// held step, if it has one (R66).
pub(crate) fn value_at_first_step(
    model: &ReadModel,
    steps: &[HeldStep],
    target: &AutomationTarget,
) -> Option<f64> {
    let first = steps.iter().map(|s| s.start).min()?;
    edited_clip(model)?
        .envelopes
        .iter()
        .find(|e| e.kind == EnvelopeKind::Automation && e.target == *target)?
        .envelope
        .value_at(first)
}

/// Writes `value` over every held step's span in the clip's Automation
/// envelope for `target`: the envelope takes it from the step's start to its
/// end, with breakpoints at the edges (R66). The commands are part of the
/// gesture on `gesture`, so one touch is one undo step.
pub(crate) fn write_steps(
    ctx: &mut Ctx<'_>,
    gesture: u32,
    clip: bl_model::ClipId,
    target: &AutomationTarget,
    value: f64,
) {
    for step in ctx.held_steps {
        let end: Ticks = step.start + step.length;
        for at in [step.start, end] {
            ctx.cx.run_held(
                gesture,
                "clip.set_envelope_breakpoint",
                json!({
                    "clip": clip,
                    "target": target,
                    "kind": "automation",
                    "at": at,
                    "value": value,
                }),
            );
        }
    }
}
