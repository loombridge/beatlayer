//! The Arrangement's locators in the command registry (WP-6.24): adding,
//! deleting, moving, renaming and describing them, looping to the next one,
//! and the song start time, with the jumps between them. The time-signature
//! lane's commands (WP-6.61) sit here too: selecting, inserting, typing and
//! moving a marker.
//!
//! The locators are part of the set, so the commands that change them are
//! edits, and each is one undo step. The jumps change nothing in the set, so
//! they are in the read class, and they are refused while the audio engine
//! is off. Positions are ticks or bars, beats, and sixteenths
//! ([`Position`]). [`register`] adds their specs, which are pure data, so
//! `cargo xtask` can check them and list them in the command reference;
//! `bl-shell` runs them by ID. The behaviour they follow is in the time
//! editing spec.

use bl_commands::Context::Arrangement;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::LocatorId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::arrangement::ArrangementState;
use crate::editing::{Position, SelectMode};

/// `locator.add`: add a locator, as the Set Locator button does.
pub const ADD: &str = "locator.add";
/// `locator.delete`: delete the selected locators.
pub const DELETE: &str = "locator.delete";
/// `locator.move`: move a locator, and the other selected locators with it.
pub const MOVE: &str = "locator.move";
/// `locator.rename`: rename the selected locators.
pub const RENAME: &str = "locator.rename";
/// `locator.set_info_text`: set a locator's info text.
pub const SET_INFO_TEXT: &str = "locator.set_info_text";
/// `locator.loop_to_next`: loop from a locator to the next one.
pub const LOOP_TO_NEXT: &str = "locator.loop_to_next";
/// `locator.select`: select locators.
pub const SELECT: &str = "locator.select";
/// `locator.jump`: jump to a locator.
pub const JUMP: &str = "locator.jump";
/// `locator.jump_previous`: jump to the locator before this position.
pub const JUMP_PREVIOUS: &str = "locator.jump_previous";
/// `locator.jump_next`: jump to the locator after this position.
pub const JUMP_NEXT: &str = "locator.jump_next";
/// `song.set_start_time`: set where the song starts.
pub const SET_START_TIME: &str = "song.set_start_time";

/// `arrangement.select_time_signature_change`: select a time-signature marker.
pub const SELECT_SIGNATURE: &str = "arrangement.select_time_signature_change";
/// `arrangement.insert_time_signature_change`: add a marker at the insert marker.
pub const INSERT_SIGNATURE: &str = "arrangement.insert_time_signature_change";
/// `arrangement.edit_time_signature`: set the selected marker's signature.
pub const EDIT_SIGNATURE: &str = "arrangement.edit_time_signature";
/// `arrangement.move_time_signature_change`: move a marker, keeping it selected.
pub const MOVE_SIGNATURE: &str = "arrangement.move_time_signature_change";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    ADD,
    DELETE,
    MOVE,
    RENAME,
    SET_INFO_TEXT,
    LOOP_TO_NEXT,
    SELECT,
    JUMP,
    JUMP_PREVIOUS,
    JUMP_NEXT,
    SET_START_TIME,
    SELECT_SIGNATURE,
    INSERT_SIGNATURE,
    EDIT_SIGNATURE,
    MOVE_SIGNATURE,
];

/// Whether `id` is one of the locator commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `locator.add`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddLocatorArgs {
    /// Where the locator goes. Left out, it goes where the Set Locator
    /// button puts one: on the line of the global launch quantisation
    /// nearest the playhead while the song plays, and at the time
    /// selection's start, or the insert marker, while it is stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
}

/// Arguments of `locator.delete`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteLocatorArgs {
    /// The locators to delete. Left out, the selected ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locators: Option<Vec<LocatorId>>,
}

/// Arguments of `locator.move`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveLocatorArgs {
    /// The locator to move. Left out, the selected ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<LocatorId>,
    /// Where it goes: the position the locator moves to. The keys pass the
    /// grid step Left and Right move.
    pub to: Position,
}

/// Arguments of `locator.rename`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameLocatorArgs {
    /// The locators to rename. Left out, the selected ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locators: Option<Vec<LocatorId>>,
    /// The new name. An empty name keeps the old one. Left out, the name
    /// is typed in place, as Cmd-R does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Arguments of `locator.set_info_text`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetInfoTextArgs {
    /// The locator. Left out, the selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<LocatorId>,
    /// The info text, which the Info View shows while the pointer is over
    /// the locator. Left out, the Info View takes the text, as Edit Info
    /// Text does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info_text: Option<String>,
}

/// Arguments of `locator.loop_to_next`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoopToNextArgs {
    /// The locator. Left out, the selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<LocatorId>,
}

