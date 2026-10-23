//! The Arrangement View's view commands in the command registry (WP-1.8a;
//! decisions D16 and D17): zoom, scroll, Follow, the editing grid, the
//! Overview, track heights and folding, the insert marker, and the
//! selection the zoom commands work from; and, from WP-6.64, which of the
//! track headers' controls show.
//!
//! These change only what the Arrangement View shows, never the set, so
//! every one is in the read class. [`register`] adds their specs, which are
//! pure data, so `cargo xtask` can check them and list them in
//! the command reference; `bl-shell` runs them by ID. The behaviour they follow
//! is in the arrangement view spec.

use bl_commands::Context::{Arrangement, Global, SampleEditor, TakeLanes, TrackHeaders};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, ClipId, LocatorId, TakeLaneId, TrackId};

use crate::editing::Edge;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `arrangement.state`: what the Arrangement View shows.
pub const STATE: &str = "arrangement.state";
/// `arrangement.zoom`: zoom the time axis in or out by a factor.
pub const ZOOM: &str = "arrangement.zoom";
/// `arrangement.show_range`: show a span of time across the lanes.
pub const SHOW_RANGE: &str = "arrangement.show_range";
/// `arrangement.scroll`: scroll the lanes by a distance.
pub const SCROLL: &str = "arrangement.scroll";
/// `arrangement.show_track`: scroll a track into view.
pub const SHOW_TRACK: &str = "arrangement.show_track";
/// `arrangement.zoom_to_selection`: zoom to the time selection (Z).
pub const ZOOM_TO_SELECTION: &str = "arrangement.zoom_to_selection";
/// `arrangement.zoom_back`: go back to the view before a zoom (X).
pub const ZOOM_BACK: &str = "arrangement.zoom_back";
/// `arrangement.optimize_height`: fit every track into the lanes (H).
pub const OPTIMIZE_HEIGHT: &str = "arrangement.optimize_height";
/// `arrangement.optimize_width`: fit the whole arrangement across (W).
pub const OPTIMIZE_WIDTH: &str = "arrangement.optimize_width";
/// `arrangement.follow`: the Follow switch.
pub const FOLLOW: &str = "arrangement.follow";
/// `arrangement.overview`: show or hide the Overview.
pub const OVERVIEW: &str = "arrangement.overview";
/// `arrangement.narrow_grid`: make the grid finer (Cmd-1).
pub const NARROW_GRID: &str = "arrangement.narrow_grid";
/// `arrangement.widen_grid`: make the grid coarser (Cmd-2).
pub const WIDEN_GRID: &str = "arrangement.widen_grid";
/// `arrangement.triplet_grid`: switch the triplet grid (Cmd-3).
pub const TRIPLET_GRID: &str = "arrangement.triplet_grid";
/// `arrangement.snap_to_grid`: switch snapping (Cmd-4).
pub const SNAP_TO_GRID: &str = "arrangement.snap_to_grid";
/// `arrangement.fixed_grid`: switch between the fixed and the adaptive grid
/// (Cmd-5).
pub const FIXED_GRID: &str = "arrangement.fixed_grid";
/// `arrangement.set_grid`: choose grid settings directly.
pub const SET_GRID: &str = "arrangement.set_grid";
/// `arrangement.set_insert_marker`: place the insert marker.
pub const SET_INSERT_MARKER: &str = "arrangement.set_insert_marker";
/// `arrangement.select_tracks`: select tracks.
pub const SELECT_TRACKS: &str = "arrangement.select_tracks";
/// `arrangement.select_time`: select a span of time on tracks.
pub const SELECT_TIME: &str = "arrangement.select_time";
/// `arrangement.select_fade_handle`: select an audio clip's fade handle.
pub const SELECT_FADE_HANDLE: &str = "arrangement.select_fade_handle";
/// `arrangement.select_take_lanes`: select take lanes by their headers.
pub const SELECT_TAKE_LANES: &str = "arrangement.select_take_lanes";
/// `arrangement.show_take_lanes`: show or hide tracks' take lanes
/// (Cmd+Option+U).
pub const SHOW_TAKE_LANES: &str = "arrangement.show_take_lanes";
/// `arrangement.fold_tracks`: fold or unfold tracks.
pub const FOLD_TRACKS: &str = "arrangement.fold_tracks";
/// `arrangement.unfold_all`: unfold every track (Option-U).
pub const UNFOLD_ALL: &str = "arrangement.unfold_all";
/// `arrangement.fold_groups`: show or hide the tracks inside the selected
/// groups (+ and − on a group's title bar).
pub const FOLD_GROUPS: &str = "arrangement.fold_groups";
/// `arrangement.set_track_height`: make tracks taller or shorter.
pub const SET_TRACK_HEIGHT: &str = "arrangement.set_track_height";
/// `arrangement.focus_title_bar`: move to the highlighted track's title bar
/// (Esc).
pub const FOCUS_TITLE_BAR: &str = "arrangement.focus_title_bar";
/// `arrangement.automation_mode`: show or hide the tracks' automation lanes
/// (A).
pub const AUTOMATION_MODE: &str = "arrangement.automation_mode";
/// `arrangement.automation_arm`: arm or disarm Automation Arm (the Control
/// Bar's button).
pub const AUTOMATION_ARM: &str = "arrangement.automation_arm";
/// `arrangement.show_automation`: choose the parameter a track's automation
/// lane shows, or hide the lane.
pub const SHOW_AUTOMATION: &str = "arrangement.show_automation";
/// `arrangement.show_in_out`: show or hide the track headers' In/Out column.
pub const SHOW_IN_OUT: &str = "arrangement.show_in_out";
/// `arrangement.show_volume`: show or hide the track headers' volume, pan,
/// and sends.
pub const SHOW_VOLUME: &str = "arrangement.show_volume";
/// `arrangement.show_return_tracks`: show or hide the return tracks' rows.
pub const SHOW_RETURN_TRACKS: &str = "arrangement.show_return_tracks";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    STATE,
    ZOOM,
    SHOW_RANGE,
    SCROLL,
    SHOW_TRACK,
    ZOOM_TO_SELECTION,
    ZOOM_BACK,
    OPTIMIZE_HEIGHT,
    OPTIMIZE_WIDTH,
    FOLLOW,
    OVERVIEW,
    NARROW_GRID,
    WIDEN_GRID,
    TRIPLET_GRID,
    SNAP_TO_GRID,
    FIXED_GRID,
    SET_GRID,
    SET_INSERT_MARKER,
    SELECT_TRACKS,
    SELECT_TIME,
    SELECT_FADE_HANDLE,
    SELECT_TAKE_LANES,
    SHOW_TAKE_LANES,
    FOLD_TRACKS,
    UNFOLD_ALL,
    FOLD_GROUPS,
    SET_TRACK_HEIGHT,
    FOCUS_TITLE_BAR,
    AUTOMATION_MODE,
    AUTOMATION_ARM,
    SHOW_AUTOMATION,
    SHOW_IN_OUT,
    SHOW_VOLUME,
    SHOW_RETURN_TRACKS,
];

