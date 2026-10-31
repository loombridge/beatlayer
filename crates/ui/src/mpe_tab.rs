//! The Clip View's MPE tab (WP-6.18; the MPE spec, R22 to R34): the
//! Expression Editor. It is the MIDI Note Editor's note area and rulers with
//! the expression lanes in the velocity lane's place: Slide, Pressure,
//! Velocity and Release Velocity, a Pitch Bend lane first on a Drum Rack's
//! track, a selector toggle for each, and a triangle under the toggles. Each
//! selected note's Pitch envelope is drawn over it in the note area.
//!
//! Every edit is one of the `note.*_expression*` commands agents run, and a
//! drag sends its command on each frame as a [`UiEvent::Gesture`], measured
//! from where it began, so the whole drag is one undo step and Esc puts the
//! clip back. Which lanes show, their heights and the tab's grid are the
//! shell's (`view.mpe_lanes`, `view.mpe_grid`); only the drag under way and a
//! value being typed live here.

use bl_core::time::Ticks;
use bl_model::{Breakpoint, ClipId, Expression, MpeLane, Note, envelope_value};
use bl_shell::note_commands::{self as ids, BreakpointRef, NoteField, TickRange};
use bl_shell::{GesturePhase, Invocation};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, CursorIcon, Event, Id, Key, Modifiers, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::arrangement::automation::{
    BREAKPOINT_GRAB, BREAKPOINT_SIZE, CURVE_PER_POINT, FINE_VALUE, SEGMENT_GRAB,
};
use crate::arrangement::editing::EditKeys;
use crate::clip_view::{KEY_HEIGHT, KEY_WIDTH, STRIP_HEIGHT, TOOLBAR_HEIGHT, tick_at, x_of_tick};
use crate::main_window::UiEvent;
use crate::note_lanes::NoteLayout;
use crate::theme::Theme;

/// How big a lane's selector toggle is, and how far apart the toggles sit.
pub const TOGGLE: f32 = 12.0;
const TOGGLE_STEP: f32 = 14.0;
/// How tall the band under the note area is at least while the toggles show:
/// four toggles and the triangle.
const BAND_MIN: f32 = TOGGLE_STEP * 5.0 + 1.0;
/// How tall the band is while the triangle has hidden the lanes: the
/// triangle alone.
const BAND_HIDDEN: f32 = TOGGLE_STEP + 1.0;
/// The least the note area keeps, however tall the lanes are made.
const MIN_GRID: f32 = 4.0 * KEY_HEIGHT;
/// A lane dragged shorter than this is hidden (I-DRAG-055).
const MIN_LANE: f32 = 8.0;
/// The tallest a lane is made.
const MAX_LANE: f32 = 400.0;
/// How far from a segment, in points, a click selects it rather than adding a
/// breakpoint on it (I-CLICK-047).
const SEGMENT_NEAR: f32 = SEGMENT_GRAB * 2.5;
/// How far from a grid line or a neighbouring breakpoint, in points, a
/// breakpoint snaps to it while the tab's grid is on (R27).
const SNAP_PX: f32 = 6.0;
/// How far above and below a selected note's row, in semitones, the
/// background of its Pitch envelope reaches (I-DRAG-051).
const PITCH_REACH: f32 = 12.0;
/// How far a press must move before a Shift-drag chooses its axis.
const AXIS_DECIDES: f32 = 3.0;
/// The most characters a typed value takes.
const LONGEST: usize = 12;

// ---------------------------------------------------------------- geometry

/// Where the parts of the tab's body sit: the note area, the band under it
/// with the toggles, the triangle and the lanes.
#[derive(Clone, Debug, PartialEq)]
pub struct MpeLayout {
    /// The note area.
    pub grid: Rect,
    /// The band under the note area: the toggles and triangle at its left,
    /// the lanes stacked from its top.
    pub band: Rect,
    /// The lanes drawn, from the top, each as wide as the Clip View.
    pub lanes: Vec<(MpeLane, Rect)>,
    /// The selector toggles, in lane order; none while the lanes are hidden.
    pub toggles: Vec<(MpeLane, Rect)>,
    /// The triangle under the toggles.
    pub triangle: Rect,
    /// Where the lanes end: the time selection and the insert marker run
    /// down to here.
    pub lanes_bottom: f32,
}

impl MpeLayout {
    /// The rectangle of `lane`, when it is drawn.
    #[must_use]
    pub fn lane(&self, lane: MpeLane) -> Option<Rect> {
        self.lanes.iter().find(|(l, _)| *l == lane).map(|(_, r)| *r)
    }

    /// The part of a lane that holds the envelopes: right of its header.
    #[must_use]
    pub fn plot(&self, lane: Rect) -> Rect {
        Rect::from_min_max(pos2(self.grid.left(), lane.top()), lane.max)
    }

    /// The layout as the note editor's own paint and input take it: a body
    /// with no lanes of theirs, whose footer line is where the lanes end.
    #[must_use]
    pub fn note_layout(&self) -> NoteLayout {
        NoteLayout {
            grid: self.grid,
            lanes: Vec::new(),
            footer: Rect::from_min_max(pos2(self.band.left(), self.lanes_bottom), self.band.max),
        }
    }
}

/// The tab's body in `view`: the lanes with the heights given stacked in a
/// band along the bottom, at least tall enough for the toggles, and the note
/// area above them, which keeps room for a few rows however tall the lanes
/// are made.
#[must_use]
pub fn layout(view: Rect, lanes: &[(MpeLane, f32)], hidden: bool) -> MpeLayout {
    let top = view.top() + TOOLBAR_HEIGHT + STRIP_HEIGHT;
    let room = (view.bottom() - top - MIN_GRID).max(BAND_HIDDEN);
    let wanted: f32 = lanes.iter().map(|(_, h)| h).sum();
    let band_min = if hidden { BAND_HIDDEN } else { BAND_MIN };
    let band_height = wanted.max(band_min).min(room.max(band_min));
    let scale = if wanted > band_height && wanted > 0.0 {
        band_height / wanted
    } else {
        1.0
    };
    let band_top = (view.bottom() - band_height).max(top);
    let band = Rect::from_min_max(pos2(view.left(), band_top), view.max);
    let mut y = band_top;
    let lanes: Vec<(MpeLane, Rect)> = lanes
        .iter()
        .map(|(lane, h)| {
            let rect = Rect::from_min_max(pos2(view.left(), y), pos2(view.max.x, y + h * scale));
            y += h * scale;
            (*lane, rect)
        })
        .collect();
    let toggle = |index: usize| {
        Rect::from_min_size(
            pos2(
                view.left() + (KEY_WIDTH - TOGGLE) / 2.0,
                band_top + 1.0 + TOGGLE_STEP * index as f32,
            ),
            vec2(TOGGLE, TOGGLE),
        )
    };
    let toggles = if hidden {
        Vec::new()
    } else {
        MpeLane::TOGGLED
            .iter()
            .enumerate()
            .map(|(index, lane)| (*lane, toggle(index)))
            .collect()
    };
    MpeLayout {
        grid: Rect::from_min_max(
            pos2(view.left() + KEY_WIDTH, top),
            pos2(view.max.x, band_top),
        ),
        band,
        lanes_bottom: y,
        lanes,
        triangle: toggle(if hidden { 0 } else { 4 }),
        toggles,
    }
}

/// Where a value sits on screen for one note's envelope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plot {
    /// Pitch, drawn over the note: the note's row centre is 0, and a
    /// semitone is a row (R24).
    Pitch {
        /// The height of the note's row centre.
        centre: f32,
    },
    /// A lane: its lowest value at the bottom edge, its highest at the top.
    Lane {
        /// The part of the lane that holds envelopes.
        rect: Rect,
        /// The lowest value.
        low: f64,
        /// The highest value.
        high: f64,
    },
}

impl Plot {
    /// The plot of `lane` in `rect`.
    #[must_use]
    pub fn of_lane(lane: MpeLane, rect: Rect) -> Self {
        let (low, high) = match lane {
            MpeLane::PitchBend => Expression::Pitch.range(),
            MpeLane::Slide | MpeLane::Pressure => (0.0, 127.0),
            MpeLane::Velocity | MpeLane::ReleaseVelocity => (1.0, 127.0),
        };
        Self::Lane { rect, low, high }
    }

    /// The height of `value`.
    #[must_use]
    pub fn y(self, value: f64) -> f32 {
        match self {
            Self::Pitch { centre } => centre - value as f32 * KEY_HEIGHT,
            Self::Lane { rect, low, high } => {
                let at = ((value - low) / (high - low)).clamp(0.0, 1.0) as f32;
                rect.bottom() - at * rect.height()
            }
        }
    }

    /// The value at height `y`, held to the envelope's range.
    #[must_use]
    pub fn value(self, y: f32, expression: Expression) -> f64 {
        let (low, high) = expression.range();
        match self {
            Self::Pitch { centre } => f64::from((centre - y) / KEY_HEIGHT).clamp(low, high),
            Self::Lane {
                rect,
                low: bottom,
                high: top,
            } => {
                let at = f64::from((rect.bottom() - y) / rect.height().max(1.0));
                (bottom + (top - bottom) * at).clamp(bottom.max(low), top.min(high))
            }
        }
    }
}

