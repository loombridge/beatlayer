//! The layout layer's mixer (WP-1.9): the Session View's columns and its
//! mixer, the Arrangement's mixer, and the controls inside a strip, from
//! layout.md's L-SES, L-MIX, and L-ARR-018 rows measured in Live 12.4.3 at
//! 1600 × 1000 points, and the strip's insides observed in Live 12.4.6 on
//! 11 October 2026 (the mixer view spec, R1 to R5).
//!
//! The columns' widths and where they start, the returns and Main at the
//! right, the title bars, the status row, the sections' heights and order,
//! the colour strip, the Arrangement's mixer, and every control inside a
//! strip are placed where Live has them. Only the In/Out rows' and the send
//! knobs' sizes, which no observation has measured yet, are Beatlayer's
//! ([`unmeasured`]).

use bl_model::{ChainId, DeviceId};
use egui::{Rect, pos2};

pub use bl_shell::mixer_view_commands::{
    COLUMN_WIDTH, EXTENDED_METERS_HEIGHT, MIN_STRIP_HEIGHT, PAN_HIDES_BELOW, TICK_LABELS_WIDTH,
};

use super::Row;

/// A Session title bar's height (L-SES-002: y 61 to 78).
pub const TITLE_BAR_HEIGHT: f32 = 17.0;

/// The Session's track status row's height, under the slots and above the
/// mixer (L-SES-008).
pub const STATUS_ROW: f32 = 17.0;

/// One clip slot row's height (L-SES-003: 18 points per scene).
pub const SLOT_ROW: f32 = 18.0;

/// The Main column's scene row parts (L-SES-006), measured from its edges:
/// the scene launch button at its left, the scene's name across the middle,
/// and its number at its right edge.
pub mod scene_controls {
    /// The scene launch button's width, at the column's left edge.
    pub const LAUNCH: f32 = 18.0;
    /// The scene's number's width, at the column's right edge.
    pub const NUMBER: f32 = 22.0;
}

/// One scene's row (L-SES-003, L-SES-006): the whole row across the grid,
/// the Main column's launch button, and where its name and number sit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneRow {
    /// The row across the grid, 18 points tall.
    pub row: Rect,
    /// The scene launch button in the Main column (I-CLICK-009).
    pub launch: Rect,
    /// Where the scene's name is drawn, between the button and the number.
    pub name: Rect,
    /// Where the scene's number is drawn, at the Main column's right edge.
    pub number: Rect,
}

impl SceneRow {
    /// Places a scene's row against the Main column.
    #[must_use]
    pub fn compute(row: Rect, main: Rect) -> Self {
        use scene_controls::{LAUNCH, NUMBER};
        let launch = Rect::from_min_max(
            pos2(main.left(), row.top()),
            pos2(main.left() + LAUNCH, row.bottom()),
        );
        let number = Rect::from_min_max(
            pos2(main.right() - NUMBER, row.top()),
            pos2(main.right(), row.bottom()),
        );
        let name = Rect::from_min_max(
            pos2(launch.right(), row.top()),
            pos2(number.left(), row.bottom()),
        );
        Self {
            row,
            launch,
            name,
            number,
        }
    }
}

/// One track's parts of the status row (L-SES-008): the stop button at the
/// column's left, and the status display beside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusCells {
    /// The track's stop button.
    pub stop: Rect,
    /// The track's status display.
    pub field: Rect,
}

impl StatusCells {
    /// Places a track column's status cells in the status row.
    #[must_use]
    pub fn track(column: Rect, row: Rect) -> Self {
        let stop = Rect::from_min_max(
            pos2(column.left(), row.top()),
            pos2(column.left() + unmeasured::STATUS_BUTTON, row.bottom()),
        );
        let field = Rect::from_min_max(
            pos2(stop.right(), row.top()),
            pos2(column.right(), row.bottom()),
        );
        Self { stop, field }
    }
}

/// The Main column's parts of the status row (L-SES-008, L-SES-012): Stop
/// All Clips at its left, and at its right the global follow-action switch
/// and the Back to Arrangement button.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MainStatusCells {
    /// Stop All Clips.
    pub stop_all: Rect,
    /// Enable Follow Actions Globally.
    pub follow: Rect,
    /// Back to Arrangement.
    pub back: Rect,
}

impl MainStatusCells {
    /// Places the Main column's status cells in the status row.
    #[must_use]
    pub fn compute(main: Rect, row: Rect) -> Self {
        let button = |left: f32| {
            Rect::from_min_max(
                pos2(left, row.top()),
                pos2(left + unmeasured::STATUS_BUTTON, row.bottom()),
            )
        };
        let back = button(main.right() - unmeasured::STATUS_BUTTON);
        Self {
            stop_all: button(main.left()),
            follow: button(back.left() - unmeasured::STATUS_BUTTON),
            back,
        }
    }
}

/// The In/Out section's height (L-MIX-001).
pub const IN_OUT_HEIGHT: f32 = 115.0;

/// The Sends section's height (L-MIX-002).
pub const SENDS_HEIGHT: f32 = 56.0;

/// The mixer strip's height in a new set (L-MIX-003).
pub const STRIP_HEIGHT: f32 = bl_shell::mixer_view_commands::STRIP_HEIGHT;

/// The Crossfader section's height under the strip (L-MIX-006; its
/// controls' sizes are [`unmeasured`]).
pub const CROSSFADER_HEIGHT: f32 = 28.0;

/// The Track Options section's height under the strip (L-MIX-005).
pub const TRACK_OPTIONS_HEIGHT: f32 = 26.0;

/// The Devices section's height, between In/Out and Sends (IMP-25, the
/// mixer view spec R25): room for eight rows of [`DEVICE_ROW`].
pub const DEVICES_HEIGHT: f32 = 112.0;

/// One row of the Devices section (IMP-25, the mixer view spec R25).
pub const DEVICE_ROW: f32 = 14.0;

/// The Performance Impact section's height under the strip (L-MIX-007;
/// its segments' sizes are [`unmeasured`]).
pub const PERFORMANCE_IMPACT_HEIGHT: f32 = 12.0;

/// The Main track's column width (L-SES-006: 1487 to 1591).
pub const MAIN_COLUMN: f32 = bl_shell::mixer_view_commands::MAIN_COLUMN_WIDTH;

/// The track colour strip along each column's bottom, under the strip
/// section (L-MIX-009; observed: about 6.5 points).
pub const COLOUR_STRIP: f32 = 6.5;

/// The row of track title bars along the top of the Arrangement's mixer
/// (observed: each track's colour and name, about 17 points).
pub const ARRANGEMENT_TITLE_ROW: f32 = 17.0;

/// Sizes no observation has measured yet: Beatlayer's until one does (the
/// mixer view spec, "To observe in Live").
pub mod unmeasured {
    /// The status row's buttons' width: a stop button, Stop All Clips, the
    /// follow-action switch and Back to Arrangement are each as wide as
    /// the row is tall.
    pub const STATUS_BUTTON: f32 = super::STATUS_ROW;
    /// The margin inside a column for the In/Out rows and the send knobs.
    pub const PAD: f32 = 4.0;
    /// An In/Out row's pitch: seven rows over the section.
    pub const IN_OUT_ROW: f32 = 16.0;
    /// A crossfader assign button's width and height, and the gap between
    /// the two.
    pub const CROSSFADE_BUTTON: (f32, f32) = (20.0, 14.0);
    pub const CROSSFADE_GAP: f32 = 4.0;
    /// The crossfader's height in its section.
    pub const CROSSFADE_FADER: f32 = 12.0;
}

/// The meter's width (observed: 11 points, as L-ARR-006's meter column).
pub const METER_WIDTH: f32 = 11.0;

/// Which sections show (View ▸ Mixer Controls), and whether the return
/// tracks scroll with the tracks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sections {
    /// In/Out.
    pub in_out: bool,
    /// Sends.
    pub sends: bool,
    /// The strips (Volume).
    pub volume: bool,
    /// The strip section's height, 123 in a new set (the mixer view spec,
    /// R32). The layout keeps it within what the area leaves.
    pub strip: f32,
    /// The Crossfader section: the tracks' A/B assign buttons and Main's
    /// crossfader. Hidden by default (observed).
    pub crossfader: bool,
    /// The return tracks' columns.
    pub returns: bool,
    /// The tracks' Track Options sections: the Track Delay with its unit
    /// switch and Keep Monitoring Latency. Hidden by default (observed).
    pub track_options: bool,
    /// The Performance Impact section: the six segments under each strip
    /// that light up with the track's share of the set's processing time.
    /// Hidden by default (observed).
    pub performance_impact: bool,
    /// The Devices section: each track's top-level devices between In/Out
    /// and Sends (IMP-25, the mixer view spec R24). Hidden by default.
    pub devices: bool,
    /// Whether the return tracks scroll with the tracks (IMP-01, the mixer
    /// view spec R21). Off in a new install, so they stay pinned, as in
    /// Live.
    pub return_tracks_scroll: bool,
}

