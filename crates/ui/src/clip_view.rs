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
//! Nothing here owns the state: whether the view shows, Draw Mode, the note
//! selection, and the Quantize settings are the shell's. Only the vertical
//! scroll and the drags under way live here.

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::registry::{SET_MIDI_LOOP, SET_MIDI_REGION};
use bl_model::{ClipData, ClipId, Document, MidiData, Note};
use bl_shell::arrangement::ArrangementView;
use bl_shell::clip_view::{move_limits, notes_in_order, resize_limit};
use bl_shell::note_commands as ids;
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
/// How close to a note's end, in points, a press resizes it rather than
/// moving it.
pub const EDGE_GRAB: f32 = 4.0;
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

/// The editor ruler and marker strip above the note area (L-CLIP-011):
/// the clip's Start and End markers and its loop brace.
#[must_use]
pub fn strip_rect(view: Rect) -> Rect {
    Rect::from_min_max(view.min, pos2(view.max.x, view.top() + STRIP_HEIGHT))
}

/// The note area: the Clip View without its marker strip and key track.
#[must_use]
pub fn grid_rect(view: Rect) -> Rect {
    Rect::from_min_max(
        pos2(view.left() + KEY_WIDTH, view.top() + STRIP_HEIGHT),
        view.max,
    )
}

/// Where a MIDI clip's End marker is, in ticks: its `end`, or the clip's
/// end.
fn end_tick(midi: &MidiData, length: Ticks) -> Ticks {
    midi.end.unwrap_or(midi.start + length)
}

/// The loop brace's ends, in ticks: `loop_start`, and `loop_end` (`null`
/// ending at the clip's end).
fn loop_ticks(midi: &MidiData, length: Ticks) -> (Ticks, Ticks) {
    (
        midi.loop_start,
        midi.loop_end.unwrap_or(midi.start + length),
    )
}

/// Where a key's row starts in `grid`, with `top` the key at the grid's
/// top edge.
#[must_use]
pub fn y_of_key(grid: Rect, top: f32, key: u8) -> f32 {
    grid.top() + (top - f32::from(key)) * KEY_HEIGHT
}