/// One selected note's envelope as drawn: whose, which, and where.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    /// The note's place in the clip's notes in order.
    pub place: u32,
    /// Which envelope.
    pub expression: Expression,
    /// Where its values sit on screen.
    pub plot: Plot,
}

/// What the tab needs of the editor's frame.
pub struct Frame<'a> {
    /// The clip.
    pub clip: ClipId,
    /// The tuning the clip's track plays, which names the notes (the tunings
    /// spec, R29).
    pub tuning: Option<&'a bl_model::Tuning>,
    /// The note area.
    pub grid: Rect,
    /// The contents tick at the note area's left edge, and the clip's
    /// length in ticks.
    pub start: Ticks,
    /// The clip's length in ticks.
    pub length: Ticks,
    /// The clip's notes in order.
    pub notes: &'a [Note],
    /// The selected notes.
    pub selected: &'a [u32],
    /// The clip's track colour.
    pub face: Color32,
    /// The editing grid's step in ticks.
    pub step: Ticks,
    /// Whether the tab's own grid is on (R27).
    pub grid_on: bool,
    /// The row centres of the selected notes whose Pitch envelope is drawn:
    /// none while Fold is on, or on a Drum Rack's track (R24).
    pub pitch: Vec<(u32, f32)>,
    /// The envelope that has the keyboard, and the breakpoints chosen on it.
    pub focus: Option<Expression>,
    /// The chosen breakpoints.
    pub chosen: &'a [BreakpointRef],
    /// The time selection, in ticks of the clip's contents.
    pub time: Option<(Ticks, Ticks)>,
    /// Whether Draw Mode is on.
    pub draw_mode: bool,
}

impl Frame<'_> {
    fn x(&self, tick: Ticks) -> f32 {
        x_of_tick(self.grid, self.start, self.length, tick)
    }

    fn tick(&self, x: f32) -> Ticks {
        tick_at(self.grid, self.start, self.length, x)
    }

    fn is_selected(&self, place: u32) -> bool {
        self.selected.contains(&place)
    }

    fn is_chosen(&self, place: u32, expression: Expression, at: Ticks) -> bool {
        self.focus == Some(expression)
            && self
                .chosen
                .iter()
                .any(|point| point.note == place && point.at == at)
    }

    /// Where a breakpoint of a note is drawn.
    fn point_pos(&self, note: &Note, plot: Plot, point: &Breakpoint) -> Pos2 {
        pos2(self.x(note.start + point.at), plot.y(point.value))
    }

    /// The envelopes drawn in `lane`'s plot, one for each selected note.
    fn lane_targets(&self, lane: MpeLane, plot: Rect) -> Vec<Target> {
        let Some(expression) = lane_expression(lane) else {
            return Vec::new();
        };
        self.selected
            .iter()
            .map(|place| Target {
                place: *place,
                expression,
                plot: Plot::of_lane(lane, plot),
            })
            .collect()
    }

    /// The Pitch envelopes drawn over the selected notes.
    fn pitch_targets(&self) -> Vec<Target> {
        self.pitch
            .iter()
            .map(|(place, centre)| Target {
                place: *place,
                expression: Expression::Pitch,
                plot: Plot::Pitch { centre: *centre },
            })
            .collect()
    }

    fn note(&self, place: u32) -> Option<&Note> {
        self.notes.get(place as usize)
    }

    /// The tick `pos` snaps to with the tab's grid on (R27): the nearest of
    /// the grid's lines and of the other breakpoints of `expression`, within
    /// reach; `bypass` (Cmd) holds it off.
    fn snapped(
        &self,
        tick: Ticks,
        expression: Expression,
        skip: &[BreakpointRef],
        bypass: bool,
    ) -> Ticks {
        if !self.grid_on || bypass {
            return tick;
        }
        let reach = (f64::from(SNAP_PX) / f64::from(self.grid.width().max(1.0))
            * self.length.get() as f64) as i64;
        let step = self.step.get().max(1);
        let line = Ticks((tick.get() as f64 / step as f64).round() as i64 * step);
        let mut best = ((line - tick).get().abs(), line);
        for (place, note) in self.notes.iter().enumerate() {
            for point in note.envelope(expression) {
                let place = u32::try_from(place).unwrap_or(u32::MAX);
                if skip.iter().any(|s| s.note == place && s.at == point.at) {
                    continue;
                }
                let at = note.start + point.at;
                let distance = (at - tick).get().abs();
                if distance < best.0 {
                    best = (distance, at);
                }
            }
        }
        if best.0 <= reach.max(1) { best.1 } else { tick }
    }
}

/// The envelope a lane shows, if it shows one.
fn lane_expression(lane: MpeLane) -> Option<Expression> {
    match lane {
        MpeLane::PitchBend => Some(Expression::Pitch),
        MpeLane::Slide => Some(Expression::Slide),
        MpeLane::Pressure => Some(Expression::Pressure),
        MpeLane::Velocity | MpeLane::ReleaseVelocity => None,
    }
}

fn lane_field(lane: MpeLane) -> Option<NoteField> {
    match lane {
        MpeLane::Velocity => Some(NoteField::Velocity),
        MpeLane::ReleaseVelocity => Some(NoteField::ReleaseVelocity),
        _ => None,
    }
}

/// A value as the tab writes it beside a breakpoint: semitones to two
/// decimals for Pitch, a whole number for the others (R25).
#[must_use]
pub fn value_text(expression: Expression, value: f64) -> String {
    match expression {
        Expression::Pitch => format!("{value:+.2} st"),
        _ => format!("{}", value.round()),
    }
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

fn gesture(events: &mut Vec<UiEvent>, phase: GesturePhase, id: &str, args: Value) {
    events.push(UiEvent::Gesture {
        phase,
        invocation: Some(Invocation::new(id, args)),
    });
}

/// The name of an envelope in the accessibility tree and the menus.
fn expression_name(expression: Expression) -> &'static str {
    match expression {
        Expression::Pitch => "Pitch",
        Expression::Slide => "Slide",
        Expression::Pressure => "Pressure",
    }
}

// --------------------------------------------------------------- hit tests

/// What is under the pointer on an envelope.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    /// A breakpoint.
    Point(Target, Ticks),
    /// The segment starting at a breakpoint, close to it (`on`) or near it.
    Segment { target: Target, at: Ticks, on: bool },
    /// The band of a selected note's lane or envelope region, no envelope
    /// hit.
    Band(Target),
}

fn points_of<'a>(frame: &'a Frame<'_>, target: &Target) -> &'a [Breakpoint] {
    frame
        .note(target.place)
        .map_or(&[], |note| note.envelope(target.expression))
}

/// The envelope drawn at `x`: its value, or none for an empty envelope.
fn line_y(frame: &Frame<'_>, target: &Target, x: f32) -> Option<f32> {
    let note = frame.note(target.place)?;
    let at = (frame.tick(x) - note.start).clamp(Ticks::ZERO, note.length);
    let value = envelope_value(note.envelope(target.expression), at.get() as f64)?;
    Some(target.plot.y(value))
}

/// The segment starting at breakpoint `index`, as its distance from `pos`
/// when `pos` lies over it in time.
fn segment_distance(frame: &Frame<'_>, target: &Target, index: usize, pos: Pos2) -> Option<f32> {
    let note = frame.note(target.place)?;
    let points = note.envelope(target.expression);
    let from = frame.x(note.start + points[index].at);
    let to = points.get(index + 1).map_or_else(
        || frame.x(note.start + note.length),
        |next| frame.x(note.start + next.at),
    );
    // Before the first breakpoint and after the last the envelope holds its
    // value, so the first and last segments reach out to the note's ends.
    let from = if index == 0 {
        from.min(frame.x(note.start))
    } else {
        from
    };
    (pos.x >= from && pos.x <= to)
        .then(|| line_y(frame, target, pos.x).map(|y| (y - pos.y).abs()))
        .flatten()
}

fn hit(frame: &Frame<'_>, targets: &[Target], pos: Pos2) -> Option<Hit> {
    // A breakpoint first, then a segment, then the band.
    for target in targets {
        let Some(note) = frame.note(target.place) else {
            continue;
        };
        for point in points_of(frame, target) {
            if frame.point_pos(note, target.plot, point).distance(pos) <= BREAKPOINT_GRAB {
                return Some(Hit::Point(*target, point.at));
            }
        }
    }
    let mut best: Option<(f32, Hit)> = None;
    for target in targets {
        let points = points_of(frame, target);
        for (index, point) in points.iter().enumerate() {
            let Some(distance) = segment_distance(frame, target, index, pos) else {
                continue;
            };
            if distance <= SEGMENT_NEAR && best.is_none_or(|(d, _)| distance < d) {
                best = Some((
                    distance,
                    Hit::Segment {
                        target: *target,
                        at: point.at,
                        on: distance <= SEGMENT_GRAB,
                    },
                ));
            }
        }
    }
    if let Some((_, hit)) = best {
        return Some(hit);
    }
    targets.iter().copied().find_map(|target| {
        let note = frame.note(target.place)?;
        let (x0, x1) = (frame.x(note.start), frame.x(note.start + note.length));
        let inside = pos.x >= x0 && pos.x <= x1;
        let vertical = match target.plot {
            Plot::Pitch { centre } => (pos.y - centre).abs() <= PITCH_REACH * KEY_HEIGHT,
            Plot::Lane { rect, .. } => pos.y >= rect.top() && pos.y <= rect.bottom(),
        };
        (inside && vertical).then_some(Hit::Band(target))
    })
}