impl Sections {
    /// Every section a new set shows.
    pub const DEFAULT: Self = Self {
        in_out: true,
        sends: true,
        volume: true,
        strip: STRIP_HEIGHT,
        crossfader: false,
        returns: true,
        track_options: false,
        performance_impact: false,
        devices: false,
        return_tracks_scroll: false,
    };

    /// The same sections with the strip section raised above its default
    /// height only as far as `room` points for the whole mixer allow (R32):
    /// a strip at its default height or lower stays as it is.
    #[must_use]
    pub fn fitted(self, room: f32) -> Self {
        if !self.volume || self.strip <= STRIP_HEIGHT {
            return self;
        }
        let others = self.height() - self.strip;
        Self {
            strip: self.strip.min((room - others).max(STRIP_HEIGHT)),
            ..self
        }
    }

    /// The height of the sections that show, stacked, with the colour strip
    /// under them.
    #[must_use]
    pub fn height(self) -> f32 {
        let mut height = COLOUR_STRIP;
        if self.in_out {
            height += IN_OUT_HEIGHT;
        }
        if self.devices {
            height += DEVICES_HEIGHT;
        }
        if self.sends {
            height += SENDS_HEIGHT;
        }
        if self.volume {
            height += self.strip.max(MIN_STRIP_HEIGHT);
        }
        if self.track_options {
            height += TRACK_OPTIONS_HEIGHT;
        }
        if self.performance_impact {
            height += PERFORMANCE_IMPACT_HEIGHT;
        }
        if self.crossfader {
            height += CROSSFADER_HEIGHT;
        }
        height
    }
}

/// A column's kind, for its strip's controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnKind {
    /// An audio, MIDI, or group track.
    Track,
    /// A rack's chain folded out as a strip after its track's column (racks
    /// spec, R27).
    Chain(ChainColumn),
    /// A return track.
    Return,
    /// The Main track.
    Main,
}

/// A chain's column: the chain and the rack it belongs to (racks spec,
/// R27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainColumn {
    /// The chain.
    pub chain: ChainId,
    /// The rack the chain belongs to.
    pub rack: DeviceId,
}

/// One column of a mixer's track area: a track's, or a chain's strip folded
/// out after it (racks spec, R27).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackColumn {
    /// The chain the column shows, for a chain's column.
    pub chain: Option<ChainColumn>,
    /// The column's width in points, 94 unless it was resized (R36).
    pub width: f32,
}

impl TrackColumn {
    /// A track's column, at the default width.
    #[must_use]
    pub const fn track() -> Self {
        Self {
            chain: None,
            width: COLUMN_WIDTH,
        }
    }

    /// A chain's column, folded out after its track's.
    #[must_use]
    pub const fn chain(chain: ChainColumn) -> Self {
        Self {
            chain: Some(chain),
            width: COLUMN_WIDTH,
        }
    }

    /// The same column, `width` points wide.
    #[must_use]
    pub const fn wide(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

/// One column of a mixer: its kind, its index among its kind, and its
/// rectangle, the mixer's whole height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    /// What it is.
    pub kind: ColumnKind,
    /// Its index among the tracks (in display order), the returns, or 0
    /// for Main.
    pub index: usize,
    /// Where it is.
    pub rect: Rect,
}

/// A mixer's columns and sections in its rectangle (L-SES-001, 004 to 006,
/// L-MIX-001 to 003).
#[derive(Clone, Debug, PartialEq)]
pub struct MixerLayout {
    /// The whole mixer.
    pub area: Rect,
    /// Where the track columns scroll: from the mixer's left to the first
    /// return (or Main).
    pub tracks: Rect,
    /// The track columns in view, left to right, cut to [`Self::tracks`].
    pub columns: Vec<Column>,
    /// The drop area between the last track and the first return, if any
    /// of it is in view (L-SES-004).
    pub drop_area: Option<Rect>,
    /// The return columns (L-SES-005), if they show.
    pub returns: Vec<Column>,
    /// The Main column (L-SES-006).
    pub main: Column,
    /// The In/Out section's rows, top and bottom, if it shows.
    pub in_out: Option<(f32, f32)>,
    /// The Devices section's, if it shows.
    pub devices: Option<(f32, f32)>,
    /// The Sends section's, if it shows.
    pub sends: Option<(f32, f32)>,
    /// The Crossfader section's, if it shows.
    pub crossfader: Option<(f32, f32)>,
    /// The Track Options section's, if it shows.
    pub track_options: Option<(f32, f32)>,
    /// The Performance Impact section's, if it shows.
    pub performance_impact: Option<(f32, f32)>,
    /// The strip's, if it shows.
    pub strip: Option<(f32, f32)>,
    /// The colour strip's, at the bottom (L-MIX-009).
    pub colour: (f32, f32),
    /// How far the track columns are scrolled, within what the set allows.
    pub scroll: f32,
    /// Where each track column starts among the scrolling columns, and
    /// where the last ends: the sum of the widths before it.
    offsets: Vec<f32>,
    /// Whether the return tracks scroll with the tracks (IMP-01, the mixer
    /// view spec R22).
    pub returns_scroll: bool,
}

impl MixerLayout {
    /// Lays out a mixer in `area` with `tracks` track columns and `returns`
    /// return columns, all at the default width, scrolled `scroll` points
    /// (kept within the tracks): [`Self::compute_with_widths`] with nothing
    /// resized.
    #[must_use]
    pub fn compute(
        area: Rect,
        tracks: &[TrackColumn],
        returns: usize,
        sections: Sections,
        scroll: f32,
    ) -> Self {
        Self::compute_with_widths(
            area,
            tracks,
            &vec![COLUMN_WIDTH; returns],
            MAIN_COLUMN,
            sections,
            scroll,
        )
    }

