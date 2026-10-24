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
use crate::note_filters::{InvertSelectionArgs, NoteFilters, NoteFiltersArgs, NoteFiltersResult};
use bl_commands::Context::{
    ClipView, Envelope, Global, LoopBrace, NoteEditor, NoteSelection as NoteSelectionContext,
    SampleEditor,
};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Breakpoint, ClipId, Expression, GroupKind, MpeLane};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `clip_view.select_notes`: select notes in the MIDI Note Editor.
pub const SELECT_NOTES: &str = "clip_view.select_notes";
/// `clip_view.note_filters`: Find and Select Notes' filter bar and filters.
pub const NOTE_FILTERS: &str = "clip_view.note_filters";
/// `clip_view.invert_selection`: swap the selected and unselected notes.
pub const INVERT_SELECTION: &str = "clip_view.invert_selection";
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
/// `note.set_expression`: replace one expression envelope of notes (Pitch,
/// Slide or Pressure).
pub const SET_EXPRESSION: &str = "note.set_expression";
/// `note.add_expression_breakpoint`: add a breakpoint to notes' envelopes.
pub const ADD_EXPRESSION_BREAKPOINT: &str = "note.add_expression_breakpoint";
/// `note.move_expression_breakpoints`: move breakpoints in time and value.
pub const MOVE_EXPRESSION_BREAKPOINTS: &str = "note.move_expression_breakpoints";
/// `note.delete_expression_breakpoints`: delete breakpoints.
pub const DELETE_EXPRESSION_BREAKPOINTS: &str = "note.delete_expression_breakpoints";
/// `note.set_expression_curve`: bend the segment a breakpoint starts.
pub const SET_EXPRESSION_CURVE: &str = "note.set_expression_curve";
/// `note.scale_expression`: scale a value over the whole length of notes.
pub const SCALE_EXPRESSION: &str = "note.scale_expression";
/// `note.draw_expression`: draw an envelope freehand.
pub const DRAW_EXPRESSION: &str = "note.draw_expression";
/// `note.clear_expressions`: clear every envelope of notes.
pub const CLEAR_EXPRESSIONS: &str = "note.clear_expressions";
/// `clip_view.select_expression_breakpoints`: select breakpoints in the
/// MPE tab.
pub const SELECT_EXPRESSION_BREAKPOINTS: &str = "clip_view.select_expression_breakpoints";
/// `clip_view.mpe_zoom`: bring the key rows of the Pitch values in the time
/// selection into view in the MPE tab.
pub const MPE_ZOOM: &str = "clip_view.mpe_zoom";
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
/// `clip_view.foreground`: choose the foreground clip of several shown.
pub const FOREGROUND: &str = "clip_view.foreground";
/// `clip_view.select_loop_bars`: choose the selected loop bars.
pub const SELECT_LOOP_BARS: &str = "clip_view.select_loop_bars";
/// `clip_view.focus_mode`: switch Focus Mode.
pub const FOCUS_MODE: &str = "clip_view.focus_mode";
/// `clip.duplicate_loop`: double clips' loops and copy their notes.
pub const DUPLICATE_LOOP: &str = "clip.duplicate_loop";
/// `note.group`: put notes in a new probability group.
pub const GROUP: &str = "note.group";
/// `note.ungroup`: take notes out of their probability groups.
pub const UNGROUP: &str = "note.ungroup";

/// `clip_view.set_preview`: the MIDI Note Editor's Preview switch.
pub const SET_PREVIEW: &str = "clip_view.set_preview";
/// `note.preview`: play a note on a MIDI clip's track.
pub const PREVIEW_NOTE: &str = "note.preview";
/// `note.step_record`: step-record the held notes at the insert marker.
pub const STEP_RECORD: &str = "note.step_record";
/// `clip_view.step_input_mode`: get the editor ready for step recording.
pub const STEP_INPUT_MODE: &str = "clip_view.step_input_mode";
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
    NOTE_FILTERS,
    INVERT_SELECTION,
    ADD_NOTE,
    DELETE_NOTES,
    MOVE_NOTES,
    RESIZE_NOTES,
    QUANTIZE_NOTES,
    QUANTIZE_SETTINGS,
    SET_VALUE,
    SET_EXPRESSION,
    ADD_EXPRESSION_BREAKPOINT,
    MOVE_EXPRESSION_BREAKPOINTS,
    DELETE_EXPRESSION_BREAKPOINTS,
    SET_EXPRESSION_CURVE,
    SCALE_EXPRESSION,
    DRAW_EXPRESSION,
    CLEAR_EXPRESSIONS,
    SELECT_EXPRESSION_BREAKPOINTS,
    MPE_ZOOM,
    RAMP_VALUE,
    ADJUST_VALUE,
    RANDOMIZE,
    DEACTIVATE,
    COPY,
    CUT,
    PASTE,
    DUPLICATE,
    FOREGROUND,
    SELECT_LOOP_BARS,
    FOCUS_MODE,
    DUPLICATE_LOOP,
    GROUP,
    UNGROUP,
    SET_PREVIEW,
    PREVIEW_NOTE,
    STEP_RECORD,
    STEP_INPUT_MODE,
    SPLIT,
    CHOP,
    JOIN,
    FIT_TO_TIME,
    STRETCH,
    SPLIT_CLIP,
];

