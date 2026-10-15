//! The Clip View's note-editing commands in the command registry (WP-2.3):
//! selecting the MIDI Note Editor's notes, drawing, deleting, moving and
//! resizing them.
//!
//! A note is named by its place in the clip's notes in order (by start,
//! then pitch): place 0 is the earliest note, 0 again the lowest of those
//! that start together. The commands act on the notes named in their
//! arguments, or, when they're left out, on the notes selected in the Clip
//! View, as a person's click or drag does. Every edit is one undo step, and
//! a drag that edits is one step however long it lasts: the mouse runs
//! these same commands inside one transaction.
//!
//! Notes' positions and lengths are ticks in the clip's contents, counted
//! from the contents' beginning; the clip plays its contents from its Start
//! marker, which falls at the clip's start (Spec: midi-clips R3, R4). The
//! note editor reads `MidiData::start`, as playback does.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID. The behaviour they follow is in the piano roll spec.

use bl_commands::Context::{ClipView, NoteEditor, NoteSelection};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::ClipId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `clip_view.select_notes`: select notes in the MIDI Note Editor.
pub const SELECT_NOTES: &str = "clip_view.select_notes";
/// `note.add`: add a note to a MIDI clip.
pub const ADD_NOTE: &str = "note.add";
/// `note.delete`: delete notes from a MIDI clip.
pub const DELETE_NOTES: &str = "note.delete";
/// `note.move`: move notes in time and in pitch.
pub const MOVE_NOTES: &str = "note.move";
/// `note.resize`: move notes' ends.
pub const RESIZE_NOTES: &str = "note.resize";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SELECT_NOTES,
    ADD_NOTE,
    DELETE_NOTES,
    MOVE_NOTES,
    RESIZE_NOTES,
];

/// Whether `id` is one of the Clip View's note-editing commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// How `clip_view.select_notes` changes the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "How to change the note selection: `replace` it (a click or a marquee), or \
                   `toggle` each note in or out (Cmd-click, Ctrl-click on Windows)."
)]
pub enum NoteSelectMode {
    /// Select only these notes.
    #[default]
    Replace,
    /// Select each note that isn't selected and deselect each that is.
    Toggle,
}

/// Arguments of `clip_view.select_notes`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Empty with `replace` clears the selection.
    pub notes: Vec<u32>,
    /// How to change the selection; `replace` if left out.
    #[serde(default)]
    pub mode: NoteSelectMode,
}

/// Arguments of `note.add`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddNoteArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The pitch, a MIDI key from 0 to 127; 60 is middle C.
    pub key: u8,
    /// Where the note starts, in ticks from the beginning of the clip's
    /// contents. Never negative.
    pub start: Ticks,
    /// How long the note lasts, in ticks. Always positive.
    pub length: Ticks,
    /// The velocity, from 1 to 127; 100 unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u8>,
}

/// Arguments of `note.delete`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
}

/// Arguments of `note.move`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// How far to move them in time, in ticks; negative is earlier. Give
    /// `by` or `semitones`, or both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Ticks>,
    /// How far to move them in pitch, in semitones; negative is lower.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semitones: Option<i32>,
}

/// Arguments of `note.resize`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResizeNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// How far each note's end moves, in ticks; negative is shorter. A note
    /// keeps at least one tick of length.
    pub by: Ticks,
}

/// What a note edit did, and what's selected afterwards.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoteEditResult {
    /// The notes the edit created or changed, by their place in the clip's
    /// notes in order afterwards.
    pub notes: Vec<u32>,
    /// The notes selected afterwards.
    pub selected: Vec<u32>,
    /// Whether the edit was recorded as an undo step: false when it changed
    /// nothing.
    pub undo_step: bool,
}

/// What the Clip View shows: which clip its MIDI Note Editor is editing,
/// and the notes selected in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipViewState {
    /// Whether the Clip View shows in the detail area.
    pub visible: bool,
    /// Whether Draw Mode is on (B): clicking the note area adds a note, and
    /// clicking a note removes it.
    pub draw_mode: bool,
    /// The clip the note editor shows; `null` when none does.
    pub clip: Option<ClipId>,
    /// The selected notes, by their place in the clip's notes in order (by
    /// start, then pitch).
    pub selected: Vec<u32>,
}

/// Adds the Clip View's note-editing commands to a registry.
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

fn edit(
    id: &'static str,
    label: &'static str,
    permission: Permission,
    description: &'static str,
) -> CommandSpec {
    CommandSpec::new(id, label)
        .description(description)
        .permission(permission)
        .contexts([ClipView, NoteEditor, NoteSelection])
        .result::<NoteEditResult>()
}

fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(SELECT_NOTES, "Select Notes")
            .description(
                "Selects notes in the Clip View's MIDI Note Editor, as clicking a note or \
                 dragging a marquee over them does: `replace` selects only these, `toggle` \
                 adds or removes each (Cmd-click, Ctrl-click on Windows). Notes are named by \
                 their place in the clip's notes in order, by start and then pitch, so place \
                 0 has the earliest start, and of notes that start together the lowest pitch. \
                 Selecting is never an undo step and changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor, NoteSelection])
            .args::<SelectNotesArgs>()
            .result::<ClipViewState>(),
        edit(
            ADD_NOTE,
            "Add Note",
            Permission::Edit,
            "Adds a note to a MIDI clip: pitch `key` (0 to 127; 60 is middle C), starting \
             `start` ticks from the beginning of the clip's contents, lasting `length` ticks, \
             with `velocity` (1 to 127; 100 unless given). With Draw Mode on, clicking the \
             note area runs this for a note one grid step long. The new note is selected \
             afterwards, and the edit is one undo step.",
        )
        .args::<AddNoteArgs>(),
        edit(
            DELETE_NOTES,
            "Delete Notes",
            Permission::Destructive,
            "Deletes notes (`notes`, or the selected notes) from a MIDI clip. With Draw Mode \
             on, clicking a note runs this. Because it removes the person's work, an agent \
             asks first. The edit is one undo step.",
        )
        .args::<DeleteNotesArgs>(),
        edit(
            MOVE_NOTES,
            "Move Notes",
            Permission::Edit,
            "Moves notes (`notes`, or the selected notes) in time by `by` ticks and in pitch \
             by `semitones`, as dragging them in the note editor does; dragging right and left \
             runs it with `semitones` 0, and up and down snap the pitch. Nothing moves if any \
             note would start before the clip's contents or leave the range of MIDI keys 0 to \
             127. The moved notes stay selected, and the whole drag is one undo step.",
        )
        .args::<MoveNotesArgs>(),
        edit(
            RESIZE_NOTES,
            "Resize Notes",
            Permission::Edit,
            "Moves notes' ends (`notes`, or the selected notes) by `by` ticks, as dragging a \
             note's end in the note editor does: a positive `by` makes them longer, a negative \
             one shorter, and every note keeps at least one tick of length. A note's start \
             doesn't move. The resized notes stay selected, and the whole drag is one undo \
             step.",
        )
        .args::<ResizeNotesArgs>(),
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
        // Deleting notes removes work; selecting changes nothing.
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(DELETE_NOTES), Some(Permission::Destructive));
        assert_eq!(class(SELECT_NOTES), Some(Permission::Read));
        assert_eq!(class(ADD_NOTE), Some(Permission::Edit));
        assert_eq!(class(MOVE_NOTES), Some(Permission::Edit));
        assert_eq!(class(RESIZE_NOTES), Some(Permission::Edit));
    }
}