    /// Lays out a mixer in `area` with `tracks` track columns, each as wide
    /// as it says (R36), and a return column of each width in `returns`,
    /// scrolled `scroll` points (kept within the tracks), with Main
    /// `main_width` points wide. A track's column may be followed by the
    /// chain strips folded out of its rack (racks spec, R27), which are
    /// columns of their own. The sections stack up from the area's bottom:
    /// the colour strip, then the strip, Sends, and In/Out above it; any
    /// that don't fit are cut at the area's top, except a strip raised
    /// above its default height, which gives up the height that doesn't fit
    /// (R32).
    ///
    /// With the return tracks scrolling with the tracks (IMP-01, the mixer
    /// view spec R22), the returns come after the drop area and scroll with
    /// the track columns, and only Main stays pinned at the right.
    #[must_use]
    pub fn compute_with_widths(
        area: Rect,
        tracks: &[TrackColumn],
        returns: &[f32],
        main_width: f32,
        sections: Sections,
        scroll: f32,
    ) -> Self {
        let main_rect = Rect::from_min_max(pos2(area.right() - main_width, area.top()), area.max);
        let shown_returns: &[f32] = if sections.returns { returns } else { &[] };
        // Where each track column starts among the scrolling columns, and
        // where the last ends.
        let mut offsets = Vec::with_capacity(tracks.len() + 1);
        let mut tracks_width = 0.0;
        for column in tracks {
            offsets.push(tracks_width);
            tracks_width += column.width;
        }
        offsets.push(tracks_width);
        let returns_width: f32 = shown_returns.iter().sum();
        // With no return tracks showing, there is nothing to scroll with
        // them, and the layout is the one with the returns pinned.
        let scrolling = sections.return_tracks_scroll && !shown_returns.is_empty();
        // The columns scroll in the area from the mixer's left to Main
        // when the returns scroll with them, and to the first return when
        // they stay pinned at the right.
        let track_area = if scrolling {
            Rect::from_min_max(area.min, pos2(main_rect.left(), area.bottom()))
        } else {
            let returns_left = main_rect.left() - returns_width;
            Rect::from_min_max(area.min, pos2(returns_left.max(area.left()), area.bottom()))
        };
        // The drop area keeps its place between the last track and the
        // first return: the space left over when everything fits, and at
        // least one column when the returns scroll (R22).
        let drop_width = if scrolling {
            (track_area.width() - tracks_width - returns_width).max(COLUMN_WIDTH)
        } else {
            (track_area.width() - tracks_width).max(0.0)
        };
        // What the columns scroll over: the tracks alone when the returns
        // stay pinned, and the tracks, the drop area and the returns when
        // they scroll with them.
        let content = if scrolling {
            tracks_width + drop_width + returns_width
        } else {
            tracks_width
        };
        let max_scroll = (content - track_area.width()).max(0.0);
        let scroll = scroll.clamp(0.0, max_scroll);
        // The first column with any of it left of the scroll still in view.
        let first = offsets[1..].partition_point(|right| *right <= scroll);
        // The track each column shows: the track's own column, and the chain
        // strips folded out after it (racks spec, R27).
        let mut seen = 0usize;
        let mut track_of: Vec<usize> = Vec::with_capacity(tracks.len());
        for column in tracks {
            if column.chain.is_none() {
                seen += 1;
            }
            track_of.push(seen.saturating_sub(1));
        }
        let columns: Vec<Column> = (first..tracks.len())
            .map(|index| {
                let left = track_area.left() + offsets[index] - scroll;
                let kind = match tracks[index].chain {
                    Some(chain) => ColumnKind::Chain(chain),
                    None => ColumnKind::Track,
                };
                Column {
                    kind,
                    index: track_of[index],
                    rect: Rect::from_min_max(
                        pos2(left, area.top()),
                        pos2(left + tracks[index].width, area.bottom()),
                    ),
                }
            })
            .take_while(|column| column.rect.left() < track_area.right())
            .collect();
        // The returns: pinned at the right of the tracks, or after the drop
        // area, scrolling with them.
        let returns_left = if scrolling {
            track_area.left() + tracks_width + drop_width - scroll
        } else {
            main_rect.left() - returns_width
        };
        let mut left = returns_left;
        let returns: Vec<Column> = shown_returns
            .iter()
            .enumerate()
            .map(|(index, width)| {
                let rect =
                    Rect::from_min_max(pos2(left, area.top()), pos2(left + width, area.bottom()));
                left += width;
                Column {
                    kind: ColumnKind::Return,
                    index,
                    rect,
                }
            })
            .filter(|column| {
                !scrolling
                    || (column.rect.right() > track_area.left()
                        && column.rect.left() < track_area.right())
            })
            .collect();
        let tracks_end = track_area.left() + tracks_width - scroll;
        let drop_area = if scrolling {
            // The drop area's own width, cut to the area the columns
            // scroll in.
            let rect = Rect::from_min_max(
                pos2(tracks_end, area.top()),
                pos2(tracks_end + drop_width, area.bottom()),
            );
            let visible = rect.intersect(track_area);
            (visible.width() > 0.0).then_some(visible)
        } else {
            (tracks_end < track_area.right()).then(|| {
                Rect::from_min_max(
                    pos2(tracks_end.max(track_area.left()), area.top()),
                    track_area.max,
                )
            })
        };

        let colour = (area.bottom() - COLOUR_STRIP, area.bottom());
        // A strip raised above its default height gives up what the area
        // doesn't have for it (R32).
        let strip_height = {
            let strip = sections.strip.max(MIN_STRIP_HEIGHT);
            if sections.volume && strip > STRIP_HEIGHT {
                let others = sections.height() - strip;
                strip.min((area.height() - others).max(STRIP_HEIGHT))
            } else {
                strip
            }
        };
        let mut bottom = colour.0;
        let mut stack = |shown: bool, height: f32| {
            shown.then(|| {
                let span = (bottom - height, bottom);
                bottom -= height;
                span
            })
        };
        // L-SES-009's order from the bottom: the colour strip, then the
        // Crossfader section under the strips (L-MIX-006), then the Track
        // Options under the strips (L-MIX-005), then the Performance Impact
        // under the strips (L-MIX-007), then the strips, Sends, and In/Out
        // above them.
        let crossfader = stack(sections.crossfader, CROSSFADER_HEIGHT);
        let track_options = stack(sections.track_options, TRACK_OPTIONS_HEIGHT);
        let performance_impact = stack(sections.performance_impact, PERFORMANCE_IMPACT_HEIGHT);
        let strip = stack(sections.volume, strip_height);
        let sends = stack(sections.sends, SENDS_HEIGHT);
        let devices = stack(sections.devices, DEVICES_HEIGHT);
        let in_out = stack(sections.in_out, IN_OUT_HEIGHT);
        Self {
            area,
            tracks: track_area,
            columns,
            drop_area,
            returns,
            main: Column {
                kind: ColumnKind::Main,
                index: 0,
                rect: main_rect,
            },
            in_out,
            devices,
            sends,
            crossfader,
            track_options,
            performance_impact,
            strip,
            colour,
            scroll,
            returns_scroll: scrolling,
            offsets,
        }
    }

    /// Where track `index`'s own column starts and ends among the columns
    /// that scroll, in points from the first column's left, wherever the
    /// mixer is scrolled to.
    #[must_use]
    pub fn track_span(&self, columns: &[TrackColumn], index: usize) -> Option<(f32, f32)> {
        let at = columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.chain.is_none())
            .nth(index)?
            .0;
        Some((self.offsets[at], self.offsets[at + 1]))
    }

    /// The part of a column that shows: its own rectangle, cut to the
    /// area the columns scroll in, except Main's, which is pinned at the
    /// right (the mixer view spec, R22).
    #[must_use]
    pub fn clip_of(&self, kind: ColumnKind, rect: Rect) -> Rect {
        match kind {
            ColumnKind::Main => rect,
            ColumnKind::Track => rect.intersect(self.tracks),
            _ if self.returns_scroll => rect.intersect(self.tracks),
            _ => rect,
        }
    }

    /// Every column in view, left to right: tracks, returns, Main.
    pub fn every_column(&self) -> impl Iterator<Item = &Column> {
        self.columns
            .iter()
            .chain(&self.returns)
            .chain(std::iter::once(&self.main))
    }

    /// The part of a column a section covers.
    #[must_use]
    pub fn section(column: Rect, span: (f32, f32)) -> Rect {
        Rect::from_min_max(pos2(column.left(), span.0), pos2(column.right(), span.1))
    }

    /// Every layout.md row this mixer places: its first track column, the
    /// drop area, the returns, Main, and each section across the mixer.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(first) = self.columns.first() {
            rows.push(super::row("L-SES-001", "Track Column", first.rect));
        }
        if let Some(drop) = self.drop_area {
            rows.push(super::row("L-SES-004", "Drop Area", drop));
        }
        for column in &self.returns {
            rows.push(super::row("L-SES-005", "Return Track Column", column.rect));
        }
        rows.push(super::row("L-SES-006", "Main Track Column", self.main.rect));
        let across = |span: (f32, f32)| {
            Rect::from_min_max(
                pos2(self.area.left(), span.0),
                pos2(self.area.right(), span.1),
            )
        };
        if let Some(span) = self.in_out {
            rows.push(super::row("L-MIX-001", "In/Out", across(span)));
        }
        if let Some(span) = self.devices {
            rows.push(super::row("IMP-25", "Devices section", across(span)));
        }
        if let Some(span) = self.sends {
            rows.push(super::row("L-MIX-002", "Sends", across(span)));
        }
        if let Some(span) = self.crossfader {
            rows.push(super::row(
                "L-MIX-006",
                "Crossfader and Assign Buttons",
                across(span),
            ));
        }
        if let Some(span) = self.track_options {
            rows.push(super::row("L-MIX-005", "Track Options", across(span)));
        }
        if let Some(span) = self.performance_impact {
            rows.push(super::row("L-MIX-007", "Performance Impact", across(span)));
        }
        if let Some(span) = self.strip {
            rows.push(super::row("L-MIX-003", "Mixer Strip", across(span)));
        }
        rows.push(super::row(
            "L-MIX-009",
            "Track Colour Strips",
            across(self.colour),
        ));
        rows
    }
}

/// The Session View's parts in the main view (L-SES-002, 003, 008, 009).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionLayout {
    /// The whole view.
    pub view: Rect,
    /// The title bars' row.
    pub title_bars: Rect,
    /// The clip slots (M3).
    pub slots: Rect,
    /// The track status row (M3).
    pub status_row: Rect,
    /// The mixer, if it shows.
    pub mixer: Option<Rect>,
    /// The relative Session mapping strip while a map mode is on (L-SES-015):
    /// one slot row above the status row, taken from the slots.
    pub strip: Option<Rect>,
}

impl SessionLayout {
    /// Lays the Session View out in the main view `view`: title bars at the
    /// top, the mixer (if it shows) at the bottom as tall as its sections,
    /// the status row above it, and the slots between.
    #[must_use]
    pub fn compute(view: Rect, mixer: Option<Sections>) -> Self {
        let title_bars =
            Rect::from_min_max(view.min, pos2(view.right(), view.top() + TITLE_BAR_HEIGHT));
        let mixer_height = mixer.map_or(0.0, |sections| sections.fitted(Self::room(view)).height());
        let floor = title_bars.bottom() + STATUS_ROW;
        let mixer_top = (view.bottom() - mixer_height).max(floor);
        let mixer = mixer.map(|_| Rect::from_min_max(pos2(view.left(), mixer_top), view.max));
        let status_row = Rect::from_min_max(
            pos2(view.left(), mixer_top - STATUS_ROW),
            pos2(view.right(), mixer_top),
        );
        Self {
            view,
            title_bars,
            slots: Rect::from_min_max(
                pos2(view.left(), title_bars.bottom()),
                pos2(view.right(), status_row.top()),
            ),
            status_row,
            mixer,
            strip: None,
        }
    }

