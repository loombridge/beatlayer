//! The arranger track in the command registry (WP-6.45): making, renaming,
//! colouring, resizing, moving, copying, duplicating, removing and deleting
//! sections, selecting them, and showing the lane.
//!
//! Sections are part of the set, so the commands that change them are edits,
//! each one undo step; deleting one with its time is destructive. Moving,
//! copying, duplicating and deleting are the …Time commands over the
//! section's span, run inside one transaction ([`crate::editing`], the time
//! editing spec). Selecting and showing the lane change no section, so they
//! are in the read class. Positions are ticks or bars, beats, and sixteenths
//! ([`Position`]). [`register`] adds the specs, which are pure data, so
//! `cargo xtask` can check them and list them in the command reference;
//! `bl-shell` runs them by ID. The behaviour is in the arranger track spec.

use bl_commands::Context::Arrangement;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::SectionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::arrangement::{ArrangementState, SwitchArgs};
use crate::editing::{Position, SelectMode};

/// `arranger.add_section`: make a section over a span.
pub const ADD_SECTION: &str = "arranger.add_section";
/// `arranger.sections_from_locators`: make a section from each locator.
pub const SECTIONS_FROM_LOCATORS: &str = "arranger.sections_from_locators";
/// `arranger.rename_section`: rename the selected section.
pub const RENAME_SECTION: &str = "arranger.rename_section";
/// `arranger.set_section_colour`: colour the selected section.
pub const SET_SECTION_COLOUR: &str = "arranger.set_section_colour";
/// `arranger.resize_section`: move a section's edges.
pub const RESIZE_SECTION: &str = "arranger.resize_section";
/// `arranger.move_section`: move a section and its time.
pub const MOVE_SECTION: &str = "arranger.move_section";
/// `arranger.copy_section`: copy a section and its time.
pub const COPY_SECTION: &str = "arranger.copy_section";
/// `arranger.duplicate_section`: copy a section straight after itself.
pub const DUPLICATE_SECTION: &str = "arranger.duplicate_section";
/// `arranger.remove_section`: remove a section and leave the tracks.
pub const REMOVE_SECTION: &str = "arranger.remove_section";
/// `arranger.delete_section`: delete a section and its time.
pub const DELETE_SECTION: &str = "arranger.delete_section";
/// `arranger.select_section`: select sections.
pub const SELECT_SECTION: &str = "arranger.select_section";
/// `arrangement.arranger_track`: show or hide the arranger track.
pub const ARRANGER_TRACK: &str = "arrangement.arranger_track";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    ADD_SECTION,
    SECTIONS_FROM_LOCATORS,
    RENAME_SECTION,
    SET_SECTION_COLOUR,
    RESIZE_SECTION,
    MOVE_SECTION,
    COPY_SECTION,
    DUPLICATE_SECTION,
    REMOVE_SECTION,
    DELETE_SECTION,
    SELECT_SECTION,
    ARRANGER_TRACK,
];

/// Whether `id` is one of the arranger track's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `arranger.add_section`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddSectionArgs {
    /// Where the section starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where it ends, after `start`. Left out, the time selection's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// Its name. Left out, "Section" and the next whole number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its colour, a swatch from 0 to 69. Left out, the palette's next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<u8>,
}

/// Arguments of the commands that act on one section.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionArgs {
    /// The section. Left out, the first selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SectionId>,
}

/// Arguments of `arranger.rename_section`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameSectionArgs {
    /// The section. Left out, the first selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SectionId>,
    /// The new name. An empty name keeps the old one. Left out, the name
    /// is typed in place, as Cmd-R does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Arguments of `arranger.set_section_colour`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSectionColourArgs {
    /// The section. Left out, the first selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SectionId>,
    /// The colour, a swatch from 0 to 69.
    pub colour: u8,
}

/// Arguments of `arranger.resize_section`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResizeSectionArgs {
    /// The section. Left out, the first selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SectionId>,
    /// Where its left edge goes. Left out, the edge stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where its right edge goes. Left out, the edge stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// The section a Move Earlier or Move Later swaps with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Neighbour {
    /// The section before it.
    Earlier,
    /// The section after it.
    Later,
}

/// Arguments of `arranger.move_section`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveSectionArgs {
    /// The section. Left out, the first selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SectionId>,
    /// Where it goes: the position its start lands at, as the song stands
    /// before the move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Position>,
    /// Instead of `to`, swap it with the section before or after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<Neighbour>,
}

/// Arguments of `arranger.copy_section`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopySectionArgs {
    /// The section. Left out, the first selected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SectionId>,
    /// Where the copy goes: the position its start lands at.
    pub to: Position,
}

/// Arguments of `arranger.select_section`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectSectionArgs {
    /// The sections.
    pub sections: Vec<SectionId>,
    /// How to change the selection; `replace` if left out. `extend` selects
    /// the sections from the first selected one to the last given.
    #[serde(default)]
    pub mode: SelectMode,
}

