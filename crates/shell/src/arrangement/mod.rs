//! The Arrangement View's state without a window (WP-1.8a): what the view
//! shows (zoom, scroll, track heights and folds, the Overview), Follow, the
//! editing grid, the insert marker and the selection, and the commands that
//! change them. The UI (`bl-ui`) draws from it; people, agents, and tests
//! change it through the commands in `bl_shell_spec::arrangement`, which
//! [`ArrangementView::run`] carries out. None of it is song state: the set
//! never changes here, and nothing here is an undo step.
//!
//! The behaviour is in the arrangement view spec. The clip selection and the
//! edits that act on it are in [`editing`] (WP-1.8b), whose behaviour is in
//! the arrangement editing spec.

pub mod automation;
pub mod editing;
pub mod grid;
pub mod locator;
pub mod placing;
pub mod rows;

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::{Ticks, TimeSignature};
use bl_model::{AutomationTarget, ClipId, Document, LocatorId, MixerParam, TrackId, TrackKind};
use bl_shell_spec::arrangement::{
    self as ids, ArrangementState, AutomationLane, FoldArgs, FoldGroupsArgs, FollowArgs,
    InsertMarkerArgs, Lane, ScrollArgs, SelectTimeArgs, SelectTracksArgs, SetGridArgs,
    ShowAutomationArgs, ShowRangeArgs, ShowTrackArgs, SwitchArgs, TimeRange, TrackHeightArgs,
    ZoomArgs,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use self::grid::{Grid, Step};
use self::rows::{
    AUTOMATION_LANE_HEIGHT, DEFAULT_TRACK_HEIGHT, DROP_AREA, FOLDED_HEIGHT, MAX_TRACK_HEIGHT,
    MIN_OPTIMIZED_HEIGHT, MIN_TRACK_HEIGHT, Row,
};

/// A new set's zoom: 30 points to a 4/4 bar, about 32 bars across the lanes
/// at the reference window size. Live's own default hasn't been measured
/// (arrangement-view, "To observe in Live").
pub const DEFAULT_TICKS_PER_POINT: f64 = Ticks::WHOLE.get() as f64 / 30.0;

/// The closest zoom: a sixteenth note across 400 points.
pub const MIN_TICKS_PER_POINT: f64 = Ticks::SIXTEENTH.get() as f64 / 400.0;

/// The farthest zoom: 2,048 bars of 4/4 across 200 points.
pub const MAX_TICKS_PER_POINT: f64 = Ticks::WHOLE.get() as f64 * 2048.0 / 200.0;

/// The size of the lanes' scrolling area at the reference window size
/// (layout.md, L-ARR-005 and L-ARR-014: x 320 to 1295, y 119 to 688), which
/// the view assumes until a window reports its own.
pub const REFERENCE_VIEWPORT: Viewport = Viewport {
    width: 975.0,
    height: 569.0,
};

/// The shortest arrangement the whole-arrangement views show: 16 bars.
const MIN_ARRANGEMENT_BARS: i64 = 16;

/// The size of the lanes' scrolling area, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// From the lanes' left edge to the track headers.
    pub width: f32,
    /// From under the rulers to the pinned return tracks.
    pub height: f32,
}

/// The transport as the Arrangement View draws it: the playhead and the loop
/// brace. `Shell::poll_engine` fills it in before each frame: the playhead
/// from the engine's UI state (`bl_engine::EngineState`'s `playing` and
/// `position_ticks`), and the brace from the document's loop (WP-1.18). The
/// default is a new set's: stopped at 1.1.1, the brace on bars 1 to 5, off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportView {
    /// Whether the song is playing.
    pub playing: bool,
    /// The playhead.
    pub position: Ticks,
    /// Where the loop brace starts.
    pub loop_start: Ticks,
    /// How long the loop brace is.
    pub loop_length: Ticks,
    /// Whether the Arrangement loop is on.
    pub loop_on: bool,
}

impl Default for TransportView {
    fn default() -> Self {
        Self {
            playing: false,
            position: Ticks::ZERO,
            loop_start: Ticks::ZERO,
            loop_length: Ticks::WHOLE * 4,
            loop_on: false,
        }
    }
}

impl TransportView {
    /// Where the loop brace ends.
    #[must_use]
    pub fn loop_end(&self) -> Ticks {
        self.loop_start.saturating_add(self.loop_length)
    }
}

/// A horizontal view: the position at the lanes' left edge and the zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Horizontal {
    /// Ticks at the lanes' left edge.
    pub left: f64,
    /// Ticks per point.
    pub ticks_per_point: f64,
}

impl Horizontal {
    /// Where `at` is, in points from the lanes' left edge.
    #[must_use]
    pub fn x(&self, at: Ticks) -> f64 {
        (at.get() as f64 - self.left) / self.ticks_per_point
    }

    /// The position `x` points from the lanes' left edge.
    #[must_use]
    pub fn at(&self, x: f64) -> Ticks {
        Ticks((self.left + x * self.ticks_per_point).round() as i64)
    }

    /// The span `width` points across shows.
    #[must_use]
    pub fn range(&self, width: f32) -> TimeRange {
        TimeRange {
            start: Ticks(self.left.floor() as i64),
            end: Ticks((self.left + f64::from(width) * self.ticks_per_point).ceil() as i64),
        }
    }
}

/// A view to go back to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Saved {
    horizontal: Horizontal,
    scroll_y: f64,
}