    /// The same layout with the relative Session mapping strip shown (map
    /// modes R14): one slot row tall, above the status row, taking its
    /// height from the slot area.
    #[must_use]
    pub fn with_strip(self) -> Self {
        let height = SLOT_ROW.min(self.slots.height());
        let strip = Rect::from_min_max(
            pos2(self.slots.left(), self.slots.bottom() - height),
            self.slots.max,
        );
        Self {
            slots: Rect::from_min_max(self.slots.min, pos2(self.slots.right(), strip.top())),
            strip: Some(strip),
            ..self
        }
    }

    /// The height a mixer in the main view `view` may take (R32): all but
    /// the title bars, the status row and one clip-slot row.
    #[must_use]
    pub fn room(view: Rect) -> f32 {
        view.height() - TITLE_BAR_HEIGHT - STATUS_ROW - SLOT_ROW
    }

    /// The rectangle of scene `index` (counting from 0) across the slots,
    /// scrolled `scroll_y` points down (L-SES-003): 18 points tall, cut to
    /// the slots area.
    #[must_use]
    pub fn scene_row(&self, index: usize, scroll_y: f32) -> Rect {
        let top = self.slots.top() + SLOT_ROW * index as f32 - scroll_y;
        Rect::from_min_max(
            pos2(self.slots.left(), top),
            pos2(self.slots.right(), top + SLOT_ROW),
        )
    }

    /// The first scene whose row is in view at `scroll_y`, so a frame builds
    /// only the rows on screen.
    #[must_use]
    pub fn first_scene(&self, scroll_y: f32) -> usize {
        (scroll_y / SLOT_ROW).floor().max(0.0) as usize
    }

    /// Every layout.md row it places.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![
            super::row("L-SES-002", "Track Title Bars", self.title_bars),
            super::row("L-SES-003", "Clip Slots", self.slots),
            super::row("L-SES-008", "Track Status Row", self.status_row),
        ];
        if let Some(mixer) = self.mixer {
            rows.push(super::row("L-SES-009", "Session Mixer", mixer));
        }
        if let Some(strip) = self.strip {
            rows.push(super::row("L-SES-015", "Relative Session Mapping Strip", strip));
        }
        rows
    }
}

/// The Arrangement's mixer while it shows (L-ARR-018; observed in 12.4.6):
/// the Session mixer's columns and sections under the lanes, starting at
/// the height the Session mixer's do, with a row of track title bars on
/// top. Returns the mixer's rectangle, title row included, for a main view
/// `view` and the sections that show; the lanes, the pinned returns and
/// Main, and the time ruler take the rest above it.
#[must_use]
pub fn arrangement_mixer(view: Rect, sections: Sections) -> Rect {
    let height = sections.fitted(arrangement_room(view)).height();
    let top = (view.bottom() - height - ARRANGEMENT_TITLE_ROW).max(view.top());
    Rect::from_min_max(pos2(view.left(), top), view.max)
}

/// What the Arrangement View keeps above its mixer: its rulers with the
/// Overview, one track row, a pinned row and the buttons under the lanes
/// (R32; Beatlayer's own).
pub const ARRANGEMENT_ABOVE_MIN: f32 = 118.0;

/// The height a mixer in the Arrangement's main view `view` may take, its
/// title row included (R32).
#[must_use]
pub fn arrangement_room(view: Rect) -> f32 {
    view.height() - ARRANGEMENT_ABOVE_MIN - ARRANGEMENT_TITLE_ROW
}

/// A strip control's rectangle from Live's position relative to the strip
/// section's top-left corner.
fn at(column: Rect, top: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(
        pos2(column.left() + x0, top + y0),
        pos2(column.left() + x1, top + y1),
    )
}

/// The meter scale's labels (dB below 0) and their heights in the strip
/// section (observed).
pub const SCALE_LABELS: [(&str, f32); 6] = [
    ("0", 23.5),
    ("12", 44.5),
    ("24", 64.5),
    ("36", 84.5),
    ("48", 99.0),
    ("60", 111.0),
];

/// The height of the volume field under the fader while the extended meters
/// show (R33): Beatlayer's own.
pub const VOLUME_FIELD_HEIGHT: f32 = 14.0;

/// The top and bottom of the activator, Solo and Arm in a strip section
/// `height` points tall (R32), in points from its top. At the default
/// height they sit where Live has them (y 64 to 85, 91 to 102.5, and 106 to
/// 119), the bottom of the last 4 points up; in a taller strip they keep
/// their distance from the bottom, and in a shorter one they squeeze
/// together between `top_free`, where the controls above them end, and that
/// distance (Beatlayer's own).
fn switch_rows(height: f32, top_free: f32) -> [(f32, f32); 3] {
    // Measured up from the last one's bottom at the default height: 119 down
    // to 64 is 55 points, holding 21, 11.5 and 13.
    let bottom = height - 4.0;
    let squeeze = ((bottom - top_free) / 55.0).clamp(0.0, 1.0);
    [(34.0, 55.0), (16.5, 28.0), (0.0, 13.0)]
        .map(|(near, far)| (bottom - far * squeeze, bottom - near * squeeze))
}

/// The controls inside one column's strip section, where Live 12.4.6 has
/// them (observed on an audio track at the default mixer height and column
/// width, ±1 point): the peak level readout at the top left, the pan knob
/// with its reset triangle under it, the activator, Solo, and Arm in a column
/// at the left, the fader's handle left of the meter, and the meter's scale.
///
/// Taller, the meter, the fader and the scale stretch, and the switches keep
/// their distance from the bottom; shorter than 100 points the pan knob
/// hides and the switches squeeze together; from 200 points the extended
/// meters show a volume field under the fader (R32, R33). A column narrower
/// than 94 points hides the pan knob, the peak readout and the scale's
/// labels and packs the switches, the fader and the meter into its width
/// (R36). All of that is Beatlayer's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripLayout {
    /// The peak level readout (x 8 to 42, y 13 to 24).
    pub peak_field: Rect,
    /// The small triangle above the pan knob that resets it (y about 30).
    pub pan_triangle: Rect,
    /// The pan knob, about 25 points across, centred at (25, 46).
    pub pan: Rect,
    /// The activator (x 15 to 36, y 64 to 85).
    pub activator: Rect,
    /// Solo (y 91 to 102.5).
    pub solo: Rect,
    /// Arm (y 106 to 119).
    pub arm: Rect,
    /// The fader: its handle's column (x 46 to 59) over its travel (the
    /// meter's scale, +6 dB to −∞).
    pub fader: Rect,
    /// The meter (x 60 to 71, y 11 to 119).
    pub meter: Rect,
    /// The gain-reduction bar, a thin bar beside the meter (x 71 to 74, y 11
    /// to 119; IMP-25, the mixer view spec R40).
    pub gain_reduction: Rect,
    /// The scale's labels' column (x 74 to 86).
    pub labels: (f32, f32),
    /// The strip section's top, which [`SCALE_LABELS`] count from.
    pub top: f32,
    /// How far the meter's scale is stretched from the default height's: 1
    /// at 123 points.
    pub stretch: f32,
    /// Whether the pan knob, its triangle and the split controls show.
    pub pan_shown: bool,
    /// Whether the peak level readout shows.
    pub peak_shown: bool,
    /// Whether the scale's labels show.
    pub labels_shown: bool,
    /// Whether the extended meters show (R33): tick marks across the
    /// meters, a volume field under the fader, and a readout that holds its
    /// peak until it is clicked.
    pub extended: bool,
    /// The volume field under the fader, while the extended meters show in
    /// a column at least 94 points wide (R33).
    pub volume_field: Option<Rect>,
    /// Whether each tick mark has its value in dB beside it (R35).
    pub tick_labels: bool,
}

