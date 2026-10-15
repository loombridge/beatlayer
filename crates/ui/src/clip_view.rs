//! The Clip View (WP-2.3): the MIDI Note Editor in the detail area.
//!
//! It shows the Arrangement's selected MIDI clip: a key track with the note
//! names down its left edge (L-CLIP-012), and the note area on the clip's
//! editing grid (L-CLIP-013). The clip's contents fill the editor's width:
//! contents tick `MidiData::start`, where the clip starts playing, sits at
//! the clip's start, and the clip's length fills the rest.
//!
//! People select notes by clicking and by dragging a marquee, move them by
//! dragging (in time and in pitch, snapping to the grid, with Cmd — Alt on
//! Windows — bypassing it), resize them by dragging their ends, and, with
//! Draw Mode on (B), add a note by clicking empty grid and remove one by
//! clicking it. Every one runs the note-editing commands
//! ([`bl_shell::note_commands`]), the same ones agents run: a click is an
//! invocation, and a drag sends its command on each frame as a
//! [`UiEvent::Gesture`] measured from where the drag began, so the whole
//! drag is one undo step (the piano roll spec, R11).
//!
//! Nothing here owns the state: whether the view shows, Draw Mode, and the
//! note selection are the shell's. Only the vertical scroll and the drags
//! under way live here.

use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, Document, MidiData, Note};
use bl_shell::arrangement::ArrangementView;
use bl_shell::clip_view::{move_limits, notes_in_order, resize_limit};
use bl_shell::commands::VIEW_LANE;
use bl_shell::note_commands as ids;
use bl_shell::note_commands::NoteField;
use bl_shell::{GesturePhase, Invocation, Shell};
use egui::{Align2, CursorIcon, Id, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2};
use serde_json::{Value, json};

use crate::arrangement::editing::EditKeys;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How wide the key track is, at the note area's left (L-CLIP-012).
pub const KEY_WIDTH: f32 = 27.0;
/// How tall one key's row is.
pub const KEY_HEIGHT: f32 = 8.0;
/// How tall the lane under the note area is (L-CLIP-014).
pub const LANE_HEIGHT: f32 = 43.0;
/// How close to a note's end, in points, a press resizes it rather than
/// moving it.
pub const EDGE_GRAB: f32 = 4.0;
/// The key at the top edge a view with no notes starts at: middle C, C3 in
/// Live's names.
const DEFAULT_TOP: f32 = 60.0;
/// How wide a note's stem in the lane is.
const STEM_WIDTH: f32 = 3.0;

/// The note area: the Clip View without its key track and its lane.
#[must_use]
pub fn grid_rect(view: Rect) -> Rect {
    Rect::from_min_max(
        pos2(view.left() + KEY_WIDTH, view.top()),
        pos2(view.max.x, (view.bottom() - LANE_HEIGHT).max(view.top())),
    )
}

/// The lane under the note area, as wide as the Clip View: its labels sit
/// left of the note area, under the key track (L-CLIP-014).
#[must_use]
pub fn lane_rect(view: Rect) -> Rect {
    Rect::from_min_max(
        pos2(view.left(), (view.bottom() - LANE_HEIGHT).max(view.top())),
        view.max,
    )
}

/// The keys the note editor shows, top to bottom: every key, or only the
/// rows that hold notes while it's folded (F).
#[must_use]
pub fn shown_keys(folded: bool, notes: &[Note]) -> Vec<u8> {
    if !folded {
        return (0..=127u8).rev().collect();
    }
    let mut keys: Vec<u8> = notes.iter().map(|note| note.key).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.reverse();
    keys
}

/// Where a key's row starts in `grid`, with `top` the key at the grid's
/// top edge: the unfolded editor's geometry.
#[must_use]
pub fn y_of_key(grid: Rect, top: f32, key: u8) -> f32 {
    grid.top() + (top - f32::from(key)) * KEY_HEIGHT
}

/// The keys the editor shows and where they sit: the whole keyboard, or
/// only the rows that hold notes while folded.
#[derive(Clone, Debug, PartialEq)]
struct KeyRows {
    /// The keys, top to bottom.
    keys: Vec<u8>,
    /// Whether the editor is folded.
    folded: bool,
    /// The key at the grid's top edge, continuous, while unfolded; the
    /// wheel moves it (R12).
    top: f32,
    /// The row at the grid's top edge, continuous, while folded.
    first: f32,
}

impl KeyRows {
    fn of(folded: bool, notes: &[Note], top: f32, first: f32) -> Self {
        Self {
            keys: shown_keys(folded, notes),
            folded,
            top,
            first,
        }
    }

