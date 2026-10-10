//! The Arrangement View's view commands in the command registry (WP-1.8a;
//! decisions D16 and D17): zoom, scroll, Follow, the editing grid, the
//! Overview, track heights and folding, the insert marker, and the
//! selection the zoom commands work from.
//!
//! These change only what the Arrangement View shows, never the set, so
//! every one is in the read class. [`register`] adds their specs, which are
//! pure data, so `cargo xtask` can check them and list them in
//! `docs/commands.md`; `bl-shell` runs them by ID. The behaviour they follow
//! is in `docs/specs/arrangement-view.md`.

use bl_commands::Context::{Arrangement, Global, TrackHeaders};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{ClipId, TrackId};
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
    FOLD_TRACKS,
    UNFOLD_ALL,
    FOLD_GROUPS,
    SET_TRACK_HEIGHT,
    FOCUS_TITLE_BAR,
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
    /// The selected span of time on the selected tracks, if any.
    pub time_selection: Option<TimeRange>,
    /// The folded tracks.
    pub folded_tracks: Vec<TrackId>,
    /// How many views `arrangement.zoom_back` can go back to.
    pub zoom_back_steps: usize,
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
    /// The tracks. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// The new height of each unfolded track, in points, from 18 to 800.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Points to add to each track's height (negative makes it shorter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
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
             lanes or the scrub area; `paused` does the same directly. With no arguments it \
             toggles Follow, which also resumes it. Changes only the view.",
        )
        .contexts([Global, Arrangement])
        .binding("S-ZOOM-005")
        .binding("S-ARR-023")
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
             dragging across track lanes does. `arrangement.zoom_to_selection` zooms to it. \
             Changes only the view.",
        )
        .context(Arrangement)
        .args::<SelectTimeArgs>(),
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
            "Sets the height of unfolded tracks (`tracks`, or the selected ones) to `height` \
             points, or changes it by `delta`, from 18 to 800, as dragging a track's lower edge \
             and Option with + or - (Alt on Windows) do. Turns Optimize Height off. Changes \
             only the view.",
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