// ----------------------------------------------------------------- drags

/// A drag under way.
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// Moving breakpoints, or a segment in value.
    Move {
        expression: Expression,
        refs: Vec<BreakpointRef>,
        grabbed: BreakpointRef,
        plot: Plot,
        press: Pos2,
        value: f64,
        /// The grabbed note's start, to turn its positions into ticks of
        /// the contents.
        note_start: Ticks,
        segment: bool,
        fill: Option<TickRange>,
        /// With Shift: whether the drag keeps to the horizontal.
        horizontal: Option<bool>,
    },
    /// Curving the segment that starts at a breakpoint.
    Curve {
        place: u32,
        expression: Expression,
        at: Ticks,
        bend: f64,
        press_y: f32,
    },
    /// Selecting the breakpoints a rectangle encloses.
    Marquee {
        expression: Expression,
        from: Pos2,
        to: Pos2,
        targets: Vec<Target>,
    },
    /// Drawing steps freehand over an envelope.
    Draw {
        expression: Expression,
        plot: Plot,
        /// The key of the note the plot is relative to, for Pitch.
        key: f64,
        path: Vec<Pos2>,
    },
    /// Scaling a value over the whole length of the selected notes.
    Scale {
        lane: MpeLane,
        press_y: f32,
        height: f32,
    },
    /// Moving the velocity markers of the selected notes.
    Marker {
        field: NoteField,
        places: Vec<u32>,
        /// Each note's value when the drag began.
        starts: Vec<f64>,
        press_y: f32,
        height: f32,
    },
}

/// A value being typed (R26).
#[derive(Clone, Debug, PartialEq)]
struct Typed {
    text: String,
    action: TypedAction,
}

#[derive(Clone, Debug, PartialEq)]
enum TypedAction {
    /// Edit Value: the breakpoint takes the value, and the other chosen
    /// breakpoints move by the same change.
    Edit {
        expression: Expression,
        target: BreakpointRef,
        current: f64,
        others: Vec<BreakpointRef>,
    },
    /// Add Value: a new breakpoint of this value.
    Add {
        expression: Expression,
        place: u32,
        at: Ticks,
        near: Pos2,
    },
}

/// What a right-click offered.
#[derive(Clone, Debug, PartialEq)]
enum Menu {
    Edit {
        expression: Expression,
        target: BreakpointRef,
        current: f64,
    },
    Add {
        expression: Expression,
        place: u32,
        at: Ticks,
        pos: Pos2,
    },
    Nothing,
}

/// The MPE tab's state between frames: the drag under way, a value being
/// typed, and what the last right-click offered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MpeUi {
    drag: Option<Drag>,
    typed: Option<Typed>,
    menu: Option<Menu>,
    /// Whether a lane's split line is being dragged.
    splitting: bool,
    /// How many Pitch envelopes the last frame drew.
    pitch_drawn: usize,
}

impl MpeUi {
    /// Drops everything under way, as another clip showing does. Returns
    /// whether a drag, which may hold a gesture open, was dropped.
    pub fn reset(&mut self) -> bool {
        let dragged = self.drag.take().is_some() | std::mem::take(&mut self.splitting);
        self.typed = None;
        self.menu = None;
        dragged
    }

    /// Whether a number is being typed, so the keys go to it and not to
    /// shortcuts.
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.typed.is_some()
    }

    /// Whether a drag is under way.
    #[must_use]
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Esc: drops a drag, or else a value being typed. Returns whether a
    /// drag ended, which the caller cancels the gesture of.
    pub fn cancel(&mut self) -> Cancelled {
        if self.drag.take().is_some() {
            Cancelled::Gesture
        } else if self.typed.take().is_some() {
            Cancelled::Typed
        } else {
            Cancelled::Nothing
        }
    }
}

/// What Esc ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cancelled {
    /// A drag, which put the clip back.
    Gesture,
    /// A value being typed.
    Typed,
    /// Nothing.
    Nothing,
}

fn ref_of(place: u32, at: Ticks) -> BreakpointRef {
    BreakpointRef { note: place, at }
}

/// The breakpoints a drag that grabbed `grabbed` moves: the chosen ones of
/// the same envelope if it is one of them, else it alone.
fn moving(frame: &Frame<'_>, expression: Expression, grabbed: BreakpointRef) -> Vec<BreakpointRef> {
    if frame.focus == Some(expression) && frame.chosen.contains(&grabbed) {
        frame.chosen.to_vec()
    } else {
        vec![grabbed]
    }
}

impl MpeUi {
    /// The command of a move drag with the pointer at `pos`, measured from
    /// where it began (R26, R28).
    fn move_args(
        &self,
        frame: &Frame<'_>,
        drag: &Drag,
        pos: Pos2,
        modifiers: Modifiers,
    ) -> Option<Value> {
        let Drag::Move {
            expression,
            refs,
            grabbed,
            plot,
            press,
            value,
            note_start,
            segment,
            fill,
            horizontal,
        } = drag
        else {
            return None;
        };
        let keys = EditKeys::from(modifiers);
        let mut delta = plot.delta(press.y, pos.y);
        if modifiers.shift {
            delta *= FINE_VALUE;
        }
        let mut time = Ticks::ZERO;
        if !*segment {
            let moved = frame.tick(pos.x) - frame.tick(press.x);
            let at = *note_start + grabbed.at;
            time = frame.snapped(at + moved, *expression, refs, keys.bypass) - at;
        }
        // Shift keeps the drag to the axis it starts along.
        match horizontal {
            Some(true) => delta = 0.0,
            Some(false) => time = Ticks::ZERO,
            None => {}
        }
        // Cmd (Alt) with the grid off snaps Pitch to a semitone.
        if *expression == Expression::Pitch && keys.bypass && !frame.grid_on {
            delta = (value + delta).round() - value;
        }
        Some(json!({
            "clip": frame.clip,
            "envelope": expression,
            "breakpoints": refs,
            "time": time,
            "value": delta,
            "fill": fill,
        }))
    }

    /// The command of a drag at `pos`.
    fn drag_command(
        &self,
        frame: &Frame<'_>,
        pos: Pos2,
        modifiers: Modifiers,
    ) -> Option<(&'static str, Value)> {
        let drag = self.drag.as_ref()?;
        match drag {
            Drag::Move { .. } => self
                .move_args(frame, drag, pos, modifiers)
                .map(|args| (ids::MOVE_EXPRESSION_BREAKPOINTS, args)),
            Drag::Curve {
                place,
                expression,
                at,
                bend,
                press_y,
            } => {
                let bend = (bend + f64::from(press_y - pos.y) * CURVE_PER_POINT).clamp(-1.0, 1.0);
                Some((
                    ids::SET_EXPRESSION_CURVE,
                    json!({ "clip": frame.clip, "note": place, "envelope": expression,
                        "at": at, "bend": bend }),
                ))
            }
            Drag::Draw {
                expression,
                plot,
                key,
                path,
            } => {
                let steps = self.drawn(frame, *expression, *plot, *key, path, modifiers);
                (!steps.is_empty()).then(|| {
                    (
                        ids::DRAW_EXPRESSION,
                        json!({ "clip": frame.clip, "notes": frame.selected,
                            "envelope": expression, "steps": steps,
                            "absolute": matches!(plot, Plot::Pitch { .. }) }),
                    )
                })
            }
            Drag::Scale {
                lane,
                press_y,
                height,
            } => {
                let factor =
                    (1.0 + f64::from(press_y - pos.y) / f64::from(height.max(1.0))).max(0.0);
                Some((
                    ids::SCALE_EXPRESSION,
                    json!({ "clip": frame.clip, "notes": frame.selected, "lane": lane,
                        "factor": factor }),
                ))
            }
            Drag::Marker {
                field,
                places,
                starts,
                press_y,
                height,
            } => {
                let (low, high) = field.range();
                let moved = f64::from(press_y - pos.y) / f64::from(height.max(1.0)) * (high - low);
                let values: Vec<f64> = starts
                    .iter()
                    .map(|start| (start + moved).round().clamp(low, high))
                    .collect();
                Some((
                    ids::SET_VALUE,
                    json!({ "clip": frame.clip, "notes": places, "field": field,
                        "values": values }),
                ))
            }
            Drag::Marquee { .. } => None,
        }
    }