    /// Where the row of `key` starts in `grid`, if it shows.
    fn y_of(&self, grid: Rect, key: u8) -> Option<f32> {
        if !self.folded {
            return Some(y_of_key(grid, self.top, key));
        }
        let row = self.keys.iter().position(|shown| *shown == key)?;
        Some(self.y_of_row(grid, row))
    }

    /// Where the row at `row` starts in `grid`.
    fn y_of_row(&self, grid: Rect, row: usize) -> f32 {
        grid.top() + (row as f32 - self.top_row()) * KEY_HEIGHT
    }

    /// The key at `y` in `grid`: continuous while unfolded, the shown row
    /// whose band holds `y` while folded. `None` while folded with no rows.
    fn key_at(&self, grid: Rect, y: f32) -> Option<f32> {
        if self.keys.is_empty() {
            return None;
        }
        let row = (y - grid.top()) / KEY_HEIGHT + self.top_row();
        if !self.folded {
            return Some(127.0 - row);
        }
        let row = row.floor().clamp(0.0, (self.keys.len() - 1) as f32);
        self.keys.get(row as usize).map(|key| f32::from(*key))
    }

    /// The continuous row index at the grid's top edge.
    fn top_row(&self) -> f32 {
        if self.folded {
            self.first
        } else {
            127.0 - self.top
        }
    }

    /// How far the wheel may scroll: the last row's index.
    fn scroll_room(&self) -> f32 {
        ((self.keys.len().max(1) - 1) as f32).max(0.0)
    }
}

/// Where a contents tick falls, x, in `grid`: contents tick `start` sits at
/// the clip's start, and the clip's `length` fills the grid.
#[must_use]
pub fn x_of_tick(grid: Rect, start: Ticks, length: Ticks, tick: Ticks) -> f32 {
    let span = length.get().max(1) as f64;
    let from = (tick - start).get() as f64;
    grid.left() + (from / span) as f32 * grid.width()
}

/// The contents tick under `x` in `grid`.
#[must_use]
pub fn tick_at(grid: Rect, start: Ticks, length: Ticks, x: f32) -> Ticks {
    let span = length.get().max(1) as f64;
    let at = f64::from(x - grid.left()) / f64::from(grid.width().max(1.0)) * span;
    start + Ticks(at.round() as i64)
}

/// A note's rectangle in `grid`, for an editor whose top edge is the key
/// `top`, unfolded.
#[must_use]
pub fn note_rect(grid: Rect, start: Ticks, length: Ticks, top: f32, note: &Note) -> Rect {
    note_rect_at(grid, start, length, y_of_key(grid, top, note.key), note)
}

/// A note's rectangle in `grid`, its row starting at `y`.
fn note_rect_at(grid: Rect, start: Ticks, length: Ticks, y: f32, note: &Note) -> Rect {
    let x0 = x_of_tick(grid, start, length, note.start);
    let x1 = x_of_tick(grid, start, length, note.start + note.length);
    Rect::from_min_max(pos2(x0, y), pos2(x1.max(x0 + 2.0), y + KEY_HEIGHT))
}

/// A note's value of `field`, as the lane shows and edits it.
#[must_use]
fn note_value(note: &Note, field: NoteField) -> f64 {
    match field {
        NoteField::Velocity => f64::from(note.velocity),
        NoteField::ReleaseVelocity => f64::from(note.release_velocity),
        NoteField::Chance => note.probability,
        NoteField::VelocityDeviation => f64::from(note.velocity_deviation),
    }
}

/// Where a value of `field` sits in `lane`: the field's highest at the
/// lane's top edge, its lowest at the bottom.
#[must_use]
pub fn y_of_value(lane: Rect, field: NoteField, value: f64) -> f32 {
    let (low, high) = field.range();
    let at = ((value - low) / (high - low)).clamp(0.0, 1.0) as f32;
    lane.bottom() - at * lane.height()
}

/// The value of `field` at `y` in `lane`, clamped to the field's range.
#[must_use]
fn value_at(lane: Rect, field: NoteField, y: f32) -> f64 {
    let (low, high) = field.range();
    let at = f64::from((lane.bottom() - y) / lane.height().max(1.0));
    (low + (high - low) * at).clamp(low, high)
}

/// The value of `field` at `y` in `lane`, clamped to the field's range and
/// whole for the whole-number fields, as their commands take it.
#[must_use]
fn lane_value(lane: Rect, field: NoteField, y: f32) -> f64 {
    let value = value_at(lane, field, y);
    if field.whole() { value.round() } else { value }
}

/// The lane's three height labels, top to bottom: the field's highest
/// value, its middle, and its lowest (L-CLIP-014: 127, 64, 1).
fn lane_labels(field: NoteField) -> [&'static str; 3] {
    match field {
        NoteField::Velocity | NoteField::ReleaseVelocity => ["127", "64", "1"],
        NoteField::Chance => ["100%", "50%", "0%"],
        NoteField::VelocityDeviation => ["+127", "0", "-127"],
    }
}

