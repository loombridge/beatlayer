//! The Clip View (WP-2.3): the MIDI Note Editor in the detail area.
//!
//! It shows the Arrangement's selected MIDI clip: a key track with the note
//! names down its left edge (L-CLIP-012), and the note area on the clip's
//! editing grid (L-CLIP-013). The clip's contents fill the editor's width:
//! contents tick `MidiData::start`, where the clip starts playing, sits at
//! the clip's start, and the clip's length fills the rest. Above the note
//! area, the editor ruler and marker strip (L-CLIP-011) holds the clip's
//! Start and End markers and its loop brace: dragging a marker or the brace
//! edits the clip through the document's own commands, in the clip's
//! contents ticks, snapping to the editing grid (Cmd — Alt on Windows —
//! bypasses it).
//!
//! People select notes by clicking and by dragging a marquee, move them by
//! dragging (in time and in pitch, snapping to the grid, with Cmd — Alt on
//! Windows — bypassing it), resize them by dragging their ends, and, with
//! Draw Mode on (B), add a note by clicking empty grid and remove one by
//! clicking it. Every one runs the note-editing commands
//! ([`bl_shell::note_commands`]), the same ones agents run: a click is an
//! invocation, and a drag sends its command on each frame as a
//! [`UiEvent::Gesture`] measured from where the drag began, so the whole
//! drag is one undo step (the piano roll spec, R11). Up and Down transpose
//! the selected notes by a semitone, and with Shift by an octave
//! (S-A11Y-008, S-A11Y-009), running `note.move`.
//!
//! Under the note area sit the expression lanes and the footer
//! ([`crate::note_lanes`]): the Velocity, Chance and Release Velocity lanes,
//! whose markers and controls run the same value commands.
//!
//! Nothing here owns the state: whether the view shows, Draw Mode, the note
//! selection, the lanes shown and the Quantize settings are the shell's. Only
//! the vertical scroll, the lanes' heights and the drags under way live here.

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::registry::{SET_MIDI_LOOP, SET_MIDI_REGION};
use bl_model::{ClipData, ClipId, Document, MidiData, Note};
use bl_shell::arrangement::ArrangementView;
use bl_shell::arrangement_commands::{AdaptiveGrid, FixedGrid};
use bl_shell::clip_view::{move_limits, notes_in_order, resize_limit, span};
use bl_shell::context_menus;
use bl_shell::menus::{Action, MenuItem, shown};
use bl_shell::note_commands as ids;
use bl_shell::note_commands::{ChopStep, ClipTab, NoteField};
use bl_shell::{Area, GesturePhase, Invocation, Shell};
use egui::{
    Align2, CursorIcon, Id, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType,
    pos2, vec2,
};
use serde_json::{Value, json};

use crate::arrangement::editing::EditKeys;
use crate::clip_envelope::{self, EnvelopeEditor};
use crate::keymap::{Keymap, Platform};
use crate::launch_panel;
use crate::main_window::UiEvent;
use crate::note_lanes::{self, Cancelled, LaneUi, NoteLayout};
use crate::sample_editor::SampleEditorUi;
use crate::theme::Theme;
use crate::widgets::{self, Choice, NO_LEDS};

/// How wide the key track is, at the note area's left (L-CLIP-012).
pub const KEY_WIDTH: f32 = 27.0;
/// How tall one key's row is.
pub const KEY_HEIGHT: f32 = 8.0;
/// How tall the lane under the note area is (L-CLIP-014).
pub const LANE_HEIGHT: f32 = 43.0;
/// How close to a note's end, in points, a press resizes it rather than
/// moving it.
pub const EDGE_GRAB: f32 = 4.0;
/// How tall the editor toolbar is, in points: the tabs and the grid chooser
/// (L-CLIP-010).
pub const TOOLBAR_HEIGHT: f32 = 16.0;
/// How tall the clip title bar over the panels is, in points (L-CLIP-002).
pub const TITLE_BAR_HEIGHT: f32 = 14.0;
/// How wide a tab is, and how far apart their left edges are (L-CLIP-010's
/// tabs sit 61 points apart).
const TAB_WIDTH: f32 = 58.0;
const TAB_STEP: f32 = 61.0;
/// How far the first tab sits from the Clip View's left edge (x 1029 against
/// the toolbar's 593, L-CLIP-010).
const TABS_LEFT: f32 = 436.0;
/// How wide the grid chooser is, in points.
const GRID_CHOOSER_WIDTH: f32 = 72.0;
/// How tall the editor ruler and marker strip is, in points (L-CLIP-011).
pub const STRIP_HEIGHT: f32 = 16.0;
/// How close to a marker's or a loop-brace end's line, in points, a press
/// grabs it rather than the brace's body.
pub const HANDLE_GRAB: f32 = 4.0;
/// How wide a marker's flag is, in points, at the strip's top.
const FLAG: f32 = 5.0;
/// How tall the loop brace is, in points, along the strip's bottom.
const BRACE: f32 = 6.0;
/// The key at the top edge a view with no notes starts at: middle C, C3 in
/// Live's names.
const DEFAULT_TOP: f32 = 60.0;
/// How wide a note's stem in the lane is.
pub(crate) const STEM_WIDTH: f32 = 3.0;

/// The editor toolbar along the Clip View's top (L-CLIP-010): the Notes,
/// Envelopes and MPE tabs, and the grid chooser.
#[must_use]
pub fn toolbar_rect(view: Rect) -> Rect {
    Rect::from_min_max(view.min, pos2(view.max.x, view.top() + TOOLBAR_HEIGHT))
}

/// The tabs in the order the toolbar lists them: the tab each shows, with
/// its name. The MPE tab comes with its own package, so it shows disabled.
const TABS: [(Option<ClipTab>, &str); 3] = [
    (Some(ClipTab::Notes), "Notes"),
    (Some(ClipTab::Envelopes), "Envelopes"),
    (None, "MPE"),
];

/// The tab at `index` in [`TABS`].
#[must_use]
pub fn tab_rect(view: Rect, index: usize) -> Rect {
    let toolbar = toolbar_rect(view);
    let left = (view.left() + TABS_LEFT)
        .min(view.right() - TAB_STEP * TABS.len() as f32 - GRID_CHOOSER_WIDTH)
        .max(view.left());
    Rect::from_min_size(
        pos2(left + TAB_STEP * index as f32, toolbar.top()),
        vec2(TAB_WIDTH, toolbar.height()),
    )
}

/// The grid chooser, at the toolbar's right end.
#[must_use]
pub fn grid_chooser_rect(view: Rect) -> Rect {
    let toolbar = toolbar_rect(view);
    Rect::from_min_size(
        pos2(toolbar.right() - GRID_CHOOSER_WIDTH - 3.0, toolbar.top()),
        vec2(GRID_CHOOSER_WIDTH, toolbar.height()),
    )
}

/// The editor ruler and marker strip above the note area (L-CLIP-011):
/// the clip's Start and End markers and its loop brace.
#[must_use]
pub fn strip_rect(view: Rect) -> Rect {
    let top = view.top() + TOOLBAR_HEIGHT;
    Rect::from_min_max(pos2(view.left(), top), pos2(view.max.x, top + STRIP_HEIGHT))
}

/// The note area: the Clip View without its toolbar, its marker strip, its
/// key track and its lane.
#[must_use]
pub fn grid_rect(view: Rect) -> Rect {
    let top = view.top() + TOOLBAR_HEIGHT + STRIP_HEIGHT;
    Rect::from_min_max(
        pos2(view.left() + KEY_WIDTH, top),
        pos2(view.max.x, (view.bottom() - LANE_HEIGHT).max(top)),
    )
}

/// The lane under the note area, as wide as the Clip View: its labels sit
/// left of the note area, under the key track (L-CLIP-014).
#[must_use]
pub fn lane_rect(view: Rect) -> Rect {
    Rect::from_min_max(
        pos2(
            view.left(),
            (view.bottom() - LANE_HEIGHT).max(view.top() + TOOLBAR_HEIGHT + STRIP_HEIGHT),
        ),
        view.max,
    )
}

/// Where a MIDI clip's End marker is, in ticks: its `end`, or the clip's
/// end.
fn end_tick(midi: &MidiData, length: Ticks) -> Ticks {
    midi.end.unwrap_or(midi.start + length)
}

/// How wide the Clip View's panels are: the left group, and the right group
/// (L-CLIP-003).
#[must_use]
pub fn panels_width(view: Rect) -> f32 {
    (launch_panel::panel_width(view) + crate::tool_panel::WIDTH).min(view.width())
}

/// The Clip View's panels: the title bar over them, and the panel groups
/// under it.
#[must_use]
pub fn panels_rect(view: Rect) -> Rect {
    Rect::from_min_max(
        pos2(view.left(), view.top() + TITLE_BAR_HEIGHT),
        pos2(view.left() + panels_width(view), view.bottom()),
    )
}

/// The clip title bar over the panels (L-CLIP-002), `wide` points wide.
#[must_use]
pub fn title_bar_rect(view: Rect, wide: f32) -> Rect {
    Rect::from_min_max(
        view.min,
        pos2(view.left() + wide, view.top() + TITLE_BAR_HEIGHT),
    )
}

/// The Clip View's editor: the place beside the panels.
#[must_use]
pub fn editor_rect(view: Rect) -> Rect {
    Rect::from_min_max(pos2(view.left() + panels_width(view), view.top()), view.max)
}