impl StripLayout {
    /// The strip's controls in `column` across the strip section's span.
    #[must_use]
    pub fn compute(column: Rect, span: (f32, f32)) -> Self {
        let top = span.0;
        let height = span.1 - span.0;
        let narrow = column.width() < COLUMN_WIDTH;
        let pan_shown = height >= PAN_HIDES_BELOW && !narrow;
        let peak_shown = !narrow;
        let extended = height >= EXTENDED_METERS_HEIGHT;
        let field_shown = extended && !narrow;
        // The meter runs from y 11 down to 4 points above the strip's
        // bottom, or above the volume field under it.
        let bottom = height
            - 4.0
            - if field_shown {
                VOLUME_FIELD_HEIGHT + 2.0
            } else {
                0.0
            };
        let stretch = (bottom - 11.0) / 108.0;
        let y = |y: f32| 11.0 + (y - 11.0) * stretch;
        let (travel_top, travel_bottom) = bl_shell::mixer_controls::FADER_TRAVEL;
        // Across: Live's places, or packed into a narrow column.
        let (switch_x, fader_x, meter_x, reduction_x) = if narrow {
            let k = ((column.width() - 12.0) / 45.0).min(1.0);
            let x0 = ((column.width() - 45.0 * k - 8.0) / 2.0).max(0.0);
            let switch = (x0, x0 + 21.0 * k);
            let fader = (switch.1 + 2.0, switch.1 + 2.0 + 13.0 * k);
            let meter = (fader.1 + 2.0, fader.1 + 2.0 + 11.0 * k);
            (switch, fader, meter, (meter.1 + 0.5, meter.1 + 2.5))
        } else {
            ((15.0, 36.0), (46.0, 59.0), (60.0, 71.0), (71.5, 73.5))
        };
        let top_free = if pan_shown {
            64.0
        } else if peak_shown {
            27.0
        } else {
            13.0
        };
        let [activator, solo, arm] = switch_rows(height, top_free)
            .map(|(y0, y1)| at(column, top, switch_x.0, y0, switch_x.1, y1));
        Self {
            peak_field: at(column, top, 8.0, 13.0, 42.0, 24.0),
            pan_triangle: at(column, top, 22.0, 27.5, 28.0, 32.5),
            pan: at(column, top, 12.5, 33.5, 37.5, 58.5),
            activator,
            solo,
            arm,
            fader: at(
                column,
                top,
                fader_x.0,
                y(travel_top),
                fader_x.1,
                y(travel_bottom),
            ),
            meter: at(column, top, meter_x.0, 11.0, meter_x.1, bottom),
            gain_reduction: at(column, top, reduction_x.0, 11.0, reduction_x.1, bottom),
            labels: (column.left() + 74.0, column.left() + 86.0),
            top,
            stretch,
            pan_shown,
            peak_shown,
            labels_shown: !narrow,
            extended,
            volume_field: field_shown.then(|| {
                at(
                    column,
                    top,
                    42.0,
                    height - 4.0 - VOLUME_FIELD_HEIGHT,
                    86.0,
                    height - 4.0,
                )
            }),
            tick_labels: extended && column.width() >= TICK_LABELS_WIDTH,
        }
    }

    /// The height in the strip of a mark on the meter's scale that sits at
    /// `y` points from the top of a strip at the default height: the scale
    /// stretches with the strip.
    #[must_use]
    pub fn scale_y(&self, y: f32) -> f32 {
        self.top + 11.0 + (y - 11.0) * self.stretch
    }

    /// Where the tick mark at `db` (0 or below) sits on the meter's scale,
    /// the same as the fader's handle at that level (R33).
    #[must_use]
    pub fn tick_y(&self, db: f64) -> f32 {
        let position =
            bl_shell::mixer_controls::fader_position(bl_shell::mixer_controls::db_gain(db));
        self.fader.bottom() - self.fader.height() * position as f32
    }

    /// Every control that shows, for checks.
    #[must_use]
    pub fn shown_parts(&self) -> Vec<Rect> {
        let mut parts = vec![
            self.activator,
            self.solo,
            self.arm,
            self.fader,
            self.meter,
            self.gain_reduction,
        ];
        if self.peak_shown {
            parts.push(self.peak_field);
        }
        if self.pan_shown {
            parts.push(self.pan_triangle);
            parts.push(self.pan);
        }
        parts.extend(self.volume_field);
        parts
    }

    /// Every control at its default place, for checks.
    #[must_use]
    pub fn parts(&self) -> [Rect; 9] {
        [
            self.peak_field,
            self.pan_triangle,
            self.pan,
            self.activator,
            self.solo,
            self.arm,
            self.fader,
            self.meter,
            self.gain_reduction,
        ]
    }
}

/// The controls in a chain's strip in a mixer (racks spec, R27): the chain's
/// name, its Volume and Pan sliders, its activator and its Solo button, in
/// the strip section, where a track's strip has its controls. The fold
/// button of the rack inside the chain is at the strip's top right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainStripLayout {
    /// The chain's name, at the strip's top left.
    pub name: Rect,
    /// The fold button of the rack inside the chain, at the strip's top
    /// right; empty while the chain holds none.
    pub fold: Rect,
    /// The Volume slider.
    pub volume: Rect,
    /// The Pan slider.
    pub pan: Rect,
    /// Whether the Pan slider shows: not in a strip shorter than 100
    /// points, as a track's pan knob hides (R32).
    pub pan_shown: bool,
    /// The Chain Activator.
    pub activator: Rect,
    /// The Solo button.
    pub solo: Rect,
}

impl ChainStripLayout {
    /// The chain's controls in `column` across the strip section's span.
    #[must_use]
    pub fn compute(column: Rect, span: (f32, f32)) -> Self {
        let top = span.0;
        let height = span.1 - span.0;
        let pan_shown = height >= PAN_HIDES_BELOW;
        let [activator, solo, _] = switch_rows(height, if pan_shown { 56.0 } else { 44.0 })
            .map(|(y0, y1)| at(column, top, 15.0, y0, 36.0, y1));
        Self {
            name: at(column, top, 8.0, 13.0, 42.0, 24.0),
            fold: at(column, top, 78.0, 13.0, 90.0, 25.0),
            volume: at(column, top, 8.0, 33.0, 86.0, 42.0),
            pan: at(column, top, 8.0, 45.0, 86.0, 54.0),
            pan_shown,
            activator,
            solo,
        }
    }
}

/// The Track Options section's three controls in a column across the
/// section's span (L-MIX-005): the Track Delay field, its unit switch
/// beside it, and Keep Monitoring Latency at the right. No observation has
/// measured these yet, so their sizes are [`unmeasured`].
#[must_use]
pub fn track_options(column: Rect, span: (f32, f32)) -> [Rect; 3] {
    use unmeasured::PAD;
    let top = span.0 + 3.0;
    let bottom = span.1 - 3.0;
    let delay = Rect::from_min_max(
        pos2(column.left() + PAD, top),
        pos2(column.left() + PAD + 46.0, bottom),
    );
    let unit = Rect::from_min_max(
        pos2(delay.right() + 2.0, top),
        pos2(delay.right() + 2.0 + 22.0, bottom),
    );
    let keep = Rect::from_min_max(
        pos2(unit.right() + 3.0, top),
        pos2(column.right() - PAD, bottom),
    );
    [delay, unit, keep]
}

/// The Performance Impact section's six segments in a column across the
/// section's span (L-MIX-007): a row of six rectangles, lit from the left
/// with the track's share of the set's processing time. No observation has
/// measured these yet, so their sizes are [`unmeasured`].
#[must_use]
pub fn performance_impact(column: Rect, span: (f32, f32)) -> [Rect; 6] {
    use unmeasured::PAD;
    let top = span.0 + 2.0;
    let bottom = span.1 - 2.0;
    let width = (column.width() - 2.0 * PAD) / 6.0;
    std::array::from_fn(|i| {
        let left = column.left() + PAD + width * i as f32;
        Rect::from_min_max(pos2(left, top), pos2(left + width, bottom))
    })
}

/// Where a Devices section's row `index` sits in a column (IMP-25, the
/// mixer view spec R25 and R26), `scroll` points down: 14 points tall
/// across the column, the first at the section's top.
#[must_use]
pub fn device_row(column: Rect, span: (f32, f32), index: usize, scroll: f32) -> Rect {
    let top = span.0 + DEVICE_ROW * index as f32 - scroll;
    Rect::from_min_max(
        pos2(column.left(), top),
        pos2(column.right(), top + DEVICE_ROW),
    )
}

/// A Devices section row's parts (the mixer view spec R26): the activator
/// square at its left, and the device's name across the rest.
#[must_use]
pub fn device_row_parts(row: Rect) -> (Rect, Rect) {
    use unmeasured::PAD;
    let side = row.height() - 4.0;
    let square = Rect::from_min_size(
        pos2(row.left() + PAD, row.center().y - side / 2.0),
        egui::vec2(side, side),
    );
    let name = Rect::from_min_max(pos2(square.right() + PAD, row.top()), row.max);
    (square, name)
}

/// The two Split Stereo Pan Mode controls beside the pan knob in a strip
/// (the mixer spec, R29): to its left and right, each as tall as it.
#[must_use]
pub fn split_pans(pan: Rect) -> [Rect; 2] {
    const WIDTH: f32 = 6.0;
    const GAP: f32 = 1.0;
    [
        Rect::from_min_max(
            pos2(pan.left() - GAP - WIDTH, pan.top()),
            pos2(pan.left() - GAP, pan.bottom()),
        ),
        Rect::from_min_max(
            pos2(pan.right() + GAP, pan.top()),
            pos2(pan.right() + GAP + WIDTH, pan.bottom()),
        ),
    ]
}

/// A header's pan row in Split Stereo Pan Mode (the mixer spec, R29): the
/// left split control, the pan control between them, and the right split
/// control, across `area`.
#[must_use]
pub fn split_pan_row(area: Rect) -> [Rect; 3] {
    const WIDTH: f32 = 9.0;
    let left = Rect::from_min_max(area.min, pos2(area.left() + WIDTH, area.bottom()));
    let right = Rect::from_min_max(pos2(area.right() - WIDTH, area.top()), area.max);
    let pan = Rect::from_min_max(
        pos2(left.right() + 1.0, area.top()),
        pos2(right.left() - 1.0, area.bottom()),
    );
    [left, pan, right]
}

