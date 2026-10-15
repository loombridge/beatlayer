//! The Clip View's note-editing commands in the command registry (WP-2.3):
//! selecting the MIDI Note Editor's notes, drawing, deleting, moving,
//! resizing and quantising them, and the view's Quantize settings.
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
/// `note.quantize`: quantise notes to a grid, over their starts or ends.
pub const QUANTIZE_NOTES: &str = "note.quantize";
/// `clip_view.quantize_settings`: the Clip View's Quantize settings.
pub const QUANTIZE_SETTINGS: &str = "clip_view.quantize_settings";
/// `note.set_value`: set one value of notes (velocity, release velocity,
/// chance, or velocity deviation).
pub const SET_VALUE: &str = "note.set_value";
/// `note.ramp_value`: spread evenly spaced values across notes.
pub const RAMP_VALUE: &str = "note.ramp_value";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SELECT_NOTES,
    ADD_NOTE,
    DELETE_NOTES,
    MOVE_NOTES,
    RESIZE_NOTES,
    QUANTIZE_NOTES,
    QUANTIZE_SETTINGS,
    SET_VALUE,
    RAMP_VALUE,
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

/// Which of a note's editable values the lane shows and the value commands
/// (WP-2.3) set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Which of a note's values the Clip View's lane shows and edits: `velocity` \
                   (1 to 127), `release_velocity` (1 to 127), `chance` (0 to 1), or \
                   `velocity_deviation` (-127 to 127)."
)]
pub enum NoteField {
    /// The velocity the note sounds at, 1 to 127.
    #[default]
    Velocity,
    /// The note-off velocity, 1 to 127.
    ReleaseVelocity,
    /// The chance the note plays, 0 to 1.
    Chance,
    /// How far the velocity may deviate randomly, -127 to 127.
    VelocityDeviation,
}

impl NoteField {
    /// Every field, in the lane's menu's order.
    pub const ALL: [Self; 4] = [
        Self::Velocity,
        Self::ReleaseVelocity,
        Self::Chance,
        Self::VelocityDeviation,
    ];

    /// The field's range: its lowest and highest value.
    #[must_use]
    pub const fn range(self) -> (f64, f64) {
        match self {
            Self::Velocity | Self::ReleaseVelocity => (1.0, 127.0),
            Self::Chance => (0.0, 1.0),
            Self::VelocityDeviation => (-127.0, 127.0),
        }
    }

    /// Whether the field takes whole numbers only.
    #[must_use]
    pub const fn whole(self) -> bool {
        !matches!(self, Self::Chance)
    }

    /// The field's name, as the commands and the lane call it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Velocity => "velocity",
            Self::ReleaseVelocity => "release velocity",
            Self::Chance => "chance",
            Self::VelocityDeviation => "velocity deviation",
        }
    }
}

/// Arguments of `note.set_value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetValueArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Which value to set.
    pub field: NoteField,
    /// The value, within the field's range.
    pub value: f64,
}

/// Arguments of `note.ramp_value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RampValueArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Which value to ramp.
    pub field: NoteField,
    /// The value the first note takes.
    pub from: f64,
    /// The value the last note takes.
    pub to: f64,
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

/// Whether `note.quantize` moves notes' starts or their ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "What `note.quantize` moves toward the grid: `note_start` (the default) \
                   moves each note's start, `note_end` moves its end instead, leaving the \
                   start alone."
)]
pub enum QuantizeTarget {
    /// Move the notes' starts.
    #[default]
    NoteStart,
    /// Move the notes' ends; the starts stay put.
    NoteEnd,
}

/// The Clip View's Quantize settings, as `clip_view.quantize_settings`
/// reports them: what `note.quantize` uses when its arguments leave them
/// out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuantizeSettings {
    /// The spacing between the quantise grid's lines, in ticks. `null`
    /// (nothing) follows the Clip View's editing grid, the step the note
    /// area draws, at the moment the command runs.
    pub grid: Option<Ticks>,
    /// Whether quantise moves note starts or note ends.
    pub target: QuantizeTarget,
    /// How far each note moves toward its grid line, as a percentage from 0
    /// (nothing moves) to 100 (every note lands on the line).
    pub amount: u8,
}

/// What a new Clip View starts with: the editor's grid, note starts, all
/// the way (100 percent).
impl Default for QuantizeSettings {
    fn default() -> Self {
        Self {
            grid: None,
            target: QuantizeTarget::NoteStart,
            amount: 100,
        }
    }
}