/// The clip title bar over the panels (L-CLIP-002): the clip's name, and on
/// an audio clip the Save Default Clip button, which runs `clip.save_default`
/// for the clip or for each selected audio clip (R17).
fn title_bar(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    clip: ClipId,
    events: &mut Vec<UiEvent>,
) {
    let doc = shell.editor().document();
    let Some(value) = doc.clip(clip) else {
        return;
    };
    let c = &theme.colors;
    ui.painter().rect_filled(rect, 0.0, c.panel.0);
    ui.painter().text(
        pos2(rect.left() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        &value.name,
        theme.small_font(),
        c.text.0,
    );
    // The Save Default Clip button.
    let button = Rect::from_min_size(
        pos2(rect.right() - 98.0, rect.top() + 1.0),
        vec2(92.0, (rect.height() - 2.0).max(1.0)),
    );
    let id = Id::new("clip-save-default");
    let response = ui.interact(button, id, Sense::click());
    let painter = ui.painter_at(button.expand(3.0));
    painter.rect_filled(
        button,
        theme.sizes.corner,
        if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    painter.text(
        button.center(),
        Align2::CENTER_CENTER,
        "Save Default Clip",
        theme.small_font(),
        c.text.0,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Save Default Clip"));
    if response.clicked() {
        // R17: the button saves the clip, or each selected audio clip.
        let clips = crate::audio_panel::targets(shell, doc, clip);
        for target in clips {
            events.push(UiEvent::Invoke(Invocation::new(
                "clip.save_default",
                json!({ "clip": target }),
            )));
        }
    }
}

/// A file dropped from the browser on the Clip View replaces the shown
/// clip's file (`clip.replace_sample`; R18, I-DROP-009).
fn drop_sample(ui: &mut Ui, rect: Rect, clip: ClipId, events: &mut Vec<UiEvent>) {
    let ctx = ui.ctx().clone();
    let Some(payload) = egui::DragAndDrop::payload::<crate::browser::BrowserDrag>(&ctx) else {
        return;
    };
    if payload.plugin.is_some() {
        return;
    }
    let (released, pointer) = ctx.input(|i| {
        (
            i.pointer.any_released(),
            i.pointer.hover_pos().or_else(|| i.pointer.latest_pos()),
        )
    });
    if !released {
        return;
    }
    let Some(pointer) = pointer else {
        return;
    };
    if !rect.contains(pointer) {
        return;
    }
    let Some(path) = payload.paths.first() else {
        return;
    };
    egui::DragAndDrop::clear_payload(&ctx);
    events.push(UiEvent::Invoke(Invocation::new(
        "clip.replace_sample",
        json!({ "clip": clip, "path": path }),
    )));
}
/// The loop brace's ends, in ticks: `loop_start`, and `loop_end` (`null`
/// ending at the clip's end).
fn loop_ticks(midi: &MidiData, length: Ticks) -> (Ticks, Ticks) {
    (
        midi.loop_start,
        midi.loop_end.unwrap_or(midi.start + length),
    )
}

/// Paints the editor's background and the editing grid's vertical lines
/// over `grid`, with contents tick `start` at its left edge and the clip's
/// `length` across it.
pub(crate) fn paint_grid_lines(
    ui: &Ui,
    shell: &Shell,
    theme: &Theme,
    grid: Rect,
    start: Ticks,
    length: Ticks,
) {
    let painter = ui.painter();
    let colours = &theme.colors;
    painter.rect_filled(grid, 0.0, colours.lane.0);
    let step = shell.arrangement().grid_ticks(shell.editor().document());
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
pub(crate) fn value_at(lane: Rect, field: NoteField, y: f32) -> f64 {
    let (low, high) = field.range();
    let at = f64::from((lane.bottom() - y) / lane.height().max(1.0));
    (low + (high - low) * at).clamp(low, high)
}

/// The value of `field` at `y` in `lane`, clamped to the field's range and
/// whole for the whole-number fields, as their commands take it.
#[must_use]
pub(crate) fn lane_value(lane: Rect, field: NoteField, y: f32) -> f64 {
    let value = value_at(lane, field, y);
    if field.whole() { value.round() } else { value }
}

/// The lane's three height labels, top to bottom: the field's highest
/// value, its middle, and its lowest (L-CLIP-014: 127, 64, 1).
pub(crate) fn lane_labels(field: NoteField) -> [&'static str; 3] {
    match field {
        NoteField::Velocity | NoteField::ReleaseVelocity => ["127", "64", "1"],
        NoteField::Chance => ["100%", "50%", "0%"],
        NoteField::VelocityDeviation => ["+127", "0", "-127"],
    }
}

/// Where a note's stem starts, x, in `grid`: at the note's start, one point
/// right of it, as the lane paints it.
#[must_use]
pub(crate) fn stem_x(grid: Rect, start: Ticks, length: Ticks, note: &Note) -> f32 {
    x_of_tick(grid, start, length, note.start) + 1.0
}

/// A note's name as Live writes it: C3 is middle C (MIDI key 60).
pub(crate) fn key_name(key: u8) -> String {
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
        /// The notes dragged, by clip and their places in its notes in
        /// order: one clip's, or the selected notes of several (WP-6.6).
        targets: Vec<(ClipId, Vec<u32>)>,
        /// The grabbed note's start when the drag began, the key under the
        /// pointer then (continuous), and where the grab fell.
        start: Ticks,
        grab_key: f32,
        grab: Ticks,
        /// The furthest the drag may move them: earlier in time, and in
        /// pitch.
        limits: (Ticks, i32, i32),
        /// Copy the notes instead of moving them, as Option-dragging a note
        /// does (S-NOTE-002, piano-roll R29).
        copy: bool,
        /// The grabbed note's pitch and velocity when the drag began, which
        /// the Preview plays at the pitch the drag has reached (Spec:
        /// step-recording R2).
        grabbed: (u8, u8),
    },
    /// A Cmd-drag that hasn't gone far enough to say which it is: moving
    /// the notes off the grid when it runs along the note area, or changing
    /// their velocity when it runs up or down (note-editing R17, piano-roll
    /// R10).
    Undecided {
        press: Pos2,
        moving: Box<NoteDrag>,
        velocity: Box<NoteDrag>,
    },
    /// Dragging notes up or down with Cmd held: their velocity.
    Velocity {
        targets: Vec<(ClipId, Vec<u32>)>,
        /// Where the press fell, y.
        from: f32,
    },
    /// Dragging the notes' ends.
    Resize {
        targets: Vec<(ClipId, Vec<u32>)>,
        /// The grabbed note's end when the drag began, and where the grab
        /// fell.
        end: Ticks,
        grab: Ticks,
        /// How much shorter the drag may make them, at least.
        limit: Ticks,
    },
}

/// What a drag in the marker strip is moving: a marker or the loop brace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StripPart {
    /// The Start marker, where the clip starts playing.
    Start,
    /// The End marker, where an unlooped clip stops.
    End,
    /// The loop brace's start.
    LoopStart,
    /// The loop brace's end.
    LoopEnd,
    /// The loop brace's body: both its ends move together.
    LoopBody,
}

/// A drag in the marker strip: what it grabs, and the clip's markers, loop
/// and length when the drag began. Every frame's command is measured from
/// there, so the whole drag is one gesture and one undo step (piano-roll
/// R11), and the marker being dragged holds still under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StripDrag {
    clip: ClipId,
    /// What the press grabbed.
    part: StripPart,
    /// The contents tick under the press.
    grab: Ticks,
    /// The Start and End markers when the drag began, in contents ticks;
    /// `end` `None` is the clip's end.
    start: Ticks,
    end: Option<Ticks>,
    /// The loop brace when the drag began; `loop_end` `None` is the clip's
    /// end.
    loop_start: Ticks,
    loop_end: Option<Ticks>,
    /// Whether the loop is on: a Start marker must stay before the brace's
    /// end while it is.
    looping: bool,
    /// The clip's length in ticks, so `None` ends resolve to the clip's
    /// end.
    length: Ticks,
}

/// A chop Cmd-E opened: the notes were chopped on the grid, and Cmd with
/// Up and Down keeps changing the parts, all one undo step until Cmd is
/// released (note-editing R36).
#[derive(Clone, Debug, PartialEq)]
struct ChopRun {
    /// The steps pressed since the chop.
    steps: Vec<ChopStep>,
}

/// A drag of E across the note area: the line from where it began splits
/// each note it crosses, where it crosses it (note-editing R33).
#[derive(Clone, Debug, PartialEq)]
struct SplitDrag {
    clip: ClipId,
    from: Pos2,
    /// The clip's notes in order when the drag began, which the frames'
    /// places name (the set changes under the drag as it goes).
    notes: Vec<Note>,
}

/// A drag up or down with E and Option (Cmd, Ctrl on Windows) held, which
/// chops the selected notes into more or fewer equal parts (note-editing
/// R37).
#[derive(Clone, Copy, Debug, PartialEq)]
struct ChopDrag {
    clip: ClipId,
    /// The height the drag began at.
    from: f32,
}

/// What a drag in the stretch markers' band grabs (note-editing R43,
/// R44).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StretchPart {
    /// The marker at the time selection's start.
    First,
    /// The marker at its end.
    Second,
    /// The pseudo marker, at this tick between them.
    Pseudo(Ticks),
}

/// A drag of a stretch marker: the time selection when it began, which
/// every frame's `note.stretch` is measured from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StretchDrag {
    clip: ClipId,
    part: StretchPart,
    range: (Ticks, Ticks),
}

/// The MIDI Note Editor's context menus: the one a note or the empty grid
/// opens (C-NOTE) and the one a key in the piano ruler opens (C-KEYS). Each
/// item runs the command its shortcut runs, and an item whose command isn't
/// built yet is drawn disabled and names its package (piano-roll R30).
#[derive(Clone, Debug, PartialEq)]
pub struct NoteMenus {
    keymap: Keymap,
    platform: Platform,
}

impl Default for NoteMenus {
    fn default() -> Self {
        Self {
            keymap: Keymap::live12(),
            platform: Platform::current(),
        }
    }
}

impl NoteMenus {
    /// Draws the open menu's items: the ones whose command is built run
    /// it, and the rest are drawn disabled and name their package.
    fn show(&self, ui: &mut Ui, shell: &Shell, menu: &[MenuItem], events: &mut Vec<UiEvent>) {
        let contexts = shell.note_menu_contexts();
        let doc = shell.editor().document();
        for item in shown(menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            // Spec: note-editing R35, R41. Split Note(s) shows only while a
            // note would be split, and Split Clip only for an Arrangement
            // clip.
            if (item.row == "C-NOTE-010"
                && !bl_shell::clip_view::would_split(doc, shell.clip_view(), shell.arrangement()))
                || (item.row == "C-NOTE-015"
                    && shell
                        .clip_view()
                        .shown_session_clip(doc, shell.arrangement())
                        .is_some())
            {
                continue;
            }
            // Spec: piano-roll R30, menus.md C-NOTE-014. Clip Markers is a
            // submenu of the clip markers' commands, as M-EDIT-013 is.
            if matches!(item.action, Action::Submenu) {
                ui.menu_button(item.label, |ui| {
                    self.show(ui, shell, &item.items, events);
                });
                continue;
            }
            let package = context_menus::pending_package(item.row);
            let response = crate::menus::item_in(
                ui,
                item,
                shell,
                &self.keymap,
                self.platform,
                &contexts,
                events,
            );
            let response = match package {
                Some(package) => {
                    let text = format!("Not built yet: arrives with {package}.");
                    ui.ctx().accesskit_node_builder(response.id, |node| {
                        node.set_description(text.clone());
                    });
                    response.on_disabled_hover_text(text)
                }
                None => response,
            };
            crate::menus::take_focus_first(ui, &response);
        }
    }
}

/// Paints the key track left of `grid`: white and black keys, each named when
/// its row has the room, the C rows otherwise. Folded, only the rows that
/// hold notes show, one under the next (F).
fn paint_key_track(painter: &egui::Painter, theme: &Theme, grid: Rect, rows: &KeyRows) {
    let colours = &theme.colors;
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
}

/// Paints the lines between the key rows across `grid`.
fn paint_row_lines(ui: &Ui, theme: &Theme, grid: Rect, rows: &KeyRows) {
    let painter = ui.painter();
    let colours = &theme.colors;
    let px = (ui.ctx().pixels_per_point() * 0.5).max(0.5);
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
}

/// The key at the top edge a view with notes starts at: the highest note
/// and two rows above it.
fn default_top(midi: &MidiData) -> f32 {
    let highest = midi.notes.iter().map(|note| note.key).max();
    highest.map_or(DEFAULT_TOP, |key| (f32::from(key) + 2.0).min(127.0))
}

/// The note edges in the clip's span: every note's start and end, in order
/// (piano-roll R23, R24).
fn note_edges(notes: &[Note], span: (Ticks, Ticks)) -> Vec<Ticks> {
    let mut edges: Vec<Ticks> = notes
        .iter()
        .flat_map(|note| [note.start, note.start + note.length])
        .filter(|at| *at >= span.0 && *at <= span.1)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    edges
}

