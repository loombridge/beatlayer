//! The arrangement: bar ruler, track lanes, clips, and waveforms, drawn in
//! one `canvas` with GPUI's quad and text primitives.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    App, Bounds, ContentMask, Context, Entity, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, Role,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Subscription, TextAlign,
    TextRun, Window, canvas, div, fill, point, px, rgb, rgba, size,
};

use crate::data::{BARS, Peaks, SAMPLES_PER_BAR};
use crate::session::{Session, SessionEvent, TRACK_H};

pub const RULER_H: f32 = 28.0;
const CLIP_HEADER_H: f32 = 14.0;

#[derive(Clone, Copy, Debug)]
struct ClipDrag {
    clip: usize,
    /// Where in the clip, in bars, the pointer grabbed it.
    grab: f64,
    track: usize,
    start: f64,
}

/// Counts from the last paint, for the bench report.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaintStats {
    pub quads: usize,
    pub labels: usize,
    pub clips: usize,
}

pub struct Timeline {
    session: Entity<Session>,
    /// The lane area's bounds in window coordinates, from the last prepaint.
    lanes: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<ClipDrag>,
    pub stats: Rc<Cell<PaintStats>>,
    _subscription: Subscription,
}

impl Timeline {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event, cx| match event {
            SessionEvent::Horizontal | SessionEvent::Vertical | SessionEvent::Clips => cx.notify(),
            SessionEvent::Tracks => {}
        });
        Self {
            session,
            lanes: Rc::default(),
            drag: None,
            stats: Rc::default(),
            _subscription: subscription,
        }
    }

    /// The lane area's bounds in window coordinates, from the last layout.
    pub fn lanes(&self) -> Bounds<Pixels> {
        self.lanes.get()
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(20.0));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        let anchor = f32::from(event.position.x - self.lanes.get().origin.x);
        self.session.update(cx, |s, cx| {
            if event.modifiers.platform || event.modifiers.control {
                s.zoom_at((dy * 0.01).exp(), anchor, cx);
            } else {
                if dx != 0.0 {
                    let x = s.scroll_x - (dx / s.px_per_bar) as f64;
                    s.set_horizontal(x, s.px_per_bar, cx);
                }
                if dy != 0.0 {
                    s.set_scroll_y(s.scroll_y - dy, cx);
                }
            }
        });
        cx.stop_propagation();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lanes = self.lanes.get();
        if !lanes.contains(&event.position) {
            return;
        }
        let local = event.position - lanes.origin;
        let s = self.session.read(cx);
        let track = s.track_at(f32::from(local.y));
        let bar = s.bar_at(f32::from(local.x));
        if let Some(clip) = s.project.hit_test(track, bar) {
            let c = &s.project.clips[clip];
            self.drag = Some(ClipDrag {
                clip,
                grab: bar - c.start,
                track: c.track,
                start: c.start,
            });
            cx.notify();
        }
    }

    fn on_drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(mut drag) = self.drag else { return };
        let local = event.position - self.lanes.get().origin;
        let s = self.session.read(cx);
        let bars = s.project.clips[drag.clip].bars as f64;
        // Snap to a sixteenth of a bar.
        let start = ((s.bar_at(f32::from(local.x)) - drag.grab) * 16.0).round() / 16.0;
        drag.start = start.clamp(0.0, BARS as f64 - bars);
        drag.track = s
            .track_at(f32::from(local.y))
            .min(s.project.tracks.len() - 1);
        self.drag = Some(drag);
        cx.notify();
    }

    fn on_drag_end(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        self.session.update(cx, |s, cx| {
            if s.project.move_clip(drag.clip, drag.track, drag.start) {
                cx.emit(SessionEvent::Clips);
            }
        });
        cx.notify();
    }
}

impl Render for Timeline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.clone();
        let lanes_cell = self.lanes.clone();
        let stats = self.stats.clone();
        let drag = self.drag;
        let view = cx.entity().downgrade();
        div()
            .id("arrangement")
            .role(Role::Group)
            .aria_label("Arrangement")
            .size_full()
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let lanes = lanes_bounds(bounds);
                        lanes_cell.set(lanes);
                        let s = session.read(cx);
                        s.lanes_width.set(f32::from(lanes.size.width));
                        s.lanes_height.set(f32::from(lanes.size.height));
                        session
                    },
                    move |bounds, session, window, cx| {
                        if drag.is_some() {
                            let move_view = view.clone();
                            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                                if phase.bubble() {
                                    move_view
                                        .update(cx, |this, cx| this.on_drag_move(e, cx))
                                        .ok();
                                }
                            });
                            let up_view = view.clone();
                            window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                                if phase.bubble() && e.button == MouseButton::Left {
                                    up_view.update(cx, |this, cx| this.on_drag_end(cx)).ok();
                                }
                            });
                        }
                        stats.set(paint_timeline(&session, drag, bounds, window, cx));
                    },
                )
                .size_full(),
            )
    }
}