/// Where a note's stem starts, x, in `grid`: at the note's start, one point
/// right of it, as the lane paints it.
#[must_use]
fn stem_x(grid: Rect, start: Ticks, length: Ticks, note: &Note) -> f32 {
    x_of_tick(grid, start, length, note.start) + 1.0
}

/// The notes whose stems a drag from `from_x` to `to_x` runs over, in the
/// clip's note order.
fn covered_notes(
    grid: Rect,
    start: Ticks,
    length: Ticks,
    notes: &[Note],
    from_x: f32,
    to_x: f32,
) -> Vec<u32> {
    let low = from_x.min(to_x) - STEM_WIDTH / 2.0;
    let high = from_x.max(to_x) + STEM_WIDTH / 2.0;
    notes
        .iter()
        .enumerate()
        .filter(|(_, note)| {
            let stem = stem_x(grid, start, length, note) + STEM_WIDTH / 2.0;
            (low..=high).contains(&stem)
        })
        .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
        .collect()
}

/// A note's name as Live writes it: C3 is middle C (MIDI key 60).
fn key_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i32::from(key) / 12 - 2;
    format!("{}{octave}", NAMES[usize::from(key) % 12])
}

/// Where on a note the pointer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoteZone {
    /// The note's body, which moves it.
    Body,
    /// Its end, which resizes it.
    End,
}

/// What a drag in the note area is doing.
#[derive(Clone, Debug, PartialEq)]
enum NoteDrag {
    /// Dragging notes in time and pitch.
    Move {
        /// The clip the drag began on.
        clip: ClipId,
        /// The notes, by their places in the clip's notes in order.
        places: Vec<u32>,
        /// The grabbed note's start when the drag began, the key under the
        /// pointer then (continuous), and where the grab fell.
        start: Ticks,
        grab_key: f32,
        grab: Ticks,
        /// The furthest the drag may move them: earlier in time, and in
        /// pitch.
        limits: (Ticks, i32, i32),
    },
    /// Dragging the notes' ends.
    Resize {
        clip: ClipId,
        places: Vec<u32>,
        /// The grabbed note's end when the drag began, and where the grab
        /// fell.
        end: Ticks,
        grab: Ticks,
        /// How much shorter the drag may make them, at least.
        limit: Ticks,
    },
}

/// The key at the top edge a view with notes starts at: the highest note
/// and two rows above it.
fn default_top(midi: &MidiData) -> f32 {
    let highest = midi.notes.iter().map(|note| note.key).max();
    highest.map_or(DEFAULT_TOP, |key| (f32::from(key) + 2.0).min(127.0))
}

/// A note under a point in the note area.
fn hit_note(
    grid: Rect,
    start: Ticks,
    length: Ticks,
    rows: &KeyRows,
    notes: &[Note],
    pos: Pos2,
) -> Option<(u32, NoteZone)> {
    if !grid.contains(pos) {
        return None;
    }
    // Later notes draw over earlier ones, so the last one wins.
    notes.iter().enumerate().rev().find_map(|(place, note)| {
        let rect = rows
            .y_of(grid, note.key)
            .map(|y| note_rect_at(grid, start, length, y, note))?;
        rect.contains(pos).then(|| {
            let grab = EDGE_GRAB.min(rect.width() / 4.0);
            let zone = if rect.right() - pos.x <= grab {
                NoteZone::End
            } else {
                NoteZone::Body
            };
            (u32::try_from(place).unwrap_or(u32::MAX), zone)
        })
    })
}

/// The notes a drag grabbed: the selection when the grabbed note is in it,
/// otherwise the grabbed note alone, which becomes the selection
/// (arrangement-editing R4 does the same for clips).
fn grabbed(
    shell: &Shell,
    doc: &Document,
    arrangement: &ArrangementView,
    clip: ClipId,
    place: u32,
    events: &mut Vec<UiEvent>,
) -> Vec<u32> {
    let selected = shell.clip_view().selected_notes(doc, arrangement);
    if selected.contains(&place) {
        selected.to_vec()
    } else {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SELECT_NOTES,
            json!({ "clip": clip, "notes": [place], "mode": "replace" }),
        )));
        vec![place]
    }
}

/// The notes at these places.
fn notes_at(notes: &[Note], places: &[u32]) -> Vec<Note> {
    places
        .iter()
        .filter_map(|place| notes.get(*place as usize).copied())
        .collect()
}

fn gesture(events: &mut Vec<UiEvent>, phase: GesturePhase, id: &str, args: Value) {
    events.push(UiEvent::Gesture {
        phase,
        invocation: Some(Invocation::new(id, args)),
    });
}