/// Why a view command didn't run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// The arguments don't fit the command.
    InvalidArgs(String),
    /// The command can't run now, for the reason given.
    Unavailable(String),
}

/// The Arrangement View's state. See the [module docs](self).
#[derive(Clone, Debug)]
pub struct ArrangementView {
    horizontal: Horizontal,
    scroll_y: f64,
    viewport: Viewport,
    heights: BTreeMap<TrackId, f32>,
    folded: BTreeSet<TrackId>,
    optimize_height: bool,
    /// While Optimize Width is on, the view to return to.
    optimize_width: Option<Saved>,
    follow: bool,
    follow_paused: bool,
    overview: bool,
    grid: Grid,
    insert_marker: Ticks,
    selected: Vec<TrackId>,
    /// The selected clips (WP-1.8b): see [`editing`].
    selected_clips: Vec<ClipId>,
    /// The selected locators (WP-6.24): see [`locator`].
    selected_locators: Vec<LocatorId>,
    title_bars: bool,
    time_selection: Option<(Ticks, Ticks)>,
    /// The lanes the time selection covers: each a track's own lane or its
    /// automation lane (the automation editing spec, R1).
    time_selection_lanes: Vec<Lane>,
    history: Vec<Saved>,
    /// The view `arrangement.zoom_to_selection` last left, so running it
    /// again straight away goes back.
    zoomed_to_selection: Option<Saved>,
    transport: TransportView,
    /// Whether Automation Mode is on (WP-2.12): every track shows its
    /// automation lane under its lane.
    automation_mode: bool,
    /// Whether Automation Arm is on (WP-2.12): moving a control while the
    /// transport records writes it into the parameter's envelope.
    automation_arm: bool,
    /// The parameter each track's automation lane shows, where the choosers
    /// have picked one; `None` hides that track's lane. A track with no
    /// entry shows its volume.
    automation_lanes: BTreeMap<TrackId, Option<AutomationTarget>>,
    /// The segment selected in each track's automation lane: the parameter
    /// it shows and the breakpoint the segment starts at (the automation
    /// editing spec, R4).
    automation_segment: BTreeMap<TrackId, (AutomationTarget, Ticks)>,
    /// Whether the track headers show their In/Out column (WP-6.64).
    show_in_out: bool,
    /// Whether the track headers show their volume, pan, and sends
    /// (WP-6.64).
    show_volume: bool,
    /// Whether the return tracks' rows show in the lanes (WP-6.64).
    show_return_tracks: bool,
}

impl Default for ArrangementView {
    fn default() -> Self {
        Self::new()
    }
}

impl ArrangementView {
    /// A new set's view: bar 1 at the left edge at the default zoom, the
    /// first track at the top, Follow on, the Overview shown, and the
    /// default grid.
    #[must_use]
    pub fn new() -> Self {
        Self {
            horizontal: Horizontal {
                left: 0.0,
                ticks_per_point: DEFAULT_TICKS_PER_POINT,
            },
            scroll_y: 0.0,
            viewport: REFERENCE_VIEWPORT,
            heights: BTreeMap::new(),
            folded: BTreeSet::new(),
            optimize_height: false,
            optimize_width: None,
            follow: true,
            follow_paused: false,
            overview: true,
            grid: Grid::default(),
            insert_marker: Ticks::ZERO,
            selected: Vec::new(),
            selected_clips: Vec::new(),
            selected_locators: Vec::new(),
            title_bars: false,
            time_selection: None,
            time_selection_lanes: Vec::new(),
            history: Vec::new(),
            zoomed_to_selection: None,
            transport: TransportView::default(),
            automation_mode: false,
            automation_arm: false,
            automation_lanes: BTreeMap::new(),
            automation_segment: BTreeMap::new(),
            show_in_out: true,
            show_volume: true,
            show_return_tracks: true,
        }
    }

    /// Goes back to a new set's view, as a new or opened set shows, keeping
    /// the window's size and the transport.
    pub fn reset(&mut self) {
        *self = Self {
            viewport: self.viewport,
            transport: self.transport,
            ..Self::new()
        };
    }

    /// Records the size of the lanes' scrolling area, as the window lays it
    /// out. Returns whether it changed.
    pub fn set_viewport(&mut self, viewport: Viewport, doc: &Document) -> bool {
        let viewport = Viewport {
            width: viewport.width.max(1.0),
            height: viewport.height.max(1.0),
        };
        if viewport == self.viewport {
            return false;
        }
        self.viewport = viewport;
        self.clamp_scroll(doc);
        true
    }

    /// The size of the lanes' scrolling area the view works with.
    #[must_use]
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// The transport as the view last heard it.
    #[must_use]
    pub fn transport(&self) -> TransportView {
        self.transport
    }

    /// Takes the transport's latest state: the playhead and the loop. With
    /// Follow on, starting or stopping playback resumes it, and while the
    /// song plays the view turns the page whenever the playhead leaves it.
    /// Returns whether anything but the playhead's position changed: the
    /// view, whether the song plays, or the loop. The playhead moves every
    /// frame while the song plays, and the view redraws for it anyway, so
    /// that alone doesn't count (menus needn't refresh for it).
    pub fn set_transport(&mut self, transport: TransportView) -> bool {
        let mut changed = TransportView {
            position: self.transport.position,
            ..transport
        } != self.transport;
        if transport.playing != self.transport.playing && self.follow_paused {
            // Spec: arrangement-view R10.
            self.follow_paused = false;
        }
        self.transport = transport;
        if self.follow && !self.follow_paused && transport.playing && self.optimize_width.is_none()
        {
            let x = self.horizontal.x(transport.position);
            let width = f64::from(self.viewport.width);
            if x < 0.0 || x >= width {
                self.horizontal.left = transport.position.get().max(0) as f64;
                self.zoomed_to_selection = None;
                changed = true;
            }
        }
        changed
    }