/// The In/Out section's `index`th row (of seven) in a column (L-MIX-001).
#[must_use]
pub fn in_out_row(column: Rect, span: (f32, f32), index: usize) -> Rect {
    use unmeasured::{IN_OUT_ROW, PAD};
    let top = span.0 + 2.0 + IN_OUT_ROW * index as f32;
    Rect::from_min_max(
        pos2(column.left() + PAD, top),
        pos2(column.right() - PAD, top + IN_OUT_ROW - 2.0),
    )
}

/// The Sends section's knob for return `index` of `returns` in a column,
/// staggered over two rows (L-MIX-002).
#[must_use]
pub fn send_knob(column: Rect, span: (f32, f32), index: usize, returns: usize) -> Rect {
    use unmeasured::PAD;
    let inner = column.width() - 2.0 * PAD;
    let width = inner / returns.max(2) as f32;
    let row_height = (span.1 - span.0) / 2.0;
    let top = span.0 + row_height * (index % 2) as f32;
    let left = column.left() + PAD + width * index as f32;
    Rect::from_min_max(
        pos2(left + 1.0, top + 2.0),
        pos2(left + width - 1.0, top + row_height - 2.0),
    )
}

/// The Crossfader section's A and B assign buttons in a track's or a
/// return's column (L-MIX-006), left to right. Both off, the track plays on
/// neither side.
#[must_use]
pub fn crossfade_buttons(column: Rect, span: (f32, f32)) -> [Rect; 2] {
    use unmeasured::{CROSSFADE_BUTTON, CROSSFADE_GAP, PAD};
    let (width, height) = CROSSFADE_BUTTON;
    let top = span.0 + (span.1 - span.0 - height) / 2.0;
    [0.0, width + CROSSFADE_GAP].map(|offset| {
        Rect::from_min_max(
            pos2(column.left() + PAD + offset, top),
            pos2(column.left() + PAD + offset + width, top + height),
        )
    })
}