/// What a drag in the lane under the note area is doing (WP-2.3).
#[derive(Clone, Copy, Debug, PartialEq)]
struct LaneDrag {
    /// Where the press fell.
    from: Pos2,
    /// The value at the press's height, where a ramp starts.
    value: f64,
}

/// The Clip View's own state: the vertical scroll, the lane's drag, the
/// note drags, and the marquee.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipViewUi {
    /// The clip the editor was last laid out for.
    shown: Option<ClipId>,
    /// Whether the editor was folded when it was last laid out for that
    /// clip.
    folded: bool,
    /// The key at the note area's top edge, continuous, while unfolded.
    top: f32,
    /// The row at the note area's top edge, continuous, while folded.
    first: f32,
    /// The drag under way in the note area, if any.
    drag: Option<NoteDrag>,
    /// The drag under way in the lane, if any.
    lane_drag: Option<LaneDrag>,
    /// The marquee under way: where it began, and where it is now.
    marquee: Option<(Pos2, Pos2)>,
}

impl Default for ClipViewUi {
    fn default() -> Self {
        Self {
            shown: None,
            folded: false,
            top: DEFAULT_TOP,
            first: 0.0,
            drag: None,
            lane_drag: None,
            marquee: None,
        }
    }
}

impl ClipViewUi {
    /// The key at the note area's top edge, while unfolded.
    #[must_use]
    pub fn top(&self) -> f32 {
        self.top
    }

    /// The row at the note area's top edge, while folded.
    #[must_use]
    pub fn first_row(&self) -> f32 {
        self.first
    }