    /// Moves the insert marker to where the transport put it (a position
    /// field, Home, a second Stop, an agent; WP-1.18), keeping the
    /// selection and Follow as they are. Returns whether it moved.
    pub fn move_insert_marker(&mut self, at: Ticks) -> bool {
        let moved = self.insert_marker != at;
        self.insert_marker = at;
        moved
    }

    /// The horizontal view in effect: with Optimize Width on, the whole
    /// arrangement across the lanes.
    #[must_use]
    pub fn horizontal(&self, doc: &Document) -> Horizontal {
        if self.optimize_width.is_some() {
            self.whole(doc)
        } else {
            self.horizontal
        }
    }

    /// How far the lanes are scrolled down, in points.
    #[must_use]
    pub fn scroll_y(&self) -> f64 {
        self.scroll_y
    }

    /// Whether Follow is on.
    #[must_use]
    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Whether Follow is on but paused.
    #[must_use]
    pub fn follow_paused(&self) -> bool {
        self.follow_paused
    }

    /// Whether the Overview shows.
    #[must_use]
    pub fn overview(&self) -> bool {
        self.overview
    }

    /// Whether Optimize Height is on.
    #[must_use]
    pub fn optimize_height(&self) -> bool {
        self.optimize_height
    }

    /// Whether Optimize Width is on.
    #[must_use]
    pub fn optimize_width(&self) -> bool {
        self.optimize_width.is_some()
    }

    /// Whether Automation Mode is on: the tracks show their automation lanes.
    #[must_use]
    pub fn automation_mode(&self) -> bool {
        self.automation_mode
    }

    /// Whether Automation Arm is on.
    #[must_use]
    pub fn automation_arm(&self) -> bool {
        self.automation_arm
    }

    /// Whether the track headers show their In/Out column.
    #[must_use]
    pub fn show_in_out(&self) -> bool {
        self.show_in_out
    }

    /// Whether the track headers show their volume, pan, and sends.
    #[must_use]
    pub fn show_volume(&self) -> bool {
        self.show_volume
    }

    /// Whether the return tracks' rows show in the lanes.
    #[must_use]
    pub fn show_return_tracks(&self) -> bool {
        self.show_return_tracks
    }

    /// The parameter a track's automation lane shows: the choosers' last
    /// choice, or its volume by default. `None` draws no envelope, while
    /// the lane and its choosers stay.
    #[must_use]
    pub fn automation_lane(&self, track: TrackId) -> Option<AutomationTarget> {
        self.automation_lanes
            .get(&track)
            .copied()
            .unwrap_or(Some(AutomationTarget::Mixer {
                param: MixerParam::Volume,
            }))
    }

    /// The segment selected in a track's automation lane: the parameter it
    /// shows and the breakpoint the segment starts at (the automation
    /// editing spec, R4).
    #[must_use]
    pub fn selected_segment(&self, track: TrackId) -> Option<(AutomationTarget, Ticks)> {
        self.automation_segment.get(&track).copied()
    }

    /// Selects a segment in a track's automation lane, or clears it with
    /// `at` `None` (the automation editing spec, R4).
    pub fn select_segment(&mut self, track: TrackId, target: AutomationTarget, at: Option<Ticks>) {
        match at {
            Some(at) => {
                self.automation_segment.insert(track, (target, at));
            }
            None => {
                self.automation_segment.remove(&track);
            }
        }
    }

    /// How tall `track`'s automation lane is, in points: [`AUTOMATION_LANE_HEIGHT`]
    /// while Automation Mode is on and the track is unfolded, and 0 otherwise.
    /// The lane shows even with nothing chosen (None), so its choosers can
    /// pick a parameter again.
    #[must_use]
    pub fn automation_lane_height(&self, track: TrackId) -> f32 {
        if self.automation_mode && !self.is_folded(track) {
            AUTOMATION_LANE_HEIGHT
        } else {
            0.0
        }
    }

    /// The editing grid's settings.
    #[must_use]
    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// The grid's spacing at the current zoom, under the signature at the
    /// view's left edge.
    #[must_use]
    pub fn grid_step(&self, doc: &Document) -> Step {
        let horizontal = self.horizontal(doc);
        self.grid.step(
            self.signature_at_left(doc, horizontal),
            horizontal.ticks_per_point,
        )
    }

    /// `at` snapped to the grid, if snapping is on.
    #[must_use]
    pub fn snap(&self, doc: &Document, at: Ticks) -> Ticks {
        if !self.grid.snap {
            return at;
        }
        grid::snap(&doc.settings().time_signatures, self.grid_step(doc), at)
    }

    fn signature_at_left(&self, doc: &Document, horizontal: Horizontal) -> TimeSignature {
        doc.settings()
            .time_signatures
            .signature_at(Ticks(horizontal.left.max(0.0) as i64))
    }

    /// Where the insert marker is.
    #[must_use]
    pub fn insert_marker(&self) -> Ticks {
        self.insert_marker
    }

    /// The selected tracks, in the order they were selected.
    #[must_use]
    pub fn selected_tracks(&self) -> &[TrackId] {
        &self.selected
    }

