//! The Clip View's note editing (WP-2.3): the MIDI Note Editor's notes, the
//! notes selected in it, the commands that draw, delete, move, resize and
//! quantise them, and the view's Quantize settings.
//!
//! A note has no ID in the document, so the editor and the commands name one
//! by its place in the clip's notes in order: by start, then pitch (see
//! [`notes_in_order`]). The notes' positions and lengths are ticks in the
//! clip's contents; the editor shows the contents from the clip's Start
//! marker, which falls at the clip's start (Spec: midi-clips R3, R4), and
//! the playback path reads `MidiData::start` the same way.
//!
//! Every edit is one transaction of [`Command::SetClipData`], so it is one
//! undo step, and a drag runs the same command again on each frame from
//! where it began, inside one transaction (see `Shell::gesture`). People
//! reach the commands through the Clip View's clicks and drags; agents and
//! tests name the clip and the notes. The behaviour is in the piano roll
//! spec.

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::{Ticks, TimeSignature, TimeSignatureMap};
use bl_model::note_ops::{self, MOST_PARTS, Mapping};
use bl_model::{
    AutomationTarget, ClipData, ClipFolds, ClipId, Command, Document, Editor, GroupKind, MidiData,
    Note, NoteGroup,
};
use bl_shell_spec::editing::{Position, SplitArgs};
use bl_shell_spec::notes::{
    self as ids, AddNoteArgs, AdjustValueArgs, ChopNotesArgs, ChopStep, ClipEdit, ClipNotes,
    ClipTab, ClipViewState, CopyNotesArgs, DeactivateArgs, DeleteNotesArgs, DuplicateLoopArgs,
    DuplicateNotesArgs, FitToTimeArgs, FocusModeArgs, ForegroundArgs, GroupNotesArgs, HeldKey,
    JoinNotesArgs, MoveNotesArgs, NoteClipboardState, NoteEditResult, NoteField, NoteSelectMode,
    NoteSelection, PasteNotesArgs, QuantizeNotesArgs, QuantizeSettings, QuantizeSettingsArgs,
    QuantizeTarget, RampValueArgs, RandomizeAmounts, RandomizeArgs, ResizeNotesArgs,
    SelectLoopBarsArgs, SelectNotesArgs, SetValueArgs, SplitNotesArgs, StepRecordArgs,
    StepRecordResult, StretchNotesArgs, UngroupNotesArgs,
};
use bl_shell_spec::sample_editor as sample_ids;
use bl_shell_spec::sample_editor::{SelectTimeArgs, SetInsertMarkerArgs};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{EditFailure, transaction};
use crate::arrangement::grid::{Step, lines, snap};
use crate::sample_editor::SampleView;

/// The most clips the note editor shows at once (Spec: multi-clip-editing
/// R1).
pub const MOST_CLIPS: usize = 8;

fn invalid(message: impl Into<String>) -> EditFailure {
    EditFailure::InvalidArgs(message.into())
}

fn unavailable(reason: impl Into<String>) -> EditFailure {
    EditFailure::Unavailable(reason.into())
}

fn refused(reason: impl Into<String>) -> EditFailure {
    EditFailure::Refused(reason.into())
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, EditFailure> {
    let args = if args.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        args
    };
    serde_json::from_value(args).map_err(|error| invalid(error.to_string()))
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A clip's notes in the editor's order: by start, then pitch. Two notes
/// that start on the same tick at the same pitch keep the order they're
/// stored in.
#[must_use]
pub fn notes_in_order(midi: &MidiData) -> Vec<Note> {
    let mut notes: Vec<Note> = midi.notes.iter().cloned().collect();
    notes.sort_by_key(|note| (note.start, note.key));
    notes
}

/// A clip's span of its contents, in ticks: the contents tick the editor
/// shows at its left edge, and the one at its right edge (piano-roll R4).
#[must_use]
pub fn span(midi: &MidiData, length: Ticks) -> (Ticks, Ticks) {
    (midi.start, midi.start + length)
}

/// The place of the note whose start is nearest `at`: the nearest start,
/// and of the notes that start together the lowest pitch (piano-roll R25).
fn nearest(ordered: &[Note], at: Ticks) -> Option<u32> {
    let start = ordered
        .iter()
        .map(|note| (note.start - at).abs())
        .min()
        .and_then(|distance| {
            ordered
                .iter()
                .find(|note| (note.start - at).abs() == distance)
                .map(|note| note.start)
        })?;
    ordered
        .iter()
        .position(|note| note.start == start)
        .and_then(|place| u32::try_from(place).ok())
}

/// The place of the note before or after `place` in the clip's order, at
/// the same pitch or at any pitch (piano-roll R25).
fn neighbour(ordered: &[Note], place: u32, same_pitch: bool, forward: bool) -> Option<u32> {
    let note = ordered.get(place as usize)?;
    let at = place as usize;
    let found = if forward {
        ordered
            .iter()
            .enumerate()
            .skip(at + 1)
            .find(|(_, other)| !same_pitch || other.key == note.key)
    } else {
        ordered
            .iter()
            .enumerate()
            .take(at)
            .rev()
            .find(|(_, other)| !same_pitch || other.key == note.key)
    };
    found.map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
}

/// The place of the note the keyboard moves from: the last of the selected
/// notes in the clip's order.
fn current(selected: &[u32]) -> Option<u32> {
    selected.iter().copied().max()
}

/// The notes `what` selects in `ordered`, as the keyboard and the piano
/// ruler reach them (piano-roll R25, R26).
fn select_by(
    what: NoteSelection,
    ordered: &[Note],
    span: (Ticks, Ticks),
    insert: Option<Ticks>,
    selected: &[u32],
    add: bool,
) -> Vec<u32> {
    let in_span = |note: &Note| note.start >= span.0 && note.start < span.1;
    let places = |notes: Vec<usize>| {
        notes
            .into_iter()
            .filter_map(|place| u32::try_from(place).ok())
            .collect::<Vec<u32>>()
    };
    let chosen: Vec<u32> = match what {
        NoteSelection::Nearest => nearest(ordered, insert.unwrap_or(span.0))
            .into_iter()
            .collect(),
        NoteSelection::Previous => current(selected)
            .and_then(|place| neighbour(ordered, place, false, false))
            .into_iter()
            .collect(),
        NoteSelection::Next => current(selected)
            .and_then(|place| neighbour(ordered, place, false, true))
            .into_iter()
            .collect(),
        NoteSelection::SamePitchPrevious => current(selected)
            .and_then(|place| neighbour(ordered, place, true, false))
            .into_iter()
            .collect(),
        NoteSelection::SamePitchNext => current(selected)
            .and_then(|place| neighbour(ordered, place, true, true))
            .into_iter()
            .collect(),
        NoteSelection::All => places(
            ordered
                .iter()
                .enumerate()
                .filter(|(_, note)| in_span(note))
                .map(|(place, _)| place)
                .collect(),
        ),
        NoteSelection::Key(key) => places(
            ordered
                .iter()
                .enumerate()
                .filter(|(_, note)| note.key == key && in_span(note))
                .map(|(place, _)| place)
                .collect(),
        ),
    };
    if !add {
        // There's no such note to jump to: the selection stays.
        if chosen.is_empty()
            && matches!(
                what,
                NoteSelection::Nearest
                    | NoteSelection::Previous
                    | NoteSelection::Next
                    | NoteSelection::SamePitchPrevious
                    | NoteSelection::SamePitchNext
            )
        {
            return selected.to_vec();
        }
        return chosen;
    }
    // A key in the piano ruler takes its notes out of the selection when
    // they are all selected (piano-roll R26).
    if let NoteSelection::Key(_) = what
        && !chosen.is_empty()
        && chosen.iter().all(|place| selected.contains(place))
    {
        return selected
            .iter()
            .copied()
            .filter(|place| !chosen.contains(place))
            .collect();
    }
    let mut out = selected.to_vec();
    for place in chosen {
        if !out.contains(&place) {
            out.push(place);
        }
    }
    out.sort_unstable();
    out
}

/// The time selection the notes at `places` cover: from their earliest
/// start to their latest end (piano-roll R24).
fn span_of(doc: &Document, clip: ClipId, places: &[u32]) -> Option<(Ticks, Ticks)> {
    let midi = match &doc.clip(clip)?.data {
        ClipData::Midi(midi) => midi,
        ClipData::Audio(_) => return None,
    };
    let ordered = notes_in_order(midi);
    let notes: Vec<&Note> = places
        .iter()
        .filter_map(|place| ordered.get(*place as usize))
        .collect();
    let from = notes.iter().map(|note| note.start).min()?;
    let to = notes.iter().map(|note| note.start + note.length).max()?;
    Some((from, to))
}

/// The furthest a drag may move `notes`: in time, so no note starts before
/// the clip's contents begin, and in pitch, within MIDI keys 0 to 127. For
/// the UI, which clamps its drags rather than have the edit refused.
#[must_use]
pub fn move_limits(notes: &[Note]) -> (Ticks, i32, i32) {
    let earliest = -notes
        .iter()
        .map(|note| note.start)
        .min()
        .unwrap_or(Ticks::ZERO);
    let lowest = i32::from(notes.iter().map(|note| note.key).min().unwrap_or(0));
    let highest = i32::from(notes.iter().map(|note| note.key).max().unwrap_or(127));
    (earliest, -lowest, 127 - highest)
}

/// How much shorter a drag may make `notes` before one of them is shorter
/// than `shortest`: the smallest length less `shortest`. For the UI, which
/// clamps its drags.
#[must_use]
pub fn resize_limit(notes: &[Note], shortest: Ticks) -> Ticks {
    let room = notes
        .iter()
        .map(|note| note.length - shortest)
        .min()
        .unwrap_or(Ticks::ZERO)
        .max(Ticks::ZERO);
    -room
}

/// The values a ramp gives notes that start at `starts`, in the clip's
/// order: `from` at the first note by start, `to` at the last, and those
/// between on a straight line by their start times, so notes that start
/// together take the same value; whole numbers for the whole-number fields.
/// When every note starts together they all take `from` (Spec: note-editing
/// R11).
#[must_use]
pub fn ramp_values(field: NoteField, from: f64, to: f64, starts: &[Ticks]) -> Vec<f64> {
    let (Some(first), Some(last)) = (starts.first(), starts.last()) else {
        return Vec::new();
    };
    let width = (*last - *first).get() as f64;
    starts
        .iter()
        .map(|start| {
            let along = if width > 0.0 {
                (*start - *first).get() as f64 / width
            } else {
                0.0
            };
            let at = from + (to - from) * along;
            if field.whole() { at.round() } else { at }
        })
        .collect()
}

/// A note's value of `field`.
#[must_use]
pub fn note_value(note: &Note, field: NoteField) -> f64 {
    match field {
        NoteField::Velocity => f64::from(note.velocity),
        NoteField::ReleaseVelocity => f64::from(note.release_velocity),
        NoteField::Chance => note.probability,
        NoteField::VelocityDeviation => f64::from(note.velocity_deviation),
    }
}

/// The room a note's velocity leaves its velocity deviation: from 1 less
/// than the velocity to 127 more (Spec: note-editing R7).
#[must_use]
pub fn deviation_room(velocity: u8) -> (i16, i16) {
    (1 - i16::from(velocity), 127 - i16::from(velocity))
}

/// A value for `field`, checked against the field's range and its
/// whole-number rule.
fn value_of(field: NoteField, value: f64) -> Result<f64, EditFailure> {
    let (low, high) = field.range();
    if !(low..=high).contains(&value) {
        return Err(invalid(format!(
            "`{}` must be from {low} to {high}",
            field.name()
        )));
    }
    if field.whole() && value.fract() != 0.0 {
        return Err(invalid(format!("`{}` takes whole numbers", field.name())));
    }
    Ok(value)
}

/// Sets a note's `field` to `value`, which [`value_of`] checked, or to the
/// nearest the note has room for: a velocity deviation stays within what its
/// velocity leaves (Spec: note-editing R8).
fn set_note_value(note: &mut Note, field: NoteField, value: f64) {
    match field {
        NoteField::Velocity => note.velocity = value as u8,
        NoteField::ReleaseVelocity => note.release_velocity = value as u8,
        NoteField::Chance => note.probability = value,
        NoteField::VelocityDeviation => {
            let (low, high) = deviation_room(note.velocity);
            note.velocity_deviation = (value as i16).clamp(low, high) as i8;
        }
    }
}

/// A chance with float noise rounded off, so ten steps of 10 % land on 100 %.
fn tidy(field: NoteField, value: f64) -> f64 {
    if field.whole() {
        value.round()
    } else {
        (value * 1e6).round() / 1e6
    }
}

/// One note's draw for `seed`, from -1 to 1: a hash of the seed and the
/// note's place, so each note keeps its own draw however the amount
/// changes (Spec: note-editing R13).
fn draw(seed: u64, place: u32) -> f64 {
    let mut x = seed ^ u64::from(place).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// Which of the three expression lanes show (Spec: note-editing R1, R2): a
/// new editor shows the Velocity lane alone. Always listed in the lanes'
/// order, whatever order they were shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lanes {
    shown: [bool; 3],
    /// Whether the Show/Hide All Expression Editors switch has hidden every
    /// lane: the lanes that were shown come back when it's switched on.
    hidden: bool,
}

impl Default for Lanes {
    fn default() -> Self {
        Self {
            shown: [true, false, false],
            hidden: false,
        }
    }
}

impl Lanes {
    fn index(field: NoteField) -> usize {
        NoteField::LANES
            .iter()
            .position(|lane| *lane == field.lane())
            .unwrap_or(0)
    }

    /// The lanes shown now, from the top.
    #[must_use]
    pub fn shown(&self) -> Vec<NoteField> {
        NoteField::LANES
            .iter()
            .zip(self.shown)
            .filter(|(_, on)| *on && !self.hidden)
            .map(|(lane, _)| *lane)
            .collect()
    }

    /// Whether the switch has hidden every lane.
    #[must_use]
    pub fn all_hidden(&self) -> bool {
        self.hidden
    }

    /// Whether `lane` shows now.
    #[must_use]
    pub fn is_shown(&self, lane: NoteField) -> bool {
        self.shown[Self::index(lane)] && !self.hidden
    }

    /// Shows or hides `lane`. Showing a lane brings the lanes back if the
    /// switch had hidden them.
    pub fn show(&mut self, lane: NoteField, on: bool) {
        self.shown[Self::index(lane)] = on;
        if on {
            self.hidden = false;
        }
    }

    /// The Show/Hide All Expression Editors switch.
    pub fn show_all(&mut self, on: bool) {
        self.hidden = !on;
    }

    /// Hides `lane` and shows in its place the first hidden lane after it,
    /// going round to the first (Swap Lane). Returns the lane that now
    /// shows, or `None` when every lane shows already and nothing changes.
    pub fn swap(&mut self, lane: NoteField) -> Option<NoteField> {
        let at = Self::index(lane);
        let next = (1..3)
            .map(|step| (at + step) % 3)
            .find(|other| !self.shown[*other])?;
        self.shown[at] = false;
        self.shown[next] = true;
        self.hidden = false;
        Some(NoteField::LANES[next])
    }
}

/// The notes on the note clipboard, apart from the Arrangement's clip
/// clipboard: the notes themselves, with their starts counting from the
/// earliest of them, so pasting them keeps their spacing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteClipboard {
    /// The notes, with their starts counting from the earliest of them.
    notes: Vec<Note>,
    /// The groups the notes were in, which pasting them makes anew.
    groups: Vec<NoteGroup>,
}

