//! Editing envelopes from the keyboard in the command registry (WP-6.26):
//! the insert marker along an envelope, making, selecting, moving and
//! deleting its breakpoints, and typed values, in the Arrangement's
//! automation lanes and the Clip View's Envelope Editor.
//!
//! The keys act on the envelope that has the keyboard: the automation lane
//! that is selected, or the Envelope Editor while the Clip View's Envelopes
//! tab has the focus. Selecting a breakpoint opens an edit that ends when it
//! is kept, dropped, or the focus leaves; everything in between is one undo
//! step. [`register`] adds their specs, which are pure data, so `cargo xtask`
//! can check them and list them in the command reference; `bl-shell` runs
//! them by ID. The behaviour they follow is in the automation editing spec
//! (R20 to R29).

use bl_commands::Context::{Arrangement, ClipView, Envelope};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, ClipId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `automation.select_breakpoint`: select a breakpoint in an automation lane.
pub const SELECT_BREAKPOINT: &str = "automation.select_breakpoint";
/// `clip.select_envelope_breakpoint`: select a breakpoint in a clip envelope.
pub const SELECT_ENVELOPE_BREAKPOINT: &str = "clip.select_envelope_breakpoint";
/// `envelope.move_marker`: move the insert marker along the envelope.
pub const MOVE_MARKER: &str = "envelope.move_marker";
/// `envelope.enter`: select or make a breakpoint at the insert marker, or
/// keep the edit of the selected one.
pub const ENTER: &str = "envelope.enter";
/// `envelope.cancel`: drop the edit of the selected breakpoint.
pub const CANCEL: &str = "envelope.cancel";
/// `envelope.move_selected`: move the selected breakpoint in time or in
/// value.
pub const MOVE_SELECTED: &str = "envelope.move_selected";
/// `envelope.select_adjacent`: select the next or previous breakpoint.
pub const SELECT_ADJACENT: &str = "envelope.select_adjacent";
/// `envelope.delete_breakpoint`: delete the selected breakpoint, or the one
/// at the insert marker.
pub const DELETE_BREAKPOINT: &str = "envelope.delete_breakpoint";
/// `envelope.delete`: delete the shown envelope.
pub const DELETE: &str = "envelope.delete";
/// `envelope.set_value`: give the selected breakpoint, or a new one, a typed
/// value.
pub const SET_VALUE: &str = "envelope.set_value";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SELECT_BREAKPOINT,
    SELECT_ENVELOPE_BREAKPOINT,
    MOVE_MARKER,
    ENTER,
    CANCEL,
    MOVE_SELECTED,
    SELECT_ADJACENT,
    DELETE_BREAKPOINT,
    DELETE,
    SET_VALUE,
];

/// Whether `id` is one of the envelope keys' commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A direction a key points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrowKey {
    /// Earlier in time.
    Left,
    /// Later in time.
    Right,
    /// A higher value.
    Up,
    /// A lower value.
    Down,
}

/// How far `envelope.move_marker` moves the insert marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarkerStep {
    /// One grid step.
    Grid,
    /// To the previous or next breakpoint.
    Breakpoint,
}

/// Arguments of `automation.select_breakpoint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectBreakpointArgs {
    /// The track whose automation lane holds the breakpoint.
    pub track: TrackId,
    /// The parameter the lane shows.
    pub target: AutomationTarget,
    /// The position of the breakpoint to select, or `null` to keep the edit
    /// and select none.
    #[serde(default)]
    pub at: Option<Ticks>,
}

/// Arguments of `clip.select_envelope_breakpoint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectEnvelopeBreakpointArgs {
    /// The clip whose envelope holds the breakpoint.
    pub clip: ClipId,
    /// The parameter of the clip's track the envelope modulates.
    pub target: AutomationTarget,
    /// The position of the breakpoint to select, in ticks from the beginning
    /// of the clip's contents, or `null` to keep the edit and select none.
    #[serde(default)]
    pub at: Option<Ticks>,
}

