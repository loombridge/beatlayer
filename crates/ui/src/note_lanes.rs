//! The MIDI Note Editor's expression lanes and footer (WP-6.1): the
//! Velocity, Chance and Release Velocity lanes under the note area, the
//! Lane Selector and the Show/Hide All Expression Editors switch, and the
//! footer with the active lane's controls.
//!
//! Each lane shows one value of every note as a marker at the note's start.
//! Dragging a marker changes its note's value, or every selected note's by
//! the same amount; with Draw Mode on, dragging across a lane draws values;
//! Cmd-dragging a Velocity marker sets its deviation. Every gesture is one
//! undo step through the same commands agents run (`note.adjust_value`,
//! `note.set_value`, `note.randomize`, `note.ramp_value`), and the lanes
//! shown, the active lane and Randomize's amounts are the shell's
//! (`view.lanes`, `view.lane`). Only the lanes' heights, the drags under way
//! and a number being typed live here. Spec: note-editing R1 to R17.

use std::collections::BTreeSet;

use bl_core::time::Ticks;
use bl_model::{ClipId, GroupKind, Note, NoteGroup};
use bl_shell::clip_view::note_value;
use bl_shell::commands::{VIEW_LANE, VIEW_LANES};
use bl_shell::note_commands::{self as ids, NoteField, RandomizeAmounts};
use bl_shell::{GesturePhase, Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, CursorIcon, Id, Key, Modifiers, Pos2, Rect, Sense, Shape, Stroke, StrokeKind,
    Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::clip_view::{
    KEY_HEIGHT, KEY_WIDTH, LANE_HEIGHT, STEM_WIDTH, STRIP_HEIGHT, TOOLBAR_HEIGHT, lane_labels,
    lane_value, stem_x, y_of_value,
};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How tall the footer under the lanes is (L-CLIP-015: y 948 to 961).
pub const FOOTER_HEIGHT: f32 = 14.0;
/// The least the note area keeps, however tall the lanes are made.
const MIN_GRID: f32 = 4.0 * KEY_HEIGHT;
/// A lane dragged shorter than this is hidden (I-DRAG-038).
const MIN_LANE: f32 = 8.0;
/// The tallest a lane is made.
const MAX_LANE: f32 = 400.0;
/// How far from a marker's stem, in points, a press still grabs it.
const MARKER_GRAB: f32 = 3.0;
/// Points of vertical drag for one step of a footer value.
const POINTS_PER_STEP: f32 = 3.0;
/// How wide the room the footer leaves for its MIDI tool menu is (L-CLIP-015,
/// built by a later package).
const MIDI_TOOL_ROOM: f32 = 64.0;
/// How tall a key's row must be for the corner triangle of a note that
/// doesn't always play to show (note-editing R4; Beatlayer's own).
pub const CORNER_ROW: f32 = 9.0;
/// How big that triangle is.
const CORNER: f32 = 4.0;
/// The most digits a typed value takes.
const MAX_TYPED: usize = 3;

/// Where the parts of the note editor's body sit: the note area, each shown
/// lane, and the footer, from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteLayout {
    /// The note area.
    pub grid: Rect,
    /// The lanes shown, from the top, each as wide as the Clip View: its
    /// labels sit left of the note area.
    pub lanes: Vec<(NoteField, Rect)>,
    /// The footer under the lanes.
    pub footer: Rect,
}

impl NoteLayout {
    /// The rectangle of `field`'s lane, when it shows.
    #[must_use]
    pub fn lane(&self, field: NoteField) -> Option<Rect> {
        self.lanes
            .iter()
            .find(|(lane, _)| *lane == field)
            .map(|(_, rect)| *rect)
    }

    /// The part of a lane that holds the markers: right of its header.
    #[must_use]
    pub fn plot(&self, lane: Rect) -> Rect {
        Rect::from_min_max(pos2(self.grid.left(), lane.top()), lane.max)
    }
}

/// The note editor's body in `view`: the footer along the bottom, the lanes
/// above it with the heights given, and the note area above them, which
/// keeps room for a few rows however tall the lanes are made.
#[must_use]
pub fn note_layout(view: Rect, heights: &[(NoteField, f32)]) -> NoteLayout {
    let top = view.top() + TOOLBAR_HEIGHT + STRIP_HEIGHT;
    let footer_top = (view.bottom() - FOOTER_HEIGHT).max(top);
    let room = (footer_top - top - MIN_GRID).max(0.0);
    let wanted: f32 = heights.iter().map(|(_, h)| h).sum();
    let scale = if wanted > room && wanted > 0.0 {
        room / wanted
    } else {
        1.0
    };
    let total = wanted * scale;
    let grid_bottom = footer_top - total;
    let mut y = grid_bottom;
    let lanes = heights
        .iter()
        .map(|(field, h)| {
            let rect = Rect::from_min_max(pos2(view.left(), y), pos2(view.max.x, y + h * scale));
            y += h * scale;
            (*field, rect)
        })
        .collect();
    NoteLayout {
        grid: Rect::from_min_max(
            pos2(view.left() + KEY_WIDTH, top),
            pos2(view.max.x, grid_bottom),
        ),
        lanes,
        footer: Rect::from_min_max(pos2(view.left(), footer_top), view.max),
    }
}

/// Where the footer's controls sit for the active lane `lane`, by name: the
/// Show/Hide All switch and the Lane Selector at its left, room for the MIDI
/// tool menu, then Randomize with its amount, and the lane's own controls
/// (note-editing R10; L-CLIP-015).
#[must_use]
pub fn footer_rects(footer: Rect, lane: NoteField) -> Vec<(&'static str, Rect)> {
    let y0 = footer.top() + 1.0;
    let h = footer.height() - 2.0;
    let mut out = vec![
        (
            "Show/Hide All Expression Editors",
            Rect::from_min_size(pos2(footer.left() + 2.0, y0), vec2(11.0, h)),
        ),
        (
            "Lane Selector",
            Rect::from_min_size(pos2(footer.left() + 14.0, y0), vec2(11.0, h)),
        ),
    ];
    let mut x = footer.left() + KEY_WIDTH + MIDI_TOOL_ROOM;
    let mut push = |name: &'static str, w: f32| {
        out.push((name, Rect::from_min_size(pos2(x, y0), vec2(w, h))));
        x += w + 3.0;
    };
    push("Randomize", 58.0);
    push("Randomize Amount", 30.0);
    if lane == NoteField::Chance {
        push("Play All", 48.0);
        push("Play One", 48.0);
        push("Ungroup", 48.0);
    } else {
        push("Ramp", 26.0);
        push("Ramp Start", 30.0);
        push("Ramp End", 30.0);
        if lane == NoteField::Velocity {
            push("Deviation", 44.0);
            push("Velocity Deviation", 40.0);
        }
    }
    out
}

/// Whether a note shows the small triangle at its top-left corner: when it
/// doesn't always play, or belongs to a probability group, and its key row
/// is at least [`CORNER_ROW`] points tall (note-editing R4, R24).
#[must_use]
pub fn shows_corner(note: &Note, row_height: f32) -> bool {
    (note.probability < 1.0 || note.group.is_some()) && row_height >= CORNER_ROW
}

/// Whether the pointer marks the note at `place`: it hovers the note or its
/// marker, or one of its probability group (note-editing R4, R24).
#[must_use]
pub fn is_marked(notes: &[Note], hovered: Option<u32>, place: usize) -> bool {
    hovered
        .and_then(|h| notes.get(h as usize))
        .is_some_and(|h| {
            hovered == u32::try_from(place).ok()
                || (h.group.is_some() && h.group == notes[place].group)
        })
}

