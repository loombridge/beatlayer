//! The Clip View's Envelope Editor in the command registry (WP-6.25c): a
//! time selection in the editor, moving several breakpoints at once, the
//! edit commands, the automation shapes, stretching, skewing and Simplify
//! Envelope, on a clip's envelope.
//!
//! They are the automation editing commands ([`crate::automation`]) for a
//! clip's envelope: positions are ticks from the beginning of the clip's
//! contents or bars, beats and sixteenths ([`Position`]), and values are on
//! the envelope's own scale, the modulation scale of a Modulation envelope.
//! Each takes the clip, the parameter and the kind of envelope as the other
//! clip envelope commands do, and each of those left out is the Clip View's:
//! the shown clip, the parameter the Envelopes tab shows and Modulation. A
//! span left out is the editor's time selection. Every edit is one undo
//! step.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID. The behaviour they follow is in the clip envelopes spec (R12, R13).

use bl_commands::Context::{ClipView, Envelope};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, ChangeSet, ClipId, EnvelopeKind, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::arrangement::TimeRange;
use crate::automation::SkewEdge;
use crate::editing::Position;

/// `clip.select_envelope_time`: select time in the Envelope Editor.
pub const SELECT_TIME: &str = "clip.select_envelope_time";
/// `clip.select_envelope_segment`: select a segment in the Envelope Editor.
pub const SELECT_SEGMENT: &str = "clip.select_envelope_segment";
/// `clip.move_envelope_breakpoints`: move several breakpoints of a clip's
/// envelope as one.
pub const MOVE_BREAKPOINTS: &str = "clip.move_envelope_breakpoints";
/// `clip.copy_envelope`: copy the shape of a clip's envelope over a span.
pub const COPY: &str = "clip.copy_envelope";
/// `clip.cut_envelope`: copy the shape of a clip's envelope over a span, then
/// delete it.
pub const CUT: &str = "clip.cut_envelope";
/// `clip.delete_envelope_range`: delete the shape of a clip's envelope over a
/// span.
pub const DELETE_RANGE: &str = "clip.delete_envelope_range";
/// `clip.paste_envelope`: paste a copied shape into a clip's envelope.
pub const PASTE: &str = "clip.paste_envelope";
/// `clip.duplicate_envelope`: write a copy of a shape straight after it.
pub const DUPLICATE: &str = "clip.duplicate_envelope";
/// `clip.insert_envelope_shape`: write a ready-made shape over a span.
pub const INSERT_SHAPE: &str = "clip.insert_envelope_shape";
/// `clip.stretch_envelope_values`: stretch the values of a span between two
/// edges.
pub const STRETCH_VALUES: &str = "clip.stretch_envelope_values";
/// `clip.stretch_envelope_time`: stretch a span in time.
pub const STRETCH_TIME: &str = "clip.stretch_envelope_time";
/// `clip.skew_envelope`: tilt the top or bottom edge of a span's values.
pub const SKEW: &str = "clip.skew_envelope";
/// `clip.simplify_envelope`: replace a span's breakpoints with fewer.
pub const SIMPLIFY: &str = "clip.simplify_envelope";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SELECT_TIME,
    SELECT_SEGMENT,
    MOVE_BREAKPOINTS,
    COPY,
    CUT,
    DELETE_RANGE,
    PASTE,
    DUPLICATE,
    INSERT_SHAPE,
    STRETCH_VALUES,
    STRETCH_TIME,
    SKEW,
    SIMPLIFY,
];

/// Whether `id` is one of the Envelope Editor's editing commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `clip.select_envelope_time`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectTimeArgs {
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Where the selection starts, in the clip's contents. Left out with
    /// `end`, the selection is let go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where it ends, after `start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// Arguments of `clip.select_envelope_segment`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectSegmentArgs {
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The breakpoint the segment starts at, or `null` to select no
    /// segment.
    #[serde(default)]
    pub at: Option<Ticks>,
}

/// Arguments of `clip.move_envelope_breakpoints`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveBreakpointsArgs {
    /// The breakpoints to move, where they are now.
    pub at: Vec<Ticks>,
    /// How far to move them in time, in ticks; negative is earlier.
    pub by: Ticks,
    /// How far to move their values, as a share of the envelope's travel:
    /// 0.1 is a tenth of the way up the editor, and −0.1 a tenth down.
    pub share: f64,
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The parameter of the clip's track. Left out, the one the Envelopes
    /// tab shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AutomationTarget>,
    /// Which of the clip's envelopes for the parameter: `modulation` (the
    /// default) or a Session clip's `automation`.
    #[serde(default)]
    pub kind: EnvelopeKind,
}