/// Arguments of `envelope.move_marker`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveMarkerArgs {
    /// `left` for earlier, `right` for later.
    pub direction: ArrowKey,
    /// `grid` for one grid step, or `breakpoint` for the previous or next
    /// breakpoint.
    pub by: MarkerStep,
}

/// Arguments of `envelope.move_selected`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveSelectedArgs {
    /// `left` and `right` move it one grid step in time, `up` and `down`
    /// change its value by the step its parameter's control takes for an
    /// arrow key.
    pub direction: ArrowKey,
}

/// Arguments of `envelope.select_adjacent`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectAdjacentArgs {
    /// Select the previous breakpoint instead of the next.
    #[serde(default)]
    pub reverse: bool,
}

/// Arguments of `envelope.set_value`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetValueArgs {
    /// The value, as the parameter's control shows it: `-6` or `-6 dB` for a
    /// volume, `25L` for a pan, `50%` for a clip envelope.
    pub text: String,
    /// Where to make a new breakpoint with the value. Left out, the value
    /// goes to the selected breakpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Ticks>,
}

/// Which envelope has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeKeys {
    /// An automation lane in the Arrangement.
    Lane,
    /// The Clip View's Envelope Editor.
    Clip,
}

/// What the envelope keys' commands report: the envelope they acted on, the
/// insert marker, the selected breakpoint, and the envelope's value at the
/// marker as its parameter reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeKeyState {
    /// The envelope that has the keyboard.
    pub envelope: EnvelopeKeys,
    /// The insert marker, in ticks (from the beginning of the clip's
    /// contents in the Envelope Editor).
    pub marker: Ticks,
    /// The selected breakpoint's position, or `null` when none is selected.
    pub selected: Option<Ticks>,
    /// The envelope's value at the insert marker, in the parameter's own
    /// units as its control shows it.
    pub value: String,
}