impl NoteClipboard {
    /// The clipboard holding `notes`, with their starts counting from the
    /// earliest of them.
    fn of(notes: &[Note], midi: &MidiData) -> Self {
        let earliest = notes
            .iter()
            .map(|note| note.start)
            .min()
            .unwrap_or(Ticks::ZERO);
        Self::since(notes, earliest, midi.groups.clone())
    }

    /// The clipboard holding `notes`, with their starts counting from
    /// `earliest`, and the groups they were in.
    fn since(notes: &[Note], earliest: Ticks, groups: Vec<NoteGroup>) -> Self {
        let mut notes: Vec<Note> = notes.to_vec();
        for note in &mut notes {
            note.start -= earliest;
        }
        notes.sort_by_key(|note| (note.start, note.key));
        Self { notes, groups }
    }

    /// Whether it holds no notes.
    fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// The notes to put into a clip at `at`: the copied notes, with the
    /// earliest of them starting at `at`.
    fn at(&self, at: Ticks) -> Vec<Note> {
        self.notes
            .iter()
            .cloned()
            .map(|mut note| {
                note.start += at;
                note
            })
            .collect()
    }

    /// How long the copied notes last, from the earliest of them.
    fn length(&self) -> Ticks {
        self.notes
            .iter()
            .map(|note| note.start + note.length)
            .max()
            .unwrap_or(Ticks::ZERO)
    }

    /// What it holds, as `note.copy` reports it.
    fn state(&self) -> NoteClipboardState {
        NoteClipboardState {
            notes: self.notes.len(),
            length: self.length(),
        }
    }
}

/// What the Clip View shows: whether it's open, Draw Mode, the lane's value,
/// the folded editors, the notes selected in the clip its note editor shows
/// (WP-2.3), its insert marker and time selection, and its Quantize
/// settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipView {
    /// Whether the Clip View shows in the detail area, as the shell's
    /// detail area has it (detail-views R1).
    visible: bool,
    /// Whether Draw Mode is on (B).
    draw_mode: bool,
    /// The active lane, whose controls the footer shows: Velocity,
    /// Chance or Release Velocity (note-editing R3).
    lane: NoteField,
    /// The expression lanes shown under the note editor (R1).
    lanes: Lanes,
    /// Each lane's Randomize amount (R13).
    amounts: RandomizeAmounts,
    /// How many unseeded randomisations have run, which seeds the next.
    draws: u64,
    /// The kind of group made last, which Group Notes makes again
    /// (note-editing R22).
    group_kind: GroupKind,
    /// The clips whose note editor is folded to the rows that hold notes
    /// (F). Saved with the set, like the floating devices' windows.
    folds: ClipFolds,
    /// The notes selected in each clip, by their places in its notes in
    /// order. A clip's selection holds only while the note editor shows it.
    selected: BTreeMap<ClipId, Vec<u32>>,
    /// The foreground clip of several shown, once one was clicked (Spec:
    /// multi-clip-editing R3).
    foreground: Option<ClipId>,
    /// Whether Focus Mode is on (R13). Never saved.
    focus: bool,
    /// The loop bars selected (R10), in no order.
    bars: Vec<ClipId>,
    /// The clip whose insert marker and time selection the editor holds;
    /// they belong to the clip the editor shows.
    marked_clip: Option<ClipId>,
    /// The insert marker, in ticks from the beginning of the clip's
    /// contents.
    insert: Option<Ticks>,
    /// The time selection, in ticks from the beginning of the clip's
    /// contents.
    time: Option<(Ticks, Ticks)>,
    /// The notes on the note clipboard, apart from the Arrangement's clip
    /// clipboard.
    clipboard: NoteClipboard,
    /// What each clip gave a copy across several clips, with the clips
    /// shown and the foreground clip then, which a paste puts back while
    /// they're the same (Spec: multi-clip-editing R12).
    pieces: Vec<(ClipId, NoteClipboard)>,
    copied_with: Option<(Vec<ClipId>, Option<ClipId>)>,
    /// What Quantize uses when its arguments leave them out. View state,
    /// like Draw Mode: not part of the set.
    quantize: QuantizeSettings,
    /// The tab the Clip View shows (L-CLIP-010).
    tab: ClipTab,
    /// The parameter whose envelope the Envelopes tab shows.
    envelope: Option<AutomationTarget>,
    /// The Envelope Editor's insert marker (WP-6.26): the clip it belongs to
    /// and its place, in ticks from the beginning of the clip's contents.
    envelope_marker: Option<(ClipId, Ticks)>,
    /// The breakpoint selected in the Envelope Editor: the clip and the
    /// parameter its envelope is for, and its position.
    envelope_breakpoint: Option<(ClipId, AutomationTarget, Ticks)>,
    /// The clips of the Session slots selected, in track then scene order,
    /// which the Clip View shows until an Arrangement selection replaces
    /// them, and the one selected last.
    session_clips: Vec<ClipId>,
    session_last: Option<ClipId>,
    /// What the Sample Editor has selected and shows for the audio clip the
    /// Clip View shows (WP-4.10).
    sample: SampleView,
    /// Whether the Preview switch is on (Spec: step-recording R1): the
    /// shell keeps it as a preference.
    preview: bool,
    /// The notes step recording just added or lengthened, as the key and
    /// start of each, in `stepped_clip` (Spec: step-recording R5, R6).
    stepped_clip: Option<ClipId>,
    stepped: Vec<(u8, Ticks)>,
}

impl ClipView {
    /// Whether the Clip View shows.
    #[must_use]
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Whether Draw Mode is on (B).
    #[must_use]
    pub fn draw_mode(&self) -> bool {
        self.draw_mode
    }

    /// The active lane: Velocity, Chance or Release Velocity.
    #[must_use]
    pub fn lane(&self) -> NoteField {
        self.lane
    }

    /// Makes `lane` the active lane and shows it (note-editing R3);
    /// velocity deviation is the Velocity lane's.
    pub fn set_lane(&mut self, lane: NoteField) {
        self.lane = lane.lane();
        self.lanes.show(self.lane, true);
    }

    /// The expression lanes shown under the note editor.
    #[must_use]
    pub fn lanes(&self) -> &Lanes {
        &self.lanes
    }

    /// Shows or hides `lane`, as the Lane Selector does. Hiding the active
    /// lane makes the first lane still shown the active one.
    pub fn show_lane(&mut self, lane: NoteField, on: bool) {
        if on {
            self.set_lane(lane);
        } else {
            self.lanes.show(lane, false);
            self.keep_lane_active();
        }
    }

    /// The Show/Hide All Expression Editors switch.
    pub fn show_all_lanes(&mut self, on: bool) {
        self.lanes.show_all(on);
    }

    /// Swap Lane: hides `lane` and shows the next hidden one in its place,
    /// which becomes the active lane when `lane` was.
    pub fn swap_lane(&mut self, lane: NoteField) {
        let lane = lane.lane();
        if let Some(now) = self.lanes.swap(lane)
            && self.lane == lane
        {
            self.lane = now;
        }
    }

    /// Keeps the active lane among the shown ones while any shows.
    fn keep_lane_active(&mut self) {
        if let Some(first) = self.lanes.shown().first()
            && !self.lanes.is_shown(self.lane)
        {
            self.lane = *first;
        }
    }

    /// Each lane's Randomize amount.
    #[must_use]
    pub fn randomize_amounts(&self) -> RandomizeAmounts {
        self.amounts
    }

    /// Sets the Randomize amount of the lane that edits `lane`, held to the
    /// lane's limit.
    pub fn set_randomize_amount(&mut self, lane: NoteField, amount: u8) {
        let amount = amount.min(RandomizeAmounts::limit(lane));
        match lane.lane() {
            NoteField::Chance => self.amounts.chance = amount,
            NoteField::ReleaseVelocity => self.amounts.release_velocity = amount,
            _ => self.amounts.velocity = amount,
        }
    }

    /// The clips whose note editor is folded to the rows that hold notes.
    #[must_use]
    pub fn folds(&self) -> &ClipFolds {
        &self.folds
    }

    /// Replaces the folded editors, as opening a set does.
    pub fn set_folds(&mut self, folds: ClipFolds) {
        self.folds = folds;
    }

    /// Whether the note editor shows only the rows that hold notes for
    /// `clip`.
    #[must_use]
    pub fn is_folded(&self, clip: ClipId) -> bool {
        self.folds.contains(&clip)
    }

    /// Folds the note editor to the rows that hold notes, or unfolds it.
    pub fn set_folded(&mut self, clip: ClipId, folded: bool) {
        if folded {
            self.folds.insert(clip);
        } else {
            self.folds.remove(&clip);
        }
    }

    /// Records whether the Clip View shows: the detail area decides, and
    /// the shell passes it on.
    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Switches Draw Mode on or off.
    pub fn toggle_draw_mode(&mut self) {
        self.draw_mode = !self.draw_mode;
    }

    /// Whether the Preview switch is on (Spec: step-recording R1).
    #[must_use]
    pub fn preview(&self) -> bool {
        self.preview
    }

    /// Switches Preview on or off.
    pub fn set_preview(&mut self, on: bool) {
        self.preview = on;
    }

    /// Forgets that the note at `key` was just recorded: its key went up
    /// (Spec: step-recording R5).
    pub fn step_release(&mut self, key: u8) {
        self.stepped.retain(|(k, _)| *k != key);
    }

    /// The notes step recording just added or lengthened in `clip` whose
    /// keys are still in `held`, as the key and start of each (Spec:
    /// step-recording R5, R6).
    fn just_recorded(&self, clip: ClipId, held: &[HeldKey]) -> Vec<(u8, Ticks)> {
        if self.stepped_clip != Some(clip) {
            return Vec::new();
        }
        self.stepped
            .iter()
            .copied()
            .filter(|(key, _)| held.iter().any(|h| h.key == *key))
            .collect()
    }

    fn set_just_recorded(&mut self, clip: ClipId, just: Vec<(u8, Ticks)>) {
        self.stepped_clip = Some(clip);
        self.stepped = just;
    }

    /// The Quantize settings `note.quantize` uses when its arguments leave
    /// them out.
    #[must_use]
    pub fn quantize(&self) -> QuantizeSettings {
        self.quantize
    }

    /// Changes the Quantize settings: the fields given, keeping the rest.
    pub fn set_quantize(
        &mut self,
        grid: Option<Ticks>,
        target: Option<QuantizeTarget>,
        amount: Option<u8>,
    ) {
        if let Some(grid) = grid {
            self.quantize.grid = Some(grid);
        }
        if let Some(target) = target {
            self.quantize.target = target;
        }
        if let Some(amount) = amount {
            self.quantize.amount = amount;
        }
    }

    /// The tab the Clip View shows.
    #[must_use]
    pub fn tab(&self) -> ClipTab {
        self.tab
    }

    /// Shows a tab of the Clip View.
    pub fn set_tab(&mut self, tab: ClipTab) {
        self.tab = tab;
    }

    /// The parameter whose envelope the Envelopes tab shows.
    #[must_use]
    pub fn envelope(&self) -> Option<AutomationTarget> {
        self.envelope
    }

    /// Chooses the parameter whose envelope the Envelopes tab shows.
    pub fn set_envelope(&mut self, target: Option<AutomationTarget>) {
        if self.envelope != target {
            self.envelope_breakpoint = None;
        }
        self.envelope = target;
    }

    /// The Envelope Editor's insert marker for `clip`, in ticks from the
    /// beginning of its contents: where Left and Right have put it, or the
    /// beginning (WP-6.26).
    #[must_use]
    pub fn envelope_marker(&self, clip: ClipId) -> Ticks {
        self.envelope_marker
            .filter(|(marked, _)| *marked == clip)
            .map_or(Ticks::ZERO, |(_, at)| at)
    }

    /// Puts the Envelope Editor's insert marker for `clip` at `at`.
    pub fn set_envelope_marker(&mut self, clip: ClipId, at: Ticks) {
        self.envelope_marker = Some((clip, at));
    }

    /// The breakpoint selected in the Envelope Editor: the clip, the
    /// parameter and the position (WP-6.26).
    #[must_use]
    pub fn envelope_breakpoint(&self) -> Option<(ClipId, AutomationTarget, Ticks)> {
        self.envelope_breakpoint
    }

    /// Lets go of the selected breakpoint.
    pub fn deselect_envelope_breakpoint(&mut self) {
        self.envelope_breakpoint = None;
    }

    /// Selects a breakpoint of `clip`'s envelope for `target`, or none with
    /// `at` `None`. Selecting puts the insert marker on it.
    pub fn select_envelope_breakpoint(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        at: Option<Ticks>,
    ) {
        self.envelope_breakpoint = at.map(|at| (clip, target, at));
        if let Some(at) = at {
            self.envelope_marker = Some((clip, at));
        }
    }

    /// Shows the Session clip in a slot just selected, in place of the
    /// Arrangement's selection: the Clip View follows the clip most recently
    /// selected, from either view.
    pub fn show_session_clip(&mut self, clip: ClipId) {
        self.show_session_clips(vec![clip], Some(clip));
    }

    /// Shows the clips of the Session slots just selected, in track then
    /// scene order, `last` being the one selected last (R1, R3).
    pub fn show_session_clips(&mut self, clips: Vec<ClipId>, last: Option<ClipId>) {
        self.session_clips = clips;
        self.session_last = last;
        self.foreground = None;
        self.bars.clear();
    }

