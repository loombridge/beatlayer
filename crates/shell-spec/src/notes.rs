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

use crate::editing::EditResult;
use bl_commands::Context::{NoteEditor, NoteSelection as NoteSelectionContext, SampleEditor};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, ClipId};
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
/// `note.adjust_value`: change one value of notes by an amount.
pub const ADJUST_VALUE: &str = "note.adjust_value";
/// `note.randomize`: move one value of notes at random, up to an amount.
pub const RANDOMIZE: &str = "note.randomize";
/// `note.deactivate`: deactivate notes, or activate them again.
pub const DEACTIVATE: &str = "note.deactivate";
/// `note.copy`: copy notes to the note clipboard.
pub const COPY: &str = "note.copy";
/// `note.cut`: copy notes to the note clipboard and delete them.
pub const CUT: &str = "note.cut";
/// `note.paste`: paste the note clipboard's notes into a clip.
pub const PASTE: &str = "note.paste";
/// `note.duplicate`: put a copy of the notes straight after them.
pub const DUPLICATE: &str = "note.duplicate";

/// `note.split`: split notes at ticks.
pub const SPLIT: &str = "note.split";
/// `note.chop`: chop notes on the grid or into equal parts.
pub const CHOP: &str = "note.chop";
/// `note.join`: join the notes of each pitch into one.
pub const JOIN: &str = "note.join";
/// `note.fit_to_time`: give notes a time range's start and end.
pub const FIT_TO_TIME: &str = "note.fit_to_time";
/// `note.stretch`: scale notes from one range of time to another.
pub const STRETCH: &str = "note.stretch";
/// `note.split_clip`: split the Arrangement clip the editor shows at the
/// time selection.
pub const SPLIT_CLIP: &str = "note.split_clip";

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
    ADJUST_VALUE,
    RANDOMIZE,
    DEACTIVATE,
    COPY,
    CUT,
    PASTE,
    DUPLICATE,
    SPLIT,
    CHOP,
    JOIN,
    FIT_TO_TIME,
    STRETCH,
    SPLIT_CLIP,
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

/// What `clip_view.select_notes` selects when its arguments name no notes:
/// the notes the keyboard reaches, and the ones a key in the piano ruler
/// reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "What to select in the note editor: `nearest` (the note whose start is nearest \
                   the insert marker), `previous` and `next` (the previous or next note in the \
                   clip's order), `same_pitch_previous` and `same_pitch_next` (the previous or \
                   next note at the selected note's pitch), `all` (every note in the clip's \
                   span), or `key` (every note at a pitch, as clicking a key in the piano ruler \
                   does)."
)]
pub enum NoteSelection {
    /// The note whose start is nearest the insert marker.
    Nearest,
    /// The previous note in the clip's order.
    Previous,
    /// The next note in the clip's order.
    Next,
    /// The previous note at the selected note's pitch.
    SamePitchPrevious,
    /// The next note at the selected note's pitch.
    SamePitchNext,
    /// Every note in the clip's span.
    All,
    /// Every note at a pitch, as clicking a key in the piano ruler does.
    Key(u8),
}

/// The Clip View's tabs (layout.md, L-CLIP-010): which editor it shows for
/// the clip. The MPE tab comes with its own package.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A tab of the Clip View: `notes` (the MIDI Note Editor) or `envelopes` (the \
                   clip's envelopes)."
)]
pub enum ClipTab {
    /// The MIDI Note Editor.
    #[default]
    Notes,
    /// The clip's envelopes (WP-3.16).
    Envelopes,
}

impl ClipTab {
    /// The tabs `view.clip_tab` cycles through, in the order the Clip View
    /// lists them.
    pub const ALL: [Self; 2] = [Self::Notes, Self::Envelopes];

    /// The tab after this one, going back to the first after the last.
    #[must_use]
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
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