    /// The steps a Draw Mode drag has drawn (R30): with the grid, one grid
    /// step wide each; without, one pixel column; each holds the pointer's
    /// height there. Shift gives finer values, and Cmd with the grid off
    /// snaps Pitch to semitones and draws on the grid.
    fn drawn(
        &self,
        frame: &Frame<'_>,
        expression: Expression,
        plot: Plot,
        key: f64,
        path: &[Pos2],
        modifiers: Modifiers,
    ) -> Vec<Value> {
        let keys = EditKeys::from(modifiers);
        let on_grid = frame.grid_on != keys.bypass;
        let step = frame.step.get().max(1);
        let column =
            (frame.length.get() as f64 / f64::from(frame.grid.width().max(1.0))).ceil() as i64;
        let width = if on_grid { step } else { column.max(1) };
        let mut steps: Vec<(i64, f64)> = Vec::new();
        for point in path {
            let tick = frame.tick(point.x).get();
            let index = tick.div_euclid(width);
            let mut value = plot.value(point.y, expression);
            value = if expression == Expression::Pitch {
                // Over the notes Pitch is drawn in key rows; the Pitch Bend
                // lane's axis is the same for every pad.
                let drawn = if matches!(plot, Plot::Pitch { .. }) {
                    key + value
                } else {
                    value
                };
                if keys.bypass && !frame.grid_on {
                    drawn.round()
                } else {
                    drawn
                }
            } else if modifiers.shift {
                (value * 10.0).round() / 10.0
            } else {
                value.round()
            };
            match steps.last_mut() {
                Some(last) if last.0 == index => last.1 = value,
                _ => steps.push((index, value)),
            }
        }
        steps.sort_by_key(|(index, _)| *index);
        steps.dedup_by_key(|(index, _)| *index);
        steps
            .into_iter()
            .map(|(index, value)| json!({ "at": index * width, "length": width, "value": value }))
            .collect()
    }
}

impl Plot {
    /// The change in value from height `from` to height `to`, unclamped, so
    /// a drag past an end carries every breakpoint to it.
    fn delta(self, from: f32, to: f32) -> f64 {
        let per_point = match self {
            Self::Pitch { .. } => 1.0 / f64::from(KEY_HEIGHT),
            Self::Lane { rect, low, high } => (high - low) / f64::from(rect.height().max(1.0)),
        };
        f64::from(from - to) * per_point
    }
}

// ------------------------------------------------------------------ paint

fn dim(colour: Color32, on: bool) -> Color32 {
    if on {
        colour.gamma_multiply(0.35)
    } else {
        colour
    }
}

/// Draws one note's envelope: the line across the note, and its breakpoints
/// unless `dimmed` (R25).
fn paint_envelope(
    ui: &Ui,
    theme: &Theme,
    frame: &Frame<'_>,
    clip_rect: Rect,
    target: &Target,
    dimmed: bool,
) {
    let Some(note) = frame.note(target.place) else {
        return;
    };
    let points = note.envelope(target.expression);
    if points.is_empty() {
        return;
    }
    let painter = ui.painter_at(clip_rect);
    let colours = &theme.colors;
    let line = dim(colours.modulation.0, dimmed);
    let (x0, x1) = (frame.x(note.start), frame.x(note.start + note.length));
    let mut path: Vec<Pos2> = Vec::new();
    let mut x = x0.max(clip_rect.left());
    while x <= x1.min(clip_rect.right()) {
        if let Some(y) = line_y(frame, target, x) {
            path.push(pos2(x, y));
        }
        x += 2.0;
    }
    if path.len() >= 2 {
        painter.add(Shape::line(path, Stroke::new(1.5, line)));
    }
    if dimmed {
        return;
    }
    for point in points {
        let at = frame.point_pos(note, target.plot, point);
        let chosen = frame.is_chosen(target.place, target.expression, point.at);
        let square = Rect::from_center_size(at, vec2(BREAKPOINT_SIZE, BREAKPOINT_SIZE));
        painter.rect_filled(
            square,
            0.0,
            if chosen {
                colours.time_selection.0
            } else {
                colours.text.0
            },
        );
    }
}

impl MpeUi {
    /// Paints the Pitch envelopes over the note area: each selected note's,
    /// in full; the others' dimmed (R24, R25).
    pub fn paint_pitch(&mut self, ui: &Ui, theme: &Theme, frame: &Frame<'_>, rows: &[(u32, f32)]) {
        self.pitch_drawn = rows.len();
        for (place, centre) in rows {
            let target = Target {
                place: *place,
                expression: Expression::Pitch,
                plot: Plot::Pitch { centre: *centre },
            };
            paint_envelope(
                ui,
                theme,
                frame,
                frame.grid,
                &target,
                !frame.is_selected(*place),
            );
        }
    }

    /// How many notes' Pitch envelopes the last frame drew over the note
    /// area: none while Fold is on or on a Drum Rack's track.
    #[must_use]
    pub fn pitch_drawn(&self) -> usize {
        self.pitch_drawn
    }

    /// Paints what hovering and dragging show over the note area and the
    /// lanes: the preview breakpoint on a segment, the value beside a
    /// breakpoint, the marquee and the value being typed.
    pub fn paint_overlay(&self, ui: &Ui, theme: &Theme, frame: &Frame<'_>, targets: &[Target]) {
        let painter = ui.painter();
        let colours = &theme.colors;
        if let Some(Drag::Marquee { from, to, .. }) = &self.drag {
            let marquee = Rect::from_two_pos(*from, *to);
            painter.rect_filled(marquee, 0.0, colours.time_selection.0.gamma_multiply(0.2));
            painter.rect_stroke(
                marquee,
                0.0,
                Stroke::new(1.0, colours.time_selection.0),
                StrokeKind::Inside,
            );
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let label = |at: Pos2, expression: Expression, value: f64| {
            painter.text(
                at + vec2(8.0, -8.0),
                Align2::LEFT_BOTTOM,
                value_text(expression, value),
                theme.small_font(),
                colours.text.0,
            );
        };
        if let Some(Drag::Move {
            expression, plot, ..
        }) = &self.drag
        {
            // The grabbed breakpoint's value beside the pointer.
            if let Some(pos) = pointer {
                label(pos, *expression, plot.value(pos.y, *expression));
            }
        } else if self.drag.is_none()
            && let Some(pos) = pointer
        {
            match hit(frame, targets, pos) {
                Some(Hit::Point(target, at)) => {
                    if let Some(point) = points_of(frame, &target).iter().find(|p| p.at == at)
                        && let Some(note) = frame.note(target.place)
                    {
                        label(
                            frame.point_pos(note, target.plot, point),
                            target.expression,
                            point.value,
                        );
                    }
                }
                Some(Hit::Segment {
                    target, on: true, ..
                }) => {
                    // The preview breakpoint at the pointer (C-MPE-003).
                    if let Some(y) = line_y(frame, &target, pos.x) {
                        let at = pos2(pos.x, y);
                        painter.circle_stroke(
                            at,
                            BREAKPOINT_SIZE * 0.7,
                            Stroke::new(1.0, colours.text.0),
                        );
                        label(
                            at,
                            target.expression,
                            target.plot.value(y, target.expression),
                        );
                    }
                }
                _ => {}
            }
        }
        if let Some(typed) = &self.typed {
            let at = match &typed.action {
                TypedAction::Add { near, .. } => *near,
                TypedAction::Edit { .. } => pointer.unwrap_or(frame.grid.center()),
            };
            painter.text(
                at + vec2(8.0, -8.0),
                Align2::LEFT_BOTTOM,
                format!("{}|", typed.text),
                theme.font(),
                colours.accent.0,
            );
        }
    }
}

/// Paints the band: each lane's header and envelopes, the toggles and the
/// triangle.
pub fn paint_band(
    ui: &Ui,
    theme: &Theme,
    frame: &Frame<'_>,
    layout: &MpeLayout,
    shown: &[MpeLane],
    hidden: bool,
) {
    let painter = ui.painter();
    let colours = &theme.colors;
    painter.rect_filled(layout.band, 0.0, colours.panel.0);
    for (lane, rect) in &layout.lanes {
        if rect.height() < 1.0 {
            continue;
        }
        painter.line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, colours.lane_line.0),
        );
        let plot = layout.plot(*rect);
        let (low, high) = match Plot::of_lane(*lane, plot) {
            Plot::Lane { low, high, .. } => (low, high),
            Plot::Pitch { .. } => (-48.0, 48.0),
        };
        for (value, align) in [(high, Align2::LEFT_TOP), (low, Align2::LEFT_BOTTOM)] {
            let text = if *lane == MpeLane::PitchBend {
                format!("{value:+.0}")
            } else {
                format!("{value:.0}")
            };
            painter.text(
                pos2(plot.left() + 2.0, Plot::of_lane(*lane, plot).y(value)),
                align,
                text,
                theme.small_font(),
                colours.text_dim.0,
            );
        }
        let clip_rect = plot;
        // The selected notes' spans are highlighted in the lane (R25), and
        // their values drawn over them.
        let lane_painter = ui.painter_at(clip_rect);
        for place in frame.selected {
            let Some(note) = frame.note(*place) else {
                continue;
            };
            let band = Rect::from_min_max(
                pos2(frame.x(note.start), plot.top()),
                pos2(
                    frame
                        .x(note.start + note.length)
                        .max(frame.x(note.start) + 2.0),
                    plot.bottom(),
                ),
            );
            lane_painter.rect_filled(band, 0.0, colours.time_selection.0.gamma_multiply(0.12));
        }
        match lane_field(*lane) {
            Some(field) => paint_markers(&lane_painter, theme, frame, field, plot),
            None => {
                let targets = frame.lane_targets(*lane, plot);
                for place in 0..u32::try_from(frame.notes.len()).unwrap_or(u32::MAX) {
                    if frame.is_selected(place) {
                        continue;
                    }
                    let Some(expression) = lane_expression(*lane) else {
                        continue;
                    };
                    let target = Target {
                        place,
                        expression,
                        plot: Plot::of_lane(*lane, plot),
                    };
                    paint_envelope(ui, theme, frame, clip_rect, &target, true);
                }
                for target in &targets {
                    paint_envelope(ui, theme, frame, clip_rect, target, false);
                }
            }
        }
    }
    // The toggles and the triangle.
    for (lane, rect) in &layout.toggles {
        paint_switch(painter, theme, *rect, shown.contains(lane), lane.name());
    }
    paint_triangle(painter, theme, layout.triangle, hidden);
}