    /// Goes back to the Arrangement's selection, as selecting a clip there
    /// does.
    pub fn show_arranged_clips(&mut self) {
        self.session_clips.clear();
        self.session_last = None;
        self.foreground = None;
        self.bars.clear();
    }

    /// The MIDI clips the note editor shows, in the loop bars' order (R1,
    /// R2): the selected Session slots' clips, up to eight, every one with
    /// its Loop on, in track then scene order, or the clip selected last
    /// alone when one is unlooped; or else the Arrangement's selected MIDI
    /// clips, the first eight by track and then by time.
    #[must_use]
    pub fn shown(&self, doc: &Document, arrangement: &ArrangementView) -> Vec<ClipId> {
        let midi = |id: &ClipId| {
            doc.clip(*id)
                .is_some_and(|clip| matches!(clip.data, ClipData::Midi(_)))
        };
        let looped = |id: &ClipId| {
            matches!(
                doc.clip(*id).map(|clip| &clip.data),
                Some(ClipData::Midi(data)) if data.looping
            )
        };
        if !self.session_clips.is_empty() {
            let mut clips: Vec<ClipId> = self.session_clips.iter().copied().filter(midi).collect();
            if clips.len() > 1 {
                if clips.iter().all(looped) {
                    clips.truncate(MOST_CLIPS);
                } else {
                    let last = self
                        .session_last
                        .filter(|clip| clips.contains(clip))
                        .or_else(|| clips.last().copied());
                    clips = last.into_iter().collect();
                }
            }
            return clips;
        }
        let mut clips: Vec<ClipId> = arrangement
            .selected_clips()
            .iter()
            .copied()
            .filter(midi)
            .collect();
        // A large selection (Select All) is sorted by one lookup of each
        // clip's track, not a search of the track list.
        let order: std::collections::HashMap<_, _> = doc
            .track_order()
            .into_iter()
            .enumerate()
            .map(|(at, track)| (track, at))
            .collect();
        clips.sort_by_cached_key(|id| {
            (
                doc.clip_track(*id)
                    .and_then(|track| order.get(&track).copied()),
                doc.clip(*id).map(|clip| clip.start),
            )
        });
        clips.truncate(MOST_CLIPS);
        clips
    }

    /// Where the clips shown play on the editor's shared time axis (R4, R5).
    #[must_use]
    pub fn layout(
        &self,
        doc: &Document,
        arrangement: &ArrangementView,
    ) -> crate::multi_clip::Layout {
        crate::multi_clip::layout(doc, &self.shown(doc, arrangement), self.shows_session())
    }

    /// Whether the editor shows Session clips, so their loops line up (R5).
    #[must_use]
    pub fn shows_session(&self) -> bool {
        !self.session_clips.is_empty()
    }

    /// The foreground clip of `shown`: the one clicked last, or else the
    /// one selected last (R3).
    fn pick_foreground(&self, shown: &[ClipId], arrangement: &ArrangementView) -> Option<ClipId> {
        self.foreground
            .filter(|clip| shown.contains(clip))
            .or_else(|| {
                if self.shows_session() {
                    self.session_last.filter(|clip| shown.contains(clip))
                } else {
                    arrangement
                        .selected_clips()
                        .iter()
                        .rev()
                        .copied()
                        .find(|clip| shown.contains(clip))
                }
            })
            .or_else(|| shown.first().copied())
    }

    /// The clip the Clip View shows: the foreground clip of the MIDI clips
    /// shown, or else the Session clip most recently selected, or the
    /// Arrangement's first selected clip, of either kind.
    #[must_use]
    pub fn shown_any_clip(&self, doc: &Document, arrangement: &ArrangementView) -> Option<ClipId> {
        let exists = |id: &ClipId| doc.clip(*id).is_some();
        self.pick_foreground(&self.shown(doc, arrangement), arrangement)
            .or_else(|| {
                self.session_last
                    .or_else(|| self.session_clips.first().copied())
                    .filter(exists)
            })
            .or_else(|| {
                if self.shows_session() {
                    None
                } else {
                    arrangement.selected_clips().iter().copied().find(exists)
                }
            })
    }

    /// The clip the Clip View shows when it holds MIDI notes: the
    /// foreground clip (R3).
    #[must_use]
    pub fn shown_clip(&self, doc: &Document, arrangement: &ArrangementView) -> Option<ClipId> {
        self.shown_any_clip(doc, arrangement).filter(|id| {
            doc.clip(*id)
                .is_some_and(|clip| matches!(clip.data, ClipData::Midi(_)))
        })
    }

    /// The clip the Clip View shows when it is an audio clip, whose editor
    /// is the Sample Editor (WP-4.10).
    #[must_use]
    pub fn shown_audio_clip(
        &self,
        doc: &Document,
        arrangement: &ArrangementView,
    ) -> Option<ClipId> {
        self.shown_any_clip(doc, arrangement).filter(|id| {
            doc.clip(*id)
                .is_some_and(|clip| matches!(clip.data, ClipData::Audio(_)))
        })
    }

    /// What the Sample Editor has selected and shows.
    #[must_use]
    pub fn sample(&self) -> &SampleView {
        &self.sample
    }

    /// The Sample Editor's state, ready for `clip`, which it starts afresh
    /// for if it isn't the clip it was last for.
    pub(crate) fn sample_mut(
        &mut self,
        clip: ClipId,
        audio: &bl_model::AudioData,
    ) -> &mut SampleView {
        self.sample.bind_to(clip, audio);
        &mut self.sample
    }

    /// Lets the Sample Editor follow the set's changes.
    pub(crate) fn sync_sample(&mut self, doc: &Document) {
        self.sample.sync(doc);
    }

    /// Lets the Sample Editor follow playback again, or stop until the song
    /// starts or stops.
    pub(crate) fn sample_follow(&mut self, paused: bool) {
        if paused {
            self.sample.pause_follow();
        } else {
            self.sample.resume_follow();
        }
    }

    /// The shown clip when it sits in a Session slot, which is when the
    /// Launch Properties panel shows beside the editor.
    #[must_use]
    pub fn shown_session_clip(
        &self,
        doc: &Document,
        arrangement: &ArrangementView,
    ) -> Option<ClipId> {
        self.shown_any_clip(doc, arrangement)
            .filter(|clip| self.session_clips.contains(clip))
    }

    /// Whether Focus Mode is on (R13). It acts only while the editor shows
    /// several clips: see [`ClipView::focus_active`].
    #[must_use]
    pub fn focus_mode(&self) -> bool {
        self.focus
    }

    /// Switches Focus Mode. With it on only one loop bar can be selected, so
    /// the selection of several is set aside (R14).
    pub fn set_focus_mode(&mut self, on: bool) {
        self.focus = on;
    }

    /// Whether Focus Mode is acting now: it's on and the editor shows
    /// several clips (R13).
    #[must_use]
    pub fn focus_active(&self, doc: &Document, arrangement: &ArrangementView) -> bool {
        self.focus && self.shown(doc, arrangement).len() > 1
    }

    /// The clips the edits reach: every clip shown, or with Focus Mode on
    /// the active clip alone (R7, R15).
    #[must_use]
    pub fn active_clips(&self, doc: &Document, arrangement: &ArrangementView) -> Vec<ClipId> {
        let shown = self.shown(doc, arrangement);
        if self.focus && shown.len() > 1 {
            self.pick_foreground(&shown, arrangement)
                .into_iter()
                .collect()
        } else {
            shown
        }
    }

    /// Makes `clip`, one of those shown, the foreground clip (R3).
    pub fn set_foreground(&mut self, clip: ClipId) {
        self.foreground = Some(clip);
    }

    /// The loop bars selected: with Focus Mode on, only the foreground
    /// clip's, the rest being set aside (R14).
    #[must_use]
    pub fn loop_bars(&self, doc: &Document, arrangement: &ArrangementView) -> Vec<ClipId> {
        let shown = self.shown(doc, arrangement);
        let mut bars: Vec<ClipId> = shown
            .iter()
            .copied()
            .filter(|clip| self.bars.contains(clip))
            .collect();
        if self.focus && shown.len() > 1 {
            bars.retain(|clip| self.pick_foreground(&shown, arrangement) == Some(*clip));
        }
        bars
    }

    /// Selects the loop bars of `clips` (R10).
    pub fn set_loop_bars(&mut self, clips: Vec<ClipId>) {
        self.bars = clips;
    }

    /// The selected notes' places in the foreground clip's notes in order,
    /// empty when the selection belongs to other clips.
    #[must_use]
    pub fn selected_notes(&self, doc: &Document, arrangement: &ArrangementView) -> &[u32] {
        self.shown_clip(doc, arrangement)
            .map_or(&[][..], |clip| self.selection_of(clip))
    }

    /// The notes selected in `clip`, by their places in its notes in order.
    #[must_use]
    pub fn selection_of(&self, clip: ClipId) -> &[u32] {
        self.selected.get(&clip).map_or(&[][..], Vec::as_slice)
    }

    /// Whether the notes selected in the shown clip are all deactivated, so
    /// Deactivate Note(s) reads Activate Note(s) (Spec: note-editing R18).
    #[must_use]
    pub fn selection_deactivated(&self, doc: &Document, arrangement: &ArrangementView) -> bool {
        let selected = self.selected_notes(doc, arrangement);
        let Some(ClipData::Midi(midi)) = self
            .shown_clip(doc, arrangement)
            .and_then(|clip| doc.clip(clip))
            .map(|clip| &clip.data)
        else {
            return false;
        };
        let ordered = notes_in_order(midi);
        !selected.is_empty()
            && selected
                .iter()
                .all(|place| ordered.get(*place as usize).is_some_and(|note| note.mute))
    }

    /// Which kinds of probability group the notes selected in the shown clip
    /// belong to: Play All, Play One (Spec: note-editing R23, R24).
    #[must_use]
    pub fn selection_group_kinds(
        &self,
        doc: &Document,
        arrangement: &ArrangementView,
    ) -> (bool, bool) {
        let selected = self.selected_notes(doc, arrangement);
        let Some(ClipData::Midi(midi)) = self
            .shown_clip(doc, arrangement)
            .and_then(|clip| doc.clip(clip))
            .map(|clip| &clip.data)
        else {
            return (false, false);
        };
        let ordered = notes_in_order(midi);
        let mut kinds = (false, false);
        for group in selected
            .iter()
            .filter_map(|place| ordered.get(*place as usize)?.group)
        {
            match midi.group_kind(group) {
                Some(GroupKind::PlayAll) => kinds.0 = true,
                Some(GroupKind::PlayOne) => kinds.1 = true,
                None => {}
            }
        }
        kinds
    }

    /// What the Status Bar says of the selected notes' probability groups:
    /// their kind, with an asterisk when they belong to groups of both
    /// (Spec: note-editing R24).
    #[must_use]
    pub fn group_status(&self, doc: &Document, arrangement: &ArrangementView) -> Option<String> {
        match self.selection_group_kinds(doc, arrangement) {
            (false, false) => None,
            (true, false) => Some("Probability group: Play All".to_owned()),
            (false, true) => Some("Probability group: Play One".to_owned()),
            (true, true) => Some("Probability group: Play All, Play One*".to_owned()),
        }
    }

    /// Selects notes of `clip`, by their places in its notes in order, and
    /// no notes of any other clip.
    pub fn select(&mut self, clip: ClipId, places: Vec<u32>) {
        self.selected.clear();
        self.selected.insert(clip, places);
    }

    /// Selects the notes of several clips at once, and no others (R7).
    pub fn select_in(&mut self, selections: Vec<(ClipId, Vec<u32>)>) {
        self.selected = selections.into_iter().collect();
    }

    /// The insert marker, in ticks from the beginning of the shown clip's
    /// contents, when the editor holds one.
    #[must_use]
    pub fn insert_marker(&self, doc: &Document, arrangement: &ArrangementView) -> Option<Ticks> {
        if self.marked_clip == self.shown_clip(doc, arrangement) {
            self.insert
        } else {
            None
        }
    }

    /// The time selection's start and end, in ticks from the beginning of
    /// the shown clip's contents, when the editor holds one.
    #[must_use]
    pub fn time_selection(
        &self,
        doc: &Document,
        arrangement: &ArrangementView,
    ) -> Option<(Ticks, Ticks)> {
        if self.marked_clip == self.shown_clip(doc, arrangement) {
            self.time
        } else {
            None
        }
    }

    /// Puts the insert marker at `at`, in ticks from the beginning of the
    /// clip's contents, and takes the time selection away: moving the
    /// marker clears it (piano-roll R24).
    pub fn set_insert_marker(&mut self, clip: ClipId, at: Option<Ticks>) {
        self.marked_clip = Some(clip);
        self.insert = at;
        if at.is_some() {
            self.time = None;
        }
    }

    /// Makes the time selection span `from` to `to`, in ticks from the
    /// beginning of the clip's contents, or takes it away.
    pub fn set_time_selection(&mut self, clip: ClipId, span: Option<(Ticks, Ticks)>) {
        self.marked_clip = Some(clip);
        self.time = span;
    }

    /// The notes on the note clipboard, apart from the Arrangement's clip
    /// clipboard.
    #[must_use]
    pub fn note_clipboard(&self) -> &NoteClipboard {
        &self.clipboard
    }

    /// Puts the notes copied from several clips on the note clipboard, kept
    /// apart by clip too (R12). `shown` and `foreground` are the clips shown
    /// and the foreground clip at the copy.
    fn set_clipboard_pieces(
        &mut self,
        pieces: Vec<(ClipId, Vec<Note>, Vec<NoteGroup>)>,
        shown: Vec<ClipId>,
        foreground: Option<ClipId>,
    ) {
        // Each clip numbers its groups apart, so the notes all together
        // take new group IDs, one for each clip's group.
        let mut all = Vec::new();
        let mut groups = Vec::new();
        for (_, notes, kinds) in &pieces {
            let first = groups
                .iter()
                .map(|group: &NoteGroup| group.id.saturating_add(1))
                .max()
                .unwrap_or(0);
            for group in kinds {
                let id = first.saturating_add(group.id);
                groups.push(NoteGroup {
                    id,
                    kind: group.kind,
                });
            }
            all.extend(notes.iter().cloned().map(|mut note| {
                note.group = note.group.map(|id| first.saturating_add(id));
                note
            }));
        }
        let earliest = all
            .iter()
            .map(|note| note.start)
            .min()
            .unwrap_or(Ticks::ZERO);
        self.clipboard = NoteClipboard::since(&all, earliest, groups);
        self.pieces = pieces
            .into_iter()
            .map(|(clip, notes, groups)| (clip, NoteClipboard::since(&notes, earliest, groups)))
            .collect();
        self.copied_with = Some((shown, foreground));
    }