    /// The three fields that have a lane under the note editor, in the
    /// order the lanes sit from the top (Spec: note-editing R1).
    pub const LANES: [Self; 3] = [Self::Velocity, Self::Chance, Self::ReleaseVelocity];

    /// The lane that edits this field: the field's own, or for velocity
    /// deviation the Velocity lane, which shows it (Spec: note-editing R3).
    #[must_use]
    pub const fn lane(self) -> Self {
        match self {
            Self::VelocityDeviation => Self::Velocity,
            other => other,
        }
    }

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
    /// The value, within the field's range, for every one of the notes.
    /// Give this or `values`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// One value for each of the notes, in the order they're named, each
    /// within the field's range: what drawing across a lane gives. Give this
    /// or `value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<f64>>,
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

/// Arguments of `note.adjust_value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdjustValueArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Which value to change.
    pub field: NoteField,
    /// How much to add to each note's value; negative lowers it. Whole
    /// numbers for the whole-number fields; for `chance`, a fraction (0.1 is
    /// 10 %).
    pub by: f64,
}

/// Arguments of `note.randomize`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RandomizeArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes, or with none selected
    /// every note in the clip's span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Which value to randomise: `velocity`, `release_velocity` or `chance`.
    pub field: NoteField,
    /// How far from its value each note may move, in the field's own units:
    /// 0 to 127 for the velocities, 0 to 1 for `chance` (0.25 is 25 %).
    pub amount: f64,
    /// Seeds the draw: the same seed with the same notes gives the same
    /// values. Left out, each call draws afresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// Arguments of `note.deactivate`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeactivateArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Whether the notes end up active (`true`) or deactivated (`false`).
    /// Left out, they are deactivated, or activated when every one of them
    /// is deactivated already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}

/// Arguments of `clip_view.select_notes`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Empty with `replace` clears the selection, and empty
    /// with `select` chooses the notes `select` names.
    #[serde(default)]
    pub notes: Vec<u32>,
    /// How to change the selection; `replace` if left out.
    #[serde(default)]
    pub mode: NoteSelectMode,
    /// What to select when `notes` is empty: the notes the keyboard reaches
    /// (Option with the arrows), every note in the clip's span (Cmd-A), or
    /// the notes at a pitch (a key in the piano ruler).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<NoteSelection>,
    /// Add to the selection instead of replacing it, as Shift with the
    /// note-jump keys does.
    #[serde(default)]
    pub add: bool,
}

/// Arguments of `note.copy` and `note.cut`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes, or with none selected the
    /// notes that start inside the time selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
}

/// Arguments of `note.paste`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Where to paste, in ticks from the beginning of the clip's contents.
    /// Left out, the insert marker, or the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Ticks>,
}

/// Arguments of `note.duplicate`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes, or with none selected the
    /// notes that start inside the time selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
}

/// One cut of `note.split`: a note and the tick it is split at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteCut {
    /// The note, by its place in the clip's notes in order.
    pub note: u32,
    /// The tick to split it at, from the beginning of the clip's contents.
    pub at: Ticks,
}

/// Arguments of `note.split`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes to split, by their place in the clip's notes in order.
    /// Left out, the selected notes, or with none selected every note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// The ticks to split them at, from the beginning of the clip's
    /// contents. Left out, the time selection's start and end, or the
    /// insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Vec<Ticks>>,
    /// Instead of `notes` and `at`: each note with its own tick, as
    /// dragging E across several notes splits each where the drag crosses
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuts: Option<Vec<NoteCut>>,
}

/// How `note.chop` changes the number of parts it has made, one press of
/// Up or Down with Cmd held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "One change of a chop's parts, counted from each note's own parts: `more`                    and `fewer` add or take away one, `next_power` and `previous_power` go to                    the next or previous power of two (1, 2, 4 and up to 64)."
)]
pub enum ChopStep {
    /// One part more.
    More,
    /// One part fewer.
    Fewer,
    /// The next power of two.
    NextPower,
    /// The previous power of two.
    PreviousPower,
}