/// Arguments of the commands that act on the shape of an envelope over a
/// span: `clip.copy_envelope`, `clip.cut_envelope`,
/// `clip.delete_envelope_range`, `clip.duplicate_envelope` and
/// `clip.simplify_envelope`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpanArgs {
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The parameter of the clip's track. Left out, the one the Envelopes
    /// tab shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AutomationTarget>,
    /// Which of the clip's envelopes for the parameter: `modulation` (the
    /// default) or a Session clip's `automation`.
    #[serde(default)]
    pub kind: EnvelopeKind,
    /// Where the span starts, in the clip's contents. Left out, the
    /// editor's time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends, after `start`. Left out, the time selection's
    /// end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// Arguments of `clip.paste_envelope`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteArgs {
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The parameter of the clip's track. Left out, the one the Envelopes
    /// tab shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AutomationTarget>,
    /// Which of the clip's envelopes for the parameter: `modulation` (the
    /// default) or a Session clip's `automation`.
    #[serde(default)]
    pub kind: EnvelopeKind,
    /// Where the shape starts. Left out, the time selection's start, or the
    /// insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
}

/// Arguments of `clip.insert_envelope_shape`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertShapeArgs {
    /// The shape to write.
    pub shape: Shape,
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The parameter of the clip's track. Left out, the one the Envelopes
    /// tab shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AutomationTarget>,
    /// Which of the clip's envelopes for the parameter: `modulation` (the
    /// default) or a Session clip's `automation`.
    #[serde(default)]
    pub kind: EnvelopeKind,
    /// Where the span starts. Left out, the time selection's start, or the
    /// insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends, after `start`. Left out, the time selection's
    /// end, or one grid step after the insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// Arguments of `clip.stretch_envelope_values`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StretchValuesArgs {
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The parameter of the clip's track. Left out, the one the Envelopes
    /// tab shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AutomationTarget>,
    /// Which of the clip's envelopes for the parameter: `modulation` (the
    /// default) or a Session clip's `automation`.
    #[serde(default)]
    pub kind: EnvelopeKind,
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends. Left out, the time selection's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// Where the box's bottom edge goes, as a share of the envelope's
    /// travel: 0 is the bottom of the editor, which is where it starts. It
    /// can't pass `top`.
    #[serde(default)]
    pub bottom: f64,
    /// Where the box's top edge goes: 1 is the top of the editor, which is
    /// where it starts. Past 1 or below 0, values are clipped to the travel.
    #[serde(default = "one")]
    pub top: f64,
}

fn one() -> f64 {
    1.0
}

/// Arguments of `clip.skew_envelope`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkewArgs {
    /// The edge to tilt.
    pub edge: SkewEdge,
    /// Where the edge goes at the span's start, as a share of the
    /// envelope's travel.
    pub at_start: f64,
    /// Where it goes at the span's end.
    pub at_end: f64,
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The parameter of the clip's track. Left out, the one the Envelopes
    /// tab shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AutomationTarget>,
    /// Which of the clip's envelopes for the parameter: `modulation` (the
    /// default) or a Session clip's `automation`.
    #[serde(default)]
    pub kind: EnvelopeKind,
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends. Left out, the time selection's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// Arguments of `clip.stretch_envelope_time`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StretchTimeArgs {
    /// The clip. Left out, the shown clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The parameter of the clip's track. Left out, the one the Envelopes
    /// tab shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AutomationTarget>,
    /// Which of the clip's envelopes for the parameter: `modulation` (the
    /// default) or a Session clip's `automation`.
    #[serde(default)]
    pub kind: EnvelopeKind,
    /// Where the span starts now. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends now. Left out, the time selection's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// Where the span starts afterwards. Left out, where it starts now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_start: Option<Position>,
    /// Where the span ends afterwards. Left out, where it ends now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_end: Option<Position>,
    /// Move the breakpoints beyond a moving end with it, instead of removing
    /// the ones it passes over.
    #[serde(default)]
    pub carry: bool,
}