/// Whether `id` is one of the Clip View's note-editing commands that edits
/// the notes, which `bl-shell`'s `clip_view::run` runs. The Preview switch,
/// a preview and Step Input Mode reach the audio engine, the preferences
/// and the focus, so the shell runs those itself ([`is_preview_or_mode`]).
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id) && !is_preview_or_mode(id)
}

/// Whether `id` is the Preview switch, a preview or Step Input Mode
/// (Spec: step-recording R1, R2, R9).
#[must_use]
pub fn is_preview_or_mode(id: &str) -> bool {
    matches!(id, SET_PREVIEW | PREVIEW_NOTE | STEP_INPUT_MODE)
}

/// Notes of one clip among several shown (Spec: multi-clip-editing R18).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipNotes {
    /// The MIDI clip.
    pub clip: ClipId,
    /// The notes, by their place in the clip's notes in order. Left out, the
    /// clip's selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
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
/// the clip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A tab of the Clip View: `notes` (the MIDI Note Editor), `envelopes` (the \
                   clip's envelopes) or `mpe` (the Expression Editor, a MIDI clip's per-note \
                   Pitch, Slide and Pressure)."
)]
pub enum ClipTab {
    /// The MIDI Note Editor.
    #[default]
    Notes,
    /// The clip's envelopes (WP-3.16).
    Envelopes,
    /// The Expression Editor, the MPE tab (WP-6.18): a MIDI clip's per-note
    /// Pitch, Slide and Pressure envelopes.
    Mpe,
}

impl ClipTab {
    /// The tabs `view.clip_tab` cycles through, in the order the Clip View
    /// lists them.
    pub const ALL: [Self; 3] = [Self::Notes, Self::Envelopes, Self::Mpe];

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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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

/// Arguments of `note.set_expression`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetExpressionArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Which envelope to replace: `pitch` (semitones from the note's key,
    /// −48 to 48), `slide` or `pressure` (0 to 127).
    pub envelope: Expression,
    /// The breakpoints every one of the notes gets, each at a position in
    /// ticks counted from the note's start (0 to its length, in order, none
    /// twice), with a value in the envelope's range and an optional `bend`
    /// from −1 to 1 for the segment starting there. An empty list clears
    /// the envelope.
    pub breakpoints: Vec<Breakpoint>,
}

/// A breakpoint of a note's expression envelope, named by the note and its
/// position (Spec: mpe R32).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct BreakpointRef {
    /// The note, by its place in the clip's notes in order (by start, then
    /// pitch).
    pub note: u32,
    /// The breakpoint's position in ticks counted from the note's start.
    pub at: Ticks,
}

/// Arguments of `note.add_expression_breakpoint`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddExpressionBreakpointArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order. Left out, the
    /// selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Which envelope: `pitch`, `slide` or `pressure`.
    pub envelope: Expression,
    /// The breakpoint's position in ticks counted from each note's start,
    /// from 0 to the note's length.
    pub at: Ticks,
    /// The value in the envelope's range.
    pub value: f64,
    /// The bend of the segment starting there, from −1 to 1. Left out, a
    /// breakpoint already there keeps its bend and a new one is straight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bend: Option<f64>,
}

