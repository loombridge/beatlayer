//! The benchmark scene's UI: toolbar, browser, track headers, ruler, and the
//! arrangement timeline.
//!
//! Standard widgets are egui's. The dense parts (track lanes, grid, clips,
//! waveforms, meters) are collected into [`DenseList`]s and drawn by
//! [`paint_cpu`] or [`paint_gpu`], whichever [`DenseMode`] is selected.

use egui::accesskit;
use egui::{
    Align, Align2, Button, CentralPanel, Color32, CursorIcon, FontId, Key, Label, Layout, Panel,
    Rect, Response, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, UiBuilder, Vec2, WidgetInfo,
    WidgetType, pos2, vec2,
};

use crate::bench::{Phase, Script};
use crate::dense::{DenseList, pick_level};
use crate::paint_cpu;
use crate::paint_gpu::{DenseCallback, Slot};
use crate::scene::{BEATS_PER_BAR, Scene, meter_level};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenseMode {
    /// egui's own painting: an `epaint::Mesh` rebuilt every frame.
    Painter,
    /// A wgpu paint callback with instanced rectangles and a waveform shader.
    Gpu,
}

pub const HEADER_WIDTH: f32 = 230.0;
pub const RULER_HEIGHT: f32 = 22.0;
pub const TRACK_HEIGHT: f32 = 40.0;
pub const BROWSER_WIDTH: f32 = 260.0;
const MAX_POINTS_PER_BEAT: f64 = 400.0;

#[derive(Clone, Debug)]
pub struct View {
    /// The first visible beat.
    pub scroll_beats: f64,
    pub points_per_beat: f64,
    pub scroll_y: f32,
    pub track_height: f32,
}

#[derive(Clone, Copy, Debug)]
struct ClipDrag {
    clip: u32,
    from_track: usize,
    /// Where the pointer grabbed the clip, in beats from its start.
    grab_beats: f64,
    track: usize,
    start_beat: u32,
}

#[derive(Clone, Debug)]
struct Rename {
    track: usize,
    text: String,
    focus_requested: bool,
}

/// What the window host should do with the pointer after this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerLock {
    Lock,
    Unlock,
}

pub struct App {
    pub scene: Scene,
    pub view: View,
    pub mode: DenseMode,
    pub meters_running: bool,
    meter_clock: f64,
    meter_offset: f64,
    clip_drag: Option<ClipDrag>,
    rename: Option<Rename>,
    selected_file: Option<usize>,
    /// Set by the knob; the host applies it to the window and clears it.
    pub pointer_lock_request: Option<PointerLock>,
    pub pointer_lock_status: String,
    pub plugin_window_requested: bool,
    pub plugin_window_open: bool,
    pub plugin_window_status: String,
    pub bench: Option<Script>,
    pub bench_phase: Phase,
    /// The track-name labels drawn last frame, for tests and tooling.
    pub name_rects: Vec<(usize, Rect, egui::Id)>,
    frame_ms: f32,
    timeline_list: DenseList,
    meter_list: DenseList,
    clip_names: Vec<(Rect, u32)>,
}

impl App {
    pub fn new(scene: Scene, mode: DenseMode) -> Self {
        Self {
            scene,
            view: View {
                scroll_beats: 0.0,
                points_per_beat: 1100.0 / 128.0,
                scroll_y: 0.0,
                track_height: TRACK_HEIGHT,
            },
            mode,
            meters_running: true,
            meter_clock: 0.0,
            meter_offset: 0.0,
            clip_drag: None,
            rename: None,
            selected_file: None,
            pointer_lock_request: None,
            pointer_lock_status: String::new(),
            plugin_window_requested: false,
            plugin_window_open: false,
            plugin_window_status: String::new(),
            bench: None,
            bench_phase: Phase::Off,
            name_rects: Vec::new(),
            frame_ms: 0.0,
            timeline_list: DenseList::default(),
            meter_list: DenseList::default(),
            clip_names: Vec::new(),
        }
    }

    /// Starts renaming a track, as a double-click on its name does.
    pub fn start_rename(&mut self, track: usize) {
        self.rename = Some(Rename {
            track,
            text: self.scene.tracks[track].name.clone(),
            focus_requested: false,
        });
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let (now, dt) = ui.input(|i| (i.time, i.unstable_dt));
        self.frame_ms = if self.frame_ms == 0.0 {
            dt * 1000.0
        } else {
            0.95 * self.frame_ms + 0.05 * dt * 1000.0
        };
        if let Some(script) = &mut self.bench {
            self.bench_phase = script.phase(now);
            ui.ctx().request_repaint();
        }
        if self.meters_running {
            self.meter_clock = now - self.meter_offset;
            ui.ctx().request_repaint();
        }

        self.toolbar(ui, now);
        self.browser(ui);
        CentralPanel::no_frame().show(ui, |ui| self.arrangement(ui));
    }

