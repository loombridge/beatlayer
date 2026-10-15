//! The Clip View's note editing (WP-2.3): the MIDI Note Editor's notes, the
//! notes selected in it, and the commands that draw, delete, move and
//! resize them.
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

use std::collections::BTreeSet;

use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, Command, Document, Editor, MidiData, Note};
use bl_shell_spec::notes::{
    self as ids, AddNoteArgs, ClipViewState, DeleteNotesArgs, MoveNotesArgs, NoteEditResult,
    NoteSelectMode, ResizeNotesArgs, SelectNotesArgs,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{EditFailure, transaction};

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
    let mut notes: Vec<Note> = midi.notes.iter().copied().collect();
    notes.sort_by_key(|note| (note.start, note.key));
    notes
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

/// What the Clip View shows: whether it's open, Draw Mode, and the notes
/// selected in the clip its note editor shows (WP-2.3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipView {
    /// Whether the Clip View shows in the detail area.
    visible: bool,
    /// Whether Draw Mode is on (B).
    draw_mode: bool,
    /// The clip whose notes are selected, and their places in its notes in
    /// order. The selection holds only while the note editor shows that
    /// clip.
    selected_clip: Option<ClipId>,
    selected: Vec<u32>,
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

    /// Shows the Clip View, or hides it.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Switches Draw Mode on or off.
    pub fn toggle_draw_mode(&mut self) {
        self.draw_mode = !self.draw_mode;
    }

    /// The clip the note editor shows: the Arrangement's first selected
    /// clip that holds MIDI notes.
    #[must_use]
    pub fn shown_clip(&self, doc: &Document, arrangement: &ArrangementView) -> Option<ClipId> {
        arrangement.selected_clips().iter().copied().find(|id| {
            doc.clip(*id)
                .is_some_and(|clip| matches!(clip.data, ClipData::Midi(_)))
        })
    }

    /// The selected notes' places in the shown clip's notes in order, empty
    /// when the selection belongs to another clip.
    #[must_use]
    pub fn selected_notes(&self, doc: &Document, arrangement: &ArrangementView) -> &[u32] {
        if self.selected_clip == self.shown_clip(doc, arrangement) {
            &self.selected
        } else {
            &[]
        }
    }

    /// Selects notes of `clip`, by their places in its notes in order.
    pub fn select(&mut self, clip: ClipId, places: Vec<u32>) {
        self.selected_clip = Some(clip);
        self.selected = places;
    }

    /// What the view shows, as a command reports it.
    #[must_use]
    pub fn state(&self, doc: &Document, arrangement: &ArrangementView) -> ClipViewState {
        ClipViewState {
            visible: self.visible,
            draw_mode: self.draw_mode,
            clip: self.shown_clip(doc, arrangement),
            selected: self.selected_notes(doc, arrangement).to_vec(),
        }
    }

    /// Forgets a note selection that a replaced set can't hold any more.
    pub(crate) fn forget_selection(&mut self) {
        self.selected_clip = None;
        self.selected.clear();
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
    doc: &Document,
    arrangement: &ArrangementView,
    clip: ClipId,
    notes: Option<Vec<u32>>,
    count: usize,
) -> Result<Vec<u32>, EditFailure> {
    let given = notes.is_some();
    let places = notes.unwrap_or_else(|| view.selected_notes(doc, arrangement).to_vec());
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
    let mut data = midi.clone();
    data.notes = notes.into_iter().collect();
    editor
        .apply(Command::SetClipData {
            clip,
            data: ClipData::Midi(data),
        })
        .map(|_| ())?;
    Ok(())
}

/// The place of `note` among `ordered`: the first equal note, which is
/// the note itself when no identical one is already there.
fn place_of(ordered: &[Note], note: Note) -> u32 {
    let place = ordered.iter().position(|other| *other == note).unwrap_or(0);
    u32::try_from(place).unwrap_or(u32::MAX)
}

/// The name of the undo step a note command makes.
#[must_use]
pub fn label(id: &str) -> &'static str {
    match id {
        ids::ADD_NOTE => "Add note",
        ids::DELETE_NOTES => "Delete notes",
        ids::MOVE_NOTES => "Move notes",
        ids::RESIZE_NOTES => "Resize notes",
        _ => "Select notes",
    }
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
    match id {
        ids::SELECT_NOTES => {
            let SelectNotesArgs { clip, notes, mode } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            for place in &notes {
                if *place as usize >= ordered.len() {
                    return Err(invalid(format!(
                        "clip {} has no note at place {place}",
                        clip.0
                    )));
                }
            }
            let mut selected = view.selected_notes(doc, arrangement).to_vec();
            match mode {
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
            }
            view.select(clip, selected);
            Ok(to_value(&view.state(doc, arrangement)))
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
                ordered.push(note);
                ordered.sort_by_key(|note| (note.start, note.key));
                let place = place_of(&ordered, note);
                set_notes(editor, clip, &midi, ordered)?;
                Ok(place)
            })?;
            view.select(clip, vec![place]);
            Ok(to_value(&NoteEditResult {
                notes: vec![place],
                selected: vec![place],
                undo_step: recorded,
            }))
        }
        ids::DELETE_NOTES => {
            let DeleteNotesArgs { clip, notes } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, doc, arrangement, clip, notes, ordered.len())?;
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
            }))
        }
        ids::MOVE_NOTES => {
            let MoveNotesArgs {
                clip,
                notes,
                by,
                semitones,
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, doc, arrangement, clip, notes, ordered.len())?;
            let by = by.unwrap_or_default();
            let semitones = semitones.unwrap_or(0);
            let (after, recorded) = transaction(editor, "Move notes", |editor| {
                let mut moved = ordered.clone();
                let mut moved_notes = Vec::with_capacity(places.len());
                for place in &places {
                    let mut note = moved[*place as usize];
                    note.start = note_start(note, by)?;
                    note.key = note_key(note, semitones)?;
                    moved[*place as usize] = note;
                    moved_notes.push(note);
                }
                moved.sort_by_key(|note| (note.start, note.key));
                let places: Vec<u32> = moved_notes
                    .iter()
                    .map(|note| place_of(&moved, *note))
                    .collect();
                set_notes(editor, clip, &midi, moved)?;
                Ok(places)
            })?;
            view.select(clip, after.clone());
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
            }))
        }
        ids::RESIZE_NOTES => {
            let ResizeNotesArgs { clip, notes, by } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, doc, arrangement, clip, notes, ordered.len())?;
            let (after, recorded) = transaction(editor, "Resize notes", |editor| {
                let mut resized = ordered.clone();
                let mut places_after = Vec::with_capacity(places.len());
                for place in &places {
                    let mut note = resized[*place as usize];
                    note.length += by;
                    if note.length <= Ticks::ZERO {
                        return Err(refused("a note would be shorter than a tick"));
                    }
                    resized[*place as usize] = note;
                    places_after.push(place_of(&resized, note));
                }
                set_notes(editor, clip, &midi, resized)?;
                Ok(places_after)
            })?;
            view.select(clip, after.clone());
            Ok(to_value(&NoteEditResult {
                notes: after.clone(),
                selected: after,
                undo_step: recorded,
            }))
        }
        other => Err(invalid(format!("`{other}` isn't a note command"))),
    }
}

/// The note's start after moving `by` ticks, if it stays inside the clip's
/// contents.
fn note_start(note: Note, by: Ticks) -> Result<Ticks, EditFailure> {
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
fn note_key(note: Note, semitones: i32) -> Result<u8, EditFailure> {
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