    /// What the view shows, as a command reports it.
    #[must_use]
    pub fn state(&self, doc: &Document, arrangement: &ArrangementView) -> ClipViewState {
        let clip = self.shown_clip(doc, arrangement);
        let shown = self.shown(doc, arrangement);
        let any_shown = self.shown_any_clip(doc, arrangement);
        ClipViewState {
            visible: self.visible,
            draw_mode: self.draw_mode,
            clip,
            clip_selections: shown
                .iter()
                .filter(|clip| !self.selection_of(**clip).is_empty())
                .map(|clip| ClipEdit {
                    clip: *clip,
                    notes: Vec::new(),
                    selected: self.selection_of(*clip).to_vec(),
                })
                .collect(),
            loop_bars: self.loop_bars(doc, arrangement),
            focus_mode: self.focus,
            clips: shown,
            selected: self.selected_notes(doc, arrangement).to_vec(),
            insert_marker: self.insert_marker(doc, arrangement),
            time_selection: self.time_selection(doc, arrangement),
            lane: self.lane,
            lanes: self.lanes.shown(),
            lanes_hidden: self.lanes.all_hidden(),
            randomize: self.amounts,
            folded: clip.is_some_and(|clip| self.is_folded(clip)),
            tab: self.tab,
            preview: self.preview,
            envelope: self.envelope,
            envelope_marker: any_shown
                .and_then(|clip| self.envelope_marker.filter(|(marked, _)| *marked == clip))
                .map(|(_, at)| at),
            envelope_breakpoint: self
                .envelope_breakpoint
                .filter(|(selected, target, _)| {
                    Some(*selected) == any_shown && Some(*target) == self.envelope
                })
                .map(|(_, _, at)| at),
        }
    }

    /// Forgets a note selection that a replaced set can't hold any more.
    pub(crate) fn forget_selection(&mut self) {
        self.selected.clear();
        self.foreground = None;
        self.bars.clear();
        self.marked_clip = None;
        self.insert = None;
        self.time = None;
        self.session_clips.clear();
        self.session_last = None;
        self.envelope = None;
        self.envelope_marker = None;
        self.envelope_breakpoint = None;
        self.stepped.clear();
        self.sample = SampleView::default();
    }
}

/// The clip a command acts on: the one given, or the shown one, with its
/// MIDI data.
fn clip_and_data(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: Option<ClipId>,
) -> Result<(ClipId, MidiData), EditFailure> {
    let clip = clip
        .or_else(|| view.shown_clip(doc, arrangement))
        .ok_or_else(|| unavailable("no MIDI clip is shown in the Clip View"))?;
    let value = doc
        .clip(clip)
        .ok_or_else(|| invalid(format!("no clip has the ID {}", clip.0)))?;
    match &value.data {
        ClipData::Midi(midi) => Ok((clip, midi.clone())),
        ClipData::Audio(_) => Err(refused("the clip isn't a MIDI clip")),
    }
}

/// The notes to act on: those given by place, or the selected ones. Each
/// must exist; a place listed twice counts once.
fn places_or_selected(
    view: &ClipView,
    clip: ClipId,
    notes: Option<Vec<u32>>,
    count: usize,
) -> Result<Vec<u32>, EditFailure> {
    let given = notes.is_some();
    let places = notes.unwrap_or_else(|| view.selection_of(clip).to_vec());
    if places.is_empty() {
        return Err(if given {
            invalid("`notes` is empty")
        } else {
            unavailable("no notes are selected")
        });
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(places.len());
    for place in places {
        if place as usize >= count {
            return Err(invalid(format!(
                "clip {} has no note at place {place}",
                clip.0
            )));
        }
        if seen.insert(place) {
            out.push(place);
        }
    }
    Ok(out)
}

/// Replaces a MIDI clip's notes, keeping its markers, loop and other data.
fn set_notes(
    editor: &mut Editor,
    clip: ClipId,
    midi: &MidiData,
    notes: Vec<Note>,
) -> Result<(), EditFailure> {
    set_notes_in(editor, clip, midi.clone(), notes)
}

/// [`set_notes`] with `data` as the clip's other data, changed by the caller.
/// A group left with fewer than two notes ends (Spec: note-editing R22).
fn set_notes_in(
    editor: &mut Editor,
    clip: ClipId,
    mut data: MidiData,
    notes: Vec<Note>,
) -> Result<(), EditFailure> {
    data.notes = notes.into_iter().collect();
    data.tidy_groups();
    editor
        .apply(Command::SetClipData {
            clip,
            data: ClipData::Midi(data),
        })
        .map(|_| ())?;
    Ok(())
}

/// Gives the chance of each of `places`' notes to the rest of its group,
/// the first of a group named winning (Spec: note-editing R25).
fn share_chance(notes: &mut [Note], places: &[u32]) {
    let mut done = BTreeSet::new();
    for place in places {
        let Some(group) = notes[*place as usize].group else {
            continue;
        };
        if done.insert(group) {
            let chance = notes[*place as usize].probability;
            for note in notes.iter_mut().filter(|n| n.group == Some(group)) {
                note.probability = chance;
            }
        }
    }
}

/// Puts `copies` in groups of their own, one for each group they came from,
/// of the kinds `kinds` give (Spec: note-editing R31).
pub(crate) fn regroup(data: &mut MidiData, copies: &mut [Note], kinds: &[NoteGroup]) {
    let mut made: Vec<(u32, u32)> = Vec::new();
    for note in copies {
        let Some(old) = note.group else { continue };
        let Some(kind) = kinds.iter().find(|g| g.id == old).map(|g| g.kind) else {
            note.group = None;
            continue;
        };
        let new = match made.iter().find(|(from, _)| *from == old) {
            Some((_, new)) => *new,
            None => {
                let id = data.new_group_id();
                data.groups.push(NoteGroup { id, kind });
                made.push((old, id));
                id
            }
        };
        note.group = Some(new);
    }
}

/// The place of `note` among `ordered`: the first equal note, which is
/// the note itself when no identical one is already there.
fn place_of(ordered: &[Note], note: &Note) -> u32 {
    let place = ordered.iter().position(|other| other == note).unwrap_or(0);
    u32::try_from(place).unwrap_or(u32::MAX)
}

/// How many parts a note has after one press of Cmd with Up or Down, from
/// `parts`: one more or fewer, or the next or previous power of two, from 1
/// to [`MOST_PARTS`] (note-editing R36).
#[must_use]
pub fn step_parts(parts: u32, step: ChopStep) -> u32 {
    let power = |up: bool| {
        let powers = (0..=MOST_PARTS.ilog2()).map(|k| 1u32 << k);
        if up {
            powers
                .into_iter()
                .find(|p| *p > parts)
                .unwrap_or(MOST_PARTS)
        } else {
            powers.into_iter().rev().find(|p| *p < parts).unwrap_or(1)
        }
    };
    match step {
        ChopStep::More => (parts + 1).min(MOST_PARTS),
        ChopStep::Fewer => parts.saturating_sub(1).max(1),
        ChopStep::NextPower => power(true),
        ChopStep::PreviousPower => power(false),
    }
}

/// A note on its way through an edit that cuts, joins or reshapes notes:
/// whether it was selected, and whether the edit made it.
struct Carried {
    note: Note,
    selected: bool,
    touched: bool,
}

impl Carried {
    fn kept(note: &Note, selected: bool) -> Self {
        Self {
            note: note.clone(),
            selected,
            touched: false,
        }
    }

    fn made(note: Note, selected: bool) -> Self {
        Self {
            note,
            selected,
            touched: true,
        }
    }
}

/// Stores `carried` as the clip's notes in the editor's order, and returns
/// the places of the notes the edit made and of the ones selected.
fn store(
    editor: &mut Editor,
    clip: ClipId,
    midi: &MidiData,
    mut carried: Vec<Carried>,
) -> Result<(Vec<u32>, Vec<u32>), EditFailure> {
    carried.sort_by_key(|carried| (carried.note.start, carried.note.key));
    let places = |wanted: fn(&Carried) -> bool| -> Vec<u32> {
        carried
            .iter()
            .enumerate()
            .filter(|(_, carried)| wanted(carried))
            .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
            .collect()
    };
    let made = places(|carried| carried.touched);
    let selected = places(|carried| carried.selected);
    set_notes(
        editor,
        clip,
        midi,
        carried.into_iter().map(|carried| carried.note).collect(),
    )?;
    Ok((made, selected))
}

/// What an edit that carries notes did, and the selection it leaves.
fn carried_result(
    view: &mut ClipView,
    clip: ClipId,
    (made, selected): (Vec<u32>, Vec<u32>),
    recorded: bool,
) -> Value {
    view.select(clip, selected.clone());
    to_value(&NoteEditResult {
        notes: made,
        selected,
        undo_step: recorded,
        clips: Vec::new(),
    })
}

/// Where each cut of `note.split` falls: the ticks given for every note
/// named, or each note's own, as the places in the clip's notes in order.
fn split_cuts(
    args: SplitNotesArgs,
    view: &ClipView,
    doc: &Document,
    arrangement: &ArrangementView,
    clip: ClipId,
    count: usize,
) -> Result<Vec<(u32, Ticks)>, EditFailure> {
    let SplitNotesArgs {
        clip: _,
        notes,
        at,
        cuts,
    } = args;
    if let Some(cuts) = cuts {
        if notes.is_some() || at.is_some() {
            return Err(invalid("give `cuts`, or `notes` and `at`, not both"));
        }
        if let Some(cut) = cuts.iter().find(|cut| cut.note as usize >= count) {
            return Err(invalid(format!(
                "clip {} has no note at place {}",
                clip.0, cut.note
            )));
        }
        return Ok(cuts.iter().map(|cut| (cut.note, cut.at)).collect());
    }
    let selected = view.selection_of(clip);
    let places: Vec<u32> = match notes {
        Some(notes) => places_or_selected(view, clip, Some(notes), count)?,
        None if !selected.is_empty() => selected.to_vec(),
        None => (0..u32::try_from(count).unwrap_or(u32::MAX)).collect(),
    };
    // Spec: note-editing R35. Without ticks, the time selection's edges, or
    // the insert marker.
    let ticks = match at {
        Some(at) if at.is_empty() => return Err(invalid("`at` is empty")),
        Some(at) => at,
        None => match view.time_selection(doc, arrangement) {
            Some((from, to)) => vec![from, to],
            None => vec![
                view.insert_marker(doc, arrangement)
                    .ok_or_else(|| unavailable("there is no insert marker to split at"))?,
            ],
        },
    };
    Ok(places
        .iter()
        .flat_map(|place| ticks.iter().map(|at| (*place, *at)))
        .collect())
}

/// Moves the breakpoints of a clip's linked envelopes that lie in `range`
/// by `map` (note-editing R46). Each moved breakpoint is parked beyond
/// every position first, so none meets another on the way, and one that
/// lands on a breakpoint that stays replaces it.
fn stretch_envelopes(
    editor: &mut Editor,
    clip: ClipId,
    range: (Ticks, Ticks),
    map: &Mapping,
) -> Result<(), EditFailure> {
    let envelopes = editor
        .document()
        .clip(clip)
        .map(|clip| clip.envelopes.clone())
        .unwrap_or_default();
    for envelope in envelopes.iter().filter(|envelope| envelope.linked) {
        let (target, kind) = (envelope.target, envelope.kind);
        let points: Vec<Ticks> = envelope.envelope.points().map(|point| point.at).collect();
        let moved: Vec<(Ticks, Ticks)> = points
            .iter()
            .filter(|at| **at >= range.0 && **at <= range.1)
            .map(|at| (*at, map.at(*at)))
            .filter(|(at, to)| at != to)
            .collect();
        if moved.iter().any(|(_, to)| to.is_negative()) {
            return Err(refused(
                "a breakpoint would move before the clip's contents",
            ));
        }
        let top = points
            .iter()
            .chain(moved.iter().map(|(_, to)| to))
            .max()
            .copied()
            .unwrap_or(Ticks::ZERO)
            + Ticks(1);
        for (index, (at, _)) in moved.iter().enumerate() {
            let to = top + Ticks(i64::try_from(index).unwrap_or(0));
            editor.apply(Command::MoveClipEnvelopeBreakpoint {
                clip,
                target,
                kind,
                at: *at,
                to,
                value: None,
            })?;
        }
        for (index, (_, to)) in moved.iter().enumerate() {
            let parked = top + Ticks(i64::try_from(index).unwrap_or(0));
            let taken = editor
                .document()
                .clip(clip)
                .and_then(|value| {
                    value
                        .envelopes
                        .iter()
                        .find(|e| e.target == target && e.kind == kind)
                        .and_then(|e| e.envelope.point(*to))
                })
                .is_some();
            if taken {
                editor.apply(Command::DeleteClipEnvelopeBreakpoint {
                    clip,
                    target,
                    kind,
                    at: *to,
                })?;
            }
            editor.apply(Command::MoveClipEnvelopeBreakpoint {
                clip,
                target,
                kind,
                at: parked,
                to: *to,
                value: None,
            })?;
        }
    }
    Ok(())
}

/// The name of the undo step a note command makes.
#[must_use]
pub fn label(id: &str) -> &'static str {
    match id {
        ids::ADD_NOTE => "Add note",
        ids::DELETE_NOTES => "Delete notes",
        ids::MOVE_NOTES => "Move notes",
        ids::RESIZE_NOTES => "Resize notes",
        ids::QUANTIZE_NOTES => "Quantize notes",
        ids::SET_VALUE => "Set note values",
        ids::RAMP_VALUE => "Ramp note values",
        ids::SPLIT => "Split Notes",
        ids::CHOP => "Chop Notes",
        ids::JOIN => "Join Notes",
        ids::FIT_TO_TIME => "Fit to Time Range",
        ids::STRETCH => "Stretch Notes",
        ids::ADJUST_VALUE => "Adjust note values",
        ids::RANDOMIZE => "Randomize note values",
        ids::DEACTIVATE => "Deactivate notes",
        ids::GROUP => "Group Notes",
        ids::UNGROUP => "Ungroup Notes",
        ids::STEP_RECORD => "Step Record",
        _ => "Select notes",
    }
}

