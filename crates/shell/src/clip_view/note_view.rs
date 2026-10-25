//! The MIDI Note Editor's view (WP-6.54; the piano roll spec, R32 to R38):
//! the span of the clip's contents and the key rows the note area shows,
//! the zoom history X steps back through, and Follow's pause. They belong to
//! the shell, not the window, so a key, a ruler, the Clip View Selector and
//! an agent reach them alike. None of it is saved with the set or an undo
//! step.
//!
//! The span is in ticks from the beginning of the clip's contents, as the
//! insert marker and the time selection are. The rows count down from the
//! top of the keyboard: row 0 is key 127, or the highest key while the
//! editor is folded.

use bl_core::time::Ticks;
use bl_model::registry::{SET_MIDI_LOOP, SET_MIDI_REGION};
use bl_model::{Clip, ClipData, ClipId, Command, Document, Editor, MidiData};
use bl_shell_spec::notes::{
    self as ids, ClipMarker, ClipTab, NoteViewArgs, NoteViewChange, SetMarkerArgs,
};
use serde_json::{Value, json};

use super::{ClipView, clip_and_data, invalid, notes_in_order, parse, span, to_value, unavailable};
use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{EditFailure, transaction};

/// How many zooms `clip_view.note_view` remembers to go back through.
const BACK_STEPS: usize = 32;
/// The fewest key rows the note area shows.
const MIN_ROWS: f64 = 4.0;
/// How many key rows there are.
const KEYS: f64 = 128.0;

/// The narrowest span of time the note area zooms to: a 1/1024 note.
fn min_span() -> Ticks {
    Ticks(Ticks::WHOLE.get() / 1024)
}

/// The view before a zoom: its span of time and its key rows.
type Shown = (Option<(Ticks, Ticks)>, Option<(f64, f64)>);

/// What the note area shows of one MIDI clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteView {
    /// The clip the state belongs to; showing another clip starts afresh.
    clip: Option<ClipId>,
    /// The span in view, in ticks of the clip's contents; `None` shows the
    /// clip's whole span.
    span: Option<(Ticks, Ticks)>,
    /// The rows in view, as the row at the top edge and the row at the
    /// bottom edge; `None` shows the rows the editor lays out by default.
    rows: Option<(f64, f64)>,
    /// The views before each zoom, newest last.
    back: Vec<Shown>,
    /// Whether the editor has stopped following playback until the song
    /// starts or stops (an edit or a scroll by hand pauses it).
    follow_paused: bool,
}

impl NoteView {
    /// The span chosen for `clip`, in ticks of its contents; `None` is the
    /// clip's whole span.
    #[must_use]
    pub fn span(&self, clip: ClipId) -> Option<(Ticks, Ticks)> {
        self.owns(clip).then_some(self.span).flatten()
    }

    /// The rows chosen for `clip`; `None` is the rows the editor lays out by
    /// default.
    #[must_use]
    pub fn rows(&self, clip: ClipId) -> Option<(f64, f64)> {
        self.owns(clip).then_some(self.rows).flatten()
    }

    /// Whether the editor waits to follow playback again.
    #[must_use]
    pub fn follow_paused(&self) -> bool {
        self.follow_paused
    }

    fn owns(&self, clip: ClipId) -> bool {
        self.clip == Some(clip)
    }

    /// Starts afresh for `clip`, if it isn't the clip the state is for.
    fn bind_to(&mut self, clip: ClipId) {
        if self.clip != Some(clip) {
            *self = Self {
                clip: Some(clip),
                ..Self::default()
            };
        }
    }

    /// Drops the rows chosen for `clip` and the zooms before, which counted
    /// them another way: the editor was folded or unfolded.
    pub(super) fn forget_rows(&mut self, clip: ClipId) {
        if self.owns(clip) {
            self.rows = None;
            self.back.clear();
        }
    }

    fn shown(&self) -> Shown {
        (self.span, self.rows)
    }

    /// Remembers the view as it is, to go back to.
    fn push_back(&mut self) {
        if self.back.len() == BACK_STEPS {
            self.back.remove(0);
        }
        self.back.push(self.shown());
    }

    /// Takes a new view, remembering the one before when it differs.
    fn zoom_to(&mut self, span: Option<(Ticks, Ticks)>, rows: Option<(f64, f64)>) {
        if (span, rows) != self.shown() {
            self.push_back();
            self.span = span;
            self.rows = rows;
        }
    }
}

/// The span `window` shows of a clip whose contents run over `whole`: the
/// window moved to lie inside the clip, or `None` once it covers all of it.
fn fit_span(window: (Ticks, Ticks), whole: (Ticks, Ticks)) -> Option<(Ticks, Ticks)> {
    let width = (window.1 - window.0).max(min_span());
    if width >= whole.1 - whole.0 {
        return None;
    }
    let from = window.0.clamp(whole.0, whole.1 - width);
    Some((from, from + width))
}