    /// Whether the selected tracks' title bars are selected, so the
    /// track-header shortcuts apply.
    #[must_use]
    pub fn title_bars_selected(&self) -> bool {
        self.title_bars && !self.selected.is_empty()
    }

    /// The time selection, start before end.
    #[must_use]
    pub fn time_selection(&self) -> Option<(Ticks, Ticks)> {
        self.time_selection
    }

    /// The lanes the time selection covers: each a track's own lane or its
    /// automation lane (the automation editing spec, R1).
    #[must_use]
    pub fn time_selection_lanes(&self) -> &[Lane] {
        &self.time_selection_lanes
    }

    /// The selected locators, in the order they were selected (WP-6.24).
    #[must_use]
    pub fn selected_locators(&self) -> &[LocatorId] {
        &self.selected_locators
    }

    /// Whether a locator is selected.
    #[must_use]
    pub fn is_locator_selected(&self, locator: LocatorId) -> bool {
        self.selected_locators.contains(&locator)
    }

    /// Selects locators, as clicking one does. An empty list clears the
    /// locator selection and leaves the rest.
    pub fn select_locators(&mut self, locators: Vec<LocatorId>) {
        self.selected_locators = locators;
    }

    /// Drops selected locators the document no longer has.
    pub fn forget_missing_locators(&mut self, doc: &Document) {
        self.selected_locators
            .retain(|id| doc.locator(*id).is_some());
    }

    /// Clears the clip selection and the time selection, as clicking a
    /// locator does (the time editing spec, R5).
    pub(crate) fn clear_clip_selection(&mut self) {
        self.selected_clips.clear();
        self.time_selection = None;
    }

    /// Whether a track is folded.
    #[must_use]
    pub fn is_folded(&self, track: TrackId) -> bool {
        self.folded.contains(&track)
    }

    /// Unfolds `track` if it was folded.
    pub fn unfold(&mut self, track: TrackId) {
        self.folded.remove(&track);
    }

    /// A track's own height, unfolded.
    #[must_use]
    pub fn track_height(&self, track: TrackId) -> f32 {
        self.heights
            .get(&track)
            .copied()
            .unwrap_or(DEFAULT_TRACK_HEIGHT)
    }

    /// The rows of the scrolling part of the lanes, top to bottom, into
    /// `out` (cleared first): every track but the returns and Main, skipping
    /// those inside a folded group, each at the height it shows at.
    pub fn rows_into(&self, doc: &Document, out: &mut Vec<Row>) {
        out.clear();
        let shown = rows::shown_tracks(doc, |id| self.is_folded(id));
        let mut top = 0.0;
        for (track, kind, depth) in shown {
            // Spec: arrangement-view R23: with the return tracks hidden,
            // their rows aren't drawn, scrolling ones (R18) included.
            if !self.show_return_tracks && kind == TrackKind::Return {
                continue;
            }
            let folded = self.is_folded(track);
            let height = if folded {
                FOLDED_HEIGHT
            } else {
                self.track_height(track)
            };
            // WP-2.12: an automation lane under an unfolded track while
            // Automation Mode is on.
            let automation = if folded {
                0.0
            } else {
                self.automation_lane_height(track)
            };
            out.push(Row {
                track,
                kind,
                depth,
                top,
                height,
                automation,
                folded,
            });
            top += height + automation;
        }
        if self.optimize_height && !out.is_empty() {
            let each = (self.viewport.height / out.len() as f32).max(MIN_OPTIMIZED_HEIGHT);
            for (index, row) in out.iter_mut().enumerate() {
                row.top = each * index as f32;
                row.height = (each - row.automation).max(MIN_OPTIMIZED_HEIGHT);
            }
        }
    }

    /// The rows, as a new list. See [`ArrangementView::rows_into`].
    #[must_use]
    pub fn rows(&self, doc: &Document) -> Vec<Row> {
        let mut rows = Vec::new();
        self.rows_into(doc, &mut rows);
        rows
    }

    /// How tall the scrolling part's content is: the rows and the drop area
    /// under them.
    #[must_use]
    pub fn content_height(&self, rows: &[Row]) -> f32 {
        let bottom = rows.last().map_or(0.0, Row::bottom);
        if self.optimize_height {
            bottom
        } else {
            bottom + DROP_AREA
        }
    }

    /// The farthest the lanes scroll down.
    #[must_use]
    pub fn max_scroll_y(&self, rows: &[Row]) -> f64 {
        f64::from((self.content_height(rows) - self.viewport.height).max(0.0))
    }

    /// Where the arrangement ends, for the views of the whole arrangement:
    /// the end of its last clip or of the loop brace, and at least 16 bars.
    #[must_use]
    pub fn arrangement_end(&self, doc: &Document) -> Ticks {
        let clips = doc
            .tracks()
            .flat_map(|track| track.clips().map(bl_model::Clip::end))
            .max()
            .unwrap_or(Ticks::ZERO);
        let minimum = doc
            .settings()
            .time_signatures
            .bar_start(MIN_ARRANGEMENT_BARS);
        clips.max(self.transport.loop_end()).max(minimum)
    }

    /// The view of the whole arrangement across the lanes.
    fn whole(&self, doc: &Document) -> Horizontal {
        let end = self.arrangement_end(doc).get() as f64;
        Horizontal {
            left: 0.0,
            ticks_per_point: (end / f64::from(self.viewport.width))
                .clamp(MIN_TICKS_PER_POINT, MAX_TICKS_PER_POINT),
        }
    }