    /// Shows the Clip View in `rect`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let region = ui.interact(rect, Id::new("region-clip-view"), Sense::hover());
        ui.ctx().accesskit_node_builder(region.id, |node| {
            node.set_role(egui::accesskit::Role::Group);
            node.set_label("Clip View".to_owned());
            node.set_bounds(egui::accesskit::Rect {
                x0: rect.min.x.into(),
                y0: rect.min.y.into(),
                x1: rect.max.x.into(),
                y1: rect.max.y.into(),
            });
        });
        ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);

        let doc = shell.editor().document();
        let shown = shell.clip_view().shown_clip(doc, shell.arrangement());
        let editor = shown
            .and_then(|id| doc.clip(id))
            .and_then(|clip| match &clip.data {
                ClipData::Midi(midi) => Some((clip.id, midi.clone(), clip.length)),
                ClipData::Audio(_) => None,
            });
        let Some((clip, midi, length)) = editor else {
            self.shown = None;
            self.drag = None;
            self.lane_drag = None;
            self.marquee = None;
            return;
        };
        let folded = shell.clip_view().is_folded(clip);
        if self.shown != Some(clip) || self.folded != folded {
            // Another clip shows, or the editor was folded or unfolded:
            // scroll to its notes.
            self.shown = Some(clip);
            self.folded = folded;
            self.top = default_top(&midi);
            self.first = 0.0;
            self.drag = None;
            self.lane_drag = None;
            self.marquee = None;
        }
        let grid = grid_rect(rect);
        if grid.width() < 1.0 || grid.height() < KEY_HEIGHT {
            return;
        }
        let notes = notes_in_order(&midi);
        let rows = KeyRows::of(folded, &notes, self.top, self.first);
        let selected = shell
            .clip_view()
            .selected_notes(doc, shell.arrangement())
            .to_vec();
        self.paint(
            ui, shell, theme, clip, rect, grid, &rows, &midi, length, &notes, &selected,
        );
        self.input(
            ui,
            shell,
            doc,
            shell.arrangement(),
            rect,
            grid,
            &rows,
            clip,
            &midi,
            length,
            &notes,
            events,
        );
    }

    /// Paints the key track, the grid, the notes, the lane, and the
    /// marquee.
    #[expect(clippy::too_many_arguments, reason = "the editor's whole layout")]
    fn paint(
        &self,
        ui: &Ui,
        shell: &Shell,
        theme: &Theme,
        clip: ClipId,
        rect: Rect,
        grid: Rect,
        rows: &KeyRows,
        midi: &MidiData,
        length: Ticks,
        notes: &[Note],
        selected: &[u32],
    ) {
        let painter = ui.painter();
        let colours = &theme.colors;
        let start = midi.start;

        // The key track: white and black keys, each named when its row has
        // the room, the C rows otherwise. Folded, only the rows that hold
        // notes show, one under the next (F).
        let named = |key: u8| KEY_HEIGHT >= 7.0 || key.is_multiple_of(12);
        for (row, key) in rows.keys.iter().enumerate() {
            let y0 = rows.y_of_row(grid, row);
            if y0 + KEY_HEIGHT < grid.top() || y0 > grid.bottom() {
                continue;
            }
            let key = *key;
            let rect = Rect::from_min_max(
                pos2(grid.left() - KEY_WIDTH, y0),
                pos2(grid.left(), y0 + KEY_HEIGHT),
            );
            let black = matches!(key % 12, 1 | 3 | 6 | 8 | 10);
            painter.rect_filled(
                rect,
                0.0,
                if black {
                    colours.bar.0
                } else {
                    colours.control.0
                },
            );
            painter.line_segment(
                [rect.left_bottom(), rect.right_bottom()],
                Stroke::new(1.0, colours.lane_line.0),
            );
            if named(key) {
                painter.text(
                    pos2(rect.right() - 2.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    key_name(key),
                    theme.small_font(),
                    colours.text_dim.0,
                );
            }
        }

        // The clip's contents: the grid lines, then the notes over them.
        painter.rect_filled(grid, 0.0, colours.lane.0);
        let doc = shell.editor().document();
        let step = shell.arrangement().grid_ticks(doc);
        let px = (ui.ctx().pixels_per_point() * 0.5).max(0.5);
        let mut at = start - Ticks(start.get().rem_euclid(step.get()));
        while at <= start + length {
            let x = x_of_tick(grid, start, length, at);
            let bar = at.get().rem_euclid(Ticks::WHOLE.get()) == 0;
            painter.rect_filled(
                Rect::from_min_max(pos2(x, grid.top()), pos2(x + px, grid.bottom())),
                0.0,
                if bar {
                    colours.grid_bar.0
                } else {
                    colours.grid_beat.0
                },
            );
            at += step;
        }
        for row in 0..rows.keys.len() {
            let y = rows.y_of_row(grid, row) + KEY_HEIGHT;
            if y < grid.top() || y > grid.bottom() {
                continue;
            }
            painter.rect_filled(
                Rect::from_min_max(pos2(grid.left(), y), pos2(grid.right(), y + px)),
                0.0,
                colours.lane_line.0,
            );
        }
        let face = doc.clip_track(clip).map_or(colours.accent.0, |track| {
            crate::arrangement::scene::track_colour(theme, track)
        });
        for (place, note) in notes.iter().enumerate() {
            let Some(y) = rows.y_of(grid, note.key) else {
                continue;
            };
            let rect = note_rect_at(grid, start, length, y, note);
            if rect.right() < grid.left() || rect.left() > grid.right() {
                continue;
            }
            let corner = theme.sizes.corner.min(2.0);
            painter.rect_filled(rect, corner, face);
            if selected.contains(&u32::try_from(place).unwrap_or(u32::MAX)) {
                // Spec: arrangement-editing R15's selection colour, as the
                // Arrangement's selected clips wear.
                painter.rect_stroke(
                    rect,
                    corner,
                    Stroke::new(1.5, colours.time_selection.0),
                    StrokeKind::Inside,
                );
            }
        }
        if let Some((from, to)) = self.marquee {
            let marquee = Rect::from_two_pos(from, to).intersect(grid);
            painter.rect_filled(marquee, 0.0, colours.time_selection.0.gamma_multiply(0.2));
            painter.rect_stroke(
                marquee,
                0.0,
                Stroke::new(1.0, colours.time_selection.0),
                StrokeKind::Inside,
            );
        }
        self.paint_lane(
            ui,
            theme,
            rect,
            grid,
            start,
            length,
            notes,
            selected,
            face,
            shell.clip_view().lane(),
        );
    }

    /// Paints the lane under the note area: the shown value's stem for each
    /// note, over the scale's labels (L-CLIP-014).
    #[expect(clippy::too_many_arguments, reason = "the lane's whole layout")]
    fn paint_lane(
        &self,
        ui: &Ui,
        theme: &Theme,
        view: Rect,
        grid: Rect,
        start: Ticks,
        length: Ticks,
        notes: &[Note],
        selected: &[u32],
        face: egui::Color32,
        field: NoteField,
    ) {
        let painter = ui.painter();
        let colours = &theme.colors;
        let lane = lane_rect(view);
        painter.rect_filled(lane, 0.0, colours.panel.0);
        painter.line_segment(
            [lane.left_top(), lane.right_top()],
            Stroke::new(1.0, colours.lane_line.0),
        );
        // The lane's three heights, named in its left column: 127, 64 and 1
        // for velocity (L-CLIP-014).
        let (low, high) = field.range();
        for (value, label) in [
            (high, lane_labels(field)[0]),
            ((low + high) / 2.0, lane_labels(field)[1]),
            (low, lane_labels(field)[2]),
        ] {
            // The highest and lowest labels sit on the lane's edges, so
            // they keep half a row inside it.
            let y = y_of_value(lane, field, value).clamp(
                lane.top() + KEY_HEIGHT / 2.0,
                lane.bottom() - KEY_HEIGHT / 2.0,
            );
            painter.text(
                pos2(grid.left() - 4.0, y),
                Align2::RIGHT_CENTER,
                label,
                theme.small_font(),
                colours.text_dim.0,
            );
        }
        for (place, note) in notes.iter().enumerate() {
            let x = stem_x(grid, start, length, note);
            if x + STEM_WIDTH < grid.left() || x > grid.right() {
                continue;
            }
            let top = y_of_value(lane, field, note_value(note, field));
            let stem = Rect::from_min_max(pos2(x, top), pos2(x + STEM_WIDTH, lane.bottom()));
            let chosen = selected.contains(&u32::try_from(place).unwrap_or(u32::MAX));
            painter.rect_filled(
                stem,
                0.0,
                if chosen {
                    colours.time_selection.0
                } else {
                    face
                },
            );
        }
    }

    /// The note area's and lane's clicks, drags, and wheel.
    #[expect(clippy::too_many_arguments, reason = "the editor's whole input")]
    fn input(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        doc: &Document,
        arrangement: &ArrangementView,
        view: Rect,
        grid: Rect,
        rows: &KeyRows,
        clip: ClipId,
        midi: &MidiData,
        length: Ticks,
        notes: &[Note],
        events: &mut Vec<UiEvent>,
    ) {
        let response = ui.interact(
            grid,
            Id::new(("clip-view-grid", clip)),
            Sense::click_and_drag(),
        );
        let keys = EditKeys::from(ui.input(|i| i.modifiers));
        let draw_mode = shell.clip_view().draw_mode();
        let start = midi.start;
        let lane = lane_rect(view);
        let field = shell.clip_view().lane();
        let hit = |pos: Pos2| hit_note(grid, start, length, rows, notes, pos);

        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && !draw_mode
        {
            match hit(press) {
                // Spec: piano-roll R9. Dragging a note's end resizes it,
                // keeping at least one grid step, or a tick with snapping
                // off.
                Some((place, NoteZone::End)) => {
                    let places = grabbed(shell, doc, arrangement, clip, place, events);
                    let note = notes[place as usize];
                    let end = note.start + note.length;
                    let shortest = if arrangement.grid().snap {
                        arrangement.grid_ticks(doc)
                    } else {
                        Ticks(1)
                    };
                    self.drag = Some(NoteDrag::Resize {
                        clip,
                        limit: resize_limit(&notes_at(notes, &places), shortest),
                        places,
                        end,
                        grab: tick_at(grid, start, length, press.x) - end,
                    });
                }
                // Spec: piano-roll R7. Dragging moves the grabbed note, or
                // the selection it belongs to.
                Some((place, NoteZone::Body)) => {
                    let places = grabbed(shell, doc, arrangement, clip, place, events);
                    let note = notes[place as usize];
                    self.drag = Some(NoteDrag::Move {
                        clip,
                        limits: move_limits(&notes_at(notes, &places)),
                        places,
                        start: note.start,
                        grab_key: rows
                            .key_at(grid, press.y)
                            .unwrap_or_else(|| f32::from(note.key)),
                        grab: tick_at(grid, start, length, press.x) - note.start,
                    });
                }
                // Spec: piano-roll R6. Anywhere else a marquee selects.
                None => self.marquee = Some((press, press)),
            }
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        if response.dragged() {
            if let (Some(drag), Some(pos)) = (self.drag.as_ref(), pointer) {
                let (id, args) =
                    drag.invocation(arrangement, doc, keys, grid, rows, start, length, pos);
                gesture(events, GesturePhase::Update, id, args);
            } else if let (Some((from, _)), Some(pos)) = (self.marquee, pointer) {
                self.marquee = Some((from, pos));
                let marquee = Rect::from_two_pos(from, pos);
                let inside: Vec<u32> = notes
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| {
                        rows.y_of(grid, note.key)
                            .map(|y| note_rect_at(grid, start, length, y, note))
                            .is_some_and(|rect| rect.intersects(marquee))
                    })
                    .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
                    .collect();
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::SELECT_NOTES,
                    json!({ "clip": clip, "notes": inside, "mode": "replace" }),
                )));
            }
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            let note = self.drag.take().is_some();
            let lane = self.lane_drag.take().is_some();
            if note || lane {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Cancel,
                    invocation: None,
                });
            }
        }
        if response.drag_stopped() {
            let last = match (self.drag.as_ref(), pointer) {
                (Some(drag), Some(pos)) => {
                    let (id, args) =
                        drag.invocation(arrangement, doc, keys, grid, rows, start, length, pos);
                    Some(Invocation::new(id, args))
                }
                _ => None,
            };
            if self.drag.take().is_some() {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: last,
                });
            }
            self.marquee = None;
        }
        if let Some(hover) = response.hover_pos()
            && !draw_mode
            && let Some((_, zone)) = hit(hover)
        {
            ui.ctx().set_cursor_icon(match zone {
                NoteZone::End => CursorIcon::ResizeHorizontal,
                NoteZone::Body => CursorIcon::Grab,
            });
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let place = hit(pos).map(|(place, _)| place);
            match (draw_mode, place) {
                // Spec: piano-roll R8. With Draw Mode on, clicking a note
                // removes it and clicking empty grid draws one grid step
                // long.
                (true, Some(place)) => events.push(UiEvent::Invoke(Invocation::new(
                    ids::DELETE_NOTES,
                    json!({ "clip": clip, "notes": [place] }),
                ))),
                (true, None) => {
                    let step = arrangement.grid_ticks(doc);
                    let at = arrangement.snap_drag(
                        doc,
                        tick_at(grid, start, length, pos.x),
                        keys.bypass,
                    );
                    // Folded, a click draws on the rows that show.
                    if let Some(key) = rows.key_at(grid, pos.y) {
                        let key = key.round().clamp(0.0, 127.0) as u8;
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::ADD_NOTE,
                            json!({ "clip": clip, "key": key, "start": at, "length": step }),
                        )));
                    }
                }
                // Spec: piano-roll R5. With Draw Mode off, clicking a note
                // selects it alone, Cmd-click (Ctrl-click) adds or removes
                // it, and clicking empty grid clears the selection.
                (false, Some(place)) => events.push(UiEvent::Invoke(Invocation::new(
                    ids::SELECT_NOTES,
                    json!({
                        "clip": clip,
                        "notes": [place],
                        "mode": if keys.toggle { "toggle" } else { "replace" },
                    }),
                ))),
                (false, None) => events.push(UiEvent::Invoke(Invocation::new(
                    ids::SELECT_NOTES,
                    json!({ "clip": clip, "notes": [], "mode": "replace" }),
                ))),
            }
        }
        // Spec: piano-roll. The lane under the note area: dragging sets a
        // note's value, or draws a ramp across several. Its left column is
        // the lane header, whose menu chooses the value.
        let plot = Rect::from_min_max(pos2(grid.left(), lane.top()), lane.max);
        let lane_response = ui.interact(
            plot,
            Id::new(("clip-view-lane", clip)),
            Sense::click_and_drag(),
        );
        if lane_response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            self.lane_drag = Some(LaneDrag {
                from: press,
                value: lane_value(lane, field, press.y),
            });
        }
        if lane_response.dragged()
            && let (Some(drag), Some(pos)) = (self.lane_drag, pointer)
            && let Some((id, args)) =
                drag.invocation(clip, grid, field, lane, start, length, notes, pos)
        {
            gesture(events, GesturePhase::Update, id, args);
        }
        if lane_response.drag_stopped() {
            let last = match (self.lane_drag, pointer) {
                (Some(drag), Some(pos)) => drag
                    .invocation(clip, grid, field, lane, start, length, notes, pos)
                    .map(|(id, args)| Invocation::new(id, args)),
                _ => None,
            };
            if self.lane_drag.take().is_some() {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: last,
                });
            }
        }
        if let Some(hover) = lane_response.hover_pos()
            && !covered_notes(grid, start, length, notes, hover.x, hover.x).is_empty()
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        // Spec: piano-roll; L-CLIP-014. The lane header's context menu
        // chooses which of a note's values the lane shows.
        let header = Rect::from_min_max(lane.min, pos2(grid.left(), lane.bottom()));
        let header_response = ui.interact(
            header,
            Id::new(("clip-view-lane-header", clip)),
            Sense::click(),
        );
        header_response.context_menu(|ui| {
            for choice in NoteField::ALL {
                let on = field == choice;
                let response = ui.selectable_label(on, choice.name());
                crate::menus::register_item(ui.ctx(), &response);
                if response.clicked() {
                    events.push(UiEvent::Invoke(Invocation::new(
                        VIEW_LANE,
                        json!({ "lane": choice }),
                    )));
                    ui.close();
                }
            }
        });
        // The wheel scrolls the key track.
        if ui.rect_contains_pointer(grid) {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if rows.folded {
                    self.first = (self.first + scroll / KEY_HEIGHT).clamp(0.0, rows.scroll_room());
                } else {
                    self.top = (self.top + scroll / KEY_HEIGHT).clamp(0.0, 127.0);
                }
            }
        }
    }
}