/// Whether `id` is one of the Arrangement View's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// How far a key press zooms: twice as close (+) or twice as far (−).
pub const KEY_ZOOM_FACTOR: f64 = 2.0;

/// The zoom-adaptive grid's settings, from the widest lines to the
/// narrowest (the grid context menus' Adaptive Grid choices, C-CLIP-ARR-020).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A zoom-adaptive grid setting: how far apart its lines stay on screen, from \
                   `widest` to `narrowest`. Each step is twice as dense as the one before. A new \
                   set uses `narrow`."
)]
pub enum AdaptiveGrid {
    /// The fewest lines.
    Widest,
    /// Fewer lines.
    Wide,
    /// Between.
    Medium,
    /// More lines; a new set's setting.
    #[default]
    Narrow,
    /// The most lines.
    Narrowest,
}

impl AdaptiveGrid {
    /// Every setting, widest first.
    pub const ALL: [Self; 5] = [
        Self::Widest,
        Self::Wide,
        Self::Medium,
        Self::Narrow,
        Self::Narrowest,
    ];

    /// The label the grid context menus show.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Widest => "Widest",
            Self::Wide => "Wide",
            Self::Medium => "Medium",
            Self::Narrow => "Narrow",
            Self::Narrowest => "Narrowest",
        }
    }
}

/// A fixed grid's spacing, from the widest to the narrowest (the grid
/// context menus' Fixed Grid choices, C-CLIP-ARR-021).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A fixed grid's spacing, whatever the zoom: `eight_bars`, `four_bars`, \
                   `two_bars`, `one_bar`, or a note value from `half` to `thirty_second`."
)]
pub enum FixedGrid {
    /// 8 bars.
    EightBars,
    /// 4 bars.
    FourBars,
    /// 2 bars.
    TwoBars,
    /// 1 bar.
    OneBar,
    /// A half note.
    Half,
    /// A quarter note.
    Quarter,
    /// An eighth note.
    Eighth,
    /// A sixteenth note.
    #[default]
    Sixteenth,
    /// A thirty-second note.
    ThirtySecond,
}

impl FixedGrid {
    /// Every spacing, widest first.
    pub const ALL: [Self; 9] = [
        Self::EightBars,
        Self::FourBars,
        Self::TwoBars,
        Self::OneBar,
        Self::Half,
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
    ];

    /// The label the grid context menus show.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::EightBars => "8 Bars",
            Self::FourBars => "4 Bars",
            Self::TwoBars => "2 Bars",
            Self::OneBar => "1 Bar",
            Self::Half => "1/2",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
        }
    }

    /// How many bars it spans, for the bar spacings.
    #[must_use]
    pub const fn bars(self) -> Option<u32> {
        match self {
            Self::EightBars => Some(8),
            Self::FourBars => Some(4),
            Self::TwoBars => Some(2),
            Self::OneBar => Some(1),
            _ => None,
        }
    }

    /// The note value's denominator, for the note spacings: 2 for a half
    /// note, 32 for a thirty-second.
    #[must_use]
    pub const fn note(self) -> Option<u32> {
        match self {
            Self::Half => Some(2),
            Self::Quarter => Some(4),
            Self::Eighth => Some(8),
            Self::Sixteenth => Some(16),
            Self::ThirtySecond => Some(32),
            _ => None,
        }
    }
}