/// The key rows `rows` show out of `count`: at least [`MIN_ROWS`] and at
/// most all of them, starting no earlier than the first and no later than
/// the last.
fn fit_rows(rows: (f64, f64), count: f64) -> (f64, f64) {
    let height = (rows.1 - rows.0).clamp(MIN_ROWS.min(count.max(1.0)), count.max(1.0));
    let top = rows.0.clamp(0.0, (count - 1.0).max(0.0));
    (top, top + height)
}

/// How many rows the editor has: every key, or the keys that hold notes
/// while it is folded.
fn row_count(midi: &MidiData, folded: bool) -> f64 {
    if folded {
        let mut keys: Vec<u8> = midi.notes.iter().map(|note| note.key).collect();
        keys.sort_unstable();
        keys.dedup();
        keys.len() as f64
    } else {
        KEYS
    }
}

/// The row of `key`: its distance from the top of the keyboard, or from the
/// highest key that holds a note while the editor is folded.
fn row_of(midi: &MidiData, folded: bool, key: u8) -> f64 {
    if folded {
        let mut keys: Vec<u8> = midi.notes.iter().map(|note| note.key).collect();
        keys.sort_unstable_by(|a, b| b.cmp(a));
        keys.dedup();
        keys.iter()
            .position(|shown| *shown == key)
            .map_or(0.0, |row| row as f64)
    } else {
        127.0 - f64::from(key)
    }
}

/// The rows that show the keys from `low` to `high`, a row clear above and
/// below.
fn rows_for(midi: &MidiData, folded: bool, low: u8, high: u8) -> (f64, f64) {
    let top = (row_of(midi, folded, high) - 1.0).max(0.0);
    let bottom = row_of(midi, folded, low) + 2.0;
    (top, bottom)
}

/// The key rows that show the keys from `low` to `high` of a clip's notes, a
/// row clear above and below, as `clip_view.note_view` fits them (H, and
/// double-clicking the piano ruler).
#[must_use]
pub fn fit_key_rows(midi: &MidiData, folded: bool, low: u8, high: u8) -> (f64, f64) {
    fit_rows(rows_for(midi, folded, low, high), row_count(midi, folded))
}

impl ClipView {
    /// What the note area shows.
    #[must_use]
    pub fn note_view(&self) -> &NoteView {
        &self.note_view
    }

    /// Lets the note area follow playback again, or stop until the song
    /// starts or stops.
    pub(crate) fn note_follow(&mut self, paused: bool) {
        self.note_view.follow_paused = paused;
    }

    /// The span of contents the note area shows for `clip`: the chosen one
    /// moved inside the clip, or the clip's whole span.
    #[must_use]
    pub fn note_span(&self, clip: &Clip, midi: &MidiData) -> (Ticks, Ticks) {
        let whole = span(midi, clip.length);
        self.note_view
            .span(clip.id)
            .and_then(|window| fit_span(window, whole))
            .unwrap_or(whole)
    }

    /// Where the song is playing in `clip`'s contents, when the song plays
    /// and the clip is in the Arrangement under the playhead; the loop
    /// carries it back while the clip's loop is on.
    #[must_use]
    pub fn play_position(
        &self,
        doc: &Document,
        arrangement: &ArrangementView,
        clip: ClipId,
    ) -> Option<Ticks> {
        let transport = arrangement.transport();
        let value = doc.clip(clip)?;
        let ClipData::Midi(midi) = &value.data else {
            return None;
        };
        if !transport.playing
            || transport.position < value.start
            || transport.position >= value.end()
        {
            return None;
        }
        let into = midi.start + (transport.position - value.start);
        let end = midi.loop_end.unwrap_or(midi.start + value.length);
        Some(if midi.looping && into >= end && end > midi.loop_start {
            midi.loop_start + Ticks((into - midi.loop_start).get() % (end - midi.loop_start).get())
        } else {
            into
        })
    }
}

/// Whether a command edits the clip the note editor shows, which pauses
/// Follow until playback starts or stops (piano-roll R35).
#[must_use]
pub fn pauses_follow(id: &str) -> bool {
    (ids::handles(id)
        && !matches!(
            id,
            ids::SELECT_NOTES
                | ids::NOTE_FILTERS
                | ids::INVERT_SELECTION
                | ids::QUANTIZE_SETTINGS
                | ids::SELECT_EXPRESSION_BREAKPOINTS
                | ids::MPE_ZOOM
                | ids::COPY
                | ids::FOREGROUND
                | ids::SELECT_LOOP_BARS
                | ids::FOCUS_MODE
                | ids::NOTE_VIEW
        ))
        || matches!(id, SET_MIDI_REGION | SET_MIDI_LOOP)
}

