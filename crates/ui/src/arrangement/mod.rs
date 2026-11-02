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
//! The behaviour is in the arrangement view spec. Editing clips by mouse and
//! keyboard is in [`editing`] (WP-1.8b), whose behaviour is in the
//! arrangement editing spec.

pub mod arranger;
pub mod automation;
pub mod benchmark;
pub mod context_menu;
pub mod editing;
pub mod fades;
pub mod gpu;
pub mod headers;
pub mod peaks;
pub mod rulers;
pub mod scene;
pub mod signatures;
pub mod take_lanes;

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use bl_core::time::Ticks;
use bl_media::PeakEvent;
use bl_model::{ClipId, Document, TrackId, TrackKind};
use bl_shell::arrangement::rows::{DROP_AREA, FOLDED_HEIGHT, Row, rows_between};
use bl_shell::arrangement::snapping::Dragged;
use bl_shell::arrangement::{ArrangementView, Horizontal};
use bl_shell::arrangement_commands::{self as ids, Lane};
use bl_shell::clip_drag::{ClipDragEvent, DragSource, DropSpot};
use bl_shell::locator_commands as locator_ids;
use bl_shell::{GesturePhase, InfoTarget, Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, CursorIcon, EventFilter, Id, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2,
};
use serde_json::{Value, json};

use self::automation::AutomationGestures;
use self::context_menu::ContextMenus;
use self::editing::{ClipGestures, EditKeys, Lanes, Zone};
use self::gpu::{ArrangementCallback, Shared, SharedFrame, Slot, lock};
use self::headers::Header;
use self::peaks::{PeakLibrary, PeakSource};
use self::scene::{ClipIndex, FrameStats, Label, LanesInput, ShownClip};
use self::take_lanes::{Named, TakeLaneMenus};
use crate::layout::{ArrangementLayout, header_columns};
use crate::main_window::{UiEvent, main_view_id};
use crate::theme::Theme;
use crate::track_menu::TrackMenus;

/// How far one press of Option-+ or Option-− (Alt on Windows) changes the
/// selected tracks' heights, in points. Not measured in Live.
pub const HEIGHT_STEP: f64 = 16.0;

/// How fast dragging up and down in the beat-time ruler or the Overview
/// zooms: the factor per point dragged, down zooming in.
const DRAG_ZOOM_PER_POINT: f64 = 0.01;

/// How long a press in the scrub area or the beat-time ruler lasts before
/// it is a hold that loops rather than a click (time-editing R25). Beatlayer's
/// own: Live's isn't observed.
const SCRUB_HOLD_SECONDS: f64 = 0.3;

/// The ID of the track lanes' area.
#[must_use]
pub fn lanes_id() -> Id {
    Id::new("arrangement-lanes")
}