/// A span of time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
    /// Where it starts.
    pub start: Ticks,
    /// Where it ends, after `start`.
    pub end: Ticks,
}

/// The editing grid, as the Arrangement View uses it now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GridState {
    /// The zoom-adaptive setting, used while `fixed` is off.
    pub adaptive: AdaptiveGrid,
    /// The fixed spacing, used while `fixed` is on.
    pub fixed_spacing: FixedGrid,
    /// Whether the grid is fixed rather than zoom-adaptive.
    pub fixed: bool,
    /// Whether the grid divides note values into triplets.
    pub triplet: bool,
    /// Whether the insert marker and selections snap to the grid.
    pub snap: bool,
    /// The spacing of the grid's lines at the current zoom.
    pub step: Ticks,
    /// The spacing as the grid indicator in the lanes' lower right shows it,
    /// such as `1/16`, `1/8T`, `1/1`, or `4 Bars`.
    pub label: String,
}

/// What the Arrangement View shows: every view command returns it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArrangementState {
    /// The span of time across the track lanes, left edge to right edge.
    pub visible: TimeRange,
    /// The zoom: ticks of song time per point of screen.
    pub ticks_per_point: f64,
    /// How far the track lanes are scrolled down, in points.
    pub scroll_y: f64,
    /// The tracks at least partly in view, top to bottom (the return tracks
    /// and Main, pinned at the bottom, are always in view and not listed).
    pub visible_tracks: Vec<TrackId>,
    /// Whether Follow is on.
    pub follow: bool,
    /// Whether Follow is on but paused, because the view was scrolled by
    /// hand while the song played; it resumes when playback starts or
    /// stops, or a click lands in the track lanes or the scrub area.
    pub follow_paused: bool,
    /// Whether the Overview shows above the rulers.
    pub overview: bool,
    /// Whether Optimize Height is on: every track fits in the lanes.
    pub optimize_height: bool,
    /// Whether Optimize Width is on: the whole arrangement fits across.
    pub optimize_width: bool,
    /// The editing grid.
    pub grid: GridState,
    /// Where the insert marker is.
    pub insert_marker: Ticks,
    /// The selected tracks, top to bottom.
    pub selected_tracks: Vec<TrackId>,
    /// The selected clips (WP-1.8b), in the order they were selected. The
    /// editing commands act on these when there are any, and on the time
    /// selection otherwise.
    #[serde(default)]
    pub selected_clips: Vec<ClipId>,
    /// The take lanes whose headers are selected (the comping spec, R26), in
    /// the order they were selected. Their tracks are selected tracks too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_take_lanes: Vec<TakeLaneId>,
    /// The tracks whose take lanes show (the comping spec, R24).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown_take_lanes: Vec<TrackId>,
    /// The selected locators (WP-6.24), in the order they were selected.
    /// The locator commands act on these.
    #[serde(default)]
    pub selected_locators: Vec<LocatorId>,
    /// The selected fade handle (WP-6.51), if any: Delete deletes its fade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_fade: Option<FadeHandleState>,
    /// The selected span of time on the selected tracks, if any.
    pub time_selection: Option<TimeRange>,
    /// The lanes the time selection covers: each a track's own lane, a take
    /// lane under it, or its automation lane (the automation editing spec,
    /// R1; the comping spec, R26). Empty when the selection names no lane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub time_selection_lanes: Vec<Lane>,
    /// The folded tracks.
    pub folded_tracks: Vec<TrackId>,
    /// How many views `arrangement.zoom_back` can go back to.
    pub zoom_back_steps: usize,
    /// Whether Automation Mode is on: each track shows its automation lane
    /// under its lane, with the device and parameter choosers.
    pub automation_mode: bool,
    /// Whether Automation Arm is on: while Arrangement Record runs, moving a
    /// control writes its values into the parameter's envelope.
    pub automation_arm: bool,
    /// The parameter each track's automation lane shows, for the tracks whose
    /// lane isn't showing the default (their volume): the choosers' choice.
    /// `target` `null` means the lane draws no envelope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation_lanes: Vec<AutomationLane>,
    /// Whether the track headers show their In/Out column.
    pub show_in_out: bool,
    /// Whether the track headers show their volume, pan, and sends.
    pub show_volume: bool,
    /// Whether the return tracks' rows show in the lanes.
    pub show_return_tracks: bool,
}

/// One track's automation lane and the parameter it shows, as
/// `arrangement.show_automation` last set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AutomationLane {
    /// The track.
    pub track: TrackId,
    /// The parameter the lane shows, or `null` while it draws no envelope.
    pub target: Option<AutomationTarget>,
}