/// For each note, whether it wears a marker in the Chance lane: a group
/// shows one, at its first note, in place of its notes' own (note-editing
/// R24).
fn leaders(notes: &[Note]) -> Vec<bool> {
    let mut seen = BTreeSet::new();
    notes
        .iter()
        .map(|note| note.group.is_none_or(|group| seen.insert(group)))
        .collect()
}

/// The places of the notes in `group`.
fn members(notes: &[Note], group: u32) -> Vec<u32> {
    notes
        .iter()
        .enumerate()
        .filter(|(_, note)| note.group == Some(group))
        .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
        .collect()
}

/// A group's mark at the top of its marker: a diamond for Play All, a
/// triangle for Play One (note-editing R24).
fn paint_group_mark(painter: &egui::Painter, at: Pos2, kind: GroupKind, colour: Color32) {
    let r = 3.0;
    let points = match kind {
        GroupKind::PlayAll => vec![
            at + vec2(0.0, -r),
            at + vec2(r, 0.0),
            at + vec2(0.0, r),
            at + vec2(-r, 0.0),
        ],
        GroupKind::PlayOne => vec![at + vec2(0.0, -r), at + vec2(r, r), at + vec2(-r, r)],
    };
    painter.add(Shape::convex_polygon(points, colour, Stroke::NONE));
}

/// Paints the corner triangle at a note's top-left corner.
pub fn paint_corner(painter: &egui::Painter, rect: Rect, colour: Color32) {
    let at = rect.left_top();
    painter.add(Shape::convex_polygon(
        vec![at, at + vec2(CORNER, 0.0), at + vec2(0.0, CORNER)],
        colour,
        Stroke::NONE,
    ));
}

/// What a lane is called.
#[must_use]
pub fn lane_title(field: NoteField) -> &'static str {
    match field.lane() {
        NoteField::Chance => "Chance",
        NoteField::ReleaseVelocity => "Release Velocity",
        _ => "Velocity",
    }
}

fn lane_index(field: NoteField) -> usize {
    NoteField::LANES
        .iter()
        .position(|lane| *lane == field.lane())
        .unwrap_or(0)
}

/// One step of a footer value's drag, in the field's own units: a whole
/// number, or one percent of a chance.
fn unit(field: NoteField) -> f64 {
    if field.whole() { 1.0 } else { 0.01 }
}

/// A value as the lanes and the footer write it: a whole number, or a
/// percentage for chance.
#[must_use]
pub fn value_text(field: NoteField, value: f64) -> String {
    if field.whole() {
        format!("{}", value.round())
    } else {
        format!("{}%", (value * 100.0).round())
    }
}

/// One value, or the lowest and highest where they differ.
fn range_text(field: NoteField, low: f64, high: f64) -> String {
    let (low_text, high_text) = (value_text(field, low), value_text(field, high));
    if low_text == high_text {
        low_text
    } else {
        format!("{}–{high_text}", low_text.trim_end_matches('%'))
    }
}

/// The footer's controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    /// The velocity typed on the keys, which no footer field holds.
    Velocity,
    Amount,
    RampStart,
    RampEnd,
    Deviation,
}

impl Control {
    /// The field the control sets for the lane `lane`.
    fn field(self, lane: NoteField) -> NoteField {
        match self {
            Self::Deviation => NoteField::VelocityDeviation,
            Self::Velocity => NoteField::Velocity,
            _ => lane,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Velocity => "Velocity",
            Self::Amount => "Randomize Amount",
            Self::RampStart => "Ramp Start",
            Self::RampEnd => "Ramp End",
            Self::Deviation => "Velocity Deviation",
        }
    }
}

/// A number being typed: into a footer field, or as a velocity.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Typed {
    control: Control,
    text: String,
}

/// What a drag in a lane is doing.
#[derive(Clone, Debug, PartialEq)]
enum LaneDrag {
    /// A marker's drag: the grabbed note, or every selected note, moves by
    /// the same amount (R5).
    Marker {
        field: NoteField,
        places: Vec<u32>,
        /// Where the press fell, y.
        from: f32,
    },
    /// A Cmd-drag from a Velocity marker sets the deviation (R7).
    Deviation {
        places: Vec<u32>,
        /// The grabbed note's velocity.
        velocity: f64,
    },
    /// A drag in Draw Mode draws values (R6).
    Draw {
        field: NoteField,
        /// The notes drawn on: the selected ones, or none for every note.
        only: Vec<u32>,
        /// Where the pointer has been, in order, from the press.
        path: Vec<Pos2>,
    },
}

/// A drag of a footer value.
#[derive(Clone, Debug, PartialEq)]
struct FooterDrag {
    control: Control,
    /// The value when the drag began, and the other end of a ramp.
    base: f64,
    other: f64,
    /// Seeds Randomize's draw, so each note keeps its own through the drag.
    seed: u64,
    /// Whether notes were selected: only then does moving the amount
    /// randomise them.
    notes: bool,
    /// The value the drag last reached.
    last: f64,
}

/// The lanes' own view state: their heights, the drags under way, the
/// lane that holds the keyboard, and a number being typed.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneUi {
    heights: [f32; 3],
    drag: Option<LaneDrag>,
    footer_drag: Option<FooterDrag>,
    /// Whether a divider between lanes is being dragged.
    divider: bool,
    /// The lane whose marker was clicked last, which has the keyboard until
    /// the note area is clicked (R16).
    focus: Option<NoteField>,
    typed: Option<Typed>,
    /// The group the Chance lane's marker menu opened on (C-PGROUP): its
    /// ID and the places of its notes.
    group_menu: Option<(u32, Vec<u32>)>,
}

impl Default for LaneUi {
    fn default() -> Self {
        Self {
            heights: [LANE_HEIGHT; 3],
            drag: None,
            footer_drag: None,
            divider: false,
            focus: None,
            typed: None,
            group_menu: None,
        }
    }
}

/// What the lanes need of the editor's frame.
pub struct Frame<'a> {
    pub clip: ClipId,
    pub layout: &'a NoteLayout,
    /// The contents tick at the note area's left edge, and the clip's
    /// length in ticks.
    pub start: Ticks,
    pub length: Ticks,
    pub notes: &'a [Note],
    /// The clip's probability groups.
    pub groups: &'a [NoteGroup],
    pub selected: &'a [u32],
    /// The clip's track colour.
    pub face: Color32,
    /// The note the pointer marks, by hovering it or its marker (R4).
    pub hovered: Option<u32>,
    /// The editing grid's step in ticks, and whether it snaps.
    pub step: Ticks,
    pub snap: bool,
    /// The tuning the clip's track plays, which names the notes (the tunings
    /// spec, R29).
    pub tuning: Option<&'a bl_model::Tuning>,
}

/// The values of the selected notes' `field`, for the footer.
struct Summary {
    count: usize,
    low: f64,
    high: f64,
    /// The first and last selected note's value, by start.
    first: f64,
    last: f64,
    /// Whether every selected note starts together.
    together: bool,
}

fn summary(frame: &Frame<'_>, field: NoteField) -> Option<Summary> {
    let mut places: Vec<u32> = frame.selected.to_vec();
    places.sort_unstable();
    let notes: Vec<&Note> = places
        .iter()
        .filter_map(|place| frame.notes.get(*place as usize))
        .collect();
    let (first, last) = (notes.first()?, notes.last()?);
    let values: Vec<f64> = notes.iter().map(|note| note_value(note, field)).collect();
    Some(Summary {
        count: notes.len(),
        low: values.iter().copied().fold(f64::INFINITY, f64::min),
        high: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        first: note_value(first, field),
        last: note_value(last, field),
        together: first.start == last.start,
    })
}