/// Arguments of `note.move_expression_breakpoints`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveExpressionBreakpointsArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Which envelope: `pitch`, `slide` or `pressure`.
    pub envelope: Expression,
    /// The breakpoints to move. Left out, the breakpoints selected in the
    /// MPE tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Vec<BreakpointRef>>,
    /// How far to move them in time, in ticks; negative moves them earlier.
    /// The move is held so that every breakpoint of a note stays inside it.
    #[serde(default)]
    pub time: Ticks,
    /// How far to move them in value, in the envelope's units. The move is
    /// held so that every breakpoint stays inside the envelope's range.
    #[serde(default)]
    pub value: f64,
    /// A breakpoint moved past another in time takes that one's place and
    /// removes it. With `fill`, a segment inside this stretch of the clip's
    /// contents first gets breakpoints at the stretch's edges and moves
    /// whole: every breakpoint of the named notes' envelopes inside the
    /// stretch moves, and `breakpoints` only names the notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<TickRange>,
}

/// Arguments of `note.delete_expression_breakpoints`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteExpressionBreakpointsArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Which envelope: `pitch`, `slide` or `pressure`.
    pub envelope: Expression,
    /// The breakpoints to delete. Left out, the breakpoints selected in the
    /// MPE tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Vec<BreakpointRef>>,
}

/// Arguments of `note.set_expression_curve`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetExpressionCurveArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The note, by its place in the clip's notes in order.
    pub note: u32,
    /// Which envelope: `pitch`, `slide` or `pressure`.
    pub envelope: Expression,
    /// The position of the breakpoint the segment starts at.
    pub at: Ticks,
    /// The segment's bend, from −1 to 1; 0 is straight.
    pub bend: f64,
}

/// Arguments of `note.scale_expression`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScaleExpressionArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order. Left out, the
    /// selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Which value: `slide`, `pressure`, `velocity` or `release_velocity`
    /// (`pitch_bend` doesn't scale).
    pub lane: MpeLane,
    /// What every value is multiplied by, 0 or more; values stay inside the
    /// range.
    pub factor: f64,
}

/// One step of a freehand drawing: a value held for a stretch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DrawStep {
    /// Where the step starts, in ticks from the beginning of the clip's
    /// contents.
    pub at: Ticks,
    /// How long it lasts, in ticks, 1 or more.
    pub length: Ticks,
    /// The value it holds.
    pub value: f64,
}

/// Arguments of `note.draw_expression`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DrawExpressionArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order. Left out, the
    /// selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// Which envelope: `pitch`, `slide` or `pressure`.
    pub envelope: Expression,
    /// The steps drawn, in order of `at`. In each note, the stretch from the
    /// first step's start to the last step's end gets the steps in place of
    /// its breakpoints.
    pub steps: Vec<DrawStep>,
    /// For `pitch`: the steps' values are pitches in semitones above key 0
    /// (the note area's rows), and each note takes the value less its own
    /// key. Otherwise they are values in the envelope's range.
    #[serde(default)]
    pub absolute: bool,
}

/// Arguments of `note.clear_expressions`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearExpressionsArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order. Left out, the
    /// selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
}

/// Arguments of `clip_view.select_expression_breakpoints`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectExpressionBreakpointsArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Which envelope the breakpoints are on, and the envelope that has the
    /// keyboard afterwards. Left out, the one that has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Expression>,
    /// The breakpoints to select. Left out with `all` false, none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Vec<BreakpointRef>>,
    /// `replace` them (the default) or `toggle` each in or out.
    #[serde(default)]
    pub mode: NoteSelectMode,
    /// Select every breakpoint of the envelope in the selected notes, as
    /// Cmd-A does.
    #[serde(default)]
    pub all: bool,
}

/// The MPE tab's selection and lanes, as `view.state` reports them (Spec:
/// mpe R23, R25).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MpeTabState {
    /// The lanes whose selector toggle is on, in lane order.
    pub toggled: Vec<MpeLane>,
    /// Whether the triangle has hidden the lanes and their toggles.
    pub hidden: bool,
    /// The lanes drawn now, from the top, with their heights in points:
    /// the Pitch Bend lane first on a Drum Rack's track.
    pub lanes: Vec<MpeLaneState>,
    /// Whether the tab's own grid is on for the clip shown.
    pub grid: bool,
    /// The envelope that has the keyboard, if a breakpoint was selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Expression>,
    /// The selected breakpoints of that envelope.
    pub breakpoints: Vec<BreakpointRef>,
}

/// A lane drawn in the MPE tab.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MpeLaneState {
    /// Which lane.
    pub lane: MpeLane,
    /// How tall it is, in points.
    pub height: f32,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
    /// Whether the notes end up active (`true`) or deactivated (`false`).
    /// Left out, they are deactivated, or activated when every one of them
    /// is deactivated already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}