/// Which lane a selection covers: a track's own lane, where its clips are, a
/// take lane under it, or the automation lane under it, which shows a
/// parameter (the automation editing spec, R1; the comping spec, R26).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "Which lane a selection covers: `track` for a track's own lane, where its \
                   clips are, `take` for one of its take lanes, and `automation` for the \
                   automation lane under it, which shows a parameter."
)]
pub enum Lane {
    /// A track's own lane, where its clips are.
    Track {
        /// The track.
        track: TrackId,
    },
    /// One of a track's take lanes.
    Take {
        /// The track.
        track: TrackId,
        /// The take lane.
        lane: TakeLaneId,
    },
    /// A track's automation lane, which shows a parameter.
    Automation {
        /// The track.
        track: TrackId,
        /// The parameter the lane shows.
        target: AutomationTarget,
    },
}

impl Lane {
    /// The track the lane belongs to.
    #[must_use]
    pub const fn track(&self) -> TrackId {
        match *self {
            Self::Track { track } | Self::Take { track, .. } | Self::Automation { track, .. } => {
                track
            }
        }
    }

    /// The take lane, for a take lane.
    #[must_use]
    pub const fn take_lane(&self) -> Option<TakeLaneId> {
        match *self {
            Self::Take { lane, .. } => Some(lane),
            Self::Track { .. } | Self::Automation { .. } => None,
        }
    }

    /// The parameter the lane shows, for an automation lane.
    #[must_use]
    pub const fn target(&self) -> Option<AutomationTarget> {
        match *self {
            Self::Track { .. } | Self::Take { .. } => None,
            Self::Automation { target, .. } => Some(target),
        }
    }
}

/// Arguments of `arrangement.zoom`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZoomArgs {
    /// How much closer to look: 2 shows half as much time, 0.5 twice as
    /// much. From 1/64 to 64.
    pub factor: f64,
    /// The position that stays put on screen. Left out, it's the middle of
    /// the time selection if it's in view, otherwise the insert marker if
    /// that's in view, otherwise the middle of the view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Ticks>,
}

/// Arguments of `arrangement.show_range` and `arrangement.select_time`'s
/// span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowRangeArgs {
    /// The position at the lanes' left edge.
    pub start: Ticks,
    /// The position at the lanes' right edge, after `start`.
    pub end: Ticks,
}

/// Arguments of `arrangement.scroll`. Fields left out are 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollArgs {
    /// How far to scroll sideways, in points: positive moves later in the
    /// song.
    #[serde(default)]
    pub x: f64,
    /// How far to scroll the tracks, in points: positive moves down the
    /// track list.
    #[serde(default)]
    pub y: f64,
}

/// Arguments of `arrangement.show_track`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowTrackArgs {
    /// The track to bring into view.
    pub track: TrackId,
}

/// Arguments of the switch commands (`arrangement.follow`,
/// `arrangement.overview`, the grid switches, and Optimize Height and
/// Width).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchArgs {
    /// On or off. Left out, the switch toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `arrangement.follow`. With neither field, Follow toggles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowArgs {
    /// Switch Follow on or off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// Pause following (true), as clicking the beat-time ruler does while
    /// the song plays, or resume it (false), as clicking the scrub area
    /// does, without switching Follow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
}

/// Arguments of `arrangement.set_grid`. Fields left out keep their values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetGridArgs {
    /// Choose this zoom-adaptive setting and make the grid adaptive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptive: Option<AdaptiveGrid>,
    /// Choose this fixed spacing and make the grid fixed. Give `adaptive`
    /// or `fixed`, not both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<FixedGrid>,
    /// Whether note values divide into triplets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triplet: Option<bool>,
    /// Whether to snap to the grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap: Option<bool>,
}

/// Arguments of `arrangement.set_insert_marker`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertMarkerArgs {
    /// Where to put it. It isn't snapped: snap first if you want the grid.
    pub at: Ticks,
    /// The track to select with it, as clicking a track's lane does. Left
    /// out, the selected tracks stay selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The take lane the click was on, as clicking a take lane's lane does:
    /// the lane is selected with the marker, and its track with it (the
    /// comping spec, R26).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<TakeLaneId>,
}

/// Arguments of `arrangement.select_tracks`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectTracksArgs {
    /// The tracks to select. Empty clears the track selection.
    pub tracks: Vec<TrackId>,
    /// Add them to the tracks already selected instead of replacing them.
    #[serde(default)]
    pub add: bool,
    /// Select their title bars, so the track-header shortcuts (fold, show
    /// and hide grouped tracks) apply, as clicking a title bar does.
    #[serde(default)]
    pub title_bars: bool,
}

/// Arguments of `arrangement.select_time`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectTimeArgs {
    /// Where the selection starts.
    pub start: Ticks,
    /// Where it ends. Before `start` is fine: the two are swapped.
    pub end: Ticks,
    /// The tracks it covers. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// The lanes it covers: each a track's own lane, a take lane under it, or
    /// its automation lane (the automation editing spec, R1; the comping spec,
    /// R26). Left out, the tracks' own lanes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<Lane>>,
}