/// The edge before or after `at`: the one the insert marker or the time
/// selection's end moves to.
fn edge_at(edges: &[Ticks], at: Ticks, forward: bool) -> Option<Ticks> {
    if forward {
        edges.iter().copied().find(|edge| *edge > at)
    } else {
        edges.iter().copied().rev().find(|edge| *edge < at)
    }
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

/// What a point in the marker strip is over, if anything: a marker, or an
/// end or the body of the loop brace. The markers sit in the strip's upper
/// part and the brace in its lower part, so a Start marker that falls on
/// the brace's start is still reachable.
fn hit_strip(
    strip: Rect,
    grid: Rect,
    start: Ticks,
    length: Ticks,
    midi: &MidiData,
    pos: Pos2,
) -> Option<StripPart> {
    if !strip.contains(pos) {
        return None;
    }
    let x_of = |tick: Ticks| x_of_tick(grid, start, length, tick);
    let near = |tick: Ticks| (x_of(tick) - pos.x).abs() <= HANDLE_GRAB;
    if pos.y < strip.top() + STRIP_HEIGHT / 2.0 {
        // The Start marker first, then the End marker, so an End marker over
        // a Start marker stays reachable.
        if near(midi.start) {
            Some(StripPart::Start)
        } else {
            near(end_tick(midi, length)).then_some(StripPart::End)
        }
    } else {
        let (from, to) = loop_ticks(midi, length);
        if near(from) {
            Some(StripPart::LoopStart)
        } else if near(to) {
            Some(StripPart::LoopEnd)
        } else {
            let (x0, x1) = (x_of(from), x_of(to));
            (x0.min(x1)..=x0.max(x1))
                .contains(&pos.x)
                .then_some(StripPart::LoopBody)
        }
    }
}

/// The band of the strip the stretch markers sit in: between the clip
/// markers' flags and the loop brace (L-CLIP-020).
fn stretch_band(strip: Rect) -> Rect {
    Rect::from_min_max(
        pos2(strip.left(), strip.top() + FLAG),
        pos2(strip.right(), strip.bottom() - BRACE),
    )
}

/// What a press in the stretch markers' band grabs: a marker at an end of
/// `range`, or between them the pseudo marker under the pointer.
fn hit_stretch(
    strip: Rect,
    grid: Rect,
    start: Ticks,
    length: Ticks,
    range: (Ticks, Ticks),
    pos: Pos2,
) -> Option<StretchPart> {
    if !stretch_band(strip).contains(pos) {
        return None;
    }
    let x_of = |tick: Ticks| x_of_tick(grid, start, length, tick);
    let near = |tick: Ticks| (x_of(tick) - pos.x).abs() <= HANDLE_GRAB;
    if near(range.0) {
        Some(StretchPart::First)
    } else if near(range.1) {
        Some(StretchPart::Second)
    } else {
        let at = tick_at(grid, start, length, pos.x);
        (at > range.0 && at < range.1).then_some(StretchPart::Pseudo(at))
    }
}

/// Where a split lands: at the pointer, or at the nearest grid line with
/// Cmd (Ctrl on Windows) held (note-editing R33).
fn split_tick(arrangement: &ArrangementView, doc: &Document, keys: EditKeys, at: Ticks) -> Ticks {
    if keys.toggle {
        arrangement.nearest_line(doc, at)
    } else {
        at
    }
}

impl SplitDrag {
    /// The drag's `note.split` for the pointer at `pos`: each note the line
    /// from the press to the pointer crosses is split where it crosses the
    /// note's row, or, where the line stays within the row, under the
    /// pointer (note-editing R33).
    #[expect(clippy::too_many_arguments, reason = "the drag's whole geometry")]
    fn args(
        &self,
        arrangement: &ArrangementView,
        doc: &Document,
        keys: EditKeys,
        grid: Rect,
        rows: &KeyRows,
        start: Ticks,
        length: Ticks,
        pos: Pos2,
    ) -> Value {
        let (low, high) = (self.from.y.min(pos.y), self.from.y.max(pos.y));
        let cuts: Vec<Value> = self
            .notes
            .iter()
            .enumerate()
            .filter_map(|(place, note)| {
                let rect = rows
                    .y_of(grid, note.key)
                    .map(|y| note_rect_at(grid, start, length, y, note))?;
                if high < rect.top() || low > rect.bottom() {
                    return None;
                }
                let middle = rect.center().y;
                let x = if low < middle && middle < high {
                    self.from.x
                        + (pos.x - self.from.x) * (middle - self.from.y) / (pos.y - self.from.y)
                } else {
                    pos.x
                };
                let at = split_tick(arrangement, doc, keys, tick_at(grid, start, length, x));
                Some(json!({ "note": place, "at": at }))
            })
            .collect();
        json!({ "clip": self.clip, "cuts": cuts })
    }
}

impl ChopDrag {
    /// The number of parts the pointer at height `y` asks for: one more
    /// for each 8 points dragged up, one fewer for each dragged down, from
    /// 1 to 64, or with Shift the powers of two (note-editing R37).
    fn parts(&self, y: f32, powers: bool) -> u32 {
        let steps = ((self.from - y) / 8.0).floor() as i32;
        if powers {
            1 << steps.clamp(0, 6)
        } else {
            (1 + steps).clamp(1, 64) as u32
        }
    }
}

impl StretchDrag {
    /// The drag's `note.stretch` for the pointer at tick `at`, measured from
    /// the time selection the drag began on. A marker moves freely until it
    /// reaches a grid line either side of where it began, and then snaps;
    /// Cmd (Alt on Windows) or the grid off doesn't snap (R43).
    fn args(
        &self,
        arrangement: &ArrangementView,
        doc: &Document,
        keys: EditKeys,
        at: Ticks,
    ) -> Value {
        let (a, b) = self.range;
        let target = |origin: Ticks| {
            let (before, after) = arrangement.lines_around(doc, origin);
            if at >= before && at <= after {
                at
            } else {
                arrangement.snap_drag(doc, at, keys.bypass)
            }
        };
        let from = json!({ "start": a, "end": b });
        match self.part {
            StretchPart::First => json!({
                "clip": self.clip, "from": from, "to": { "start": target(a), "end": b },
            }),
            StretchPart::Second => json!({
                "clip": self.clip, "from": from, "to": { "start": a, "end": target(b) },
            }),
            StretchPart::Pseudo(grab) => {
                let point = target(grab).clamp(a + Ticks(1), (b - Ticks(1)).max(a + Ticks(1)));
                json!({
                    "clip": self.clip, "from": from, "to": from,
                    "point": { "from": grab, "to": point },
                })
            }
        }
    }
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
        .filter_map(|place| notes.get(*place as usize).cloned())
        .collect()
}

/// `args` naming the notes `targets` hold: one clip's by `clip` and `notes`,
/// several clips' by `clips` (Spec: multi-clip-editing R18).
fn named(targets: &[(ClipId, Vec<u32>)], mut args: Value) -> Value {
    if let [(clip, places)] = targets {
        args["clip"] = json!(clip);
        args["notes"] = json!(places);
    } else {
        args["clips"] = Value::Array(
            targets
                .iter()
                .map(|(clip, notes)| json!({ "clip": clip, "notes": notes }))
                .collect(),
        );
    }
    args
}

pub(crate) fn gesture(events: &mut Vec<UiEvent>, phase: GesturePhase, id: &str, args: Value) {
    events.push(UiEvent::Gesture {
        phase,
        invocation: Some(Invocation::new(id, args)),
    });
}

/// The grid chooser's choices: the zoom-adaptive settings, then the fixed
/// spacings, as the Arrangement's context menus list them (C-CLIP-ARR-020,
/// C-CLIP-ARR-021).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GridChoice {
    Adaptive(AdaptiveGrid),
    Fixed(FixedGrid),
}

/// Draws the toolbar: the Notes, Envelopes and MPE tabs, and the grid
/// chooser (L-CLIP-010). A tab runs `view.clip_tab`, the grid chooser
/// `arrangement.set_grid`.
fn toolbar(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    view: Rect,
    audio: bool,
    events: &mut Vec<UiEvent>,
) {
    let colours = &theme.colors;
    let shown = shell.clip_view().tab();
    ui.painter()
        .rect_filled(toolbar_rect(view), 0.0, colours.bar.0);
    for (index, (tab, name)) in TABS.iter().enumerate() {
        // An audio clip has the Sample and Envelopes tabs (sample editor R1).
        if audio && tab.is_none() {
            continue;
        }
        let name = if audio && *tab == Some(ClipTab::Notes) {
            &"Sample"
        } else {
            name
        };
        let rect = tab_rect(view, index);
        let id = Id::new(("clip-view-tab", index));
        let enabled = tab.is_some();
        let response = ui.interact(
            rect,
            id,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        if enabled {
            crate::focus::register(ui.ctx(), id, Area::Detail, None);
        }
        let on = *tab == Some(shown);
        let face = if on {
            colours.control_pressed.0
        } else if response.hovered() && enabled {
            colours.control_hover.0
        } else {
            colours.control.0
        };
        ui.painter().rect_filled(rect, theme.sizes.corner, face);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            name,
            theme.font(),
            if enabled {
                colours.text.0
            } else {
                colours.pending.0
            },
        );
        response.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, enabled, on, *name));
        if response.clicked()
            && let Some(tab) = tab
        {
            events.push(UiEvent::Invoke(Invocation::new(
                "view.clip_tab",
                json!({ "tab": tab }),
            )));
        }
    }
    let grid = shell.arrangement().grid();
    let choices: Vec<Choice<GridChoice>> = AdaptiveGrid::ALL
        .iter()
        .map(|choice| Choice {
            label: choice.label().to_owned(),
            value: GridChoice::Adaptive(*choice),
            leds: NO_LEDS,
        })
        .chain(FixedGrid::ALL.iter().map(|choice| Choice {
            label: choice.label().to_owned(),
            value: GridChoice::Fixed(*choice),
            leds: NO_LEDS,
        }))
        .collect();
    let current = if grid.fixed_on {
        GridChoice::Fixed(grid.fixed)
    } else {
        GridChoice::Adaptive(grid.adaptive)
    };
    crate::focus::register(
        ui.ctx(),
        Id::new(("chooser", "clip-view-grid")),
        Area::Detail,
        None,
    );
    if let Some(chosen) = widgets::chooser(
        ui,
        theme,
        "clip-view-grid",
        "Grid",
        grid_chooser_rect(view),
        &choices,
        Some(&current),
        true,
    ) {
        let args = match chosen {
            GridChoice::Adaptive(choice) => json!({ "adaptive": choice }),
            GridChoice::Fixed(choice) => json!({ "fixed": choice }),
        };
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::arrangement_commands::SET_GRID,
            args,
        )));
    }
}