/// What one step of step recording did to a clip's notes (Spec:
/// step-recording R4 to R6).
struct Stepped {
    /// The clip's notes in order afterwards.
    notes: Vec<Note>,
    /// The notes just recorded afterwards, as the key and start of each.
    just: Vec<(u8, Ticks)>,
    /// The insert marker afterwards.
    marker: Ticks,
    added: u32,
    lengthened: u32,
    shortened: u32,
    removed: u32,
}

impl Stepped {
    fn changed(&self) -> bool {
        self.added + self.lengthened + self.shortened + self.removed > 0
    }
}

/// Steps forward or `back` from the insert marker `at` with `held` keys
/// down, `just` being the notes the last step added or lengthened whose keys
/// are still down. Forward, those notes are lengthened by a grid `step` and
/// the other held keys are added at `at`, one step long; back, those notes
/// are shortened a step, and removed when none is left. Nothing held leaves
/// the notes as they are and moves the marker, a rest. Nothing goes past the
/// clip's span `(from, to)`.
fn step_notes(
    mut notes: Vec<Note>,
    step: Ticks,
    (from, to): (Ticks, Ticks),
    at: Ticks,
    just: &[(u8, Ticks)],
    held: &[HeldKey],
    back: bool,
) -> Stepped {
    let mut out = Stepped {
        notes: Vec::new(),
        just: Vec::new(),
        marker: at,
        added: 0,
        lengthened: 0,
        shortened: 0,
        removed: 0,
    };
    let find = |notes: &[Note], (key, start): (u8, Ticks)| {
        notes
            .iter()
            .position(|note| note.key == key && note.start == start)
    };
    if back {
        for &recorded in just {
            let Some(place) = find(&notes, recorded) else {
                continue;
            };
            if notes[place].length > step {
                notes[place].length -= step;
                out.shortened += 1;
                out.just.push(recorded);
            } else {
                notes.remove(place);
                out.removed += 1;
            }
        }
        out.marker = (at - step).max(from);
    } else {
        for &recorded in just {
            let Some(place) = find(&notes, recorded) else {
                continue;
            };
            let note = &mut notes[place];
            let end = (note.start + note.length + step).min(to);
            if end > note.start + note.length {
                note.length = end - note.start;
                out.lengthened += 1;
            }
            out.just.push(recorded);
        }
        for key in held
            .iter()
            .filter(|h| !just.iter().any(|(key, _)| *key == h.key))
        {
            if at < to {
                notes.push(Note::new(key.key, at, step.min(to - at), key.velocity));
                out.added += 1;
                out.just.push((key.key, at));
            }
        }
        out.marker = (at + step).min(to);
    }
    notes.sort_by_key(|note| (note.start, note.key));
    out.notes = notes;
    out
}

/// A tick of a MIDI clip's contents, from the number an argument gives.
fn tick_of(value: f64) -> Result<Ticks, EditFailure> {
    if !value.is_finite() {
        return Err(invalid("a tick must be a whole number of ticks"));
    }
    Ok(Ticks(value.round() as i64))
}