    fn toolbar(&mut self, ui: &mut Ui, now: f64) {
        Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Beatlayer UI spike: egui 0.36");
                ui.separator();
                ui.label("Dense views:");
                ui.selectable_value(&mut self.mode, DenseMode::Gpu, "wgpu callback");
                ui.selectable_value(&mut self.mode, DenseMode::Painter, "egui mesh");
                ui.separator();
                let label = if self.meters_running {
                    "Stop meters"
                } else {
                    "Start meters"
                };
                if ui.button(label).clicked() {
                    self.meters_running = !self.meters_running;
                    self.meter_offset = now - self.meter_clock;
                }
                let open =
                    ui.add_enabled(!self.plugin_window_open, Button::new("Open plug-in window"));
                if open.clicked() {
                    self.plugin_window_requested = true;
                }
                ui.separator();
                let visible_bars =
                    f64::from(self.timeline_width()) / self.view.points_per_beat / 4.0;
                ui.label(format!("{visible_bars:.1} bars"));
                ui.label(format!("{:.1} ms/frame", self.frame_ms));
                if !self.pointer_lock_status.is_empty() {
                    ui.separator();
                    ui.label(&self.pointer_lock_status);
                }
                if !self.plugin_window_status.is_empty() {
                    ui.separator();
                    ui.label(&self.plugin_window_status);
                }
                match self.bench_phase {
                    Phase::Off => {}
                    Phase::Warmup => {
                        ui.label("bench: warming up");
                    }
                    Phase::Running(t) => {
                        ui.label(format!("bench: {t:.1} s"));
                    }
                    Phase::Done => {
                        ui.label("bench: done");
                    }
                }
            });
        });
    }

    fn browser(&mut self, ui: &mut Ui) {
        Panel::right("browser")
            .resizable(false)
            .exact_size(BROWSER_WIDTH)
            .show(ui, |ui| {
                ui.heading("Browser");
                ui.label(format!("{} files", self.scene.files.len()));
                ui.separator();
                let row_height = ui.text_style_height(&egui::TextStyle::Body)
                    + 2.0 * ui.spacing().button_padding.y;
                ScrollArea::vertical().auto_shrink(false).show_rows(
                    ui,
                    row_height,
                    self.scene.files.len(),
                    |ui, rows| {
                        for i in rows {
                            let selected = self.selected_file == Some(i);
                            if ui
                                .selectable_label(selected, &self.scene.files[i])
                                .clicked()
                            {
                                self.selected_file = Some(i);
                            }
                        }
                    },
                );
            });
    }

    fn timeline_width(&self) -> f32 {
        1600.0 - HEADER_WIDTH - BROWSER_WIDTH
    }

    fn arrangement(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        let ruler = Rect::from_min_max(
            pos2(full.left() + HEADER_WIDTH, full.top()),
            pos2(full.right(), full.top() + RULER_HEIGHT),
        );
        let headers = Rect::from_min_max(
            pos2(full.left(), full.top() + RULER_HEIGHT),
            pos2(full.left() + HEADER_WIDTH, full.bottom()),
        );
        let timeline = Rect::from_min_max(
            pos2(full.left() + HEADER_WIDTH, full.top() + RULER_HEIGHT),
            full.right_bottom(),
        );

        self.drive_bench(timeline);
        if self.bench.is_none() {
            self.scroll_and_zoom(ui, full, timeline);
        }
        self.clamp_view(timeline);

        let response = ui.interact(timeline, ui.id().with("timeline"), Sense::click_and_drag());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Arrangement"));
        self.drag_clips(ui, &response, timeline);

        self.draw_timeline(ui, timeline, ppp);
        self.draw_ruler(ui, ruler, ppp);
        self.draw_headers(ui, headers, ppp);
    }

    fn drive_bench(&mut self, timeline: Rect) {
        let Some(script) = &self.bench else {
            return;
        };
        let total = f64::from(self.scene.config.total_beats());
        let t = match self.bench_phase {
            Phase::Running(t) => t,
            Phase::Done => script.duration,
            _ => 0.0,
        };
        let (left, ppb) = script.view_at(t, total, f64::from(timeline.width()));
        self.view.scroll_beats = left;
        self.view.points_per_beat = ppb;
        if script.all_tracks {
            self.view.track_height = timeline.height() / self.scene.tracks.len() as f32;
            self.view.scroll_y = 0.0;
        }
    }

    fn scroll_and_zoom(&mut self, ui: &Ui, area: Rect, timeline: Rect) {
        let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        if !area.contains(pointer) {
            return;
        }
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if zoom != 1.0 {
            let anchor_x = f64::from((pointer.x - timeline.left()).max(0.0));
            let anchor_beat = self.view.scroll_beats + anchor_x / self.view.points_per_beat;
            self.view.points_per_beat *= f64::from(zoom);
            self.clamp_zoom(timeline);
            self.view.scroll_beats = anchor_beat - anchor_x / self.view.points_per_beat;
        }
        self.view.scroll_beats -= f64::from(scroll.x) / self.view.points_per_beat;
        self.view.scroll_y -= scroll.y;
    }

    fn clamp_zoom(&mut self, timeline: Rect) {
        let total = f64::from(self.scene.config.total_beats());
        let min = f64::from(timeline.width()) / total;
        self.view.points_per_beat = self.view.points_per_beat.clamp(min, MAX_POINTS_PER_BEAT);
    }

    fn clamp_view(&mut self, timeline: Rect) {
        self.clamp_zoom(timeline);
        let total = f64::from(self.scene.config.total_beats());
        let visible = f64::from(timeline.width()) / self.view.points_per_beat;
        self.view.scroll_beats = self
            .view
            .scroll_beats
            .clamp(0.0, (total - visible).max(0.0));
        let content = self.view.track_height * self.scene.tracks.len() as f32;
        self.view.scroll_y = self
            .view
            .scroll_y
            .clamp(0.0, (content - timeline.height()).max(0.0));
    }

    fn beat_to_x(&self, timeline: Rect, beat: f64) -> f32 {
        timeline.left() + ((beat - self.view.scroll_beats) * self.view.points_per_beat) as f32
    }

    fn x_to_beat(&self, timeline: Rect, x: f32) -> f64 {
        self.view.scroll_beats + f64::from(x - timeline.left()) / self.view.points_per_beat
    }

    fn row_top(&self, timeline: Rect, track: usize) -> f32 {
        timeline.top() + track as f32 * self.view.track_height - self.view.scroll_y
    }

    fn visible_tracks(&self, timeline: Rect) -> std::ops::Range<usize> {
        let h = self.view.track_height;
        let first = (self.view.scroll_y / h).floor().max(0.0) as usize;
        let last = ((self.view.scroll_y + timeline.height()) / h).ceil() as usize;
        first.min(self.scene.tracks.len())..last.min(self.scene.tracks.len())
    }

    fn track_at(&self, timeline: Rect, y: f32) -> usize {
        let t = ((y - timeline.top() + self.view.scroll_y) / self.view.track_height).floor();
        (t.max(0.0) as usize).min(self.scene.tracks.len() - 1)
    }

    /// The top-most clip under a point, as (track, clip id).
    fn clip_at(&self, timeline: Rect, pos: egui::Pos2) -> Option<(usize, u32)> {
        if !timeline.contains(pos) {
            return None;
        }
        let track = self.track_at(timeline, pos.y);
        let beat = self.x_to_beat(timeline, pos.x);
        let clips = &self.scene.tracks[track].clips;
        let end =
            clips.partition_point(|&c| f64::from(self.scene.clips[c as usize].start_beat) <= beat);
        clips[..end]
            .iter()
            .rev()
            .find(|&&c| f64::from(self.scene.clips[c as usize].end_beat()) > beat)
            .map(|&c| (track, c))
    }

    fn drag_clips(&mut self, ui: &Ui, response: &Response, timeline: Rect) {
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            && let Some((track, clip)) = self.clip_at(timeline, origin)
        {
            let start = self.scene.clips[clip as usize].start_beat;
            self.clip_drag = Some(ClipDrag {
                clip,
                from_track: track,
                grab_beats: self.x_to_beat(timeline, origin.x) - f64::from(start),
                track,
                start_beat: start,
            });
        }

        if let Some(drag) = self.clip_drag
            && let Some(pos) = response.interact_pointer_pos()
        {
            let clip = &self.scene.clips[drag.clip as usize];
            let max_start = self.scene.config.total_beats() - clip.len_beats;
            let start = (self.x_to_beat(timeline, pos.x) - drag.grab_beats).round();
            let start = start.clamp(0.0, f64::from(max_start)) as u32;
            let track = self.track_at(timeline, pos.y);
            self.clip_drag = Some(ClipDrag {
                start_beat: start,
                track,
                ..drag
            });
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if response.hovered()
            && let Some(pos) = response.hover_pos()
            && self.clip_at(timeline, pos).is_some()
        {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }

        if response.drag_stopped()
            && let Some(drag) = self.clip_drag.take()
        {
            self.scene
                .move_clip(drag.clip, drag.from_track, drag.track, drag.start_beat);
        }
    }

    fn draw_timeline(&mut self, ui: &mut Ui, timeline: Rect, ppp: f32) {
        let mut list = std::mem::take(&mut self.timeline_list);
        list.clear();
        let mut names = std::mem::take(&mut self.clip_names);
        names.clear();
        let tracks = self.visible_tracks(timeline);
        let px = 1.0 / ppp;

        // Lanes.
        for t in tracks.clone() {
            let top = self.row_top(timeline, t);
            let row = Rect::from_min_max(
                pos2(timeline.left(), top),
                pos2(timeline.right(), top + self.view.track_height),
            );
            let shade = if t % 2 == 0 { 34 } else { 38 };
            list.rect(row.intersect(timeline), Color32::from_gray(shade));
        }

        // Grid: bar lines thinned out so they stay at least 12 points apart,
        // plus beat lines when there's room.
        let bar_points = self.view.points_per_beat * f64::from(BEATS_PER_BAR);
        let mut step = 1u32;
        while f64::from(step) * bar_points < 12.0 {
            step *= 2;
        }
        let first_bar = (self.view.scroll_beats / f64::from(BEATS_PER_BAR)).floor() as u32;
        let first_bar = first_bar - first_bar % step;
        let mut bar = first_bar;
        while bar <= self.scene.config.bars {
            let x = self.beat_to_x(timeline, f64::from(bar * BEATS_PER_BAR));
            if x > timeline.right() {
                break;
            }
            let x = (x * ppp).round() / ppp;
            list.rect(
                Rect::from_min_max(pos2(x, timeline.top()), pos2(x + px, timeline.bottom())),
                Color32::from_gray(58),
            );
            if bar_points >= 64.0 {
                for beat in 1..BEATS_PER_BAR {
                    let xb = self.beat_to_x(timeline, f64::from(bar * BEATS_PER_BAR + beat));
                    let xb = (xb * ppp).round() / ppp;
                    list.rect(
                        Rect::from_min_max(
                            pos2(xb, timeline.top()),
                            pos2(xb + px, timeline.bottom()),
                        ),
                        Color32::from_gray(46),
                    );
                }
            }
            bar += step;
        }

        // Clips.
        let view_start = self.view.scroll_beats;
        let view_end = view_start + f64::from(timeline.width()) / self.view.points_per_beat;
        let look_back = f64::from(self.scene.max_clip_beats());
        let dragged = self.clip_drag.map(|d| d.clip);
        for t in tracks {
            let clips = &self.scene.tracks[t].clips;
            let first = clips.partition_point(|&c| {
                f64::from(self.scene.clips[c as usize].start_beat) + look_back < view_start
            });
            for &c in &clips[first..] {
                let clip = &self.scene.clips[c as usize];
                if f64::from(clip.start_beat) >= view_end {
                    break;
                }
                if f64::from(clip.end_beat()) <= view_start || Some(c) == dragged {
                    continue;
                }
                self.push_clip(&mut list, &mut names, timeline, ppp, c, t, clip.start_beat);
            }
        }
        if let Some(drag) = self.clip_drag {
            list.begin_top_layer();
            self.push_clip(
                &mut list,
                &mut names,
                timeline,
                ppp,
                drag.clip,
                drag.track,
                drag.start_beat,
            );
        }

        let painter = ui.painter_at(timeline);
        match self.mode {
            DenseMode::Gpu => {
                painter.add(egui_wgpu::Callback::new_paint_callback(
                    timeline,
                    DenseCallback {
                        slot: Slot::Timeline,
                        list,
                    },
                ));
            }
            DenseMode::Painter => {
                paint_cpu::paint(&painter, &list, &self.scene, ppp);
                self.timeline_list = list;
            }
        }

        let font = FontId::proportional(10.5);
        for &(rect, c) in &names {
            painter.with_clip_rect(rect).text(
                rect.left_top() + vec2(3.0, 0.5),
                Align2::LEFT_TOP,
                &self.scene.clips[c as usize].name,
                font.clone(),
                Color32::from_black_alpha(220),
            );
        }
        self.clip_names = names;
    }

    #[expect(clippy::too_many_arguments)]
    fn push_clip(
        &self,
        list: &mut DenseList,
        names: &mut Vec<(Rect, u32)>,
        timeline: Rect,
        ppp: f32,
        id: u32,
        track: usize,
        start_beat: u32,
    ) {
        let clip = &self.scene.clips[id as usize];
        let h = self.view.track_height;
        let gap = (h * 0.06).min(1.0);
        let top = self.row_top(timeline, track) + gap;
        let bottom = top + h - 2.0 * gap;
        let x0 = self.beat_to_x(timeline, f64::from(start_beat));
        let x1 = self
            .beat_to_x(timeline, f64::from(start_beat + clip.len_beats))
            .max(x0 + 1.0 / ppp);
        let body = Rect::from_min_max(pos2(x0, top), pos2(x1, bottom));
        let visible = body.intersect(timeline);
        if !visible.is_positive() {
            return;
        }
        let [r, g, b] = clip.color;
        let colour = Color32::from_rgb(r, g, b);
        let dark = Color32::from_rgb(
            (f32::from(r) * 0.3) as u8,
            (f32::from(g) * 0.3) as u8,
            (f32::from(b) * 0.3) as u8,
        );
        list.rect(visible, dark);

        let header = if h >= 30.0 { 13.0 } else { 0.0 };
        if header > 0.0 {
            let strip = Rect::from_min_max(body.min, pos2(x1, top + header)).intersect(timeline);
            list.rect(strip, colour);
            if strip.width() > 24.0 {
                names.push((strip, id));
            }
        }
        let wave_area = Rect::from_min_max(pos2(x0, top + header), body.max);
        if wave_area.height() * ppp >= 2.0 {
            let level = pick_level(&clip.levels, (x1 - x0) * ppp);
            list.wave(wave_area, timeline, clip.levels[level], colour);
        }
    }

    fn draw_ruler(&self, ui: &Ui, ruler: Rect, ppp: f32) {
        let painter = ui.painter_at(ruler);
        painter.rect_filled(ruler, 0.0, Color32::from_gray(28));
        let bar_points = self.view.points_per_beat * f64::from(BEATS_PER_BAR);
        let mut step = 1u32;
        while f64::from(step) * bar_points < 44.0 {
            step *= 2;
        }
        let timeline_left = ruler.left();
        let first_bar = (self.view.scroll_beats / f64::from(BEATS_PER_BAR)).floor() as u32;
        let mut bar = first_bar - first_bar % step;
        let stroke = Stroke::new(1.0 / ppp, Color32::from_gray(140));
        let font = FontId::proportional(11.0);
        while bar <= self.scene.config.bars {
            let x = timeline_left
                + ((f64::from(bar * BEATS_PER_BAR) - self.view.scroll_beats)
                    * self.view.points_per_beat) as f32;
            if x > ruler.right() {
                break;
            }
            painter.line_segment(
                [pos2(x, ruler.bottom() - 7.0), pos2(x, ruler.bottom())],
                stroke,
            );
            painter.text(
                pos2(x + 3.0, ruler.top() + 3.0),
                Align2::LEFT_TOP,
                format!("{}", bar + 1),
                font.clone(),
                Color32::from_gray(190),
            );
            bar += step;
        }
    }

    fn draw_headers(&mut self, ui: &mut Ui, headers: Rect, ppp: f32) {
        ui.painter_at(headers)
            .rect_filled(headers, 0.0, Color32::from_gray(30));
        let mut meters = std::mem::take(&mut self.meter_list);
        meters.clear();
        self.name_rects.clear();
        let h = self.view.track_height;
        // The timeline and headers share rows, so measure rows against the
        // timeline's top edge, which is the headers' top edge too.
        let tracks = self.visible_tracks(headers);

        for t in tracks {
            let top = self.row_top(headers, t);
            let row = Rect::from_min_max(pos2(headers.left(), top), pos2(headers.right(), top + h));
            let clip = row.intersect(headers);
            if !clip.is_positive() {
                continue;
            }

            // Meter at the right edge.
            let pad = (h * 0.12).min(4.0);
            let meter = Rect::from_min_max(
                pos2(row.right() - 11.0, row.top() + pad),
                pos2(row.right() - 4.0, row.bottom() - pad),
            );
            meters.rect(meter.intersect(headers), Color32::from_gray(18));
            let level = if self.meters_running || self.meter_clock > 0.0 {
                meter_level(t, self.meter_clock)
            } else {
                0.0
            };
            let filled = Rect::from_min_max(
                pos2(meter.left(), meter.bottom() - meter.height() * level),
                meter.max,
            );
            let colour = if level > 0.9 {
                Color32::from_rgb(230, 70, 60)
            } else if level > 0.7 {
                Color32::from_rgb(230, 200, 60)
            } else {
                Color32::from_rgb(80, 200, 110)
            };
            meters.rect(filled.intersect(headers), colour);

            let [r, g, b] = self.scene.tracks[t].color;
            let strip = Rect::from_min_max(
                pos2(row.left() + 3.0, row.top() + pad),
                pos2(row.left() + 7.0, row.bottom() - pad),
            );
            meters.rect(strip.intersect(headers), Color32::from_rgb(r, g, b));

            if h >= 24.0 {
                let inner = Rect::from_min_max(
                    pos2(row.left() + 12.0, row.top()),
                    pos2(meter.left() - 4.0, row.bottom()),
                );
                let mut child = ui.new_child(
                    UiBuilder::new()
                        .max_rect(inner)
                        .layout(Layout::left_to_right(Align::Center))
                        .id_salt(("track", t)),
                );
                child.set_clip_rect(clip);
                self.track_header_widgets(&mut child, t, ppp);
            }
        }

        if h >= 6.0 {
            let painter = ui.painter_at(headers);
            let line = Stroke::new(1.0 / ppp, Color32::from_gray(22));
            for t in self.visible_tracks(headers) {
                let y = self.row_top(headers, t) + h;
                painter.hline(headers.x_range(), y, line);
            }
        }

        let painter = ui.painter_at(headers);
        match self.mode {
            DenseMode::Gpu => {
                painter.add(egui_wgpu::Callback::new_paint_callback(
                    headers,
                    DenseCallback {
                        slot: Slot::Meters,
                        list: meters,
                    },
                ));
            }
            DenseMode::Painter => {
                paint_cpu::paint(&painter, &meters, &self.scene, ppp);
                self.meter_list = meters;
            }
        }
    }

    fn track_header_widgets(&mut self, ui: &mut Ui, t: usize, ppp: f32) {
        let spacing = ui.spacing().item_spacing.x;
        let controls = 20.0 + 20.0 + 26.0 + 3.0 * spacing;
        let name_width = (ui.available_width() - controls).max(20.0);
        let height = ui.available_height();

        let renaming = self.rename.as_ref().is_some_and(|r| r.track == t);
        if renaming {
            let rename = self.rename.as_mut().expect("checked above");
            let edit = ui.add_sized(
                [name_width, 20.0],
                TextEdit::singleline(&mut rename.text).font(FontId::proportional(13.0)),
            );
            if !rename.focus_requested {
                edit.request_focus();
                rename.focus_requested = true;
            }
            if edit.lost_focus() {
                let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
                let text = rename.text.trim().to_owned();
                if !cancelled && !text.is_empty() {
                    self.scene.tracks[t].name = text;
                }
                self.rename = None;
            }
        } else {
            let name = self.scene.tracks[t].name.clone();
            let response = ui
                .allocate_ui(vec2(name_width, height), |ui| {
                    ui.add(
                        Label::new(RichText::new(&name).size(13.0))
                            .sense(Sense::click())
                            .truncate(),
                    )
                })
                .inner;
            self.name_rects.push((t, response.rect, response.id));
            // A double-click renames. Assistive technology can't double-click,
            // so its press action renames too, as do Enter and F2 when the
            // name has keyboard focus.
            let (pressed, key) = ui.input(|i| {
                (
                    i.has_accesskit_action_request(response.id, accesskit::Action::Click),
                    i.key_pressed(Key::Enter) || i.key_pressed(Key::F2),
                )
            });
            if response.double_clicked() || pressed || (key && response.has_focus()) {
                self.start_rename(t);
            }
        }

        let track = &mut self.scene.tracks[t];
        let name = track.name.clone();
        toggle(
            ui,
            &mut track.muted,
            "M",
            format!("Mute {name}"),
            Color32::from_rgb(230, 170, 50),
        );
        toggle(
            ui,
            &mut track.soloed,
            "S",
            format!("Solo {name}"),
            Color32::from_rgb(80, 150, 230),
        );
        let knob = knob(ui, &mut track.gain, format!("{name} gain"), ppp);
        if knob.drag_started() {
            self.pointer_lock_request = Some(PointerLock::Lock);
        }
        if knob.drag_stopped() {
            self.pointer_lock_request = Some(PointerLock::Unlock);
        }
    }
}