mod multi;
pub use multi::{BAR_HEIGHT, Geometry, geometry};

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
    /// The drag under way in the marker strip, if any.
    strip_drag: Option<StripDrag>,
    /// The expression lanes' heights, drags and typed values (WP-6.1).
    lanes: LaneUi,
    /// The marquee under way: where it began, and where it is now.
    marquee: Option<(Pos2, Pos2)>,
    /// The chop Cmd-E opened, while Cmd is held.
    chop: Option<ChopRun>,
    /// The E-drag under way that splits notes.
    split_drag: Option<SplitDrag>,
    /// The drag under way that chops notes into parts.
    chop_drag: Option<ChopDrag>,
    /// The drag under way in a stretch marker.
    stretch_drag: Option<StretchDrag>,
    /// The Envelopes tab's editor.
    envelope: EnvelopeEditor,
    /// The Sample Editor, for an audio clip (WP-4.10).
    sample: SampleEditorUi,
    /// The note editor's context menus (C-NOTE, C-KEYS).
    menus: NoteMenus,
    /// How tall the loop bars of several clips are, which dragging above
    /// the title bar changes (multi-clip-editing R2).
    bar_height: f32,
    /// The drag of loop bars' braces under way, if any.
    bar_drag: Option<multi::BarDrag>,
    /// The preview the pointer holds: the clip, the key and the velocity
    /// (Spec: step-recording R2).
    preview: Option<(ClipId, u8, u8)>,
}

impl Default for ClipViewUi {
    fn default() -> Self {
        Self {
            shown: None,
            folded: false,
            top: DEFAULT_TOP,
            first: 0.0,
            drag: None,
            strip_drag: None,
            lanes: LaneUi::default(),
            marquee: None,
            chop: None,
            split_drag: None,
            chop_drag: None,
            stretch_drag: None,
            envelope: EnvelopeEditor::default(),
            sample: SampleEditorUi::default(),
            menus: NoteMenus::default(),
            bar_height: BAR_HEIGHT,
            bar_drag: None,
            preview: None,
        }
    }
}