/// Whether `id` is one of this module's commands.
#[must_use]
pub(super) fn handles(id: &str) -> bool {
    matches!(id, ids::NOTE_VIEW | ids::SET_MARKER)
}

/// The report of the view, as `clip_view.note_view` and the commands that
/// move the markers give it.
fn report(doc: &Document, view: &ClipView, arrangement: &ArrangementView) -> Value {
    to_value(&view.state(doc, arrangement))
}

/// Runs `clip_view.note_view` and `clip_view.set_marker`.
pub(super) fn run(
    id: &str,
    args: Value,
    doc: &Document,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    match id {
        ids::NOTE_VIEW => note_view(args, doc, view, arrangement),
        ids::SET_MARKER => set_marker(args, doc, editor, view, arrangement),
        other => Err(invalid(format!("`{other}` isn't a note view command"))),
    }
}

fn note_view(
    args: Value,
    doc: &Document,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    let NoteViewArgs {
        clip,
        show,
        start,
        end,
        top,
        bottom,
    } = parse(args)?;
    let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
    let length = doc.clip(clip).map_or(Ticks::ZERO, |value| value.length);
    let whole = span(&midi, length);
    let folded = view.is_folded(clip);
    let count = row_count(&midi, folded);
    let mpe = view.tab() == ClipTab::Mpe;
    let insert = view.insert_marker(doc, arrangement);
    let selection = view.time_selection(doc, arrangement);
    let selected = view.selection_of(clip).to_vec();
    view.note_view.bind_to(clip);
    let now = view.note_view.span.unwrap_or(whole);
    let shown_span = view.note_view.span;
    let shown_rows = view.note_view.rows;
    match show {
        None => {
            // A click in the editor's scrub area asks for this, and resumes
            // Follow (R35).
            view.note_view.follow_paused = false;
        }
        Some(NoteViewChange::Selection) => {
            let ordered = notes_in_order(&midi);
            let chosen: Vec<_> = selected
                .iter()
                .filter_map(|place| ordered.get(*place as usize))
                .collect();
            let window = selection
                .or_else(|| {
                    Some((
                        chosen.iter().map(|note| note.start).min()?,
                        chosen.iter().map(|note| note.start + note.length).max()?,
                    ))
                })
                .ok_or_else(|| unavailable("make a time selection first"))?;
            if window.0 >= window.1 {
                return Err(unavailable("make a time selection first"));
            }
            // The rows of the selected notes, or of the notes that start
            // inside the time selection; the rows stay with none.
            let rows = if mpe {
                super::mpe::pitch_reach(&midi, selection, &selected)
                    .map(|(low, high)| (127.0 - high - 0.5, 127.0 - low + 1.5))
            } else {
                let inside: Vec<_> = if chosen.is_empty() {
                    ordered
                        .iter()
                        .filter(|note| note.start >= window.0 && note.start < window.1)
                        .collect()
                } else {
                    chosen
                };
                let low = inside.iter().map(|note| note.key).min();
                let high = inside.iter().map(|note| note.key).max();
                low.zip(high)
                    .map(|(low, high)| rows_for(&midi, folded, low, high))
            };
            let rows = rows.map(|rows| fit_rows(rows, count)).or(shown_rows);
            view.note_view.zoom_to(fit_span(window, whole), rows);
        }
        Some(NoteViewChange::Back) => match view.note_view.back.pop() {
            Some((span, rows)) => {
                view.note_view.span = span;
                view.note_view.rows = rows;
            }
            // With no zoom to go back to, the whole clip shows (R32).
            None => {
                view.note_view.span = None;
                view.note_view.rows = None;
            }
        },
        Some(NoteViewChange::Whole) => view.note_view.zoom_to(None, shown_rows),
        Some(NoteViewChange::Height) => {
            let low = midi.notes.iter().map(|note| note.key).min();
            let high = midi.notes.iter().map(|note| note.key).max();
            let (low, high) = low
                .zip(high)
                .ok_or_else(|| unavailable("the clip has no notes"))?;
            let rows = fit_rows(rows_for(&midi, folded, low, high), count);
            view.note_view.zoom_to(shown_span, Some(rows));
        }
        Some(change @ (NoteViewChange::In | NoteViewChange::Out)) => {
            // About the time selection's middle, or the insert marker, or
            // the view's own middle.
            let anchor = selection
                .map(|(from, to)| Ticks((from.get() + to.get()) / 2))
                .or(insert)
                .unwrap_or(Ticks((now.0.get() + now.1.get()) / 2))
                .clamp(whole.0, whole.1);
            let scale = if change == NoteViewChange::In {
                0.5
            } else {
                2.0
            };
            let grow = |edge: Ticks| {
                Ticks(anchor.get() + ((edge - anchor).get() as f64 * scale).round() as i64)
            };
            let window = (grow(now.0), grow(now.1));
            let window = if change == NoteViewChange::In && window.1 - window.0 < min_span() {
                now
            } else {
                window
            };
            view.note_view.zoom_to(fit_span(window, whole), shown_rows);
        }
        Some(change @ (NoteViewChange::Span | NoteViewChange::Scroll | NoteViewChange::Page)) => {
            if start.is_none() && end.is_none() && top.is_none() && bottom.is_none() {
                return Err(invalid(
                    "`span`, `scroll` and `page` need `start` and `end`, or `top` and `bottom`",
                ));
            }
            let window = match (start, end) {
                (Some(start), Some(end)) if start < end => fit_span((start, end), whole),
                (Some(_), Some(_)) => return Err(invalid("`start` must be before `end`")),
                (None, None) => shown_span,
                _ => return Err(invalid("give `start` and `end` together")),
            };
            let rows = match (top, bottom) {
                (Some(top), Some(bottom))
                    if top.is_finite() && bottom.is_finite() && top < bottom =>
                {
                    Some(fit_rows((top, bottom), count))
                }
                (Some(_), Some(_)) => {
                    return Err(invalid("`top` must be a row above `bottom`"));
                }
                (None, None) => shown_rows,
                _ => return Err(invalid("give `top` and `bottom` together")),
            };
            match change {
                NoteViewChange::Span => view.note_view.zoom_to(window, rows),
                _ => {
                    view.note_view.span = window;
                    view.note_view.rows = rows;
                    // Spec: piano-roll R35. Scrolling by hand pauses Follow
                    // until playback starts or stops; Follow turning the
                    // page doesn't.
                    if change == NoteViewChange::Scroll {
                        view.note_view.follow_paused = true;
                    }
                }
            }
        }
    }
    Ok(report(doc, view, arrangement))
}