    fn saved(&self, doc: &Document) -> Saved {
        Saved {
            horizontal: self.horizontal(doc),
            scroll_y: self.scroll_y,
        }
    }

    /// Ends Optimize Width where the view is, for a command that moves the
    /// view by hand.
    fn leave_optimize_width(&mut self, doc: &Document) {
        if self.optimize_width.is_some() {
            self.horizontal = self.whole(doc);
            self.optimize_width = None;
        }
    }

    fn set_horizontal(&mut self, horizontal: Horizontal) {
        self.horizontal = Horizontal {
            left: horizontal.left.max(0.0),
            ticks_per_point: horizontal
                .ticks_per_point
                .clamp(MIN_TICKS_PER_POINT, MAX_TICKS_PER_POINT),
        };
    }

    fn clamp_scroll(&mut self, doc: &Document) {
        let rows = self.rows(doc);
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll_y(&rows));
    }

    /// Zooms by `factor` around `anchor`.
    fn zoom(&mut self, doc: &Document, factor: f64, anchor: Ticks) {
        self.leave_optimize_width(doc);
        let before = self.horizontal;
        let x = before.x(anchor);
        let tpp = (before.ticks_per_point / factor).clamp(MIN_TICKS_PER_POINT, MAX_TICKS_PER_POINT);
        self.set_horizontal(Horizontal {
            left: anchor.get() as f64 - x * tpp,
            ticks_per_point: tpp,
        });
        self.zoomed_to_selection = None;
    }

    /// The position a zoom key keeps still: the middle of the time selection
    /// if it's in view, else the insert marker if it's in view, else the
    /// middle of the view.
    fn zoom_anchor(&self, doc: &Document) -> Ticks {
        let horizontal = self.horizontal(doc);
        let range = horizontal.range(self.viewport.width);
        let in_view = |at: Ticks| at >= range.start && at <= range.end;
        if let Some((start, end)) = self.time_selection {
            let middle = Ticks(start.get() + (end.get() - start.get()) / 2);
            if in_view(middle) {
                return middle;
            }
        }
        if in_view(self.insert_marker) {
            return self.insert_marker;
        }
        horizontal.at(f64::from(self.viewport.width) / 2.0)
    }

    /// Shows `start..end` across the lanes, remembering the view before.
    fn show_range(&mut self, doc: &Document, start: Ticks, end: Ticks) {
        let saved = self.saved(doc);
        self.history.push(saved);
        self.optimize_width = None;
        let span = (end.get() - start.get()).max(1) as f64;
        self.set_horizontal(Horizontal {
            left: start.get() as f64,
            ticks_per_point: span / f64::from(self.viewport.width),
        });
        if self.transport.playing {
            self.follow_paused = true;
        }
    }

    /// Scrolls so `track` is fully in view, the least distance.
    fn show_track(&mut self, doc: &Document, track: TrackId) {
        let rows = self.rows(doc);
        if let Some(row) = rows.iter().find(|row| row.track == track) {
            let top = f64::from(row.top);
            let bottom = f64::from(row.bottom());
            let height = f64::from(self.viewport.height);
            if top < self.scroll_y {
                self.scroll_y = top;
            } else if bottom > self.scroll_y + height {
                self.scroll_y = (bottom - height).min(top);
            }
            self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll_y(&rows));
        }
    }

    /// The tracks a command acts on: `tracks` if given, else the selected
    /// ones. Unknown tracks are an error.
    fn targets(
        &self,
        doc: &Document,
        tracks: Option<Vec<TrackId>>,
    ) -> Result<Vec<TrackId>, ViewError> {
        let tracks = tracks.unwrap_or_else(|| self.selected.clone());
        if let Some(missing) = tracks.iter().find(|id| doc.track(**id).is_none()) {
            return Err(ViewError::InvalidArgs(format!(
                "no track has the ID {}",
                missing.0
            )));
        }
        Ok(tracks)
    }

    /// Drops selected and folded tracks, and selected clips, the document
    /// no longer has, after an edit or a new set.
    pub fn forget_missing_tracks(&mut self, doc: &Document) {
        self.selected.retain(|id| doc.track(*id).is_some());
        self.forget_missing_clips(doc);
        self.forget_missing_locators(doc);
        self.folded.retain(|id| doc.track(*id).is_some());
        self.heights.retain(|id, _| doc.track(*id).is_some());
        self.automation_lanes
            .retain(|id, _| doc.track(*id).is_some());
        self.clamp_scroll(doc);
    }

    /// Whether a person can run view command `id` with `args` now: some
    /// can't, such as Zoom Back with nothing to go back to.
    #[must_use]
    pub fn can_run(&self, id: &str, args: &Value, doc: &Document) -> bool {
        let on = || args.get("on").and_then(Value::as_bool);
        match id {
            ids::ZOOM_BACK => !self.history.is_empty(),
            ids::NARROW_GRID => self.grid.can_narrow(),
            ids::WIDEN_GRID => self.grid.can_widen(),
            ids::TRIPLET_GRID => {
                let turning_on = on().unwrap_or(!self.grid.triplet);
                !turning_on || {
                    let plain = Grid {
                        triplet: false,
                        ..self.grid
                    };
                    let horizontal = self.horizontal(doc);
                    !plain
                        .step(
                            self.signature_at_left(doc, horizontal),
                            horizontal.ticks_per_point,
                        )
                        .is_bars()
                }
            }
            ids::FOLD_GROUPS => self.selected.iter().any(|id| {
                doc.track(*id)
                    .is_some_and(|track| track.kind() == TrackKind::Group)
            }),
            ids::FOLD_TRACKS => args.get("tracks").is_some() || !self.selected.is_empty(),
            ids::SET_TRACK_HEIGHT => args.get("tracks").is_some() || !self.selected.is_empty(),
            _ => true,
        }
    }

    /// Runs view command `id` with `args`, as `bl-shell`'s `Shell::run`
    /// does, and returns the view's state afterwards.
    ///
    /// # Errors
    ///
    /// [`ViewError`] if the arguments don't fit or the command can't run.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per command keeps each one's behaviour in sight"
    )]
    pub fn run(
        &mut self,
        id: &str,
        args: Value,
        doc: &Document,
    ) -> Result<ArrangementState, ViewError> {
        let unavailable = |reason: &str| ViewError::Unavailable(reason.to_owned());
        match id {
            ids::STATE => {
                parse::<bl_commands::NoArgs>(args)?;
            }
            ids::ZOOM => {
                let ZoomArgs { factor, anchor } = parse(args)?;
                if !(factor.is_finite() && (1.0 / 64.0..=64.0).contains(&factor)) {
                    return Err(ViewError::InvalidArgs(format!(
                        "`factor` must be from 1/64 to 64, not {factor}"
                    )));
                }
                let anchor = anchor.unwrap_or_else(|| self.zoom_anchor(doc));
                self.zoom(doc, factor, anchor);
            }
            ids::SHOW_RANGE => {
                let ShowRangeArgs { start, end } = parse(args)?;
                if end <= start {
                    return Err(ViewError::InvalidArgs(
                        "`end` must be after `start`".to_owned(),
                    ));
                }
                self.show_range(doc, start, end);
            }
            ids::SCROLL => {
                let ScrollArgs { x, y } = parse(args)?;
                if !(x.is_finite() && y.is_finite()) {
                    return Err(ViewError::InvalidArgs(
                        "`x` and `y` must be numbers".to_owned(),
                    ));
                }
                if x != 0.0 {
                    self.leave_optimize_width(doc);
                    let mut horizontal = self.horizontal;
                    horizontal.left += x * horizontal.ticks_per_point;
                    self.set_horizontal(horizontal);
                    self.zoomed_to_selection = None;
                    if self.transport.playing {
                        // Spec: arrangement-view R10.
                        self.follow_paused = true;
                    }
                }
                if y != 0.0 {
                    let rows = self.rows(doc);
                    self.scroll_y = (self.scroll_y + y).clamp(0.0, self.max_scroll_y(&rows));
                }
            }
            ids::SHOW_TRACK => {
                let ShowTrackArgs { track } = parse(args)?;
                self.targets(doc, Some(vec![track]))?;
                self.show_track(doc, track);
            }
            ids::ZOOM_TO_SELECTION => {
                parse::<bl_commands::NoArgs>(args)?;
                if let Some(after) = self.zoomed_to_selection
                    && after == self.saved(doc)
                    && let Some(before) = self.history.pop()
                {
                    // Run again straight away: back, so a held Z zooms only
                    // while it's held. Spec: arrangement-view R8.
                    self.restore(before);
                    self.zoomed_to_selection = None;
                } else {
                    let (start, end) = self
                        .time_selection
                        .unwrap_or((Ticks::ZERO, self.arrangement_end(doc)));
                    self.show_range(doc, start, end);
                    if let Some(first) = self.selected.first().copied() {
                        self.show_track(doc, first);
                    }
                    self.zoomed_to_selection = Some(self.saved(doc));
                }
            }
            ids::ZOOM_BACK => {
                parse::<bl_commands::NoArgs>(args)?;
                let before = self
                    .history
                    .pop()
                    .ok_or_else(|| unavailable("there's no earlier view to go back to"))?;
                self.restore(before);
                self.zoomed_to_selection = None;
            }
            ids::OPTIMIZE_HEIGHT => {
                let SwitchArgs { on } = parse(args)?;
                self.optimize_height = on.unwrap_or(!self.optimize_height);
                self.clamp_scroll(doc);
            }
            ids::OPTIMIZE_WIDTH => {
                let SwitchArgs { on } = parse(args)?;
                let on = on.unwrap_or(self.optimize_width.is_none());
                match (on, self.optimize_width) {
                    (true, None) => self.optimize_width = Some(self.saved(doc)),
                    (false, Some(before)) => {
                        self.optimize_width = None;
                        self.restore(before);
                    }
                    _ => {}
                }
                self.zoomed_to_selection = None;
            }
            ids::FOLLOW => {
                let FollowArgs { on, paused } = parse(args)?;
                if let Some(paused) = paused {
                    // Spec: arrangement-view R10.
                    self.follow_paused = paused && self.follow && self.transport.playing;
                }
                if on.is_some() || paused.is_none() {
                    self.follow = on.unwrap_or(!self.follow);
                    self.follow_paused &= self.follow && paused == Some(true);
                }
            }
            ids::OVERVIEW => {
                let SwitchArgs { on } = parse(args)?;
                self.overview = on.unwrap_or(!self.overview);
            }
            ids::NARROW_GRID => {
                parse::<bl_commands::NoArgs>(args)?;
                if !self.grid.narrow() {
                    return Err(unavailable("the grid is already at its narrowest"));
                }
            }
            ids::WIDEN_GRID => {
                parse::<bl_commands::NoArgs>(args)?;
                if !self.grid.widen() {
                    return Err(unavailable("the grid is already at its widest"));
                }
            }
            ids::TRIPLET_GRID => {
                let SwitchArgs { on } = parse(args)?;
                let on = on.unwrap_or(!self.grid.triplet);
                if on && !self.can_run(id, &serde_json::json!({ "on": true }), doc) {
                    return Err(unavailable(
                        "the grid is a bar or more apart, so it has no triplets; narrow it first",
                    ));
                }
                self.grid.triplet = on;
            }
            ids::SNAP_TO_GRID => {
                let SwitchArgs { on } = parse(args)?;
                self.grid.snap = on.unwrap_or(!self.grid.snap);
            }
            ids::FIXED_GRID => {
                let SwitchArgs { on } = parse(args)?;
                self.grid.fixed_on = on.unwrap_or(!self.grid.fixed_on);
            }
            ids::SET_GRID => {
                let SetGridArgs {
                    adaptive,
                    fixed,
                    triplet,
                    snap,
                } = parse(args)?;
                if adaptive.is_some() && fixed.is_some() {
                    return Err(ViewError::InvalidArgs(
                        "give `adaptive` or `fixed`, not both".to_owned(),
                    ));
                }
                if let Some(adaptive) = adaptive {
                    self.grid.adaptive = adaptive;
                    self.grid.fixed_on = false;
                }
                if let Some(fixed) = fixed {
                    self.grid.fixed = fixed;
                    self.grid.fixed_on = true;
                }
                if let Some(triplet) = triplet {
                    self.grid.triplet = triplet;
                }
                if let Some(snap) = snap {
                    self.grid.snap = snap;
                }
            }
            ids::SET_INSERT_MARKER => {
                let InsertMarkerArgs { at, track } = parse(args)?;
                if let Some(track) = track {
                    self.targets(doc, Some(vec![track]))?;
                    self.selected = vec![track];
                    self.title_bars = false;
                }
                self.insert_marker = at;
                self.time_selection = None;
                // Spec: automation-editing R1. A selection with no lanes
                // names no lane.
                self.time_selection_lanes.clear();
                // Spec: arrangement-editing R2.
                self.selected_clips.clear();
                // Spec: arrangement-view R10.
                self.follow_paused = false;
            }
            ids::SELECT_TRACKS => {
                let SelectTracksArgs {
                    tracks,
                    add,
                    title_bars,
                } = parse(args)?;
                let tracks = self.targets(doc, Some(tracks))?;
                self.selected_clips.clear();
                if add {
                    for track in tracks {
                        if !self.selected.contains(&track) {
                            self.selected.push(track);
                        }
                    }
                } else {
                    self.selected = tracks;
                }
                self.title_bars = title_bars;
                if let Some(first) = self.selected.last().copied() {
                    self.show_track(doc, first);
                }
            }
            ids::SELECT_TIME => {
                let SelectTimeArgs {
                    start,
                    end,
                    tracks,
                    lanes,
                } = parse(args)?;
                let (start, end) = if end < start {
                    (end, start)
                } else {
                    (start, end)
                };
                if let Some(tracks) = tracks {
                    self.selected = self.targets(doc, Some(tracks))?;
                }
                // Spec: automation-editing R1. The selection names the lanes
                // it covers: each a track's own lane or its automation lane.
                self.time_selection_lanes = match lanes {
                    Some(lanes) => {
                        let tracks: Vec<TrackId> = lanes.iter().map(Lane::track).collect();
                        self.selected = self.targets(doc, Some(tracks))?;
                        lanes
                    }
                    None => self
                        .selected
                        .iter()
                        .copied()
                        .map(|track| Lane::Track { track })
                        .collect(),
                };
                self.time_selection = (end > start).then_some((start, end));
                self.insert_marker = start;
                self.title_bars = false;
                // Spec: arrangement-editing R2.
                self.selected_clips.clear();
            }
            ids::FOLD_TRACKS => {
                let FoldArgs { tracks, fold } = parse(args)?;
                let tracks = self.targets(doc, tracks)?;
                let Some(first) = tracks.first() else {
                    return Err(unavailable("no track is selected"));
                };
                let fold = fold.unwrap_or(!self.is_folded(*first));
                for track in tracks {
                    if fold {
                        self.folded.insert(track);
                    } else {
                        self.folded.remove(&track);
                    }
                }
                self.clamp_scroll(doc);
            }
            ids::UNFOLD_ALL => {
                parse::<bl_commands::NoArgs>(args)?;
                self.folded.clear();
                self.clamp_scroll(doc);
            }
            ids::FOLD_GROUPS => {
                let FoldGroupsArgs { fold } = parse(args)?;
                let groups: Vec<TrackId> = self
                    .selected
                    .iter()
                    .copied()
                    .filter(|id| doc.track(*id).is_some_and(|t| t.kind() == TrackKind::Group))
                    .collect();
                if groups.is_empty() {
                    return Err(unavailable("no group track is selected"));
                }
                for group in groups {
                    if fold {
                        self.folded.insert(group);
                    } else {
                        self.folded.remove(&group);
                    }
                }
                self.clamp_scroll(doc);
            }
            ids::SET_TRACK_HEIGHT => {
                let TrackHeightArgs {
                    tracks,
                    height,
                    delta,
                } = parse(args)?;
                let tracks = self.targets(doc, tracks)?;
                if tracks.is_empty() {
                    return Err(unavailable("no track is selected"));
                }
                if height.is_some() == delta.is_some() {
                    return Err(ViewError::InvalidArgs(
                        "give `height` or `delta`".to_owned(),
                    ));
                }
                for track in tracks {
                    let now = f64::from(self.track_height(track));
                    let new = height.unwrap_or(now + delta.unwrap_or(0.0));
                    if !new.is_finite() {
                        return Err(ViewError::InvalidArgs(
                            "the height must be a number".to_owned(),
                        ));
                    }
                    let new = (new as f32).clamp(MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT);
                    self.heights.insert(track, new);
                }
                self.optimize_height = false;
                self.clamp_scroll(doc);
            }
            ids::FOCUS_TITLE_BAR => {
                parse::<bl_commands::NoArgs>(args)?;
                let first = self
                    .selected
                    .first()
                    .copied()
                    .or_else(|| doc.top_level().next())
                    .ok_or_else(|| unavailable("the set has no tracks"))?;
                self.selected = vec![first];
                self.selected_clips.clear();
                self.title_bars = true;
                self.show_track(doc, first);
            }
            ids::AUTOMATION_MODE => {
                let SwitchArgs { on } = parse(args)?;
                self.automation_mode = on.unwrap_or(!self.automation_mode);
                self.clamp_scroll(doc);
            }
            ids::AUTOMATION_ARM => {
                let SwitchArgs { on } = parse(args)?;
                self.automation_arm = on.unwrap_or(!self.automation_arm);
            }
            ids::SHOW_AUTOMATION => {
                let ShowAutomationArgs { track, target } = parse(args)?;
                self.targets(doc, Some(vec![track]))?;
                self.automation_lanes.insert(track, target);
                self.clamp_scroll(doc);
            }
            ids::SHOW_IN_OUT => {
                let SwitchArgs { on } = parse(args)?;
                self.show_in_out = on.unwrap_or(!self.show_in_out);
            }
            ids::SHOW_VOLUME => {
                let SwitchArgs { on } = parse(args)?;
                self.show_volume = on.unwrap_or(!self.show_volume);
            }
            ids::SHOW_RETURN_TRACKS => {
                let SwitchArgs { on } = parse(args)?;
                self.show_return_tracks = on.unwrap_or(!self.show_return_tracks);
                self.clamp_scroll(doc);
            }
            other => {
                return Err(ViewError::InvalidArgs(format!(
                    "`{other}` isn't a view command"
                )));
            }
        }
        Ok(self.state(doc))
    }

    fn restore(&mut self, saved: Saved) {
        self.optimize_width = None;
        self.set_horizontal(saved.horizontal);
        self.scroll_y = saved.scroll_y;
    }

    /// The view's state, as every view command returns it.
    #[must_use]
    pub fn state(&self, doc: &Document) -> ArrangementState {
        let horizontal = self.horizontal(doc);
        let rows = self.rows(doc);
        let top = self.scroll_y as f32;
        let visible_tracks = rows[rows::rows_between(&rows, top, top + self.viewport.height)]
            .iter()
            .map(|row| row.track)
            .collect();
        let signature = self.signature_at_left(doc, horizontal);
        ArrangementState {
            visible: horizontal.range(self.viewport.width),
            ticks_per_point: horizontal.ticks_per_point,
            scroll_y: self.scroll_y,
            visible_tracks,
            follow: self.follow,
            follow_paused: self.follow_paused,
            overview: self.overview,
            optimize_height: self.optimize_height,
            optimize_width: self.optimize_width.is_some(),
            grid: self.grid.state(signature, horizontal.ticks_per_point),
            insert_marker: self.insert_marker,
            selected_tracks: self.selected.clone(),
            selected_clips: self.selected_clips.clone(),
            selected_locators: self.selected_locators.clone(),
            time_selection: self
                .time_selection
                .map(|(start, end)| TimeRange { start, end }),
            time_selection_lanes: self.time_selection_lanes.clone(),
            folded_tracks: self.folded.iter().copied().collect(),
            zoom_back_steps: self.history.len(),
            automation_mode: self.automation_mode,
            automation_arm: self.automation_arm,
            automation_lanes: self
                .automation_lanes
                .iter()
                .map(|(track, target)| AutomationLane {
                    track: *track,
                    target: *target,
                })
                .collect(),
            show_in_out: self.show_in_out,
            show_volume: self.show_volume,
            show_return_tracks: self.show_return_tracks,
        }
    }

    /// Whether a switch-like view command is on: the state a menu shows as
    /// a check mark. `None` for commands that aren't switches.
    #[must_use]
    pub fn switch_state(&self, id: &str) -> Option<bool> {
        match id {
            ids::FOLLOW => Some(self.follow),
            ids::OVERVIEW => Some(self.overview),
            ids::OPTIMIZE_HEIGHT => Some(self.optimize_height),
            ids::OPTIMIZE_WIDTH => Some(self.optimize_width.is_some()),
            ids::TRIPLET_GRID => Some(self.grid.triplet),
            ids::SNAP_TO_GRID => Some(self.grid.snap),
            ids::FIXED_GRID => Some(self.grid.fixed_on),
            ids::AUTOMATION_MODE => Some(self.automation_mode),
            ids::AUTOMATION_ARM => Some(self.automation_arm),
            ids::SHOW_IN_OUT => Some(self.show_in_out),
            ids::SHOW_VOLUME => Some(self.show_volume),
            ids::SHOW_RETURN_TRACKS => Some(self.show_return_tracks),
            _ => None,
        }
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, ViewError> {
    let args = if args.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        args
    };
    serde_json::from_value(args).map_err(|error| ViewError::InvalidArgs(error.to_string()))
}