/// The place of the marker in `lane` the pointer at `pos` grabs: the nearest
/// stem within reach, a little above its top or anywhere down it.
fn marker_at(
    grid: Rect,
    (start, length): (Ticks, Ticks),
    notes: &[Note],
    field: NoteField,
    lane: Rect,
    pos: Pos2,
) -> Option<u32> {
    let leads = (field == NoteField::Chance).then(|| leaders(notes));
    notes
        .iter()
        .enumerate()
        .filter_map(|(place, note)| {
            if leads.as_ref().is_some_and(|leads| !leads[place]) {
                return None;
            }
            let x = stem_x(grid, start, length, note) + STEM_WIDTH / 2.0;
            let top = y_of_value(lane, field, note_value(note, field));
            let reach = STEM_WIDTH / 2.0 + MARKER_GRAB;
            ((pos.x - x).abs() <= reach && pos.y >= top - MARKER_GRAB && pos.y <= lane.bottom())
                .then(|| (place, (pos.x - x).abs(), (pos.y - top).abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|(place, ..)| u32::try_from(place).unwrap_or(u32::MAX))
}

impl Frame<'_> {
    /// The kind of the probability group `group`.
    fn kind_of(&self, group: u32) -> Option<GroupKind> {
        self.groups.iter().find(|g| g.id == group).map(|g| g.kind)
    }

    /// The marker of `field`'s lane under `pos`.
    fn marker_at(&self, field: NoteField, lane: Rect, pos: Pos2) -> Option<u32> {
        marker_at(
            self.layout.grid,
            (self.start, self.length),
            self.notes,
            field,
            lane,
            pos,
        )
    }
}

/// The place of the note whose marker is under `pos` in any shown lane.
#[must_use]
pub fn hovered_marker(
    layout: &NoteLayout,
    span: (Ticks, Ticks),
    notes: &[Note],
    pos: Pos2,
) -> Option<u32> {
    layout.lanes.iter().find_map(|(field, lane)| {
        layout
            .plot(*lane)
            .contains(pos)
            .then(|| marker_at(layout.grid, span, notes, *field, *lane, pos))
            .flatten()
    })
}

/// The pointer's height along a path at `x`: the last stretch of the path
/// that covers `x`, or an end of it.
fn path_y(path: &[Pos2], x: f32) -> f32 {
    let mut found = None;
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if (a.x.min(b.x)..=a.x.max(b.x)).contains(&x) {
            found = Some(if (b.x - a.x).abs() < f32::EPSILON {
                b.y
            } else {
                a.y + (b.y - a.y) * (x - a.x) / (b.x - a.x)
            });
        }
    }
    found
        .or_else(|| {
            let (first, last) = (path.first()?, path.last()?);
            Some(if (x - first.x).abs() < (x - last.x).abs() {
                first.y
            } else {
                last.y
            })
        })
        .unwrap_or_default()
}

fn gesture(events: &mut Vec<UiEvent>, phase: GesturePhase, id: &str, args: Value) {
    events.push(UiEvent::Gesture {
        phase,
        invocation: Some(Invocation::new(id, args)),
    });
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// What cancelling did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cancelled {
    /// Nothing was under way.
    Nothing,
    /// A number being typed was dropped.
    Typed,
    /// A drag was dropped, and a gesture that edits the set must be too.
    Gesture,
}

impl LaneUi {
    /// How tall the lane `field` is.
    #[must_use]
    pub fn height(&self, field: NoteField) -> f32 {
        self.heights[lane_index(field)]
    }

    /// The shown lanes with their heights, from the top.
    #[must_use]
    pub fn heights(&self, shown: &[NoteField]) -> Vec<(NoteField, f32)> {
        shown
            .iter()
            .map(|lane| (*lane, self.height(*lane)))
            .collect()
    }

    /// Whether a number is being typed, so the keys go to it and not to
    /// shortcuts (R14).
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.typed.is_some()
    }

    /// The lane that holds the keyboard after a marker was clicked (R16).
    #[must_use]
    pub fn focus(&self) -> Option<NoteField> {
        self.focus
    }

    /// Gives the keyboard back to the note area.
    pub fn leave_focus(&mut self) {
        self.focus = None;
    }

    /// The value an Up or Down key changes and by how much, in the field's
    /// own units, for the selected notes (R15, R16): with a lane holding the
    /// keyboard its own keys, otherwise Cmd with the arrows for velocity,
    /// Cmd-Shift for deviation and Cmd-Option for chance. `None` when the
    /// key isn't one of them.
    #[must_use]
    pub fn value_key(&self, up: bool, modifiers: Modifiers, mac: bool) -> Option<(NoteField, f64)> {
        let sign = if up { 1.0 } else { -1.0 };
        let fine = modifiers.shift;
        if let Some(lane) = self.focus {
            match lane {
                NoteField::Chance if !(modifiers.command || modifiers.ctrl || modifiers.alt) => {
                    return Some((lane, sign * if fine { 0.01 } else { 0.1 }));
                }
                NoteField::Velocity | NoteField::ReleaseVelocity
                    if modifiers.command && !modifiers.alt =>
                {
                    return Some((lane, sign * if fine { 1.0 } else { 10.0 }));
                }
                _ => {}
            }
        }
        if modifiers.command && modifiers.alt && !fine {
            Some((NoteField::Chance, sign * 0.1))
        } else if modifiers.command && !modifiers.alt && !fine {
            Some((NoteField::Velocity, sign * 10.0))
        } else if modifiers.command && !modifiers.alt && fine && mac {
            // On Windows, Ctrl-Shift with the arrows already adds a note
            // to the selection (S-NOTE-043).
            Some((NoteField::VelocityDeviation, sign * 10.0))
        } else {
            None
        }
    }

    /// Drops everything under way, as another clip showing does.
    pub fn reset(&mut self) {
        self.drag = None;
        self.footer_drag = None;
        self.divider = false;
        self.focus = None;
        self.typed = None;
    }

    /// Esc: drops a drag, or else a number being typed.
    pub fn cancel(&mut self) -> Cancelled {
        if self.drag.take().is_some() || self.footer_drag.take().is_some() {
            Cancelled::Gesture
        } else if self.typed.take().is_some() {
            Cancelled::Typed
        } else {
            Cancelled::Nothing
        }
    }

    /// Paints the lanes: each one's labels and header readout, and a marker
    /// for each note (R1, R4, R7).
    pub fn paint(&self, ui: &Ui, shell: &Shell, theme: &Theme, frame: &Frame<'_>) {
        let painter = ui.painter();
        let colours = &theme.colors;
        let active = shell.clip_view().lane();
        let grid = frame.layout.grid;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        for (field, lane) in &frame.layout.lanes {
            if lane.height() < 1.0 {
                continue;
            }
            painter.rect_filled(*lane, 0.0, colours.panel.0);
            painter.line_segment(
                [lane.left_top(), lane.right_top()],
                Stroke::new(1.0, colours.lane_line.0),
            );
            if *field == active {
                // The active lane's header wears the selection's colour.
                painter.line_segment(
                    [
                        lane.left_top() + vec2(1.0, 1.0),
                        lane.left_bottom() + vec2(1.0, 0.0),
                    ],
                    Stroke::new(1.0, colours.accent.0),
                );
            }
            // The lane's three heights, named in its header: 127, 64 and 1
            // for the velocities (L-CLIP-014).
            let (low, high) = field.range();
            let readout = self.readout(frame, *field, *lane, pointer);
            for (at, (value, label)) in [
                (high, lane_labels(*field)[0]),
                ((low + high) / 2.0, lane_labels(*field)[1]),
                (low, lane_labels(*field)[2]),
            ]
            .into_iter()
            .enumerate()
            {
                // The highest and lowest labels sit on the lane's edges, so
                // they keep half a row inside it.
                let y = y_of_value(*lane, *field, value).clamp(
                    lane.top() + KEY_HEIGHT / 2.0,
                    (lane.bottom() - KEY_HEIGHT / 2.0).max(lane.top() + KEY_HEIGHT / 2.0),
                );
                // The header's readout takes the middle label's place
                // (Spec: note-editing R3).
                let (text, ink) = match (&readout, at) {
                    (Some(text), 1) => (text.as_str(), colours.text.0),
                    _ => (label, colours.text_dim.0),
                };
                painter.text(
                    pos2(grid.left() - 4.0, y),
                    Align2::RIGHT_CENTER,
                    text,
                    theme.small_font(),
                    ink,
                );
            }
            let leads = (*field == NoteField::Chance).then(|| leaders(frame.notes));
            for (index, note) in frame.notes.iter().enumerate() {
                if leads.as_ref().is_some_and(|leads| !leads[index]) {
                    continue;
                }
                let x = stem_x(grid, frame.start, frame.length, note);
                if x + STEM_WIDTH < grid.left() || x > grid.right() {
                    continue;
                }
                let place = u32::try_from(index).unwrap_or(u32::MAX);
                let top = y_of_value(*lane, *field, note_value(note, *field));
                let stem = Rect::from_min_max(pos2(x, top), pos2(x + STEM_WIDTH, lane.bottom()));
                // A group's marker stands for every note of it: it is chosen
                // when one is, and marked when the pointer marks one.
                let chosen = frame.selected.contains(&place)
                    || (leads.is_some()
                        && note.group.is_some()
                        && frame.selected.iter().any(|s| {
                            frame
                                .notes
                                .get(*s as usize)
                                .is_some_and(|n| n.group == note.group)
                        }));
                let marked = is_marked(frame.notes, frame.hovered, index);
                let ink = if chosen {
                    colours.time_selection.0
                } else if note.mute {
                    colours.pending.0
                } else {
                    frame.face
                };
                // Spec: note-editing R7. A deviation shows as a shaded band
                // from the marker's top to the velocity it reaches.
                if *field == NoteField::Velocity && note.velocity_deviation != 0 {
                    let reach = f64::from(note.velocity) + f64::from(note.velocity_deviation);
                    let end = y_of_value(*lane, *field, reach);
                    let band = Rect::from_min_max(
                        pos2(x, top.min(end)),
                        pos2(x + STEM_WIDTH, top.max(end)),
                    );
                    painter.rect_filled(band, 0.0, ink.gamma_multiply(0.4));
                }
                painter.rect_filled(stem, 0.0, ink);
                if marked {
                    painter.rect_stroke(
                        stem.expand(1.0),
                        0.0,
                        Stroke::new(1.0, colours.text.0),
                        StrokeKind::Outside,
                    );
                }
                if *field == NoteField::Velocity && note.velocity_deviation != 0 {
                    painter.circle_filled(
                        pos2(x + STEM_WIDTH / 2.0, top + 1.5),
                        1.0,
                        colours.panel.0,
                    );
                }
                if leads.is_some()
                    && let Some(kind) = note.group.and_then(|group| frame.kind_of(group))
                {
                    paint_group_mark(painter, pos2(x + STEM_WIDTH / 2.0, top), kind, ink);
                }
            }
        }
    }

    /// What a lane's header reads besides its labels: the value of the
    /// marker the pointer is over or is dragging (R3), or of the selected
    /// notes' while they differ (R9), or a velocity being typed (R14).
    fn readout(
        &self,
        frame: &Frame<'_>,
        field: NoteField,
        lane: Rect,
        pointer: Option<Pos2>,
    ) -> Option<String> {
        if let Some(Typed {
            control: Control::Velocity,
            text,
        }) = &self.typed
            && field == NoteField::Velocity
        {
            return Some(format!("{text}|"));
        }
        let dragged = match &self.drag {
            Some(LaneDrag::Marker {
                field: dragged,
                places,
                ..
            }) if *dragged == field => places.first().copied(),
            _ => None,
        };
        let under = dragged.or_else(|| {
            pointer
                .filter(|pos| frame.layout.plot(lane).contains(*pos))
                .and_then(|pos| frame.marker_at(field, lane, pos))
        });
        if let Some(note) = under.and_then(|place| frame.notes.get(place as usize)) {
            return Some(value_text(field, note_value(note, field)));
        }
        let selected = summary(frame, field)?;
        (selected.low != selected.high).then(|| range_text(field, selected.low, selected.high))
    }

    /// The lanes' clicks, drags and wheel, the dividers between them, the
    /// header's menu, and what assistive technology reads of them.
    #[expect(clippy::too_many_lines, reason = "one pass over the lanes' input")]
    pub fn input(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        frame: &Frame<'_>,
        events: &mut Vec<UiEvent>,
    ) {
        let modifiers = ui.input(|i| i.modifiers);
        let draw_mode = shell.clip_view().draw_mode();
        let active = shell.clip_view().lane();
        // A lane hidden since its marker was clicked gives the keyboard back.
        if self
            .focus
            .is_some_and(|focus| !frame.layout.lanes.iter().any(|(field, _)| *field == focus))
        {
            self.focus = None;
        }
        // A lane dragged to nothing hides when the drag ends, though the
        // divider that was dragged has gone with the lane's height.
        if self.divider && !ui.input(|i| i.pointer.any_down()) {
            self.divider = false;
            self.settle_heights(frame, events);
        }
        let shown_count = frame.layout.lanes.len();
        let clip = frame.clip;
        let selected = frame.selected;
        for (index, (field, lane)) in frame.layout.lanes.iter().enumerate() {
            let field = *field;
            let lane = *lane;
            let id = Id::new(("note-lane", clip, lane_index(field)));
            // The divider above the lane: dragging resizes it and the lane
            // or the note area above it; a lane dragged to nothing hides
            // (I-DRAG-038).
            let divider = Rect::from_min_max(
                pos2(lane.left(), lane.top() - 3.0),
                pos2(lane.right(), lane.top() - 0.5),
            );
            let split = ui.interact(divider, id.with("divider"), Sense::drag());
            if split.hovered() || split.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            if split.drag_started() {
                self.divider = true;
            }
            if split.dragged() {
                let dy = split.drag_delta().y;
                let below = lane_index(field);
                self.heights[below] = (self.heights[below] - dy).clamp(0.0, MAX_LANE);
                if index > 0 {
                    let above = lane_index(frame.layout.lanes[index - 1].0);
                    self.heights[above] = (self.heights[above] + dy).clamp(0.0, MAX_LANE);
                }
            }
            if lane.height() < 1.0 {
                continue;
            }
            let plot = frame.layout.plot(lane);
            // Spec: note-editing R20. A lane is a group named for its value.
            let group = ui.interact(lane, id.with("group"), Sense::hover());
            ui.ctx().accesskit_node_builder(group.id, |node| {
                node.set_role(Role::Group);
                node.set_label(lane_title(field).to_owned());
                node.set_bounds(egui::accesskit::Rect {
                    x0: lane.min.x.into(),
                    y0: lane.min.y.into(),
                    x1: lane.max.x.into(),
                    y1: lane.max.y.into(),
                });
            });
            let response = ui.interact(plot, id, Sense::click_and_drag());
            // Spec: find-and-select-notes R3. A click in a lane switches
            // every filter off.
            if response.clicked() {
                crate::note_filters::clear(shell, clip, events);
            }
            let pointer = response
                .interact_pointer_pos()
                .or_else(|| ui.input(|i| i.pointer.interact_pos()));
            if response.drag_started()
                && let Some(press) = ui.input(|i| i.pointer.press_origin())
            {
                self.press(frame, field, lane, press, modifiers, draw_mode);
            }
            if response.dragged()
                && let (Some(drag), Some(pos)) = (self.drag.as_mut(), pointer)
            {
                if let LaneDrag::Draw { path, .. } = drag
                    && path.last() != Some(&pos)
                {
                    path.push(pos);
                }
                if let Some((id, args)) = self.invocation(frame, lane, modifiers, pos) {
                    gesture(events, GesturePhase::Update, id, args);
                }
            }
            if response.drag_stopped() {
                let last = pointer
                    .and_then(|pos| self.invocation(frame, lane, modifiers, pos))
                    .map(|(id, args)| Invocation::new(id, args));
                if self.drag.take().is_some() {
                    events.push(UiEvent::Gesture {
                        phase: GesturePhase::Commit,
                        invocation: last,
                    });
                }
            }
            // A press makes this the active lane (R3); a click on a marker
            // selects its note and gives the lane the keyboard (R5, R16).
            if response.is_pointer_button_down_on()
                && ui.input(|i| i.pointer.primary_pressed())
                && active != field
            {
                invoke(events, VIEW_LANE, json!({ "lane": field }));
            }
            if let Some(pos) = response.interact_pointer_pos()
                && (response.clicked() || response.double_clicked())
                && let Some(place) = frame.marker_at(field, lane, pos)
            {
                if response.double_clicked() {
                    // Spec: note-editing R7. A double-click on a marker with
                    // a deviation resets it.
                    let has = frame
                        .notes
                        .get(place as usize)
                        .is_some_and(|note| note.velocity_deviation != 0);
                    if field == NoteField::Velocity && has {
                        let places = targets(selected, place);
                        invoke(
                            events,
                            ids::SET_VALUE,
                            json!({
                                "clip": clip,
                                "notes": places,
                                "field": "velocity_deviation",
                                "value": 0,
                            }),
                        );
                    }
                } else {
                    invoke(
                        events,
                        ids::SELECT_NOTES,
                        json!({
                            "clip": clip,
                            "notes": [place],
                            "mode": if modifiers.shift || modifiers.command {
                                "toggle"
                            } else {
                                "replace"
                            },
                        }),
                    );
                    self.focus = Some(field);
                }
            }
            if let Some(hover) = response.hover_pos()
                && frame.marker_at(field, lane, hover).is_some()
            {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            // Spec: note-editing R23, R24. A group's marker has a menu
            // (C-PGROUP): its kind, ticked, and Ungroup Notes, which ends
            // the whole group.
            if field == NoteField::Chance {
                if response.secondary_clicked() {
                    self.group_menu = response
                        .interact_pointer_pos()
                        .and_then(|pos| frame.marker_at(field, lane, pos))
                        .and_then(|place| frame.notes.get(place as usize)?.group)
                        .map(|group| (group, members(frame.notes, group)));
                }
                if let Some((group, places)) = self.group_menu.clone() {
                    response.context_menu(|ui| {
                        let kind = frame.kind_of(group);
                        for (text, choice, name) in [
                            ("Play All", GroupKind::PlayAll, "play_all"),
                            ("Play One", GroupKind::PlayOne, "play_one"),
                        ] {
                            let item =
                                ui.add(egui::Button::new(text).selected(kind == Some(choice)));
                            crate::menus::register_item(ui.ctx(), &item);
                            if item.clicked() {
                                invoke(
                                    events,
                                    ids::GROUP,
                                    json!({ "clip": clip, "notes": places, "kind": name }),
                                );
                                ui.close();
                            }
                        }
                        ui.separator();
                        let item = ui.button("Ungroup Notes");
                        crate::menus::register_item(ui.ctx(), &item);
                        if item.clicked() {
                            invoke(
                                events,
                                ids::UNGROUP,
                                json!({ "clip": clip, "notes": places }),
                            );
                            ui.close();
                        }
                    });
                }
            }
            // Spec: note-editing R2. The lane header's context menu has
            // Swap Lane (C-LANE-001), which needs a hidden lane to show.
            let header =
                Rect::from_min_max(lane.min, pos2(frame.layout.grid.left(), lane.bottom()));
            let header_response = ui.interact(header, id.with("header"), Sense::click());
            header_response.context_menu(|ui| {
                let response = ui.add_enabled(shown_count < 3, egui::Button::new("Swap Lane"));
                crate::menus::register_item(ui.ctx(), &response);
                if response.clicked() {
                    invoke(events, VIEW_LANES, json!({ "lane": field, "swap": true }));
                    ui.close();
                }
            });
            // Spec: note-editing R20. Each marker is an item named for its
            // note and reading its value.
            let leads = (field == NoteField::Chance).then(|| leaders(frame.notes));
            for (place, note) in frame.notes.iter().enumerate() {
                if leads.as_ref().is_some_and(|leads| !leads[place]) {
                    continue;
                }
                let x = stem_x(frame.layout.grid, frame.start, frame.length, note);
                if x + STEM_WIDTH < frame.layout.grid.left() || x > frame.layout.grid.right() {
                    continue;
                }
                let top = y_of_value(lane, field, note_value(note, field));
                let rect = Rect::from_min_max(pos2(x, top), pos2(x + STEM_WIDTH, lane.bottom()));
                let item = ui.interact(rect, id.with(("marker", place)), Sense::hover());
                let value = value_text(field, note_value(note, field));
                let name = crate::clip_view::key_label(frame.tuning, note.key);
                ui.ctx().accesskit_node_builder(item.id, |node| {
                    node.set_role(Role::ListItem);
                    node.set_label(name);
                    node.set_value(value);
                });
            }
        }
        // Spec: note-editing R2. Option with the wheel (Alt on Windows), or
        // a pinch, over the lanes makes them taller or shorter.
        let lanes_area = frame
            .layout
            .lanes
            .iter()
            .map(|(_, rect)| *rect)
            .reduce(|a, b| a.union(b));
        if let Some(area) = lanes_area
            && ui.rect_contains_pointer(area)
        {
            let (scroll, pinch) = ui.input(|i| {
                (
                    if i.modifiers.alt {
                        i.smooth_scroll_delta.y
                    } else {
                        0.0
                    },
                    i.zoom_delta(),
                )
            });
            if scroll != 0.0 || (pinch - 1.0).abs() > f32::EPSILON {
                for (field, _) in &frame.layout.lanes {
                    let at = lane_index(*field);
                    self.heights[at] = (self.heights[at] * pinch + scroll).clamp(0.0, MAX_LANE);
                }
                self.settle_heights(frame, events);
            }
        }
    }

    /// Hides a lane made shorter than [`MIN_LANE`] and puts its height back
    /// for when it's shown again.
    fn settle_heights(&mut self, frame: &Frame<'_>, events: &mut Vec<UiEvent>) {
        for (field, _) in &frame.layout.lanes {
            let at = lane_index(*field);
            if self.heights[at] < MIN_LANE {
                self.heights[at] = LANE_HEIGHT;
                invoke(events, VIEW_LANES, json!({ "lane": field, "show": false }));
            }
        }
    }

    /// A press in `field`'s lane that starts a drag: draws, sets a
    /// deviation, or moves markers.
    fn press(
        &mut self,
        frame: &Frame<'_>,
        field: NoteField,
        lane: Rect,
        press: Pos2,
        modifiers: Modifiers,
        draw_mode: bool,
    ) {
        let selected = frame.selected;
        if draw_mode {
            self.drag = Some(LaneDrag::Draw {
                field,
                only: selected.to_vec(),
                path: vec![press],
            });
            return;
        }
        let Some(place) = frame.marker_at(field, lane, press) else {
            return;
        };
        let places = targets(selected, place);
        if field == NoteField::Velocity && modifiers.command {
            let velocity = frame
                .notes
                .get(place as usize)
                .map_or(0.0, |note| f64::from(note.velocity));
            self.drag = Some(LaneDrag::Deviation { places, velocity });
        } else {
            self.drag = Some(LaneDrag::Marker {
                field,
                places,
                from: press.y,
            });
        }
    }

    /// The command for the drag with the pointer at `pos`, measured from
    /// where it began, so the whole drag is one gesture (R5 to R7).
    fn invocation(
        &self,
        frame: &Frame<'_>,
        lane: Rect,
        modifiers: Modifiers,
        pos: Pos2,
    ) -> Option<(&'static str, Value)> {
        let clip = frame.clip;
        match self.drag.as_ref()? {
            LaneDrag::Marker {
                field,
                places,
                from,
            } => {
                // The drag is measured from the press and isn't held to the
                // lane, so dragging past an end brings every note to it.
                let (low, high) = field.range();
                let moved =
                    f64::from(from - pos.y) / f64::from(lane.height().max(1.0)) * (high - low);
                let by = if field.whole() {
                    moved.round()
                } else {
                    (moved * 100.0).round() / 100.0
                };
                Some((
                    ids::ADJUST_VALUE,
                    json!({ "clip": clip, "notes": places, "field": field, "by": by }),
                ))
            }
            LaneDrag::Deviation { places, velocity } => {
                let reach = lane_value(lane, NoteField::Velocity, pos.y);
                let deviation = (reach - velocity).clamp(-127.0, 127.0);
                Some((
                    ids::SET_VALUE,
                    json!({
                        "clip": clip,
                        "notes": places,
                        "field": "velocity_deviation",
                        "value": deviation,
                    }),
                ))
            }
            LaneDrag::Draw { field, only, path } => {
                let (places, values) =
                    self.drawn(frame, *field, lane, only, path, modifiers, pos)?;
                Some((
                    ids::SET_VALUE,
                    json!({
                        "clip": clip,
                        "notes": places,
                        "field": field,
                        "values": values,
                    }),
                ))
            }
        }
    }

    /// The notes a Draw Mode drag has drawn on and the value each took
    /// (R6): with the grid, every note whose start lies in a step the
    /// pointer passed takes the pointer's height in that step; without it,
    /// each takes the height at its own start. Option draws a straight line
    /// from where the drag began, and with Shift a level one.
    #[expect(clippy::too_many_arguments, reason = "the drawing's whole context")]
    fn drawn(
        &self,
        frame: &Frame<'_>,
        field: NoteField,
        lane: Rect,
        only: &[u32],
        path: &[Pos2],
        modifiers: Modifiers,
        pos: Pos2,
    ) -> Option<(Vec<u32>, Vec<f64>)> {
        let grid = frame.layout.grid;
        let from = *path.first()?;
        let line = [
            from,
            if modifiers.shift {
                pos2(pos.x, from.y)
            } else {
                pos
            },
        ];
        let path: &[Pos2] = if modifiers.alt { &line } else { path };
        let (low, high) = path
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), p| {
                (low.min(p.x), high.max(p.x))
            });
        let snap = frame.snap != crate::arrangement::editing::EditKeys::from(modifiers).bypass;
        let step = frame.step.get().max(1);
        let tick = |x: f32| (crate::clip_view::tick_at(grid, frame.start, frame.length, x)).get();
        let mut places = Vec::new();
        let mut values = Vec::new();
        for (place, note) in frame.notes.iter().enumerate() {
            let place = u32::try_from(place).unwrap_or(u32::MAX);
            if !only.is_empty() && !only.contains(&place) {
                continue;
            }
            let x = stem_x(grid, frame.start, frame.length, note) + STEM_WIDTH / 2.0;
            let sample = if snap {
                // The notes in a grid step the pointer passed share the
                // pointer's height in the step.
                let at = note.start.get().div_euclid(step);
                let (first, last) = (tick(low).div_euclid(step), tick(high).div_euclid(step));
                if at < first || at > last {
                    continue;
                }
                let centre = crate::clip_view::x_of_tick(
                    grid,
                    frame.start,
                    frame.length,
                    Ticks(at * step + step / 2),
                );
                centre.clamp(low, high)
            } else {
                if x < low - STEM_WIDTH / 2.0 || x > high + STEM_WIDTH / 2.0 {
                    continue;
                }
                x
            };
            places.push(place);
            values.push(lane_value(lane, field, path_y(path, sample)));
        }
        (!places.is_empty()).then_some((places, values))
    }

    /// Draws the footer: the Show/Hide All switch and the Lane Selector at
    /// its left, room for the MIDI tool menu, then the active lane's
    /// controls (R10).
    #[expect(clippy::too_many_lines, reason = "one pass over the footer's controls")]
    pub fn footer(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &Frame<'_>,
        events: &mut Vec<UiEvent>,
    ) {
        let footer = frame.layout.footer;
        if footer.height() < 1.0 {
            return;
        }
        let colours = &theme.colors;
        let painter = ui.painter().clone();
        let clip = frame.clip;
        painter.rect_filled(footer, 0.0, colours.bar.0);
        let state = shell
            .clip_view()
            .state(shell.editor().document(), shell.arrangement());
        let lane = state.lane;
        let rects = footer_rects(footer, lane);
        let rect_of = |name: &str| {
            rects
                .iter()
                .find(|(named, _)| *named == name)
                .map_or(Rect::NOTHING, |(_, rect)| *rect)
        };
        let mut pressed_on_a_field = false;

        // Spec: note-editing R1. The Show/Hide All Expression Editors
        // switch, and the Lane Selector beside it.
        let switch = rect_of("Show/Hide All Expression Editors");
        let response = ui.interact(switch, Id::new(("lane-switch", clip)), Sense::click());
        let visible = !state.lanes_hidden;
        painter.rect_filled(
            switch,
            theme.sizes.corner,
            if visible {
                colours.accent.0
            } else {
                colours.control.0
            },
        );
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::Checkbox,
                true,
                visible,
                "Show/Hide All Expression Editors",
            )
        });
        if response.clicked() {
            invoke(events, VIEW_LANES, json!({ "all": !visible }));
        }
        let selector = rect_of("Lane Selector");
        let response = ui.interact(selector, Id::new(("lane-selector", clip)), Sense::click());
        painter.rect_filled(
            selector,
            theme.sizes.corner,
            if response.hovered() {
                colours.control_hover.0
            } else {
                colours.control.0
            },
        );
        let a = selector.center();
        painter.add(Shape::convex_polygon(
            vec![
                a + vec2(-3.0, -1.5),
                a + vec2(3.0, -1.5),
                a + vec2(0.0, 2.0),
            ],
            colours.text.0,
            Stroke::NONE,
        ));
        response.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, true, "Lane Selector"));
        egui::Popup::menu(&response).show(|ui| {
            for choice in NoteField::LANES {
                let mut on = state.lanes.contains(&choice);
                let item = ui.checkbox(&mut on, lane_title(choice));
                crate::menus::register_item(ui.ctx(), &item);
                if item.changed() {
                    invoke(events, VIEW_LANES, json!({ "lane": choice, "show": on }));
                }
            }
        });

        // The active lane's controls follow the room kept for the MIDI
        // tool menu (L-CLIP-015).
        let label = |ui: &Ui, rect: Rect, text: &str| {
            ui.painter().text(
                rect.left_center(),
                Align2::LEFT_CENTER,
                text,
                theme.small_font(),
                colours.text_dim.0,
            );
        };
        let selected = summary(frame, lane);
        let amounts = state.randomize;

        // Randomize and its amount (R13).
        let randomize = rect_of("Randomize");
        let response = ui.interact(
            randomize,
            Id::new(("footer-randomize", clip)),
            Sense::click(),
        );
        paint_button(
            &painter,
            theme,
            randomize,
            "Randomize",
            true,
            response.hovered(),
        );
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Randomize"));
        let amount_now = f64::from(amounts.of(lane));
        let amount_shown = self
            .footer_drag
            .as_ref()
            .filter(|drag| drag.control == Control::Amount)
            .map_or(amount_now, |drag| drag.last);
        let amount_text = if lane.whole() {
            format!("{amount_shown}")
        } else {
            format!("{amount_shown}%")
        };
        self.field(
            ui,
            theme,
            frame,
            Control::Amount,
            rect_of("Randomize Amount"),
            lane,
            (amount_shown, amount_text),
            true,
            amount_now,
            0.0,
            events,
            &mut pressed_on_a_field,
        );
        if response.clicked() {
            let units = if lane.whole() {
                amount_now
            } else {
                amount_now / 100.0
            };
            invoke(
                events,
                ids::RANDOMIZE,
                json!({ "clip": clip, "field": lane, "amount": units }),
            );
        }

        match lane {
            NoteField::Chance => {
                // Spec: note-editing R10, R22, R23. Play All and Play One group
                // the selected notes, and Ungroup takes them out of their
                // groups.
                let grouped = frame.selected.iter().any(|place| {
                    frame
                        .notes
                        .get(*place as usize)
                        .is_some_and(|n| n.group.is_some())
                });
                for (name, enabled, command) in [
                    (
                        "Play All",
                        frame.selected.len() >= 2,
                        (ids::GROUP, json!({ "clip": clip, "kind": "play_all" })),
                    ),
                    (
                        "Play One",
                        frame.selected.len() >= 2,
                        (ids::GROUP, json!({ "clip": clip, "kind": "play_one" })),
                    ),
                    ("Ungroup", grouped, (ids::UNGROUP, json!({ "clip": clip }))),
                ] {
                    let rect = rect_of(name);
                    let response = ui.interact(
                        rect,
                        Id::new(("footer", name, clip)),
                        if enabled {
                            Sense::click()
                        } else {
                            Sense::hover()
                        },
                    );
                    paint_button(&painter, theme, rect, name, enabled, response.hovered());
                    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, name));
                    if enabled && response.clicked() {
                        invoke(events, command.0, command.1);
                    }
                }
            }
            _ => {
                label(ui, rect_of("Ramp"), "Ramp");
                let start_text = selected
                    .as_ref()
                    .map_or_else(String::new, |s| value_text(lane, s.first));
                let end_text = selected
                    .as_ref()
                    .map_or_else(String::new, |s| value_text(lane, s.last));
                let (first, last) = selected.as_ref().map_or((0.0, 0.0), |s| (s.first, s.last));
                let both = selected
                    .as_ref()
                    .is_some_and(|s| s.count >= 2 && !s.together);
                self.field(
                    ui,
                    theme,
                    frame,
                    Control::RampStart,
                    rect_of("Ramp Start"),
                    lane,
                    (first, start_text),
                    selected.is_some(),
                    first,
                    last,
                    events,
                    &mut pressed_on_a_field,
                );
                self.field(
                    ui,
                    theme,
                    frame,
                    Control::RampEnd,
                    rect_of("Ramp End"),
                    lane,
                    (last, end_text),
                    both,
                    last,
                    first,
                    events,
                    &mut pressed_on_a_field,
                );
                if lane == NoteField::Velocity {
                    label(ui, rect_of("Deviation"), "Deviation");
                    let deviation = summary(frame, NoteField::VelocityDeviation);
                    let text = deviation.as_ref().map_or_else(String::new, |s| {
                        range_text(NoteField::VelocityDeviation, s.low, s.high)
                    });
                    let low = deviation.as_ref().map_or(0.0, |s| s.low);
                    self.field(
                        ui,
                        theme,
                        frame,
                        Control::Deviation,
                        rect_of("Velocity Deviation"),
                        lane,
                        (low, text),
                        deviation.is_some(),
                        low,
                        0.0,
                        events,
                        &mut pressed_on_a_field,
                    );
                }
            }
        }
        // A press anywhere but on a field drops a number being typed.
        if ui.input(|i| i.pointer.primary_pressed()) && !pressed_on_a_field {
            self.typed = None;
        }
    }

    /// One footer value field: it shows `shown.1`, takes a drag up and down
    /// (a step for each few points) and a typed value after a click, and
    /// each runs the command the field stands for (R10 to R13).
    #[expect(clippy::too_many_arguments, reason = "a field has this many parts")]
    fn field(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &Frame<'_>,
        control: Control,
        rect: Rect,
        lane: NoteField,
        shown: (f64, String),
        enabled: bool,
        base: f64,
        other: f64,
        events: &mut Vec<UiEvent>,
        pressed: &mut bool,
    ) {
        let field = control.field(lane);
        let id = Id::new(("footer-field", control.name(), frame.clip));
        let response = ui.interact(
            rect,
            id,
            if enabled {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            },
        );
        let colours = &theme.colors;
        let typing = self.typed.as_ref().filter(|typed| typed.control == control);
        let text = match typing {
            Some(typed) => format!("{}|", typed.text),
            None => shown.1.clone(),
        };
        let face = if typing.is_some() {
            colours.control_pressed.0
        } else if response.hovered() && enabled {
            colours.control_hover.0
        } else {
            colours.control.0
        };
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            theme.sizes.corner,
            if enabled {
                face
            } else {
                face.gamma_multiply(0.55)
            },
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            &text,
            theme.small_font(),
            if enabled {
                colours.text.0
            } else {
                colours.pending.0
            },
        );
        response.widget_info(|| WidgetInfo::slider(enabled, shown.0, control.name()));
        let name = control.name();
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::SpinButton);
            node.set_label(name.to_owned());
            node.set_value(text.clone());
            node.set_numeric_value(shown.0);
        });
        if !enabled {
            return;
        }
        if response.is_pointer_button_down_on() {
            *pressed = true;
        }
        let selected = !frame.selected.is_empty();
        if response.drag_started() {
            self.typed = None;
            let seed = ui.input(|i| i.time).to_bits() ^ 0x5851_F42D_4C95_7F2D;
            self.footer_drag = Some(FooterDrag {
                control,
                base,
                other,
                seed,
                notes: selected,
                last: base,
            });
        }
        let steps = |response: &egui::Response| -> f64 {
            let origin = ui.input(|i| i.pointer.press_origin());
            match (origin, response.interact_pointer_pos()) {
                (Some(from), Some(to)) => f64::from(((from.y - to.y) / POINTS_PER_STEP).trunc()),
                _ => 0.0,
            }
        };
        if response.dragged()
            && let Some(drag) = self.footer_drag.as_mut()
            && drag.control == control
        {
            let (low, high) = control_range(control, lane);
            let unit = unit(field);
            let value = (drag.base + steps(&response) * unit).clamp(low, high);
            // Until the value first moves, nothing runs: a press on the
            // amount that doesn't change it randomises nothing (R13).
            let moved = value != drag.base || drag.last != drag.base;
            drag.last = value;
            let drag = drag.clone();
            if moved {
                if let Some((id, args)) = footer_command(frame, lane, &drag, value) {
                    gesture(events, GesturePhase::Update, id, args);
                } else if control == Control::Amount {
                    invoke(
                        events,
                        VIEW_LANES,
                        json!({ "lane": lane, "amount": amount_of(lane, value) }),
                    );
                }
            }
        }
        if response.drag_stopped()
            && let Some(drag) = self.footer_drag.take_if(|drag| drag.control == control)
        {
            // A drag back to where it began leaves its last frame, which
            // ran there already.
            let last = (drag.last != drag.base)
                .then(|| footer_command(frame, lane, &drag, drag.last))
                .flatten()
                .map(|(id, args)| Invocation::new(id, args));
            if last.is_some() || drag.notes {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: last,
                });
            }
            if control == Control::Amount {
                invoke(
                    events,
                    VIEW_LANES,
                    json!({ "lane": lane, "amount": amount_of(lane, drag.last) }),
                );
            }
        }
        if response.clicked() {
            self.typed = Some(Typed {
                control,
                text: String::new(),
            });
        }
    }

    /// The keys while a number is being typed (R14): digits add to it, Backspace
    /// takes one back, and Enter sets it. Esc drops it, in [`LaneUi::cancel`].
    /// Returns whether the key was taken.
    pub fn type_key(
        &mut self,
        shell: &Shell,
        frame: &Frame<'_>,
        key: Key,
        modifiers: Modifiers,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        let Some(typed) = self.typed.as_mut() else {
            return false;
        };
        let digit = match key {
            Key::Num0 => Some('0'),
            Key::Num1 => Some('1'),
            Key::Num2 => Some('2'),
            Key::Num3 => Some('3'),
            Key::Num4 => Some('4'),
            Key::Num5 => Some('5'),
            Key::Num6 => Some('6'),
            Key::Num7 => Some('7'),
            Key::Num8 => Some('8'),
            Key::Num9 => Some('9'),
            _ => None,
        };
        let plain = !(modifiers.command || modifiers.ctrl || modifiers.alt);
        match key {
            _ if digit.is_some() && plain => {
                if typed.text.len() < MAX_TYPED {
                    typed.text.push(digit.unwrap_or('0'));
                }
            }
            Key::Minus if typed.control == Control::Deviation && typed.text.is_empty() => {
                typed.text.push('-');
            }
            Key::Backspace => {
                typed.text.pop();
            }
            Key::Enter => {
                let typed = self.typed.take();
                if let Some(typed) = typed {
                    self.commit(shell, frame, &typed, events);
                }
            }
            _ => return false,
        }
        true
    }

    /// Starts a velocity on the keys: a digit from 1 to 9 with notes
    /// selected, while no number is being typed (R14).
    pub fn start_velocity(&mut self, key: Key, modifiers: Modifiers) -> bool {
        let first = match key {
            Key::Num1 => '1',
            Key::Num2 => '2',
            Key::Num3 => '3',
            Key::Num4 => '4',
            Key::Num5 => '5',
            Key::Num6 => '6',
            Key::Num7 => '7',
            Key::Num8 => '8',
            Key::Num9 => '9',
            _ => return false,
        };
        if modifiers.command || modifiers.ctrl || modifiers.alt || modifiers.shift {
            return false;
        }
        self.typed = Some(Typed {
            control: Control::Velocity,
            text: first.to_string(),
        });
        true
    }

    /// Sets what was typed, by the control it was typed into. A number out
    /// of the control's range changes nothing.
    fn commit(&self, shell: &Shell, frame: &Frame<'_>, typed: &Typed, events: &mut Vec<UiEvent>) {
        let Ok(number) = typed.text.parse::<i32>() else {
            return;
        };
        let lane = shell.clip_view().lane();
        let clip = frame.clip;
        let value = f64::from(number);
        let selected = summary(frame, lane);
        match typed.control {
            Control::Velocity => {
                if (1..=127).contains(&number) {
                    invoke(
                        events,
                        ids::SET_VALUE,
                        json!({ "clip": clip, "field": "velocity", "value": value }),
                    );
                }
            }
            Control::Amount => {
                let limit = i32::from(RandomizeAmounts::limit(lane));
                if (0..=limit).contains(&number) {
                    invoke(
                        events,
                        VIEW_LANES,
                        json!({ "lane": lane, "amount": number }),
                    );
                    let units = if lane.whole() { value } else { value / 100.0 };
                    invoke(
                        events,
                        ids::RANDOMIZE,
                        json!({ "clip": clip, "field": lane, "amount": units }),
                    );
                }
            }
            Control::RampStart | Control::RampEnd => {
                let (low, high) = control_range(typed.control, lane);
                let (Some(selected), true) =
                    (selected, (low..=high).contains(&(value * unit(lane))))
                else {
                    return;
                };
                let value = if lane.whole() { value } else { value / 100.0 };
                let (from, to) = if typed.control == Control::RampStart {
                    (value, selected.last)
                } else {
                    (selected.first, value)
                };
                invoke(
                    events,
                    ids::RAMP_VALUE,
                    json!({ "clip": clip, "field": lane, "from": from, "to": to }),
                );
            }
            Control::Deviation => {
                if (-127..=127).contains(&number) {
                    invoke(
                        events,
                        ids::SET_VALUE,
                        json!({
                            "clip": clip,
                            "field": "velocity_deviation",
                            "value": value,
                        }),
                    );
                }
            }
        }
    }
}