/// The Crossfader section's crossfader in Main's column (L-MIX-006): a
/// horizontal slider across the column, 0 (the A end) at its left and 1
/// (the B end) at its right.
#[must_use]
pub fn crossfader_slider(column: Rect, span: (f32, f32)) -> Rect {
    use unmeasured::{CROSSFADE_FADER, PAD};
    let top = span.0 + (span.1 - span.0 - CROSSFADE_FADER) / 2.0;
    Rect::from_min_max(
        pos2(column.left() + PAD, top),
        pos2(column.right() - PAD, top + CROSSFADE_FADER),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{REFERENCE, ShellLayout, TITLE_BAR};
    use egui::vec2;

    /// `count` track columns, with no chain strips folded out.
    fn tracks(count: usize) -> Vec<TrackColumn> {
        vec![TrackColumn::track(); count]
    }

    fn main_view() -> Rect {
        ShellLayout::compute(REFERENCE, true, true)
            .translate(vec2(0.0, TITLE_BAR))
            .main_view
    }

    // Spec: mixer-view R1.
    #[test]
    fn columns_follow_layout_md_and_scroll_under_the_returns() {
        let view = main_view();
        let session = SessionLayout::compute(view, Some(Sections::DEFAULT));
        let mixer = MixerLayout::compute(
            session.mixer.unwrap(),
            &tracks(4),
            2,
            Sections::DEFAULT,
            0.0,
        );
        // L-SES-001: from x 320, 94 points each.
        let lefts: Vec<f32> = mixer.columns.iter().map(|c| c.rect.left()).collect();
        assert_eq!(lefts, [320.0, 414.0, 508.0, 602.0]);
        // L-SES-005, 006: returns at 1298 and 1393 (±2), Main 1487 to 1591.
        assert!((mixer.returns[0].rect.left() - 1298.0).abs() <= 2.0);
        assert!((mixer.returns[1].rect.left() - 1393.0).abs() <= 2.0);
        assert_eq!(
            (mixer.main.rect.left(), mixer.main.rect.right()),
            (1487.0, 1591.0)
        );
        // L-SES-004: the drop area from about 700 to about 1297.
        let drop = mixer.drop_area.unwrap();
        assert!((drop.left() - 700.0).abs() <= 4.0 && (drop.right() - 1297.0).abs() <= 2.0);
        // Many tracks scroll under the returns, which stay put.
        let many = MixerLayout::compute(
            session.mixer.unwrap(),
            &tracks(200),
            2,
            Sections::DEFAULT,
            940.0,
        );
        assert_eq!(many.columns[0].index, 10);
        assert_eq!(many.columns[0].rect.left(), 320.0);
        assert!(many.columns.len() <= 12);
        assert!(many.columns.last().unwrap().rect.left() < many.returns[0].rect.left());
        assert_eq!(many.returns, mixer.returns);
        assert_eq!(many.drop_area, None);
        // The scroll stays within the tracks.
        let far = MixerLayout::compute(
            session.mixer.unwrap(),
            &tracks(200),
            2,
            Sections::DEFAULT,
            1e9,
        );
        assert_eq!(far.columns.last().unwrap().index, 199);
        let few = MixerLayout::compute(
            session.mixer.unwrap(),
            &tracks(4),
            2,
            Sections::DEFAULT,
            500.0,
        );
        assert_eq!(few.scroll, 0.0);
        // Without the returns, the tracks run to Main.
        let no_returns = Sections {
            returns: false,
            ..Sections::DEFAULT
        };
        let alone = MixerLayout::compute(session.mixer.unwrap(), &tracks(4), 2, no_returns, 0.0);
        assert!(alone.returns.is_empty());
        assert_eq!(alone.tracks.right(), 1487.0);
    }

    // Spec: mixer-view R21, R22.
    #[test]
    fn the_return_tracks_scroll_with_the_tracks_when_the_switch_is_on() {
        let view = main_view();
        let session = SessionLayout::compute(view, Some(Sections::DEFAULT));
        let area = session.mixer.unwrap();
        let scrolling = Sections {
            return_tracks_scroll: true,
            ..Sections::DEFAULT
        };
        // With everything fitting, the layout is the one with the returns
        // pinned (R22).
        let pinned = MixerLayout::compute(area, &tracks(4), 2, Sections::DEFAULT, 0.0);
        let same = MixerLayout::compute(area, &tracks(4), 2, scrolling, 0.0);
        assert_eq!(same.columns, pinned.columns);
        assert_eq!(same.returns, pinned.returns);
        assert_eq!(same.drop_area, pinned.drop_area);
        assert_eq!(same.main, pinned.main);
        assert_eq!(same.scroll, 0.0, "everything fits");
        // With 40 returns, the returns come after the drop area and scroll
        // with the tracks, and only Main stays pinned.
        let many = MixerLayout::compute(area, &tracks(4), 40, scrolling, 0.0);
        assert_eq!(
            (many.main.rect.left(), many.main.rect.right()),
            (1487.0, 1591.0)
        );
        let drop = many.drop_area.unwrap();
        assert!(drop.width() >= COLUMN_WIDTH, "{}", drop.width());
        assert_eq!(many.returns[0].rect.left(), drop.right());
        // Only the columns in view are built (R17).
        assert!(
            many.returns.last().unwrap().rect.left() < many.tracks.right(),
            "the returns in view"
        );
        // Scrolling moves the returns with the tracks.
        let scrolled = MixerLayout::compute(area, &tracks(4), 40, scrolling, 188.0);
        assert_eq!(
            scrolled.returns[0].rect.left(),
            many.returns[0].rect.left() - 188.0
        );
        let column_of = |layout: &MixerLayout, index: usize| {
            layout
                .columns
                .iter()
                .find(|c| c.index == index)
                .expect("the column in view")
                .rect
                .left()
        };
        assert_eq!(column_of(&scrolled, 2), column_of(&many, 2) - 188.0);
        // The scroll reaches the last return, which ends where Main starts.
        let end = MixerLayout::compute(area, &tracks(4), 40, scrolling, 1e9);
        assert_eq!(end.returns.last().unwrap().index, 39);
        assert_eq!(end.returns.last().unwrap().rect.right(), 1487.0);
        // With the switch off, the returns stay pinned at the right, as in
        // Live, and the ones that don't fit run off to the left.
        let off = MixerLayout::compute(area, &tracks(4), 40, Sections::DEFAULT, 0.0);
        assert_eq!(off.returns[39].rect.right(), 1487.0);
        assert!(off.returns[0].rect.left() < many.tracks.left());
        // With the returns hidden, there is nothing to scroll with them,
        // and the tracks scroll as they do with the switch off.
        let hidden = Sections {
            returns: false,
            ..scrolling
        };
        let alone = MixerLayout::compute(area, &tracks(200), 40, hidden, 940.0);
        assert_eq!(alone.columns[0].index, 10);
        assert!(alone.columns.len() <= 13);
        assert!(alone.returns.is_empty());
    }

    // Spec: mixer-view R1.
    #[test]
    fn the_session_s_sections_stack_to_the_main_view_s_bottom() {
        let view = main_view();
        let session = SessionLayout::compute(view, Some(Sections::DEFAULT));
        assert_eq!(
            (session.title_bars.top(), session.title_bars.bottom()),
            (61.0, 78.0)
        );
        let mixer = session.mixer.unwrap();
        assert_eq!((mixer.top(), mixer.bottom()), (465.5, 766.0));
        assert_eq!(session.status_row.bottom(), 465.5);
        assert_eq!(session.status_row.height(), 17.0);
        let layout = MixerLayout::compute(mixer, &tracks(4), 2, Sections::DEFAULT, 0.0);
        assert_eq!(layout.in_out, Some((465.5, 580.5)));
        assert_eq!(layout.sends, Some((580.5, 636.5)));
        assert_eq!(layout.strip, Some((636.5, 759.5)));
        assert_eq!(layout.colour, (759.5, 766.0));
        let hidden = SessionLayout::compute(view, None);
        assert_eq!(hidden.mixer, None);
        assert_eq!(hidden.status_row.bottom(), 766.0);
    }

    // Spec: mixer-view R25.
    #[test]
    fn the_devices_section_sits_between_in_out_and_sends_and_takes_the_slots_height() {
        let view = main_view();
        let plain = SessionLayout::compute(view, Some(Sections::DEFAULT));
        let with = Sections {
            devices: true,
            ..Sections::DEFAULT
        };
        let session = SessionLayout::compute(view, Some(with));
        // 112 points, in both mixers, taken from the clip slots and added to
        // the Arrangement's mixer.
        assert_eq!(DEVICES_HEIGHT, 112.0);
        assert_eq!(
            plain.slots.height() - session.slots.height(),
            DEVICES_HEIGHT
        );
        assert_eq!(
            plain.mixer.unwrap().top() - session.mixer.unwrap().top(),
            DEVICES_HEIGHT
        );
        assert_eq!(
            plain.mixer.unwrap().bottom(),
            session.mixer.unwrap().bottom()
        );
        let layout = MixerLayout::compute(session.mixer.unwrap(), &tracks(4), 2, with, 0.0);
        let before =
            MixerLayout::compute(plain.mixer.unwrap(), &tracks(4), 2, Sections::DEFAULT, 0.0);
        // Between In/Out and Sends, with every other section as tall as it
        // was.
        let devices = layout.devices.unwrap();
        assert_eq!(devices.1 - devices.0, 112.0);
        assert_eq!(layout.in_out.unwrap().1, devices.0);
        assert_eq!(devices.1, layout.sends.unwrap().0);
        let height = |span: Option<(f32, f32)>| span.map(|s| s.1 - s.0);
        assert_eq!(height(layout.in_out), height(before.in_out));
        assert_eq!(height(layout.sends), height(before.sends));
        assert_eq!(height(layout.strip), height(before.strip));
        assert_eq!(layout.strip, before.strip);
        assert_eq!(layout.colour, before.colour);
        // The Arrangement's mixer gains the same height.
        assert_eq!(
            arrangement_mixer(view, Sections::DEFAULT).top() - arrangement_mixer(view, with).top(),
            DEVICES_HEIGHT
        );
        // Eight 14-point rows fill it, the first at its top.
        let column = layout.columns[0].rect;
        assert_eq!(device_row(column, devices, 0, 0.0).top(), devices.0);
        assert_eq!(device_row(column, devices, 7, 0.0).bottom(), devices.1);
        assert_eq!(device_row(column, devices, 2, 14.0).top(), devices.0 + 14.0);
    }

    // Spec: mixer-view R2.
    #[test]
    fn the_arrangement_mixer_is_the_session_s_under_a_title_row() {
        let view = main_view();
        let session = SessionLayout::compute(view, Some(Sections::DEFAULT))
            .mixer
            .unwrap();
        let mixer = arrangement_mixer(view, Sections::DEFAULT);
        // Its sections start where the Session mixer's do, under a 17-point
        // row of title bars.
        assert_eq!(mixer.top(), session.top() - ARRANGEMENT_TITLE_ROW);
        assert_eq!(mixer.bottom(), view.bottom());
        let fewer = Sections {
            in_out: false,
            ..Sections::DEFAULT
        };
        assert_eq!(
            arrangement_mixer(view, fewer).top(),
            mixer.top() + IN_OUT_HEIGHT
        );
    }

    /// Whether no two of `rects` overlap.
    fn apart(rects: &[Rect]) -> bool {
        rects
            .iter()
            .enumerate()
            .all(|(i, a)| rects[i + 1..].iter().all(|b| a.intersect(*b).area() < 1e-3))
    }

    // Spec: mixer-view R32, R33.
    #[test]
    fn each_height_stacks_the_sections_with_nothing_overlapping() {
        let view = main_view();
        let room = SessionLayout::room(view);
        let others = Sections::DEFAULT.height() - STRIP_HEIGHT;
        let most = room - others;
        assert!(most > 200.0, "the extended meters fit at 1600 × 1000");
        let mut height = MIN_STRIP_HEIGHT;
        while height <= most {
            let sections = Sections {
                strip: height,
                ..Sections::DEFAULT
            };
            let session = SessionLayout::compute(view, Some(sections));
            let area = session.mixer.unwrap();
            assert_eq!(area.bottom(), view.bottom(), "{height}");
            let layout = MixerLayout::compute(area, &tracks(4), 2, sections, 0.0);
            // The sections run bottom to top without a gap, and the strip is
            // as tall as it was set.
            let strip = layout.strip.unwrap();
            assert_eq!(strip.1 - strip.0, height, "{height}");
            assert_eq!(strip.1, layout.colour.0, "{height}");
            assert_eq!(layout.sends.unwrap().1, strip.0, "{height}");
            assert_eq!(
                layout.in_out.unwrap().1,
                layout.sends.unwrap().0,
                "{height}"
            );
            assert_eq!(layout.in_out.unwrap().0, area.top(), "{height}");
            // The slots give up what the mixer takes, and keep a row.
            assert!(session.slots.height() >= SLOT_ROW, "{height}");
            assert_eq!(session.status_row.bottom(), area.top(), "{height}");
            // Every control that shows is inside the strip section and
            // inside its column, and none overlaps another.
            let column = layout.columns[0].rect;
            let controls = StripLayout::compute(column, strip);
            let parts = controls.shown_parts();
            for part in &parts {
                assert!(
                    column.contains_rect(*part)
                        && part.top() >= strip.0
                        && part.bottom() <= strip.1,
                    "{height}: {part:?}"
                );
            }
            let switches = [controls.activator, controls.solo, controls.arm];
            assert!(apart(&switches), "{height}: {switches:?}");
            if controls.pan_shown {
                assert!(
                    controls.pan.bottom() <= controls.activator.top(),
                    "{height}"
                );
            } else {
                assert!(
                    controls.peak_field.bottom() <= controls.activator.top(),
                    "{height}"
                );
            }
            // Pan hides below 100 points, the extended meters show from 200.
            assert_eq!(controls.pan_shown, height >= PAN_HIDES_BELOW, "{height}");
            assert_eq!(
                controls.extended,
                height >= EXTENDED_METERS_HEIGHT,
                "{height}"
            );
            assert_eq!(
                controls.volume_field.is_some(),
                controls.extended,
                "{height}"
            );
            if let Some(field) = controls.volume_field {
                assert!(field.top() >= controls.meter.bottom(), "{height}");
                assert!(field.top() >= controls.fader.bottom(), "{height}");
            }
            // The meter and the fader keep their ends on the same scale.
            assert!(controls.fader.top() >= controls.meter.top(), "{height}");
            assert!(
                controls.fader.bottom() <= controls.meter.bottom() + 0.5,
                "{height}"
            );
            height += 7.0;
        }
        // The default is Live's, whatever is taller or shorter.
        assert_eq!(Sections::DEFAULT.strip, STRIP_HEIGHT);
    }

    // Spec: mixer-view R32 (racks spec, R27).
    #[test]
    fn a_chain_strip_squeezes_and_hides_its_pan_slider_like_a_track_strip() {
        let column = Rect::from_min_max(pos2(0.0, 0.0), pos2(94.0, 400.0));
        let default = ChainStripLayout::compute(column, (0.0, STRIP_HEIGHT));
        assert!(default.pan_shown);
        assert_eq!(default.activator.top(), 64.0);
        assert_eq!(default.solo.bottom(), 102.5);
        let mut height = MIN_STRIP_HEIGHT;
        while height <= 260.0 {
            let strip = ChainStripLayout::compute(column, (0.0, height));
            assert_eq!(strip.pan_shown, height >= PAN_HIDES_BELOW, "{height}");
            assert!(strip.activator.bottom() <= strip.solo.top(), "{height}");
            let above = if strip.pan_shown {
                strip.pan.bottom()
            } else {
                strip.volume.bottom()
            };
            assert!(above <= strip.activator.top(), "{height}");
            assert!(strip.solo.bottom() <= height, "{height}");
            height += 9.0;
        }
    }

    // Spec: mixer-view R32.
    #[test]
    fn the_strip_gives_up_what_would_take_the_last_clip_slot_row() {
        let view = main_view();
        let huge = Sections {
            strip: 5000.0,
            ..Sections::DEFAULT
        };
        let session = SessionLayout::compute(view, Some(huge));
        assert_eq!(session.slots.height(), SLOT_ROW);
        let area = session.mixer.unwrap();
        let layout = MixerLayout::compute(area, &tracks(4), 2, huge, 0.0);
        let strip = layout.strip.unwrap();
        assert_eq!(layout.in_out.unwrap().0, area.top());
        assert_eq!(
            strip.1 - strip.0,
            huge.fitted(SessionLayout::room(view)).strip
        );
        // A strip below its default is never raised, and one that fits is
        // never cut.
        let lower = Sections {
            strip: MIN_STRIP_HEIGHT,
            ..Sections::DEFAULT
        };
        assert_eq!(lower.fitted(10.0).strip, MIN_STRIP_HEIGHT);
        assert_eq!(Sections::DEFAULT.fitted(10.0).strip, STRIP_HEIGHT);
        // The Arrangement keeps its rulers and a track row above its mixer.
        let mixer = arrangement_mixer(view, huge);
        assert_eq!(mixer.top(), view.top() + ARRANGEMENT_ABOVE_MIN);
        let sections = Sections {
            strip: 300.0,
            ..Sections::DEFAULT
        };
        assert_eq!(
            arrangement_mixer(view, sections).height(),
            sections.height() + ARRANGEMENT_TITLE_ROW
        );
    }

    // Spec: mixer-view R36.
    #[test]
    fn each_width_lays_out_the_strip_as_r36_says() {
        let span = (636.5, 759.5);
        let mut width = 40.0;
        while width <= 400.0 {
            let column = Rect::from_min_max(pos2(320.0, 465.5), pos2(320.0 + width, 766.0));
            let controls = StripLayout::compute(column, span);
            let narrow = width < COLUMN_WIDTH;
            // Narrower than 94: no pan knob, no peak readout, no scale
            // labels; the switches, the fader and the meter stay.
            assert_eq!(controls.pan_shown, !narrow, "{width}");
            assert_eq!(controls.peak_shown, !narrow, "{width}");
            assert_eq!(controls.labels_shown, !narrow, "{width}");
            let kept = [
                controls.activator,
                controls.solo,
                controls.arm,
                controls.fader,
                controls.meter,
            ];
            for part in controls.shown_parts() {
                assert!(column.contains_rect(part), "{width}: {part:?}");
            }
            assert!(apart(&kept), "{width}");
            // Across: the switches, then the fader, then the meter.
            assert!(
                controls.activator.right() <= controls.fader.left(),
                "{width}"
            );
            assert!(controls.fader.right() <= controls.meter.left(), "{width}");
            assert!(
                controls.meter.right() <= controls.gain_reduction.left(),
                "{width}"
            );
            assert!(controls.gain_reduction.right() <= column.right(), "{width}");
            // Live's places at the default width and wider.
            if !narrow {
                assert_eq!(controls.activator.left() - column.left(), 15.0, "{width}");
                assert_eq!(controls.meter.left() - column.left(), 60.0, "{width}");
            }
            // The dB labels from 120 points, with the extended meters only.
            assert!(!controls.tick_labels, "{width}: not extended at 123");
            let tall = StripLayout::compute(column, (636.5, 636.5 + 240.0));
            assert_eq!(tall.tick_labels, width >= TICK_LABELS_WIDTH, "{width}");
            assert_eq!(tall.volume_field.is_some(), !narrow, "{width}");
            width += 6.0;
        }
        // The widest and the narrowest.
        for width in [bl_model::MIN_COLUMN_WIDTH, bl_model::MAX_COLUMN_WIDTH] {
            let column = Rect::from_min_max(pos2(0.0, 0.0), pos2(width, 400.0));
            let controls = StripLayout::compute(column, (200.0, 323.0));
            assert!(column.contains_rect(controls.meter));
        }
    }

    // Spec: mixer-view R36.
    #[test]
    fn the_columns_follow_their_own_widths_and_scroll_by_them() {
        let area = SessionLayout::compute(main_view(), Some(Sections::DEFAULT))
            .mixer
            .unwrap();
        let widths = [94.0, 200.0, 40.0, 400.0, 94.0, 120.0];
        let columns: Vec<TrackColumn> = widths
            .iter()
            .map(|w| TrackColumn::track().wide(*w))
            .collect();
        let layout = MixerLayout::compute_with_widths(
            area,
            &columns,
            &[150.0, 60.0],
            200.0,
            Sections::DEFAULT,
            0.0,
        );
        let lefts: Vec<f32> = layout.columns.iter().map(|c| c.rect.left()).collect();
        assert_eq!(lefts[..4], [320.0, 414.0, 614.0, 654.0]);
        assert_eq!(layout.columns[3].rect.width(), 400.0);
        // Main takes its own width at the right edge, the returns end where
        // it starts.
        assert_eq!(layout.main.rect.width(), 200.0);
        assert_eq!(layout.main.rect.right(), area.right());
        assert_eq!(layout.returns[1].rect.right(), layout.main.rect.left());
        assert_eq!(layout.returns[0].rect.width(), 150.0);
        assert_eq!(layout.returns[1].rect.width(), 60.0);
        assert_eq!(
            layout.returns[0].rect.right(),
            layout.returns[1].rect.left()
        );
        // A column's span among the scrolling columns counts the widths
        // before it.
        assert_eq!(layout.track_span(&columns, 3), Some((334.0, 734.0)));
        assert_eq!(layout.track_span(&columns, 9), None);
        // The first column in view is the first with any of it right of the
        // scroll.
        let tight = Rect::from_min_max(area.min, pos2(area.left() + 900.0, area.bottom()));
        let scrolled =
            MixerLayout::compute_with_widths(tight, &columns, &[], 100.0, Sections::DEFAULT, 100.0);
        assert_eq!(scrolled.columns[0].index, 1);
        assert_eq!(scrolled.columns[0].rect.left(), area.left() - 6.0);
        // The scroll stops where the last column ends at the edge.
        let far =
            MixerLayout::compute_with_widths(tight, &columns, &[], 100.0, Sections::DEFAULT, 1e9);
        assert_eq!(
            far.columns.last().unwrap().rect.right(),
            tight.right() - 100.0
        );
    }

    // Spec: mixer-view R3.
    #[test]
    fn a_strip_holds_its_controls_where_live_has_them() {
        let column = Rect::from_min_max(pos2(320.0, 465.5), pos2(414.0, 766.0));
        let strip = StripLayout::compute(column, (636.5, 759.5));
        for part in strip.parts() {
            assert!(column.contains_rect(part), "{part:?}");
            assert!(part.top() >= 636.5 && part.bottom() <= 759.5, "{part:?}");
        }
        // Relative to the strip section's corner, as observed.
        let rel = |r: Rect| {
            (
                r.left() - 320.0,
                r.top() - 636.5,
                r.right() - 320.0,
                r.bottom() - 636.5,
            )
        };
        assert_eq!(rel(strip.peak_field), (8.0, 13.0, 42.0, 24.0));
        assert_eq!(rel(strip.activator), (15.0, 64.0, 36.0, 85.0));
        assert_eq!(rel(strip.solo), (15.0, 91.0, 36.0, 102.5));
        assert_eq!(rel(strip.arm), (15.0, 106.0, 36.0, 119.0));
        assert_eq!(rel(strip.meter), (60.0, 11.0, 71.0, 119.0));
        assert_eq!(rel(strip.gain_reduction), (71.5, 11.0, 73.5, 119.0));
        assert_eq!(
            (strip.fader.left() - 320.0, strip.fader.right() - 320.0),
            (46.0, 59.0)
        );
        assert_eq!(strip.pan.center() - pos2(320.0, 636.5), vec2(25.0, 46.0));
        // A column of switches, each under the last.
        assert!(
            strip.activator.bottom() <= strip.solo.top() && strip.solo.bottom() <= strip.arm.top()
        );
        for i in 0..7 {
            let row = in_out_row(column, (465.5, 580.5), i);
            assert!(row.bottom() <= 580.5 && column.contains_rect(row));
        }
        let a = send_knob(column, (580.5, 636.5), 0, 2);
        let b = send_knob(column, (580.5, 636.5), 1, 2);
        assert!(a.right() <= b.left() && a.bottom() <= b.top());
    }
}
