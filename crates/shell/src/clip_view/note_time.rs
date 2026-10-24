//! The note editor's own time commands (WP-6.54; the piano roll spec, R39):
//! Duplicate Time, Insert Time and Delete Time on the notes of the clip the
//! editor shows. They act on the clip's contents and never move its Start
//! or End marker or its loop brace.
//!
//! The span is the call's `start` and `end`, in ticks of the clip's
//! contents, or the editor's time selection. What happens to a note across
//! an edge is Beatlayer's own, after the Arrangement's: a note belongs to
//! the span that holds its start.

use bl_model::{Document, Editor, Note};
use bl_shell_spec::notes::{self as ids, NoteEditResult, NoteTimeArgs};
use serde_json::Value;

use super::{
    ClipView, clip_and_data, invalid, notes_in_order, parse, regroup, set_notes_in, to_value,
    unavailable,
};
use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{EditFailure, transaction};

/// Whether `id` is one of the time commands.
#[must_use]
pub(super) fn handles(id: &str) -> bool {
    matches!(
        id,
        ids::DUPLICATE_TIME | ids::INSERT_TIME | ids::DELETE_TIME
    )
}

/// A note on its way through a time edit: whether it was selected, and
/// whether the edit made it.
struct Moved {
    note: Note,
    selected: bool,
    made: bool,
}

/// Runs one of the time commands as one undo step.
pub(super) fn run(
    id: &str,
    args: Value,
    doc: &Document,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    let NoteTimeArgs { clip, start, end } = parse(args)?;
    let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
    let (from, to) = match (start, end) {
        (Some(from), Some(to)) if from < to => (from, to),
        (Some(_), Some(_)) => return Err(invalid("`start` must be before `end`")),
        (None, None) => view
            .time_selection(doc, arrangement)
            .filter(|_| view.shown_clip(doc, arrangement) == Some(clip))
            .ok_or_else(|| unavailable("make a time selection first"))?,
        _ => return Err(invalid("give `start` and `end` together")),
    };
    let length = to - from;
    let ordered = notes_in_order(&midi);
    let selected = view.selection_of(clip).to_vec();
    let mut data = midi.clone();
    let mut moved = Vec::with_capacity(ordered.len());
    let mut copies = Vec::new();
    for (place, note) in ordered.iter().enumerate() {
        let was = selected.contains(&u32::try_from(place).unwrap_or(u32::MAX));
        let mut note = note.clone();
        let inside = note.start >= from && note.start < to;
        match id {
            ids::DUPLICATE_TIME => {
                if inside {
                    let mut copy = note.clone();
                    copy.start += length;
                    copies.push(copy);
                }
                if note.start >= to {
                    note.start += length;
                }
            }
            ids::INSERT_TIME => {
                if note.start >= from {
                    note.start += length;
                }
            }
            _ => {
                if inside {
                    continue;
                }
                if note.start >= to {
                    note.start -= length;
                } else if note.start + note.length > from {
                    note.length = from - note.start;
                }
            }
        }
        moved.push(Moved {
            note,
            selected: was,
            made: false,
        });
    }
    regroup(&mut data, &mut copies, &midi.groups);
    moved.extend(copies.into_iter().map(|note| Moved {
        note,
        selected: false,
        made: true,
    }));
    moved.sort_by_key(|moved| (moved.note.start, moved.note.key));
    let places = |wanted: fn(&Moved) -> bool| -> Vec<u32> {
        moved
            .iter()
            .enumerate()
            .filter(|(_, moved)| wanted(moved))
            .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
            .collect()
    };
    let made = places(|moved| moved.made);
    let now_selected = places(|moved| moved.selected);
    let label = match id {
        ids::DUPLICATE_TIME => "Duplicate Time",
        ids::INSERT_TIME => "Insert Time",
        _ => "Delete Time",
    };
    let notes: Vec<Note> = moved.into_iter().map(|moved| moved.note).collect();
    let (_, recorded) = transaction(editor, label, |editor| {
        set_notes_in(editor, clip, data, notes)
    })?;
    view.select(clip, now_selected.clone());
    match id {
        // The copy becomes the time selection, so pressing it again lays
        // down another (as time-editing R20 does).
        ids::DUPLICATE_TIME => view.set_time_selection(clip, Some((to, to + length))),
        // The insert marker goes to where the span began, with no time
        // selected (as time-editing R15 does).
        ids::DELETE_TIME => view.set_insert_marker(clip, Some(from)),
        _ => {}
    }
    Ok(to_value(&NoteEditResult {
        notes: made,
        selected: now_selected,
        undo_step: recorded,
        clips: Vec::new(),
    }))
}