/// Arguments of `locator.select`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectLocatorArgs {
    /// The locators.
    pub locators: Vec<LocatorId>,
    /// How to change the selection; `replace` if left out.
    #[serde(default)]
    pub mode: SelectMode,
}

/// Arguments of `locator.jump`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JumpArgs {
    /// The locator. Left out, the selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<LocatorId>,
}

/// Arguments of `song.set_start_time`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetStartTimeArgs {
    /// The locator the song starts at. Left out, the selected one: the
    /// song starts there, or stops starting there if it already does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<LocatorId>,
    /// Whether the song starts at the loop brace's start.
    #[serde(default, skip_serializing_if = "is_false")]
    pub loop_brace: bool,
    /// Whether the song start time is cleared, so the song starts at the
    /// insert marker again.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear: bool,
}

/// Arguments of `arrangement.select_time_signature_change`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectSignatureArgs {
    /// The marker's position. Left out, no marker is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
}

/// Arguments of `arrangement.insert_time_signature_change`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertSignatureArgs {
    /// Where the marker goes. Left out, at the insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
}

/// Arguments of `arrangement.edit_time_signature`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditSignatureArgs {
    /// The marker's position. Left out, the selected marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
    /// The signature as typed, such as `6/8`: a numerator from 1 to 99 and
    /// a denominator of 1, 2, 4, 8 or 16, with a slash, comma, full stop or
    /// spaces between them. Left out, the signature is typed in place, as
    /// Cmd-R does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Arguments of `arrangement.move_time_signature_change`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveSignatureArgs {
    /// The marker's position. Left out, the selected marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
    /// Where it goes. The keys pass the grid step Left and Right move.
    pub to: Position,
}

/// What a time-signature command did: the marker selected afterwards, and
/// whether it was recorded as an undo step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignatureResult {
    /// The position of the selected marker, if one is.
    pub selected: Option<Ticks>,
    /// Whether the edit was recorded as an undo step: false when it
    /// changed nothing.
    pub undo_step: bool,
}

/// Whether a flag is off, for serde's `skip_serializing_if`.
fn is_false(value: &bool) -> bool {
    !*value
}

/// What a locator command did: the locators selected afterwards, and
/// whether it was recorded as an undo step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocatorResult {
    /// The locators selected afterwards.
    pub selected_locators: Vec<LocatorId>,
    /// Whether the edit was recorded as an undo step: false when it
    /// changed nothing.
    pub undo_step: bool,
}