/// The ID of a clip's node in the accessibility tree.
#[must_use]
pub fn clip_id(clip: bl_model::ClipId) -> Id {
    Id::new(("arrangement-clip", clip))
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

/// The ID of the Automation Mode switch above the track headers (WP-2.12).
#[must_use]
pub fn automation_mode_id() -> Id {
    Id::new("arrangement-automation-mode")
}

/// The ID of the Lock Envelopes switch above the track headers (the
/// automation editing spec, R8).
pub fn lock_envelopes_id() -> Id {
    Id::new("arrangement-lock-envelopes")
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
    Select {
        at: Ticks,
        row: Option<usize>,
        /// Where the drag began, in points, so the lanes it crossed are
        /// known.
        y: f32,
    },
    /// Panning with Cmd-Option (Ctrl-Alt) held.
    Pan,
    /// In the beat-time ruler or the Overview: sideways scrolls, up and
    /// down zooms around where it started.
    Ruler { anchor: Ticks, overview: bool },
    /// In the time ruler: sideways scrolls.
    TimeRuler,
    /// Moving, copying, or resizing clips (WP-1.8b), which [`editing`]
    /// follows.
    Clips,
    /// Dragging a breakpoint or curving a segment on an automation lane
    /// (WP-2.12).
    Automation,
}

/// The Arrangement View's state between frames: caches and gestures, never
/// song or view state. See the [module docs](self).
pub struct ArrangementUi {
    shared: SharedFrame,
    library: PeakLibrary,
    index: ClipIndex,
    /// Whether a take lane header is being dragged, which keeps the keys from
    /// the keymap so Esc reaches the drag (the comping spec, R35).
    header_drag: bool,
    rows: Vec<Row>,
    labels: Vec<Label>,
    shown: Vec<ShownClip>,
    every_track: Vec<TrackId>,
    stats: FrameStats,
    lanes_key: Option<LanesKey>,
    overview_key: Option<OverviewKey>,
    drag: Option<Drag>,
    reported_viewport: Option<(f32, f32)>,
    /// Editing gestures under way (WP-1.8b).
    clips: ClipGestures,
    /// Automation lane gestures under way (WP-2.12).
    automation: AutomationGestures,
    /// The clip and time-selection context menus (WP-1.8b).
    menus: ContextMenus,
    /// The track header's context menu (WP-4.22).
    track_menus: TrackMenus,
    /// The take lane header's context menu (WP-6.22).
    lane_menus: TakeLaneMenus,
    /// The playhead the last frame drew: its position and its x, if it was
    /// in view (WP-1.18).
    playhead: Option<(Ticks, f32)>,
    /// Where the drag under way is snapped to an item (a clip edge, a
    /// locator or a signature marker), which the lanes show as a line
    /// (arrangement-editing R23).
    snap_line: Option<Ticks>,
    /// The locator being dragged in the scrub area, if a drag is under way
    /// (WP-6.24).
    locator_drag: Option<bl_model::LocatorId>,
    /// The time-signature lane's gestures (WP-6.61).
    signature_lane: signatures::SignatureLane,
    arranger_lane: arranger::ArrangerLane,
    /// The point a held press in the scrub area or the beat-time ruler is
    /// looping, until the button is released (WP-6.59; time-editing R25).
    scrub_hold: Option<Ticks>,
    /// The locators the last frame drew, so a new one can take the focus
    /// (WP-6.24; time-editing R3).
    locators_drawn: Vec<bl_model::LocatorId>,
    /// Where the last frame put the Arrangement's mixer, while it shows
    /// (WP-1.9).
    mixer: Option<Rect>,
    /// The clips this frame put in the accessibility tree (WP-1.14).
    described: Vec<bl_model::ClipId>,
    /// Where the last frame put the track lanes (WP-2.12), for working out
    /// an automation lane's rectangle.
    lanes: Option<Rect>,
    /// The fade handles the last frame drew (WP-6.51).
    fades_drawn: fades::Drawn,
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

/// The loop brace's rectangle, in the beat-time ruler.
fn brace_rect(
    ruler: Rect,
    horizontal: Horizontal,
    transport: &bl_shell::arrangement::TransportView,
) -> Rect {
    let left = horizontal.x(transport.loop_start) + f64::from(ruler.left());
    let right = horizontal.x(transport.loop_end()) + f64::from(ruler.left());
    Rect::from_min_max(
        pos2(left as f32, ruler.bottom() - 6.0),
        pos2(right as f32, ruler.bottom() - 1.0),
    )
}

/// The loop brace's ID.
fn brace_id() -> Id {
    Id::new("arrangement-loop-brace")
}

/// The locator under the pointer in the scrub area, if any.
fn locator_under(
    doc: &Document,
    horizontal: Horizontal,
    left: f32,
    pos: Pos2,
) -> Option<bl_model::LocatorId> {
    let x = f64::from(pos.x - left);
    doc.locators()
        .iter()
        .filter(|locator| (horizontal.x(locator.position) - x).abs() <= 6.0)
        .min_by(|a, b| {
            let da = (horizontal.x(a.position) - x).abs();
            let db = (horizontal.x(b.position) - x).abs();
            da.total_cmp(&db)
        })
        .map(|locator| locator.id)
}

/// The Set Locator button's ID.
pub fn set_locator_id() -> Id {
    Id::new("arrangement-set-locator")
}

/// The Previous Locator button's ID.
pub fn previous_locator_id() -> Id {
    Id::new("arrangement-previous-locator")
}

/// The Next Locator button's ID.
pub fn next_locator_id() -> Id {
    Id::new("arrangement-next-locator")
}

/// A locator's ID, in the scrub area.
pub fn locator_id(locator: bl_model::LocatorId) -> Id {
    Id::new(("arrangement-locator", locator))
}

/// A locator being renamed in the scrub area, and the name being typed.
#[derive(Clone, Debug)]
struct Renaming {
    /// The locator.
    locator: bl_model::LocatorId,
    /// The name being typed.
    text: Option<String>,
    /// Whether the field has the keyboard.
    focused: bool,
}

/// Where the rename being typed is kept.
fn renaming_id() -> Id {
    Id::new("arrangement-locator-renaming")
}

/// Starts typing a new name for `locator` in the scrub area, as Cmd-R does
/// (the time editing spec, R8).
pub fn start_rename(ctx: &egui::Context, locator: bl_model::LocatorId) {
    ctx.data_mut(|d| {
        d.insert_temp(
            renaming_id(),
            Some(Renaming {
                locator,
                text: None,
                focused: false,
            }),
        );
    });
}

/// The locator being renamed, if a rename is under way.
fn renaming(ctx: &egui::Context) -> Option<Renaming> {
    ctx.data(|d| d.get_temp::<Option<Renaming>>(renaming_id()))
        .flatten()
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// Plays from `at`, as a click in the scrub area does: the jump waits for the
/// global launch quantisation while the song plays, playback starts there
/// while it's stopped, and Follow resumes (time-editing R11).
fn play_from(events: &mut Vec<UiEvent>, playing: bool, at: Ticks) {
    invoke(events, ids::SET_INSERT_MARKER, json!({ "at": at }));
    invoke(
        events,
        "transport.locate",
        json!({ "position": at, "quantized": true }),
    );
    if !playing {
        invoke(events, "transport.play_from_insert_marker", json!({}));
    }
    invoke(events, ids::FOLLOW, json!({ "paused": false }));
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
            header_drag: false,
            rows: Vec::new(),
            labels: Vec::new(),
            shown: Vec::new(),
            every_track: Vec::new(),
            stats: FrameStats::default(),
            lanes_key: None,
            overview_key: None,
            drag: None,
            reported_viewport: None,
            clips: ClipGestures::default(),
            automation: AutomationGestures::default(),
            menus: ContextMenus::new(),
            track_menus: TrackMenus::default(),
            lane_menus: TakeLaneMenus::default(),
            playhead: None,
            snap_line: None,
            locator_drag: None,
            signature_lane: signatures::SignatureLane::default(),
            arranger_lane: arranger::ArrangerLane::default(),
            scrub_hold: None,
            locators_drawn: Vec::new(),
            mixer: None,
            described: Vec::new(),
            lanes: None,
            fades_drawn: fades::Drawn::default(),
        }
    }

    /// Whether a take lane header is being dragged, so keys go to the drag
    /// rather than the keymap.
    #[must_use]
    pub fn is_dragging_header(&self) -> bool {
        self.header_drag
    }

    /// The stretches of a take-lane clip that its track's main lane uses, as
    /// start and end ticks: what the lanes draw in the track's colour (the
    /// comping spec, R33).
    #[must_use]
    pub fn used_stretches(&self, track: TrackId, clip: ClipId) -> &[(i64, i64)] {
        self.index.used(track, clip)
    }

    /// The fade handles the last frame drew (WP-6.51), and the dotted line
    /// of the selected handle's limit.
    #[must_use]
    pub fn drawn_fades(&self) -> &fades::Drawn {
        &self.fades_drawn
    }

    /// The playhead the last frame drew: the position it showed, and its x
    /// in the window, or `None` if it was out of view. It's the engine's, as
    /// the shell last took it (the control bar transport spec, R14).
    #[must_use]
    pub fn drawn_playhead(&self) -> Option<(Ticks, f32)> {
        self.playhead
    }

    /// The point the last frame drew the snap line at, if a drag was
    /// snapped to an item (arrangement-editing R23).
    #[must_use]
    pub fn drawn_snap_line(&self) -> Option<Ticks> {
        self.snap_line
    }

    /// Where the last frame put the Arrangement's mixer, if it shows: the
    /// main window draws it there (WP-1.9).
    #[must_use]
    pub fn mixer_rect(&self) -> Option<Rect> {
        self.mixer
    }

    /// A track's main automation lane as the last frame drew it (WP-2.12):
    /// `None` while Automation Mode is off, the track is folded, or the
    /// track has no row in view.
    #[must_use]
    pub fn automation_lane_rect(
        &self,
        view: &ArrangementView,
        doc: &Document,
        track: TrackId,
    ) -> Option<Rect> {
        self.automation_lane_rect_of(view, doc, track, 0)
    }

    /// Lane `number` of a track's automation lanes as the last frame drew it:
    /// 0 is the main lane over the track's clips, and 1 and up are the lanes
    /// of its own under it (the automation spec, R24, R25). `None` while it
    /// doesn't show.
    #[must_use]
    pub fn automation_lane_rect_of(
        &self,
        view: &ArrangementView,
        doc: &Document,
        track: TrackId,
        number: usize,
    ) -> Option<Rect> {
        let lanes = self.lanes?;
        let row = view.rows(doc).into_iter().find(|row| row.track == track)?;
        let shown = view.automation_mode()
            && !view.is_folded(track)
            && number <= view.shown_own_lanes(track);
        shown.then(|| automation::lane_rect(&row, number, lanes, view.scroll_y() as f32))
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

    /// The peaks of a media file if they're ready, asking for them if nobody
    /// has: the Sample Editor draws its waveform from the same library the
    /// Arrangement does (sample editor R4). It also lets the library take
    /// the source's news, so peaks become ready while the Session View
    /// shows.
    pub fn peaks_of(&mut self, media: bl_model::ContentHash) -> Option<Arc<bl_media::Peaks>> {
        let mut shared = lock(&self.shared);
        self.library.update(&mut shared.atlas);
        match self.library.get(media, &mut shared.atlas) {
            peaks::Known::Ready { peaks, .. } => Some(Arc::clone(peaks)),
            _ => None,
        }
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
        // The pinned rows: Main, after the return tracks while they show
        // and stay pinned (arrangement-view R23), and the return tracks'
        // rows that scroll with the tracks after the drop area (R18).
        let pinned: Vec<TrackId> = view.pinned_tracks(doc).collect();
        let returns: Vec<TrackId> = view.scrolling_returns(doc).collect();
        // With its mixer showing, the view splits (WP-1.9; L-ARR-018).
        let layout = if shell
            .mixer_view()
            .of(bl_shell::mixer_view::MixerOf::Arrangement)
            .shown
        {
            ArrangementLayout::compute_with_mixer(
                rect,
                view.overview(),
                pinned.len(),
                crate::mixer::sections(shell, bl_shell::mixer_view::MixerOf::Arrangement),
            )
        } else {
            ArrangementLayout::compute(rect, view.overview(), pinned.len())
        };
        // Spec: time-editing R29. The lane shows while the set has changes.
        let layout = if doc.settings().time_signatures.has_changes() {
            layout.with_signature_lane()
        } else {
            layout
        };
        // Spec: arranger-track R2. The lane shows while the set says so.
        let layout = if doc.settings().arranger_track {
            layout.with_arranger_lane()
        } else {
            layout
        };
        self.mixer = layout.mixer;
        let lanes = layout.lanes;
        self.lanes = Some(lanes);

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
                let selected_lanes = view.selection_take_lanes();
                self.stats = scene::build_lanes(
                    &LanesInput {
                        doc,
                        rows: &self.rows,
                        horizontal,
                        lanes,
                        scroll_y,
                        pixels_per_point: ppp,
                        theme,
                        view,
                        selected: view.selected_tracks(),
                        selected_lanes: &selected_lanes,
                        selected_clips: view.selected_clips(),
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
        let lanes_response = self.lanes_input(ui, shell, view, doc, &layout, horizontal, events);
        // WP-1.8b: right-clicking a clip or the lanes opens their menu.
        if lanes_response.secondary_clicked()
            && let Some(pos) = lanes_response.interact_pointer_pos()
        {
            let clip_lanes = Lanes {
                view,
                doc,
                rect: lanes,
                horizontal,
                shown: &self.shown,
                rows: &self.rows,
                scroll_y,
                draw_mode: shell.clip_view().draw_mode(),
            };
            // Spec: automation-editing R27 (C-TIME-026). A breakpoint offers
            // Edit Value, and the line of an envelope, Add Value.
            let on_envelope = automation::Lanes {
                view,
                doc,
                rect: lanes,
                horizontal,
                rows: &self.rows,
                scroll_y,
            }
            .lane_at(pos, shell)
            .filter(|lane| lane.number > 0 || !self.clip_claims(&clip_lanes, pos))
            .and_then(|lane| match lane.hit(pos) {
                automation::LaneHit::Breakpoint(at) => Some((lane.track, lane.target, at, false)),
                automation::LaneHit::Segment(_) => {
                    let modifiers = ui.input(|i| i.modifiers);
                    let at = automation::click_at(view, doc, &lane, pos, modifiers);
                    Some((lane.track, lane.target, at, true))
                }
                automation::LaneHit::Empty(_) => None,
            });
            match on_envelope {
                Some((track, target, at, line)) => {
                    self.menus.open_envelope(track, target, at, line)
                }
                None => self.menus.open(&clip_lanes, pos, events),
            }
        }
        lanes_response.context_menu(|ui| self.menus.show(ui, theme, shell, events));
        // S-CTX-002: the keys open the same menu, for the selected clips or
        // at the insert marker (the accessibility spec, R8).
        if ui.ctx().memory(|m| m.has_focus(main_view_id()))
            && !view.title_bars_selected()
            && crate::menus::context_menu_keys(ui)
        {
            let clip = self
                .shown
                .iter()
                .find(|s| view.is_clip_selected(s.clip) && lanes.intersects(s.rect));
            let at = clip.map_or_else(
                || {
                    let x = lanes.left() + horizontal.x(view.insert_marker()) as f32;
                    let row = view
                        .selected_tracks()
                        .first()
                        .and_then(|t| self.rows.iter().find(|r| r.track == *t))
                        .map_or(0.0, |r| r.top - scroll_y);
                    pos2(
                        x.clamp(lanes.left(), lanes.right()),
                        (lanes.top() + row + 4.0).clamp(lanes.top(), lanes.bottom()),
                    )
                },
                |shown| shown.rect.intersect(lanes).left_top() + egui::vec2(4.0, 4.0),
            );
            self.menus.open_from_keys(clip.is_some());
            crate::menus::open_keyboard_context_menu(ui.ctx(), lanes_id(), at);
        }
        crate::menus::keyboard_context_menu(&lanes_response, |ui| {
            self.menus.show(ui, theme, shell, events);
        });
        self.draw_overlays(ui, view, doc, theme, &layout, horizontal);
        {
            let clip_lanes = Lanes {
                view,
                doc,
                rect: lanes,
                horizontal,
                shown: &self.shown,
                rows: &self.rows,
                scroll_y: view.scroll_y() as f32,
                draw_mode: shell.clip_view().draw_mode(),
            };
            self.fades_drawn = fades::paint(
                &ui.painter_at(lanes),
                &clip_lanes,
                theme,
                ui.input(|i| i.pointer.latest_pos()),
                self.clips.handles_held(),
                view.selected_fade(),
            );
        }
        self.draw_automation(ui, shell, view, doc, theme, &layout, horizontal);

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
        if let Some(lane) = layout.signature_lane {
            rulers::signature_lane(
                &ui.painter_at(lane),
                lane,
                &doc.settings().time_signatures,
                view.selected_signature(),
                horizontal,
                theme,
            );
        }
        rulers::scrub_area(
            &ui.painter_at(layout.scrub),
            layout.scrub,
            horizontal,
            doc.locators(),
            view.selected_locators(),
            doc.settings().song_start,
            theme,
        );
        rulers::time_ruler(
            &ui.painter_at(layout.time_ruler),
            layout.time_ruler,
            &doc.settings().tempo,
            horizontal,
            theme,
        );
        self.ruler_input(ui, shell, doc, view, &layout, horizontal, theme, events);
        if let Some(lane) = layout.signature_lane {
            self.signature_lane.show(
                ui,
                shell,
                theme,
                lane,
                &mut self.menus,
                &mut self.snap_line,
                events,
            );
        }
        // Spec: arranger-track R2 to R7. The arranger track, over the track
        // lanes' top, and its header.
        if let (Some(lane), Some(header)) = (layout.arranger, layout.arranger_header) {
            rulers::arranger_header(&ui.painter_at(header), header, theme);
            rulers::arranger_lane(
                &ui.painter_at(lane),
                lane,
                doc.sections(),
                view.selected_sections(),
                horizontal,
                theme,
            );
            self.arranger_lane.show(
                ui,
                shell,
                theme,
                lane,
                &mut self.menus,
                &mut self.snap_line,
                events,
            );
        } else {
            self.arranger_lane.tab.clear();
        }

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

        // Arrangement-editing R23: the point a drag snapped to, through the
        // lanes.
        if let Some(at) = self.snap_line {
            let x = lanes.left() + horizontal.x(at) as f32;
            if x >= lanes.left() && x <= lanes.right() {
                rulers::marker(
                    &ui.painter_at(lanes),
                    x,
                    lanes.top(),
                    lanes.bottom(),
                    theme.colors.insert_marker.0,
                );
            }
        }

        // The controls above the track headers, then the headers: the
        // controls come first in the Tab order (accessibility R10).
        self.header_controls(ui, view, doc, theme, &layout, events);
        self.headers(
            ui, shell, theme, &layout, scroll_y, &pinned, &returns, events,
        );
        self.header_drag = take_lanes::is_dragging(ui.ctx());
        // Over the pinned rows' lanes, in the lanes' lower right corner.
        self.grid_indicator(ui, view, doc, theme, layout.grid_indicator);
        self.accessibility(ui, shell, view, doc, &layout);
        self.keys(ui, shell, view, layout.lanes, events);

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

    /// Puts a move of clips back where it began, as Esc does, so the clips
    /// can be carried to the Session View instead (second-window R18, R19).
    pub fn abandon_drag(&mut self, ctx: &egui::Context, events: &mut Vec<UiEvent>) {
        if self.drag == Some(Drag::Clips) && self.clips.moving().is_some() {
            crate::browser::end_clip_drag(ctx);
            self.clips.cancel(events);
            self.drag = None;
        }
    }

    /// The place in the lanes under `pos` that Session clips dropped there
    /// start at (second-window R20): the track under the pointer, and the
    /// position snapped as dragging Arrangement clips snaps it, which the
    /// bypass key (`modifiers`) turns round (I-DRAG-021).
    #[must_use]
    pub fn drop_spot(
        &self,
        shell: &Shell,
        pos: Pos2,
        modifiers: egui::Modifiers,
    ) -> Option<DropSpot> {
        let lanes = self.lanes.filter(|lanes| lanes.contains(pos))?;
        let (view, doc) = (shell.arrangement(), shell.editor().document());
        let row = self.row_at(lanes, view.scroll_y() as f32, pos)?;
        let track = self.rows[row].track;
        if !bl_shell::arrangement::editing::lanes(doc).contains(&track) {
            return None;
        }
        let at = view.horizontal(doc).at(f64::from(pos.x - lanes.left()));
        Some(DropSpot::Arrangement {
            track,
            at: view.snap_drag(doc, at, EditKeys::from(modifiers).bypass),
        })
    }

    /// Whether a clip's title bar, an edge or a fade handle is under `pos`:
    /// in the main automation lane these keep the clip's own gestures, and
    /// the rest of the lane belongs to the envelope (the automation spec,
    /// R24).
    fn clip_claims(&self, lanes: &Lanes<'_>, pos: Pos2) -> bool {
        fades::spot_at(lanes, pos, self.clips.handles_held()).is_some()
            || lanes.hit(pos).is_some_and(|hit| hit.zone != Zone::Body)
    }

    /// The row under a point in the lanes, if any.
    fn row_at(&self, lanes: Rect, scroll_y: f32, pos: Pos2) -> Option<usize> {
        let y = pos.y - lanes.top() + scroll_y;
        let index = bl_shell::arrangement::rows::first_row_at(&self.rows, y);
        (index < self.rows.len() && self.rows[index].top <= y).then_some(index)
    }

    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn lanes_input(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        view: &ArrangementView,
        doc: &Document,
        layout: &ArrangementLayout,
        horizontal: Horizontal,
        events: &mut Vec<UiEvent>,
    ) -> egui::Response {
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
        let keys = EditKeys::from(modifiers);
        // Spec: audio-clips R27. F held shows every clip's fade handles
        // (S-ARR-010, S-ENV-004).
        self.clips.hold_handles(
            ui.input(|i| i.key_down(Key::F) && i.modifiers.is_none())
                && !ui.ctx().text_edit_focused(),
        );
        let clip_lanes = Lanes {
            view,
            doc,
            rect: lanes,
            horizontal,
            shown: &self.shown,
            rows: &self.rows,
            scroll_y,
            draw_mode: shell.clip_view().draw_mode(),
        };
        let automation_lanes = automation::Lanes {
            view,
            doc,
            rect: lanes,
            horizontal,
            rows: &self.rows,
            scroll_y,
        };
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            // WP-2.12: a press on a breakpoint drags it, and Option/Alt on a
            // segment curves it.
            let automation = automation_lanes
                .lane_at(press, shell)
                .filter(|lane| lane.number > 0 || !self.clip_claims(&clip_lanes, press))
                .is_some_and(|lane| self.automation.begin(&lane, press, modifiers));
            self.drag = Some(if pan_keys {
                Drag::Pan
            } else if automation {
                Drag::Automation
            } else if self.clips.begin(&clip_lanes, press, keys, events) {
                // WP-1.8b: a clip's title bar or edge. Spec: clip files R3. A
                // clip moved by its title bar can be dropped on the browser.
                if let Some(clips) = self.clips.moving_clips() {
                    crate::browser::begin_clip_drag(ui.ctx());
                    // Spec: second-window R18, R19. The clips can be carried
                    // to the Session View.
                    events.push(UiEvent::ClipDrag(ClipDragEvent::Begin(
                        DragSource::Arrangement(clips.to_vec()),
                    )));
                }
                Drag::Clips
            } else if keys.extend {
                // Shift-drag extends the time selection (I-DRAG-010).
                let (at, row) = editing::extend_from(&clip_lanes, view.snap(doc, at(press)));
                Drag::Select {
                    at,
                    row: row.or_else(|| self.row_at(lanes, scroll_y, press)),
                    y: press.y,
                }
            } else {
                Drag::Select {
                    at: view.snap(doc, at(press)),
                    row: self.row_at(lanes, scroll_y, press),
                    y: press.y,
                }
            });
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()));
        if response.dragged() {
            match self.drag {
                Some(Drag::Clips) => {
                    if let Some(pos) = pointer {
                        self.clips.update(&clip_lanes, pos, keys, events);
                    }
                }
                Some(Drag::Automation) => {
                    if let Some(pos) = pointer {
                        self.automation
                            .update(view, doc, horizontal, pos, modifiers, events);
                    }
                }
                Some(Drag::Pan) => {
                    // S-ZOOM-007, S-ARR-024: the content follows the pointer.
                    let delta = response.drag_delta();
                    if delta != egui::Vec2::ZERO {
                        invoke(events, ids::SCROLL, json!({ "x": -delta.x, "y": -delta.y }));
                    }
                }
                Some(Drag::Select { at: from, row, y }) => {
                    if let Some(pos) = response.interact_pointer_pos() {
                        // I-DRAG-010: time across the tracks dragged over.
                        let to = view.snap(doc, at(pos));
                        let here = self.row_at(lanes, scroll_y, pos).or(row);
                        // The rows are rebuilt every frame and a track can go
                        // mid-drag (an agent deletes it, a group folds), so a
                        // row index from the press may be past the end.
                        let last = self.rows.len().checked_sub(1);
                        let (tracks, lanes) = match (row, here, last) {
                            (Some(a), Some(b), Some(last)) => {
                                let (a, b) = (a.min(b).min(last), a.max(b).min(last));
                                (
                                    self.rows[a..=b].iter().map(|r| r.track).collect(),
                                    // Spec: automation-editing R1. The
                                    // selection names the lanes it covers.
                                    lanes_crossed(
                                        &self.rows,
                                        a,
                                        b,
                                        y - lanes.top() + scroll_y,
                                        pos.y - lanes.top() + scroll_y,
                                        view,
                                        doc,
                                    ),
                                )
                            }
                            _ => (Vec::new(), Vec::new()),
                        };
                        invoke(
                            events,
                            ids::SELECT_TIME,
                            json!({ "start": from, "end": to, "tracks": tracks, "lanes": lanes }),
                        );
                    }
                }
                _ => {}
            }
        }
        // Spec: view-options R1, arrangement-view R19. A middle-button drag
        // pans the lanes, as Cmd-Option dragging does.
        if let Some(delta) = crate::middle::pan(ui, lanes) {
            invoke(events, ids::SCROLL, json!({ "x": -delta.x, "y": -delta.y }));
        }
        // Esc abandons a clip drag; egui ends the drag on it too.
        if self.drag == Some(Drag::Clips) && ui.input(|i| i.key_pressed(Key::Escape)) {
            if self.clips.moving().is_some() {
                crate::browser::end_clip_drag(ui.ctx());
            }
            self.clips.cancel(events);
            self.drag = None;
        }
        if self.drag == Some(Drag::Automation) && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.automation.cancel(events);
            self.drag = None;
        }
        if response.drag_stopped() {
            match self.drag {
                Some(Drag::Clips) => {
                    // Spec: clip files R3. Released over a folder or label of
                    // the browser, a moved clip goes back where it was and
                    // is saved as a clip file there.
                    let moving = self.clips.moving();
                    let target = moving.and_then(|_| crate::browser::end_clip_drag(ui.ctx()));
                    match (moving, target) {
                        (Some((clip, count)), Some(target)) => {
                            self.clips.cancel(events);
                            if count == 1 {
                                crate::browser::save_clip(ui.ctx(), shell, clip, &target, events);
                            } else if target == crate::browser::ClipDropTarget::GroovePool {
                                // Spec: grooves R28. The selected clips each
                                // give a groove.
                                invoke(events, bl_shell::groove_commands::EXTRACT, json!({}));
                            }
                        }
                        _ => self.clips.end(&clip_lanes, pointer, keys, events),
                    }
                }
                Some(Drag::Automation) => {
                    self.automation
                        .end(view, doc, horizontal, pointer, modifiers, events);
                }
                _ => {}
            }
            self.drag = None;
        }
        self.snap_line = pointer
            .filter(|_| response.dragged() && self.drag == Some(Drag::Clips))
            .and_then(|pos| self.clips.snap_line(&clip_lanes, pos, keys));
        if let Some(hover) = response.hover_pos()
            && !pan_keys
            && let Some(icon) = self.clips.cursor(&clip_lanes, hover, keys)
        {
            ui.ctx().set_cursor_icon(icon);
        }
        if response.double_clicked() {
            if let Some(pos) = response.interact_pointer_pos()
                && let Some(lane) = automation_lanes
                    .lane_at(pos, shell)
                    .filter(|lane| lane.number > 0 || !self.clip_claims(&clip_lanes, pos))
            {
                // WP-2.12: Alt/Option double-click straightens a segment; off
                // a segment it adds a breakpoint.
                let _ = self
                    .automation
                    .double_click(view, doc, &lane, pos, modifiers, events);
            } else if let Some(pos) = response.interact_pointer_pos()
                && let Some(hit) = clip_lanes.hit(pos)
            {
                // Spec: detail-views R4 (I-CLICK-005). Double-clicking a clip
                // selects it and shows the Clip View.
                invoke(
                    events,
                    bl_shell::editing_commands::SELECT_CLIPS,
                    json!({ "clips": [hit.clip] }),
                );
                events.extend(
                    crate::main_window::open_view(shell, bl_shell::commands::DetailView::Clip)
                        .into_iter()
                        .map(UiEvent::Invoke),
                );
            }
        } else if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            // WP-2.12: clicking a breakpoint deletes it, and a segment adds
            // one there.
            let lane_here = automation_lanes
                .lane_at(pos, shell)
                .filter(|lane| lane.number > 0 || !self.clip_claims(&clip_lanes, pos));
            let handled = lane_here.as_ref().is_some_and(|lane| {
                self.automation
                    .click(view, doc, lane, pos, modifiers, events)
            });
            if !handled && !self.clips.click(&clip_lanes, pos, keys, events) {
                // I-CLICK-011: the insert marker, and the track under it; on
                // a take lane, the lane with it (the comping spec, R26).
                let place = clip_lanes.place_at(pos);
                let mut args = json!({ "at": view.snap(doc, at(pos)) });
                match place {
                    Some((row, Some(lane))) => {
                        args["track"] = json!(self.rows[row].track);
                        args["lane"] = json!(lane);
                    }
                    Some((row, None)) => args["track"] = json!(self.rows[row].track),
                    None => {}
                }
                // Spec: automation-editing R1. A click on an automation
                // lane's background selects the lane, which gets the
                // keyboard.
                if let Some(lane) = &lane_here {
                    args["automation_lane"] = json!(lane.number);
                }
                invoke(events, ids::SET_INSERT_MARKER, args);
            }
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
                    // height of the track under the pointer; over a take
                    // lane, the selected take lanes' (I-SCROLL-006), or the
                    // lane under the pointer if none is selected.
                    if let Some((row, lane)) = clip_lanes.place_at(pos) {
                        match lane {
                            Some(lane) if view.selected_take_lanes().is_empty() => invoke(
                                events,
                                ids::SET_TRACK_HEIGHT,
                                json!({ "take_lanes": [lane], "delta": scroll.y }),
                            ),
                            Some(_) => {
                                invoke(events, ids::SET_TRACK_HEIGHT, json!({ "delta": scroll.y }))
                            }
                            None => invoke(
                                events,
                                ids::SET_TRACK_HEIGHT,
                                json!({ "tracks": [self.rows[row].track], "delta": scroll.y }),
                            ),
                        }
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
        response
    }

    fn draw_overlays(
        &self,
        ui: &Ui,
        view: &ArrangementView,
        doc: &Document,
        theme: &Theme,
        layout: &ArrangementLayout,
        horizontal: Horizontal,
    ) {
        let lanes = layout.lanes;
        let painter = ui.painter_at(lanes);
        let scroll_y = view.scroll_y() as f32;
        let x_of = |at: Ticks| lanes.left() + horizontal.x(at) as f32;
        let selected = view.selected_tracks();
        // The selected tracks' lanes: each track's own lane, or the lanes
        // the selection names there, take lanes among them (the comping
        // spec, R26).
        let rows_of_selection = || {
            let mut spans: Vec<(f32, f32)> = Vec::new();
            for row in self.rows.iter().filter(|row| selected.contains(&row.track)) {
                let top = lanes.top() + row.top - scroll_y;
                let named: Vec<&Lane> = view
                    .time_selection_lanes()
                    .iter()
                    .filter(|lane| {
                        lane.track() == row.track && !matches!(lane, Lane::Automation { .. })
                    })
                    .collect();
                if named.is_empty() {
                    spans.push((top, top + row.height));
                    continue;
                }
                for lane in named {
                    match lane {
                        Lane::Take { lane, .. } => {
                            if let Some(held) = view
                                .take_lane_rows(doc, row)
                                .find(|held| held.lane == *lane)
                            {
                                let y = lanes.top() + held.top - scroll_y;
                                spans.push((y, y + held.height));
                            }
                        }
                        _ => spans.push((top, top + row.height)),
                    }
                }
            }
            spans.into_iter()
        };
        // The time selection, over the selected tracks; selected clips show
        // their own highlight instead (WP-1.8b).
        if let Some((start, end)) = view.time_selection()
            && view.selected_clips().is_empty()
        {
            let (x0, x1) = (x_of(start), x_of(end));
            for (top, bottom) in rows_of_selection() {
                painter.rect_filled(
                    Rect::from_min_max(pos2(x0, top), pos2(x1, bottom)),
                    0.0,
                    theme.colors.time_selection.0.gamma_multiply(0.28),
                );
            }
        } else if view.time_selection().is_none() {
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
    fn draw_automation(
        &self,
        ui: &Ui,
        shell: &Shell,
        view: &ArrangementView,
        doc: &Document,
        theme: &Theme,
        layout: &ArrangementLayout,
        horizontal: Horizontal,
    ) {
        if !view.automation_mode() {
            return;
        }
        let lanes = layout.lanes;
        let scroll_y = view.scroll_y() as f32;
        let painter = ui.painter_at(lanes);
        let automation_lanes = automation::Lanes {
            view,
            doc,
            rect: lanes,
            horizontal,
            rows: &self.rows,
            scroll_y,
        };
        let keys = shell
            .keyboard_envelope()
            .filter(|keys| keys.envelope == bl_shell::envelope_commands::EnvelopeKeys::Lane);
        let keyed = view.keyboard_lane_number();
        let time = ui.input(|i| i.time);
        let typed = crate::envelope_keys::typed(ui.ctx());
        for row in &self.rows {
            // The main lane draws over the track's clips, and the lanes of
            // its own under them (the automation spec, R24, R25); a folded
            // track shows none.
            for lane_number in 0..=view.shown_own_lanes(row.track) {
                if row.folded {
                    break;
                }
                let rect = automation::lane_rect(row, lane_number, lanes, scroll_y);
                if !rect.intersects(lanes) {
                    continue;
                }
                if lane_number > 0 {
                    automation::paint_lane(&painter, rect, theme);
                }
                // Spec: automation-editing R21, R22. The lane that has the
                // keyboard shows the insert marker with its value, and the
                // focused and selected breakpoints.
                let own = keys
                    .as_ref()
                    .filter(|_| keyed == Some((row.track, lane_number)));
                let mut focused = None;
                let lane = automation_lanes
                    .lanes_of(row, shell)
                    .into_iter()
                    .find(|lane| lane.number == lane_number);
                if let Some(lane) = &lane {
                    automation::paint_envelope(&painter, lane, theme, own, time);
                    if let Some(keys) = own {
                        focused = automation::paint_keys(
                            ui,
                            &painter,
                            theme,
                            shell,
                            lane,
                            keys,
                            typed.as_ref().map(|typed| typed.text.as_str()),
                        );
                        if keys.selected.is_some() {
                            // The selected breakpoint pulses.
                            ui.ctx()
                                .request_repaint_after(crate::envelope_keys::PULSE_FRAME);
                        }
                    }
                    let outline = ui.input(|i| {
                        let pointer = i.pointer.interact_pos()?;
                        self.automation.outline(
                            (row.track, lane_number),
                            view,
                            doc,
                            pointer,
                            i.modifiers,
                        )
                    });
                    automation::paint_handles(&painter, ui, lane, theme, outline);
                }
                // The lane's accessible node, a group named for the
                // parameter it shows and valued as the envelope is at the
                // insert marker, with the focused or selected breakpoint as
                // its active descendant (R28).
                let id = automation::lane_id(row.track, lane_number);
                let _ = ui.interact(rect, id, Sense::hover());
                let name = match view
                    .lane_choice(row.track, lane_number)
                    .and_then(|choice| choice.target)
                {
                    Some(target) => automation::target_label(shell, doc, target),
                    None => "None".to_owned(),
                };
                ui.ctx().accesskit_node_builder(id, |node| {
                    node.set_role(Role::Group);
                    node.set_label(format!("{name} Automation"));
                    if let Some(keys) = own {
                        node.set_value(keys.value.clone());
                    }
                    if let Some(focused) = focused {
                        node.set_active_descendant(focused.accesskit_id());
                    }
                });
            }
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
        let content = view.content_height(doc, &self.rows).max(1.0);
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

    /// Starts and ends the held loop of a press in the scrub area or the
    /// beat-time ruler (time-editing R25, R27): once the button has been down
    /// for [`SCRUB_HOLD_SECONDS`] on `press`, a point of the area, `transport.scrub`
    /// loops there, with `permanent` or Shift held, and releasing the button
    /// ends it. Returns whether a hold ended this frame, so its release isn't
    /// also a click.
    fn scrub_hold(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        press: Option<Ticks>,
        permanent: bool,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        let (down, shift, started, now) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.modifiers.shift,
                i.pointer.press_start_time(),
                i.time,
            )
        });
        if let Some(position) = self.scrub_hold {
            if down {
                return false;
            }
            self.scrub_hold = None;
            let args = json!({ "position": position, "held": false });
            invoke(events, "transport.scrub", args);
            return true;
        }
        if let (true, true, Some(position), Some(started)) =
            (down, response.is_pointer_button_down_on(), press, started)
        {
            if now - started < SCRUB_HOLD_SECONDS {
                // Nothing else wakes the frame that finds the press long enough.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(50));
            } else if permanent || shift {
                self.scrub_hold = Some(position);
                let args = json!({ "position": position, "held": true });
                invoke(events, "transport.scrub", args);
            }
        }
        false
    }

    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn ruler_input(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        doc: &Document,
        view: &ArrangementView,
        layout: &ArrangementLayout,
        horizontal: Horizontal,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
    ) {
        // Esc abandons a locator drag, as it does a clip drag; egui ends the
        // drag on it too (the time editing spec, R7).
        if ui.input(|i| i.key_pressed(Key::Escape)) && self.locator_drag.take().is_some() {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
        let ruler = layout.beat_ruler;
        let at = |pos: Pos2| horizontal.at(f64::from(pos.x - ruler.left()));
        let response = ui.interact(ruler, beat_ruler_id(), Sense::click_and_drag());
        // Spec: time-editing R27. A Shift-hold in the ruler loops, and the
        // ruler doesn't scroll or zoom while it does.
        let press = ui.input(|i| i.pointer.press_origin()).map(at);
        let released = self.scrub_hold(ui, &response, press, false, events);
        if response.double_clicked() {
            // I-CLICK-016.
            invoke(events, ids::ZOOM_TO_SELECTION, json!({}));
        } else if response.clicked() && !released {
            if ui.input(|i| i.modifiers.shift) {
                // Spec: time-editing R27 (I-CLICK-014). Shift-click scrubs.
                if let Some(pos) = response.interact_pointer_pos() {
                    play_from(events, view.transport().playing, view.snap(doc, at(pos)));
                }
            } else {
                // Clicking the ruler pauses Follow while the song plays.
                invoke(events, ids::FOLLOW, json!({ "paused": true }));
            }
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
            && self.scrub_hold.is_none()
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

        let scrub = ui.interact(layout.scrub, scrub_id(), Sense::click_and_drag());
        let scrub_at = |pos: Pos2| horizontal.at(f64::from(pos.x - layout.scrub.left()));
        let locator_at = |pos: Pos2| locator_under(doc, horizontal, layout.scrub.left(), pos);
        let pointer = scrub.interact_pointer_pos();
        // Spec: time-editing R25 to R27. A press held away from a locator
        // loops a span, with Permanent Scrub Areas on or Shift held.
        let press = ui
            .input(|i| i.pointer.press_origin())
            .filter(|pos| locator_at(*pos).is_none())
            .map(scrub_at);
        // The ruler's call above may have taken this area's release: the
        // hold is the view's, not the area's.
        let released =
            self.scrub_hold(ui, &scrub, press, shell.permanent_scrub_areas(), events) | released;
        // The locators in the scrub area are controls the keyboard reaches
        // (accessibility R10).
        for locator in doc.locators() {
            let x = (horizontal.x(locator.position) + f64::from(layout.scrub.left())) as f32;
            // The locator being dragged keeps its widget, which alone sees
            // the release.
            if self.locator_drag != Some(locator.id)
                && (x < layout.scrub.left() - 40.0 || x > layout.scrub.right() + 40.0)
            {
                continue;
            }
            let id = locator_id(locator.id);
            let rect = Rect::from_min_max(
                pos2(x - 6.0, layout.scrub.top()),
                pos2(x + 6.0, layout.scrub.bottom()),
            );
            let response = ui.interact(rect, id, Sense::click_and_drag());
            if response.secondary_clicked() {
                // Spec: time-editing R9, R10. A locator's own menu.
                invoke(
                    events,
                    locator_ids::SELECT,
                    json!({ "locators": [locator.id] }),
                );
                self.menus.open_scrub(Some(locator.id), locator.position);
            } else if response.double_clicked() {
                // Spec: time-editing R5. Double-clicking a locator while
                // stopped selects it and starts playback from it.
                invoke(
                    events,
                    locator_ids::SELECT,
                    json!({ "locators": [locator.id] }),
                );
                invoke(events, locator_ids::JUMP, json!({ "locator": locator.id }));
                if !view.transport().playing {
                    invoke(events, "transport.play_from_insert_marker", json!({}));
                }
            } else if response.clicked() {
                // Spec: time-editing R5. Clicking a locator selects it
                // alone and jumps to it.
                invoke(
                    events,
                    locator_ids::SELECT,
                    json!({ "locators": [locator.id] }),
                );
                invoke(events, locator_ids::JUMP, json!({ "locator": locator.id }));
            }
            // The menu must be drawn every frame it is open, not only the
            // frame it opened.
            response.context_menu(|ui| self.menus.show(ui, theme, shell, events));
            let name = locator.name.clone();
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
            crate::info_text::describe(ui.ctx(), &response, shell, InfoTarget::Locator(locator.id));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Button);
                node.set_label(name.as_str());
                node.set_value(
                    doc.settings()
                        .time_signatures
                        .to_bbs(locator.position)
                        .to_string(),
                );
                node.set_selected(view.is_locator_selected(locator.id));
            });
            crate::focus::register_above(
                ui.ctx(),
                id,
                bl_shell::Area::MainView,
                crate::focus::ABOVE_LOCATORS,
            );
            // Spec: time-editing R3, R12, accessibility R10. A new locator
            // takes the keyboard focus, and Tab reaching a locator selects
            // it.
            if shell.tab_navigation()
                && !self.locators_drawn.contains(&locator.id)
                && view.is_locator_selected(locator.id)
            {
                response.request_focus();
            }
            if response.has_focus() && !view.is_locator_selected(locator.id) {
                invoke(
                    events,
                    locator_ids::SELECT,
                    json!({ "locators": [locator.id] }),
                );
            }
            // Spec: time-editing R8. The name being typed, over the
            // locator's name: Return keeps it and Esc drops it.
            if let Some(mut renaming) = renaming(ui.ctx()).filter(|r| r.locator == locator.id) {
                let text = renaming.text.get_or_insert_with(|| locator.name.clone());
                let edit = ui.put(
                    Rect::from_min_max(
                        pos2(x + 6.0, layout.scrub.center().y - 9.0),
                        pos2(x + 126.0, layout.scrub.center().y + 9.0),
                    ),
                    egui::TextEdit::singleline(text)
                        .id(Id::new(("arrangement-locator-rename", locator.id)))
                        .font(theme.small_font()),
                );
                if !renaming.focused {
                    edit.request_focus();
                    renaming.focused = true;
                }
                let (enter, escape) =
                    ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                if edit.lost_focus() || enter || escape {
                    if enter || (edit.lost_focus() && !escape) {
                        events.push(UiEvent::Invoke(Invocation::new(
                            locator_ids::RENAME,
                            json!({ "locators": [locator.id], "name": text }),
                        )));
                    }
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(renaming_id(), None::<Renaming>));
                } else {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(renaming_id(), Some(renaming)));
                }
            }
            // Spec: time-editing R7. Dragging a locator moves it, and the
            // whole drag is one undo step.
            if response.drag_started() {
                self.locator_drag = Some(locator.id);
            }
            if response.dragged()
                && self.locator_drag == Some(locator.id)
                && let Some(pos) = response.interact_pointer_pos()
            {
                // Spec: arrangement-editing R22. It snaps to clip edges,
                // other locators and signature markers as well as the grid.
                let mut carried = view.selected_locators().to_vec();
                carried.push(locator.id);
                let snapped = view.snap_to_items(
                    doc,
                    scrub_at(pos),
                    Ticks::ZERO,
                    Dragged {
                        locators: &carried,
                        ..Dragged::default()
                    },
                    horizontal.ticks_per_point,
                    ui.input(|i| EditKeys::from(i.modifiers).bypass),
                );
                self.snap_line = snapped.line;
                let to = snapped.at;
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Update,
                    invocation: Some(Invocation::new(
                        locator_ids::MOVE,
                        json!({ "locator": locator.id, "to": to }),
                    )),
                });
            }
            if response.drag_stopped() && self.locator_drag.take() == Some(locator.id) {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: None,
                });
            }
        }
        // A drag whose locator is gone (deleted mid-drag) never sees its
        // release: end it once the pointer is up.
        if self.locator_drag.is_some() && ui.input(|i| !i.pointer.any_down()) {
            self.locator_drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: None,
            });
        }
        // The locators this frame drew, so the next frame can tell a new
        // one and give it the focus (time-editing R3).
        self.locators_drawn = doc.locators().iter().map(|locator| locator.id).collect();
        if scrub.double_clicked() {
            // Spec: time-editing R5. Double-clicking a locator while stopped
            // selects it and starts playback from it.
            if let Some(locator) = pointer.and_then(locator_at) {
                invoke(
                    events,
                    locator_ids::SELECT,
                    json!({ "locators": [locator] }),
                );
                invoke(events, locator_ids::JUMP, json!({ "locator": locator }));
                if !view.transport().playing {
                    invoke(events, "transport.play_from_insert_marker", json!({}));
                }
            }
        } else if scrub.clicked() && !released {
            if let Some(locator) = pointer.and_then(locator_at) {
                // Spec: time-editing R5. Clicking a locator selects it
                // alone and jumps to it.
                invoke(
                    events,
                    locator_ids::SELECT,
                    json!({ "locators": [locator] }),
                );
                invoke(events, locator_ids::JUMP, json!({ "locator": locator }));
            } else {
                // Spec: time-editing R11. Clicking the scrub area away from
                // a locator plays from that point, on the editing grid
                // while snapping is on, and resumes Follow.
                let Some(pos) = pointer else {
                    return;
                };
                let at = view.snap(doc, scrub_at(pos));
                if shell.permanent_scrub_areas() || ui.input(|i| i.modifiers.shift) {
                    play_from(events, view.transport().playing, at);
                } else {
                    // Spec: time-editing R26. With Permanent Scrub Areas off, a
                    // click places the insert marker, as one in a lane does.
                    invoke(events, ids::SET_INSERT_MARKER, json!({ "at": at }));
                }
            }
        }
        // The scrub area's own menu (C-TIME-022), away from a locator.
        let on_locator = pointer.and_then(locator_at).is_some();
        if scrub.secondary_clicked() && !on_locator {
            // The Add Locator in it adds one where the area was
            // right-clicked, on the editing grid (the time editing spec,
            // R3).
            let at = pointer.map_or(Ticks::ZERO, |pos| view.snap(doc, scrub_at(pos)));
            self.menus.open_scrub(None, at);
        }
        if !on_locator {
            scrub.context_menu(|ui| self.menus.show(ui, theme, shell, events));
        }
        // The loop brace's menu (C-TIME-023).
        let brace = brace_rect(layout.beat_ruler, horizontal, &view.transport());
        let brace = ui.interact(brace, brace_id(), Sense::click());
        if brace.secondary_clicked() {
            self.menus.open_brace();
        }
        brace.context_menu(|ui| self.menus.show(ui, theme, shell, events));
        scrub.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Scrub Area"));
        ui.ctx().accesskit_node_builder(scrub.id, |node| {
            node.set_role(Role::Group);
            node.set_description(
                "Clicking here plays from that point; the locators sit here.".to_owned(),
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
        shell: &Shell,
        theme: &Theme,
        layout: &ArrangementLayout,
        scroll_y: f32,
        pinned: &[TrackId],
        returns: &[TrackId],
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.arrangement();
        let doc = shell.editor().document();
        let column = layout.headers;
        let visible = rows_between(&self.rows, scroll_y, scroll_y + column.height());
        let selected = view.selected_tracks();
        let title_bars = view.title_bars_selected();
        // S-CTX-002 with the view focused opens the selected title bar's
        // menu (the accessibility spec, R8), or the selected take lane
        // header's (the comping spec, R29).
        let keys = title_bars && ui.ctx().memory(|m| m.has_focus(main_view_id()));
        let keys_for_lanes = !view.selected_take_lanes().is_empty()
            && ui.ctx().memory(|m| m.has_focus(main_view_id()));
        let mut header_ui = ui.new_child(egui::UiBuilder::new().max_rect(column));
        header_ui.set_clip_rect(column);
        // The names Tab moves between while one is typed, and the take lane
        // headers in order, for Shift-click (the comping spec, R26, R28).
        let mut names: Vec<Named> = Vec::new();
        let mut lane_order: Vec<bl_model::TakeLaneId> = Vec::new();
        if self.rows.iter().any(|row| row.take > 0.0) {
            for row in &self.rows {
                names.push(Named::Track(row.track));
                for lane in view.take_lane_rows(doc, row) {
                    names.push(Named::Lane(lane.lane));
                    lane_order.push(lane.lane);
                }
            }
        }
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
                    shell,
                    track,
                    rect: Rect::from_min_max(
                        pos2(column.left(), top),
                        pos2(column.right(), top + row.height),
                    ),
                    activator: (visible.start + index + 1).to_string(),
                    colour: scene::track_colour(theme, doc, row.track),
                    folded: row.folded,
                    selected: is_selected,
                    title_selected: is_selected && title_bars,
                    foldable: true,
                    resizable: !view.optimize_height(),
                    menus: &self.track_menus,
                    keys: keys && selected.first() == Some(&row.track),
                    show_in_out: view.show_in_out(),
                    show_volume: view.show_volume(),
                    take_toggle: view
                        .take_toggle_shown(doc, row.track)
                        .then(|| view.shows_take_lanes(row.track)),
                    automation: view.automation_mode().then(|| {
                        (!view.own_lanes(row.track).is_empty())
                            .then(|| !view.own_lanes_hidden(row.track))
                    }),
                },
                &self.every_track,
                events,
            );
            // WP-6.22: a header beside each take lane under the track
            // (L-ARR-020).
            let track_lanes: Vec<(bl_model::TakeLaneId, f32, f32)> = view
                .take_lane_rows(doc, row)
                .map(|lane| {
                    let top = column.top() + lane.top - scroll_y;
                    (lane.lane, top, top + lane.height)
                })
                .collect();
            for lane in view.take_lane_rows(doc, row) {
                let Some(held) = doc.take_lane(lane.lane) else {
                    continue;
                };
                let top = column.top() + lane.top - scroll_y;
                if top > column.bottom() || top + lane.height < column.top() {
                    continue;
                }
                take_lanes::header(
                    &mut header_ui,
                    theme,
                    &self.lane_menus,
                    &take_lanes::LaneHeader {
                        shell,
                        track: row.track,
                        lane: lane.lane,
                        name: &held.name,
                        rect: Rect::from_min_max(
                            pos2(column.left(), top),
                            pos2(column.right(), top + lane.height),
                        ),
                        colour: scene::track_colour(theme, doc, row.track),
                        selected: view.is_take_lane_selected(lane.lane),
                        auditioned: doc
                            .track(row.track)
                            .is_some_and(|t| t.audition() == Some(lane.lane)),
                        show_in_out: view.show_in_out(),
                        keys: keys_for_lanes
                            && view.selected_take_lanes().first() == Some(&lane.lane),
                        order: &lane_order,
                        names: &names,
                        track_lanes: &track_lanes,
                    },
                    events,
                );
            }
            // Spec: automation R25. A header beside each lane of the track's
            // own: its choosers and its hide button.
            for number in 1..=view.shown_own_lanes(row.track) {
                let (lane_top, lane_bottom) = row.lane_span(number);
                let rect = Rect::from_min_max(
                    pos2(column.left(), column.top() + lane_top - scroll_y),
                    pos2(
                        column.left() + header_columns::title_width(view.show_in_out()),
                        column.top() + lane_bottom - scroll_y,
                    ),
                );
                if rect.bottom() < column.top() || rect.top() > column.bottom() {
                    continue;
                }
                automation::own_lane_header(
                    &mut header_ui,
                    shell,
                    theme,
                    (row.track, number),
                    rect,
                    events,
                );
            }
        }
        // The return tracks' rows, after the drop area under the last
        // track, scrolling with the tracks (the arrangement view spec, R18).
        if !returns.is_empty() {
            let gap = if view.optimize_height() {
                0.0
            } else {
                DROP_AREA
            };
            let first =
                layout.lanes.top() + self.rows.last().map_or(0.0, Row::bottom) + gap - scroll_y;
            for (index, id) in returns.iter().enumerate() {
                let Some(track) = doc.track(*id) else {
                    continue;
                };
                let top = first + FOLDED_HEIGHT * index as f32;
                if top > layout.lanes.bottom() || top + FOLDED_HEIGHT < layout.lanes.top() {
                    continue;
                }
                let lane = Rect::from_min_max(
                    pos2(layout.lanes.left(), top),
                    pos2(layout.lanes.right(), top + FOLDED_HEIGHT),
                );
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
                        shell,
                        track,
                        rect: Rect::from_min_max(
                            pos2(layout.headers.left(), top),
                            pos2(layout.headers.right(), top + FOLDED_HEIGHT),
                        ),
                        activator: char::from(
                            b'A' + (doc.returns().position(|r| r == *id).unwrap_or(index) % 26)
                                as u8,
                        )
                        .to_string(),
                        colour: scene::track_colour(theme, doc, *id),
                        folded: true,
                        selected: is_selected,
                        title_selected: is_selected && title_bars,
                        foldable: false,
                        resizable: false,
                        menus: &self.track_menus,
                        keys: keys && selected.first() == Some(id),
                        show_in_out: view.show_in_out(),
                        show_volume: view.show_volume(),
                        take_toggle: None,
                        automation: None,
                    },
                    &[],
                    events,
                );
            }
        }
        // The returns and Main, pinned under the lanes (L-ARR-014).
        let return_count = doc.returns().count();
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
            // A return's own letter, and Main's after the returns
            // (L-ARR-006), which keep their places while the return tracks
            // are hidden (R23).
            let letter = doc.returns().position(|r| r == *id).unwrap_or(return_count) % 26;
            headers::header(
                ui,
                theme,
                &Header {
                    shell,
                    track,
                    rect: Rect::from_min_max(
                        pos2(layout.pinned_headers.left(), row.top()),
                        row.max,
                    ),
                    activator: char::from(b'A' + letter as u8).to_string(),
                    colour: if track.kind() == TrackKind::Main {
                        theme.colors.control_pressed.0
                    } else {
                        scene::track_colour(theme, doc, *id)
                    },
                    folded: true,
                    selected: is_selected,
                    title_selected: is_selected && title_bars,
                    foldable: false,
                    resizable: false,
                    menus: &self.track_menus,
                    keys: keys && selected.first() == Some(id),
                    show_in_out: view.show_in_out(),
                    show_volume: view.show_volume(),
                    take_toggle: None,
                    automation: None,
                },
                &[],
                events,
            );
        }
    }

    /// The selected clip this frame put in the accessibility tree, which
    /// the view reports as its active descendant, so a screen reader reads
    /// the clip the keys select (the accessibility spec, R2).
    #[must_use]
    pub fn selected_clip_node(&self, view: &ArrangementView) -> Option<Id> {
        self.described
            .iter()
            .find(|clip| view.is_clip_selected(**clip))
            .map(|clip| clip_id(*clip))
    }

    /// The node of the automation lane that has the keyboard, which the view
    /// reports as its active descendant while it does, so a screen reader
    /// reads the parameter and the value at the insert marker (the automation
    /// editing spec, R28).
    #[must_use]
    pub fn keyboard_lane_node(&self, view: &ArrangementView) -> Option<Id> {
        view.keyboard_lane_number()
            .map(|(track, number)| automation::lane_id(track, number))
    }

    fn header_controls(
        &self,
        ui: &mut Ui,
        view: &ArrangementView,
        doc: &Document,
        theme: &Theme,
        layout: &ArrangementLayout,
        events: &mut Vec<UiEvent>,
    ) {
        // L-ARR-010: Set Locator, reading Delete Locator while a locator is
        // selected, and Previous and Next Locator, live (WP-6.24).
        let locator_selected = !view.selected_locators().is_empty();
        for (rect, id, name, text, command, rank) in [
            (
                layout.set_locator,
                set_locator_id(),
                if locator_selected {
                    "Delete Locator"
                } else {
                    "Set Locator"
                },
                if locator_selected { "Del" } else { "Set" },
                if locator_selected {
                    locator_ids::DELETE
                } else {
                    locator_ids::ADD
                },
                crate::focus::ABOVE_SET_LOCATOR,
            ),
            (
                layout.previous_locator,
                previous_locator_id(),
                "Previous Locator",
                "◀",
                locator_ids::JUMP_PREVIOUS,
                crate::focus::ABOVE_PREVIOUS_LOCATOR,
            ),
            (
                layout.next_locator,
                next_locator_id(),
                "Next Locator",
                "▶",
                locator_ids::JUMP_NEXT,
                crate::focus::ABOVE_NEXT_LOCATOR,
            ),
        ] {
            let response = ui.interact(rect, id, Sense::click());
            let painter = ui.painter_at(rect.expand(1.0));
            let fill = if response.hovered() {
                theme.colors.control_hover.0
            } else {
                theme.colors.control.0
            };
            painter.rect_filled(rect, theme.sizes.corner, fill);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                theme.small_font(),
                theme.colors.text.0,
            );
            if response.clicked() {
                invoke(events, command, json!({}));
            }
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Button);
                node.set_label(name);
            });
            // Spec: accessibility R10. The controls above the track headers
            // come first in the Arrangement's Tab order.
            crate::focus::register_above(ui.ctx(), id, bl_shell::Area::MainView, rank);
        }
        for (rect, name, text, package) in [
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
        // L-ARR-010: Lock Envelopes, live (the automation editing spec, R8).
        {
            let rect = layout.lock_envelopes;
            let id = lock_envelopes_id();
            let on = doc.settings().lock_envelopes;
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
                "L",
                theme.small_font(),
                ink,
            );
            if response.clicked() {
                invoke(events, "song.set_lock_envelopes", json!({ "enabled": !on }));
            }
            // Spec: accessibility R10. The switch is in the Tab order,
            // after Automation Mode.
            crate::focus::register(ui.ctx(), id, bl_shell::Area::MainView, None);
            // Spec: arranger-track R14. The sections follow it, in time order.
            for section in &self.arranger_lane.tab {
                crate::focus::register(ui.ctx(), *section, bl_shell::Area::MainView, None);
            }
            response.widget_info(|| {
                WidgetInfo::selected(WidgetType::Checkbox, true, on, "Lock Envelopes")
            });
        }

        // L-ARR-017: Optimize Height and Width, live; WP-2.12: Automation Mode.
        for (rect, id, name, text, on, command) in [
            (
                layout.automation_mode,
                automation_mode_id(),
                "Automation Mode",
                "A",
                view.automation_mode(),
                ids::AUTOMATION_MODE,
            ),
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
            if command == ids::AUTOMATION_MODE {
                // Spec: accessibility R10. Automation Mode is above the
                // track headers, in Live's order.
                crate::focus::register_above(
                    ui.ctx(),
                    id,
                    bl_shell::Area::MainView,
                    crate::focus::ABOVE_AUTOMATION_MODE,
                );
            } else {
                // Optimize Height and Width come after the selected track's
                // header in the Arrangement's Tab order (observed in Live
                // 12.4.6; the accessibility spec, R4).
                crate::focus::register(ui.ctx(), id, bl_shell::Area::MainView, None);
            }
            response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, name));
        }
    }

    /// The view's own accessible node, a pane named as the main view is, and
    /// the clips on screen as items in it.
    fn accessibility(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        view: &ArrangementView,
        doc: &Document,
        layout: &ArrangementLayout,
    ) {
        // Spec: info-view R11. A clip with info text shows it in the Info
        // View while the pointer is over it.
        if let Some(pos) = ui.ctx().pointer_hover_pos()
            && layout.lanes.contains(pos)
            && let Some(shown) = self.shown.iter().rev().find(|s| s.rect.contains(pos))
        {
            crate::info_text::hover(ui.ctx(), InfoTarget::Clip(shown.clip));
        }
        let enabled = ui
            .ctx()
            .accesskit_node_builder(lanes_id(), |node| {
                node.set_role(Role::Group);
                node.set_label("Track Lanes".to_owned());
            })
            .is_some();
        self.described.clear();
        if !enabled {
            return;
        }
        // Spec: comping R30. Each take lane's row, named with its track and
        // saying whether it's auditioned.
        let lanes = layout.lanes;
        let scroll_y = view.scroll_y() as f32;
        for row in &self.rows {
            for lane in view.take_lane_rows(doc, row) {
                let top = lanes.top() + lane.top - scroll_y;
                let rect = Rect::from_min_max(
                    pos2(lanes.left(), top),
                    pos2(lanes.right(), top + lane.height),
                )
                .intersect(lanes);
                if rect.height() <= 0.0 {
                    continue;
                }
                let (Some(held), Some(track)) = (doc.take_lane(lane.lane), doc.track(row.track))
                else {
                    continue;
                };
                let id = take_lanes::row_id(lane.lane);
                let _ = ui.interact(rect, id, Sense::hover());
                let label = format!("{}, take lane {}", track.name(), held.name);
                let auditioned = track.audition() == Some(lane.lane);
                ui.ctx().accesskit_node_builder(id, |node| {
                    node.set_role(Role::Group);
                    node.set_label(label.clone());
                    node.set_value(if auditioned { "auditioned" } else { "" });
                    node.set_selected(view.selection_take_lanes().contains(&lane.lane));
                });
            }
        }
        // Clips big enough to point at, up to a limit, so a zoomed-out view
        // of thousands stays cheap.
        for shown in self
            .shown
            .iter()
            .filter(|s| s.rect.width() >= 6.0 && s.rect.height() >= 8.0)
            .take(512)
        {
            let Some(clip) = doc.clip(shown.clip) else {
                continue;
            };
            let id = clip_id(shown.clip);
            self.described.push(shown.clip);
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
                if let Some((_, text)) = shell.info_text_shown(InfoTarget::Clip(clip.id)) {
                    node.set_description(text);
                }
                // Spec: arrangement-editing R15.
                node.set_selected(view.is_clip_selected(clip.id));
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
    fn keys(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        view: &ArrangementView,
        lanes: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let focused = crate::window::has_focus(ui.ctx(), shell, bl_shell::Area::MainView)
            && !ui.ctx().text_edit_focused()
            && ui.ctx().memory(|m| {
                // The view, a track's title bar in its Tab order, or a
                // locator, has the keyboard: the arrows move the selection.
                m.focused().is_none_or(|id| {
                    id == main_view_id()
                        || self.rows.iter().any(|r| headers::title_id(r.track) == id)
                        || view
                            .selected_take_lanes()
                            .iter()
                            .any(|lane| take_lanes::title_id(*lane) == id)
                        || shell
                            .editor()
                            .document()
                            .settings()
                            .time_signatures
                            .changes()
                            .any(|change| signatures::signature_id(change.at) == id)
                        || shell
                            .editor()
                            .document()
                            .locators()
                            .iter()
                            .any(|locator| locator_id(locator.id) == id)
                })
            });
        if !focused {
            return;
        }
        if let Some(focused) = ui.ctx().memory(egui::Memory::focused) {
            ui.ctx().memory_mut(|m| {
                m.set_focus_lock_filter(
                    focused,
                    EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..EventFilter::default()
                    },
                );
            });
        }
        // Spec: automation-editing R20 to R26. The automation lane that has
        // the keyboard reads Left and Right, and Up and Down with a
        // breakpoint selected, itself; so does Option with Up and Down on the
        // selected track's lane.
        let keyed = view.keyboard_lane_number();
        if let Some(lane) = keyed
            && let Some(keys) = shell
                .keyboard_envelope()
                .filter(|keys| keys.envelope == bl_shell::envelope_commands::EnvelopeKeys::Lane)
        {
            crate::envelope_keys::read(
                ui,
                &keys,
                |forward, all| automation::cycle_lane(shell, lane, forward, all),
                events,
            );
        } else if view.automation_mode()
            && let Some(track) = view.selected_tracks().first().copied()
        {
            let cycle = ui.input_mut(|i| {
                let mut found = None;
                i.events.retain(|event| match event {
                    egui::Event::Key {
                        key: key @ (Key::ArrowUp | Key::ArrowDown),
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.alt && !modifiers.command && !modifiers.ctrl => {
                        found = Some((*key == Key::ArrowDown, modifiers.shift));
                        false
                    }
                    _ => true,
                });
                found
            });
            if let Some((forward, all)) = cycle
                && let Some(invocation) = automation::cycle_lane(shell, (track, 0), forward, all)
            {
                events.push(UiEvent::Invoke(invocation));
            }
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
        // Spec: automation-editing R6. Option (Alt on Windows) with an
        // edit command applies it to the automation lanes under the tracks
        // of the time selection (S-EDIT-016).
        let (copy, cut, paste, duplicate, delete) = ui.input(|i| {
            let pressed = |keys: &[Key], command: bool| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { key, pressed: true, modifiers, .. }
                            if keys.contains(key)
                                && modifiers.alt
                                && modifiers.command == command
                                && !modifiers.shift
                    )
                })
            };
            (
                pressed(&[Key::C], true),
                pressed(&[Key::X], true),
                pressed(&[Key::V], true),
                pressed(&[Key::D], true),
                pressed(&[Key::Delete], false),
            )
        });
        if copy || cut || paste || duplicate || delete {
            // Spec: automation R30. Every lane the selected tracks show.
            let lanes: Vec<Lane> = view
                .selected_tracks()
                .iter()
                .flat_map(|track| {
                    view.shown_lane_targets(*track)
                        .into_iter()
                        .map(|(_, target)| Lane::Automation {
                            track: *track,
                            target,
                        })
                })
                .collect();
            if !lanes.is_empty() {
                let args = json!({ "lanes": lanes });
                let command = if copy {
                    "automation.copy"
                } else if cut {
                    "automation.cut"
                } else if paste {
                    "automation.paste"
                } else if duplicate {
                    "automation.duplicate"
                } else {
                    "automation.delete_range"
                };
                invoke(events, command, args);
            }
        }
        // Spec: comping R28. Cmd+Up and Cmd+Down (Ctrl on Windows) move the
        // take lanes whose headers are selected (S-COMP-008) and otherwise
        // replace the main lane's material with the previous or next take
        // (S-COMP-006); each key is the same shortcut's other direction,
        // which the keymap can't say.
        let (cmd_up, cmd_down, rename) = ui.input(|i| {
            let pressed = |key: Key| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { key: k, pressed: true, modifiers, .. }
                            if *k == key
                                && modifiers.command
                                && !modifiers.alt
                                && !modifiers.shift
                    )
                })
            };
            (
                pressed(Key::ArrowUp),
                pressed(Key::ArrowDown),
                pressed(Key::R),
            )
        });
        if cmd_up || cmd_down {
            if view.selected_take_lanes().is_empty() && view.title_bars_selected() {
                // Spec: track-context-menu R18. With title bars selected the
                // same keys move the tracks (S-TRACK-012).
                crate::track_menu::move_tracks(
                    events,
                    shell.editor().document(),
                    view.selected_tracks(),
                    cmd_down,
                );
            } else if view.selected_take_lanes().is_empty() {
                let args = json!({ "direction": if cmd_down { "next" } else { "previous" } });
                if shell.can_invoke("take_lane.replace_with_take", &args) {
                    invoke(events, "take_lane.replace_with_take", args);
                }
            } else if let Some((lanes, index)) = moved_lanes(
                shell.editor().document(),
                view.selected_take_lanes(),
                cmd_down,
            ) {
                invoke(
                    events,
                    "take_lane.move",
                    json!({ "lanes": lanes, "index": index }),
                );
            }
        }
        // Spec: comping R28. Cmd+R types a new name in the first selected
        // take lane's header.
        if rename && !view.selected_take_lanes().is_empty() {
            invoke(events, "take_lane.rename", json!({}));
        }
        if (up || down) && !view.selected_take_lanes().is_empty() {
            // Up and Down move through the headers, a track's and its take
            // lanes' (the accessibility spec, R4).
            let mut order: Vec<Named> = Vec::new();
            for row in &self.rows {
                order.push(Named::Track(row.track));
                order.extend(
                    view.take_lane_rows(shell.editor().document(), row)
                        .map(|lane| Named::Lane(lane.lane)),
                );
            }
            let at = view
                .selected_take_lanes()
                .last()
                .and_then(|lane| order.iter().position(|n| *n == Named::Lane(*lane)));
            let next = at.and_then(|at| {
                if up {
                    at.checked_sub(1)
                } else {
                    (at + 1 < order.len()).then_some(at + 1)
                }
            });
            match next.map(|next| order[next]) {
                Some(Named::Lane(lane)) => {
                    invoke(events, ids::SELECT_TAKE_LANES, json!({ "lanes": [lane] }));
                }
                Some(Named::Track(track)) => invoke(
                    events,
                    ids::SELECT_TRACKS,
                    json!({ "tracks": [track], "title_bars": true }),
                ),
                None => {}
            }
        } else if (up || down) && view.automation_mode() {
            // Spec: automation-editing R20. With Automation Mode on, Up and
            // Down move through each track's lane and then its automation
            // lane, in the order they're drawn.
            let stops: Vec<(bl_model::TrackId, Option<usize>)> = self
                .rows
                .iter()
                .flat_map(|row| {
                    [(row.track, None)].into_iter().chain(
                        view.shown_lane_targets(row.track)
                            .into_iter()
                            .map(|(number, _)| (row.track, Some(number))),
                    )
                })
                .collect();
            let current = match (keyed, view.selected_tracks().first()) {
                (Some((track, number)), _) => Some((track, Some(number))),
                (None, Some(track)) => Some((*track, None)),
                (None, None) => None,
            };
            let at = current.and_then(|current| stops.iter().position(|stop| *stop == current));
            let next = match (at, up) {
                (Some(i), true) => i.checked_sub(1),
                (Some(i), false) => (i + 1 < stops.len()).then_some(i + 1),
                (None, _) => (!stops.is_empty()).then_some(0),
            };
            if let Some((track, lane)) = next.and_then(|i| stops.get(i)).copied() {
                invoke(
                    events,
                    ids::SELECT_TRACKS,
                    json!({
                        "tracks": [track],
                        "title_bars": view.title_bars_selected() && lane.is_none(),
                        "automation_lane": lane.map_or(json!(false), |number| json!(number)),
                    }),
                );
            }
        } else if up || down {
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
            // Spec: automation R26. With Automation Mode on, a selected title
            // bar's Left hides the track's lanes of its own and Right shows
            // them; a track with none folds and unfolds.
            let lanes_of = |track: &bl_model::TrackId| !view.own_lanes(*track).is_empty();
            if view.automation_mode() && view.selected_tracks().iter().any(lanes_of) {
                invoke(
                    events,
                    ids::SHOW_AUTOMATION_LANES,
                    json!({ "tracks": view.selected_tracks(), "on": right }),
                );
            } else {
                invoke(events, ids::FOLD_TRACKS, json!({ "fold": left }));
            }
        }
        if (taller || shorter) && !view.selected_tracks().is_empty() {
            let delta = if taller { HEIGHT_STEP } else { -HEIGHT_STEP };
            invoke(events, ids::SET_TRACK_HEIGHT, json!({ "delta": delta }));
        }
        // Spec: time-editing R31, R32. With a time-signature marker
        // selected, Cmd-R types its signature and Delete deletes it.
        if let Some(at) = view.selected_signature() {
            let pressed = |wanted: Key, command: bool| {
                ui.input(|i| {
                    i.events.iter().any(|event| {
                        matches!(
                            event,
                            egui::Event::Key { key, pressed: true, modifiers, .. }
                                if *key == wanted
                                    && modifiers.command == command
                                    && !modifiers.shift
                                    && !modifiers.alt
                        )
                    })
                })
            };
            if pressed(Key::R, true) {
                invoke(events, locator_ids::EDIT_SIGNATURE, json!({}));
            }
            if pressed(Key::Delete, false) {
                invoke(
                    events,
                    "song.delete_time_signature_change",
                    json!({ "at": at }),
                );
            }
        }
        // Spec: time-editing R8. Cmd-R renames the selected locators.
        if !view.selected_locators().is_empty()
            && ui.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: Key::R,
                            pressed: true,
                            modifiers,
                            ..
                        } if modifiers.command && !modifiers.shift && !modifiers.alt
                    )
                })
            })
        {
            invoke(events, locator_ids::RENAME, json!({}));
        }
        // Spec: time-editing R4. The Delete key deletes the selected
        // locators.
        if !view.selected_locators().is_empty()
            && ui.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: Key::Delete,
                            pressed: true,
                            ..
                        }
                    )
                })
            })
        {
            invoke(events, locator_ids::DELETE, json!({}));
        }
        // Spec: arranger-track R7, R10, R11. With a section selected, Cmd-R
        // renames it, Cmd-D duplicates it and Delete deletes it, unless its
        // name is being typed.
        if !view.selected_sections().is_empty() && !arranger::is_renaming(ui.ctx()) {
            let pressed = |wanted: Key, command: bool| {
                ui.input(|i| {
                    i.events.iter().any(|event| {
                        matches!(
                            event,
                            egui::Event::Key { key, pressed: true, modifiers, .. }
                                if *key == wanted
                                    && modifiers.command == command
                                    && !modifiers.shift
                                    && !modifiers.alt
                        )
                    })
                })
            };
            use bl_shell::arranger_commands as section_ids;
            if pressed(Key::R, true) {
                invoke(events, section_ids::RENAME_SECTION, json!({}));
            } else if pressed(Key::D, true) {
                invoke(events, section_ids::DUPLICATE_SECTION, json!({}));
            } else if pressed(Key::Delete, false) {
                invoke(events, section_ids::DELETE_SECTION, json!({}));
            }
        }
        // WP-1.8b: nudging, clip edges, extending the selection, and
        // resizing from the keyboard.
        let doc = shell.editor().document();
        let clip_lanes = Lanes {
            view,
            doc,
            rect: lanes,
            horizontal: view.horizontal(doc),
            shown: &self.shown,
            rows: &self.rows,
            scroll_y: view.scroll_y() as f32,
            draw_mode: shell.clip_view().draw_mode(),
        };
        ui.input(|input| {
            self.clips
                .keys(&clip_lanes, input, view.title_bars_selected(), events);
        });
    }
}