/// What an Envelope Editor editing command reports: what it changed in the
/// set, and the selection afterwards.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipEnvelopeResult {
    /// What the edit changed in the set.
    pub changes: ChangeSet,
    /// The time selected in the editor afterwards, in ticks from the
    /// beginning of the clip's contents, if any.
    pub time_selection: Option<TimeRange>,
    /// Whether the edit was recorded as an undo step: false when it
    /// changed nothing.
    pub undo_step: bool,
}

/// What `clip.copy_envelope` put on the clipboard.
pub use crate::automation::EnvelopeClipboardState;

/// Adds the Envelope Editor's editing commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

/// The start of an Envelope Editor command's spec.
fn edit(
    id: &'static str,
    label: &'static str,
    permission: Permission,
    description: &'static str,
) -> CommandSpec {
    CommandSpec::new(id, label)
        .description(description)
        .permission(permission)
}

fn specs() -> Vec<CommandSpec> {
    vec![
        edit(
            SELECT_TIME,
            "Select Envelope Time",
            Permission::Read,
            "Selects a span of time in the Clip View's Envelope Editor, as dragging across its \
             background does: the shown envelope's breakpoints from `start` up to, not \
             including, `end` are selected, and the edit commands, the shapes, the stretch and \
             skew handles and Simplify Envelope act on the span. Left out, `start` and `end` \
             let the selection go. Changes nothing in the set.",
        )
        .contexts([ClipView, Envelope])
        .args::<SelectTimeArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            SELECT_SEGMENT,
            "Select Envelope Segment",
            Permission::Read,
            "Selects a segment in the Clip View's Envelope Editor, as clicking near it does: its \
             two breakpoints are drawn in the selection colour and move together when it is \
             dragged. `at` is the breakpoint the segment starts at, or `null` to select no \
             segment. Changes nothing in the set.",
        )
        .contexts([ClipView, Envelope])
        .args::<SelectSegmentArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            MOVE_BREAKPOINTS,
            "Move Envelope Breakpoints",
            Permission::Edit,
            "Moves several breakpoints of a clip's envelope at once, as dragging a selected \
             breakpoint in the Envelope Editor does: each moves `by` ticks in time and `share` \
             of the envelope's travel in value, staying within the envelope's range, and a \
             breakpoint the moved ones pass over is removed. `at` names the breakpoints to move, \
             where they are now, in ticks from the beginning of the clip's contents; a position \
             with no breakpoint gets one at the envelope's value there. One undo step.",
        )
        .contexts([ClipView, Envelope])
        .args::<MoveBreakpointsArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            COPY,
            "Copy Envelope",
            Permission::Read,
            "Copies the shape of a clip's envelope over the span (the Envelope Editor's time \
             selection) to the clipboard automation editing shares, for `clip.paste_envelope` \
             and `automation.paste`: the shape over the span, with its positions from the \
             span's start, its values as positions on the envelope's travel, and its bends. The \
             clipboard isn't part of the set, and copying changes nothing in it.",
        )
        .contexts([Envelope])
        .binding("S-EDIT-002")
        .args::<SpanArgs>()
        .result::<EnvelopeClipboardState>(),
        edit(
            CUT,
            "Cut Envelope",
            Permission::Destructive,
            "Copies the shape of a clip's envelope over the span, as `clip.copy_envelope` does, \
             then deletes it, as `clip.delete_envelope_range` does: the breakpoints inside the \
             span go, and the envelope's values at the span's edges are kept, so it runs \
             straight across the span. Because it removes the person's work, an agent asks \
             first.",
        )
        .contexts([Envelope])
        .binding("S-EDIT-001")
        .args::<SpanArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            DELETE_RANGE,
            "Delete Envelope Range",
            Permission::Destructive,
            "Deletes the shape of a clip's envelope over the span: the breakpoints inside it go, \
             and the envelope's values at the span's edges are kept with a breakpoint at each, \
             so the envelope runs straight across the span and is unchanged outside it. The \
             clip's notes and its other envelopes stay as they are. Because it removes the \
             person's work, an agent asks first. Delete (S-EDIT-005) runs it in the Envelope \
             Editor, through `envelope.delete_breakpoint`, while a time is selected and no \
             breakpoint is.",
        )
        .contexts([ClipView, Envelope])
        .args::<SpanArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            PASTE,
            "Paste Envelope",
            Permission::Edit,
            "Pastes the clipboard's first shape into a clip's envelope, starting at `at` (left \
             out, the time selection's start or the insert marker) and replacing the envelope \
             over the shape's length. Each value lands at the same position on the envelope's \
             travel, so a shape pastes onto another parameter. One undo step.",
        )
        .contexts([Envelope])
        .binding("S-EDIT-003")
        .args::<PasteArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            DUPLICATE,
            "Duplicate Envelope",
            Permission::Edit,
            "Writes a copy of the shape of a clip's envelope over the span straight after the \
             span, and makes the copy the time selection, so running it again repeats the \
             shape. One undo step.",
        )
        .contexts([Envelope])
        .binding("S-EDIT-004")
        .args::<SpanArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            INSERT_SHAPE,
            "Insert Shape",
            Permission::Edit,
            "Writes a ready-made shape into a clip's envelope over the span, replacing the \
             envelope there: `sine`, `triangle`, `sawtooth`, `inverse_sawtooth` or `square` run \
             one cycle between the bottom and top of the envelope's range (0 to 100% for \
             Volume and a send's level, -100% to 100% for the rest of a Modulation envelope), \
             and `rise`, `fall`, `rise_to`, `fall_to` and `adsr` join the envelope's value \
             before or after the span. With no time selection, the span is one grid step from \
             the insert marker. The breakpoints follow the shape within 1% of the envelope's \
             travel, and the envelope outside the span stays as it was. One undo step.",
        )
        .contexts([ClipView, Envelope])
        .args::<InsertShapeArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            STRETCH_VALUES,
            "Stretch Envelope Values",
            Permission::Edit,
            "Stretches the values of the breakpoints of a clip's envelope in the span between a \
             new bottom and top edge, as dragging the top or bottom handle of the Envelope \
             Editor's time selection does: each moves to the same position between the new \
             edges as it had between the old, which are the whole travel, and a value past the \
             envelope's range is clipped to it. `bottom` and `top` are shares of the travel, \
             and the bottom can't pass the top. One undo step.",
        )
        .contexts([ClipView, Envelope])
        .args::<StretchValuesArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            STRETCH_TIME,
            "Stretch Envelope in Time",
            Permission::Edit,
            "Stretches the breakpoints of a clip's envelope in the span into a new span, as \
             dragging the left or right handle of the Envelope Editor's time selection does: \
             each moves to the same place in the new span, and the time selection follows. A \
             breakpoint outside the span that a moving end passes over is removed; with \
             `carry`, the breakpoints beyond that end move with it instead. One undo step.",
        )
        .contexts([ClipView, Envelope])
        .args::<StretchTimeArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            SKEW,
            "Skew Envelope",
            Permission::Edit,
            "Tilts the top or bottom edge of the box round the values of a clip's envelope in \
             the span, as dragging a corner handle of the Envelope Editor's time selection \
             does: `at_start` and `at_end` are where the `edge` runs at the span's start and \
             end, as shares of the envelope's travel, and each breakpoint in the span moves to \
             the same position between the bottom and top edges at its own time. The top edge \
             can't pass the bottom. One undo step.",
        )
        .contexts([ClipView, Envelope])
        .args::<SkewArgs>()
        .result::<ClipEnvelopeResult>(),
        edit(
            SIMPLIFY,
            "Simplify Envelope",
            Permission::Edit,
            "Replaces the breakpoints of a clip's envelope inside the span with as few as keep \
             the envelope within 0.5% of its travel of what it was, using curved segments where \
             one follows the shape. The values at the span's edges and the envelope outside the \
             span are kept, and no breakpoint is added. One undo step.",
        )
        .contexts([ClipView, Envelope])
        .args::<SpanArgs>()
        .result::<ClipEnvelopeResult>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        // Deleting work is destructive; copying and selecting change nothing.
        let class = |id: &str| registry.get(id).unwrap().permission;
        for id in [SELECT_TIME, SELECT_SEGMENT, COPY] {
            assert_eq!(class(id), Some(Permission::Read), "{id}");
        }
        for id in [CUT, DELETE_RANGE] {
            assert_eq!(class(id), Some(Permission::Destructive), "{id}");
        }
        for id in [
            MOVE_BREAKPOINTS,
            PASTE,
            DUPLICATE,
            INSERT_SHAPE,
            STRETCH_VALUES,
            STRETCH_TIME,
            SKEW,
            SIMPLIFY,
        ] {
            assert_eq!(class(id), Some(Permission::Edit), "{id}");
        }
    }
}