/// Arguments of `note.chop`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChopNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order. Left out,
    /// the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Cut at every line of a grid this many ticks apart. Left out (and
    /// without `parts`), the editing grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<Ticks>,
    /// Instead of the grid: cut every note into this many equal parts,
    /// from 1 to 64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<u32>,
    /// Changes to make to each note's parts after the first cut, in order,
    /// as Cmd with Up and Down does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ChopStep>,
}

/// Arguments of `note.join`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JoinNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order. Left out,
    /// the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
}

/// A range of time in a MIDI clip's contents, in ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TickRange {
    /// Where it starts.
    pub start: Ticks,
    /// Where it ends. In a range a stretch ends at, this may be before
    /// `start`, which mirrors the notes' order in time.
    pub end: Ticks,
}

/// Arguments of `note.fit_to_time`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FitToTimeArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order. Left out,
    /// the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// The range every note takes. Left out, the time selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TickRange>,
}

/// A point inside a stretch's range that moves while both ends stay, as
/// dragging the pseudo marker does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StretchPoint {
    /// Where the point is in the range being stretched.
    pub from: Ticks,
    /// Where it goes.
    pub to: Ticks,
}

/// Arguments of `note.stretch`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StretchNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order. Left out,
    /// the selected notes, or with none selected the notes that start
    /// inside `from`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// The range being stretched. Left out, the time selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<TickRange>,
    /// The range it becomes: its start is where `from`'s start goes, and
    /// its end where `from`'s end goes.
    pub to: TickRange,
    /// A point inside `from` that goes somewhere else: what lies on either
    /// side of it is scaled to meet it there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<StretchPoint>,
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
    /// Copy them instead of moving them, as Option-dragging a note does:
    /// the copies land where the notes would have gone and the originals
    /// stay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<bool>,
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

/// What the note clipboard holds, as `note.copy` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoteClipboardState {
    /// How many notes it holds.
    pub notes: usize,
    /// How long they last, from the earliest of them.
    pub length: Ticks,
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

/// The Randomize amount of each lane, which the footer's control holds
/// (Spec: note-editing R13). A new editor has 0 for each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RandomizeAmounts {
    /// The Velocity lane's, from 0 to 127.
    pub velocity: u8,
    /// The Chance lane's, a percentage from 0 to 100.
    pub chance: u8,
    /// The Release Velocity lane's, from 0 to 127.
    pub release_velocity: u8,
}

impl RandomizeAmounts {
    /// The amount of the lane that edits `field`, in the footer's units.
    #[must_use]
    pub const fn of(self, field: NoteField) -> u8 {
        match field.lane() {
            NoteField::Chance => self.chance,
            NoteField::ReleaseVelocity => self.release_velocity,
            _ => self.velocity,
        }
    }

    /// The largest amount the lane that edits `field` takes.
    #[must_use]
    pub const fn limit(field: NoteField) -> u8 {
        match field.lane() {
            NoteField::Chance => 100,
            _ => 127,
        }
    }
}

