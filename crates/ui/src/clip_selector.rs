//! The Clip View Selector's outline (L-STAT-005; the piano roll spec, R36):
//! the Status Bar's selector, with a MIDI clip shown, draws the whole clip
//! the note editor shows and an outline round the part in view. Dragging
//! inside the outline sideways scrolls the note area and up or down zooms
//! it, and dragging the outline's left or right edge widens or narrows the
//! part in view. Each asks `clip_view.note_view` for the span, as the ruler
//! does, so the shell holds the view and none of it is an undo step.

use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, Note};
use bl_shell::clip_view::span;
use bl_shell::note_commands as ids;
use bl_shell::{Invocation, Shell};
use egui::{Pos2, Rect, Response, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How close to the outline's left or right edge, in points, a press grabs
/// the edge rather than the outline's inside.
const EDGE_GRAB: f32 = 3.0;
/// The most notes drawn in the picture of the clip.
const MOST_NOTES: usize = 2_000;

/// What a drag in the selector moves.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    /// The inside of the outline: sideways scrolls, up and down zooms.
    Inside,
    /// The outline's left edge.
    Left,
    /// The outline's right edge.
    Right,
}

/// A drag under way in the selector.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    clip: ClipId,
    part: Part,
    /// Where the press was.
    press: Pos2,
    /// The part in view when the drag began, as ticks of the clip's
    /// contents.
    window: (Ticks, Ticks),
}

/// The selector's drag.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipSelectorUi {
    drag: Option<Drag>,
}

/// The clip the selector pictures, with the span its whole picture covers
/// and the part in view: `None` unless the note editor shows one MIDI clip.
struct Picture<'a> {
    clip: ClipId,
    notes: &'a [Note],
    whole: (Ticks, Ticks),
    window: (Ticks, Ticks),
}

impl ClipSelectorUi {
    /// Draws the picture and outline in `rect`, over the selector's own
    /// face, and runs its drags.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        response: &Response,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let arrangement = shell.arrangement();
        let view = shell.clip_view();
        let shown = view.shown(doc, arrangement);
        let Some(clip) = view
            .shown_clip(doc, arrangement)
            .filter(|_| shown.len() <= 1)
        else {
            self.drag = None;
            return;
        };
        let Some(value) = doc.clip(clip) else {
            return;
        };
        let ClipData::Midi(midi) = &value.data else {
            return;
        };
        let notes: Vec<Note> = midi.notes.iter().take(MOST_NOTES).cloned().collect();
        let whole = span(midi, value.length);
        let (from, to) = view.note_span(value, midi);
        let picture = Picture {
            clip,
            notes: &notes,
            whole,
            window: (from, to),
        };
        let inner = rect.shrink2(vec2(2.0, 2.0));
        self.input(ui, &picture, inner, response, events);
        paint(ui, theme, &picture, inner);
    }

    fn input(
        &mut self,
        ui: &Ui,
        picture: &Picture<'_>,
        inner: Rect,
        response: &Response,
        events: &mut Vec<UiEvent>,
    ) {
        let outline = outline(picture, inner);
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && outline.expand2(vec2(EDGE_GRAB, 0.0)).contains(press)
        {
            let part = if (press.x - outline.left()).abs() <= EDGE_GRAB {
                Part::Left
            } else if (press.x - outline.right()).abs() <= EDGE_GRAB {
                Part::Right
            } else {
                Part::Inside
            };
            self.drag = Some(Drag {
                clip: picture.clip,
                part,
                press,
                window: picture.window,
            });
        }
        if response.dragged()
            && let (Some(drag), Some(pos)) = (self.drag, pointer)
            && drag.clip == picture.clip
        {
            let ticks = |points: f32| {
                let whole = (picture.whole.1 - picture.whole.0).get().max(1) as f64;
                (f64::from(points) / f64::from(inner.width().max(1.0)) * whole).round() as i64
            };
            let (from, to) = drag.window;
            let width = (to - from).get().max(1);
            let (start, end) = match drag.part {
                Part::Left => (from + Ticks(ticks(pos.x - drag.press.x)), to),
                Part::Right => (from, to + Ticks(ticks(pos.x - drag.press.x))),
                Part::Inside => {
                    // Up zooms out and down zooms in, about the tick pressed,
                    // as the ruler's drags do.
                    let dy = f64::from(pos.y - drag.press.y);
                    let wide = ((width as f64) * (-dy * 0.01).exp()).round().max(1.0) as i64;
                    let anchor = picture.whole.0.get() + ticks(drag.press.x - inner.left());
                    let fraction = (anchor - from.get()) as f64 / width as f64;
                    let start =
                        anchor as f64 - fraction * wide as f64 + ticks(pos.x - drag.press.x) as f64;
                    (
                        Ticks(start.round() as i64),
                        Ticks(start.round() as i64 + wide),
                    )
                }
            };
            if end > start {
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::NOTE_VIEW,
                    json!({
                        "clip": picture.clip,
                        "show": "scroll",
                        "start": start,
                        "end": end,
                    }),
                )));
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        if let Some(hover) = response.hover_pos()
            && outline.expand2(vec2(EDGE_GRAB, 0.0)).contains(hover)
        {
            let edge = (hover.x - outline.left()).abs() <= EDGE_GRAB
                || (hover.x - outline.right()).abs() <= EDGE_GRAB;
            ui.ctx().set_cursor_icon(if edge {
                egui::CursorIcon::ResizeHorizontal
            } else {
                egui::CursorIcon::Grab
            });
        }
    }
}

/// The rectangle of the part in view, inside the picture `inner`.
fn outline(picture: &Picture<'_>, inner: Rect) -> Rect {
    let x_of = |tick: Ticks| x_in(picture.whole, inner, tick);
    Rect::from_min_max(
        pos2(x_of(picture.window.0), inner.top()),
        pos2(
            x_of(picture.window.1).max(x_of(picture.window.0) + 1.0),
            inner.bottom(),
        ),
    )
}

fn x_in(whole: (Ticks, Ticks), inner: Rect, tick: Ticks) -> f32 {
    let length = (whole.1 - whole.0).get().max(1) as f64;
    inner.left() + ((tick - whole.0).get() as f64 / length) as f32 * inner.width()
}

/// Draws the notes across `inner`, and the outline round the part in view.
fn paint(ui: &Ui, theme: &Theme, picture: &Picture<'_>, inner: Rect) {
    let painter = ui.painter_at(inner.expand(1.0));
    let colours = &theme.colors;
    let low = picture.notes.iter().map(|note| note.key).min().unwrap_or(0);
    let high = picture.notes.iter().map(|note| note.key).max().unwrap_or(0);
    let rows = f32::from(high - low) + 1.0;
    for note in picture.notes {
        let y = inner.bottom() - (f32::from(note.key - low) + 1.0) / rows * inner.height();
        let x0 = x_in(picture.whole, inner, note.start);
        let x1 = x_in(picture.whole, inner, note.start + note.length).max(x0 + 1.0);
        painter.rect_filled(
            Rect::from_min_max(pos2(x0, y), pos2(x1, y + 1.0)),
            0.0,
            colours.text_dim.0,
        );
    }
    painter.rect_stroke(
        outline(picture, inner),
        1.0,
        Stroke::new(1.0, colours.time_selection.0),
        StrokeKind::Inside,
    );
}