/// The key at `y` in `grid`, continuous, with `top` the key at its top
/// edge.
fn key_at(grid: Rect, top: f32, y: f32) -> f32 {
    top - (y - grid.top()) / KEY_HEIGHT
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

/// A note's rectangle in `grid`.
#[must_use]
pub fn note_rect(grid: Rect, start: Ticks, length: Ticks, top: f32, note: &Note) -> Rect {
    let x0 = x_of_tick(grid, start, length, note.start);
    let x1 = x_of_tick(grid, start, length, note.start + note.length);
    let y0 = y_of_key(grid, top, note.key);
    Rect::from_min_max(pos2(x0, y0), pos2(x1.max(x0 + 2.0), y0 + KEY_HEIGHT))
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
    top: f32,
    notes: &[Note],
    pos: Pos2,
) -> Option<(u32, NoteZone)> {
    if !grid.contains(pos) {
        return None;
    }
    // Later notes draw over earlier ones, so the last one wins.
    notes.iter().enumerate().rev().find_map(|(place, note)| {
        let rect = note_rect(grid, start, length, top, note);
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

/// The Clip View's own state: the vertical scroll and the drags under way.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipViewUi {
    /// The clip the editor was last laid out for.
    shown: Option<ClipId>,
    /// The key at the note area's top edge, continuous.
    top: f32,
    /// The drag under way in the note area, if any.
    drag: Option<NoteDrag>,
    /// The drag under way in the marker strip, if any.
    strip_drag: Option<StripDrag>,
    /// The marquee under way: where it began, and where it is now.
    marquee: Option<(Pos2, Pos2)>,
}

impl Default for ClipViewUi {
    fn default() -> Self {
        Self {
            shown: None,
            top: DEFAULT_TOP,
            drag: None,
            strip_drag: None,
            marquee: None,
        }
    }
}

impl ClipViewUi {
    /// The key at the note area's top edge.
    #[must_use]
    pub fn top(&self) -> f32 {
        self.top
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
            self.strip_drag = None;
            self.marquee = None;
            return;
        };
        if self.shown != Some(clip) {
            // Another clip shows: scroll to its notes.
            self.shown = Some(clip);
            self.top = default_top(&midi);
            self.drag = None;
            self.strip_drag = None;
            self.marquee = None;
        }
        let grid = grid_rect(rect);
        let strip = strip_rect(rect);
        if grid.width() < 1.0 || grid.height() < KEY_HEIGHT || strip.height() < 1.0 {
            return;
        }
        let notes = notes_in_order(&midi);
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
        self.paint(
            ui, shell, theme, clip, strip, grid, &midi, axis, length, &notes, &selected,
        );
        self.input(
            ui,
            shell,
            doc,
            shell.arrangement(),
            strip,
            grid,
            clip,
            &midi,
            axis,
            length,
            &notes,
            events,
        );
    }

    /// Paints the marker strip with the clip's markers and loop brace, the
    /// key track, the grid, the notes, and the marquee. `axis` is the
    /// contents tick the note area's left edge shows: `MidiData::start`,
    /// or the tick a Start marker drag began on (R4).
    #[expect(clippy::too_many_arguments, reason = "the editor's whole layout")]
    fn paint(
        &self,
        ui: &Ui,
        shell: &Shell,
        theme: &Theme,
        clip: ClipId,
        strip: Rect,
        grid: Rect,
        midi: &MidiData,
        axis: Ticks,
        length: Ticks,
        notes: &[Note],
        selected: &[u32],
    ) {
        let painter = ui.painter();
        let colours = &theme.colors;
        let start = axis;
        let top = self.top;

        self.paint_strip(painter, theme, strip, grid, midi, axis, length);

        // The key track: white and black keys, each named when its row has
        // the room, the C rows otherwise.
        let named = |key: u8| KEY_HEIGHT >= 7.0 || key.is_multiple_of(12);
        for key in 0..=127u8 {
            let y0 = y_of_key(grid, top, key);
            if y0 + KEY_HEIGHT < grid.top() || y0 > grid.bottom() {
                continue;
            }
            let row = Rect::from_min_max(
                pos2(grid.left() - KEY_WIDTH, y0),
                pos2(grid.left(), y0 + KEY_HEIGHT),
            );
            let black = matches!(key % 12, 1 | 3 | 6 | 8 | 10);
            painter.rect_filled(
                row,
                0.0,
                if black {
                    colours.bar.0
                } else {
                    colours.control.0
                },
            );
            painter.line_segment(
                [row.left_bottom(), row.right_bottom()],
                Stroke::new(1.0, colours.lane_line.0),
            );
            if named(key) {
                painter.text(
                    pos2(row.right() - 2.0, row.center().y),
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
        for key in 0..=127u8 {
            let y = y_of_key(grid, top, key) + KEY_HEIGHT;
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
            let rect = note_rect(grid, start, length, top, note);
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

    /// The marker strip's and the note area's clicks, drags, and wheel, and
    /// the note-selection keys.
    #[expect(clippy::too_many_arguments, reason = "the editor's whole input")]
    fn input(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        doc: &Document,
        arrangement: &ArrangementView,
        strip: Rect,
        grid: Rect,
        clip: ClipId,
        midi: &MidiData,
        axis: Ticks,
        length: Ticks,
        notes: &[Note],
        events: &mut Vec<UiEvent>,
    ) {
        // Spec: piano-roll R11. The marker strip's drags run the document's
        // clip commands, one gesture each.
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
            events,
        );
        let response = ui.interact(
            grid,
            Id::new(("clip-view-grid", clip)),
            Sense::click_and_drag(),
        );
        let keys = EditKeys::from(ui.input(|i| i.modifiers));
        let draw_mode = shell.clip_view().draw_mode();
        let start = axis;
        let hit = |pos: Pos2| hit_note(grid, start, length, self.top, notes, pos);

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
                        grab_key: key_at(grid, self.top, press.y),
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
                    drag.invocation(arrangement, doc, keys, grid, self.top, start, length, pos);
                gesture(events, GesturePhase::Update, id, args);
            } else if let (Some((from, _)), Some(pos)) = (self.marquee, pointer) {
                self.marquee = Some((from, pos));
                let marquee = Rect::from_two_pos(from, pos);
                let inside: Vec<u32> = notes
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| {
                        note_rect(grid, start, length, self.top, note).intersects(marquee)
                    })
                    .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
                    .collect();
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::SELECT_NOTES,
                    json!({ "clip": clip, "notes": inside, "mode": "replace" }),
                )));
            }
        }
        if self.drag.is_some() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
        if response.drag_stopped() {
            let last = match (self.drag.as_ref(), pointer) {
                (Some(drag), Some(pos)) => {
                    let (id, args) =
                        drag.invocation(arrangement, doc, keys, grid, self.top, start, length, pos);
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
                    let key = key_at(grid, self.top, pos.y).round().clamp(0.0, 127.0) as u8;
                    events.push(UiEvent::Invoke(Invocation::new(
                        ids::ADD_NOTE,
                        json!({ "clip": clip, "key": key, "start": at, "length": step }),
                    )));
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
        // The wheel scrolls the key track.
        if ui.rect_contains_pointer(grid) {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.top = (self.top + scroll / KEY_HEIGHT).clamp(0.0, 127.0);
            }
        }
        // Spec: piano-roll. S-A11Y-008 and S-A11Y-009, the note-selection
        // keys: with notes selected, Up and Down transpose them a semitone,
        // and with Shift an octave. The Clip View holds the detail area's
        // keys (the shell's contexts say which apply).
        if shell.active_contexts().contains(&Context::NoteSelection) {
            let semitones = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } if !modifiers.command && !modifiers.ctrl && !modifiers.alt => match key {
                            Key::ArrowUp if modifiers.shift => Some(12),
                            Key::ArrowUp => Some(1),
                            Key::ArrowDown if modifiers.shift => Some(-12),
                            Key::ArrowDown => Some(-1),
                            _ => None,
                        },
                        _ => None,
                    })
                    .next_back()
            });
            if let Some(semitones) = semitones {
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::MOVE_NOTES,
                    json!({ "clip": clip, "semitones": semitones }),
                )));
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
        events: &mut Vec<UiEvent>,
    ) {
        let response = ui.interact(
            strip,
            Id::new(("clip-view-strip", clip)),
            Sense::click_and_drag(),
        );
        let keys = EditKeys::from(ui.input(|i| i.modifiers));
        if response.drag_started()
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
            && let (Some(drag), Some(pos)) = (self.strip_drag, pointer)
        {
            let (id, args) = drag.invocation(arrangement, doc, keys, grid, pos);
            gesture(events, GesturePhase::Update, id, args);
        }
        if self.strip_drag.is_some() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.strip_drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
        if response.drag_stopped() {
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
            && hit_strip(strip, grid, axis, length, midi, hover).is_some()
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
        top: f32,
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
                let semitones = ((key_at(grid, top, pos.y) - *grab_key).round() as i32)
                    .clamp(limits.1, limits.2);
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