/// Adds the locator commands to a registry.
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
        CommandSpec::new(ADD, "Add Locator")
            .description(
                "Adds a locator at `at`, as the Set Locator button and Add Locator do. Left \
                 out, it goes where the button puts one: on the line of the global launch \
                 quantisation nearest the playhead while the song plays, and at the time \
                 selection's start, or the insert marker, while it is stopped. A locator is \
                 named with the next whole number, one more than the highest any locator is \
                 named with, and becomes the only selected locator. Two locators never share \
                 a position: adding one where another is leaves that one as it is. The \
                 locators are saved with the set.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<AddLocatorArgs>()
            .result::<LocatorResult>(),
        CommandSpec::new(DELETE, "Delete Locator")
            .description(
                "Deletes the locators (`locators`, or the selected ones), as the Delete \
                 Locator button and the Delete key do. A song start time on one of them goes \
                 with it. Because it removes the person's work, an agent asks first.",
            )
            .permission(Permission::Destructive)
            .context(Arrangement)
            .args::<DeleteLocatorArgs>()
            .result::<LocatorResult>(),
        CommandSpec::new(MOVE, "Move Locator")
            .description(
                "Moves a locator (`locator`, or the selected ones) to `to`, the position it \
                 goes to, and moves every other selected locator by the same distance. The \
                 keys pass the grid step Left and Right move, and a drag passes the position \
                 the pointer is at, snapped to the editing grid as a clip's start does. A \
                 locator can't go before 1.1.1, and a move that would put two locators at one \
                 position changes nothing. Dragging a locator runs this, and the whole drag is \
                 one undo step.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<MoveLocatorArgs>()
            .result::<LocatorResult>(),
        CommandSpec::new(RENAME, "Rename Locator")
            .description(
                "Renames the locators (`locators`, or the selected ones), as Cmd-R (Ctrl-R) \
                 and Edit > Rename do. The new name is `name`; an empty name keeps the old \
                 one. Left out, the name is typed in place, as Cmd-R does.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<RenameLocatorArgs>()
            .result::<LocatorResult>(),
        CommandSpec::new(SET_INFO_TEXT, "Set Locator Info Text")
            .description(
                "Sets the info text of a locator (`locator`, or the selected one), which the \
                 Info View shows while the pointer is over it. Without `info_text`, the \
                 Info View becomes a text field for it, as Edit Info Text does.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<SetInfoTextArgs>()
            .result::<LocatorResult>(),
        CommandSpec::new(LOOP_TO_NEXT, "Loop to Next Locator")
            .description(
                "Sets the Arrangement's loop brace from a locator (`locator`, or the selected \
                 one) to the next one and switches the loop on, as the locator's context \
                 menu does. The last locator has none after it, and is refused.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<LoopToNextArgs>()
            .result::<LocatorResult>(),
        CommandSpec::new(SELECT, "Select Locators")
            .description(
                "Selects locators in the Arrangement, as clicking one does: `replace` selects \
                 only these, `toggle` adds or removes each, `add` adds them, and `extend` \
                 adds them and every locator between them and the first selected locator. \
                 Selecting is never an undo step and changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<SelectLocatorArgs>()
            .result::<ArrangementState>(),
        CommandSpec::new(JUMP, "Jump to Locator")
            .description(
                "Jumps to a locator (`locator`, or the selected one), as clicking it does: \
                 while the song plays, playback jumps there on the next line of the global \
                 launch quantisation; while it is stopped, the insert marker moves there. \
                 It changes nothing in the set, and it is refused while the audio engine is \
                 off.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<JumpArgs>()
            .result::<ArrangementState>(),
        CommandSpec::new(JUMP_PREVIOUS, "Jump to Previous Locator")
            .description(
                "Jumps to the locator before the current position: the playhead while the \
                 song plays, the insert marker while it is stopped. With no locator before \
                 it, it jumps to 1.1.1. It selects no locator, changes nothing in the set, \
                 and is refused while the audio engine is off.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<NoArgs>()
            .result::<ArrangementState>(),
        CommandSpec::new(JUMP_NEXT, "Jump to Next Locator")
            .description(
                "Jumps to the locator after the current position: the playhead while the song \
                 plays, the insert marker while it is stopped. With no locator after it, it \
                 jumps to the end of the arrangement. It selects no locator, changes nothing \
                 in the set, and is refused while the audio engine is off.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<NoArgs>()
            .result::<ArrangementState>(),
        CommandSpec::new(SET_START_TIME, "Set Song Start Time")
            .description(
                "Sets where the song starts: at a locator, or at the loop brace's start, as \
                 Set Song Start Time Here does. The song start time follows its locator or \
                 the brace as they move, and deleting the locator clears it. `null` clears it, \
                 so the song starts at the insert marker again. While it is set, Play starts \
                 the song there instead of at the insert marker.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<SetStartTimeArgs>()
            .result::<LocatorResult>(),
        CommandSpec::new(SELECT_SIGNATURE, "Select Time Signature Change")
            .description(
                "Selects the time-signature marker at `at`, as clicking it does, clearing the \
                 locator, clip and time selections; left out, no marker is selected. Selecting \
                 is never an undo step and changes nothing in the set. `arrangement.state` \
                 reports the marker as `selected_time_signature`.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<SelectSignatureArgs>()
            .result::<ArrangementState>(),
        CommandSpec::new(INSERT_SIGNATURE, "Insert Time Signature Change")
            .description(
                "Adds a time-signature marker at `at`, or at the insert marker, with the \
                 signature in force there, as Create > Insert Time Signature Change does, and \
                 selects it. A new bar starts at the marker. Where a marker is already, it \
                 selects that one and changes nothing. Run from a menu, it then opens the \
                 marker's value for typing. Use `arrangement.edit_time_signature` or \
                 `song.set_time_signature_change` to give it another signature.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<InsertSignatureArgs>()
            .result::<SignatureResult>(),
        CommandSpec::new(EDIT_SIGNATURE, "Edit Time Signature")
            .description(
                "Sets the signature of the time-signature marker at `at`, or the selected \
                 one, to `signature` as typed: a numerator from 1 to 99 and a denominator of 1, \
                 2, 4, 8 or 16, with a slash, comma, full stop or spaces between them. A \
                 signature outside those ranges is refused. Left out, the value is typed in \
                 place, as Edit Time Signature and Cmd-R do.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<EditSignatureArgs>()
            .result::<SignatureResult>(),
        CommandSpec::new(MOVE_SIGNATURE, "Move Time Signature Change")
            .description(
                "Moves the time-signature marker at `at`, or the selected one, to `to`, as \
                 dragging it or Left and Right do, and keeps it selected. The marker at 1.1.1 \
                 can't move, and a marker can't land on another. Dragging runs this, and the \
                 whole drag is one undo step. Fragmentary bars are left as they fall.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<MoveSignatureArgs>()
            .result::<SignatureResult>(),
    ]
}