fn lanes_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(bounds.origin.x, bounds.origin.y + px(RULER_H)),
        size(bounds.size.width, bounds.size.height - px(RULER_H)),
    )
}

struct Label {
    text: SharedString,
    origin: gpui::Point<Pixels>,
    mask: Bounds<Pixels>,
    color: Hsla,
}

fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

fn with_alpha(c: u32, a: f32) -> Hsla {
    let mut h: Hsla = rgb(c).into();
    h.a = a;
    h
}

fn paint_timeline(
    session: &Entity<Session>,
    drag: Option<ClipDrag>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> PaintStats {
    let scale = window.scale_factor();
    let ruler = Bounds::new(bounds.origin, size(bounds.size.width, px(RULER_H)));
    let lanes = lanes_bounds(bounds);
    let mut labels: Vec<Label> = Vec::new();
    let mut stats = PaintStats::default();

    {
        let s = session.read(cx);
        let ppb = s.px_per_bar;
        let left = f32::from(lanes.origin.x);
        let top = f32::from(lanes.origin.y);
        let width = f32::from(lanes.size.width);
        let first_bar = s.scroll_x;
        let last_bar = s.scroll_x + (width / ppb) as f64;
        let x_of = |bar: f64| left + ((bar - s.scroll_x) as f32) * ppb;
        let tracks = s.visible_tracks();

        window.with_content_mask(Some(ContentMask { bounds: lanes }), |window| {
            // Lane backgrounds.
            window.paint_quad(fill(lanes, hex(0x1B1D21)));
            stats.quads += 1;
            for t in tracks.clone() {
                let y = top + t as f32 * TRACK_H - s.scroll_y;
                let bg = if t % 2 == 0 { 0x202328 } else { 0x1D2024 };
                window.paint_quad(fill(
                    Bounds::new(
                        point(lanes.origin.x, px(y)),
                        size(lanes.size.width, px(TRACK_H)),
                    ),
                    hex(bg),
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        point(lanes.origin.x, px(y + TRACK_H - 1.0)),
                        size(lanes.size.width, px(1.0)),
                    ),
                    hex(0x15171A),
                ));
                stats.quads += 2;
            }

            // Grid: bar lines at least 8 px apart, plus beats when there's room.
            let step = grid_step(ppb, 8.0);
            let mut bar = (first_bar / step as f64).floor() * step as f64;
            while bar <= last_bar {
                let x = x_of(bar);
                let strong = (bar as u32).is_multiple_of(step * 4);
                window.paint_quad(fill(
                    Bounds::new(
                        point(px(x), lanes.origin.y),
                        size(px(1.0), lanes.size.height),
                    ),
                    if strong { hex(0x30343B) } else { hex(0x282B31) },
                ));
                stats.quads += 1;
                bar += step as f64;
            }

            // Clips.
            for t in tracks.clone() {
                for c in s.project.clips_in(t, first_bar..last_bar) {
                    if drag.is_some_and(|d| d.clip == c) {
                        continue;
                    }
                    let clip = &s.project.clips[c];
                    paint_clip(
                        window,
                        s,
                        c,
                        t,
                        clip.start,
                        false,
                        lanes,
                        scale,
                        &mut labels,
                        &mut stats,
                    );
                }
            }
            if let Some(d) = drag {
                paint_clip(
                    window,
                    s,
                    d.clip,
                    d.track,
                    d.start,
                    true,
                    lanes,
                    scale,
                    &mut labels,
                    &mut stats,
                );
            }
        });

        // Ruler.
        window.with_content_mask(Some(ContentMask { bounds: ruler }), |window| {
            window.paint_quad(fill(ruler, hex(0x2A2D33)));
            window.paint_quad(fill(
                Bounds::new(
                    point(ruler.origin.x, ruler.origin.y + px(RULER_H - 1.0)),
                    size(ruler.size.width, px(1.0)),
                ),
                hex(0x121417),
            ));
            stats.quads += 2;
            let tick = grid_step(ppb, 8.0);
            let label = grid_step(ppb, 48.0);
            let mut bar = (first_bar / tick as f64).floor() * tick as f64;
            while bar <= last_bar {
                let x = x_of(bar);
                let is_label = (bar as u32).is_multiple_of(label);
                let h = if is_label { 10.0 } else { 5.0 };
                window.paint_quad(fill(
                    Bounds::new(
                        point(px(x), ruler.origin.y + px(RULER_H - h)),
                        size(px(1.0), px(h)),
                    ),
                    hex(0x8A9099),
                ));
                stats.quads += 1;
                if is_label {
                    labels.push(Label {
                        text: SharedString::from(format!("{}", bar as u32 + 1)),
                        origin: point(px(x + 3.0), ruler.origin.y + px(3.0)),
                        mask: ruler,
                        color: hex(0xC8CDD4),
                    });
                }
                bar += tick as f64;
            }
        });
    }

    // Text needs `cx` mutably, so it's painted after the session borrow ends.
    let font = window.text_style().font();
    stats.labels = labels.len();
    for label in labels {
        let run = TextRun {
            len: label.text.len(),
            font: font.clone(),
            color: label.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(label.text, px(11.0), &[run], None);
        window.with_content_mask(Some(ContentMask { bounds: label.mask }), |window| {
            line.paint(label.origin, px(13.0), TextAlign::Left, None, window, cx)
                .ok();
        });
    }
    stats
}

/// The smallest power-of-two number of bars whose width is at least `min_px`.
fn grid_step(px_per_bar: f32, min_px: f32) -> u32 {
    let mut step = 1u32;
    while (step as f32) * px_per_bar < min_px && step < BARS {
        step *= 2;
    }
    step
}

#[allow(clippy::too_many_arguments)]
fn paint_clip(
    window: &mut Window,
    s: &Session,
    c: usize,
    track: usize,
    start: f64,
    dragging: bool,
    lanes: Bounds<Pixels>,
    scale: f32,
    labels: &mut Vec<Label>,
    stats: &mut PaintStats,
) {
    let clip = &s.project.clips[c];
    let color = s.project.tracks[clip.track].color;
    let ppb = s.px_per_bar;
    let left = f32::from(lanes.origin.x);
    let right = left + f32::from(lanes.size.width);
    let x0 = left + ((start - s.scroll_x) as f32) * ppb;
    let x1 = x0 + clip.bars as f32 * ppb;
    let y0 = f32::from(lanes.origin.y) + track as f32 * TRACK_H - s.scroll_y + 1.0;
    let h = TRACK_H - 3.0;
    let w = x1 - x0;
    stats.clips += 1;

    let body = Bounds::new(point(px(x0), px(y0)), size(px(w), px(h)));
    window.paint_quad(fill(body, with_alpha(color, 0.28)).corner_radii(px(2.0)));
    let header = Bounds::new(point(px(x0), px(y0)), size(px(w), px(CLIP_HEADER_H)));
    window.paint_quad(fill(header, hex(color)).corner_radii(px(2.0)));
    stats.quads += 2;
    if dragging {
        window.paint_quad(
            fill(body, rgba(0x00000000))
                .border_widths(px(1.0))
                .border_color(hex(0xFFFFFF)),
        );
        stats.quads += 1;
    }
    if w > 24.0 {
        labels.push(Label {
            text: SharedString::from(clip.name.clone()),
            origin: point(px(x0.max(left) + 3.0), px(y0)),
            mask: header.intersect(&lanes),
            color: hex(0x101114),
        });
    }

    // Waveform: one quad per device-pixel column, from the coarsest peak
    // level that still has a peak per column.
    let peaks = &s.project.peaks[c];
    let samples_per_px = SAMPLES_PER_BAR as f64 / ppb as f64;
    let level = Peaks::level_for(samples_per_px / scale as f64);
    let wave_top = y0 + CLIP_HEADER_H + 1.0;
    let wave_h = h - CLIP_HEADER_H - 2.0;
    let mid = wave_top + wave_h / 2.0;
    let half = wave_h / 2.0;
    let wave_color = with_alpha(0xE8ECF1, 0.85);
    let col = 1.0 / scale;
    let d0 = (x0.max(left) * scale).floor() as i64;
    let d1 = (x1.min(right) * scale).ceil() as i64;
    let mut columns = |window: &mut Window| {
        for d in d0..d1 {
            let x = d as f32 / scale;
            let s0 = (((x - x0) as f64) * samples_per_px).max(0.0) as u64;
            let s1 = (((x + col - x0) as f64) * samples_per_px).max(0.0) as u64;
            let Some((lo, hi)) = peaks.min_max(level, s0..s1.max(s0 + 1)) else {
                continue;
            };
            let top = mid - hi as f32 / i16::MAX as f32 * half;
            let bottom = (mid - lo as f32 / i16::MAX as f32 * half).max(top + col);
            window.paint_quad(fill(
                Bounds::new(point(px(x), px(top)), size(px(col), px(bottom - top))),
                wave_color,
            ));
            stats.quads += 1;
        }
    };
    if USE_LAYERS.load(Ordering::Relaxed) {
        // The columns never overlap, so they can share one draw order. Without
        // a layer GPUI inserts every quad into its bounds tree to order it,
        // which dominated the frame (see docs/spikes/ui-gpui.md).
        let wave = Bounds::new(point(px(x0), px(wave_top)), size(px(x1 - x0), px(wave_h)));
        window.paint_layer(wave, columns);
    } else {
        columns(window);
    }
}

/// Whether each clip's waveform is painted inside a `paint_layer`. Off with
/// `--no-layers`, to measure the difference.
pub static USE_LAYERS: AtomicBool = AtomicBool::new(true);

#[cfg(test)]
mod tests {
    use super::grid_step;

    #[test]
    fn grid_step_keeps_lines_apart() {
        assert_eq!(grid_step(24.0, 8.0), 1);
        assert_eq!(grid_step(3.0, 8.0), 4);
        assert_eq!(grid_step(2.0, 48.0), 32);
    }
}
