//! The Arrangement View (WP-1.8a): track headers, the Overview, the
//! beat-time ruler with the loop brace, the scrub area, the track lanes with
//! clips and waveforms, the playhead and the insert marker, the grid, and
//! the time ruler, placed by the layout layer's [`ArrangementLayout`]
//! (layout.md, L-ARR rows).
//!
//! It holds no song state and no view state of its own: what it shows comes
//! from the shell (the document, and the view in `bl_shell::arrangement`),
//! and every gesture becomes one of the Arrangement's view commands, which
//! the host runs. The lanes draw through Beatlayer's own wgpu pipelines in
//! an egui paint callback ([`gpu`]); text, the rulers, and the headers
//! through egui.
//!
//! The behaviour is in `docs/specs/arrangement-view.md`.

pub mod benchmark;
pub mod gpu;
pub mod headers;
pub mod peaks;
pub mod rulers;
pub mod scene;

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use bl_core::time::Ticks;
use bl_media::PeakEvent;
use bl_model::{Document, TrackId, TrackKind};
use bl_shell::arrangement::rows::{Row, rows_between};
use bl_shell::arrangement::{ArrangementView, Horizontal};
use bl_shell::arrangement_commands as ids;
use bl_shell::{Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, CursorIcon, EventFilter, Id, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2,
};
use serde_json::{Value, json};

use self::gpu::{ArrangementCallback, Shared, SharedFrame, Slot, lock};
use self::headers::Header;
use self::peaks::{PeakLibrary, PeakSource};
use self::scene::{ClipIndex, FrameStats, Label, LanesInput, ShownClip};
use crate::layout::ArrangementLayout;
use crate::main_window::{UiEvent, main_view_id};
use crate::theme::Theme;

/// How far one press of Option-+ or Option-− (Alt on Windows) changes the
/// selected tracks' heights, in points. Not measured in Live.
pub const HEIGHT_STEP: f64 = 16.0;

/// How fast dragging up and down in the beat-time ruler or the Overview
/// zooms: the factor per point dragged, down zooming in.
const DRAG_ZOOM_PER_POINT: f64 = 0.01;

/// The ID of the track lanes' area.
#[must_use]
pub fn lanes_id() -> Id {
    Id::new("arrangement-lanes")
}

/// The ID of the beat-time ruler.
#[must_use]
pub fn beat_ruler_id() -> Id {
    Id::new("arrangement-beat-ruler")
}

/// The ID of the scrub area.
#[must_use]
pub fn scrub_id() -> Id {
    Id::new("arrangement-scrub")
}

/// The ID of the Overview.
#[must_use]
pub fn overview_id() -> Id {
    Id::new("arrangement-overview")
}

/// The ID of the time ruler.
#[must_use]
pub fn time_ruler_id() -> Id {
    Id::new("arrangement-time-ruler")
}

/// The ID of the Optimize Height switch.
#[must_use]
pub fn optimize_height_id() -> Id {
    Id::new("arrangement-optimize-height")
}

/// The ID of the Optimize Width switch.
#[must_use]
pub fn optimize_width_id() -> Id {
    Id::new("arrangement-optimize-width")
}

/// What a frame's lanes were built from: when it's unchanged, the last
/// frame's instances are drawn again without being rebuilt.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LanesKey {
    shell: u64,
    document: u64,
    lanes: Rect,
    pixels_per_point: f32,
    peaks: u64,
    dark: bool,
}

/// What a frame's Overview was built from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct OverviewKey {
    shell: u64,
    document: u64,
    strip: Rect,
    end: Ticks,
}

/// A drag under way.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// Selecting time across tracks, from where it started.
    Select { at: Ticks, row: Option<usize> },
    /// Panning with Cmd-Option (Ctrl-Alt) held.
    Pan,
    /// In the beat-time ruler or the Overview: sideways scrolls, up and
    /// down zooms around where it started.
    Ruler { anchor: Ticks, overview: bool },
    /// In the time ruler: sideways scrolls.
    TimeRuler,
}

/// The Arrangement View's state between frames: caches and gestures, never
/// song or view state. See the [module docs](self).
pub struct ArrangementUi {
    shared: SharedFrame,
    library: PeakLibrary,
    index: ClipIndex,
    rows: Vec<Row>,
    labels: Vec<Label>,
    shown: Vec<ShownClip>,
    every_track: Vec<TrackId>,
    stats: FrameStats,
    lanes_key: Option<LanesKey>,
    overview_key: Option<OverviewKey>,
    drag: Option<Drag>,
    reported_viewport: Option<(f32, f32)>,
    /// The playhead the last frame drew: its position and its x, if it was
    /// in view (WP-1.18).
    playhead: Option<(Ticks, f32)>,
}