/// Arguments of `note.group`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
    /// How the group plays. Left out, the kind made last in this session,
    /// `play_all` until one is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<GroupKind>,
}

/// Arguments of `note.ungroup`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UngroupNotesArgs {
    /// The MIDI clip. Left out, the one the Clip View shows (the
    /// Arrangement's selected clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The notes, by their place in the clip's notes in order (by start,
    /// then pitch). Left out, the selected notes. Naming every note of a
    /// group ends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<u32>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
}

/// Arguments of `clip_view.set_preview`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPreviewArgs {
    /// Whether Preview is on. Left out, the switch flips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `note.preview`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewNoteArgs {
    /// The MIDI clip whose track plays the note. Left out, the one the Clip
    /// View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The pitch, a MIDI key from 0 to 127; 60 is middle C.
    pub key: u8,
    /// The velocity, from 1 to 127; 100 unless given. Used when `on`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u8>,
    /// Press (true) or release (false) the note.
    pub on: bool,
}

/// A key held on the track's MIDI input, with the velocity it was played at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeldKey {
    /// The pitch, a MIDI key from 0 to 127; 60 is middle C.
    pub key: u8,
    /// The velocity it was played at, from 1 to 127.
    pub velocity: u8,
}

/// Arguments of `note.step_record`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepRecordArgs {
    /// The MIDI clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Step back, as Left does, instead of forward, as Right does.
    #[serde(default)]
    pub back: bool,
    /// The keys held, each with its velocity. Left out, the keys held on the
    /// track's MIDI input now; an empty list means no key is held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<Vec<HeldKey>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// The clips, each with its notes, in place of `clip` and `notes`
    /// (Spec: multi-clip-editing R18). Left out with `clip` and `notes`,
    /// the selected notes of every clip shown, or of the active clip alone
    /// in Focus Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipNotes>>,
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
    /// Across several clips (Spec: multi-clip-editing R18): what the edit
    /// did in each clip. `notes` and `selected` above are the first
    /// clip's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ClipEdit>,
}

/// What an edit across several clips did in one of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipEdit {
    /// The clip.
    pub clip: ClipId,
    /// The notes the edit created or changed in it, by their place in its
    /// notes in order afterwards.
    pub notes: Vec<u32>,
    /// The notes selected in it afterwards.
    pub selected: Vec<u32>,
}

/// What `note.step_record` did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StepRecordResult {
    /// How many notes it added.
    pub added: u32,
    /// How many notes it made one grid step longer.
    pub lengthened: u32,
    /// How many notes it made one grid step shorter.
    pub shortened: u32,
    /// How many notes it removed, having no length left.
    pub removed: u32,
    /// The insert marker afterwards, in ticks from the beginning of the
    /// clip's contents.
    pub insert_marker: Ticks,
    /// Whether the step was recorded as an undo step: false when it changed
    /// no note.
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipViewState {
    /// Whether the Clip View shows in the detail area.
    pub visible: bool,
    /// Whether Draw Mode is on (B): clicking the note area adds a note, and
    /// clicking a note removes it.
    pub draw_mode: bool,
    /// The clip the note editor shows; `null` when none does. With several
    /// clips shown, the foreground clip.
    pub clip: Option<ClipId>,
    /// The clips the note editor shows, in the loop bars' order (Spec:
    /// multi-clip-editing R1, R2): none, one, or up to eight.
    pub clips: Vec<ClipId>,
    /// The loop bars selected, in the order of `clips`.
    pub loop_bars: Vec<ClipId>,
    /// Whether Focus Mode is on (Spec: multi-clip-editing R13).
    pub focus_mode: bool,
    /// The selected notes of the foreground clip, by their place in its
    /// notes in order (by start, then pitch).
    pub selected: Vec<u32>,
    /// The selected notes of every clip shown, by clip; the foreground
    /// clip's are `selected` too.
    pub clip_selections: Vec<ClipEdit>,
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
    /// Whether the Preview switch is on (Spec: step-recording R1): one
    /// switch for every MIDI clip's editor, kept between launches.
    pub preview: bool,
    /// The parameter whose envelope the Envelopes tab shows; `null` when it
    /// shows none.
    pub envelope: Option<AutomationTarget>,
    /// Find and Select Notes: whether the filter bar shows, and every
    /// filter's settings, with which are on.
    pub filters: NoteFilters,
    /// The Envelope Editor's insert marker, in ticks from the beginning of
    /// the clip's contents (WP-6.26), which Left and Right move along the
    /// envelope; `null` until it has been moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_marker: Option<Ticks>,
    /// The breakpoint selected in the Envelope Editor (WP-6.26), if any:
    /// `clip.select_envelope_breakpoint` sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_breakpoint: Option<Ticks>,
    /// The MPE tab's lanes, grid and selected breakpoints (WP-6.18).
    #[serde(default)]
    pub mpe: MpeTabState,
}