impl ClipViewUi {
    /// Ends the chop Cmd-E opened, if one is open, as one undo step.
    fn end_chop(&mut self, events: &mut Vec<UiEvent>) {
        if self.chop.take().is_some() {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: None,
            });
        }
    }

    /// What the Sample Editor built in its last frame (R22).
    #[must_use]
    pub fn sample_stats(&self) -> crate::sample_editor::Stats {
        self.sample.stats()
    }

    /// Whether a number is being typed into the lanes' footer or as a
    /// velocity, so the keys go to it and not to shortcuts (note-editing
    /// R14).
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.lanes.is_typing()
    }

    /// The lanes' heights, for the layout the editor draws.
    #[must_use]
    pub fn lane_heights(&self, shown: &[NoteField]) -> Vec<(NoteField, f32)> {
        self.lanes.heights(shown)
    }

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
        peaks: &mut dyn FnMut(bl_model::ContentHash) -> Option<std::sync::Arc<bl_media::Peaks>>,
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
        if let Some(audio) = shell.clip_view().shown_audio_clip(doc, shell.arrangement()) {
            self.show_audio(ui, shell, theme, rect, audio, events, peaks);
            return;
        }
        self.sample.reset();
        let shown = shell.clip_view().shown_clip(doc, shell.arrangement());
        let editor = shown
            .and_then(|id| doc.clip(id))
            .and_then(|clip| match &clip.data {
                ClipData::Midi(midi) => Some((clip.id, midi.clone(), clip.length)),
                ClipData::Audio(_) => None,
            });
        let Some((clip, midi, length)) = editor else {
            self.end_preview(events);
            self.shown = None;
            self.drag = None;
            self.strip_drag = None;
            self.lanes.reset();
            self.marquee = None;
            self.end_chop(events);
            self.split_drag = None;
            self.chop_drag = None;
            self.stretch_drag = None;
            self.envelope.reset();
            return;
        };
        // The panel groups take the left of the view (L-CLIP-003): the left
        // group's Launch Properties on a Session clip, or Main Properties
        // alone, and the right group's Utilities, Transform and Generate.
        let left = Rect::from_min_size(
            rect.min,
            vec2(launch_panel::panel_width(rect), rect.height()),
        );
        if shell
            .clip_view()
            .shown_session_clip(doc, shell.arrangement())
            .is_some()
        {
            launch_panel::show(ui, shell, theme, left, clip, events);
        } else {
            launch_panel::show_main_only(ui, theme, left);
        }
        let right = crate::tool_panel::panel_rect(
            Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom())),
            left.right(),
        );
        crate::tool_panel::show(ui, shell, theme, right, clip, events);
        let rect = Rect::from_min_max(pos2(right.right(), rect.top()), rect.max);
        let folded = shell.clip_view().is_folded(clip);
        if self.shown != Some(clip) || self.folded != folded {
            // Another clip shows, or the editor was folded or unfolded:
            // scroll to its notes.
            self.shown = Some(clip);
            self.folded = folded;
            self.top = default_top(&midi);
            self.first = 0.0;
            self.drag = None;
            self.strip_drag = None;
            self.lanes.reset();
            self.marquee = None;
            self.end_chop(events);
            self.split_drag = None;
            self.chop_drag = None;
            self.stretch_drag = None;
            self.envelope.reset();
        }
        let envelopes = shell.clip_view().tab() == ClipTab::Envelopes;
        // Spec: multi-clip-editing R1. Several clips share the note editor,
        // under loop bars.
        let several = shell.clip_view().shown(doc, shell.arrangement());
        if several.len() > 1 && !envelopes {
            self.show_multi(ui, shell, theme, rect, &several, events);
            return;
        }
        self.bar_drag = None;
        let shown = shell.clip_view().lanes().shown();
        let layout = note_lanes::note_layout(rect, &self.lanes.heights(&shown));
        let grid = if envelopes {
            grid_rect(rect)
        } else {
            layout.grid
        };
        let strip = strip_rect(rect);
        if grid.width() < 1.0 || grid.height() < KEY_HEIGHT || strip.height() < 1.0 {
            return;
        }
        toolbar(ui, shell, theme, rect, false, events);
        if envelopes && let Some(value) = doc.clip(clip) {
            let target = clip_envelope::shown_target(shell, doc, clip);
            let axis = match self.strip_drag {
                Some(drag) if drag.part == StripPart::Start => drag.start,
                _ => clip_envelope::axis_start(value, &midi, target),
            };
            self.paint_strip(ui.painter(), theme, strip, grid, &midi, axis, length);
            self.strip_input(
                ui,
                doc,
                shell.arrangement(),
                strip,
                grid,
                clip,
                &midi,
                axis,
                length,
                None,
                events,
            );
            let frame = clip_envelope::Frame {
                view: rect,
                clip: value,
                midi: &midi,
                target,
            };
            self.envelope.show(ui, shell, theme, &frame, events);
            return;
        }
        let notes = notes_in_order(&midi);
        let rows = KeyRows::of(folded, &notes, self.top, self.first);
        let selected = shell
            .clip_view()
            .selected_notes(doc, shell.arrangement())
            .to_vec();
        // While the Start marker is dragged, the editor keeps the axis the
        // drag began on, so the notes hold still under the moving marker;
        // releasing re-anchors the contents to the new marker (R4).
        let axis = match self.strip_drag {
            Some(drag) if drag.part == StripPart::Start => drag.start,
            _ => midi.start,
        };
        let face = doc.clip_track(clip).map_or(theme.colors.accent.0, |track| {
            crate::arrangement::scene::track_colour(theme, track)
        });
        let hovered = Self::hovered_note(ui, &layout, &rows, &notes, axis, length);
        let frame = note_lanes::Frame {
            clip,
            layout: &layout,
            start: axis,
            length,
            notes: &notes,
            groups: &midi.groups,
            selected: &selected,
            face,
            hovered,
            step: shell.arrangement().grid_ticks(doc),
            snap: shell.arrangement().grid().snap,
        };
        self.paint(
            ui, shell, theme, clip, strip, &frame, &rows, &midi, axis, length, &notes, &selected,
        );
        self.input(
            ui,
            shell,
            doc,
            shell.arrangement(),
            strip,
            rect,
            &frame,
            &rows,
            clip,
            &midi,
            axis,
            length,
            &notes,
            &selected,
            events,
        );
        self.lanes.footer(ui, shell, theme, &frame, events);
        Self::preview_switch(ui, shell, theme, strip, events);
    }

    /// The Preview switch above the piano ruler (L-CLIP-012), which runs
    /// `clip_view.set_preview` (Spec: step-recording R1).
    fn preview_switch(
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        strip: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let on = shell.clip_view().preview();
        let rect = Rect::from_min_size(
            strip.min + vec2(2.0, 2.0),
            vec2(KEY_WIDTH - 4.0, STRIP_HEIGHT - 4.0),
        );
        let id = Id::new("clip-view-preview");
        let response = ui.interact(rect, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let colours = &theme.colors;
        let face = if on {
            colours.accent.0
        } else if response.hovered() {
            colours.control_hover.0
        } else {
            colours.control.0
        };
        let ink = if on {
            colours.accent_text.0
        } else {
            colours.text.0
        };
        let painter = ui.painter();
        painter.rect_filled(rect, theme.sizes.corner, face);
        // A note: a head and its stem.
        let head = rect.center() + vec2(-2.0, 2.0);
        painter.circle_filled(head, 2.0, ink);
        painter.line_segment(
            [head + vec2(2.0, 0.0), head + vec2(2.0, -6.0)],
            Stroke::new(1.0, ink),
        );
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, "Preview"));
        if response.clicked() {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SET_PREVIEW,
                json!({ "on": !on }),
            )));
        }
    }

    /// Lets go of the preview the pointer holds, if there is one.
    fn end_preview(&mut self, events: &mut Vec<UiEvent>) {
        if let Some((clip, key, _)) = self.preview.take() {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::PREVIEW_NOTE,
                json!({ "clip": clip, "key": key, "on": false }),
            )));
        }
    }

    /// Plays what the pointer holds, as `note.preview`, and lets the
    /// previous one go (Spec: step-recording R2).
    #[expect(
        clippy::too_many_arguments,
        reason = "the editor's pointer and geometry"
    )]
    fn preview_input(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        keys_rect: Rect,
        grid: &Rect,
        rows: &KeyRows,
        clip: ClipId,
        notes: &[Note],
        pointer: Option<Pos2>,
        draw_mode: bool,
        hit: &dyn Fn(Pos2) -> Option<(u32, NoteZone)>,
        events: &mut Vec<UiEvent>,
    ) {
        let held = ui
            .input(|i| i.pointer.primary_down())
            .then(|| ui.input(|i| i.pointer.press_origin()));
        let row = |y: f32| {
            rows.key_at(*grid, y)
                .map(|key| key.round().clamp(0.0, 127.0) as u8)
        };
        let want = match (shell.clip_view().preview(), held.flatten(), pointer) {
            (true, Some(press), Some(pos)) if keys_rect.contains(press) => {
                row(pos.y).map(|key| (key, 100))
            }
            (true, Some(press), pos) if grid.contains(press) => match &self.drag {
                Some(NoteDrag::Move {
                    grab_key,
                    limits,
                    grabbed: (key, velocity),
                    ..
                }) => {
                    let now = pos
                        .and_then(|pos| rows.key_at(*grid, pos.y))
                        .unwrap_or(*grab_key);
                    let semitones = ((now - *grab_key).round() as i32).clamp(limits.1, limits.2);
                    Some(((i32::from(*key) + semitones).clamp(0, 127) as u8, *velocity))
                }
                // A resize or a velocity drag keeps what the press played.
                Some(_) => self.preview.map(|(_, key, velocity)| (key, velocity)),
                None => match (hit(press), draw_mode) {
                    (Some((place, _)), _) => notes
                        .get(place as usize)
                        .map(|note| (note.key, note.velocity)),
                    (None, true) => row(press.y).map(|key| (key, 100)),
                    (None, false) => None,
                },
            },
            _ => None,
        };
        let sounding = self.preview.map(|(_, key, velocity)| (key, velocity));
        if want == sounding {
            return;
        }
        self.end_preview(events);
        if let Some((key, velocity)) = want {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::PREVIEW_NOTE,
                json!({ "clip": clip, "key": key, "velocity": velocity, "on": true }),
            )));
            self.preview = Some((clip, key, velocity));
        }
    }

    /// The note the pointer marks: the one under it in the note area, or
    /// whose marker it is over in a lane (note-editing R4).
    fn hovered_note(
        ui: &Ui,
        layout: &NoteLayout,
        rows: &KeyRows,
        notes: &[Note],
        start: Ticks,
        length: Ticks,
    ) -> Option<u32> {
        let pos = ui.input(|i| i.pointer.hover_pos())?;
        if layout.grid.contains(pos) {
            return hit_note(layout.grid, start, length, rows, notes, pos).map(|(place, _)| place);
        }
        note_lanes::hovered_marker(layout, (start, length), notes, pos)
    }

    /// Shows an audio clip's editor: the Sample Editor under the Sample tab
    /// (WP-4.10), or the Envelopes tab (R1).
    #[expect(clippy::too_many_arguments, reason = "the editor's whole frame")]
    fn show_audio(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        clip: ClipId,
        events: &mut Vec<UiEvent>,
        peaks: &mut dyn FnMut(bl_model::ContentHash) -> Option<std::sync::Arc<bl_media::Peaks>>,
    ) {
        let doc = shell.editor().document();
        let Some(value) = doc.clip(clip) else {
            return;
        };
        let ClipData::Audio(audio) = &value.data else {
            return;
        };
        // The panels' groups: the left group holds the Launch Properties
        // panel (a Session clip), the right group the Audio Utilities panel
        // (an audio clip). The clip title bar is over them (L-CLIP-002).
        let session = shell
            .clip_view()
            .shown_session_clip(doc, shell.arrangement())
            .is_some();
        let panels = panels_rect(rect);
        title_bar(
            ui,
            shell,
            theme,
            title_bar_rect(rect, panels.width()),
            clip,
            events,
        );
        // The left group holds the Launch Properties panel, a Session clip's.
        let left = Rect::from_min_size(
            panels.min,
            vec2(launch_panel::panel_width(rect), panels.height()),
        );
        if session {
            launch_panel::show(ui, shell, theme, left, clip, events);
        }
        let right = crate::audio_panel::panel_rect(panels, left.right());
        crate::audio_panel::show(ui, shell, theme, right, clip, events);
        let rect = Rect::from_min_max(pos2(right.right(), rect.top()), rect.max);
        // A file dropped from the browser on the Clip View replaces the
        // clip's sample (R18, I-DROP-009).
        drop_sample(ui, rect, clip, events);
        if self.shown != Some(clip) {
            self.shown = Some(clip);
            self.drag = None;
            self.strip_drag = None;
            self.lanes.reset();
            self.marquee = None;
            self.end_chop(events);
            self.split_drag = None;
            self.chop_drag = None;
            self.stretch_drag = None;
            self.envelope.reset();
            self.sample.reset();
        }
        toolbar(ui, shell, theme, rect, true, events);
        if shell.clip_view().tab() == ClipTab::Envelopes {
            self.sample.reset();
            // An audio clip's envelopes run from the clip's start, one tick
            // per tick (the clip envelopes spec, R2).
            let target = clip_envelope::shown_target(shell, doc, clip);
            let stand_in = MidiData::new([]);
            let frame = clip_envelope::Frame {
                view: rect,
                clip: value,
                midi: &stand_in,
                target,
            };
            self.envelope.show(ui, shell, theme, &frame, events);
            return;
        }
        self.envelope.reset();
        let ready = peaks(audio.media);
        self.sample
            .show(ui, shell, theme, rect, clip, ready, events);
    }

    /// Paints the marker strip with the clip's markers and loop brace, the
    /// key track, the grid, the notes, the lane, and the marquee. `axis` is
    /// the contents tick the note area's left edge shows: `MidiData::start`,
    /// or the tick a Start marker drag began on (R4).
    #[expect(clippy::too_many_arguments, reason = "the editor's whole layout")]
    fn paint(
        &self,
        ui: &Ui,
        shell: &Shell,
        theme: &Theme,
        clip: ClipId,
        strip: Rect,
        frame: &note_lanes::Frame<'_>,
        rows: &KeyRows,
        midi: &MidiData,
        axis: Ticks,
        length: Ticks,
        notes: &[Note],
        selected: &[u32],
    ) {
        let painter = ui.painter();
        let colours = &theme.colors;
        let start = axis;
        let grid = frame.layout.grid;

        self.paint_strip(painter, theme, strip, grid, midi, axis, length);
        let range = shell
            .clip_view()
            .time_selection(shell.editor().document(), shell.arrangement());
        if let Some(range) = range {
            self.paint_stretch_markers(ui, theme, strip, grid, axis, length, range);
        }

        paint_key_track(painter, theme, grid, rows);

        // The clip's contents: the grid lines, then the notes over them.
        paint_grid_lines(ui, shell, theme, grid, start, length);
        paint_row_lines(ui, theme, grid, rows);
        let face = frame.face;
        for (place, note) in notes.iter().enumerate() {
            let Some(y) = rows.y_of(grid, note.key) else {
                continue;
            };
            let rect = note_rect_at(grid, start, length, y, note);
            if rect.right() < grid.left() || rect.left() > grid.right() {
                continue;
            }
            let corner = theme.sizes.corner.min(2.0);
            // Spec: piano-roll R43. The note is drawn in its clip's colour,
            // paler the softer its velocity; a deactivated note is grey.
            let colour = if note.mute {
                colours.clip_muted.0
            } else {
                crate::theme::note_colour(face, note.velocity)
            };
            painter.rect_filled(rect, corner, colour);
            // Spec: note-editing R4. A note that doesn't always play wears a
            // corner triangle; the note the pointer marks is outlined.
            if note_lanes::shows_corner(note, KEY_HEIGHT) {
                note_lanes::paint_corner(painter, rect, colours.text.0);
            }
            // Hovering a grouped note marks every note of its group (R24).
            if note_lanes::is_marked(frame.notes, frame.hovered, place) {
                painter.rect_stroke(
                    rect,
                    corner,
                    Stroke::new(1.0, colours.text.0),
                    StrokeKind::Outside,
                );
            }
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
        self.lanes.paint(ui, shell, theme, frame);
        // Spec: piano-roll R23, R24. The time selection is drawn across the
        // note area and the lanes, and the insert marker is a line through
        // both.
        let lane = Rect::from_min_max(
            pos2(grid.left(), grid.top()),
            pos2(grid.right(), frame.layout.footer.top()),
        );
        let time = shell
            .clip_view()
            .time_selection(shell.editor().document(), shell.arrangement());
        let insert = shell
            .clip_view()
            .insert_marker(shell.editor().document(), shell.arrangement());
        if let Some((from, to)) = time {
            let x0 = x_of_tick(grid, start, length, from);
            let x1 = x_of_tick(grid, start, length, to);
            let band = Rect::from_min_max(pos2(x0, grid.top()), pos2(x1, lane.bottom())).intersect(
                Rect::from_min_max(
                    pos2(grid.left(), grid.top()),
                    pos2(grid.right(), lane.bottom()),
                ),
            );
            painter.rect_filled(band, 0.0, colours.time_selection.0.gamma_multiply(0.2));
        }
        // Spec: piano-roll R23. The insert marker is a flashing line: on for
        // half of each second, off for the next. The frame after the next
        // turn of the phase is asked for, so it costs two frames a second.
        if let Some(at) = insert {
            let phase = ui.input(|i| i.time).rem_euclid(1.0);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    0.5 - phase.rem_euclid(0.5),
                ));
            if phase < 0.5 {
                let x = x_of_tick(grid, start, length, at);
                painter.rect_filled(
                    Rect::from_min_max(pos2(x - 0.5, grid.top()), pos2(x + 0.5, lane.bottom())),
                    0.0,
                    colours.insert_marker.0,
                );
            }
        }
        // Spec: piano-roll R23, R24. The insert marker's place and the time
        // selection reach assistive technology, as the Sample Editor's
        // markers do: an item with its name and the ticks of the clip's
        // contents.
        let describe = |tick: Ticks| format!("tick {}", tick.get());
        if let Some(at) = insert {
            let x = x_of_tick(grid, start, length, at);
            let rect = Rect::from_min_max(pos2(x - 0.5, grid.top()), pos2(x + 0.5, lane.bottom()));
            let response = ui.interact(
                rect,
                Id::new(("clip-view-insert-marker", clip)),
                Sense::hover(),
            );
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(egui::accesskit::Role::ListItem);
                node.set_label("Insert Marker".to_owned());
                node.set_value(describe(at));
                node.set_bounds(egui::accesskit::Rect {
                    x0: rect.min.x.into(),
                    y0: rect.min.y.into(),
                    x1: rect.max.x.into(),
                    y1: rect.max.y.into(),
                });
            });
        }
        if let Some((from, to)) = time {
            let x0 = x_of_tick(grid, start, length, from);
            let x1 = x_of_tick(grid, start, length, to);
            let rect = Rect::from_min_max(pos2(x0, grid.top()), pos2(x1, lane.bottom()));
            let response = ui.interact(
                rect,
                Id::new(("clip-view-time-selection", clip)),
                Sense::hover(),
            );
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(egui::accesskit::Role::ListItem);
                node.set_label("Time Selection".to_owned());
                node.set_value(format!("{} to {}", describe(from), describe(to)));
                node.set_bounds(egui::accesskit::Rect {
                    x0: rect.min.x.into(),
                    y0: rect.min.y.into(),
                    x1: rect.max.x.into(),
                    y1: rect.max.y.into(),
                });
            });
        }
    }

    /// Paints the editor ruler and marker strip (L-CLIP-011): the clip's
    /// Start and End markers, each a line with a flag at its top, and the
    /// loop brace along the strip's lower edge in the on and off colours
    /// the Arrangement's brace wears.
    #[expect(clippy::too_many_arguments, reason = "the strip's whole layout")]
    fn paint_strip(
        &self,
        painter: &egui::Painter,
        theme: &Theme,
        strip: Rect,
        grid: Rect,
        midi: &MidiData,
        axis: Ticks,
        length: Ticks,
    ) {
        let colours = &theme.colors;
        painter.rect_filled(strip, 0.0, colours.ruler.0);
        let x_of = |tick: Ticks| x_of_tick(grid, axis, length, tick);
        let marker = |tick: Ticks| {
            let x = x_of(tick);
            painter.line_segment(
                [pos2(x, strip.top()), pos2(x, grid.top())],
                Stroke::new(1.0, colours.insert_marker.0),
            );
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(x - FLAG / 2.0, strip.top()),
                    pos2(x + FLAG / 2.0, strip.top() + FLAG),
                ),
                0.0,
                colours.insert_marker.0,
            );
        };
        marker(midi.start);
        marker(end_tick(midi, length));
        let (from, to) = loop_ticks(midi, length);
        let brace = Rect::from_min_max(
            pos2(x_of(from), grid.top() - BRACE),
            pos2(x_of(to), grid.top()),
        );
        painter.rect_filled(
            brace,
            1.0,
            if midi.looping {
                colours.loop_on.0
            } else {
                colours.loop_off.0
            },
        );
        painter.line_segment(
            [strip.left_bottom(), strip.right_bottom()],
            Stroke::new(1.0, colours.separator.0),
        );
    }

    /// The stretch markers at the time selection's ends, and the pseudo
    /// marker under the pointer while it is between them (note-editing
    /// R42, R44).
    #[expect(clippy::too_many_arguments, reason = "the strip's whole layout")]
    fn paint_stretch_markers(
        &self,
        ui: &Ui,
        theme: &Theme,
        strip: Rect,
        grid: Rect,
        axis: Ticks,
        length: Ticks,
        range: (Ticks, Ticks),
    ) {
        let painter = ui.painter();
        let band = stretch_band(strip);
        let marker = |x: f32, colour: egui::Color32| {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(x - 3.0, band.top()),
                    pos2(x + 3.0, band.top()),
                    pos2(x, band.bottom()),
                ],
                colour,
                Stroke::NONE,
            ));
        };
        for tick in [range.0, range.1] {
            marker(x_of_tick(grid, axis, length, tick), theme.colors.accent.0);
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let pseudo = match (self.stretch_drag, pointer) {
            (Some(drag), Some(pos)) if matches!(drag.part, StretchPart::Pseudo(_)) => Some(pos.x),
            (None, Some(pos)) => matches!(
                hit_stretch(strip, grid, axis, length, range, pos),
                Some(StretchPart::Pseudo(_))
            )
            .then_some(pos.x),
            _ => None,
        };
        if let Some(x) = pseudo {
            marker(x, theme.colors.text_dim.0);
        }
    }

    /// The marker strip's, the note area's and the lanes' clicks, drags, and
    /// wheel, and the note-selection keys.
    #[expect(clippy::too_many_arguments, reason = "the editor's whole input")]
    fn input(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        doc: &Document,
        arrangement: &ArrangementView,
        strip: Rect,
        view: Rect,
        frame: &note_lanes::Frame<'_>,
        rows: &KeyRows,
        clip: ClipId,
        midi: &MidiData,
        axis: Ticks,
        length: Ticks,
        notes: &[Note],
        selected: &[u32],
        events: &mut Vec<UiEvent>,
    ) {
        let grid = frame.layout.grid;
        // Spec: piano-roll R11. The marker strip's drags run the document's
        // clip commands, one gesture each.
        let time = shell.clip_view().time_selection(doc, arrangement);
        self.strip_input(
            ui,
            doc,
            arrangement,
            strip,
            grid,
            clip,
            midi,
            axis,
            length,
            time,
            events,
        );
        // Spec: note-editing R36. Releasing Cmd ends the chop Cmd-E opened.
        if self.chop.is_some() && self.chop_drag.is_none() && !ui.input(|i| i.modifiers.command) {
            self.chop = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: None,
            });
        }
        let response = ui.interact(
            grid,
            Id::new(("clip-view-grid", clip)),
            Sense::click_and_drag(),
        );
        let keys = EditKeys::from(ui.input(|i| i.modifiers));
        let draw_mode = shell.clip_view().draw_mode();
        let start = axis;
        let hit = |pos: Pos2| hit_note(grid, start, length, rows, notes, pos);
        // Spec: note-editing R16. A click or a drag in the note area gives
        // the keyboard back from a lane.
        if response.clicked() || response.drag_started() {
            self.lanes.leave_focus();
        }

        // Spec: note-editing R33, R37. With Draw Mode off, E held changes
        // what the pointer does: a click or drag splits the notes it crosses,
        // and with Option (Cmd, Ctrl on Windows) held, a drag on a selected
        // note chops the selected notes into parts.
        let e_down = !draw_mode && ui.input(|i| i.key_down(Key::E));
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && e_down
        {
            let on_selected = matches!(hit(press), Some((place, _)) if selected.contains(&place));
            if (keys.copy || keys.toggle) && on_selected {
                self.chop_drag = Some(ChopDrag {
                    clip,
                    from: press.y,
                });
            } else {
                self.split_drag = Some(SplitDrag {
                    clip,
                    from: press,
                    notes: notes.to_vec(),
                });
            }
        } else if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && !draw_mode
        {
            match hit(press) {
                // Spec: piano-roll R9. Dragging a note's end resizes it,
                // keeping at least one grid step, or a tick with snapping
                // off.
                Some((place, NoteZone::End)) => {
                    let places = grabbed(shell, doc, arrangement, clip, place, events);
                    let note = &notes[place as usize];
                    let end = note.start + note.length;
                    let shortest = if arrangement.grid().snap {
                        arrangement.grid_ticks(doc)
                    } else {
                        Ticks(1)
                    };
                    self.drag = Some(NoteDrag::Resize {
                        limit: resize_limit(&notes_at(notes, &places), shortest),
                        targets: vec![(clip, places)],
                        end,
                        grab: tick_at(grid, start, length, press.x) - end,
                    });
                }
                // Spec: piano-roll R7. Dragging moves the grabbed note, or
                // the selection it belongs to.
                Some((place, NoteZone::Body)) => {
                    let places = grabbed(shell, doc, arrangement, clip, place, events);
                    let note = &notes[place as usize];
                    let moving = NoteDrag::Move {
                        limits: move_limits(&notes_at(notes, &places)),
                        start: note.start,
                        grab_key: rows
                            .key_at(grid, press.y)
                            .unwrap_or_else(|| f32::from(note.key)),
                        grab: tick_at(grid, start, length, press.x) - note.start,
                        // Spec: piano-roll R29. Option-dragging a note
                        // copies it.
                        copy: keys.copy,
                        targets: vec![(clip, places.clone())],
                        grabbed: (note.key, note.velocity),
                    };
                    // Spec: note-editing R17. Cmd-dragging a note (Alt on
                    // Windows) up or down changes its velocity, or every
                    // selected note's.
                    self.drag = Some(if keys.bypass {
                        NoteDrag::Undecided {
                            press,
                            moving: Box::new(moving),
                            velocity: Box::new(NoteDrag::Velocity {
                                targets: vec![(clip, places)],
                                from: press.y,
                            }),
                        }
                    } else {
                        moving
                    });
                }
                // Spec: piano-roll R6. Anywhere else a marquee selects.
                None => self.marquee = Some((press, press)),
            }
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        if let (Some(NoteDrag::Undecided { press, .. }), Some(pos)) = (self.drag.as_ref(), pointer)
            && (pos - *press).length() >= 3.0
        {
            let across = (pos - *press).x.abs() >= (pos - *press).y.abs();
            if let Some(NoteDrag::Undecided {
                moving, velocity, ..
            }) = self.drag.take()
            {
                self.drag = Some(if across { *moving } else { *velocity });
            }
        }
        if response.dragged() {
            if let (Some(drag), Some(pos)) = (self.split_drag.as_ref(), pointer) {
                let args = drag.args(arrangement, doc, keys, grid, rows, start, length, pos);
                gesture(events, GesturePhase::Update, ids::SPLIT, args);
            } else if let (Some(drag), Some(pos)) = (self.chop_drag, pointer) {
                let parts = drag.parts(pos.y, keys.extend);
                gesture(
                    events,
                    GesturePhase::Update,
                    ids::CHOP,
                    json!({ "clip": drag.clip, "parts": parts }),
                );
                events.push(UiEvent::StatusHint(Some(format!("Chop: {parts} parts"))));
            } else if let (Some(drag), Some(pos)) = (self.drag.as_ref(), pointer) {
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
                // Spec: piano-roll R24. The marquee's time span is the
                // time selection.
                let (low, high) = (from.x.min(pos.x), from.x.max(pos.x));
                events.push(UiEvent::Invoke(Invocation::new(
                    "clip_view.select_time",
                    json!({
                        "clip": clip,
                        "start": tick_at(grid, start, length, low).get() as f64,
                        "end": tick_at(grid, start, length, high).get() as f64,
                    }),
                )));
            }
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            let note = self.drag.take().is_some();
            let lanes = self.lanes.cancel();
            // A drag on a MIDI Tool's control that Esc ended (the MIDI Tools
            // spec, R22) has put the notes back already.
            let tool = crate::tool_panel::took_escape(ui.ctx());
            let split = self.split_drag.take().is_some();
            let chopping = self.chop_drag.take().is_some();
            let chopped = self.chop.take().is_some();
            if chopping {
                events.push(UiEvent::StatusHint(None));
            }
            if note || lanes == Cancelled::Gesture || split || chopping || chopped {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Cancel,
                    invocation: None,
                });
            } else if tool || lanes == Cancelled::Typed {
                // Spec: note-editing R14. Esc drops a number being typed.
            } else if shell.active_contexts().contains(&Context::NoteEditor) {
                // Spec: piano-roll R24, R25. Esc deselects the notes and
                // clears the time selection.
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::SELECT_NOTES,
                    json!({ "clip": clip, "notes": [], "mode": "replace" }),
                )));
                events.push(UiEvent::Invoke(Invocation::new(
                    "clip_view.select_time",
                    json!({ "clip": clip }),
                )));
            }
        }
        if response.drag_stopped() {
            if let Some(drag) = self.split_drag.take() {
                let last = pointer.map(|pos| {
                    let args = drag.args(arrangement, doc, keys, grid, rows, start, length, pos);
                    Invocation::new(ids::SPLIT, args)
                });
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: last,
                });
            }
            if let Some(drag) = self.chop_drag.take() {
                let last = pointer.map(|pos| {
                    let parts = drag.parts(pos.y, keys.extend);
                    Invocation::new(ids::CHOP, json!({ "clip": drag.clip, "parts": parts }))
                });
                self.chop = None;
                events.push(UiEvent::StatusHint(None));
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: last,
                });
            }
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
            if e_down {
                // Spec: note-editing R33. E and a click splits the note
                // where it was clicked, at the nearest grid line with Cmd
                // (Ctrl on Windows) held.
                if let Some(place) = place {
                    let at =
                        split_tick(arrangement, doc, keys, tick_at(grid, start, length, pos.x));
                    events.push(UiEvent::Invoke(Invocation::new(
                        ids::SPLIT,
                        json!({ "clip": clip, "cuts": [{ "note": place, "at": at }] }),
                    )));
                }
            } else {
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
                    // Spec: piano-roll R5, R26. With Draw Mode off, clicking a
                    // note selects it alone, Cmd-click (Ctrl-click) and
                    // Shift-click add or remove it, and clicking empty grid
                    // clears the selection.
                    (false, Some(place)) => events.push(UiEvent::Invoke(Invocation::new(
                        ids::SELECT_NOTES,
                        json!({
                            "clip": clip,
                            "notes": [place],
                            "mode": if keys.toggle || keys.extend { "toggle" } else { "replace" },
                        }),
                    ))),
                    // Spec: piano-roll R23. Clicking empty grid places the
                    // insert marker on the nearest grid line, and clears the
                    // time selection.
                    (false, None) => {
                        let at = arrangement.snap_drag(
                            doc,
                            tick_at(grid, start, length, pos.x),
                            keys.bypass,
                        );
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::SELECT_NOTES,
                            json!({ "clip": clip, "notes": [], "mode": "replace" }),
                        )));
                        events.push(UiEvent::Invoke(Invocation::new(
                            "clip_view.set_insert_marker",
                            json!({ "clip": clip, "beat": at }),
                        )));
                    }
                }
            }
        }
        // Spec: piano-roll R26. Clicking a key in the piano ruler selects
        // the notes at that pitch, and Shift-clicking adds them or takes
        // them out.
        let keys_rect = Rect::from_min_max(
            pos2(view.left(), grid.top()),
            pos2(grid.left(), grid.bottom()),
        );
        let keys_response =
            ui.interact(keys_rect, Id::new(("clip-view-keys", clip)), Sense::click());
        if keys_response.clicked()
            && let Some(pos) = keys_response.interact_pointer_pos()
            && let Some(key) = rows.key_at(grid, pos.y)
        {
            let key = key.round().clamp(0.0, 127.0) as u8;
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SELECT_NOTES,
                json!({
                    "clip": clip,
                    "notes": [],
                    "select": { "key": key },
                    "add": keys.extend,
                }),
            )));
        }
        // Spec: step-recording R2. With Preview on, a key held in the piano
        // ruler, a note drawn or clicked, and a note dragged to a new pitch
        // sound until the button is let go.
        self.preview_input(
            ui,
            shell,
            keys_rect,
            &frame.layout.grid,
            rows,
            clip,
            notes,
            pointer,
            draw_mode,
            &hit,
            events,
        );
        // Spec: piano-roll R30. Right-clicking a note, which selects it
        // alone first if it isn't selected, or empty grid opens the editor's
        // context menu; right-clicking a key in the piano ruler opens that
        // one's.
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((place, _)) = hit(pos)
            && !selected.contains(&place)
        {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SELECT_NOTES,
                json!({ "clip": clip, "notes": [place], "mode": "replace" }),
            )));
        }
        response.context_menu(|ui| {
            self.menus.show(ui, shell, &context_menus::note(), events);
        });
        // Spec: note-editing R19. Right-clicking a key selects that key's
        // notes first, so the menu's Deactivate Note(s) acts on them.
        if keys_response.secondary_clicked()
            && let Some(pos) = keys_response.interact_pointer_pos()
            && let Some(key) = rows.key_at(grid, pos.y)
        {
            let key = key.round().clamp(0.0, 127.0) as u8;
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SELECT_NOTES,
                json!({ "clip": clip, "notes": [], "select": { "key": key } }),
            )));
        }
        keys_response.context_menu(|ui| {
            self.menus.show(ui, shell, &context_menus::keys(), events);
        });
        // Spec: note-editing R1 to R9. The expression lanes under the note
        // area.
        self.lanes.input(ui, shell, frame, events);
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
        // Spec: view-options R1, piano-roll R44. A middle-button drag pans
        // the note area, the key track following the pointer: the rows move
        // down the view as `top` grows (`y_of_key`), as the wheel moves
        // them.
        if let Some(delta) = crate::middle::pan(ui, grid) {
            if rows.folded {
                self.first = (self.first + delta.y / KEY_HEIGHT).clamp(0.0, rows.scroll_room());
            } else {
                self.top = (self.top + delta.y / KEY_HEIGHT).clamp(0.0, 127.0);
            }
        }
        // Spec: piano-roll. S-A11Y-008 and S-A11Y-009, the note-selection
        // keys: with notes selected, Up and Down transpose them a semitone,
        // and with Shift an octave. The Clip View holds the detail area's
        // keys (the shell's contexts say which apply).
        self.keys(
            ui,
            shell,
            doc,
            arrangement,
            frame,
            length,
            midi,
            notes,
            selected,
            events,
        );
    }

    /// The note editor's keys: the insert marker, the time selection, the
    /// selection from the keyboard, and the moves and resizes from the
    /// keys (piano-roll R23 to R27).
    #[expect(clippy::too_many_arguments, reason = "the editor's whole keyboard")]
    fn keys(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        doc: &Document,
        arrangement: &ArrangementView,
        frame: &note_lanes::Frame<'_>,
        length: Ticks,
        midi: &MidiData,
        notes: &[Note],
        selected: &[u32],
        events: &mut Vec<UiEvent>,
    ) {
        let clip = frame.clip;
        let pressed: Vec<(Key, egui::Modifiers)> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        // Spec: note-editing R14. While a number is being typed, the keys go
        // to it: digits (0 among them), Backspace and Enter.
        let mut pressed = pressed.into_iter();
        while self.lanes.is_typing() {
            let Some((key, modifiers)) = pressed.next() else {
                break;
            };
            self.lanes.type_key(shell, frame, key, modifiers, events);
        }
        let pressed: Vec<(Key, egui::Modifiers)> = pressed.collect();
        // The Clip View holds the detail area's keys (the shell's contexts
        // say which apply).
        if !shell.active_contexts().contains(&Context::NoteEditor) {
            return;
        }
        let insert = shell.clip_view().insert_marker(doc, arrangement);
        let time = shell.clip_view().time_selection(doc, arrangement);
        let span = span(midi, length);
        let step = arrangement.grid_ticks(doc);
        let edges = note_edges(notes, span);
        let mac = cfg!(target_os = "macos");
        let marker = |events: &mut Vec<UiEvent>, at: Ticks| {
            events.push(UiEvent::Invoke(Invocation::new(
                "clip_view.set_insert_marker",
                json!({ "clip": clip, "beat": at }),
            )));
        };
        let selection = |events: &mut Vec<UiEvent>, from: Ticks, to: Ticks| {
            events.push(UiEvent::Invoke(Invocation::new(
                "clip_view.select_time",
                json!({
                    "clip": clip,
                    "start": from.get() as f64,
                    "end": to.get() as f64,
                }),
            )));
        };
        let select = |events: &mut Vec<UiEvent>, what: &str, add: bool| {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SELECT_NOTES,
                json!({ "clip": clip, "notes": [], "select": what, "add": add }),
            )));
        };
        for (key, modifiers) in pressed {
            // Spec: note-editing R14. With notes selected and no number being
            // typed, a digit from 1 to 9 starts a velocity; 0 deactivates
            // them (S-NOTE-041), which the keymap runs.
            if !selected.is_empty() && self.lanes.start_velocity(key, modifiers) {
                continue;
            }
            let shift = modifiers.shift;
            let alt = modifiers.alt;
            let ctrl = modifiers.ctrl;
            // The note-jump keys: Option on macOS, Alt elsewhere.
            let jump = alt;
            // The key that moves the insert marker to a note edge: Option on
            // macOS, Ctrl elsewhere (S-A11Y-006).
            let edge = if mac { alt } else { ctrl };
            // The key that adds to the selection while jumping (S-NOTE-043).
            let adding = if mac { alt && shift } else { ctrl && shift };
            // Spec: step-recording R4 to R6. While step recording, Left and
            // Right step: the held notes are added or lengthened, shortened
            // or removed, and with none held the marker moves a step. With
            // notes selected and none held they move the selection as usual.
            if matches!(key, Key::ArrowLeft | Key::ArrowRight)
                && !(shift || alt || ctrl || modifiers.command)
                && shell.step_recording()
                && (selected.is_empty() || !shell.held_keys().is_empty())
            {
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::STEP_RECORD,
                    json!({ "clip": clip, "back": key == Key::ArrowLeft }),
                )));
                continue;
            }
            match key {
                // Spec: note-editing R35, R36. Cmd-E splits at the insert
                // marker or the time selection's edges, or with notes
                // selected chops them on the grid and opens the chop the
                // arrows then change while Cmd stays held.
                Key::E if modifiers.command && !alt && !shift => {
                    if selected.is_empty() {
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::SPLIT,
                            json!({ "clip": clip }),
                        )));
                    } else {
                        self.chop = Some(ChopRun { steps: Vec::new() });
                        gesture(
                            events,
                            GesturePhase::Update,
                            ids::CHOP,
                            json!({ "clip": clip }),
                        );
                    }
                }
                Key::ArrowUp | Key::ArrowDown
                    if modifiers.command && !alt && self.chop.is_some() =>
                {
                    let step = match (key == Key::ArrowUp, shift) {
                        (true, false) => ChopStep::More,
                        (false, false) => ChopStep::Fewer,
                        (true, true) => ChopStep::NextPower,
                        (false, true) => ChopStep::PreviousPower,
                    };
                    if let Some(run) = self.chop.as_mut() {
                        run.steps.push(step);
                        gesture(
                            events,
                            GesturePhase::Update,
                            ids::CHOP,
                            json!({ "clip": clip, "steps": run.steps }),
                        );
                    }
                }
                Key::ArrowLeft | Key::ArrowRight => {
                    let forward = key == Key::ArrowRight;
                    if jump && shift {
                        // Spec: piano-roll R24. Option-Shift with the
                        // arrows moves the time selection's end to a note
                        // edge.
                        if let Some((from, to)) = time {
                            let end = edge_at(&edges, to, forward).unwrap_or(to);
                            selection(events, from, end.max(from));
                        }
                    } else if shift {
                        // Spec: piano-roll R24, R27. With notes selected,
                        // Shift with the arrows lengthens or shortens them;
                        // with none, it selects time.
                        if selected.is_empty() {
                            // With no time selection, the arrows select
                            // time from the insert marker a step at a time.
                            let (from, to) = match time {
                                Some((from, to)) => {
                                    let to = if forward {
                                        to + step
                                    } else {
                                        (to - step).max(from)
                                    };
                                    (from, to)
                                }
                                None => {
                                    let at = insert.unwrap_or(span.0);
                                    if forward {
                                        (at, at + step)
                                    } else {
                                        ((at - step).max(span.0), at)
                                    }
                                }
                            };
                            selection(events, from.min(to), to.max(from));
                        } else {
                            let by = if forward { step } else { -step };
                            events.push(UiEvent::Invoke(Invocation::new(
                                ids::RESIZE_NOTES,
                                json!({ "clip": clip, "by": by }),
                            )));
                        }
                    } else if jump {
                        // Spec: piano-roll R25. Option with the arrows
                        // selects the previous or next note of the same
                        // pitch; with none selected, on macOS the same
                        // keys move the insert marker to a note edge.
                        if !selected.is_empty() && (mac || !edge) {
                            select(
                                events,
                                if forward {
                                    "same_pitch_next"
                                } else {
                                    "same_pitch_previous"
                                },
                                false,
                            );
                        } else if mac {
                            let at = insert.unwrap_or(span.0);
                            if let Some(edge) = edge_at(&edges, at, forward) {
                                marker(events, edge);
                            }
                        }
                    } else if edge {
                        // Spec: piano-roll R23. Option (Ctrl) with the
                        // arrows moves the insert marker to a note edge.
                        let at = insert.unwrap_or(span.0);
                        if let Some(edge) = edge_at(&edges, at, forward) {
                            marker(events, edge);
                        }
                    } else if selected.is_empty() {
                        // Spec: piano-roll R23. Left and Right move the
                        // insert marker a grid step.
                        let at = insert.unwrap_or(span.0);
                        let at = if forward { at + step } else { at - step };
                        marker(events, at.clamp(span.0, span.1));
                    } else {
                        // Spec: piano-roll R27. Left and Right move the
                        // selection a grid step.
                        let by = if forward { step } else { -step };
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::MOVE_NOTES,
                            json!({ "clip": clip, "by": by }),
                        )));
                    }
                }
                Key::ArrowUp | Key::ArrowDown => {
                    // Spec: note-editing R15, R16. The value keys raise and
                    // lower the selected notes' velocity, deviation, chance,
                    // or the value of the lane that holds the keyboard.
                    if !selected.is_empty()
                        && let Some((field, by)) =
                            self.lanes.value_key(key == Key::ArrowUp, modifiers, mac)
                    {
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::ADJUST_VALUE,
                            json!({ "clip": clip, "field": field, "by": by }),
                        )));
                        continue;
                    }
                    // Spec: piano-roll R25. Option with the arrows selects
                    // the note nearest the insert marker, and the previous
                    // or next note after that.
                    let forward = key == Key::ArrowUp;
                    if adding {
                        // Spec: piano-roll R25. Shift with the note-jump keys
                        // adds the note they reach: the nearest one when
                        // nothing is selected, and the next or previous one
                        // after that.
                        let what = if selected.is_empty() {
                            "nearest"
                        } else if forward {
                            "next"
                        } else {
                            "previous"
                        };
                        select(events, what, true);
                    } else if jump {
                        if selected.is_empty() {
                            select(events, "nearest", false);
                        } else {
                            select(events, if forward { "next" } else { "previous" }, false);
                        }
                    } else if !selected.is_empty()
                        && !modifiers.command
                        && !modifiers.ctrl
                        && !modifiers.alt
                    {
                        // Spec: piano-roll R20. Up and Down transpose the
                        // selected notes a semitone, and with Shift an
                        // octave.
                        let semitones = match (key, shift) {
                            (Key::ArrowUp, true) => 12,
                            (Key::ArrowUp, false) => 1,
                            (_, true) => -12,
                            (_, false) => -1,
                        };
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::MOVE_NOTES,
                            json!({ "clip": clip, "semitones": semitones }),
                        )));
                    }
                }
                Key::Home | Key::End => {
                    // Spec: piano-roll R23. Home and End move the insert
                    // marker to the clip's first and last tick.
                    let at = if key == Key::Home { span.0 } else { span.1 };
                    marker(events, at);
                }
                Key::Enter => {
                    // Spec: piano-roll R24. Enter with a time selection and
                    // no notes selected selects the notes that start inside
                    // it, keeping the time selection; with notes selected it
                    // deselects them and keeps the time selection.
                    if !selected.is_empty() {
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::SELECT_NOTES,
                            json!({ "clip": clip, "notes": [], "mode": "replace" }),
                        )));
                    } else if let Some((from, to)) = time {
                        let inside: Vec<u32> = notes
                            .iter()
                            .enumerate()
                            .filter(|(_, note)| note.start >= from && note.start < to)
                            .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
                            .collect();
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::SELECT_NOTES,
                            json!({ "clip": clip, "notes": inside, "mode": "replace" }),
                        )));
                        selection(events, from, to);
                    }
                }
                _ => {}
            }
        }
    }

    /// The marker strip's clicks and drags: each drag moves a marker or the
    /// loop brace, through the document's clip commands, as one gesture
    /// (piano-roll R11).
    #[expect(clippy::too_many_arguments, reason = "the strip's whole input")]
    fn strip_input(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        arrangement: &ArrangementView,
        strip: Rect,
        grid: Rect,
        clip: ClipId,
        midi: &MidiData,
        axis: Ticks,
        length: Ticks,
        stretch: Option<(Ticks, Ticks)>,
        events: &mut Vec<UiEvent>,
    ) {
        let response = ui.interact(
            strip,
            Id::new(("clip-view-strip", clip)),
            Sense::click_and_drag(),
        );
        let keys = EditKeys::from(ui.input(|i| i.modifiers));
        // Spec: note-editing R42 to R44. The stretch markers sit at the
        // time selection's ends, in the band between the clip markers and
        // the loop brace; a pointer between them shows the pseudo marker.
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && let Some(range) = stretch
            && let Some(part) = hit_stretch(strip, grid, axis, length, range, press)
        {
            self.stretch_drag = Some(StretchDrag { clip, part, range });
        } else if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && let Some(part) = hit_strip(strip, grid, axis, length, midi, press)
        {
            self.strip_drag = Some(StripDrag {
                clip,
                part,
                grab: tick_at(grid, axis, length, press.x),
                start: midi.start,
                end: midi.end,
                loop_start: midi.loop_start,
                loop_end: midi.loop_end,
                looping: midi.looping,
                length,
            });
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        if response.dragged()
            && let (Some(drag), Some(pos)) = (self.stretch_drag, pointer)
        {
            let args = drag.args(arrangement, doc, keys, tick_at(grid, axis, length, pos.x));
            gesture(events, GesturePhase::Update, ids::STRETCH, args);
        } else if response.dragged()
            && let (Some(drag), Some(pos)) = (self.strip_drag, pointer)
        {
            let (id, args) = drag.invocation(arrangement, doc, keys, grid, pos);
            gesture(events, GesturePhase::Update, id, args);
        }
        // Spec: multi-clip-editing R11. A click on the loop brace selects it,
        // which gives it Cmd-D, Duplicate Loop.
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && matches!(
                hit_strip(strip, grid, axis, length, midi, pos),
                Some(StripPart::LoopStart | StripPart::LoopEnd | StripPart::LoopBody)
            )
        {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SELECT_LOOP_BARS,
                json!({ "clips": [clip] }),
            )));
        }
        if (self.strip_drag.is_some() || self.stretch_drag.is_some())
            && ui.input(|i| i.key_pressed(Key::Escape))
        {
            self.strip_drag = None;
            self.stretch_drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
        if response.drag_stopped()
            && let Some(drag) = self.stretch_drag.take()
        {
            let last = pointer.map(|pos| {
                let args = drag.args(arrangement, doc, keys, tick_at(grid, axis, length, pos.x));
                Invocation::new(ids::STRETCH, args)
            });
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: last,
            });
        } else if response.drag_stopped() {
            let last = match (self.strip_drag, pointer) {
                (Some(drag), Some(pos)) => {
                    let (id, args) = drag.invocation(arrangement, doc, keys, grid, pos);
                    Some(Invocation::new(id, args))
                }
                _ => None,
            };
            if self.strip_drag.take().is_some() {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: last,
                });
            }
        }
        if let Some(hover) = response.hover_pos()
            && (stretch.is_some_and(|range| {
                hit_stretch(strip, grid, axis, length, range, hover).is_some()
            }) || hit_strip(strip, grid, axis, length, midi, hover).is_some())
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
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
                targets,
                start: from,
                grab_key,
                grab,
                limits,
                copy,
                grabbed: _,
            } => {
                // Spec: piano-roll R10. With snapping on, the grabbed note's
                // start lands on the grid, and Cmd (Alt) bypasses it.
                let snapped = arrangement.snap_drag(doc, at - *grab, keys.bypass);
                let by = (snapped - *from).max(limits.0);
                let key = rows.key_at(grid, pos.y).unwrap_or(*grab_key);
                let semitones = ((key - *grab_key).round() as i32).clamp(limits.1, limits.2);
                (
                    ids::MOVE_NOTES,
                    named(
                        targets,
                        json!({ "by": by, "semitones": semitones, "copy": copy }),
                    ),
                )
            }
            Self::Undecided { moving, .. } => {
                moving.invocation(arrangement, doc, keys, grid, rows, start, length, pos)
            }
            Self::Velocity { targets, from } => (
                ids::ADJUST_VALUE,
                named(
                    targets,
                    json!({
                        "field": NoteField::Velocity,
                        // One for each point dragged up (note-editing R17).
                        "by": (*from - pos.y).round(),
                    }),
                ),
            ),
            Self::Resize {
                targets,
                end,
                grab,
                limit,
            } => {
                let snapped = arrangement.snap_drag(doc, at - *grab, keys.bypass);
                let by = (snapped - *end).max(*limit);
                (ids::RESIZE_NOTES, named(targets, json!({ "by": by })))
            }
        }
    }
}