/// The velocity markers of a lane: a stem for each note, dimmed for the
/// notes that aren't selected (R25).
fn paint_markers(
    painter: &egui::Painter,
    theme: &Theme,
    frame: &Frame<'_>,
    field: NoteField,
    plot: Rect,
) {
    let lane = Plot::of_lane(
        if field == NoteField::Velocity {
            MpeLane::Velocity
        } else {
            MpeLane::ReleaseVelocity
        },
        plot,
    );
    for (place, note) in frame.notes.iter().enumerate() {
        let place = u32::try_from(place).unwrap_or(u32::MAX);
        let x = frame.x(note.start) + 1.0;
        if x < plot.left() || x > plot.right() {
            continue;
        }
        let top = lane.y(bl_shell::clip_view::note_value(note, field));
        let stem = Rect::from_min_max(
            pos2(x, top),
            pos2(x + crate::clip_view::STEM_WIDTH, plot.bottom()),
        );
        let ink = if frame.is_selected(place) {
            theme.colors.time_selection.0
        } else {
            theme.colors.clip_muted.0
        };
        painter.rect_filled(stem, 0.0, ink);
    }
}

fn paint_switch(painter: &egui::Painter, theme: &Theme, rect: Rect, on: bool, name: &str) {
    let colours = &theme.colors;
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if on {
            colours.accent.0
        } else {
            colours.control.0
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        name.chars().next().unwrap_or(' ').to_string(),
        theme.small_font(),
        if on {
            colours.accent_text.0
        } else {
            colours.text.0
        },
    );
}

fn paint_triangle(painter: &egui::Painter, theme: &Theme, rect: Rect, hidden: bool) {
    let colours = &theme.colors;
    painter.rect_filled(rect, theme.sizes.corner, colours.control.0);
    let c = rect.center();
    // Pointing down while the lanes show, and right while they are hidden.
    let points = if hidden {
        vec![
            c + vec2(-2.0, -3.5),
            c + vec2(3.0, 0.0),
            c + vec2(-2.0, 3.5),
        ]
    } else {
        vec![
            c + vec2(-3.5, -2.0),
            c + vec2(3.5, -2.0),
            c + vec2(0.0, 3.0),
        ]
    };
    painter.add(Shape::convex_polygon(points, colours.text.0, Stroke::NONE));
}

// ------------------------------------------------------------------ input

impl MpeUi {
    /// The toggles, the triangle, the split lines and the lanes: their
    /// clicks and drags, headers' menus and what assistive technology reads.
    #[expect(clippy::too_many_lines, reason = "one pass over the band's input")]
    pub fn band_input(
        &mut self,
        ui: &mut Ui,
        frame: &Frame<'_>,
        layout: &MpeLayout,
        shown: &[MpeLane],
        hidden: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let clip = frame.clip;
        let modifiers = ui.input(|i| i.modifiers);
        let lanes_command = "view.mpe_lanes";
        // The split under the note area: dragging it resizes the lanes
        // together, and dragging it to the bottom hides them (R23).
        if !layout.lanes.is_empty() {
            let split = Rect::from_min_max(
                pos2(layout.band.left(), layout.band.top() - 3.0),
                pos2(layout.band.right(), layout.band.top() - 0.5),
            );
            let response = ui.interact(split, Id::new(("mpe-split", clip)), Sense::drag());
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            if response.dragged() {
                self.splitting = true;
                let total: f32 = layout.lanes.iter().map(|(_, r)| r.height()).sum();
                let wanted = (total - response.drag_delta().y).max(0.0);
                if total > 0.0 {
                    let scale = wanted / total;
                    for (lane, rect) in &layout.lanes {
                        let height = (rect.height() * scale).min(MAX_LANE);
                        invoke(
                            events,
                            lanes_command,
                            json!({ "lane": lane, "height": height.max(MIN_LANE) }),
                        );
                    }
                    if wanted < MIN_LANE {
                        invoke(events, lanes_command, json!({ "visible": false }));
                    }
                }
            }
        }
        for (lane, rect) in &layout.lanes {
            let lane = *lane;
            let rect = *rect;
            let id = Id::new(("mpe-lane", clip, lane));
            if rect.height() < 1.0 {
                continue;
            }
            let plot = layout.plot(rect);
            // Spec: mpe R34. A lane is a group with its name and range.
            let group = ui.interact(rect, id.with("group"), Sense::hover());
            let (low, high) = match Plot::of_lane(lane, plot) {
                Plot::Lane { low, high, .. } => (low, high),
                Plot::Pitch { .. } => (-48.0, 48.0),
            };
            ui.ctx().accesskit_node_builder(group.id, |node| {
                node.set_role(Role::Group);
                node.set_label(format!("{} lane", lane.name()));
                node.set_value(format!("{low:.0} to {high:.0}"));
                node.set_bounds(bounds(rect));
            });
            // The header's menu: swap, and clear.
            let header = Rect::from_min_max(rect.min, pos2(layout.grid.left(), rect.bottom()));
            let header_response = ui.interact(header, id.with("header"), Sense::click());
            header_response.context_menu(|ui| {
                if lane != MpeLane::PitchBend {
                    let item = ui.add_enabled(
                        shown.len() < MpeLane::TOGGLED.len(),
                        egui::Button::new("Swap Lane"),
                    );
                    crate::menus::register_item(ui.ctx(), &item);
                    if item.clicked() {
                        invoke(events, lanes_command, json!({ "lane": lane, "swap": true }));
                        ui.close();
                    }
                }
                clear_item(ui, frame, events);
            });
            let response = ui.interact(plot, id, Sense::click_and_drag());
            let targets = frame.lane_targets(lane, plot);
            self.surface(ui, frame, &response, &targets, Some(lane), plot, events);
            response.context_menu(|ui| {
                self.menu_items(ui, frame, events);
            });
        }

        // The split line under each lane resizes it (I-DRAG-055). They sit
        // over the lanes' own widgets.
        for (lane, rect) in &layout.lanes {
            let (lane, rect) = (*lane, *rect);
            let id = Id::new(("mpe-lane", clip, lane));
            // The split line under the lane resizes it (I-DRAG-055).
            let under = Rect::from_min_max(
                pos2(rect.left(), rect.bottom() - 1.5),
                pos2(rect.right(), rect.bottom() + 1.5),
            );
            let split = ui.interact(under, id.with("split"), Sense::drag());
            if split.hovered() || split.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            if split.dragged() {
                let height = (rect.height() + split.drag_delta().y).clamp(MIN_LANE, MAX_LANE);
                invoke(
                    events,
                    lanes_command,
                    json!({ "lane": lane, "height": height }),
                );
            }
        }
        // The toggles and the triangle are switches that Tab reaches.
        for (lane, rect) in &layout.toggles {
            let id = Id::new(("mpe-toggle", clip, *lane));
            let on = shown.contains(lane);
            if switch(ui, id, *rect, on, lane.name()) {
                invoke(events, lanes_command, json!({ "lane": lane, "show": !on }));
            }
        }
        {
            let id = Id::new(("mpe-triangle", clip));
            let all_off = shown.is_empty();
            if switch(ui, id, layout.triangle, !hidden, "Expression Lanes") {
                // Option (Alt), or every toggle off, shows all four.
                if modifiers.alt || (all_off && !hidden) {
                    invoke(events, lanes_command, json!({ "all": true }));
                } else {
                    invoke(events, lanes_command, json!({ "visible": hidden }));
                }
            }
        }
    }

    /// Every envelope the selected notes show: over the notes, then in the
    /// lanes.
    #[must_use]
    pub fn all_targets(frame: &Frame<'_>, layout: &MpeLayout) -> Vec<Target> {
        let mut targets = frame.pitch_targets();
        for (lane, rect) in &layout.lanes {
            targets.extend(frame.lane_targets(*lane, layout.plot(*rect)));
        }
        targets
    }