/// Arguments of `clip_view.foreground`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForegroundArgs {
    /// The shown clip to make the foreground clip.
    pub clip: ClipId,
}

/// Arguments of `clip_view.select_loop_bars`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectLoopBarsArgs {
    /// The clips whose loop bars to select; empty selects none.
    #[serde(default)]
    pub clips: Vec<ClipId>,
    /// Add them to the selected bars, or take out those already selected,
    /// as Cmd-click does, instead of replacing the selection.
    #[serde(default)]
    pub toggle: bool,
}

/// Arguments of `clip_view.focus_mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FocusModeArgs {
    /// Whether Focus Mode ends up on; left out, it switches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `clip.duplicate_loop`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateLoopArgs {
    /// The MIDI clips. Left out, the clips of the selected loop bars, or
    /// the clip shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
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
        CommandSpec::new(NOTE_FILTERS, "Note Filters")
            .description(
                "Find and Select Notes. Shows or hides the filter bar (`visible`, or `toggle`, \
                 as Cmd-Shift-F does), chooses the filter whose controls it shows (`shown`), \
                 and sets the controls and Invert of any filter (`pitch`, `time`, `chance`, \
                 `condition`, `count`, `duration`, `scale`, `velocity`). Setting a control, or \
                 Invert, switches that filter on, and while any filter is on the selection is \
                 the notes in the clip's span that every filter that is on lets through, worked \
                 out again each time, with the time selection following it. `select` presses \
                 the Select button of each filter it lists: it is switched on and the notes are \
                 selected again. `off` switches filters off, keeping their settings; a click in \
                 the note area switches all eight off. Time's Start, Length and Repeat are in \
                 beats of the clip's time signature, to a hundredth, Start from 1; a Repeat of \
                 `null` is one bar. Duration's Min and Max are in beats too, a Max of `null` \
                 being the clip's span. Chance is in percent. Moving Min past Max takes Max \
                 with it, and the other way round. The settings belong to the editor: they are \
                 kept while the app runs, every filter is switched off when the editor shows \
                 another clip, and none of it is saved with the set or is an undo step. The \
                 result is every filter's settings and the selected notes.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, NoteSelectionContext])
            .binding(Binding::new("S-NOTE-037").arg("toggle", true))
            .args::<NoteFiltersArgs>()
            .result::<NoteFiltersResult>(),
        CommandSpec::new(INVERT_SELECTION, "Invert Selection")
            .description(
                "Selects every note in the clip's span that was not selected and deselects \
                 those that were, as Edit \u{25b8} Invert Selection does (Cmd-Shift-A, \
                 Ctrl-Shift-A on Windows). The time selection follows the new selection. It \
                 needs a clip that holds notes, is never an undo step, and changes nothing \
                 in the set.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, NoteSelectionContext])
            .binding("S-NOTE-034")
            .args::<InvertSelectionArgs>()
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
            SET_EXPRESSION,
            "Set Note Expression",
            Permission::Edit,
            "Replaces one expression envelope of notes (`notes`, or the selected notes) with \
             `breakpoints`, as editing the Clip View's MPE tab does: `envelope` is `pitch` \
             (the note's own pitch bend in semitones from its key, -48 to 48), `slide` (0 to \
             127) or `pressure` (0 to 127). Each breakpoint is an `at` in ticks counted from \
             the note's start, from 0 to the note's length, a `value` in the envelope's range, \
             and a `bend` from -1 to 1 for the segment starting there (0 is straight); they go \
             in order of `at`, none twice. An empty list clears the envelope. Nothing changes \
             if a value, a position or a bend is out of range, two breakpoints share a \
             position, or the clip isn't a MIDI clip. One undo step, Set Note Expression; \
             the notes stay selected. The clip query's notes read each envelope back.",
        )
        .args::<SetExpressionArgs>(),
        edit(
            ADD_EXPRESSION_BREAKPOINT,
            "Add Expression Breakpoint",
            Permission::Edit,
            "Adds a breakpoint at `at` (ticks counted from each note's start, from 0 to its \
             length) with `value` to the `envelope` (`pitch`, `slide` or `pressure`) of notes \
             (`notes`, or the selected notes), as clicking a segment in the Clip View's MPE \
             tab does. A breakpoint already at `at` takes the value, and `bend` if given. \
             Nothing changes if the value, the position or the bend is out of range for any \
             of the notes. One undo step, Add Expression Breakpoint. The clip query's notes \
             read the envelope back.",
        )
        .args::<AddExpressionBreakpointArgs>(),
        edit(
            MOVE_EXPRESSION_BREAKPOINTS,
            "Move Expression Breakpoints",
            Permission::Edit,
            "Moves breakpoints of one `envelope` in time by `time` ticks and in value by \
             `value`, as dragging one in the MPE tab does. They are named in `breakpoints` \
             (a note's place and the breakpoint's position in ticks from its start), or are \
             the breakpoints selected in the tab. The move is held so every breakpoint of a \
             note stays inside the note and inside the envelope's range, and a breakpoint \
             that is not moved and that a moved one passes or lands on is removed. With \
             `fill`, a stretch of the clip's contents, the breakpoints of the named notes \
             inside it move whole, after breakpoints are added at its edges. One undo step, \
             Move Expression Breakpoints; a drag runs this once per frame, measured from \
             where it began, in one transaction.",
        )
        .args::<MoveExpressionBreakpointsArgs>(),
        edit(
            DELETE_EXPRESSION_BREAKPOINTS,
            "Delete Expression Breakpoints",
            Permission::Edit,
            "Deletes breakpoints of one `envelope`, named in `breakpoints` (a note's place and \
             the breakpoint's position in ticks from its start) or selected in the MPE tab, \
             as clicking one does. One undo step, Delete Expression Breakpoints. The clip \
             query's notes read the envelopes back.",
        )
        .args::<DeleteExpressionBreakpointsArgs>(),
        edit(
            SET_EXPRESSION_CURVE,
            "Set Expression Curve",
            Permission::Edit,
            "Bends the segment that starts at the breakpoint at `at` in a note's `envelope`, \
             by `bend` from −1 to 1 (0 is straight), as an Option-drag on a segment does; a \
             double-click with Option sets 0. Nothing changes if there is no such breakpoint. \
             One undo step, Set Expression Curve.",
        )
        .args::<SetExpressionCurveArgs>(),
        edit(
            SCALE_EXPRESSION,
            "Scale Expression",
            Permission::Edit,
            "Multiplies a value of notes (`notes`, or the selected notes) by `factor` over \
             the note's whole length, as dragging up or down outside a note over its \
             highlighted lane in the MPE tab does: `lane` is `slide` or `pressure` (every \
             breakpoint's value, 0 to 127), or `velocity` or `release_velocity` (1 to 127). \
             Values scale in proportion from 0 and stay in the range. One undo step, Scale \
             Expression.",
        )
        .args::<ScaleExpressionArgs>(),
        edit(
            DRAW_EXPRESSION,
            "Draw Expression",
            Permission::Edit,
            "Draws an `envelope` freehand over notes (`notes`, or the selected notes) as \
             Draw Mode does in the MPE tab: in each note, the stretch from the first step's \
             start to the last step's end loses its breakpoints and gets the `steps`, each a \
             value held for `length` ticks (a breakpoint at its start and one tick before the \
             next). With `absolute`, a pitch step is in semitones above key 0 and each note \
             takes it less its own key. Values outside the range are held in it. One undo \
             step, Draw Expression; a drag runs this once per frame in one transaction.",
        )
        .args::<DrawExpressionArgs>(),
        edit(
            CLEAR_EXPRESSIONS,
            "Clear All Envelopes",
            Permission::Edit,
            "Clears the Pitch, Slide and Pressure envelopes of notes (`notes`, or the \
             selected notes), as Clear All Envelopes in the MPE tab's context menus does. One \
             undo step, Clear All Envelopes. The clip query's notes read the envelopes back.",
        )
        .args::<ClearExpressionsArgs>(),
        CommandSpec::new(SELECT_EXPRESSION_BREAKPOINTS, "Select Expression Breakpoints")
            .description(
                "Selects breakpoints of one `envelope` in the Clip View's MPE tab, as a \
                 marquee, a click on a segment or Cmd-A does: the `breakpoints` named (a \
                 note's place and the breakpoint's position in ticks from its start), \
                 replacing the selection or, with `mode` `toggle`, each in or out; or with \
                 `all`, every breakpoint of the envelope in the selected notes (Cmd-A with an \
                 envelope focused). The envelope becomes the one that has the keyboard. \
                 Selecting is never an undo step and changes nothing in the set. The result \
                 is the Clip View's state, whose `mpe` lists the selection.",
            )
            .permission(Permission::Read)
            .contexts([Envelope])
            .binding(Binding::new("S-NOTE-053").arg("all", true))
            .args::<SelectExpressionBreakpointsArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(MPE_ZOOM, "Zoom to Expression")
            .description(
                "Scrolls the MPE tab's note area to the key rows that the Pitch envelopes of \
                 the selected notes reach inside the time selection (Z). The editor has no \
                 time zoom yet, so only the key rows move. Changes only the view, so it is \
                 no undo step. The result is the Clip View's state.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor])
            .binding("S-NOTE-023")
            .args::<NoArgs>()
            .result::<ClipViewState>(),
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
        edit(
            GROUP,
            "Group Notes",
            Permission::Edit,
            "Puts notes (`notes`, or the selected notes) in a new probability group of \
             `kind`: `play_all` plays all of its notes or none, `play_one` plays one of them, \
             chosen evenly. Left out, `kind` is the one made last in this session, `play_all` \
             until one is chosen. Notes leave the group they were in, a group left with fewer \
             than two notes ends, and the new group's chance is that of its first note in the \
             clip's order, which every note of it takes. Naming every note of one group \
             changes its kind. Refused with fewer than two notes. One undo step, named Group \
             Notes, or Change Group when it changes one group's kind; the notes stay \
             selected. Read the groups back from the clip's `data.midi.groups` and each note's \
             `group`.",
        )
        .binding("S-NOTE-031")
        .args::<GroupNotesArgs>(),
        edit(
            UNGROUP,
            "Ungroup Notes",
            Permission::Edit,
            "Takes notes (`notes`, or the selected notes) out of their probability groups, \
             each keeping its group's chance as its own; a group left with fewer than two \
             notes ends, and naming every note of a group ends it. One undo step, named \
             Ungroup Notes; the notes stay selected. Read the groups back from the clip's \
             `data.midi.groups` and each note's `group`.",
        )
        .binding("S-NOTE-032")
        .args::<UngroupNotesArgs>(),
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
        CommandSpec::new(FOREGROUND, "Foreground Clip")
            .description(
                "Makes one of the clips the note editor shows the foreground clip, as clicking \
                 its note or its loop bar does: the expression lanes and the footer edit its \
                 notes alone, and with Focus Mode on it is the active clip. Changes only the \
                 view: it isn't saved with the set and isn't an undo step. The result is the \
                 Clip View's state.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, NoteSelectionContext])
            .args::<ForegroundArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(SELECT_LOOP_BARS, "Select Loop Bars")
            .description(
                "Selects the loop bars above the note editor of the given clips, in place of \
                 the selected ones, or with `toggle` adds each to the selection or takes it \
                 out, as Cmd-click does (Ctrl-click on Windows). The last clip given becomes \
                 the foreground clip. With Focus Mode on only one bar can be selected. \
                 Changes only the view: it isn't an undo step. The result is the Clip View's \
                 state.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, NoteSelectionContext])
            .args::<SelectLoopBarsArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(FOCUS_MODE, "Focus Mode")
            .description(
                "Switches Focus Mode (N, or the Focus button): a tap switches it, and holding \
                 N switches it while it's held. It acts while the note editor shows several \
                 clips: with it on, edits, drawing and the filters reach the foreground clip \
                 alone, and the others show grey. It is off when the app starts and isn't \
                 saved or an undo step. `on` says which way; left out, it switches. The result \
                 is the Clip View's state.",
            )
            .permission(Permission::Read)
            .contexts([NoteEditor, NoteSelectionContext])
            .binding("S-NOTE-040")
            .args::<FocusModeArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(DUPLICATE_LOOP, "Duplicate Loop")
            .description(
                "Doubles the loop brace of each MIDI clip (`clips`, or the clips of the \
                 selected loop bars, or the clip shown) and copies the notes inside it into \
                 the new half; notes after the loop's end move later with it, keeping their \
                 distance from the end. Cmd-D (Ctrl-D) with the loop brace or loop bars \
                 selected, and Duplicate in a loop bar's menu, run it. A clip whose loop \
                 would reach past the clip's end is refused. One undo step, named Duplicate \
                 Loop.",
            )
            .permission(Permission::Edit)
            .contexts([LoopBrace])
            .binding("S-LOOP-012")
            .args::<DuplicateLoopArgs>()
            .result::<NoteEditResult>(),
        CommandSpec::new(SET_PREVIEW, "MIDI Note Editor Preview")
            .description(
                "Sets the MIDI Note Editor's Preview switch (`on`), the one above the piano \
                 ruler and Options \u{25b8} MIDI Note Editor Preview: with it on, clicking a key, \
                 drawing, clicking and keyboard-selecting a note, and dragging notes to a new \
                 pitch play into the clip's track, and an armed track can be step-recorded. \
                 Left out, `on` flips the switch. It is one switch for every MIDI clip, off in \
                 a new install and kept between launches as a preference; it is never an undo \
                 step and changes nothing in the set. Switching it off ends the previews \
                 sounding.",
            )
            .permission(Permission::Edit)
            .contexts([Global, ClipView, NoteEditor, NoteSelectionContext])
            .args::<SetPreviewArgs>()
            .result::<ClipViewState>(),
        CommandSpec::new(PREVIEW_NOTE, "Preview Note")
            .description(
                "Plays a note on the track of a MIDI clip (`clip`, or the one the Clip View \
                 shows), through its devices and mixer, whether or not the song plays and \
                 whether or not the track is armed or monitoring: `on` presses it (`velocity`, \
                 100 unless given) and a later call with `on` false releases it. A press with \
                 no release plays until the release, so an agent that presses must release. \
                 Needs the Preview switch on. It changes nothing in the set and is never an \
                 undo step.",
            )
            .permission(Permission::Edit)
            .contexts([Global, ClipView, NoteEditor, NoteSelectionContext])
            .args::<PreviewNoteArgs>()
            .result::<ClipViewState>(),
        edit(
            STEP_RECORD,
            "Step Record",
            Permission::Edit,
            "Step-records at the insert marker of a MIDI clip while its track is armed, Preview \
             is on and the transport is stopped. Forward (Right): each held key (`held`, or \
             the keys held on the track's MIDI input) is added at the insert marker, one grid \
             step long at its velocity, and the marker moves one step on; held keys that this \
             stepping just added are made one step longer instead, and a key let go is left as \
             it is. Back (`back`, Left): the held notes just added are made one step shorter, a \
             note left with no length is removed, and the marker moves one step back. With \
             nothing held, the marker moves a step and leaves a rest. No record quantisation \
             applies. One undo step, `Step Record`.",
        )
        .args::<StepRecordArgs>()
        .result::<StepRecordResult>(),
        CommandSpec::new(STEP_INPUT_MODE, "Step Input Mode")
            .description(
                "Gets the editor ready for step recording: arms the track of the clip the \
                 Clip View shows (as the track's arm button does), moves the keyboard focus to \
                 the Clip View, and switches the computer MIDI keyboard to the other state. It \
                 is a command, not a switch, and is available while the editor shows a MIDI \
                 clip. Arming is an undo step, as arming is.",
            )
            .permission(Permission::Edit)
            .contexts([Global, ClipView, NoteEditor, NoteSelectionContext])
            .binding("S-NOTE-044")
            .args::<NoArgs>()
            .result::<ClipViewState>(),
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
        for id in [
            SET_VALUE,
            RAMP_VALUE,
            ADJUST_VALUE,
            RANDOMIZE,
            DEACTIVATE,
            GROUP,
            UNGROUP,
        ] {
            assert_eq!(class(id), Some(Permission::Edit), "{id}");
        }
        // The note clipboard: copying changes nothing, cutting removes work.
        assert_eq!(class(COPY), Some(Permission::Read));
        assert_eq!(class(CUT), Some(Permission::Destructive));
        assert_eq!(class(PASTE), Some(Permission::Edit));
        assert_eq!(class(DUPLICATE), Some(Permission::Edit));
        // The Preview switch, a preview, step recording and Step Input Mode
        // (step-recording R13).
        for id in [SET_PREVIEW, PREVIEW_NOTE, STEP_RECORD, STEP_INPUT_MODE] {
            assert_eq!(class(id), Some(Permission::Edit), "{id}");
        }
        assert!(handles(STEP_RECORD) && !handles(SET_PREVIEW));
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