impl NoteDrag {
    /// The drag's command for the pointer at `pos`, measured from where the
    /// drag began.
    #[expect(clippy::too_many_arguments, reason = "the drag's whole geometry")]
    fn invocation(
        &self,
        arrangement: &ArrangementView,
        doc: &Document,
        keys: EditKeys,
        grid: Rect,
        rows: &KeyRows,
        start: Ticks,
        length: Ticks,
        pos: Pos2,
    ) -> (&'static str, Value) {
        let at = tick_at(grid, start, length, pos.x);
        match self {
            Self::Move {
                clip,
                places,
                start: from,
                grab_key,
                grab,
                limits,
            } => {
                // Spec: piano-roll R10. With snapping on, the grabbed note's
                // start lands on the grid, and Cmd (Alt) bypasses it.
                let snapped = arrangement.snap_drag(doc, at - *grab, keys.bypass);
                let by = (snapped - *from).max(limits.0);
                let key = rows.key_at(grid, pos.y).unwrap_or(*grab_key);
                let semitones = ((key - *grab_key).round() as i32).clamp(limits.1, limits.2);
                (
                    ids::MOVE_NOTES,
                    json!({ "clip": clip, "notes": places, "by": by, "semitones": semitones }),
                )
            }
            Self::Resize {
                clip,
                places,
                end,
                grab,
                limit,
            } => {
                let snapped = arrangement.snap_drag(doc, at - *grab, keys.bypass);
                let by = (snapped - *end).max(*limit);
                (
                    ids::RESIZE_NOTES,
                    json!({ "clip": clip, "notes": places, "by": by }),
                )
            }
        }
    }
}