impl StripDrag {
    /// The drag's command for the pointer at `pos`, measured from where the
    /// drag began: a marker's or the loop brace's new place in the clip's
    /// contents, snapped to the editing grid (Cmd — Alt on Windows —
    /// bypasses it).
    fn invocation(
        &self,
        arrangement: &ArrangementView,
        doc: &Document,
        keys: EditKeys,
        grid: Rect,
        pos: Pos2,
    ) -> (&'static str, Value) {
        let at = tick_at(grid, self.start, self.length, pos.x);
        let snapped = arrangement.snap_drag(doc, at, keys.bypass);
        let delta = snapped - self.grab;
        let clip_end = self.start + self.length;
        let end = self.end.unwrap_or(clip_end);
        let loop_end = self.loop_end.unwrap_or(clip_end);
        match self.part {
            // Spec: midi-clips R4. The Start marker stays before the End
            // marker, and before the brace's end while the loop is on.
            StripPart::Start => {
                let mut upper = end;
                if self.looping {
                    upper = upper.min(loop_end);
                }
                let start =
                    (self.start + delta).clamp(Ticks::ZERO, (upper - Ticks(1)).max(Ticks::ZERO));
                (
                    SET_MIDI_REGION,
                    json!({ "clip": self.clip, "start": start, "end": self.end }),
                )
            }
            StripPart::End => {
                let end = (end + delta).max(self.start + Ticks(1));
                (
                    SET_MIDI_REGION,
                    json!({ "clip": self.clip, "start": self.start, "end": end }),
                )
            }
            StripPart::LoopStart => {
                let start = (self.loop_start + delta)
                    .clamp(Ticks::ZERO, (loop_end - Ticks(1)).max(Ticks::ZERO));
                (
                    SET_MIDI_LOOP,
                    json!({ "clip": self.clip, "start": start, "end": self.loop_end }),
                )
            }
            StripPart::LoopEnd => {
                let end = (loop_end + delta).max(self.loop_start + Ticks(1));
                (
                    SET_MIDI_LOOP,
                    json!({ "clip": self.clip, "start": self.loop_start, "end": end }),
                )
            }
            // The body moves the whole brace, keeping its length.
            StripPart::LoopBody => {
                let shift = delta.max(-self.loop_start);
                (
                    SET_MIDI_LOOP,
                    json!({
                        "clip": self.clip,
                        "start": self.loop_start + shift,
                        "end": loop_end + shift,
                    }),
                )
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
}