/// What an arranger command did: the sections selected afterwards, and
/// whether it was recorded as an undo step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArrangerResult {
    /// The sections selected afterwards.
    pub selected_sections: Vec<SectionId>,
    /// Whether the edit was recorded as an undo step: false when it
    /// changed nothing.
    pub undo_step: bool,
}

/// Adds the arranger track's commands to a registry.
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
        CommandSpec::new(ADD_SECTION, "Create Section")
            .description(
                "Makes a section from `start` to `end`, or over the time selection, as \
                 dragging across the arranger track and Create Section do. The span is cut \
                 short where it would reach another section, and one that starts inside a \
                 section is refused. It is named `name`, or \"Section\" and the next whole \
                 number, and takes `colour`, or the palette's next swatch. The new section \
                 becomes the selected one. It moves nothing on the tracks. One undo step, \
                 `Add Section`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<AddSectionArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(SECTIONS_FROM_LOCATORS, "Create Sections From Locators")
            .description(
                "Makes a section from each locator to the next, the last to the \
                 Arrangement's end, wherever no section is, each named after its locator. \
                 A locator inside a section makes none, and a section is cut short where it \
                 would reach another. One undo step, `Add Section`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<NoArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(RENAME_SECTION, "Rename Section")
            .description(
                "Renames a section (`section`, or the selected one) to `name`, as Cmd-R \
                 (Ctrl-R) and a double-click do; an empty name keeps the old one. Left out, \
                 the name is typed in place. One undo step, `Rename Section`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<RenameSectionArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(SET_SECTION_COLOUR, "Set Section Colour")
            .description(
                "Sets the colour of a section (`section`, or the selected one) to `colour`, \
                 a swatch from 0 to 69. One undo step, `Set Section Colour`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<SetSectionColourArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(RESIZE_SECTION, "Resize Section")
            .description(
                "Moves a section's edges (`section`, or the selected one) to `start` and \
                 `end`, as dragging an edge does, changing only its span and nothing on the \
                 tracks. A span that would overlap another section is refused. One undo \
                 step, `Resize Section`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<ResizeSectionArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(MOVE_SECTION, "Move Section")
            .description(
                "Moves a section (`section`, or the selected one) and everything inside its \
                 span on every track to `to`, as Cut Time and Paste Time at the drop would: \
                 the clips (split at the span's edges), every track's automation, the Song \
                 Tempo, the time-signature markers, the locators and the section itself, \
                 closing the gap it leaves. `to` is a position in the song as it stands \
                 before the move; one inside the section's own span changes nothing. With \
                 `with` instead of `to`, it swaps places with the section before or after \
                 it, as Move Earlier and Move Later do. The time clipboard is left as it \
                 was. One undo step, `Move Section`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<MoveSectionArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(COPY_SECTION, "Copy Section")
            .description(
                "Puts a copy of a section (`section`, or the selected one) and everything \
                 inside its span at `to`, as Copy Time and Paste Time would, opening time \
                 there and leaving the original where it is. The time clipboard is left as \
                 it was. One undo step, `Copy Section`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<CopySectionArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(DUPLICATE_SECTION, "Duplicate Section")
            .description(
                "Puts a copy of a section (`section`, or the selected one) and everything \
                 inside its span straight after it, as Duplicate Time does, the copy taking \
                 the same name and colour and becoming the selected section. One undo step, \
                 `Duplicate Section`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<SectionArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(REMOVE_SECTION, "Remove Section")
            .description(
                "Removes a section (`section`, or the selected one) and leaves the tracks as \
                 they are: its clips, automation, tempo, time signatures and locators stay. \
                 One undo step, `Remove Section`.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<SectionArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(DELETE_SECTION, "Delete Section")
            .description(
                "Deletes a section (`section`, or the selected one) and everything inside \
                 its span from every track, closing the song up as Delete Time does, as the \
                 Delete key on a selected section does. Undo restores it, but because it \
                 removes the person's work an agent asks first. One undo step, `Delete \
                 Section`.",
            )
            .permission(Permission::Destructive)
            .context(Arrangement)
            .args::<SectionArgs>()
            .result::<ArrangerResult>(),
        CommandSpec::new(SELECT_SECTION, "Select Section")
            .description(
                "Selects sections in the arranger track, as clicking one does, and makes \
                 their span the time selection on every track, so the …Time commands, Loop \
                 Selection and Zoom to Time Selection act on it: `replace` selects only \
                 these, `toggle` adds or removes each, `add` adds them, and `extend` selects \
                 the sections from the first selected one to the last given. Selecting is \
                 never an undo step.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<SelectSectionArgs>()
            .result::<ArrangementState>(),
        CommandSpec::new(ARRANGER_TRACK, "Arranger Track")
            .description(
                "Shows or hides the arranger track, the lane of sections above the \
                 Arrangement's tracks (`on`; left out, it toggles). It is hidden in a new \
                 set, and whether it shows is saved with the set. It is no undo step.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<SwitchArgs>()
            .result::<ArrangementState>(),
    ]
}