/// Which of a fade's handles (the audio clips spec, R27).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A fade's handle: `peak`, where it reaches full level; `outer`, where it \
                   reaches silence, at the clip's edge or a crossfade's far end; or `curve`, \
                   between them."
)]
pub enum FadeHandle {
    /// Where the fade reaches full level.
    Peak,
    /// Where it reaches silence.
    Outer,
    /// The handle that shapes it.
    Curve,
}

/// A selected fade handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FadeHandleState {
    /// The audio clip.
    pub clip: ClipId,
    /// Which edge's fade: the fade-in at `start`, the fade-out at `end`.
    pub edge: Edge,
    /// Which of its handles.
    pub handle: FadeHandle,
}

/// Arguments of `arrangement.select_fade_handle`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectFadeHandleArgs {
    /// The audio clip. Left out, no handle is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Which edge's fade: the fade-in at `start`, the fade-out at `end`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<Edge>,
    /// Which of the fade's handles; `peak` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<FadeHandle>,
}

/// Arguments of `arrangement.fold_tracks`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FoldArgs {
    /// The tracks. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// Fold (true) or unfold (false). Left out, each track toggles, taking
    /// the first track's new state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fold: Option<bool>,
}

/// Arguments of `arrangement.select_take_lanes`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectTakeLanesArgs {
    /// The take lanes whose headers to select. Empty clears the selection of
    /// lane headers.
    pub lanes: Vec<TakeLaneId>,
    /// Add them to the lanes already selected instead of replacing them.
    #[serde(default)]
    pub add: bool,
    /// Take away from the selection each of them that is selected, as
    /// Cmd-clicking a header does, and add the others.
    #[serde(default)]
    pub toggle: bool,
}

/// Arguments of `arrangement.show_take_lanes`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowTakeLanesArgs {
    /// The tracks. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// Show (true) or hide (false) their take lanes. Left out, they toggle,
    /// taking the first track's new state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// Also select the tracks' title bars and leave the take lanes' headers,
    /// as Left does from a take lane's header.
    #[serde(default)]
    pub title_bars: bool,
}

/// Arguments of `arrangement.fold_groups`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FoldGroupsArgs {
    /// Hide the grouped tracks (true) or show them (false).
    pub fold: bool,
}

/// Arguments of `arrangement.set_track_height`. Give `height` or `delta`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackHeightArgs {
    /// The tracks. Left out, the selected take lanes if any are, otherwise
    /// the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// Take lanes to resize as well (the comping spec, R28).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_lanes: Option<Vec<TakeLaneId>>,
    /// The new height of each unfolded track or take lane, in points, from 18
    /// to 800.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Points to add to each track's height (negative makes it shorter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
}

/// Arguments of `arrangement.show_automation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowAutomationArgs {
    /// The track whose automation lane to show.
    pub track: TrackId,
    /// The parameter the lane shows: a mixer control, a send, or a device
    /// parameter. `null` (or nothing) hides the track's automation lane.
    #[serde(default)]
    pub target: Option<AutomationTarget>,
}

/// Adds the Arrangement View's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

/// A view command: read class, returning the view's state.
fn view(id: &'static str, label: &'static str, description: &'static str) -> CommandSpec {
    CommandSpec::new(id, label)
        .description(description)
        .permission(Permission::Read)
        .result::<ArrangementState>()
}