    /// The gestures on one surface: a lane's plot, or (through
    /// [`MpeUi::pitch_input`]) the note area. Returns whether a press, click
    /// or drag was taken, so the note editor's own don't run.
    #[expect(clippy::too_many_arguments, reason = "one surface's whole context")]
    pub fn surface(
        &mut self,
        ui: &mut Ui,
        frame: &Frame<'_>,
        response: &Response,
        targets: &[Target],
        lane: Option<MpeLane>,
        plot: Rect,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        let modifiers = ui.input(|i| i.modifiers);
        let keys = EditKeys::from(modifiers);
        let clip = frame.clip;
        let mut taken = false;
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        // A lane of markers: dragging a marker sets the note's value
        // (R26); anything else in a selected note's band scales (R29).
        let field = lane.and_then(lane_field);
        if response.drag_started()
            && self.drag.is_none()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            taken = self.press(frame, targets, lane, field, plot, press, modifiers);
        }
        // Esc ends a drag in egui's eyes too; the editor's Esc puts the clip
        // back, so the drag isn't committed on that frame.
        let escaped = ui.input(|i| i.key_pressed(Key::Escape));
        if taken || self.drag.is_some() {
            if response.dragged()
                && !escaped
                && let Some(pos) = pointer
            {
                self.drag_update(frame, pos, modifiers, events);
            }
            if response.drag_stopped() && !escaped {
                self.drag_stop(frame, pointer, modifiers, events);
            }
            taken = true;
        }
        // Spec: mpe R26. Clicks.
        if response.clicked()
            && !taken
            && let Some(pos) = response.interact_pointer_pos()
        {
            taken = self.click(
                frame, targets, lane, field, plot, pos, keys, modifiers, events,
            );
        }
        if response.double_clicked()
            && modifiers.alt
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(Hit::Segment { target, at, .. }) = hit(frame, targets, pos)
        {
            invoke(
                events,
                ids::SET_EXPRESSION_CURVE,
                json!({ "clip": clip, "note": target.place, "envelope": target.expression,
                    "at": at, "bend": 0.0 }),
            );
        }
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.menu = Some(self.menu_at(frame, targets, pos, keys));
        }
        if let Some(pos) = response.hover_pos()
            && !targets.is_empty()
            && self.drag.is_none()
            && matches!(
                hit(frame, targets, pos),
                Some(Hit::Point(..) | Hit::Segment { .. })
            )
        {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        taken
    }

    /// A press that starts a drag. Returns whether it was taken.
    #[expect(clippy::too_many_arguments, reason = "one press's whole context")]
    fn press(
        &mut self,
        frame: &Frame<'_>,
        targets: &[Target],
        lane: Option<MpeLane>,
        field: Option<NoteField>,
        plot: Rect,
        press: Pos2,
        modifiers: Modifiers,
    ) -> bool {
        let height = plot.height();
        // Draw Mode draws over an envelope's lane, or over a selected note's
        // Pitch (R30).
        if frame.draw_mode {
            let target = match hit(frame, targets, press) {
                Some(Hit::Point(t, _) | Hit::Segment { target: t, .. } | Hit::Band(t)) => Some(t),
                None => None,
            };
            let target = target.or_else(|| lane.and_then(|_| targets.first().copied()));
            return match target {
                Some(target) if lane.is_none_or(|l| lane_field(l).is_none()) => {
                    let key = frame
                        .note(target.place)
                        .map_or(0.0, |note| f64::from(note.key));
                    self.drag = Some(Drag::Draw {
                        expression: target.expression,
                        plot: target.plot,
                        key,
                        path: vec![press],
                    });
                    true
                }
                _ => false,
            };
        }
        // A velocity lane: a marker drags its note's value; elsewhere in a
        // selected note's band the lane scales.
        if let (Some(field), Some(lane)) = (field, lane) {
            let lane_plot = Plot::of_lane(lane, plot);
            let marker = frame.notes.iter().enumerate().find_map(|(place, note)| {
                let x = frame.x(note.start) + crate::clip_view::STEM_WIDTH / 2.0 + 1.0;
                let top = lane_plot.y(bl_shell::clip_view::note_value(note, field));
                ((press.x - x).abs() <= crate::clip_view::STEM_WIDTH / 2.0 + 3.0
                    && press.y >= top - 3.0)
                    .then(|| u32::try_from(place).unwrap_or(u32::MAX))
            });
            if let Some(place) = marker {
                let places = if frame.is_selected(place) {
                    frame.selected.to_vec()
                } else {
                    vec![place]
                };
                let starts = places
                    .iter()
                    .filter_map(|place| frame.note(*place))
                    .map(|note| bl_shell::clip_view::note_value(note, field))
                    .collect();
                self.drag = Some(Drag::Marker {
                    field,
                    places,
                    starts,
                    press_y: press.y,
                    height,
                });
                return true;
            }
            if let Some(Hit::Band(_)) = hit(
                frame,
                &frame
                    .selected
                    .iter()
                    .map(|p| Target {
                        place: *p,
                        expression: Expression::Slide,
                        plot: lane_plot,
                    })
                    .collect::<Vec<_>>(),
                press,
            ) {
                self.drag = Some(Drag::Scale {
                    lane,
                    press_y: press.y,
                    height,
                });
                return true;
            }
            return false;
        }
        match hit(frame, targets, press) {
            Some(Hit::Point(target, at)) => {
                let grabbed = ref_of(target.place, at);
                let note_start = frame.note(target.place).map_or(Ticks::ZERO, |n| n.start);
                let value = points_of(frame, &target)
                    .iter()
                    .find(|p| p.at == at)
                    .map_or(0.0, |p| p.value);
                self.drag = Some(Drag::Move {
                    expression: target.expression,
                    refs: moving(frame, target.expression, grabbed),
                    grabbed,
                    plot: target.plot,
                    press,
                    value,
                    note_start,
                    segment: false,
                    fill: None,
                    horizontal: None,
                });
                true
            }
            Some(Hit::Segment { target, at, .. }) => {
                let points = points_of(frame, &target);
                let note_start = frame.note(target.place).map_or(Ticks::ZERO, |n| n.start);
                let bend = points.iter().find(|p| p.at == at).map_or(0.0, |p| p.bend);
                if modifiers.alt {
                    self.drag = Some(Drag::Curve {
                        place: target.place,
                        expression: target.expression,
                        at,
                        bend,
                        press_y: press.y,
                    });
                    return true;
                }
                let index = points.iter().position(|p| p.at == at).unwrap_or(0);
                let ends: Vec<BreakpointRef> =
                    [Some(index), points.get(index + 1).map(|_| index + 1)]
                        .into_iter()
                        .flatten()
                        .map(|i| ref_of(target.place, points[i].at))
                        .collect();
                let chosen = ends
                    .iter()
                    .all(|e| frame.focus == Some(target.expression) && frame.chosen.contains(e));
                if chosen {
                    // A selected segment moves in value, whole inside the
                    // time selection (I-DRAG-050).
                    let (from, to) = (
                        note_start + points[index].at,
                        note_start + points.get(index + 1).map_or(points[index].at, |p| p.at),
                    );
                    let fill = frame
                        .time
                        .filter(|(a, b)| from < *b && to > *a)
                        .map(|(start, end)| TickRange { start, end });
                    let refs = if frame.chosen.is_empty() {
                        ends.clone()
                    } else {
                        frame.chosen.to_vec()
                    };
                    self.drag = Some(Drag::Move {
                        expression: target.expression,
                        grabbed: ends[0],
                        refs,
                        plot: target.plot,
                        press,
                        value: points[index].value,
                        note_start,
                        segment: true,
                        fill,
                        horizontal: None,
                    });
                    true
                } else if let Some(lane) = lane.filter(|_| target.expression != Expression::Pitch) {
                    // An unselected segment: dragging it scales (R29).
                    self.drag = Some(Drag::Scale {
                        lane,
                        press_y: press.y,
                        height,
                    });
                    true
                } else {
                    self.marquee(targets, target.expression, press);
                    true
                }
            }
            Some(Hit::Band(target)) => {
                self.marquee(targets, target.expression, press);
                true
            }
            None => {
                // Outside every selected note's band, a lane selects nothing
                // by dragging.
                false
            }
        }
    }

    fn marquee(&mut self, targets: &[Target], expression: Expression, press: Pos2) {
        self.drag = Some(Drag::Marquee {
            expression,
            from: press,
            to: press,
            targets: targets.to_vec(),
        });
    }

    /// One frame of a drag.
    fn drag_update(
        &mut self,
        frame: &Frame<'_>,
        pos: Pos2,
        modifiers: Modifiers,
        events: &mut Vec<UiEvent>,
    ) {
        match &mut self.drag {
            Some(Drag::Draw { path, .. }) if path.last() != Some(&pos) => path.push(pos),
            Some(Drag::Marquee { to, .. }) => *to = pos,
            Some(Drag::Move {
                press, horizontal, ..
            }) if modifiers.shift && horizontal.is_none() => {
                let moved = pos - *press;
                if moved.length() >= AXIS_DECIDES {
                    *horizontal = Some(moved.x.abs() >= moved.y.abs());
                }
            }
            Some(Drag::Move { horizontal, .. }) if !modifiers.shift => *horizontal = None,
            _ => {}
        }
        if let Some(Drag::Marquee {
            expression,
            from,
            to,
            targets,
        }) = &self.drag
        {
            let area = Rect::from_two_pos(*from, *to);
            let inside: Vec<BreakpointRef> = targets
                .iter()
                .filter(|t| t.expression == *expression)
                .flat_map(|target| {
                    let note = frame.note(target.place);
                    points_of(frame, target)
                        .iter()
                        .filter(move |p| {
                            note.is_some_and(|note| {
                                area.contains(frame.point_pos(note, target.plot, p))
                            })
                        })
                        .map(|p| ref_of(target.place, p.at))
                        .collect::<Vec<_>>()
                })
                .collect();
            invoke(
                events,
                ids::SELECT_EXPRESSION_BREAKPOINTS,
                json!({ "clip": frame.clip, "envelope": expression, "breakpoints": inside }),
            );
        } else if let Some((id, args)) = self.drag_command(frame, pos, modifiers) {
            gesture(events, GesturePhase::Update, id, args);
        }
    }

    /// The end of a drag: its last command and the commit.
    fn drag_stop(
        &mut self,
        frame: &Frame<'_>,
        pointer: Option<Pos2>,
        modifiers: Modifiers,
        events: &mut Vec<UiEvent>,
    ) {
        let last = pointer
            .and_then(|pos| self.drag_command(frame, pos, modifiers))
            .map(|(id, args)| Invocation::new(id, args));
        let marquee = matches!(self.drag, Some(Drag::Marquee { .. }));
        if self.drag.take().is_some() && !marquee {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: last,
            });
        }
    }

    /// A click on a surface. Returns whether it did something.
    #[expect(clippy::too_many_arguments, reason = "one click's whole context")]
    fn click(
        &mut self,
        frame: &Frame<'_>,
        targets: &[Target],
        lane: Option<MpeLane>,
        field: Option<NoteField>,
        plot: Rect,
        pos: Pos2,
        keys: EditKeys,
        modifiers: Modifiers,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        let clip = frame.clip;
        if let (Some(field), Some(lane)) = (field, lane) {
            // A click on a velocity marker selects its note.
            let lane_plot = Plot::of_lane(lane, plot);
            let place = frame.notes.iter().position(|note| {
                let x = frame.x(note.start) + crate::clip_view::STEM_WIDTH / 2.0 + 1.0;
                let top = lane_plot.y(bl_shell::clip_view::note_value(note, field));
                (pos.x - x).abs() <= crate::clip_view::STEM_WIDTH / 2.0 + 3.0 && pos.y >= top - 3.0
            });
            if let Some(place) = place {
                invoke(
                    events,
                    ids::SELECT_NOTES,
                    json!({ "clip": clip, "notes": [place],
                        "mode": if modifiers.shift || modifiers.command { "toggle" } else { "replace" } }),
                );
                return true;
            }
            return false;
        }
        let envelope_of = |target: &Target| target.expression;
        match hit(frame, targets, pos) {
            // Clicking a breakpoint deletes it (I-CLICK-046).
            Some(Hit::Point(target, at)) => {
                invoke(
                    events,
                    ids::DELETE_EXPRESSION_BREAKPOINTS,
                    json!({ "clip": clip, "envelope": envelope_of(&target),
                        "breakpoints": [ref_of(target.place, at)] }),
                );
                true
            }
            // Clicking on a segment adds a breakpoint; near it, or with
            // Shift, selects the segment (I-CLICK-045, I-CLICK-047).
            Some(Hit::Segment { target, at, on }) => {
                if on && !modifiers.shift {
                    if let Some(note) = frame.note(target.place) {
                        let tick =
                            frame.snapped(frame.tick(pos.x), target.expression, &[], keys.bypass);
                        let rel = (tick - note.start).clamp(Ticks::ZERO, note.length);
                        let on_line =
                            envelope_value(note.envelope(target.expression), rel.get() as f64)
                                .unwrap_or_else(|| target.plot.value(pos.y, target.expression));
                        let value = if target.expression == Expression::Pitch
                            && keys.bypass
                            && !frame.grid_on
                        {
                            on_line.round()
                        } else {
                            on_line
                        };
                        invoke(
                            events,
                            ids::ADD_EXPRESSION_BREAKPOINT,
                            json!({ "clip": clip, "notes": [target.place],
                                "envelope": target.expression, "at": rel, "value": value }),
                        );
                        invoke(
                            events,
                            ids::SELECT_EXPRESSION_BREAKPOINTS,
                            json!({ "clip": clip, "envelope": target.expression,
                                "breakpoints": [ref_of(target.place, rel)] }),
                        );
                    }
                } else {
                    let points = points_of(frame, &target);
                    let index = points.iter().position(|p| p.at == at).unwrap_or(0);
                    let ends: Vec<BreakpointRef> =
                        [Some(index), points.get(index + 1).map(|_| index + 1)]
                            .into_iter()
                            .flatten()
                            .map(|i| ref_of(target.place, points[i].at))
                            .collect();
                    invoke(
                        events,
                        ids::SELECT_EXPRESSION_BREAKPOINTS,
                        json!({ "clip": clip, "envelope": target.expression, "breakpoints": ends }),
                    );
                }
                true
            }
            // Clicking the background of an envelope gives it the keyboard
            // (Cmd-A then selects its breakpoints); the first breakpoint of
            // an empty envelope is made where it was clicked.
            Some(Hit::Band(target)) => {
                if points_of(frame, &target).is_empty()
                    && lane.is_some()
                    && let Some(note) = frame.note(target.place)
                {
                    let tick =
                        frame.snapped(frame.tick(pos.x), target.expression, &[], keys.bypass);
                    let rel = (tick - note.start).clamp(Ticks::ZERO, note.length);
                    invoke(
                        events,
                        ids::ADD_EXPRESSION_BREAKPOINT,
                        json!({ "clip": clip, "notes": [target.place],
                            "envelope": target.expression, "at": rel,
                            "value": target.plot.value(pos.y, target.expression).round() }),
                    );
                }
                invoke(
                    events,
                    ids::SELECT_EXPRESSION_BREAKPOINTS,
                    json!({ "clip": clip, "envelope": target.expression, "breakpoints": [] }),
                );
                lane.is_some()
            }
            None => {
                if let Some(lane) = lane
                    && let Some(expression) = lane_expression(lane)
                {
                    invoke(
                        events,
                        ids::SELECT_EXPRESSION_BREAKPOINTS,
                        json!({ "clip": clip, "envelope": expression, "breakpoints": [] }),
                    );
                    return true;
                }
                false
            }
        }
    }

    /// What a right-click at `pos` offers (C-MPE-002, C-MPE-003).
    fn menu_at(&self, frame: &Frame<'_>, targets: &[Target], pos: Pos2, keys: EditKeys) -> Menu {
        match hit(frame, targets, pos) {
            Some(Hit::Point(target, at)) => Menu::Edit {
                expression: target.expression,
                target: ref_of(target.place, at),
                current: points_of(frame, &target)
                    .iter()
                    .find(|p| p.at == at)
                    .map_or(0.0, |p| p.value),
            },
            Some(Hit::Segment { target, .. } | Hit::Band(target)) => {
                let Some(note) = frame.note(target.place) else {
                    return Menu::Nothing;
                };
                let tick = frame.snapped(frame.tick(pos.x), target.expression, &[], keys.bypass);
                Menu::Add {
                    expression: target.expression,
                    place: target.place,
                    at: (tick - note.start).clamp(Ticks::ZERO, note.length),
                    pos,
                }
            }
            None => Menu::Nothing,
        }
    }

    /// The pointer's right-click in the note area, for the Pitch envelopes.
    pub fn pitch_menu(&mut self, frame: &Frame<'_>, pos: Pos2, keys: EditKeys) {
        self.menu = Some(self.menu_at(frame, &frame.pitch_targets(), pos, keys));
    }

    /// The items the tab adds to a context menu: Edit Value or Add Value
    /// where the right-click fell, and Clear All Envelopes (C-MPE-001 to
    /// C-MPE-003).
    pub fn menu_items(&mut self, ui: &mut Ui, frame: &Frame<'_>, events: &mut Vec<UiEvent>) {
        match self.menu.clone() {
            Some(Menu::Edit {
                expression,
                target,
                current,
            }) => {
                let item = ui.button("Edit Value");
                crate::menus::register_item(ui.ctx(), &item);
                if item.clicked() {
                    let others =
                        if frame.focus == Some(expression) && frame.chosen.contains(&target) {
                            frame.chosen.to_vec()
                        } else {
                            vec![target]
                        };
                    self.typed = Some(Typed {
                        text: String::new(),
                        action: TypedAction::Edit {
                            expression,
                            target,
                            current,
                            others,
                        },
                    });
                    ui.close();
                }
            }
            Some(Menu::Add {
                expression,
                place,
                at,
                pos,
            }) => {
                let item = ui.button("Add Value");
                crate::menus::register_item(ui.ctx(), &item);
                if item.clicked() {
                    self.typed = Some(Typed {
                        text: String::new(),
                        action: TypedAction::Add {
                            expression,
                            place,
                            at,
                            near: pos,
                        },
                    });
                    ui.close();
                }
            }
            _ => {}
        }
        clear_item(ui, frame, events);
    }

    /// Reads the keys of a value being typed (R26): digits, a minus and a
    /// point, Backspace, Return to set it and Esc to drop it. Takes them out
    /// of the frame's input so shortcuts don't run.
    pub fn type_keys(&mut self, ui: &Ui, frame: &Frame<'_>, events: &mut Vec<UiEvent>) {
        if self.typed.is_none() {
            return;
        }
        let taken: Vec<Key> = ui.input_mut(|i| {
            let mut taken = Vec::new();
            i.events.retain(|event| match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command
                    && !modifiers.ctrl
                    && (character(*key).is_some()
                        || matches!(key, Key::Backspace | Key::Enter | Key::Escape)) =>
                {
                    taken.push(*key);
                    false
                }
                _ => true,
            });
            taken
        });
        for key in taken {
            let Some(mut typed) = self.typed.take() else {
                break;
            };
            match key {
                Key::Escape => {}
                Key::Enter => {
                    if let Ok(value) = typed.text.parse::<f64>() {
                        self.set_typed_value(frame, &typed.action, value, events);
                    } else {
                        self.typed = Some(typed);
                    }
                }
                Key::Backspace => {
                    typed.text.pop();
                    self.typed = Some(typed);
                }
                key => {
                    if let Some(c) = character(key)
                        && typed.text.len() < LONGEST
                    {
                        typed.text.push(c);
                    }
                    self.typed = Some(typed);
                }
            }
        }
    }

    fn set_typed_value(
        &self,
        frame: &Frame<'_>,
        action: &TypedAction,
        value: f64,
        events: &mut Vec<UiEvent>,
    ) {
        match action {
            TypedAction::Edit {
                expression,
                target,
                current,
                others,
            } => {
                let (low, high) = expression.range();
                invoke(
                    events,
                    ids::MOVE_EXPRESSION_BREAKPOINTS,
                    json!({ "clip": frame.clip, "envelope": expression,
                        "breakpoints": others, "time": 0,
                        "value": value.clamp(low, high) - current }),
                );
                invoke(
                    events,
                    ids::SELECT_EXPRESSION_BREAKPOINTS,
                    json!({ "clip": frame.clip, "envelope": expression,
                        "breakpoints": [target] }),
                );
            }
            TypedAction::Add {
                expression,
                place,
                at,
                ..
            } => {
                let (low, high) = expression.range();
                invoke(
                    events,
                    ids::ADD_EXPRESSION_BREAKPOINT,
                    json!({ "clip": frame.clip, "notes": [place], "envelope": expression,
                        "at": at, "value": value.clamp(low, high) }),
                );
                invoke(
                    events,
                    ids::SELECT_EXPRESSION_BREAKPOINTS,
                    json!({ "clip": frame.clip, "envelope": expression,
                        "breakpoints": [ref_of(*place, *at)] }),
                );
            }
        }
    }

    /// Reads what assistive technology gets of the selected notes' envelopes
    /// (R34): for each, a list with its breakpoints' positions and values.
    pub fn describe(&self, ui: &Ui, frame: &Frame<'_>, layout: &MpeLayout) {
        let lanes_shown = |expression: Expression| {
            layout.lanes.iter().find_map(|(lane, rect)| {
                (lane_expression(*lane) == Some(expression)).then_some(layout.plot(*rect))
            })
        };
        for place in frame.selected {
            let Some(note) = frame.note(*place) else {
                continue;
            };
            for expression in Expression::ALL {
                let points = note.envelope(expression);
                if points.is_empty() {
                    continue;
                }
                let rect = match expression {
                    Expression::Pitch => frame.grid,
                    _ => lanes_shown(expression).unwrap_or(layout.band),
                };
                let id = Id::new((
                    "mpe-envelope",
                    frame.clip,
                    *place,
                    expression_name(expression),
                ));
                let list = ui.interact(
                    Rect::from_min_size(rect.min, vec2(1.0, 1.0)),
                    id,
                    Sense::hover(),
                );
                ui.ctx().accesskit_node_builder(list.id, |node| {
                    node.set_role(Role::List);
                    node.set_label(format!(
                        "{} of {}",
                        expression_name(expression),
                        crate::clip_view::key_label(frame.tuning, note.key)
                    ));
                    node.set_value(format!("{} breakpoints", points.len()));
                    node.set_bounds(bounds(rect));
                });
                for point in points {
                    let item = ui.interact(
                        Rect::from_center_size(
                            pos2(frame.x(note.start + point.at), rect.center().y),
                            vec2(1.0, 1.0),
                        ),
                        id.with(point.at.get()),
                        Sense::hover(),
                    );
                    ui.ctx().accesskit_node_builder(item.id, |node| {
                        node.set_role(Role::ListItem);
                        node.set_label(format!(
                            "{} breakpoint at tick {}",
                            expression_name(expression),
                            point.at.get()
                        ));
                        node.set_value(value_text(expression, point.value));
                    });
                }
            }
        }
    }

    /// The Pitch envelopes' gestures in the note area, ahead of the note
    /// editor's own. Returns whether the press, click or drag was taken.
    pub fn pitch_input(
        &mut self,
        ui: &mut Ui,
        frame: &Frame<'_>,
        response: &Response,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        let targets = frame.pitch_targets();
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let keys = EditKeys::from(ui.input(|i| i.modifiers));
            self.pitch_menu(frame, pos, keys);
        }
        self.surface(ui, frame, response, &targets, None, frame.grid, events)
    }
}