impl LaneDrag {
    /// The lane's command for the pointer at `pos`: the one note the drag
    /// covers has its value set, and several take a ramp from where the
    /// drag began (piano-roll R14). `None` when it covers no note.
    #[expect(clippy::too_many_arguments, reason = "the lane's whole geometry")]
    fn invocation(
        &self,
        clip: ClipId,
        grid: Rect,
        field: NoteField,
        lane: Rect,
        start: Ticks,
        length: Ticks,
        notes: &[Note],
        pos: Pos2,
    ) -> Option<(&'static str, Value)> {
        let places = covered_notes(grid, start, length, notes, self.from.x, pos.x);
        let value = lane_value(lane, field, pos.y);
        match places.as_slice() {
            [] => None,
            [place] => Some((
                ids::SET_VALUE,
                json!({ "clip": clip, "notes": [place], "field": field, "value": value }),
            )),
            _ => {
                // The ramp runs from where the drag began to the pointer:
                // dragging leftwards ramps the other way.
                let (from, to) = if pos.x >= self.from.x {
                    (self.value, value)
                } else {
                    (value, self.value)
                };
                Some((
                    ids::RAMP_VALUE,
                    json!({
                        "clip": clip,
                        "notes": places,
                        "field": field,
                        "from": from,
                        "to": to,
                    }),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 300.0))
    }

    fn note(key: u8, start: i64) -> Note {
        Note::new(key, Ticks(start), Ticks(10), 100)
    }

    #[test]
    fn the_key_track_rows_sit_where_the_notes_show() {
        // Spec: piano-roll R12, R16. A key's row is where `y_of_key` puts
        // the key, however the wheel has scrolled the editor: the key track,
        // the grid lines and the notes share one geometry. Folded, a row is
        // the key's place among the rows that show.
        let notes = [note(60, 0), note(64, 40)];
        for top in [60.0f32, 127.0, 0.0, 62.5] {
            let rows = KeyRows::of(false, &notes, top, 0.0);
            for key in [0u8, 40, 60, 64, 127] {
                let row = rows.keys.iter().position(|shown| *shown == key).unwrap();
                assert_eq!(
                    rows.y_of_row(grid(), row),
                    y_of_key(grid(), top, key),
                    "key {key} with top {top}"
                );
            }
        }
        let rows = KeyRows::of(true, &notes, 60.0, 0.0);
        assert_eq!(rows.keys, vec![64u8, 60]);
        assert_eq!(rows.y_of_row(grid(), 0), grid().top());
        assert_eq!(rows.y_of_row(grid(), 1), grid().top() + KEY_HEIGHT);
    }

    #[test]
    fn a_drag_covers_every_stem_it_runs_over() {
        // Spec: piano-roll R15. A note's stem is STEM_WIDTH wide, a point
        // right of the note's start: a press anywhere on the stem covers the
        // note, and one clear of every stem covers none.
        let notes = [note(60, 0), note(64, 40)];
        let (start, length) = (Ticks::ZERO, Ticks(80));
        for (place, note) in notes.iter().enumerate() {
            let left = stem_x(grid(), start, length, note);
            for x in [left, left + STEM_WIDTH / 2.0, left + STEM_WIDTH] {
                assert_eq!(
                    covered_notes(grid(), start, length, &notes, x, x),
                    [u32::try_from(place).unwrap()],
                    "a press at {x}"
                );
            }
            let clear = left - KEY_WIDTH;
            assert!(
                covered_notes(grid(), start, length, &notes, clear, clear).is_empty(),
                "a press at {clear} covers nothing"
            );
        }
    }
}