/// Arguments of `note.quantize`. Fields left out use the Quantize settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuantizeNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// The grid line spacing in ticks, over the Quantize settings. Lines
    /// are laid out from each bar's start, as the editing grid's are; a
    /// spacing of whole bars counts bars from 1.1.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<Ticks>,
    /// What to move toward the grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<QuantizeTarget>,
    /// How far each note moves toward its grid line, from 0 to 100 percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u8>,
}

/// Arguments of `clip_view.quantize_settings`. Fields left out keep their
/// values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuantizeSettingsArgs {
    /// The quantise grid's line spacing in ticks; must be positive. A new
    /// view follows the Clip View's editing grid until one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<Ticks>,
    /// What quantise moves toward the grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<QuantizeTarget>,
    /// How far each note moves toward its grid line, from 0 to 100 percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u8>,
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
/// the notes selected in it, and the lane's setting.
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
    /// Which value the lane under the note editor shows and edits.
    pub lane: NoteField,
    /// Whether the note editor shows only the rows that hold notes (Fold,
    /// F).
    pub folded: bool,
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
        CommandSpec::new(QUANTIZE_NOTES, "Quantize")
            .description(
                "Quantises notes (`notes`, or the selected notes) to `grid`: each note moves \
                 `amount` percent of the way from its start (or its end, with `target` \
                 `note_end`) to the nearest grid line, so 100 percent lands it on the line \
                 and 50 percent halves its distance to it. The lines are laid out from each \
                 bar's start, as the note editor's grid is, and a spacing of whole bars \
                 counts bars from 1.1.1. `grid`, `target`, and `amount` left out use the \
                 Clip View's Quantize settings (`clip_view.quantize_settings`), which follow \
                 the note editor's editing grid until a grid is set. Quantising is one undo \
                 step, and the quantised notes stay selected.",
            )
            .permission(Permission::Edit)
            .contexts([ClipView, NoteEditor, NoteSelection])
            .binding("S-NOTE-011")
            .args::<QuantizeNotesArgs>()
            .result::<NoteEditResult>(),
        CommandSpec::new(QUANTIZE_SETTINGS, "Quantize Settings")
            .description(
                "Reads or changes the Clip View's Quantize settings, the values `note.quantize \
                 and Cmd-U use when their arguments leave them out: `grid`, the spacing \
                 between the quantise grid's lines in ticks (positive; left unset, it follows \
                 the note editor's editing grid), `target`, whether quantise moves note \
                 starts or note ends, and `amount`, how far each note moves toward its grid \
                 line as a percentage from 0 to 100. Fields left out keep their values, and \
                 the result reports all of them. The settings belong to the view, like Draw \
                 Mode: changing them changes nothing in the set and is no undo step.",
            )
            .permission(Permission::Read)
            .contexts([ClipView, NoteEditor, NoteSelection])
            .args::<QuantizeSettingsArgs>()
            .result::<QuantizeSettings>(),
        edit(
            SET_VALUE,
            "Set Note Value",
            Permission::Edit,
            "Sets one value of notes (`notes`, or the selected notes) to `value`, as dragging \
             a note's stem in the Clip View's lane does: `field` is `velocity` (whole numbers \
             1 to 127), `release_velocity` (whole numbers 1 to 127), `chance` (0 to 1), or \
             `velocity_deviation` (whole numbers -127 to 127). Nothing changes if `value` is \
             outside the field's range. The edited notes stay selected, and the whole drag is \
             one undo step.",
        )
        .args::<SetValueArgs>(),
        edit(
            RAMP_VALUE,
            "Ramp Note Values",
            Permission::Edit,
            "Sets one value of notes (`notes`, or the selected notes) as a ramp, as dragging \
             across several notes in the Clip View's lane does: the first note in the clip's \
             order takes `from`, the last takes `to`, and those between take values evenly \
             spaced between them (whole numbers for the whole-number fields). `field` takes \
             the same fields and ranges as `note.set_value`; nothing changes if `from` or `to` \
             is out of range. \
             One note takes `from`. The edited notes stay selected, and the whole drag is one \
             undo step.",
        )
        .args::<RampValueArgs>(),
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
        assert_eq!(class(QUANTIZE_NOTES), Some(Permission::Edit));
        // The Quantize settings are view state, like Draw Mode.
        assert_eq!(class(QUANTIZE_SETTINGS), Some(Permission::Read));
        // Cmd-U quantises the selected notes (S-NOTE-011).
        let bindings: Vec<&str> = registry
            .get(QUANTIZE_NOTES)
            .unwrap()
            .bindings
            .iter()
            .map(|binding| binding.shortcut.as_ref())
            .collect();
        assert_eq!(bindings, ["S-NOTE-011"]);
    }
}