/// Where `set_marker` puts `marker`: the call's `at`, or the time
/// selection's start (Start marker, loop start) or end (loop end, End
/// marker), or the insert marker, or where the clip is playing.
fn marker_place(
    marker: ClipMarker,
    at: Option<Ticks>,
    clip: ClipId,
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
) -> Result<Ticks, EditFailure> {
    if let Some(at) = at {
        return Ok(at);
    }
    let late = matches!(marker, ClipMarker::LoopEnd | ClipMarker::End);
    view.time_selection(doc, arrangement)
        .map(|(from, to)| if late { to } else { from })
        .or_else(|| view.insert_marker(doc, arrangement))
        .or_else(|| view.play_position(doc, arrangement, clip))
        .ok_or_else(|| {
            unavailable("make a time selection or an insert marker, or play the clip, first")
        })
}

fn set_marker(
    args: Value,
    doc: &Document,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    let SetMarkerArgs { clip, marker, at } = parse(args)?;
    let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
    let at = marker_place(marker, at, clip, doc, view, arrangement)?;
    let (name, command) = match marker {
        ClipMarker::Start => (
            SET_MIDI_REGION,
            Command::SetMidiClipRegion {
                clip,
                start: at,
                end: midi.end,
            },
        ),
        ClipMarker::End => (
            SET_MIDI_REGION,
            Command::SetMidiClipRegion {
                clip,
                start: midi.start,
                end: Some(at),
            },
        ),
        ClipMarker::LoopStart => (
            SET_MIDI_LOOP,
            Command::SetMidiClipLoop {
                clip,
                start: at,
                end: midi.loop_end,
            },
        ),
        ClipMarker::LoopEnd => (
            SET_MIDI_LOOP,
            Command::SetMidiClipLoop {
                clip,
                start: midi.loop_start,
                end: Some(at),
            },
        ),
    };
    let label = if name == SET_MIDI_REGION {
        "Change clip region"
    } else {
        "Change clip loop"
    };
    transaction(editor, label, |editor| {
        editor.apply(command)?;
        Ok(())
    })?;
    Ok(report(editor.document(), view, arrangement))
}

/// The arguments `clip.set_midi_region` takes to move the region of `midi`
/// by `by`, for the UI's Shift-arrow keys: the Start and End markers
/// together.
#[must_use]
pub fn region_moved(clip: ClipId, midi: &MidiData, length: Ticks, by: Ticks) -> Value {
    let end = midi.end.unwrap_or(midi.start + length);
    json!({ "clip": clip, "start": midi.start + by, "end": end + by })
}
