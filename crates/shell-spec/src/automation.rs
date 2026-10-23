//! The Arrangement's automation editing commands in the command registry
//! (WP-6.25, WP-6.25b): moving the breakpoints of an envelope, the clipboard
//! of envelope shapes, the edit commands that act on them, and inserting
//! shapes, stretching, skewing and simplifying.
//!
//! Each acts on the lanes of a span of time, given explicitly in its
//! arguments or, when they're left out, on the lanes the Arrangement's time
//! selection covers. Every edit is one undo step. Positions are ticks or
//! bars, beats, and sixteenths ([`Position`]).
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID. The behaviour they follow is in the automation editing spec.

use bl_commands::Context::{Arrangement, Envelope};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, ChangeSet, Shape, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::arrangement::{Lane, TimeRange};
use crate::editing::Position;

/// `automation.move_breakpoints`: move several breakpoints of one envelope
/// as one.
pub const MOVE_BREAKPOINTS: &str = "automation.move_breakpoints";
/// `automation.copy`: copy the shape of an envelope over a span.
pub const COPY: &str = "automation.copy";
/// `automation.cut`: copy the shape of an envelope over a span, then delete
/// it.
pub const CUT: &str = "automation.cut";
/// `automation.delete_range`: delete the shape of an envelope over a span.
pub const DELETE_RANGE: &str = "automation.delete_range";
/// `automation.paste`: paste a copied shape into envelopes.
pub const PASTE: &str = "automation.paste";
/// `automation.duplicate`: write a copy of a shape straight after it.
pub const DUPLICATE: &str = "automation.duplicate";
/// `automation.select_segment`: select a segment in an automation lane.
pub const SELECT_SEGMENT: &str = "automation.select_segment";
/// `automation.insert_shape`: write a ready-made shape over a span.
pub const INSERT_SHAPE: &str = "automation.insert_shape";
/// `automation.stretch_values`: stretch the values of a span between two
/// edges.
pub const STRETCH_VALUES: &str = "automation.stretch_values";
/// `automation.stretch_time`: stretch a span in time.
pub const STRETCH_TIME: &str = "automation.stretch_time";
/// `automation.skew`: tilt the top or bottom edge of a span's values.
pub const SKEW: &str = "automation.skew";
/// `automation.simplify`: replace a span's breakpoints with fewer.
pub const SIMPLIFY: &str = "automation.simplify";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    MOVE_BREAKPOINTS,
    COPY,
    CUT,
    DELETE_RANGE,
    PASTE,
    DUPLICATE,
    SELECT_SEGMENT,
    INSERT_SHAPE,
    STRETCH_VALUES,
    STRETCH_TIME,
    SKEW,
    SIMPLIFY,
];

/// Whether `id` is one of the automation editing commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `automation.move_breakpoints`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveBreakpointsArgs {
    /// The track the parameter belongs to.
    pub track: TrackId,
    /// The parameter: a mixer control, a send, or a device parameter.
    pub target: AutomationTarget,
    /// The breakpoints to move, where they are now.
    pub at: Vec<Ticks>,
    /// How far to move them in time, in ticks; negative is earlier.
    pub by: Ticks,
    /// How far to move their values, as a share of the parameter's travel:
    /// 0.1 is a tenth of the way up the control, and −0.1 a tenth down.
    pub share: f64,
}

/// Arguments of the commands that act on the shape of an envelope over a
/// span: `automation.copy`, `automation.cut` and
/// `automation.delete_range`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeSpanArgs {
    /// The lanes the shape is in: each a track and the parameter its
    /// automation lane shows. Left out, the automation lanes the time
    /// selection covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends, after `start`. Left out, the time selection's
    /// end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// Arguments of `automation.insert_shape`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertShapeArgs {
    /// The shape to write.
    pub shape: Shape,
    /// The lanes to write it in: each a track and the parameter its
    /// automation lane shows. Left out, the automation lanes the time
    /// selection covers, or those the selected tracks show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
    /// Where the span starts. Left out, the time selection's start, or the
    /// insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends, after `start`. Left out, the time selection's
    /// end, or one grid step after the insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// Arguments of `automation.stretch_values`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StretchValuesArgs {
    /// The lanes, as for `automation.copy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends. Left out, the time selection's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// Where the box's bottom edge goes, as a share of the parameter's
    /// travel: 0 is the bottom of the control, which is where it starts.
    /// It can't pass `top`.
    #[serde(default)]
    pub bottom: f64,
    /// Where the box's top edge goes: 1 is the top of the control, which is
    /// where it starts. Past 1 or below 0, values are clipped to the travel.
    #[serde(default = "one")]
    pub top: f64,
}