fn character(key: Key) -> Option<char> {
    Some(match key {
        Key::Num0 => '0',
        Key::Num1 => '1',
        Key::Num2 => '2',
        Key::Num3 => '3',
        Key::Num4 => '4',
        Key::Num5 => '5',
        Key::Num6 => '6',
        Key::Num7 => '7',
        Key::Num8 => '8',
        Key::Num9 => '9',
        Key::Minus => '-',
        Key::Period => '.',
        _ => return None,
    })
}

fn bounds(rect: Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// Clear All Envelopes (C-MPE-001), in the note area's and each lane's menu.
fn clear_item(ui: &mut Ui, frame: &Frame<'_>, events: &mut Vec<UiEvent>) {
    ui.separator();
    let item = ui.add_enabled(
        !frame.selected.is_empty(),
        egui::Button::new("Clear All Envelopes"),
    );
    crate::menus::register_item(ui.ctx(), &item);
    if item.clicked() {
        invoke(
            events,
            ids::CLEAR_EXPRESSIONS,
            json!({ "clip": frame.clip, "notes": frame.selected }),
        );
        ui.close();
    }
}

/// A small switch that Tab reaches: returns whether it was clicked, or Enter
/// pressed on it (R34).
fn switch(ui: &mut Ui, id: Id, rect: Rect, on: bool, name: &str) -> bool {
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let response = ui.interact(rect, id, Sense::click());
    let pressed =
        response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        ui.painter().rect_stroke(
            rect,
            2.0,
            Stroke::new(1.5, ui.visuals().selection.stroke.color),
            StrokeKind::Outside,
        );
    }
    let name = name.to_owned();
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, name.clone()));
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 400.0))
    }

    // Spec: mpe R23. The toggles stack at the left of the band, the triangle
    // under them, and the lanes stack from the band's top.
    #[test]
    fn the_band_holds_the_toggles_the_triangle_and_the_lanes() {
        let layout = layout(
            view(),
            &[(MpeLane::Slide, 43.0), (MpeLane::Pressure, 43.0)],
            false,
        );
        assert_eq!(layout.toggles.len(), 4);
        assert!(layout.triangle.top() > layout.toggles[3].1.bottom() - 1.0);
        assert_eq!(layout.lanes[0].1.top(), layout.band.top());
        assert_eq!(layout.lanes[1].1.top(), layout.lanes[0].1.bottom());
        assert_eq!(layout.grid.bottom(), layout.band.top());
        // One lane is shorter than the toggles need: the band keeps its room.
        let one = self::layout(view(), &[(MpeLane::Slide, 43.0)], false);
        assert!(one.band.height() >= BAND_MIN);
        // Hidden, only the triangle remains.
        let hidden = self::layout(view(), &[], true);
        assert!(hidden.toggles.is_empty() && hidden.lanes.is_empty());
        assert!(hidden.band.height() <= BAND_HIDDEN + 0.5);
    }

    // Spec: mpe R24. A semitone is a row, and +12 sits an octave above.
    #[test]
    fn pitch_is_drawn_a_row_to_the_semitone_from_the_notes_row() {
        let plot = Plot::Pitch { centre: 100.0 };
        assert_eq!(plot.y(0.0), 100.0);
        assert_eq!(plot.y(12.0), 100.0 - 12.0 * KEY_HEIGHT);
        assert!((plot.value(100.0 - 12.0 * KEY_HEIGHT, Expression::Pitch) - 12.0).abs() < 1e-6);
        assert_eq!(plot.value(-10_000.0, Expression::Pitch), 48.0);
    }

    // Spec: mpe R25. Pitch shows to two decimals and the others whole.
    #[test]
    fn values_read_as_the_tab_writes_them() {
        assert_eq!(value_text(Expression::Pitch, 1.5), "+1.50 st");
        assert_eq!(value_text(Expression::Slide, 63.6), "64");
    }

    #[test]
    fn a_lane_plot_runs_from_its_lowest_value_at_the_bottom() {
        let rect = Rect::from_min_size(pos2(0.0, 100.0), vec2(100.0, 40.0));
        let plot = Plot::of_lane(MpeLane::Slide, rect);
        assert_eq!(plot.y(0.0), 140.0);
        assert_eq!(plot.y(127.0), 100.0);
        assert!((plot.value(120.0, Expression::Slide) - 63.5).abs() < 1e-6);
    }
}