/// Runs the Clip View's view commands for a MIDI clip: its insert marker
/// and its time selection (`clip_view.set_insert_marker` and
/// `clip_view.select_time`, which the Sample Editor's editor shares). The
/// marker's place and the selection are ticks of the clip's contents
/// (piano-roll R23, R24, R31).
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run_view(
    id: &str,
    args: Value,
    doc: &Document,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    match id {
        sample_ids::SET_INSERT_MARKER => {
            let SetInsertMarkerArgs { clip, time, beat } = parse(args)?;
            if time.is_some() && beat.is_some() {
                return Err(invalid("give `time` or `beat`, not both"));
            }
            if time.is_some() {
                return Err(invalid(
                    "a MIDI clip's insert marker is a tick of its contents: give `beat`",
                ));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let length = doc.clip(clip).map_or(Ticks::ZERO, |clip| clip.length);
            let (from, to) = span(&midi, length);
            // The marker stays inside the clip's span (piano-roll R23).
            let at = beat.map(|beat| beat.clamp(from, to));
            view.set_insert_marker(clip, at);
            Ok(to_value(&view.state(doc, arrangement)))
        }
        sample_ids::SELECT_TIME => {
            let SelectTimeArgs {
                clip,
                start,
                end,
                ticks: _,
            } = parse(args)?;
            let (clip, _) = clip_and_data(doc, view, arrangement, clip)?;
            let span = match (start, end) {
                (None, None) => None,
                (Some(start), Some(end)) => {
                    if start > end {
                        return Err(invalid("`start` must be before `end`"));
                    }
                    Some((tick_of(start)?, tick_of(end)?))
                }
                _ => return Err(invalid("give both `start` and `end`, or neither")),
            };
            view.set_time_selection(clip, span);
            Ok(to_value(&view.state(doc, arrangement)))
        }
        other => Err(invalid(format!("`{other}` isn't a Clip View command"))),
    }
}

/// Whether Split Note(s) would split a note now: one the insert marker or
/// an edge of the time selection crosses, among the selected notes or with
/// none selected all of them (note-editing R35). The editor's menu shows
/// the item only while this holds.
#[must_use]
pub fn would_split(doc: &Document, view: &ClipView, arrangement: &ArrangementView) -> bool {
    let Ok((clip, midi)) = clip_and_data(doc, view, arrangement, None) else {
        return false;
    };
    let ordered = notes_in_order(&midi);
    let Ok(cuts) = split_cuts(
        SplitNotesArgs::default(),
        view,
        doc,
        arrangement,
        clip,
        ordered.len(),
    ) else {
        return false;
    };
    cuts.iter().any(|(place, at)| {
        ordered
            .get(*place as usize)
            .is_some_and(|note| *at > note.start && *at < note.start + note.length)
    })
}

/// What Split Clip runs (note-editing R41): the `arrangement.split`
/// arguments that cut the Arrangement clip the editor shows at the time
/// selection's edges that fall strictly inside it, and the clip's span
/// from the time selection, in song ticks, whose parts the split selects.
///
/// # Errors
///
/// [`EditFailure`] for a Session clip, a missing time selection, or edges
/// that all fall outside the clip.
pub fn split_clip_plan(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
) -> Result<(Value, ClipId, (Ticks, Ticks)), EditFailure> {
    let clip = view
        .shown_clip(doc, arrangement)
        .ok_or_else(|| unavailable("no MIDI clip is shown in the Clip View"))?;
    let value = doc
        .clip(clip)
        .ok_or_else(|| invalid(format!("no clip has the ID {}", clip.0)))?;
    let arranged = doc
        .clip_track(clip)
        .and_then(|track| doc.track(track))
        .is_some_and(|track| track.clips().any(|other| other.id == clip));
    if !arranged {
        return Err(refused(
            "a Session clip can't be split: it isn't on the Arrangement",
        ));
    }
    let ClipData::Midi(midi) = &value.data else {
        return Err(refused("the clip isn't a MIDI clip"));
    };
    let (from, to) = view
        .time_selection(doc, arrangement)
        .ok_or_else(|| unavailable("there is no time selection to split at"))?;
    // Contents tick `t` plays at the clip's start once the Start marker
    // is taken off it.
    let song = |tick: Ticks| value.start + (tick - midi.start);
    let (from, to) = (song(from), song(to));
    let at: Vec<Position> = [from, to]
        .into_iter()
        .filter(|at| *at > value.start && *at < value.end())
        .map(Position::from)
        .collect();
    if at.is_empty() {
        return Err(refused(
            "the time selection's edges don't fall inside the clip",
        ));
    }
    let args = SplitArgs {
        clips: Some(vec![clip]),
        at: Some(at),
        ..SplitArgs::default()
    };
    Ok((
        to_value(&args),
        clip,
        (from.max(value.start), to.min(value.end())),
    ))
}

/// Runs a note command, as `Shell::run` does: an edit returns a
/// [`NoteEditResult`], and a selection the view's state.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per command keeps each one's behaviour in sight"
)]
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    let before = editor.document().clone();
    let doc = &before;
    if let Some(targets) = edit_targets(id, &args, doc, view, arrangement)? {
        return run_across(id, &args, targets, editor, view, arrangement);
    }
    match id {
        ids::SELECT_NOTES => {
            let SelectNotesArgs {
                clip,
                notes,
                clips,
                mode,
                select,
                add,
            } = parse(args)?;
            if clips.is_some() && (clip.is_some() || !notes.is_empty()) {
                return Err(invalid("give `clips`, or `clip` and `notes`, not both"));
            }
            // Spec: multi-clip-editing R18. The clips and notes named; or
            // with `select` and none named, every clip the edits reach;
            // else the one clip.
            let named: Vec<ClipNotes> = match clips {
                Some(clips) if clips.is_empty() => return Err(invalid("`clips` is empty")),
                Some(clips) => clips,
                None if clip.is_none() && select.is_some() => {
                    let active = view.active_clips(doc, arrangement);
                    if active.is_empty() {
                        return Err(unavailable("no MIDI clip is shown in the Clip View"));
                    }
                    active
                        .into_iter()
                        .map(|clip| ClipNotes { clip, notes: None })
                        .collect()
                }
                None if clip.is_none() && view.shown_clip(doc, arrangement).is_none() => {
                    return Err(unavailable("no MIDI clip is shown in the Clip View"));
                }
                None => {
                    let (clip, _) = clip_and_data(doc, view, arrangement, clip)?;
                    vec![ClipNotes {
                        clip,
                        notes: Some(notes),
                    }]
                }
            };
            let mut chosen = Vec::with_capacity(named.len());
            let mut spans = Vec::with_capacity(named.len());
            for ClipNotes { clip, notes } in named {
                let (clip, midi) = clip_and_data(doc, view, arrangement, Some(clip))?;
                if chosen.iter().any(|(other, _)| *other == clip) {
                    return Err(invalid(format!("clip {} is named twice", clip.0)));
                }
                let ordered = notes_in_order(&midi);
                let notes = notes.unwrap_or_default();
                for place in &notes {
                    if *place as usize >= ordered.len() {
                        return Err(invalid(format!(
                            "clip {} has no note at place {place}",
                            clip.0
                        )));
                    }
                }
                let mut selected = view.selection_of(clip).to_vec();
                match select {
                    // The notes the keyboard and the piano ruler reach
                    // (piano-roll R25, R26).
                    Some(what) => {
                        let length = doc.clip(clip).map_or(Ticks::ZERO, |clip| clip.length);
                        let insert = view
                            .insert_marker(doc, arrangement)
                            .filter(|_| view.shown_clip(doc, arrangement) == Some(clip));
                        selected =
                            select_by(what, &ordered, span(&midi, length), insert, &selected, add);
                    }
                    None => match mode {
                        NoteSelectMode::Replace => selected = notes,
                        NoteSelectMode::Toggle => {
                            for place in notes {
                                match selected.iter().position(|p| *p == place) {
                                    Some(at) => {
                                        selected.remove(at);
                                    }
                                    None => selected.push(place),
                                }
                            }
                        }
                    },
                }
                spans.push((clip, span_of(doc, clip, &selected)));
                chosen.push((clip, selected));
            }
            // A replacing selection lets go of the notes in every other clip.
            let keeps = match select {
                Some(_) => add,
                None => mode == NoteSelectMode::Toggle,
            };
            let mut all: Vec<(ClipId, Vec<u32>)> = if keeps {
                view.shown(doc, arrangement)
                    .into_iter()
                    .filter(|clip| chosen.iter().all(|(other, _)| other != clip))
                    .map(|clip| (clip, view.selection_of(clip).to_vec()))
                    .collect()
            } else {
                Vec::new()
            };
            all.extend(chosen);
            view.select_in(all);
            // Working in the notes lets go of the loop bars (R10, R14).
            view.set_loop_bars(Vec::new());
            // The time selection follows the notes (piano-roll R24): the
            // foreground clip's when it was named, else the first named.
            let foreground = view.shown_clip(doc, arrangement);
            let follow = spans
                .iter()
                .find(|(clip, _)| Some(*clip) == foreground)
                .or_else(|| spans.first());
            if let Some((clip, Some(span))) = follow {
                view.set_time_selection(*clip, Some(*span));
            }
            Ok(to_value(&view.state(doc, arrangement)))
        }
        ids::STEP_RECORD => {
            let StepRecordArgs { clip, back, held } = parse(args)?;
            let held = held.ok_or_else(|| invalid("`held` must name the keys held"))?;
            if let Some(key) = held
                .iter()
                .find(|h| h.key > 127 || !(1..=127).contains(&h.velocity))
            {
                return Err(invalid(format!(
                    "held key {} needs a key up to 127 and a velocity from 1 to 127",
                    key.key
                )));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let length = doc.clip(clip).map_or(Ticks::ZERO, |clip| clip.length);
            let (from, to) = span(&midi, length);
            let at = view.insert_marker(doc, arrangement).unwrap_or(from);
            let just = view.just_recorded(clip, &held);
            let stepped = step_notes(
                notes_in_order(&midi),
                arrangement.grid_ticks(doc),
                (from, to),
                at,
                &just,
                &held,
                back,
            );
            let undo_step = if stepped.changed() {
                transaction(editor, "Step Record", |editor| {
                    set_notes(editor, clip, &midi, stepped.notes.clone())
                })?
                .1
            } else {
                false
            };
            // The places the selection named may have moved.
            view.select(clip, Vec::new());
            view.set_just_recorded(clip, stepped.just.clone());
            view.set_insert_marker(clip, Some(stepped.marker));
            Ok(to_value(&StepRecordResult {
                added: stepped.added,
                lengthened: stepped.lengthened,
                shortened: stepped.shortened,
                removed: stepped.removed,
                insert_marker: stepped.marker,
                undo_step,
            }))
        }
        ids::ADD_NOTE => {
            let AddNoteArgs {
                clip,
                key,
                start,
                length,
                velocity,
            } = parse(args)?;
            if key > 127 {
                return Err(invalid("`key` must be 127 or less; 60 is middle C"));
            }
            if start.is_negative() {
                return Err(invalid("`start` must be zero or more ticks"));
            }
            if length <= Ticks::ZERO {
                return Err(invalid("`length` must be positive"));
            }
            let velocity = velocity.unwrap_or(100);
            if !(1..=127).contains(&velocity) {
                return Err(invalid("`velocity` must be from 1 to 127"));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let note = Note::new(key, start, length, velocity);
            let (place, recorded) = transaction(editor, "Add note", |editor| {
                let mut ordered = notes_in_order(&midi);
                ordered.push(note.clone());
                ordered.sort_by_key(|note| (note.start, note.key));
                let place = place_of(&ordered, &note);
                set_notes(editor, clip, &midi, ordered)?;
                Ok(place)
            })?;
            view.select(clip, vec![place]);
            Ok(to_value(&NoteEditResult {
                notes: vec![place],
                selected: vec![place],
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::DELETE_NOTES => {
            let DeleteNotesArgs { clip, notes, .. } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            // The notes to delete: those given, or the selected ones, or
            // with none selected the notes in the time selection
            // (piano-roll R28).
            let places = copied_places(view, doc, arrangement, clip, &ordered, notes)?;
            let (_, recorded) = transaction(editor, "Delete notes", |editor| {
                let mut keep = ordered.clone();
                let mut at = places.clone();
                at.sort_unstable();
                for place in at.iter().rev() {
                    keep.remove(*place as usize);
                }
                set_notes(editor, clip, &midi, keep)
            })?;
            view.select(clip, Vec::new());
            Ok(to_value(&NoteEditResult {
                notes: Vec::new(),
                selected: Vec::new(),
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::MOVE_NOTES => {
            let MoveNotesArgs {
                clip,
                notes,
                by,
                semitones,
                copy,
                ..
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let by = by.unwrap_or_default();
            let semitones = semitones.unwrap_or(0);
            let copy = copy.unwrap_or(false);
            let (after, recorded) = transaction(editor, "Move notes", |editor| {
                let mut data = midi.clone();
                let mut moved = ordered.clone();
                let mut moved_notes = Vec::with_capacity(places.len());
                for place in &places {
                    let note = &ordered[*place as usize];
                    let mut moved_note = note.clone();
                    moved_note.start = note_start(note, by)?;
                    moved_note.key = note_key(note, semitones)?;
                    moved_notes.push(moved_note);
                }
                if copy {
                    // The copies land where the notes would have gone, and
                    // the originals stay (piano-roll R29); a group's copies
                    // form a group of their own (note-editing R31).
                    regroup(&mut data, &mut moved_notes, &midi.groups);
                    moved.extend(moved_notes.iter().cloned());
                } else {
                    for (place, note) in places.iter().zip(&moved_notes) {
                        moved[*place as usize] = note.clone();
                    }
                }
                moved.sort_by_key(|note| (note.start, note.key));
                let places: Vec<u32> = moved_notes
                    .iter()
                    .map(|note| place_of(&moved, note))
                    .collect();
                set_notes_in(editor, clip, data, moved)?;
                Ok(places)
            })?;
            view.select(clip, after.clone());
            // The time selection follows the notes (piano-roll R24, R27).
            if let Some(span) = span_of(editor.document(), clip, &after) {
                view.set_time_selection(clip, Some(span));
            }
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::RESIZE_NOTES => {
            let ResizeNotesArgs {
                clip, notes, by, ..
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let (after, recorded) = transaction(editor, "Resize notes", |editor| {
                let mut resized = ordered.clone();
                let mut places_after = Vec::with_capacity(places.len());
                for place in &places {
                    let mut note = resized[*place as usize].clone();
                    note.length += by;
                    if note.length <= Ticks::ZERO {
                        return Err(refused("a note would be shorter than a tick"));
                    }
                    // Pressure past the note's new end goes with that part.
                    note.pressure.retain(|point| point.at < note.length);
                    resized[*place as usize] = note.clone();
                    places_after.push(place_of(&resized, &note));
                }
                set_notes(editor, clip, &midi, resized)?;
                Ok(places_after)
            })?;
            view.select(clip, after.clone());
            // The time selection follows the notes (piano-roll R24, R27).
            if let Some(span) = span_of(editor.document(), clip, &after) {
                view.set_time_selection(clip, Some(span));
            }
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::QUANTIZE_SETTINGS => {
            let QuantizeSettingsArgs {
                grid,
                target,
                amount,
            } = parse(args)?;
            check_grid(grid)?;
            check_amount(amount)?;
            view.set_quantize(grid, target, amount);
            Ok(to_value(&view.quantize()))
        }
        ids::QUANTIZE_NOTES => {
            let QuantizeNotesArgs {
                clip,
                notes,
                grid,
                target,
                amount,
                ..
            } = parse(args)?;
            check_grid(grid)?;
            check_amount(amount)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let settings = view.quantize();
            let grid = grid
                .or(settings.grid)
                .unwrap_or_else(|| arrangement.grid_ticks(doc));
            let target = target.unwrap_or(settings.target);
            let amount = amount.unwrap_or(settings.amount);
            let signatures = &doc.settings().time_signatures;
            let clip_start = doc.clip(clip).map_or(Ticks::ZERO, |value| value.start);
            let step = quantize_step(grid, signatures.signature_at(clip_start));
            let (after, recorded) = transaction(editor, "Quantize notes", |editor| {
                let mut quantized = ordered.clone();
                let mut moved = Vec::with_capacity(places.len());
                for place in &places {
                    let note = quantize_note(
                        &quantized[*place as usize],
                        target,
                        step,
                        signatures,
                        amount,
                    )?;
                    quantized[*place as usize] = note.clone();
                    moved.push(note);
                }
                quantized.sort_by_key(|note| (note.start, note.key));
                let places_after: Vec<u32> = moved
                    .iter()
                    .map(|note| place_of(&quantized, note))
                    .collect();
                set_notes(editor, clip, &midi, quantized)?;
                Ok(places_after)
            })?;
            view.select(clip, after.clone());
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::SET_VALUE => {
            let SetValueArgs {
                clip,
                notes,
                field,
                value,
                values,
                ..
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let values = match (value, values) {
                (Some(value), None) => vec![value_of(field, value)?; places.len()],
                (None, Some(values)) if values.len() == places.len() => values
                    .into_iter()
                    .map(|value| value_of(field, value))
                    .collect::<Result<Vec<_>, _>>()?,
                (None, Some(_)) => {
                    return Err(invalid("`values` needs one value for each of the notes"));
                }
                _ => return Err(invalid("give `value` or `values`, not both or neither")),
            };
            let (after, recorded) = transaction(editor, label(ids::SET_VALUE), |editor| {
                let mut edited = ordered.clone();
                for (place, value) in places.iter().zip(&values) {
                    set_note_value(&mut edited[*place as usize], field, *value);
                }
                if field == NoteField::Chance {
                    share_chance(&mut edited, &places);
                }
                set_notes(editor, clip, &midi, edited)?;
                Ok(places.clone())
            })?;
            view.select(clip, after.clone());
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::RAMP_VALUE => {
            let RampValueArgs {
                clip,
                notes,
                field,
                from,
                to,
                ..
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let from = value_of(field, from)?;
            let to = value_of(field, to)?;
            // The ramp runs in the clip's note order, whatever order the
            // places came in, spread by the notes' start times (R11).
            let mut places = places;
            places.sort_unstable();
            let starts: Vec<Ticks> = places.iter().map(|p| ordered[*p as usize].start).collect();
            let values = ramp_values(field, from, to, &starts);
            let (after, recorded) = transaction(editor, label(ids::RAMP_VALUE), |editor| {
                let mut edited = ordered.clone();
                for (place, value) in places.iter().zip(&values) {
                    set_note_value(&mut edited[*place as usize], field, *value);
                }
                if field == NoteField::Chance {
                    share_chance(&mut edited, &places);
                }
                set_notes(editor, clip, &midi, edited)?;
                Ok(places.clone())
            })?;
            view.select(clip, after.clone());
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::ADJUST_VALUE => {
            let AdjustValueArgs {
                clip,
                notes,
                field,
                by,
                ..
            } = parse(args)?;
            if !by.is_finite() {
                return Err(invalid("`by` must be a number"));
            }
            if field.whole() && by.fract() != 0.0 {
                return Err(invalid(format!("`{}` takes whole numbers", field.name())));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let (low, high) = field.range();
            let (after, recorded) = transaction(editor, label(ids::ADJUST_VALUE), |editor| {
                let mut edited = ordered.clone();
                for place in &places {
                    let note = &mut edited[*place as usize];
                    let next = tidy(field, (note_value(note, field) + by).clamp(low, high));
                    set_note_value(note, field, next);
                }
                if field == NoteField::Chance {
                    share_chance(&mut edited, &places);
                }
                set_notes(editor, clip, &midi, edited)?;
                Ok(places.clone())
            })?;
            view.select(clip, after.clone());
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::RANDOMIZE => {
            let RandomizeArgs {
                clip,
                notes,
                field,
                amount,
                seed,
                ..
            } = parse(args)?;
            if field == NoteField::VelocityDeviation {
                return Err(invalid(
                    "`field` must be `velocity`, `release_velocity` or `chance`",
                ));
            }
            let limit = if field.whole() { 127.0 } else { 1.0 };
            if !(0.0..=limit).contains(&amount) {
                return Err(invalid(format!("`amount` must be from 0 to {limit}")));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let length = doc.clip(clip).map_or(Ticks::ZERO, |clip| clip.length);
            let ordered = notes_in_order(&midi);
            // With none named or selected, every note in the clip's span.
            let places = if notes.is_none() && view.selection_of(clip).is_empty() {
                let (from, to) = span(&midi, length);
                let inside: Vec<u32> = (0..ordered.len() as u32)
                    .filter(|place| {
                        let start = ordered[*place as usize].start;
                        start >= from && start < to
                    })
                    .collect();
                if inside.is_empty() {
                    return Err(unavailable("the clip has no notes in view"));
                }
                inside
            } else {
                places_or_selected(view, clip, notes, ordered.len())?
            };
            let seed = seed.unwrap_or_else(|| {
                view.draws += 1;
                view.draws.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            });
            let (low, high) = field.range();
            let (after, recorded) = transaction(editor, label(ids::RANDOMIZE), |editor| {
                let mut edited = ordered.clone();
                // A group draws once, at its first note named (R25).
                let mut drawn = BTreeSet::new();
                for place in &places {
                    let note = &mut edited[*place as usize];
                    if field == NoteField::Chance
                        && note.group.is_some_and(|group| !drawn.insert(group))
                    {
                        continue;
                    }
                    let next = note_value(note, field) + amount * draw(seed, *place);
                    set_note_value(note, field, tidy(field, next.clamp(low, high)));
                }
                if field == NoteField::Chance {
                    share_chance(&mut edited, &places);
                }
                set_notes(editor, clip, &midi, edited)?;
                Ok(places.clone())
            })?;
            view.select(clip, after.clone());
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::DEACTIVATE => {
            let DeactivateArgs {
                clip,
                notes,
                active,
                ..
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            // Spec: note-editing R18. Deactivate, or activate when every
            // one of them is off already.
            let mute = match active {
                Some(active) => !active,
                None => !places.iter().all(|place| ordered[*place as usize].mute),
            };
            let name = if mute {
                label(ids::DEACTIVATE)
            } else {
                "Activate notes"
            };
            let (after, recorded) = transaction(editor, name, |editor| {
                let mut edited = ordered.clone();
                for place in &places {
                    edited[*place as usize].mute = mute;
                }
                set_notes(editor, clip, &midi, edited)?;
                Ok(places.clone())
            })?;
            view.select(clip, after.clone());
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::GROUP => {
            let GroupNotesArgs { clip, notes, kind } = parse(args)?;
            // Spec: multi-clip-editing R8. Notes of different clips can't
            // share a group.
            if clip.is_none()
                && notes.is_none()
                && view
                    .active_clips(doc, arrangement)
                    .iter()
                    .filter(|clip| !view.selection_of(**clip).is_empty())
                    .count()
                    > 1
            {
                return Err(refused("notes in different clips can't be grouped"));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let mut places = places_or_selected(view, clip, notes, ordered.len())?;
            if places.len() < 2 {
                return Err(refused("a group needs two or more notes"));
            }
            places.sort_unstable();
            if let Some(kind) = kind {
                view.group_kind = kind;
            }
            let kind = view.group_kind;
            // Spec: note-editing R22, R23. Naming a group's every note
            // changes its kind.
            let groups: BTreeSet<_> = places
                .iter()
                .map(|place| ordered[*place as usize].group)
                .collect();
            let whole = groups.len() == 1
                && groups.first().copied().flatten().is_some_and(|group| {
                    ordered.iter().filter(|n| n.group == Some(group)).count() == places.len()
                });
            let name = if whole {
                "Change Group"
            } else {
                label(ids::GROUP)
            };
            let (_, recorded) = transaction(editor, name, |editor| {
                let mut data = midi.clone();
                let mut edited = ordered.clone();
                let id = data.new_group_id();
                data.groups.push(NoteGroup { id, kind });
                let chance = edited[places[0] as usize].probability;
                for place in &places {
                    let note = &mut edited[*place as usize];
                    note.group = Some(id);
                    note.probability = chance;
                }
                set_notes_in(editor, clip, data, edited)
            })?;
            view.select(clip, places.clone());
            Ok(to_value(&NoteEditResult {
                notes: places.clone(),
                selected: places,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::UNGROUP => {
            let UngroupNotesArgs { clip, notes } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let (_, recorded) = transaction(editor, label(ids::UNGROUP), |editor| {
                let mut edited = ordered.clone();
                for place in &places {
                    edited[*place as usize].group = None;
                }
                set_notes(editor, clip, &midi, edited)
            })?;
            view.select(clip, places.clone());
            Ok(to_value(&NoteEditResult {
                notes: places.clone(),
                selected: places,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::COPY | ids::CUT => {
            let CopyNotesArgs { clip, notes, .. } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = copied_places(view, doc, arrangement, clip, &ordered, notes)?;
            let copied: Vec<Note> = places
                .iter()
                .filter_map(|place| ordered.get(*place as usize))
                .cloned()
                .collect();
            view.clipboard = NoteClipboard::of(&copied, &midi);
            view.pieces.clear();
            let mut recorded = false;
            if id == ids::CUT {
                // Cut copies the notes and deletes them, as one undo step.
                let (_, step) = transaction(editor, "Cut notes", |editor| {
                    let mut keep = ordered.clone();
                    let mut at = places.clone();
                    at.sort_unstable();
                    for place in at.iter().rev() {
                        keep.remove(*place as usize);
                    }
                    set_notes(editor, clip, &midi, keep)
                })?;
                recorded = step;
                view.select(clip, Vec::new());
            }
            if id == ids::COPY {
                Ok(to_value(&view.clipboard.state()))
            } else {
                Ok(to_value(&NoteEditResult {
                    notes: Vec::new(),
                    selected: Vec::new(),
                    undo_step: recorded,
                    clips: Vec::new(),
                }))
            }
        }
        ids::PASTE => {
            let PasteNotesArgs { clip, at } = parse(args)?;
            let named = clip.is_some();
            let (clip, _) = clip_and_data(doc, view, arrangement, clip)?;
            if view.clipboard.is_empty() {
                return Err(unavailable("there is nothing on the note clipboard"));
            }
            // The paste lands at the insert marker, or at the time
            // selection's start (piano-roll R28).
            let at = at
                .or_else(|| view.insert_marker(doc, arrangement))
                .or_else(|| view.time_selection(doc, arrangement).map(|(from, _)| from))
                .ok_or_else(|| unavailable("there is no insert marker to paste at"))?;
            // Spec: multi-clip-editing R12. Notes copied from several clips
            // go back into those clips while the clips shown and the
            // foreground clip are the same; else all into one clip.
            let same = (
                view.shown(doc, arrangement),
                view.shown_clip(doc, arrangement),
            );
            let back = !named
                && view.copied_with.as_ref() == Some(&same)
                && !view.active_clips(doc, arrangement).is_empty()
                && view.active_clips(doc, arrangement) == same.0;
            let pieces: Vec<(ClipId, NoteClipboard)> = if back {
                view.pieces.clone()
            } else {
                vec![(clip, view.clipboard.clone())]
            };
            let (after, recorded) = transaction(editor, "Paste notes", |editor| {
                let mut out = Vec::with_capacity(pieces.len());
                for (clip, piece) in &pieces {
                    let (_, midi) =
                        clip_and_data(editor.document(), view, arrangement, Some(*clip))?;
                    let mut data = midi.clone();
                    let mut pasted = piece.at(at);
                    regroup(&mut data, &mut pasted, &piece.groups);
                    let mut notes = notes_in_order(&midi);
                    notes.extend(pasted.iter().cloned());
                    notes.sort_by_key(|note| (note.start, note.key));
                    let places: Vec<u32> =
                        pasted.iter().map(|note| place_of(&notes, note)).collect();
                    set_notes_in(editor, *clip, data, notes)?;
                    out.push((*clip, places));
                }
                Ok(out)
            })?;
            view.select_in(after.clone());
            let first = after.first().cloned().unwrap_or((clip, Vec::new()));
            if let Some(span) = span_of(editor.document(), first.0, &first.1) {
                view.set_time_selection(first.0, Some(span));
            }
            Ok(to_value(&NoteEditResult {
                notes: first.1.clone(),
                selected: first.1,
                undo_step: recorded,
                clips: if after.len() > 1 {
                    after
                        .into_iter()
                        .map(|(clip, places)| ClipEdit {
                            clip,
                            notes: places.clone(),
                            selected: places,
                        })
                        .collect()
                } else {
                    Vec::new()
                },
            }))
        }
        ids::DUPLICATE => {
            let DuplicateNotesArgs { clip, notes, .. } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = copied_places(view, doc, arrangement, clip, &ordered, notes)?;
            let copied: Vec<Note> = places
                .iter()
                .filter_map(|place| ordered.get(*place as usize))
                .cloned()
                .collect();
            // The copy lands straight after the time selection, and the time
            // selection moves onto it (piano-roll R28).
            let own = (
                copied
                    .iter()
                    .map(|note| note.start)
                    .min()
                    .unwrap_or(Ticks::ZERO),
                copied
                    .iter()
                    .map(|note| note.start + note.length)
                    .max()
                    .unwrap_or(Ticks::ZERO),
            );
            let (_, to) = view.time_selection(doc, arrangement).unwrap_or(own);
            let mut data = midi.clone();
            let mut pasted = NoteClipboard::of(&copied, &midi).at(to);
            regroup(&mut data, &mut pasted, &midi.groups);
            let (after, recorded) = transaction(editor, "Duplicate notes", |editor| {
                let mut notes = notes_in_order(&midi);
                notes.extend(pasted.iter().cloned());
                notes.sort_by_key(|note| (note.start, note.key));
                let places: Vec<u32> = pasted.iter().map(|note| place_of(&notes, note)).collect();
                set_notes_in(editor, clip, data, notes)?;
                Ok(places)
            })?;
            view.select(clip, after.clone());
            if let Some(span) = span_of(editor.document(), clip, &after) {
                view.set_time_selection(clip, Some(span));
            }
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
                clips: Vec::new(),
            }))
        }
        ids::FOREGROUND => {
            let ForegroundArgs { clip } = parse(args)?;
            if !view.shown(doc, arrangement).contains(&clip) {
                return Err(invalid(format!(
                    "clip {} isn't one of the clips the note editor shows",
                    clip.0
                )));
            }
            view.set_foreground(clip);
            Ok(to_value(&view.state(doc, arrangement)))
        }
        ids::SELECT_LOOP_BARS => {
            let SelectLoopBarsArgs { clips, toggle } = parse(args)?;
            let shown = view.shown(doc, arrangement);
            if let Some(clip) = clips.iter().find(|clip| !shown.contains(clip)) {
                return Err(invalid(format!(
                    "clip {} isn't one of the clips the note editor shows",
                    clip.0
                )));
            }
            let mut bars = if toggle {
                view.loop_bars(doc, arrangement)
            } else {
                Vec::new()
            };
            for clip in &clips {
                match bars.iter().position(|bar| bar == clip) {
                    Some(at) if toggle => {
                        bars.remove(at);
                    }
                    Some(_) => {}
                    None => bars.push(*clip),
                }
            }
            // One bar at a time with Focus Mode on (R14).
            if view.focus_active(doc, arrangement) {
                bars = clips.last().into_iter().copied().collect();
            }
            // Clicking a bar makes its clip the foreground clip (R3, R10).
            if let Some(last) = clips.last().filter(|last| bars.contains(last)) {
                view.set_foreground(*last);
            }
            view.set_loop_bars(bars);
            Ok(to_value(&view.state(doc, arrangement)))
        }
        ids::FOCUS_MODE => {
            let FocusModeArgs { on } = parse(args)?;
            view.set_focus_mode(on.unwrap_or(!view.focus_mode()));
            Ok(to_value(&view.state(doc, arrangement)))
        }
        ids::DUPLICATE_LOOP => {
            let DuplicateLoopArgs { clips } = parse(args)?;
            let clips = match clips {
                Some(clips) if clips.is_empty() => return Err(invalid("`clips` is empty")),
                Some(clips) => clips,
                None => {
                    let bars = view.loop_bars(doc, arrangement);
                    if bars.is_empty() {
                        view.active_clips(doc, arrangement)
                    } else {
                        bars
                    }
                }
            };
            if clips.is_empty() {
                return Err(unavailable("no MIDI clip is shown in the Clip View"));
            }
            let (_, recorded) = transaction(editor, "Duplicate Loop", |editor| {
                for clip in &clips {
                    crate::multi_clip::duplicate_loop(editor, *clip)?;
                }
                Ok(())
            })?;
            Ok(to_value(&NoteEditResult {
                undo_step: recorded,
                clips: clips
                    .iter()
                    .map(|clip| ClipEdit {
                        clip: *clip,
                        notes: Vec::new(),
                        selected: view.selection_of(*clip).to_vec(),
                    })
                    .collect(),
                ..NoteEditResult::default()
            }))
        }
        ids::SPLIT => {
            let args: SplitNotesArgs = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, args.clip)?;
            let ordered = notes_in_order(&midi);
            let cuts = split_cuts(args, view, doc, arrangement, clip, ordered.len())?;
            let selected = view.selection_of(clip).to_vec();
            let (done, recorded) = transaction(editor, label(ids::SPLIT), |editor| {
                let mut carried = Vec::with_capacity(ordered.len());
                for (place, note) in ordered.iter().enumerate() {
                    let place = u32::try_from(place).unwrap_or(u32::MAX);
                    let at: Vec<Ticks> = cuts
                        .iter()
                        .filter(|(cut, _)| *cut == place)
                        .map(|(_, at)| *at)
                        .collect();
                    let was = selected.contains(&place);
                    let parts = note_ops::split(note, &at);
                    if parts.len() == 1 {
                        carried.push(Carried::kept(note, was));
                    } else {
                        carried.extend(parts.into_iter().map(|part| Carried::made(part, was)));
                    }
                }
                store(editor, clip, &midi, carried)
            })?;
            Ok(carried_result(view, clip, done, recorded))
        }
        ids::CHOP => {
            let ChopNotesArgs {
                clip,
                notes,
                grid,
                parts,
                steps,
            } = parse(args)?;
            if grid.is_some() && parts.is_some() {
                return Err(invalid("give `grid` or `parts`, not both"));
            }
            if parts.is_some_and(|parts| !(1..=MOST_PARTS).contains(&parts)) {
                return Err(invalid("`parts` must be from 1 to 64"));
            }
            check_grid(grid)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let signatures = &doc.settings().time_signatures;
            let clip_start = doc.clip(clip).map_or(Ticks::ZERO, |value| value.start);
            let step = grid.map_or_else(
                || arrangement.grid_step(doc),
                |grid| quantize_step(grid, signatures.signature_at(clip_start)),
            );
            let selected = view.selection_of(clip).to_vec();
            let (done, recorded) = transaction(editor, label(ids::CHOP), |editor| {
                let mut carried = Vec::with_capacity(ordered.len());
                for (place, note) in ordered.iter().enumerate() {
                    let at = u32::try_from(place).unwrap_or(u32::MAX);
                    let was = selected.contains(&at);
                    if !places.contains(&at) {
                        carried.push(Carried::kept(note, was));
                        continue;
                    }
                    // Spec: note-editing R36. Cut on the grid, or into
                    // equal parts, then change the count by each step.
                    let mut cuts = Vec::new();
                    let mut count = parts.unwrap_or(1);
                    if parts.is_none() {
                        lines(
                            signatures,
                            step,
                            note.start + Ticks(1),
                            note.start + note.length - Ticks(1),
                            4096,
                            |line, _| cuts.push(line),
                        );
                        count = u32::try_from(cuts.len() + 1).unwrap_or(MOST_PARTS);
                    }
                    let pieces = if steps.is_empty() && parts.is_none() {
                        note_ops::split(note, &cuts)
                    } else {
                        let count = steps.iter().fold(count.min(MOST_PARTS), |count, step| {
                            step_parts(count, *step)
                        });
                        note_ops::chop_equal(note, count)
                    };
                    if pieces.len() == 1 {
                        carried.push(Carried::kept(note, was));
                    } else {
                        carried.extend(pieces.into_iter().map(|piece| Carried::made(piece, was)));
                    }
                }
                store(editor, clip, &midi, carried)
            })?;
            Ok(carried_result(view, clip, done, recorded))
        }
        ids::JOIN => {
            let JoinNotesArgs { clip, notes } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let selected = view.selection_of(clip).to_vec();
            // Spec: note-editing R39. The notes of a pitch with two or more.
            let mut groups: std::collections::BTreeMap<u8, Vec<u32>> = Default::default();
            for place in &places {
                groups
                    .entry(ordered[*place as usize].key)
                    .or_default()
                    .push(*place);
            }
            groups.retain(|_, group| group.len() > 1);
            if groups.is_empty() {
                return Err(unavailable("there are no two notes of one pitch to join"));
            }
            for group in groups.values_mut() {
                group.sort_unstable();
            }
            let (done, recorded) = transaction(editor, label(ids::JOIN), |editor| {
                let mut carried = Vec::with_capacity(ordered.len());
                for (place, note) in ordered.iter().enumerate() {
                    let at = u32::try_from(place).unwrap_or(u32::MAX);
                    match groups.get(&note.key) {
                        Some(group) if group.first() == Some(&at) => {
                            let members: Vec<Note> = group
                                .iter()
                                .map(|member| ordered[*member as usize].clone())
                                .collect();
                            carried.push(Carried::made(
                                note_ops::join(&members),
                                selected.contains(&at),
                            ));
                        }
                        Some(group) if group.contains(&at) => {}
                        _ => carried.push(Carried::kept(note, selected.contains(&at))),
                    }
                }
                store(editor, clip, &midi, carried)
            })?;
            Ok(carried_result(view, clip, done, recorded))
        }
        ids::FIT_TO_TIME => {
            let FitToTimeArgs { clip, notes, range } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let (from, to) = match range {
                Some(range) => (range.start, range.end),
                None => view
                    .time_selection(doc, arrangement)
                    .ok_or_else(|| unavailable("there is no time selection to fit to"))?,
            };
            if from < Ticks::ZERO || to <= from {
                return Err(invalid(
                    "the range must start at 0 or later and end after it",
                ));
            }
            let selected = view.selection_of(clip).to_vec();
            let (done, recorded) = transaction(editor, label(ids::FIT_TO_TIME), |editor| {
                let mut carried = Vec::with_capacity(ordered.len());
                for (place, note) in ordered.iter().enumerate() {
                    let at = u32::try_from(place).unwrap_or(u32::MAX);
                    let was = selected.contains(&at);
                    if !places.contains(&at) {
                        carried.push(Carried::kept(note, was));
                        continue;
                    }
                    let own = (note.start, note.start + note.length);
                    let map = Mapping::new(own, (from, to), None).map_err(invalid)?;
                    let fitted = note_ops::remap(note, |tick| map.at(tick)).map_err(refused)?;
                    carried.push(Carried::made(fitted, was));
                }
                store(editor, clip, &midi, carried)
            })?;
            view.set_time_selection(clip, Some((from, to)));
            Ok(carried_result(view, clip, done, recorded))
        }
        ids::STRETCH => {
            let StretchNotesArgs {
                clip,
                notes,
                from,
                to,
                point,
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let (start, end) = match from {
                Some(range) => (range.start, range.end),
                None => view
                    .time_selection(doc, arrangement)
                    .ok_or_else(|| unavailable("there is no time selection to stretch"))?,
            };
            let map = Mapping::new(
                (start, end),
                (to.start, to.end),
                point.map(|point| (point.from, point.to)),
            )
            .map_err(invalid)?;
            let selected = view.selection_of(clip).to_vec();
            // Spec: note-editing R43. The selected notes, or with none
            // selected the notes that start inside the range.
            let places: Vec<u32> = match notes {
                Some(notes) => places_or_selected(view, clip, Some(notes), ordered.len())?,
                None if !selected.is_empty() => selected.clone(),
                None => ordered
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| note.start >= start && note.start < end)
                    .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
                    .collect(),
            };
            let (done, recorded) = transaction(editor, label(ids::STRETCH), |editor| {
                let mut carried = Vec::with_capacity(ordered.len());
                for (place, note) in ordered.iter().enumerate() {
                    let at = u32::try_from(place).unwrap_or(u32::MAX);
                    let was = selected.contains(&at);
                    if places.contains(&at) {
                        let moved = note_ops::remap(note, |tick| map.at(tick)).map_err(refused)?;
                        carried.push(Carried::made(moved, was));
                    } else {
                        carried.push(Carried::kept(note, was));
                    }
                }
                let done = store(editor, clip, &midi, carried)?;
                stretch_envelopes(editor, clip, (start, end), &map)?;
                Ok(done)
            })?;
            view.set_time_selection(clip, Some((to.start.min(to.end), to.start.max(to.end))));
            Ok(carried_result(view, clip, done, recorded))
        }
        other => Err(invalid(format!("`{other}` isn't a note command"))),
    }
}

/// The commands that take `clips` and, with no clip or notes named, act on
/// the selected notes of every clip the edits reach (Spec: multi-clip-editing
/// R7, R18).
const ACROSS: &[&str] = &[
    ids::DELETE_NOTES,
    ids::MOVE_NOTES,
    ids::RESIZE_NOTES,
    ids::QUANTIZE_NOTES,
    ids::SET_VALUE,
    ids::RAMP_VALUE,
    ids::ADJUST_VALUE,
    ids::RANDOMIZE,
    ids::DEACTIVATE,
    ids::COPY,
    ids::CUT,
    ids::DUPLICATE,
];

/// The clips an edit across clips reaches, each with the notes it names:
/// those `clips` lists, or with no `clip` and `notes` given and several
/// clips reached, the clips with notes selected. `None` leaves the edit to
/// the one clip it names or the foreground clip.
fn edit_targets(
    id: &str,
    args: &Value,
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
) -> Result<Option<Vec<ClipNotes>>, EditFailure> {
    let Some(fields) = args.as_object().filter(|_| ACROSS.contains(&id)) else {
        return Ok(None);
    };
    let given = |name: &str| fields.get(name).is_some_and(|value| !value.is_null());
    if given("clips") {
        if given("clip") || given("notes") {
            return Err(invalid("give `clips`, or `clip` and `notes`, not both"));
        }
        let clips: Vec<ClipNotes> =
            serde_json::from_value(fields["clips"].clone()).map_err(|e| invalid(e.to_string()))?;
        if clips.is_empty() {
            return Err(invalid("`clips` is empty"));
        }
        let mut seen = BTreeSet::new();
        if let Some(twice) = clips.iter().find(|target| !seen.insert(target.clip)) {
            return Err(invalid(format!("clip {} is named twice", twice.clip.0)));
        }
        return Ok(Some(clips));
    }
    if given("clip") || given("notes") {
        return Ok(None);
    }
    let reached: Vec<ClipNotes> = view
        .active_clips(doc, arrangement)
        .into_iter()
        .filter(|clip| !view.selection_of(*clip).is_empty())
        .map(|clip| ClipNotes { clip, notes: None })
        .collect();
    // One clip's selection that isn't the foreground clip's is that clip's
    // to edit too.
    let foreground = view.shown_clip(doc, arrangement);
    let elsewhere =
        reached.len() > 1 || reached.iter().any(|target| Some(target.clip) != foreground);
    Ok(elsewhere.then_some(reached))
}

/// Runs an edit once in each of `targets`' clips as one undo step, each
/// note staying in its own clip (Spec: multi-clip-editing R7). Cut and Copy
/// put every clip's notes on the note clipboard together (R12).
fn run_across(
    id: &str,
    args: &Value,
    targets: Vec<ClipNotes>,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    let name = if id == ids::CUT {
        "Cut notes"
    } else {
        label(id)
    };
    let one = |target: &ClipNotes, id: &str, notes: Option<&Vec<u32>>| {
        let mut fields = args.as_object().cloned().unwrap_or_default();
        fields.remove("clips");
        fields.insert("clip".to_owned(), to_value(&target.clip));
        match notes.or(target.notes.as_ref()) {
            Some(notes) => fields.insert("notes".to_owned(), to_value(notes)),
            None => fields.remove("notes"),
        };
        (id.to_owned(), Value::Object(fields))
    };
    let mut results: Vec<ClipEdit> = Vec::with_capacity(targets.len());
    let doc = editor.document().clone();
    // Each run below selects in its one clip alone, so put every clip's
    // selection back before the next.
    let saved: Vec<(ClipId, Vec<u32>)> = targets
        .iter()
        .map(|target| (target.clip, view.selection_of(target.clip).to_vec()))
        .collect();
    if matches!(id, ids::COPY | ids::CUT) {
        // Spec: R12. What each clip gives, kept apart so a paste can put it
        // back, and together for a paste into one clip.
        let mut pieces = Vec::with_capacity(targets.len());
        let mut places = Vec::with_capacity(targets.len());
        for target in &targets {
            let (clip, midi) = clip_and_data(&doc, view, arrangement, Some(target.clip))?;
            let ordered = notes_in_order(&midi);
            let chosen = copied_places(
                view,
                &doc,
                arrangement,
                clip,
                &ordered,
                target.notes.clone(),
            )?;
            pieces.push((
                clip,
                chosen
                    .iter()
                    .filter_map(|place| ordered.get(*place as usize))
                    .cloned()
                    .collect::<Vec<Note>>(),
                midi.groups.clone(),
            ));
            places.push((clip, chosen));
        }
        view.set_clipboard_pieces(
            pieces,
            view.shown(&doc, arrangement),
            view.shown_clip(&doc, arrangement),
        );
        if id == ids::COPY {
            return Ok(to_value(&view.clipboard.state()));
        }
        let (_, recorded) = transaction(editor, name, |editor| {
            for (clip, chosen) in &places {
                let target = ClipNotes {
                    clip: *clip,
                    notes: None,
                };
                let (id, args) = one(&target, ids::DELETE_NOTES, Some(chosen));
                run(&id, args, editor, view, arrangement)?;
            }
            Ok(())
        })?;
        view.select_in(places.iter().map(|(clip, _)| (*clip, Vec::new())).collect());
        return Ok(to_value(&NoteEditResult {
            undo_step: recorded,
            ..NoteEditResult::default()
        }));
    }
    let (_, recorded) = transaction(editor, name, |editor| {
        for target in &targets {
            view.select_in(saved.clone());
            let (id, args) = one(target, id, None);
            let result: NoteEditResult =
                serde_json::from_value(run(&id, args, editor, view, arrangement)?)
                    .map_err(|e| invalid(e.to_string()))?;
            results.push(ClipEdit {
                clip: target.clip,
                notes: result.notes,
                selected: result.selected,
            });
        }
        Ok(())
    })?;
    // Each note stays selected in its own clip (R7); the time selection
    // follows the foreground clip's notes (piano-roll R24).
    view.select_in(
        results
            .iter()
            .map(|edit| (edit.clip, edit.selected.clone()))
            .collect(),
    );
    let after = editor.document();
    if let Some(edit) = results
        .iter()
        .find(|edit| Some(edit.clip) == view.shown_clip(after, arrangement))
        && let Some(span) = span_of(after, edit.clip, &edit.selected)
    {
        view.set_time_selection(edit.clip, Some(span));
    }
    let (notes, selected) = results
        .first()
        .map(|edit| (edit.notes.clone(), edit.selected.clone()))
        .unwrap_or_default();
    Ok(to_value(&NoteEditResult {
        notes,
        selected,
        undo_step: recorded,
        clips: results,
    }))
}

/// The notes the clipboard commands act on: those given, or the selected
/// ones, or with none selected the notes that start inside the time
/// selection (piano-roll R28).
fn copied_places(
    view: &ClipView,
    doc: &Document,
    arrangement: &ArrangementView,
    clip: ClipId,
    ordered: &[Note],
    notes: Option<Vec<u32>>,
) -> Result<Vec<u32>, EditFailure> {
    if let Some(places) = notes {
        return places_or_selected(view, clip, Some(places), ordered.len());
    }
    let selected = view.selection_of(clip);
    if !selected.is_empty() {
        return Ok(selected.to_vec());
    }
    let Some((from, to)) = view
        .time_selection(doc, arrangement)
        .filter(|_| view.shown_clip(doc, arrangement) == Some(clip))
    else {
        return Err(unavailable("no notes are selected"));
    };
    let inside: Vec<u32> = ordered
        .iter()
        .enumerate()
        .filter(|(_, note)| note.start >= from && note.start < to)
        .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
        .collect();
    if inside.is_empty() {
        return Err(unavailable("no notes are selected"));
    }
    Ok(inside)
}

/// Checks a quantise grid: a spacing of ticks, always positive.
fn check_grid(grid: Option<Ticks>) -> Result<(), EditFailure> {
    match grid {
        Some(grid) if grid <= Ticks::ZERO => {
            Err(invalid("`grid` must be a positive number of ticks"))
        }
        _ => Ok(()),
    }
}

/// Checks a quantise amount: a percentage from 0 to 100.
fn check_amount(amount: Option<u8>) -> Result<(), EditFailure> {
    match amount {
        Some(amount) if amount > 100 => Err(invalid("`amount` must be from 0 to 100 percent")),
        _ => Ok(()),
    }
}

/// The quantise grid's step for a spacing of `grid` ticks under
/// `signature`: a spacing of whole bars counts bars from 1.1.1, as the
/// editing grid's bar steps do; anything else is a spacing of ticks laid
/// out inside each bar.
pub(crate) fn quantize_step(grid: Ticks, signature: TimeSignature) -> Step {
    let bar = signature.bar_length();
    let bars =
        (grid >= bar && grid.get().rem_euclid(bar.get()) == 0).then(|| grid.get() / bar.get());
    Step {
        ticks: grid,
        bars,
        note: None,
        triplet: false,
    }
}

/// One note quantised: its start, or its end with `target`
/// [`QuantizeTarget::NoteEnd`], moves `amount` percent of the way to the
/// nearest grid line. The other end stays where it is; an end that would
/// leave a note shorter than a tick is refused.
fn quantize_note(
    note: &Note,
    target: QuantizeTarget,
    step: Step,
    signatures: &TimeSignatureMap,
    amount: u8,
) -> Result<Note, EditFailure> {
    let mut quantized = note.clone();
    match target {
        QuantizeTarget::NoteStart => {
            quantized.start = toward(note.start, snap(signatures, step, note.start), amount);
        }
        QuantizeTarget::NoteEnd => {
            let end = note.start + note.length;
            let moved = toward(end, snap(signatures, step, end), amount);
            quantized.length = moved - note.start;
            if quantized.length <= Ticks::ZERO {
                return Err(refused("a note would be shorter than a tick"));
            }
            quantized
                .pressure
                .retain(|point| point.at < quantized.length);
        }
    }
    Ok(quantized)
}

/// `at` moved `amount` percent of the way toward `to`, rounded to the
/// nearest tick.
pub(crate) fn toward(at: Ticks, to: Ticks, amount: u8) -> Ticks {
    let distance = i128::from(to.get()) - i128::from(at.get());
    let rounded = (distance * i128::from(amount) + 50).div_euclid(100);
    let moved = i64::try_from(rounded).unwrap_or(if rounded.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    });
    Ticks(at.get().saturating_add(moved))
}

/// The note's start after moving `by` ticks, if it stays inside the clip's
/// contents.
fn note_start(note: &Note, by: Ticks) -> Result<Ticks, EditFailure> {
    note.start
        .checked_add(by)
        .filter(|start| !start.is_negative())
        .ok_or_else(|| {
            refused(format!(
                "a note would start at {}, before the clip's contents begin",
                note.start.saturating_add(by)
            ))
        })
}

/// The note's key after moving `semitones`, if it stays a MIDI key.
fn note_key(note: &Note, semitones: i32) -> Result<u8, EditFailure> {
    let key = i32::from(note.key) + semitones;
    u8::try_from(key)
        .ok()
        .filter(|key| *key <= 127)
        .ok_or_else(|| {
            refused(format!(
                "a note would leave the range of MIDI keys 0 to 127 (key {key})"
            ))
        })
}

#[cfg(test)]
mod tests {
    use bl_model::PressurePoint;

    use super::*;

    // Spec: note-editing R36.
    #[test]
    fn the_parts_of_a_chop_stay_from_one_to_sixty_four() {
        assert_eq!(step_parts(4, ChopStep::More), 5);
        assert_eq!(step_parts(5, ChopStep::NextPower), 8);
        assert_eq!(step_parts(8, ChopStep::NextPower), 16);
        assert_eq!(step_parts(8, ChopStep::PreviousPower), 4);
        assert_eq!(step_parts(1, ChopStep::Fewer), 1);
        assert_eq!(step_parts(1, ChopStep::PreviousPower), 1);
        assert_eq!(step_parts(64, ChopStep::More), 64);
        assert_eq!(step_parts(64, ChopStep::NextPower), 64);
    }

    // Spec: midi-controller-data R2. A note made shorter loses the pressure
    // points past its new end, so the edit isn't refused for them.
    #[test]
    fn shortening_a_note_drops_the_pressure_past_its_new_end() {
        let point = |at: Ticks, value| PressurePoint { at, value };
        let note = Note {
            pressure: vec![
                point(Ticks::ZERO, 10),
                point(Ticks::QUARTER - Ticks(1), 20),
                point(Ticks::QUARTER + Ticks(50_000), 30),
            ],
            ..Note::new(60, Ticks::ZERO, Ticks::QUARTER + Ticks(100_000), 100)
        };
        let step = quantize_step(Ticks::QUARTER, TimeSignature::new(4, 4).unwrap());
        let signatures = TimeSignatureMap::default();
        let quantized =
            quantize_note(&note, QuantizeTarget::NoteEnd, step, &signatures, 100).unwrap();
        assert_eq!(quantized.length, Ticks::QUARTER);
        assert_eq!(
            quantized.pressure,
            vec![point(Ticks::ZERO, 10), point(Ticks::QUARTER - Ticks(1), 20)]
        );
    }
}