/// What the Clip View shows: which clip it is editing, the notes selected in
/// it, its insert marker and time selection, its expression lanes, and the
/// tab with the envelope it shows.
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
    /// The insert marker, in ticks from the beginning of the clip's
    /// contents; `null` when there is none.
    pub insert_marker: Option<Ticks>,
    /// The time selection's start and end, in ticks from the beginning of
    /// the clip's contents; `null` when there is none.
    pub time_selection: Option<(Ticks, Ticks)>,
    /// The active lane: `velocity`, `chance` or `release_velocity`, whose
    /// controls the footer shows.
    pub lane: NoteField,
    /// The lanes shown under the note editor now, from the top: none while
    /// they are all hidden.
    pub lanes: Vec<NoteField>,
    /// Whether the Show/Hide All Expression Editors switch has hidden every
    /// lane.
    pub lanes_hidden: bool,
    /// The Randomize amounts of the three lanes.
    pub randomize: RandomizeAmounts,
    /// Whether the note editor shows only the rows that hold notes (Fold,
    /// F).
    pub folded: bool,
    /// The tab the Clip View shows.
    pub tab: ClipTab,
    /// The parameter whose envelope the Envelopes tab shows; `null` when it
    /// shows none.
    pub envelope: Option<AutomationTarget>,
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
        .contexts([NoteEditor, NoteSelectionContext])
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
                 With no notes named, `select` chooses them instead: `nearest` (the note whose \
                 start is nearest the insert marker), `previous` and `next` (the previous or \
                 next note in the clip's order), `same_pitch_previous` and `same_pitch_next` \
                 (the previous or next note at the selected note's pitch), `all` (every note in \
                 the clip's span, as Cmd-A does), or `key` (every note at a pitch, as clicking a \
                 key in the piano ruler does). `add` adds to the selection instead of replacing \
                 it, as Shift with the note-jump keys does. Selecting is never an undo step and \
                 changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, NoteSelectionContext])
            .binding(Binding::new("S-NOTE-001").arg("select", "all"))
            .binding(Binding::new("S-EDIT-009").arg("select", "all"))
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
             on, clicking a note runs this. Delete (S-EDIT-005) runs it on the selected notes, \
             or with none selected the notes in the time selection. Because it removes the \
             person's work, an agent asks first. The edit is one undo step.",
        )
        .binding("S-EDIT-005")
        .args::<DeleteNotesArgs>(),
        edit(
            MOVE_NOTES,
            "Move Notes",
            Permission::Edit,
            "Moves notes (`notes`, or the selected notes) in time by `by` ticks and in pitch \
             by `semitones`, as dragging them in the note editor does; dragging right and left \
             runs it with `semitones` 0, and up and down snap the pitch. Nothing moves if any \
             note would start before the clip's contents or leave the range of MIDI keys 0 to \
             127. With `copy` the notes are copied where they would have gone and the originals \
             stay, as Option-dragging a note does. The moved notes stay selected, and the whole \
             drag is one undo step.",
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
            .contexts([NoteEditor, NoteSelectionContext])
            .binding("S-NOTE-011")
            .args::<QuantizeNotesArgs>()
            .result::<NoteEditResult>(),
        CommandSpec::new(QUANTIZE_SETTINGS, "Quantize Settings")
            .description(
                "Reads or changes the Clip View's Quantize settings, the values `note.quantize`, \
                 `clip.quantize_audio` and Cmd-U use when their arguments leave them out: `grid`, \
                 the spacing between the quantise grid's lines in ticks (positive; left unset, \
                 it follows the note or sample editor's editing grid), `target`, whether quantise \
                 moves note starts or note ends, and `amount`, how far each note moves toward its \
                 grid line as a percentage from 0 to 100. Fields left out keep their values, and \
                 the result reports all of them. The settings belong to the view, like Draw \
                 Mode: changing them changes nothing in the set and is no undo step.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, NoteSelectionContext, SampleEditor])
            .binding("S-SAMPLE-002")
            .args::<QuantizeSettingsArgs>()
            .result::<QuantizeSettings>(),
        edit(
            SET_VALUE,
            "Set Note Value",
            Permission::Edit,
            "Sets one value of notes (`notes`, or the selected notes) to `value`, or to each \
             note's own entry of `values` (one for each note, in the order named), as typing \
             a value or drawing across an expression lane does: `field` is `velocity` (whole numbers \
             1 to 127), `release_velocity` (whole numbers 1 to 127), `chance` (0 to 1), or \
             `velocity_deviation` (whole numbers -127 to 127). Nothing changes if `value` is \
             outside the field's range. A velocity deviation is held within the room each \
             note's velocity leaves: at most 127 less its velocity and at least 1 less. The \
             edited notes stay selected, and the whole drag is one undo step.",
        )
        .args::<SetValueArgs>(),
        edit(
            RAMP_VALUE,
            "Ramp Note Values",
            Permission::Edit,
            "Sets one value of notes (`notes`, or the selected notes) as a ramp, as the \
             footer's Ramp Start and Ramp End do: the first note by start takes `from`, the \
             last takes `to`, and those between take values on a straight line between them \
             by their start times, notes that start together taking the same value (whole \
             numbers for the whole-number fields). `field` takes the same fields and \
             ranges as `note.set_value`; nothing changes if `from` or `to` is out of range. \
             One note takes `from`, and so do notes that all start together. The edited notes \
             stay selected, and the whole edit is one undo step.",
        )
        .args::<RampValueArgs>(),
        edit(
            ADJUST_VALUE,
            "Adjust Note Values",
            Permission::Edit,
            "Changes one value of notes (`notes`, or the selected notes) by `by`, as the \
             value keys and dragging a marker in an expression lane do: `field` is \
             `velocity` or `release_velocity` (whole numbers, 1 to 127), `chance` (0 to 1; \
             0.1 is 10 %), or `velocity_deviation` (whole numbers, held to the room each \
             note's velocity leaves: at most 127 less its velocity and at least 1 less). \
             Each note is held within its range, so a change past an end brings it to that \
             end, and a call that changes no note records no undo step. The edited notes \
             stay selected, and the whole drag is one undo step.",
        )
        .args::<AdjustValueArgs>(),
        edit(
            RANDOMIZE,
            "Randomize Note Values",
            Permission::Edit,
            "Moves one value of notes (`notes`, or the selected notes, or with none \
             selected every note in the clip's span) at random, as the footer's Randomize \
             does: each note takes a value drawn evenly from its value less `amount` to \
             its value plus `amount`, held within the field's range and whole for the \
             velocities. `field` is `velocity`, `release_velocity` or `chance`; `amount` \
             is in the field's own units, 0 to 127 or 0 to 1 for `chance`. The same `seed` \
             with the same notes gives the same values, each note keeping its own draw so \
             a larger `amount` moves it further the same way; left out, each call draws \
             afresh. An `amount` of 0 changes nothing. The edit is one undo step, and the \
             notes stay selected.",
        )
        .args::<RandomizeArgs>(),
        edit(
            DEACTIVATE,
            "Deactivate Note(s)",
            Permission::Edit,
            "Deactivates notes (`notes`, or the selected notes), as 0 does: a deactivated \
             note keeps its place and values, is drawn grey and never plays. With `active` \
             left out the notes are deactivated, or activated again when every one of them \
             is deactivated already (the menu item then reads Activate Note(s)); `active` \
             `true` or `false` sets them. The edit is one undo step, and the notes stay \
             selected.",
        )
        .binding("S-NOTE-041")
        .args::<DeactivateArgs>(),
        CommandSpec::new(COPY, "Copy Notes")
            .description(
                "Copies notes (`notes`, or the selected notes, or with none selected the notes \
                 that start inside the time selection) to the note clipboard, keeping their \
                 pitches, values and their places in time from the earliest of them. The note \
                 clipboard is apart from the Arrangement's clip clipboard. Copy changes nothing \
                 in the set.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, NoteSelectionContext])
            .binding("S-EDIT-002")
            .args::<CopyNotesArgs>()
            .result::<NoteClipboardState>(),
        edit(
            CUT,
            "Cut Notes",
            Permission::Destructive,
            "Copies notes to the note clipboard, as `note.copy` does, then deletes them, as \
             `note.delete` does. Cmd-X (Ctrl-X) runs this on the selected notes, or with none \
             selected the notes in the time selection. Because it removes the person's work, an \
             agent asks first.",
        )
        .binding("S-EDIT-001")
        .args::<CopyNotesArgs>(),
        edit(
            PASTE,
            "Paste Notes",
            Permission::Edit,
            "Puts the note clipboard's notes into the clip at `at`, in ticks from the beginning \
             of the clip's contents, or at the insert marker, or at the time selection's start, \
             with their pitches, values and spacing, and selects them. It fails, changing \
             nothing, if there is nothing on the clipboard. Cmd-V (Ctrl-V) runs this.",
        )
        .binding("S-EDIT-003")
        .args::<PasteNotesArgs>(),
        edit(
            DUPLICATE,
            "Duplicate Notes",
            Permission::Edit,
            "Puts a copy of notes (`notes`, or the selected notes, or with none selected the \
             notes that start inside the time selection) straight after the time selection, \
             selects the copy and moves the time selection onto it, so running it again lays \
             down another. Cmd-D (Ctrl-D) runs this.",
        )
        .binding("S-EDIT-004")
        .args::<DuplicateNotesArgs>(),
        CommandSpec::new(SPLIT, "Split Note(s)")
            .description(
                "Splits notes at ticks, as holding E and clicking a note, or Cmd-E (Ctrl-E) \
                 with no notes selected, does. `notes` (or the selected notes, or with none \
                 selected every note) are each split at every tick of `at` (or of the time \
                 selection's start and end, or the insert marker) that falls strictly inside \
                 them; `cuts` instead names a tick for each note. A note split becomes parts \
                 that together cover it: the first keeps its start, the last its end, each \
                 keeps the note's pitch, velocity, release velocity, chance, deviation and \
                 deactivation, and its pressure points go with the part they fall in. A tick \
                 at or outside a note's start or end splits nothing, and when no note splits \
                 nothing changes and no undo step is recorded. The parts of the notes that \
                 were selected stay selected. The edit is one undo step, `Split Notes`.",
            )
            .permission(Permission::Edit)
            .contexts([NoteEditor])
            .args::<SplitNotesArgs>()
            .result::<NoteEditResult>(),
        CommandSpec::new(CHOP, "Chop Note(s) on Grid")
            .description(
                "Chops notes (`notes`, or the selected notes), as Cmd-E (Ctrl-E) with notes \
                 selected does: each is cut at every line of `grid` (ticks apart; the editing \
                 grid if left out) that falls strictly inside it, or with `parts`, into that \
                 many equal parts, from 1 to 64. `steps` then change each note's number of \
                 parts, one part more or fewer or the next or previous power of two, as Cmd \
                 with Up and Down does while Cmd is held; the number stays from 1 to 64, \
                 and a note keeps parts of at least a tick. The parts keep what Split keeps, \
                 and stay selected. The edit is one undo step, `Chop Notes`.",
            )
            .permission(Permission::Edit)
            .contexts([NoteSelectionContext])
            .args::<ChopNotesArgs>()
            .result::<NoteEditResult>(),
        CommandSpec::new(JOIN, "Join Notes")
            .description(
                "Joins notes (`notes`, or the selected notes), as Cmd-J (Ctrl-J) does: the \
                 notes of each pitch that has two or more become one note from the earliest \
                 start among them to the latest end, with the values of the earliest of them. \
                 Every joined note's pressure points stay at their places in time, and where \
                 two overlap the later note's win. A pitch with one note is left alone, and \
                 the command is refused when there is nothing to join. The joined notes are \
                 selected afterwards. The edit is one undo step, `Join Notes`.",
            )
            .permission(Permission::Edit)
            .contexts([NoteEditor, NoteSelectionContext])
            .binding("S-NOTE-007")
            .args::<JoinNotesArgs>()
            .result::<NoteEditResult>(),
        CommandSpec::new(FIT_TO_TIME, "Fit to Time Range")
            .description(
                "Gives every note (`notes`, or the selected notes) the start and end of \
                 `range`, or of the time selection, as Cmd-Option-J (Ctrl-Alt-J) does. Each \
                 note's pressure points scale with it. Refused with no notes or no range. \
                 The edit is one undo step, `Fit to Time Range`.",
            )
            .permission(Permission::Edit)
            .contexts([NoteEditor, NoteSelectionContext])
            .binding("S-NOTE-008")
            .args::<FitToTimeArgs>()
            .result::<NoteEditResult>(),
        CommandSpec::new(STRETCH, "Stretch Notes")
            .description(
                "Scales notes in time from the range `from` (or the time selection) to the \
                 range `to`, as dragging a note stretch marker does: `from`'s start goes to \
                 `to`'s start and its end to `to`'s end, and each note's start and end keep \
                 their place in proportion, so starts and lengths scale together. The notes \
                 are `notes`, or the selected notes, or with none selected those that start \
                 inside `from`. With `point`, what lies between the start and the point is \
                 scaled to end at the point's new place, and what lies between the point and \
                 the end to start there, as dragging the pseudo marker does. A `to` that ends \
                 before it starts mirrors the notes' order in time. A note's pressure points \
                 scale with it, and the breakpoints of the clip's linked envelopes inside \
                 `from` move by the same mapping, replacing any breakpoint they land on; \
                 unlinked envelopes stay. Refused, changing nothing, if a note would be \
                 shorter than a tick or start before the clip's contents. The time selection \
                 becomes `to`. The edit is one undo step, `Stretch Notes`.",
            )
            .permission(Permission::Edit)
            .contexts([NoteEditor, NoteSelectionContext])
            .args::<StretchNotesArgs>()
            .result::<NoteEditResult>(),
        CommandSpec::new(SPLIT_CLIP, "Split Clip")
            .description(
                "Splits the Arrangement clip the Clip View shows at the start and end of the \
                 editor's time selection, each that falls strictly inside the clip, as \
                 `arrangement.split` does, and selects the part inside. Cmd-Shift-E \
                 (Ctrl-Shift-E) runs it. Refused for a Session clip, or with no time \
                 selection.",
            )
            .permission(Permission::Edit)
            .contexts([NoteEditor, NoteSelectionContext])
            .binding("S-NOTE-036")
            .args::<NoArgs>()
            .result::<EditResult>(),
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
        // The value commands the lanes, the footer and the keys run.
        for id in [SET_VALUE, RAMP_VALUE, ADJUST_VALUE, RANDOMIZE, DEACTIVATE] {
            assert_eq!(class(id), Some(Permission::Edit), "{id}");
        }
        // The note clipboard: copying changes nothing, cutting removes work.
        assert_eq!(class(COPY), Some(Permission::Read));
        assert_eq!(class(CUT), Some(Permission::Destructive));
        assert_eq!(class(PASTE), Some(Permission::Edit));
        assert_eq!(class(DUPLICATE), Some(Permission::Edit));
        // The Edit menu's clipboard rows run the note commands in the note
        // editor (S-EDIT-001 to S-EDIT-005).
        for (id, shortcut) in [
            (CUT, "S-EDIT-001"),
            (COPY, "S-EDIT-002"),
            (PASTE, "S-EDIT-003"),
            (DUPLICATE, "S-EDIT-004"),
            (DELETE_NOTES, "S-EDIT-005"),
        ] {
            let bindings: Vec<&str> = registry
                .get(id)
                .unwrap()
                .bindings
                .iter()
                .map(|binding| binding.shortcut.as_ref())
                .collect();
            assert_eq!(bindings, [shortcut], "{id}");
        }
        // 0 deactivates the selected notes (S-NOTE-041).
        let deactivate: Vec<&str> = registry
            .get(DEACTIVATE)
            .unwrap()
            .bindings
            .iter()
            .map(|binding| binding.shortcut.as_ref())
            .collect();
        assert_eq!(deactivate, ["S-NOTE-041"]);
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
        let settings_bindings: Vec<&str> = registry
            .get(QUANTIZE_SETTINGS)
            .unwrap()
            .bindings
            .iter()
            .map(|binding| binding.shortcut.as_ref())
            .collect();
        assert_eq!(settings_bindings, ["S-SAMPLE-002"]);
    }
}