/// A small toggle button exposed as a toggle to assistive technology.
fn toggle(ui: &mut Ui, on: &mut bool, text: &str, label: String, on_colour: Color32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(20.0, 18.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let state = *on;
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, state, &label));
    let visuals = ui.style().interact_selectable(&response, state);
    let fill = if state {
        on_colour
    } else {
        visuals.weak_bg_fill
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            3.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Outside,
        );
    }
    let text_colour = if state {
        Color32::BLACK
    } else {
        visuals.text_color()
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.0),
        text_colour,
    );
    response
}

/// A rotary knob. Dragging hides the pointer and asks the host to lock it, so
/// the drag is unbounded; raw mouse motion drives the value.
fn knob(ui: &mut Ui, value: &mut f32, label: String, ppp: f32) -> Response {
    let size = Vec2::splat(26.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let before = *value;

    if response.dragged() {
        // Raw motion arrives even while the pointer is locked; fall back to
        // the pointer delta (in points) on the first frame of a drag.
        let delta = ui
            .input(|i| i.pointer.motion())
            .unwrap_or(response.drag_delta() * ppp);
        *value = (*value - delta.y * 0.004).clamp(0.0, 1.0);
        ui.ctx().set_cursor_icon(CursorIcon::None);
    }
    if response.has_focus() {
        let step = ui.input(|i| {
            let up = i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::ArrowRight);
            let down = i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::ArrowLeft);
            f32::from(i8::from(up) - i8::from(down))
        });
        *value = (*value + step * 0.02).clamp(0.0, 1.0);
    }
    let (inc, dec, set) = ui.input(|i| {
        let set = i
            .accesskit_action_requests(response.id, accesskit::Action::SetValue)
            .find_map(|r| match r.data {
                Some(accesskit::ActionData::NumericValue(v)) => Some(v as f32),
                _ => None,
            });
        (
            i.num_accesskit_action_requests(response.id, accesskit::Action::Increment),
            i.num_accesskit_action_requests(response.id, accesskit::Action::Decrement),
            set,
        )
    });
    *value = (*value + 0.05 * (inc as f32 - dec as f32)).clamp(0.0, 1.0);
    if let Some(v) = set {
        *value = v.clamp(0.0, 1.0);
    }
    if *value != before {
        response.mark_changed();
    }

    let current = f64::from(*value);
    response.widget_info(|| WidgetInfo::slider(true, current, &label));
    // Declare the range and the actions handled above, as egui's Slider does.
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(1.0);
        node.set_numeric_value_step(0.05);
        node.add_action(accesskit::Action::SetValue);
        if current < 1.0 {
            node.add_action(accesskit::Action::Increment);
        }
        if current > 0.0 {
            node.add_action(accesskit::Action::Decrement);
        }
    });

    let painter = ui.painter();
    let centre = rect.center();
    let radius = rect.width() * 0.5 - 1.0;
    let visuals = ui.style().interact(&response);
    painter.circle_filled(centre, radius, visuals.bg_fill);
    painter.circle_stroke(centre, radius, visuals.fg_stroke);
    // Clockwise from about 7:30 (0) through 12 (0.5) to about 4:30 (1), with
    // angles measured in screen coordinates, where y points down.
    let angle = (135.0f32 + 270.0 * *value).to_radians();
    let tip = centre + vec2(angle.cos(), angle.sin()) * (radius - 2.0);
    painter.line_segment(
        [centre, tip],
        Stroke::new(2.0, Color32::from_rgb(240, 160, 60)),
    );
    if response.has_focus() {
        painter.circle_stroke(centre, radius + 2.0, ui.visuals().selection.stroke);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Config, Scene};
    use egui::{Context, Event, Modifiers, PointerButton, RawInput};

    fn small_app() -> App {
        let scene = Scene::generate(Config {
            tracks: 30,
            clips_per_track: 4,
            bars: 64,
            files: 1_000,
        });
        let mut app = App::new(scene, DenseMode::Painter);
        app.meters_running = false;
        app
    }

    fn context() -> Context {
        let ctx = Context::default();
        let (fonts, _) = crate::fonts::system_fonts().expect("a system font");
        ctx.set_fonts(fonts);
        ctx.enable_accesskit();
        ctx
    }

    fn input(time: f64, events: Vec<Event>) -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1600.0, 1000.0))),
            time: Some(time),
            events,
            ..Default::default()
        }
    }

    /// Runs one frame and drops its texture changes, which have no GPU here.
    fn frame(ctx: &Context, app: &mut App, input: RawInput) -> egui::FullOutput {
        let mut output = ctx.run_ui(input, |ui| app.ui(ui));
        output.textures_delta.clear();
        output
    }

    fn nodes(output: &egui::FullOutput) -> Vec<(accesskit::Role, String, String)> {
        let update = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("an AccessKit tree update");
        update
            .nodes
            .iter()
            .map(|(_, node)| {
                (
                    node.role(),
                    node.label().unwrap_or_default().to_owned(),
                    node.value().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn the_accessibility_tree_names_tracks_buttons_and_knobs() {
        let ctx = context();
        let mut app = small_app();
        frame(&ctx, &mut app, input(0.0, vec![]));
        let out = frame(&ctx, &mut app, input(0.1, vec![]));
        let nodes = nodes(&out);
        let first = app.scene.tracks[0].name.clone();

        assert!(
            nodes
                .iter()
                .any(|(role, _, value)| *role == accesskit::Role::Label && *value == first),
            "no label node for track name {first:?}"
        );
        assert!(nodes.iter().any(|(role, label, _)| {
            *role == accesskit::Role::Button && *label == format!("Mute {first}")
        }));
        assert!(nodes.iter().any(|(role, label, _)| {
            *role == accesskit::Role::Button && *label == format!("Solo {first}")
        }));
        assert!(nodes.iter().any(|(role, label, _)| {
            *role == accesskit::Role::Slider && *label == format!("{first} gain")
        }));
        // Only on-screen rows are in the tree: the list is virtualised.
        let labelled = nodes
            .iter()
            .filter(|(role, _, value)| {
                *role == accesskit::Role::Label && app.scene.tracks.iter().any(|t| t.name == *value)
            })
            .count();
        assert!(
            (10..30).contains(&labelled),
            "{labelled} track names in the tree"
        );
    }

    #[test]
    fn a_screen_reader_can_step_a_knob() {
        let ctx = context();
        let mut app = small_app();
        frame(&ctx, &mut app, input(0.0, vec![]));
        let out = frame(&ctx, &mut app, input(0.1, vec![]));
        let label = format!("{} gain", app.scene.tracks[0].name);
        let update = out.platform_output.accesskit_update.expect("a tree");
        let (knob, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some(label.as_str()))
            .expect("the first track's knob");
        assert!(node.supports_action(accesskit::Action::Increment));
        assert!(node.supports_action(accesskit::Action::SetValue));

        let before = app.scene.tracks[0].gain;
        let increment = Event::AccessKitActionRequest(accesskit::ActionRequest {
            action: accesskit::Action::Increment,
            target_tree: accesskit::TreeId::ROOT,
            target_node: *knob,
            data: None,
        });
        frame(&ctx, &mut app, input(0.2, vec![increment]));
        assert!((app.scene.tracks[0].gain - (before + 0.05)).abs() < 1e-6);
    }

    #[test]
    fn a_screen_reader_press_on_a_track_name_renames_it() {
        let ctx = context();
        let mut app = small_app();
        frame(&ctx, &mut app, input(0.0, vec![]));
        let (track, _, id) = app.name_rects[4];
        let press = Event::AccessKitActionRequest(accesskit::ActionRequest {
            action: accesskit::Action::Click,
            target_tree: accesskit::TreeId::ROOT,
            target_node: id.accesskit_id(),
            data: None,
        });
        frame(&ctx, &mut app, input(0.1, vec![press]));
        assert_eq!(app.rename.as_ref().map(|r| r.track), Some(track));
        // The next frame shows a text field in the tree, named by its text.
        let out = frame(&ctx, &mut app, input(0.2, vec![]));
        let name = app.scene.tracks[track].name.clone();
        assert!(
            nodes(&out)
                .iter()
                .any(|(role, _, value)| *role == accesskit::Role::TextInput && *value == name),
            "no text input holding {name:?}"
        );
    }

    #[test]
    fn double_clicking_a_track_name_renames_it() {
        let ctx = context();
        let mut app = small_app();
        frame(&ctx, &mut app, input(0.0, vec![]));
        let (track, rect, _) = app.name_rects[2];
        let at = rect.center();
        let click = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let mut time = 0.1;
        frame(&ctx, &mut app, input(time, vec![Event::PointerMoved(at)]));
        for _ in 0..2 {
            for pressed in [true, false] {
                time += 0.05;
                frame(&ctx, &mut app, input(time, vec![click(pressed)]));
            }
        }
        assert!(app.rename.is_some(), "a double-click starts a rename");

        // The text field takes focus, then gets new text and Enter.
        for events in [
            vec![],
            vec![Event::Key {
                key: Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }],
            vec![Event::Text("Lead vocal".into())],
            vec![Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        ] {
            time += 0.05;
            frame(&ctx, &mut app, input(time, events));
        }
        assert_eq!(app.scene.tracks[track].name, "Lead vocal");
        assert!(app.rename.is_none());
    }

    #[test]
    fn dragging_a_clip_moves_it() {
        let ctx = context();
        let mut app = small_app();
        frame(&ctx, &mut app, input(0.0, vec![]));
        // Find an on-screen clip and drag it one bar right and two tracks down.
        let (track, id) = (0..10)
            .find_map(|t| {
                let c = app.scene.tracks[t].clips[0];
                (app.scene.clips[c as usize].start_beat < 100).then_some((t, c))
            })
            .expect("a clip near the start");
        let start = app.scene.clips[id as usize].start_beat;
        // Rows line up with the track names egui laid out.
        let (_, name, _) = app.name_rects[track];
        let y = name.center().y;
        let x = HEADER_WIDTH
            + ((f64::from(start) + 0.5 - app.view.scroll_beats) * app.view.points_per_beat) as f32;
        let from = pos2(x, y);
        let to = pos2(
            x + 4.0 * app.view.points_per_beat as f32,
            y + 2.0 * TRACK_HEIGHT,
        );
        let mut time = 0.1;
        let mut step = |events: Vec<Event>, app: &mut App| {
            time += 0.02;
            frame(&ctx, app, input(time, events));
        };
        step(vec![Event::PointerMoved(from)], &mut app);
        step(
            vec![Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
            &mut app,
        );
        for i in 1..=10 {
            let p = from + (to - from) * (i as f32 / 10.0);
            step(vec![Event::PointerMoved(p)], &mut app);
        }
        step(
            vec![Event::PointerButton {
                pos: to,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            &mut app,
        );
        assert!(
            app.scene.tracks[track + 2].clips.contains(&id),
            "clip moved two tracks down"
        );
        assert!(!app.scene.tracks[track].clips.contains(&id));
        assert_eq!(app.scene.clips[id as usize].start_beat, start + 4);
    }

    #[test]
    fn wheel_scrolls_and_pinch_zooms_around_the_pointer() {
        let ctx = context();
        let mut app = small_app();
        frame(&ctx, &mut app, input(0.0, vec![]));
        let over = pos2(700.0, 400.0);
        let mut time = 0.1;
        let mut step = |events: Vec<Event>, app: &mut App| {
            time += 0.1;
            frame(&ctx, app, input(time, events));
        };
        step(vec![Event::PointerMoved(over)], &mut app);

        // Zoom in around the pointer: the beat under it stays put.
        let beat_under = |app: &App| {
            app.view.scroll_beats + f64::from(over.x - HEADER_WIDTH) / app.view.points_per_beat
        };
        let before = (app.view.points_per_beat, beat_under(&app));
        step(vec![Event::Zoom(2.0)], &mut app);
        assert!((app.view.points_per_beat / before.0 - 2.0).abs() < 1e-3);
        assert!((beat_under(&app) - before.1).abs() < 1e-3);

        // Scroll right and down. egui smooths wheel input over a few frames.
        let scroll = |delta| Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        };
        let (x0, y0) = (app.view.scroll_beats, app.view.scroll_y);
        step(vec![scroll(vec2(-80.0, -120.0))], &mut app);
        for _ in 0..30 {
            step(vec![], &mut app);
        }
        assert!(app.view.scroll_beats > x0, "scrolled right");
        assert!(app.view.scroll_y > y0, "scrolled down");
    }
}