/// The notes a marker drag or a Cmd-drag moves: every selected note when
/// the grabbed one is selected, else just it (R5, R7).
fn targets(selected: &[u32], place: u32) -> Vec<u32> {
    if selected.contains(&place) {
        selected.to_vec()
    } else {
        vec![place]
    }
}

/// A footer control's range while it is dragged, in the field's own units
/// (and for the amount, in the footer's: whole, a percentage for chance).
fn control_range(control: Control, lane: NoteField) -> (f64, f64) {
    match control {
        Control::Amount => (0.0, f64::from(RandomizeAmounts::limit(lane))),
        Control::Deviation => NoteField::VelocityDeviation.range(),
        _ => lane.range(),
    }
}

/// An amount as the footer holds it: whole, a percentage for chance.
fn amount_of(lane: NoteField, value: f64) -> u8 {
    value
        .round()
        .clamp(0.0, f64::from(RandomizeAmounts::limit(lane))) as u8
}

/// The command for a footer drag that has reached `value`: Randomize moves
/// the selected notes at once, Ramp spreads from the new end, Deviation sets.
/// The amount of a drag with no notes selected has none.
fn footer_command(
    frame: &Frame<'_>,
    lane: NoteField,
    drag: &FooterDrag,
    value: f64,
) -> Option<(&'static str, Value)> {
    let clip = frame.clip;
    match drag.control {
        Control::Amount => drag.notes.then(|| {
            let units = if lane.whole() { value } else { value / 100.0 };
            (
                ids::RANDOMIZE,
                json!({
                    "clip": clip,
                    "field": lane,
                    "amount": units,
                    "seed": drag.seed,
                }),
            )
        }),
        Control::RampStart => Some((
            ids::RAMP_VALUE,
            json!({ "clip": clip, "field": lane, "from": value, "to": drag.other }),
        )),
        Control::RampEnd => Some((
            ids::RAMP_VALUE,
            json!({ "clip": clip, "field": lane, "from": drag.other, "to": value }),
        )),
        Control::Deviation => Some((
            ids::SET_VALUE,
            json!({ "clip": clip, "field": "velocity_deviation", "value": value }),
        )),
        Control::Velocity => None,
    }
}

/// Draws a footer button.
fn paint_button(
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    text: &str,
    enabled: bool,
    hovered: bool,
) {
    let colours = &theme.colors;
    let face = if hovered && enabled {
        colours.control_hover.0
    } else {
        colours.control.0
    };
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if enabled {
            face
        } else {
            face.gamma_multiply(0.55)
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        if enabled {
            colours.text.0
        } else {
            colours.pending.0
        },
    );
}