/// Adds the envelope keys' commands to a registry.
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
        CommandSpec::new(SELECT_BREAKPOINT, "Select Automation Breakpoint")
            .description(
                "Selects a breakpoint in a track's automation lane, as Enter or Option-click \
                 does: the lane gets the keyboard, the insert marker moves to the breakpoint, \
                 and an edit begins that ends when the selection is cleared with `at` `null` \
                 (keeping the changes) or the focus leaves; every change in between is one \
                 undo step. `at` may be a position where no breakpoint is yet, for \
                 `automation.set_breakpoint` to make one there as part of the same edit. \
                 `arrangement.state` reports the selection. Changes nothing in the set itself.",
            )
            .permission(Permission::Read)
            .contexts([Arrangement, Envelope])
            .args::<SelectBreakpointArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(SELECT_ENVELOPE_BREAKPOINT, "Select Envelope Breakpoint")
            .description(
                "Selects a breakpoint of a clip's envelope in the Clip View's Envelope Editor, \
                 as `automation.select_breakpoint` does in an automation lane: the insert \
                 marker moves to it, and an edit begins that ends when the selection is \
                 cleared with `at` `null` or the focus leaves; every change in between is one \
                 undo step. `clip_view.state` reports the selection. Changes nothing in the \
                 set itself.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, Envelope])
            .args::<SelectEnvelopeBreakpointArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(MOVE_MARKER, "Move Insert Marker Along Envelope")
            .description(
                "Moves the insert marker along the envelope that has the keyboard, one grid \
                 step to the `left` or `right` with `by` `grid`, or to the previous or next \
                 breakpoint with `by` `breakpoint`; it stays at the end if there is none \
                 further. The result says the envelope's value at the marker. Does nothing \
                 while a breakpoint is selected: `envelope.move_selected` moves that. Changes \
                 only the view.",
            )
            .permission(Permission::Read)
            .contexts([Arrangement, ClipView, Envelope])
            .args::<MoveMarkerArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(ENTER, "Envelope Enter")
            .description(
                "With no breakpoint selected, selects the breakpoint at the insert marker of \
                 the envelope that has the keyboard, or makes one there at the envelope's \
                 value at that point (the parameter's value when there is no envelope) and \
                 selects it. With one selected, keeps the changes made since it was selected \
                 and deselects it. All of it is one undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Envelope])
            .binding("S-A11Y-019")
            .args::<NoArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(CANCEL, "Envelope Cancel")
            .description(
                "Drops the edit of the selected breakpoint of the envelope that has the \
                 keyboard: it goes back to how it was when it was selected, a breakpoint \
                 `envelope.enter` made is removed, and nothing is selected. No undo step is \
                 recorded.",
            )
            .permission(Permission::Edit)
            .contexts([Envelope])
            .binding("S-A11Y-024")
            .args::<NoArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(MOVE_SELECTED, "Move Selected Breakpoint")
            .description(
                "Moves the selected breakpoint of the envelope that has the keyboard: `left` \
                 and `right` one grid step in time, not past its neighbours, and `up` and \
                 `down` by one step of the parameter's control (1% of its travel, or one step \
                 of a stepped value), within its range. The insert marker goes with it. It is \
                 part of the edit that began when the breakpoint was selected.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, ClipView, Envelope])
            .args::<MoveSelectedArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(SELECT_ADJACENT, "Select Adjacent Breakpoint")
            .description(
                "While a breakpoint of the envelope that has the keyboard is selected, \
                 selects the next one (the previous one with `reverse`) and moves the insert \
                 marker to it; it stays on the last or first. Part of the edit that began \
                 when the first breakpoint was selected. Does nothing with none selected.",
            )
            .permission(Permission::Read)
            .contexts([Envelope])
            .binding("S-A11Y-022")
            .args::<SelectAdjacentArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(DELETE_BREAKPOINT, "Delete Envelope Breakpoint")
            .description(
                "Deletes the selected breakpoint of the envelope that has the keyboard, or \
                 with none selected the one at the insert marker, if there is one. One undo \
                 step. Because it removes the person's work, an agent asks first.",
            )
            .permission(Permission::Destructive)
            .contexts([Envelope])
            .binding("S-A11Y-025")
            .args::<NoArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(DELETE, "Delete Shown Envelope")
            .description(
                "Deletes the automation of the parameter the selected automation lane shows, \
                 as `automation.delete` does, or clears the envelope the Envelope Editor \
                 shows, as `clip.delete_envelope` does. One undo step. Because it removes the \
                 person's work, an agent asks first.",
            )
            .permission(Permission::Destructive)
            .contexts([Envelope])
            .binding("S-ENV-005")
            .args::<NoArgs>()
            .result::<EnvelopeKeyState>(),
        CommandSpec::new(SET_VALUE, "Set Breakpoint Value")
            .description(
                "Types a value for the selected breakpoint of the envelope that has the \
                 keyboard, as its parameter's control reads it (`-6 dB`, `25L`, `50%`); the \
                 other selected breakpoints of a lane move by the same share of the travel. \
                 With `at`, makes a new breakpoint there with the value instead. A value the \
                 parameter doesn't read is refused. Part of the edit that began when the \
                 breakpoint was selected, or one undo step on its own.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, ClipView, Envelope])
            .args::<SetValueArgs>()
            .result::<EnvelopeKeyState>(),
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
        // Choosing and moving what is shown changes nothing in the set;
        // deleting the person's work is destructive.
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(SELECT_BREAKPOINT), Some(Permission::Read));
        assert_eq!(class(MOVE_MARKER), Some(Permission::Read));
        assert_eq!(class(ENTER), Some(Permission::Edit));
        assert_eq!(class(DELETE_BREAKPOINT), Some(Permission::Destructive));
        assert_eq!(class(DELETE), Some(Permission::Destructive));
    }
}