#[expect(
    clippy::too_many_lines,
    reason = "one spec per command, written out in full"
)]
fn specs() -> Vec<CommandSpec> {
    vec![
        view(
            STATE,
            "Arrangement View State",
            "Reports what the Arrangement View shows: the span of time across the track lanes, \
             the tracks in view, the zoom, Follow, the editing grid, the insert marker, and the \
             selection. Changes nothing.",
        )
        .contexts([Global, Arrangement])
        .args::<NoArgs>(),
        view(
            ZOOM,
            "Zoom Arrangement",
            "Zooms the Arrangement View's time axis by `factor` around `anchor`: 2 shows half as \
             much time, 0.5 twice as much. The + and - keys zoom by 2 around the selection, the \
             insert marker, or the middle of the view. Changes only the view, never the set; \
             turns Optimize Width off.",
        )
        .context(Arrangement)
        .binding(Binding::new("S-ZOOM-003").arg("factor", KEY_ZOOM_FACTOR))
        .binding(Binding::new("S-ZOOM-004").arg("factor", 1.0 / KEY_ZOOM_FACTOR))
        .args::<ZoomArgs>(),
        view(
            SHOW_RANGE,
            "Show Time Range",
            "Zooms and scrolls the Arrangement View so the track lanes show exactly the span \
             from `start` to `end`, and remembers the previous view for \
             `arrangement.zoom_back`. Use it to look at a section of the song. Changes only the \
             view; pauses Follow while the song plays.",
        )
        .contexts([Global, Arrangement])
        .args::<ShowRangeArgs>(),
        view(
            SCROLL,
            "Scroll Arrangement",
            "Scrolls the Arrangement View by `x` points sideways (positive goes later in the \
             song) and `y` points down the track list, as the scroll wheel, Shift with the \
             wheel, and dragging with Cmd-Option (Ctrl-Alt) held do. It can't scroll before \
             1.1.1 or past the last track. Scrolling sideways while the song plays pauses \
             Follow. Changes only the view; turns Optimize Width off when it scrolls sideways.",
        )
        .context(Arrangement)
        .binding("S-ZOOM-006")
        .binding("S-ZOOM-007")
        .binding("S-ARR-024")
        .args::<ScrollArgs>(),
        view(
            SHOW_TRACK,
            "Show Track",
            "Scrolls the track lanes the least distance that brings `track` fully into view. \
             The return tracks and Main are always in view. Changes only the view.",
        )
        .contexts([Global, Arrangement])
        .args::<ShowTrackArgs>(),
        view(
            ZOOM_TO_SELECTION,
            "Zoom to Time Selection",
            "Zooms the Arrangement View so the time selection fills the lanes, or to the whole \
             arrangement if nothing is selected, and remembers the previous view; \
             `arrangement.zoom_back` returns to it. Run again straight after, it goes back, so \
             holding Z zooms only while it's held. Changes only the view.",
        )
        .context(Arrangement)
        .binding("S-ARR-030")
        .args::<NoArgs>(),
        view(
            ZOOM_BACK,
            "Zoom Back from Time Selection",
            "Returns the Arrangement View to the view before the last zoom to a selection, \
             range, or the whole arrangement, stepping further back each time. Can't run when \
             there's nothing to go back to. Changes only the view.",
        )
        .context(Arrangement)
        .binding("S-ARR-031")
        .args::<NoArgs>(),
        view(
            OPTIMIZE_HEIGHT,
            "Optimize Height",
            "Switches Optimize Height: while it's on, every track shares the lanes' height so \
             all of them are in view, and they keep fitting as tracks come and go. Turning it \
             off gives the tracks back their own heights. Changes only the view.",
        )
        .context(Arrangement)
        .binding("S-ARR-025")
        .args::<SwitchArgs>(),
        view(
            OPTIMIZE_WIDTH,
            "Optimize Width",
            "Switches Optimize Width: while it's on, the whole arrangement, from 1.1.1 to the \
             end of its last clip, fits across the lanes. Turning it off returns to the view \
             before; zooming or scrolling sideways turns it off where the view is. Changes only \
             the view.",
        )
        .context(Arrangement)
        .binding("S-ARR-026")
        .args::<SwitchArgs>(),
        view(
            FOLLOW,
            "Follow",
            "Switches Follow, the Control Bar's switch: while it's on and the song plays, the \
             Arrangement View turns the page whenever the playhead leaves the view, so the \
             playhead stays in sight. Scrolling sideways by hand, or clicking the beat-time \
             ruler, pauses it until playback starts or stops, or a click lands in the track \
             lanes or the scrub area; `paused` does the same directly. The Sample Editor \
             follows playback on the same switch (Option-Shift-F there too). With no arguments it \
             toggles Follow, which also resumes it. Changes only the view.",
        )
        .contexts([Global, Arrangement, SampleEditor])
        .binding("S-ZOOM-005")
        .binding("S-ARR-023")
        .binding("S-SAMPLE-005")
        .args::<FollowArgs>(),
        view(
            OVERVIEW,
            "Overview",
            "Shows or hides the Arrangement View's Overview, the strip above the rulers that \
             shows the whole arrangement with the part in view outlined. Changes only the view.",
        )
        .contexts([Global, Arrangement])
        .binding("S-VIEW-011")
        .args::<SwitchArgs>(),
        view(
            NARROW_GRID,
            "Narrow Grid",
            "Makes the Arrangement's editing grid twice as dense: the next narrower adaptive \
             setting, or the next finer fixed spacing. Can't run at the narrowest. The grid \
             decides where the insert marker and selections snap. Changes only the view.",
        )
        .context(Arrangement)
        .binding("S-GRID-002")
        .args::<NoArgs>(),
        view(
            WIDEN_GRID,
            "Widen Grid",
            "Makes the Arrangement's editing grid half as dense: the next wider adaptive \
             setting, or the next coarser fixed spacing. Can't run at the widest. Changes only \
             the view.",
        )
        .context(Arrangement)
        .binding("S-GRID-003")
        .args::<NoArgs>(),
        view(
            TRIPLET_GRID,
            "Triplet Grid",
            "Switches the Arrangement's triplet grid, which divides note values in three \
             instead of two (1/8T instead of 1/8). Can't switch on while the grid is a bar or \
             more apart. Changes only the view.",
        )
        .context(Arrangement)
        .binding("S-GRID-004")
        .args::<SwitchArgs>(),
        view(
            SNAP_TO_GRID,
            "Snap to Grid",
            "Switches snapping to the Arrangement's editing grid: while it's on, clicks place \
             the insert marker and selections on the nearest grid line. Changes only the view.",
        )
        .context(Arrangement)
        .binding("S-GRID-005")
        .args::<SwitchArgs>(),
        view(
            FIXED_GRID,
            "Fixed Grid",
            "Switches the Arrangement's editing grid between fixed, which keeps its spacing \
             (such as 1/16) at any zoom, and zoom-adaptive, which keeps its lines a set \
             distance apart on screen as the view zooms. Each keeps its own setting. Changes \
             only the view.",
        )
        .context(Arrangement)
        .binding("S-GRID-006")
        .args::<SwitchArgs>(),
        view(
            SET_GRID,
            "Grid",
            "Chooses the Arrangement's editing grid directly, as its context menu does: an \
             adaptive setting (`widest` to `narrowest`) or a fixed spacing (`eight_bars` to \
             `thirty_second`), and the triplet and snap switches. Fields left out keep their \
             values. Changes only the view.",
        )
        .context(Arrangement)
        .args::<SetGridArgs>(),
        view(
            SET_INSERT_MARKER,
            "Set Insert Marker",
            "Places the Arrangement's insert marker at `at` and, with `track`, selects that \
             track, as clicking in a track's lane does; a click also snaps to the grid when Snap \
             to Grid is on, and resumes Follow. Clears the time selection. Not an undo step; \
             changes only the view.",
        )
        .context(Arrangement)
        .args::<InsertMarkerArgs>(),
        view(
            SELECT_TRACKS,
            "Select Tracks",
            "Selects tracks in the Arrangement, replacing the selection or, with `add`, adding \
             to it; with `title_bars`, their title bars, as clicking a title bar does, so the \
             fold and grouped-track keys apply. Changes only the view.",
        )
        .context(Arrangement)
        .args::<SelectTracksArgs>(),
        view(
            SELECT_TIME,
            "Select Time",
            "Selects the span from `start` to `end` on `tracks` (or the selected tracks), as \
             dragging across track lanes does. `lanes` names the lanes the selection covers, \
             each a track's own lane, one of its take lanes or its automation lane; left out, \
             the tracks' own lanes. `arrangement.zoom_to_selection` zooms to it. Changes only \
             the view.",
        )
        .context(Arrangement)
        .args::<SelectTimeArgs>(),
        view(
            SELECT_FADE_HANDLE,
            "Select Fade Handle",
            "Selects one of an audio clip's fade handles, as clicking it does: the `peak` \
             handle where the fade reaches full level, the `outer` handle where it reaches \
             silence, or the `curve` handle, of the fade-in at the clip's `start` or the \
             fade-out at its `end`. A selected handle shows how far it can go, and Delete \
             deletes its fade (`arrangement.delete_fades`). Left without `clip`, it clears the \
             selection. Changes only the view.",
        )
        .context(Arrangement)
        .args::<SelectFadeHandleArgs>(),
        view(
            SELECT_TAKE_LANES,
            "Select Take Lanes",
            "Selects take lanes by their headers, as clicking a take lane's header does, \
             replacing the lane headers selected or, with `add`, adding to them; with `toggle`, \
             Cmd-clicking: each lane selected is taken away and the others added. Their tracks \
             become the selected tracks, and the clips and time selected are let go, so \
             Enter, T, Cmd+D, Delete and Cmd+Up and Cmd+Down act on the lanes. An empty list \
             clears the selection of lane headers. Changes only the view.",
        )
        .context(Arrangement)
        .args::<SelectTakeLanesArgs>(),
        view(
            SHOW_TAKE_LANES,
            "Show Take Lanes",
            "Shows or hides the take lanes of `tracks` (or the selected tracks) under their main \
             lanes, as the track header's Show Take Lanes item and the Show/Hide Take Lanes \
             toggle do; left without `on`, they toggle. Showing take lanes switches Automation \
             Mode off, which hides them. With `title_bars`, the tracks' title bars are selected \
             too and the take lane headers are let go, as Left does from a take lane's header. \
             Never an undo step; changes only the view.",
        )
        .contexts([Arrangement, TakeLanes])
        .binding("S-COMP-001")
        .binding(
            Binding::new("S-COMP-007")
                .arg("on", false)
                .arg("title_bars", true),
        )
        .args::<ShowTakeLanesArgs>(),
        view(
            FOLD_TRACKS,
            "Fold Track",
            "Folds tracks, so each shows only its title row, or unfolds them to their own \
             height; a folded group track also hides the tracks inside it. Acts on `tracks` or \
             the selected tracks; left without `fold`, it toggles. The fold button beside a \
             track's name does the same, and U toggles the selected tracks. Changes only the \
             view.",
        )
        .contexts([Arrangement, TrackHeaders])
        .binding("S-TRACK-010")
        .args::<FoldArgs>(),
        view(
            UNFOLD_ALL,
            "Unfold All Tracks",
            "Unfolds every track in the Arrangement, showing every grouped track too. Changes \
             only the view.",
        )
        .context(Arrangement)
        .binding("S-ARR-021")
        .args::<NoArgs>(),
        view(
            FOLD_GROUPS,
            "Show Grouped Tracks",
            "Shows (`fold` false) or hides (`fold` true) the tracks inside the selected group \
             tracks, as + and - do with a group's title bar selected. Can't run unless a group \
             track is selected. Changes only the view.",
        )
        .context(TrackHeaders)
        .binding(Binding::new("S-TRACK-008").arg("fold", false))
        .binding(Binding::new("S-TRACK-009").arg("fold", true))
        .args::<FoldGroupsArgs>(),
        view(
            SET_TRACK_HEIGHT,
            "Set Track Height",
            "Sets the height of unfolded tracks (`tracks`, or the selected ones) and take lanes \
             (`take_lanes`, or the selected ones when they are) to `height` points, or changes \
             it by `delta`, from 18 to 800, as dragging a track's lower edge and Option with + \
             or - (Alt on Windows) do. Turns Optimize Height off. Changes only the view.",
        )
        .context(Arrangement)
        .args::<TrackHeightArgs>(),
        view(
            FOCUS_TITLE_BAR,
            "Go to Track Title Bar",
            "Selects the title bar of the highlighted track (the first selected track, or the \
             top track), as Esc does from a track's controls, so the track-header keys apply. \
             Changes only the view.",
        )
        .context(Arrangement)
        .binding("S-ARR-034")
        .args::<NoArgs>(),
        view(
            AUTOMATION_MODE,
            "Automation Mode",
            "Switches Automation Mode: while it's on, every track shows an automation lane under \
             its lane, with the device and parameter choosers that pick the parameter it shows, \
             and its envelope is drawn and edited there. A (the View menu item and the button \
             above the track headers) does the same. Changes only the view.",
        )
        .contexts([Global, Arrangement])
        .binding("S-ENV-001")
        .args::<SwitchArgs>(),
        view(
            AUTOMATION_ARM,
            "Automation Arm",
            "Switches Automation Arm, the Control Bar's button beside Arrangement Record: while \
             it's on and the transport records, moving a control (a track's volume, pan, mute, or \
             send, or a device's parameter) writes its values into that parameter's envelope at \
             the playback position, replacing what was there over the range written, and the \
             envelope resumes where the control is released. The whole recording is one undo \
             step, made when recording stops. Changes only the view until then.",
        )
        .contexts([Global, Arrangement])
        .args::<SwitchArgs>(),
        view(
            SHOW_AUTOMATION,
            "Show Automation",
            "Picks the parameter `track`'s automation lane shows, as its device chooser (the \
             track mixer, one of its devices, or `null` for None) and parameter chooser do. A \
             track's lane shows its volume until the choosers pick another parameter; with \
             `target` `null` it draws no envelope, while the lane and its choosers stay. Changes \
             only the view.",
        )
        .contexts([Global, Arrangement])
        .args::<ShowAutomationArgs>(),
        view(
            SHOW_IN_OUT,
            "Arrangement Track Controls In/Out",
            "Shows or hides the Arrangement's track headers' In/Out column (the input and output \
             routing and monitoring), as View > Arrangement Track Controls > In/Out does. \
             Switches unless `on` says which. Changes only the view; the mixers' own In/Out \
             section is a switch of its own.",
        )
        .contexts([Global, Arrangement])
        .args::<SwitchArgs>(),
        view(
            SHOW_VOLUME,
            "Arrangement Track Controls Volume",
            "Shows or hides the Arrangement's track headers' volume, pan, and sends, as View > \
             Arrangement Track Controls > Volume does. Switches unless `on` says which. Changes \
             only the view; the mixers' own sections are switches of their own.",
        )
        .contexts([Global, Arrangement])
        .args::<SwitchArgs>(),
        view(
            SHOW_RETURN_TRACKS,
            "Arrangement Track Controls Return Tracks",
            "Shows or hides the return tracks' rows in the Arrangement's lanes, as View > \
             Arrangement Track Controls > Return Tracks does. Switches unless `on` says which. \
             Changes only the view; the returns play as before, and the mixers' own Return Tracks \
             section is a switch of its own.",
        )
        .contexts([Global, Arrangement])
        .args::<SwitchArgs>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
            assert_eq!(
                registry.get(id).unwrap().permission,
                Some(Permission::Read),
                "{id} only changes the view"
            );
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn grid_choices_run_widest_first() {
        assert!(AdaptiveGrid::Widest < AdaptiveGrid::Narrowest);
        assert_eq!(AdaptiveGrid::default(), AdaptiveGrid::Narrow);
        assert_eq!(FixedGrid::ALL.map(FixedGrid::label)[3], "1 Bar");
        assert!(
            FixedGrid::ALL
                .iter()
                .all(|g| g.bars().is_some() != g.note().is_some())
        );
    }
}
