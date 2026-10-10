//! The state the views share: the project and the arrangement's viewport.
//!
//! In Beatlayer proper the project would be a `bl-model` snapshot edited
//! through commands; the spike keeps a plain struct.

use std::cell::Cell;

use gpui::{Context, EventEmitter};

use crate::data::{BARS, Project, TRACKS};

pub const TRACK_H: f32 = 44.0;
pub const MIN_PX_PER_BAR: f32 = 1.0;
pub const MAX_PX_PER_BAR: f32 = 800.0;

/// What changed, so each view re-renders only for what it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// Horizontal scroll or zoom.
    Horizontal,
    /// Vertical scroll.
    Vertical,
    /// Clip positions.
    Clips,
    /// Track names, mute, solo, or volume.
    Tracks,
}

pub struct Session {
    pub project: Project,
    /// The bar at the arrangement's left edge.
    pub scroll_x: f64,
    /// Pixels scrolled down from the first track.
    pub scroll_y: f32,
    pub px_per_bar: f32,
    /// The arrangement's visible lane area, set by the timeline as it lays
    /// out, so scrolling can be clamped.
    pub lanes_width: Cell<f32>,
    pub lanes_height: Cell<f32>,
}

impl EventEmitter<SessionEvent> for Session {}

impl Session {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            scroll_x: 0.0,
            scroll_y: 0.0,
            px_per_bar: 24.0,
            lanes_width: Cell::new(1100.0),
            lanes_height: Cell::new(900.0),
        }
    }

    pub fn visible_bars(&self) -> f64 {
        self.lanes_width.get() as f64 / self.px_per_bar as f64
    }

    pub fn max_scroll_x(&self) -> f64 {
        (BARS as f64 - self.visible_bars()).max(0.0)
    }

    pub fn max_scroll_y(&self) -> f32 {
        (TRACKS as f32 * TRACK_H - self.lanes_height.get()).max(0.0)
    }

    pub fn set_horizontal(&mut self, scroll_x: f64, px_per_bar: f32, cx: &mut Context<Self>) {
        let min_zoom = (self.lanes_width.get() / BARS as f32).max(MIN_PX_PER_BAR);
        self.px_per_bar = px_per_bar.clamp(min_zoom, MAX_PX_PER_BAR);
        self.scroll_x = scroll_x.clamp(0.0, self.max_scroll_x());
        cx.emit(SessionEvent::Horizontal);
    }

    pub fn set_scroll_y(&mut self, scroll_y: f32, cx: &mut Context<Self>) {
        let y = scroll_y.clamp(0.0, self.max_scroll_y());
        if y != self.scroll_y {
            self.scroll_y = y;
            cx.emit(SessionEvent::Vertical);
        }
    }

    /// Zooms by `factor`, keeping the bar under `anchor_x` (pixels from the
    /// lanes' left edge) in place.
    pub fn zoom_at(&mut self, factor: f32, anchor_x: f32, cx: &mut Context<Self>) {
        let anchor_bar = self.scroll_x + (anchor_x / self.px_per_bar) as f64;
        let new_zoom = self.px_per_bar * factor;
        let min_zoom = (self.lanes_width.get() / BARS as f32).max(MIN_PX_PER_BAR);
        let new_zoom = new_zoom.clamp(min_zoom, MAX_PX_PER_BAR);
        let scroll_x = anchor_bar - (anchor_x / new_zoom) as f64;
        self.set_horizontal(scroll_x, new_zoom, cx);
    }

    /// The track index under a y offset within the lanes area.
    pub fn track_at(&self, y: f32) -> usize {
        ((y + self.scroll_y) / TRACK_H).floor().max(0.0) as usize
    }

    /// The song position, in bars, at an x offset within the lanes area.
    pub fn bar_at(&self, x: f32) -> f64 {
        self.scroll_x + (x / self.px_per_bar) as f64
    }

    /// The range of tracks with any part visible.
    pub fn visible_tracks(&self) -> std::ops::Range<usize> {
        let first = (self.scroll_y / TRACK_H).floor() as usize;
        let last = ((self.scroll_y + self.lanes_height.get()) / TRACK_H).ceil() as usize;
        first.min(TRACKS)..last.min(TRACKS)
    }
}