fn one() -> f64 {
    1.0
}

/// Which edge of the box `automation.skew` tilts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkewEdge {
    /// The top edge, which starts at 1.
    Top,
    /// The bottom edge, which starts at 0.
    Bottom,
}

/// Arguments of `automation.skew`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkewArgs {
    /// The edge to tilt.
    pub edge: SkewEdge,
    /// Where the edge goes at the span's start, as a share of the
    /// parameter's travel.
    pub at_start: f64,
    /// Where it goes at the span's end.
    pub at_end: f64,
    /// The lanes, as for `automation.copy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends. Left out, the time selection's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// Arguments of `automation.stretch_time`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StretchTimeArgs {
    /// The lanes, as for `automation.copy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
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

/// Arguments of `automation.paste`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteEnvelopeArgs {
    /// The lanes the shape is pasted into: each a track and the parameter
    /// its automation lane shows. Left out, the automation lanes the time
    /// selection covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
    /// Where the shape starts. Left out, the time selection's start, or the
    /// insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
}

/// Arguments of `automation.select_segment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectSegmentArgs {
    /// The track whose automation lane holds the segment.
    pub track: TrackId,
    /// The parameter the lane shows.
    pub target: AutomationTarget,
    /// The breakpoint the segment starts at, or `null` to select no
    /// segment.
    #[serde(default)]
    pub at: Option<Ticks>,
}

/// What an automation editing command reports: what it changed in the
/// set, and the selection afterwards.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AutomationResult {
    /// What the edit changed in the set.
    pub changes: ChangeSet,
    /// The time selected afterwards, if any.
    pub time_selection: Option<TimeRange>,
    /// Whether the edit was recorded as an undo step: false when it
    /// changed nothing.
    pub undo_step: bool,
}

/// What `automation.copy` put on the clipboard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeClipboardState {
    /// How many lanes' shapes it holds.
    pub lanes: usize,
    /// How long the shape is.
    pub length: Ticks,
}

/// Adds the automation editing commands to a registry.
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

fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(MOVE_BREAKPOINTS, "Move Automation Breakpoints")
            .description(
                "Moves several breakpoints of one track's parameter at once, as dragging a \
             selected breakpoint on an automation lane does: each moves `by` ticks in time and \
             `share` of the parameter's travel in value, staying within the parameter's \
             range, and a breakpoint the moved ones pass over is removed. `at` names the \
             breakpoints to move, where they are now. One undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Envelope])
            .args::<MoveBreakpointsArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(COPY, "Copy Automation")
            .description(
                "Copies the shape of the envelopes the time selection covers to the \
                 Arrangement's clipboard, for `automation.paste`: the shape over the span, \
                 with its positions from the span's start, its values as positions on each \
                 parameter's travel, and its bends. The clipboard isn't part of the set, and \
                 copying changes nothing in it.",
            )
            .permission(Permission::Read)
            .contexts([Arrangement, Envelope])
            .args::<EnvelopeSpanArgs>()
            .result::<EnvelopeClipboardState>(),
        CommandSpec::new(CUT, "Cut Automation")
            .description(
                "Copies the shape of the envelopes the time selection covers, as \
                 `automation.copy` does, then deletes it, as `automation.delete_range` does: \
                 the breakpoints inside the span go, and the envelope's values at the span's \
                 edges are kept, so it runs straight across the span. Because it removes the \
                 person's work, an agent asks first.",
            )
            .permission(Permission::Destructive)
            .contexts([Arrangement, Envelope])
            .args::<EnvelopeSpanArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(DELETE_RANGE, "Delete Automation Range")
            .description(
                "Deletes the shape of the envelopes the time selection covers over the span: \
                 the breakpoints inside it go, and the envelope's values at the span's edges \
                 are kept with a breakpoint at each, so the envelope runs straight across the \
                 span and is unchanged outside it. The clips and the other envelopes stay as \
                 they are. Because it removes the person's work, an agent asks first.",
            )
            .permission(Permission::Destructive)
            .contexts([Arrangement, Envelope])
            .args::<EnvelopeSpanArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(PASTE, "Paste Automation")
            .description(
                "Pastes the clipboard's shape into the envelopes the time selection covers, \
                 starting at `at` (left out, the time selection's start or the insert marker) \
                 and replacing each envelope over the shape's length. Each value lands at the \
                 same position on the target parameter's travel, so a shape pastes onto \
                 another parameter. One undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Envelope])
            .args::<PasteEnvelopeArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(DUPLICATE, "Duplicate Automation")
            .description(
                "Writes a copy of the shape the time selection covers straight after the \
                 span, and makes the copy the time selection, so running it again repeats \
                 the shape. One undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Envelope])
            .args::<EnvelopeSpanArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(SELECT_SEGMENT, "Select Automation Segment")
            .description(
                "Selects a segment in a track's automation lane, as clicking near it does: \
                 its two breakpoints are drawn in the selection colour and move together \
                 when it is dragged. `at` is the breakpoint the segment starts at, or \
                 `null` to select no segment. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Arrangement, Envelope])
            .args::<SelectSegmentArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(INSERT_SHAPE, "Insert Shape")
            .description(
                "Writes a ready-made shape into the envelopes of the lanes the time selection \
                 covers, over the span, replacing each envelope there: `sine`, `triangle`, \
                 `sawtooth`, `inverse_sawtooth` or `square` run one cycle between the bottom \
                 and top of the parameter's range, and `rise`, `fall`, `rise_to`, `fall_to` and \
                 `adsr` join the envelope's value before or after the span. With no time \
                 selection, the span is one grid step from the insert marker. The breakpoints \
                 follow the shape within 1% of the parameter's travel, and the envelope \
                 outside the span stays as it was. One undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Envelope])
            .args::<InsertShapeArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(STRETCH_VALUES, "Stretch Automation Values")
            .description(
                "Stretches the values of the breakpoints in the span between a new bottom and \
                 top edge, as dragging the top or bottom handle of a time selection in an \
                 automation lane does: each moves to the same position between the new edges \
                 as it had between the old, which are the whole travel, and a value past the \
                 parameter's range is clipped to it. `bottom` and `top` are shares of the \
                 travel, and the bottom can't pass the top. One undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Envelope])
            .args::<StretchValuesArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(STRETCH_TIME, "Stretch Automation in Time")
            .description(
                "Stretches the breakpoints in the span into a new span, as dragging the left \
                 or right handle of a time selection in an automation lane does: each moves to \
                 the same place in the new span, and the time selection follows. A breakpoint \
                 outside the span that a moving end passes over is removed; with `carry`, the \
                 breakpoints beyond that end move with it instead. One undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Envelope])
            .args::<StretchTimeArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(SKEW, "Skew Automation")
            .description(
                "Tilts the top or bottom edge of the box round a span's values, as dragging a \
                 corner handle of a time selection in an automation lane does: `at_start` and \
                 `at_end` are where the `edge` runs at the span's start and end, as shares of \
                 the parameter's travel, and each breakpoint in the span moves to the same \
                 position between the bottom and top edges at its own time. The top edge can't \
                 pass the bottom. One undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Envelope])
            .args::<SkewArgs>()
            .result::<AutomationResult>(),
        CommandSpec::new(SIMPLIFY, "Simplify Envelope")
            .description(
                "Replaces the breakpoints of the lanes the time selection covers, inside the \
                 span, with as few as keep each envelope within 0.5% of the parameter's travel \
                 of what it was, using curved segments where one follows the shape. The values \
                 at the span's edges and the envelope outside the span are kept, and no \
                 breakpoint is added. One undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Envelope])
            .args::<EnvelopeSpanArgs>()
            .result::<AutomationResult>(),
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
        // Deleting work is destructive; copying changes nothing.
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(COPY), Some(Permission::Read));
        assert_eq!(class(CUT), Some(Permission::Destructive));
        assert_eq!(class(DELETE_RANGE), Some(Permission::Destructive));
        assert_eq!(class(PASTE), Some(Permission::Edit));
        assert_eq!(class(DUPLICATE), Some(Permission::Edit));
        assert_eq!(class(MOVE_BREAKPOINTS), Some(Permission::Edit));
        for id in [INSERT_SHAPE, STRETCH_VALUES, STRETCH_TIME, SKEW, SIMPLIFY] {
            assert_eq!(class(id), Some(Permission::Edit), "{id}");
        }
        assert!(registry.get("arrangement.delete").is_none());
    }
}