/// Where Cmd+Up (`later` false) or Cmd+Down (true) moves the take lanes whose
/// headers are selected: the lanes, and the place they take among the
/// track's other take lanes (the comping spec, R8). `None` past the first or
/// last, or with lanes of several tracks selected.
fn moved_lanes(
    doc: &Document,
    selected: &[bl_model::TakeLaneId],
    later: bool,
) -> Option<(Vec<bl_model::TakeLaneId>, usize)> {
    let track = doc.take_lane_track(*selected.first()?)?;
    if selected
        .iter()
        .any(|lane| doc.take_lane_track(*lane) != Some(track))
    {
        return None;
    }
    let all: Vec<bl_model::TakeLaneId> = doc.track(track)?.take_lanes().map(|l| l.id).collect();
    let moving: Vec<bl_model::TakeLaneId> = all
        .iter()
        .copied()
        .filter(|lane| selected.contains(lane))
        .collect();
    let rest = all.len() - moving.len();
    let first = all.iter().position(|lane| selected.contains(lane))?;
    // Lanes before the first one moved, none of them moving.
    let index = first;
    let to = if later {
        (index < rest).then_some(index + 1)?
    } else {
        index.checked_sub(1)?
    };
    Some((moving, to.min(rest)))
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

/// The lanes a drag across the rows passed through: each row's own lane,
/// its automation lanes where the drag went into them (the automation editing
/// spec, R1), and the take lanes it crossed (the comping spec, R26). While
/// Automation Mode is on, a track's main lane draws over its own lane, so
/// where it shows an envelope that is the lane the drag covers (the
/// automation spec, R24, R30).
fn lanes_crossed(
    rows: &[Row],
    from: usize,
    to: usize,
    press_y: f32,
    now_y: f32,
    view: &ArrangementView,
    doc: &Document,
) -> Vec<Lane> {
    let (lo, hi) = (press_y.min(now_y), press_y.max(now_y));
    let mut out = Vec::new();
    for row in &rows[from..=to] {
        let shown = view.shown_lane_targets(row.track);
        let covers = |number: usize| {
            let (top, bottom) = row.lane_span(number);
            hi > top && lo < bottom
        };
        match shown.iter().find(|(number, _)| *number == 0) {
            Some((_, target)) if covers(0) => out.push(Lane::Automation {
                track: row.track,
                target: *target,
            }),
            _ if covers(0) => out.push(Lane::Track { track: row.track }),
            _ => {}
        }
        for (number, target) in shown.iter().filter(|(number, _)| *number > 0) {
            if covers(*number) {
                out.push(Lane::Automation {
                    track: row.track,
                    target: *target,
                });
            }
        }
        for lane in view.take_lane_rows(doc, row) {
            if hi > lane.top && lo < lane.top + lane.height {
                out.push(Lane::Take {
                    track: row.track,
                    lane: lane.lane,
                });
            }
        }
    }
    out
}