impl std::fmt::Debug for ArrangementUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArrangementUi")
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl Default for ArrangementUi {
    fn default() -> Self {
        Self::new()
    }
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

impl ArrangementUi {
    /// A view with no peak source: clips draw without waveforms until one
    /// is set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(std::sync::Mutex::new(Shared::default())),
            library: PeakLibrary::new(),
            index: ClipIndex::default(),
            rows: Vec::new(),
            labels: Vec::new(),
            shown: Vec::new(),
            every_track: Vec::new(),
            stats: FrameStats::default(),
            lanes_key: None,
            overview_key: None,
            drag: None,
            reported_viewport: None,
            playhead: None,
        }
    }

    /// The playhead the last frame drew: the position it showed, and its x
    /// in the window, or `None` if it was out of view. It's the engine's, as
    /// the shell last took it (`docs/specs/control-bar-transport.md`, R14).
    #[must_use]
    pub fn drawn_playhead(&self) -> Option<(Ticks, f32)> {
        self.playhead
    }

    /// Takes waveform peaks from `source`, such as the app's
    /// `bl_media::PeakCache`, with its events (`PeakCache::subscribe`) so
    /// waveforms appear when their peaks are ready and are redrawn when the
    /// cache invalidates them.
    pub fn set_peak_source(
        &mut self,
        source: Box<dyn PeakSource>,
        events: Option<Receiver<PeakEvent>>,
    ) {
        let mut shared = lock(&self.shared);
        self.library.set_source(source, events, &mut shared.atlas);
        self.lanes_key = None;
    }

    /// What the last frame built: the rows and clips on screen, out of how
    /// many.
    #[must_use]
    pub fn stats(&self) -> FrameStats {
        self.stats
    }

    /// The block the GPU callback reads, for hosts and benchmarks that
    /// render offscreen.
    #[must_use]
    pub fn shared(&self) -> SharedFrame {
        Arc::clone(&self.shared)
    }

    /// Draws the Arrangement View in the main view's rectangle `rect` and
    /// adds what the person did to `events`.
    #[expect(clippy::too_many_lines, reason = "the view's parts, top to bottom")]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let view = shell.arrangement();
        let pinned: Vec<TrackId> = bl_shell::arrangement::rows::pinned_tracks(doc).collect();
        let layout = ArrangementLayout::compute(rect, view.overview(), pinned.len());
        let lanes = layout.lanes;

        // The lanes' size, for the view commands that need it.
        let size = (lanes.width(), lanes.height());
        if self.reported_viewport != Some(size) {
            self.reported_viewport = Some(size);
            let viewport = view.viewport();
            if (viewport.width - size.0).abs() > 0.01 || (viewport.height - size.1).abs() > 0.01 {
                events.push(UiEvent::ArrangementViewport {
                    width: size.0,
                    height: size.1,
                });
            }
        }

        let horizontal = view.horizontal(doc);
        view.rows_into(doc, &mut self.rows);
        self.every_track.clear();
        self.every_track
            .extend(self.rows.iter().map(|row| row.track));
        let index_changed = self.index.update(doc, shell.document_revision());
        let scroll_y = view.scroll_y() as f32;
        let ppp = ui.ctx().pixels_per_point();

        ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);

        // The lanes, built only when something they show changed.
        {
            let mut shared = lock(&self.shared);
            let shared = &mut *shared;
            let peaks_changed = self.library.update(&mut shared.atlas);
            let key = LanesKey {
                shell: shell.revision(),
                document: shell.document_revision(),
                lanes,
                pixels_per_point: ppp,
                peaks: self.library.generation(),
                dark: theme.dark,
            };
            if self.lanes_key != Some(key) || index_changed || peaks_changed {
                let slot = &mut shared.slots[Slot::Lanes as usize];
                self.stats = scene::build_lanes(
                    &LanesInput {
                        doc,
                        rows: &self.rows,
                        horizontal,
                        lanes,
                        scroll_y,
                        pixels_per_point: ppp,
                        theme,
                        selected: view.selected_tracks(),
                        grid_step: view.grid_step(doc),
                    },
                    &self.index,
                    &mut self.library,
                    &mut shared.atlas,
                    slot,
                    &mut self.labels,
                    &mut self.shown,
                );
                slot.version = slot.version.wrapping_add(1);
                // Asking for peaks may have changed the library.
                self.lanes_key = Some(LanesKey {
                    peaks: self.library.generation(),
                    ..key
                });
            } else {
                self.stats.rebuilt = false;
            }
            if let Some(strip) = layout.overview {
                let end = view.arrangement_end(doc);
                let key = OverviewKey {
                    shell: shell.revision(),
                    document: shell.document_revision(),
                    strip,
                    end,
                };
                if self.overview_key != Some(key) {
                    let slot = &mut shared.slots[Slot::Overview as usize];
                    scene::build_overview(&self.rows, &self.index, strip, end, theme, slot);
                    slot.version = slot.version.wrapping_add(1);
                    self.overview_key = Some(key);
                }
            }
        }
        ui.painter()
            .with_clip_rect(lanes)
            .add(egui_wgpu::Callback::new_paint_callback(
                lanes,
                ArrangementCallback {
                    shared: Arc::clone(&self.shared),
                    slot: Slot::Lanes,
                },
            ));
        self.draw_labels(ui, doc, theme, lanes);
        self.lanes_input(ui, view, doc, &layout, horizontal, events);
        self.draw_overlays(ui, view, theme, &layout, horizontal);

        // The rulers.
        if let Some(strip) = layout.overview {
            ui.painter()
                .with_clip_rect(strip)
                .add(egui_wgpu::Callback::new_paint_callback(
                    strip,
                    ArrangementCallback {
                        shared: Arc::clone(&self.shared),
                        slot: Slot::Overview,
                    },
                ));
            self.overview(ui, view, doc, theme, strip, horizontal, lanes, events);
        }
        let ruler_painter = ui.painter_at(layout.beat_ruler);
        rulers::beat_ruler(
            &ruler_painter,
            layout.beat_ruler,
            &doc.settings().time_signatures,
            horizontal,
            &view.transport(),
            theme,
        );
        rulers::scrub_area(&ui.painter_at(layout.scrub), layout.scrub, theme);
        rulers::time_ruler(
            &ui.painter_at(layout.time_ruler),
            layout.time_ruler,
            &doc.settings().tempo,
            horizontal,
            theme,
        );
        self.ruler_input(ui, doc, &layout, horizontal, events);

        // The playhead, over the rulers and the lanes.
        let transport = view.transport();
        let x = lanes.left() + horizontal.x(transport.position) as f32;
        self.playhead =
            (x >= lanes.left() && x <= lanes.right()).then_some((transport.position, x));
        if x >= lanes.left() && x <= lanes.right() {
            let top = layout.beat_ruler.top();
            rulers::marker(
                &ui.painter_at(Rect::from_min_max(pos2(lanes.left(), top), lanes.max)),
                x,
                top,
                lanes.bottom(),
                theme.colors.playhead.0,
            );
        }

        // The headers.
        self.headers(ui, view, doc, theme, &layout, scroll_y, &pinned, events);
        self.header_controls(ui, view, theme, &layout, events);
        // Over the pinned rows' lanes, in the lanes' lower right corner.
        self.grid_indicator(ui, view, doc, theme, layout.grid_indicator);
        self.accessibility(ui, doc, &layout);
        self.keys(ui, shell, view, events);

        if transport.playing {
            // The playhead moves.
            ui.ctx().request_repaint();
        }
    }

    fn draw_labels(&self, ui: &Ui, doc: &Document, theme: &Theme, lanes: Rect) {
        let painter = ui.painter_at(lanes);
        for label in &self.labels {
            let Some(clip) = doc.clip(label.clip) else {
                continue;
            };
            if clip.name.is_empty() {
                continue;
            }
            painter.with_clip_rect(label.rect.intersect(lanes)).text(
                pos2(label.rect.left() + 3.0, label.rect.center().y),
                Align2::LEFT_CENTER,
                &clip.name,
                theme.small_font(),
                theme.colors.clip_text.0,
            );
        }
    }

    /// The row under a point in the lanes, if any.
    fn row_at(&self, lanes: Rect, scroll_y: f32, pos: Pos2) -> Option<usize> {
        let y = pos.y - lanes.top() + scroll_y;
        let index = bl_shell::arrangement::rows::first_row_at(&self.rows, y);
        (index < self.rows.len() && self.rows[index].top <= y).then_some(index)
    }

    fn lanes_input(
        &mut self,
        ui: &mut Ui,
        view: &ArrangementView,
        doc: &Document,
        layout: &ArrangementLayout,
        horizontal: Horizontal,
        events: &mut Vec<UiEvent>,
    ) {
        let lanes = layout.lanes;
        let scroll_y = view.scroll_y() as f32;
        let response = ui.interact(lanes, lanes_id(), Sense::click_and_drag());
        let at = |pos: Pos2| horizontal.at(f64::from(pos.x - lanes.left()));
        let modifiers = ui.input(|i| i.modifiers);
        let pan_keys = if cfg!(target_os = "macos") {
            modifiers.mac_cmd && modifiers.alt
        } else {
            modifiers.ctrl && modifiers.alt
        };
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            self.drag = Some(if pan_keys {
                Drag::Pan
            } else {
                Drag::Select {
                    at: view.snap(doc, at(press)),
                    row: self.row_at(lanes, scroll_y, press),
                }
            });
        }
        if response.dragged() {
            match self.drag {
                Some(Drag::Pan) => {
                    // S-ZOOM-007, S-ARR-024: the content follows the pointer.
                    let delta = response.drag_delta();
                    if delta != egui::Vec2::ZERO {
                        invoke(events, ids::SCROLL, json!({ "x": -delta.x, "y": -delta.y }));
                    }
                }
                Some(Drag::Select { at: from, row }) => {
                    if let Some(pos) = response.interact_pointer_pos() {
                        // I-DRAG-010: time across the tracks dragged over.
                        let to = view.snap(doc, at(pos));
                        let here = self.row_at(lanes, scroll_y, pos).or(row);
                        let tracks: Vec<TrackId> = match (row, here) {
                            (Some(a), Some(b)) => {
                                let (a, b) = (a.min(b), a.max(b));
                                self.rows[a..=b].iter().map(|r| r.track).collect()
                            }
                            _ => Vec::new(),
                        };
                        invoke(
                            events,
                            ids::SELECT_TIME,
                            json!({ "start": from, "end": to, "tracks": tracks }),
                        );
                    }
                }
                _ => {}
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            // I-CLICK-011: the insert marker, and the track under it.
            let track = self
                .row_at(lanes, scroll_y, pos)
                .map(|i| self.rows[i].track);
            let mut args = json!({ "at": view.snap(doc, at(pos)) });
            if let Some(track) = track {
                args["track"] = json!(track);
            }
            invoke(events, ids::SET_INSERT_MARKER, args);
        }

        // The wheel and the trackpad, over the lanes or the headers.
        let over = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|pos| lanes.contains(*pos) || layout.headers.contains(*pos));
        if let Some(pos) = over {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta(), i.zoom_delta()));
            if (zoom - 1.0).abs() > 1e-4 {
                // I-SCROLL-003: Cmd with the wheel, or a pinch, zooms around
                // the pointer.
                invoke(
                    events,
                    ids::ZOOM,
                    json!({ "factor": f64::from(zoom), "anchor": at(pos) }),
                );
            } else if scroll != egui::Vec2::ZERO {
                if modifiers.alt {
                    // I-SCROLL-004: Option (Alt) with the wheel changes the
                    // height of the track under the pointer.
                    if let Some(row) = self.row_at(lanes, scroll_y, pos) {
                        invoke(
                            events,
                            ids::SET_TRACK_HEIGHT,
                            json!({ "tracks": [self.rows[row].track], "delta": scroll.y }),
                        );
                    }
                } else {
                    // I-SCROLL-001, S-ZOOM-006: egui turns Shift with the
                    // wheel into sideways scrolling.
                    invoke(
                        events,
                        ids::SCROLL,
                        json!({ "x": -scroll.x, "y": -scroll.y }),
                    );
                }
            }
        }
        if pan_keys && response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
    }

    fn draw_overlays(
        &self,
        ui: &Ui,
        view: &ArrangementView,
        theme: &Theme,
        layout: &ArrangementLayout,
        horizontal: Horizontal,
    ) {
        let lanes = layout.lanes;
        let painter = ui.painter_at(lanes);
        let scroll_y = view.scroll_y() as f32;
        let x_of = |at: Ticks| lanes.left() + horizontal.x(at) as f32;
        let selected = view.selected_tracks();
        let rows_of_selection = || {
            self.rows
                .iter()
                .filter(|row| selected.contains(&row.track))
                .map(|row| {
                    let top = lanes.top() + row.top - scroll_y;
                    (top, top + row.height)
                })
        };
        // The time selection, over the selected tracks.
        if let Some((start, end)) = view.time_selection() {
            let (x0, x1) = (x_of(start), x_of(end));
            for (top, bottom) in rows_of_selection() {
                painter.rect_filled(
                    Rect::from_min_max(pos2(x0, top), pos2(x1, bottom)),
                    0.0,
                    theme.colors.time_selection.0.gamma_multiply(0.28),
                );
            }
        } else {
            // The insert marker, across the selected tracks.
            let x = x_of(view.insert_marker());
            if x >= lanes.left() && x <= lanes.right() {
                for (top, bottom) in rows_of_selection() {
                    rulers::marker(&painter, x, top, bottom, theme.colors.insert_marker.0);
                }
            }
        }
        // The drop area under the last track (L-ARR-013).
        let bottom = self.rows.last().map_or(0.0, Row::bottom);
        let drop_top = lanes.top() + bottom - scroll_y;
        if drop_top < lanes.bottom() - 12.0 && !view.optimize_height() {
            painter.text(
                pos2(
                    lanes.center().x,
                    (drop_top + 22.0).min(lanes.bottom() - 8.0),
                ),
                Align2::CENTER_CENTER,
                "Drop Files and Devices Here",
                theme.small_font(),
                theme.colors.pending.0,
            );
        }
    }

    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn overview(
        &mut self,
        ui: &mut Ui,
        view: &ArrangementView,
        doc: &Document,
        theme: &Theme,
        strip: Rect,
        horizontal: Horizontal,
        lanes: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let end = view.arrangement_end(doc);
        let overview_tpp = end.get() as f64 / f64::from(strip.width().max(1.0));
        let range = horizontal.range(lanes.width());
        let x0 = strip.left() + (range.start.get() as f64 / overview_tpp) as f32;
        let x1 = strip.left() + (range.end.get() as f64 / overview_tpp) as f32;
        // The outline round the part in view, rows included.
        let content = view.content_height(&self.rows).max(1.0);
        let scroll = view.scroll_y() as f32;
        let y0 = strip.top() + strip.height() * (scroll / content).clamp(0.0, 1.0);
        let y1 =
            strip.top() + strip.height() * ((scroll + lanes.height()) / content).clamp(0.0, 1.0);
        let outline = Rect::from_min_max(
            pos2(x0.max(strip.left()), y0),
            pos2(x1.min(strip.right()).max(x0 + 2.0), y1.max(y0 + 2.0)),
        );
        ui.painter_at(strip.expand(1.0)).rect_stroke(
            outline,
            0.0,
            Stroke::new(1.0, theme.colors.overview_outline.0),
            StrokeKind::Inside,
        );
        let response = ui.interact(strip, overview_id(), Sense::click_and_drag());
        let time_at = |pos: Pos2| Ticks(((pos.x - strip.left()) as f64 * overview_tpp) as i64);
        if response.double_clicked() {
            if let Some(pos) = response.interact_pointer_pos()
                && outline.contains(pos)
            {
                // I-CLICK-015: out to the whole arrangement.
                invoke(
                    events,
                    ids::SHOW_RANGE,
                    json!({ "start": Ticks::ZERO, "end": end }),
                );
            }
        } else if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && !outline.contains(pos)
        {
            // A click outside the outline brings that part into view.
            let span = range.end.get() - range.start.get();
            let start = (time_at(pos).get() - span / 2).max(0);
            invoke(
                events,
                ids::SHOW_RANGE,
                json!({ "start": start, "end": start + span }),
            );
        }
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            self.drag = Some(Drag::Ruler {
                anchor: time_at(press),
                overview: true,
            });
        }
        if response.dragged()
            && let Some(Drag::Ruler { anchor, .. }) = self.drag
        {
            let delta = response.drag_delta();
            ruler_drag(
                events,
                delta,
                anchor,
                overview_tpp / horizontal.ticks_per_point,
            );
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Overview"));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::ScrollBar);
            let signatures = &doc.settings().time_signatures;
            let bar = |at: Ticks| rulers::position_label(signatures, at, 0);
            node.set_value(format!(
                "bars {} to {} in view, of bars {} to {}",
                bar(range.start),
                bar(range.end),
                bar(Ticks::ZERO),
                bar(end)
            ));
            node.set_description(
                "The whole arrangement, with the part in view outlined. Drag sideways to \
                 scroll, up and down to zoom."
                    .to_owned(),
            );
        });
    }

    fn ruler_input(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        layout: &ArrangementLayout,
        horizontal: Horizontal,
        events: &mut Vec<UiEvent>,
    ) {
        let ruler = layout.beat_ruler;
        let at = |pos: Pos2| horizontal.at(f64::from(pos.x - ruler.left()));
        let response = ui.interact(ruler, beat_ruler_id(), Sense::click_and_drag());
        if response.double_clicked() {
            // I-CLICK-016.
            invoke(events, ids::ZOOM_TO_SELECTION, json!({}));
        } else if response.clicked() {
            // Clicking the ruler pauses Follow while the song plays.
            invoke(events, ids::FOLLOW, json!({ "paused": true }));
        }
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            self.drag = Some(Drag::Ruler {
                anchor: at(press),
                overview: false,
            });
        }
        if response.dragged()
            && let Some(Drag::Ruler {
                anchor,
                overview: false,
            }) = self.drag
        {
            // I-DRAG-007.
            ruler_drag(events, response.drag_delta(), anchor, -1.0);
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Beat-Time Ruler"));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::ScrollBar);
            let range = horizontal.range(ruler.width());
            let signatures = &doc.settings().time_signatures;
            node.set_value(format!(
                "bars {} to {}",
                rulers::position_label(signatures, range.start, 0),
                rulers::position_label(signatures, range.end, 0)
            ));
            node.set_description(
                "Drag sideways to scroll and up or down to zoom; double-click to zoom to the \
                 selection."
                    .to_owned(),
            );
        });

        let scrub = ui.interact(layout.scrub, scrub_id(), Sense::click());
        if scrub.clicked() {
            // Clicking the scrub area resumes Follow. Starting playback from
            // the click (I-CLICK-012) jumps by the global launch
            // quantisation, which comes with the Quantization Menu (M3;
            // control-bar-transport, "Open questions").
            invoke(events, ids::FOLLOW, json!({ "paused": false }));
        }
        scrub.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Scrub Area"));
        ui.ctx().accesskit_node_builder(scrub.id, |node| {
            node.set_role(Role::Group);
            node.set_description(
                "Playing from a click here comes with the global launch quantisation (M3); the \
                 locators sit here."
                    .to_owned(),
            );
        });

        let time = ui.interact(layout.time_ruler, time_ruler_id(), Sense::drag());
        if time.drag_started() {
            self.drag = Some(Drag::TimeRuler);
        }
        if time.dragged() && self.drag == Some(Drag::TimeRuler) {
            // I-DRAG-008.
            let dx = time.drag_delta().x;
            if dx != 0.0 {
                invoke(events, ids::SCROLL, json!({ "x": -dx }));
            }
        }
        if time.drag_stopped() {
            self.drag = None;
        }
        time.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Time Ruler"));
        ui.ctx().accesskit_node_builder(time.id, |node| {
            node.set_role(Role::ScrollBar);
            let tempo = &doc.settings().tempo;
            let range = horizontal.range(layout.time_ruler.width());
            let seconds = |at: Ticks| tempo.ticks_to_seconds(at.max(Ticks::ZERO));
            node.set_value(format!(
                "{} to {}",
                rulers::time_label(seconds(range.start), false),
                rulers::time_label(seconds(range.end), false)
            ));
            node.set_description("Drag sideways to scroll.".to_owned());
        });
    }

    fn grid_indicator(
        &self,
        ui: &mut Ui,
        view: &ArrangementView,
        doc: &Document,
        theme: &Theme,
        rect: Rect,
    ) {
        let grid = view.grid();
        let step = view.grid_step(doc);
        let text = if grid.snap {
            step.label()
        } else {
            format!("({})", step.label())
        };
        // Right-aligned: a longer label ("16 Bars") grows to the left.
        ui.painter_at(rect.with_min_x(rect.left() - 40.0)).text(
            rect.right_center(),
            Align2::RIGHT_CENTER,
            &text,
            theme.small_font(),
            theme.colors.text_dim.0,
        );
        let id = Id::new("arrangement-grid-indicator");
        let response = ui.interact(rect, id, Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, "Grid"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Status);
            node.set_label("Grid".to_owned());
            node.set_value(format!(
                "{text}, {}{}",
                if grid.fixed_on { "fixed" } else { "adaptive" },
                if grid.snap { "" } else { ", snapping off" }
            ));
        });
    }

    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn headers(
        &self,
        ui: &mut Ui,
        view: &ArrangementView,
        doc: &Document,
        theme: &Theme,
        layout: &ArrangementLayout,
        scroll_y: f32,
        pinned: &[TrackId],
        events: &mut Vec<UiEvent>,
    ) {
        let column = layout.headers;
        let visible = rows_between(&self.rows, scroll_y, scroll_y + column.height());
        let selected = view.selected_tracks();
        let title_bars = view.title_bars_selected();
        let mut header_ui = ui.new_child(egui::UiBuilder::new().max_rect(column));
        header_ui.set_clip_rect(column);
        for (index, row) in self.rows[visible.clone()].iter().enumerate() {
            let Some(track) = doc.track(row.track) else {
                continue;
            };
            let top = column.top() + row.top - scroll_y;
            let is_selected = selected.contains(&row.track);
            headers::header(
                &mut header_ui,
                theme,
                &Header {
                    track,
                    rect: Rect::from_min_max(
                        pos2(column.left(), top),
                        pos2(column.right(), top + row.height),
                    ),
                    activator: (visible.start + index + 1).to_string(),
                    colour: scene::track_colour(theme, row.track),
                    folded: row.folded,
                    selected: is_selected,
                    title_selected: is_selected && title_bars,
                    foldable: true,
                    resizable: !view.optimize_height(),
                },
                &self.every_track,
                events,
            );
        }
        // The returns and Main, pinned under the lanes (L-ARR-014).
        for (index, id) in pinned.iter().enumerate() {
            let Some(track) = doc.track(*id) else {
                continue;
            };
            let row = layout.pinned_row(index, pinned.len());
            let lane = Rect::from_min_max(row.min, pos2(layout.pinned_lanes.right(), row.bottom()));
            ui.painter_at(lane)
                .rect_filled(lane, 0.0, theme.colors.lane.0);
            ui.painter_at(lane).line_segment(
                [lane.left_bottom(), lane.right_bottom()],
                Stroke::new(1.0, theme.colors.lane_line.0),
            );
            let is_selected = selected.contains(id);
            headers::header(
                ui,
                theme,
                &Header {
                    track,
                    rect: Rect::from_min_max(
                        pos2(layout.pinned_headers.left(), row.top()),
                        row.max,
                    ),
                    activator: char::from(b'A' + (index % 26) as u8).to_string(),
                    colour: if track.kind() == TrackKind::Main {
                        theme.colors.control_pressed.0
                    } else {
                        scene::track_colour(theme, *id)
                    },
                    folded: true,
                    selected: is_selected,
                    title_selected: is_selected && title_bars,
                    foldable: false,
                    resizable: false,
                },
                &[],
                events,
            );
        }
    }

    fn header_controls(
        &self,
        ui: &mut Ui,
        view: &ArrangementView,
        theme: &Theme,
        layout: &ArrangementLayout,
        events: &mut Vec<UiEvent>,
    ) {
        // L-ARR-010: inert until locators (M6) and automation (WP-2.12).
        for (rect, name, text, package) in [
            (layout.set_locator, "Set Locator", "Set", "milestone M6"),
            (
                layout.previous_locator,
                "Previous Locator",
                "◀",
                "milestone M6",
            ),
            (layout.next_locator, "Next Locator", "▶", "milestone M6"),
            (layout.automation_mode, "Automation Mode", "A", "WP-2.12"),
            (layout.lock_envelopes, "Lock Envelopes", "L", "WP-2.12"),
            (layout.waveform_zoom, "Waveform Zoom", "", "milestone M2"),
            (
                layout.waveform_zoom_level,
                "Waveform Vertical Zoom Level",
                "0 dB",
                "milestone M2",
            ),
        ] {
            let id = Id::new(("arrangement-control", name));
            let response = ui.interact(rect, id, Sense::hover());
            let painter = ui.painter_at(rect.expand(1.0));
            painter.rect(
                rect,
                theme.sizes.corner,
                theme.colors.control.0.gamma_multiply(0.6),
                Stroke::new(1.0, theme.colors.separator.0),
                StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                theme.small_font(),
                theme.colors.pending.0,
            );
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, false, name));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_description(format!("Not built yet: arrives with {package}."));
            });
        }
        // L-ARR-017: Optimize Height and Width, live.
        for (rect, id, name, text, on, command) in [
            (
                layout.optimize_height,
                optimize_height_id(),
                "Optimize Height",
                "H",
                view.optimize_height(),
                ids::OPTIMIZE_HEIGHT,
            ),
            (
                layout.optimize_width,
                optimize_width_id(),
                "Optimize Width",
                "W",
                view.optimize_width(),
                ids::OPTIMIZE_WIDTH,
            ),
        ] {
            let response = ui.interact(rect, id, Sense::click());
            let fill = if on {
                theme.colors.accent.0
            } else if response.hovered() {
                theme.colors.control_hover.0
            } else {
                theme.colors.control.0
            };
            let ink = if on {
                theme.colors.accent_text.0
            } else {
                theme.colors.text.0
            };
            let painter = ui.painter_at(rect.expand(1.0));
            painter.rect_filled(rect, theme.sizes.corner, fill);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                theme.small_font(),
                ink,
            );
            if response.clicked() {
                invoke(events, command, json!({}));
            }
            response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, name));
        }
    }

    /// The view's own accessible node, a pane named as the main view is, and
    /// the clips on screen as items in it.
    fn accessibility(&self, ui: &mut Ui, doc: &Document, layout: &ArrangementLayout) {
        let enabled = ui
            .ctx()
            .accesskit_node_builder(lanes_id(), |node| {
                node.set_role(Role::Group);
                node.set_label("Track Lanes".to_owned());
            })
            .is_some();
        if !enabled {
            return;
        }
        // Clips big enough to point at, up to a limit, so a zoomed-out view
        // of thousands stays cheap.
        let lanes = layout.lanes;
        for shown in self
            .shown
            .iter()
            .filter(|s| s.rect.width() >= 6.0 && s.rect.height() >= 8.0)
            .take(512)
        {
            let Some(clip) = doc.clip(shown.clip) else {
                continue;
            };
            let id = Id::new(("arrangement-clip", shown.clip));
            let rect = shown.rect.intersect(lanes);
            let _ = ui.interact(rect, id, Sense::hover());
            let signatures = &doc.settings().time_signatures;
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::ListItem);
                let name = if clip.name.is_empty() {
                    if clip.data.is_audio() {
                        "Audio Clip"
                    } else {
                        "MIDI Clip"
                    }
                } else {
                    &clip.name
                };
                node.set_label(name.to_owned());
                node.set_value(format!(
                    "{} to {}{}",
                    signatures.to_bbs(clip.start),
                    signatures.to_bbs(clip.end()),
                    if clip.mute { ", deactivated" } else { "" }
                ));
            });
        }
    }

    /// Keys the view reads itself, because one shortcut's keys act
    /// differently (the keymap gives each shortcut one command and
    /// arguments): Up and Down move between tracks (S-A11Y-003), Left and
    /// Right fold and unfold the selected title bars (S-ARR-020), and
    /// Option-+ and Option-− (Alt on Windows) make the selected tracks taller
    /// and shorter (S-ARR-022).
    fn keys(&self, ui: &mut Ui, shell: &Shell, view: &ArrangementView, events: &mut Vec<UiEvent>) {
        let focused = shell.focused_area() == bl_shell::Area::MainView
            && shell.focused_window() == bl_shell::WindowKind::Main
            && !ui.ctx().text_edit_focused()
            && ui
                .ctx()
                .memory(|m| m.focused().is_none_or(|id| id == main_view_id()));
        if !focused {
            return;
        }
        if ui.ctx().memory(|m| m.has_focus(main_view_id())) {
            ui.ctx().memory_mut(|m| {
                m.set_focus_lock_filter(
                    main_view_id(),
                    EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..EventFilter::default()
                    },
                );
            });
        }
        // Each press with the modifiers held as it went down.
        let (up, down, left, right, taller, shorter) = ui.input(|i| {
            let pressed = |keys: &[Key], option: bool| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { key, pressed: true, modifiers, .. }
                            if keys.contains(key)
                                && modifiers.alt == option
                                && !modifiers.command
                                && !modifiers.ctrl
                                && !modifiers.shift
                    )
                })
            };
            (
                pressed(&[Key::ArrowUp], false),
                pressed(&[Key::ArrowDown], false),
                pressed(&[Key::ArrowLeft], false),
                pressed(&[Key::ArrowRight], false),
                pressed(&[Key::Plus, Key::Equals], true),
                pressed(&[Key::Minus], true),
            )
        });
        if up || down {
            let current = view
                .selected_tracks()
                .first()
                .and_then(|id| self.rows.iter().position(|row| row.track == *id));
            let next = match (current, up) {
                (Some(i), true) => i.checked_sub(1),
                (Some(i), false) => (i + 1 < self.rows.len()).then_some(i + 1),
                (None, _) => (!self.rows.is_empty()).then_some(0),
            };
            if let Some(next) = next {
                invoke(
                    events,
                    ids::SELECT_TRACKS,
                    json!({
                        "tracks": [self.rows[next].track],
                        "title_bars": view.title_bars_selected(),
                    }),
                );
            }
        }
        if (left || right) && view.title_bars_selected() {
            invoke(events, ids::FOLD_TRACKS, json!({ "fold": left }));
        }
        if (taller || shorter) && !view.selected_tracks().is_empty() {
            let delta = if taller { HEIGHT_STEP } else { -HEIGHT_STEP };
            invoke(events, ids::SET_TRACK_HEIGHT, json!({ "delta": delta }));
        }
    }
}

/// A drag in the beat-time ruler or the Overview: up and down zooms around
/// `anchor` (down zooms in), sideways scrolls by `scale` lane points to a
/// dragged point. The ruler is dragged like the lanes it holds (`scale` −1:
/// dragging right shows earlier bars); the Overview's outline like a
/// scroll bar's thumb (positive).
fn ruler_drag(events: &mut Vec<UiEvent>, delta: egui::Vec2, anchor: Ticks, scale: f64) {
    if delta.y != 0.0 {
        let factor = (f64::from(delta.y) * DRAG_ZOOM_PER_POINT).exp();
        invoke(
            events,
            ids::ZOOM,
            json!({ "factor": factor.clamp(1.0 / 64.0, 64.0), "anchor": anchor }),
        );
    }
    if delta.x != 0.0 {
        invoke(
            events,
            ids::SCROLL,
            json!({ "x": f64::from(delta.x) * scale }),
        );
    }
}

/// Colour of a track, re-exported for the headers and the benchmark.
#[must_use]
pub fn colour_of(theme: &Theme, track: TrackId) -> Color32 {
    scene::track_colour(theme, track)
}
