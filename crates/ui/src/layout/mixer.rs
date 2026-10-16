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

use egui::{Rect, pos2};

pub use bl_shell::mixer_view_commands::COLUMN_WIDTH;

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

/// The In/Out section's height (L-MIX-001).
pub const IN_OUT_HEIGHT: f32 = 115.0;

/// The Sends section's height (L-MIX-002).
pub const SENDS_HEIGHT: f32 = 56.0;

/// The mixer strip's height (L-MIX-003).
pub const STRIP_HEIGHT: f32 = 123.0;

/// The Crossfader section's height under the strip (L-MIX-006; its
/// controls' sizes are [`unmeasured`]).
pub const CROSSFADER_HEIGHT: f32 = 28.0;

/// The Main track's column width (L-SES-006: 1487 to 1591).
pub const MAIN_COLUMN: f32 = 104.0;

/// The track colour strip along each column's bottom, under the strip
/// section (L-MIX-009; observed: about 6.5 points).
pub const COLOUR_STRIP: f32 = 6.5;

/// The row of track title bars along the top of the Arrangement's mixer
/// (observed: each track's colour and name, about 17 points).
pub const ARRANGEMENT_TITLE_ROW: f32 = 17.0;

/// Sizes no observation has measured yet: Beatlayer's until one does (the
/// mixer view spec, "To observe in Live").
pub mod unmeasured {
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

/// Which sections show (View ▸ Mixer Controls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sections {
    /// In/Out.
    pub in_out: bool,
    /// Sends.
    pub sends: bool,
    /// The strips (Volume).
    pub volume: bool,
    /// The Crossfader section: the tracks' A/B assign buttons and Main's
    /// crossfader. Hidden by default (observed).
    pub crossfader: bool,
    /// The return tracks' columns.
    pub returns: bool,
}

impl Sections {
    /// Every section a new set shows.
    pub const DEFAULT: Self = Self {
        in_out: true,
        sends: true,
        volume: true,
        crossfader: false,
        returns: true,
    };

    /// The height of the sections that show, stacked, with the colour strip
    /// under them.
    #[must_use]
    pub fn height(self) -> f32 {
        let mut height = COLOUR_STRIP;
        if self.in_out {
            height += IN_OUT_HEIGHT;
        }
        if self.sends {
            height += SENDS_HEIGHT;
        }
        if self.volume {
            height += STRIP_HEIGHT;
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
    /// A return track.
    Return,
    /// The Main track.
    Main,
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
    /// The Sends section's, if it shows.
    pub sends: Option<(f32, f32)>,
    /// The Crossfader section's, if it shows.
    pub crossfader: Option<(f32, f32)>,
    /// The strip's, if it shows.
    pub strip: Option<(f32, f32)>,
    /// The colour strip's, at the bottom (L-MIX-009).
    pub colour: (f32, f32),
    /// How far the track columns are scrolled, within what the set allows.
    pub scroll: f32,
}

impl MixerLayout {
    /// Lays out a mixer in `area` with `tracks` track columns and `returns`
    /// return columns, scrolled `scroll` points (kept within the tracks).
    /// The sections stack up from the area's bottom: the colour strip, then
    /// the strip, Sends, and In/Out above it; any that don't fit are cut at
    /// the area's top.
    #[must_use]
    pub fn compute(
        area: Rect,
        tracks: usize,
        returns: usize,
        sections: Sections,
        scroll: f32,
    ) -> Self {
        let main_rect = Rect::from_min_max(pos2(area.right() - MAIN_COLUMN, area.top()), area.max);
        let shown_returns = if sections.returns { returns } else { 0 };
        let returns_left = main_rect.left() - COLUMN_WIDTH * shown_returns as f32;
        let returns: Vec<Column> = (0..shown_returns)
            .map(|index| Column {
                kind: ColumnKind::Return,
                index,
                rect: Rect::from_min_max(
                    pos2(returns_left + COLUMN_WIDTH * index as f32, area.top()),
                    pos2(
                        returns_left + COLUMN_WIDTH * (index + 1) as f32,
                        area.bottom(),
                    ),
                ),
            })
            .collect();
        let track_area =
            Rect::from_min_max(area.min, pos2(returns_left.max(area.left()), area.bottom()));
        let content = COLUMN_WIDTH * tracks as f32;
        let max_scroll = (content - track_area.width()).max(0.0);
        let scroll = scroll.clamp(0.0, max_scroll);
        let first = (scroll / COLUMN_WIDTH).floor() as usize;
        let columns: Vec<Column> = (first..tracks)
            .map(|index| {
                let left = track_area.left() + COLUMN_WIDTH * index as f32 - scroll;
                Column {
                    kind: ColumnKind::Track,
                    index,
                    rect: Rect::from_min_max(
                        pos2(left, area.top()),
                        pos2(left + COLUMN_WIDTH, area.bottom()),
                    ),
                }
            })
            .take_while(|column| column.rect.left() < track_area.right())
            .collect();
        let tracks_end = track_area.left() + content - scroll;
        let drop_area = (tracks_end < track_area.right()).then(|| {
            Rect::from_min_max(
                pos2(tracks_end.max(track_area.left()), area.top()),
                track_area.max,
            )
        });

        let colour = (area.bottom() - COLOUR_STRIP, area.bottom());
        let mut bottom = colour.0;
        let mut stack = |shown: bool, height: f32| {
            shown.then(|| {
                let span = (bottom - height, bottom);
                bottom -= height;
                span
            })
        };
        // L-SES-009's order from the bottom: the colour strip, then the
        // Crossfader section under the strips (L-MIX-006), then the strips,
        // Sends, and In/Out above them.
        let crossfader = stack(sections.crossfader, CROSSFADER_HEIGHT);
        let strip = stack(sections.volume, STRIP_HEIGHT);
        let sends = stack(sections.sends, SENDS_HEIGHT);
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
            sends,
            crossfader,
            strip,
            colour,
            scroll,
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
}

impl SessionLayout {
    /// Lays the Session View out in the main view `view`: title bars at the
    /// top, the mixer (if it shows) at the bottom as tall as its sections,
    /// the status row above it, and the slots between.
    #[must_use]
    pub fn compute(view: Rect, mixer: Option<Sections>) -> Self {
        let title_bars =
            Rect::from_min_max(view.min, pos2(view.right(), view.top() + TITLE_BAR_HEIGHT));
        let mixer_height = mixer.map_or(0.0, Sections::height);
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
        }
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
    let top = (view.bottom() - sections.height() - ARRANGEMENT_TITLE_ROW).max(view.top());
    Rect::from_min_max(pos2(view.left(), top), view.max)
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

/// The controls inside one column's strip section, where Live 12.4.6 has
/// them (observed on an audio track at the default mixer height, ±1
/// point): the peak level readout at the top left, the pan knob with its
/// reset triangle under it, the activator, Solo, and Arm in a column at the
/// left, the fader's handle left of the meter, and the meter's scale.
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
    /// The scale's labels' column (x 74 to 86).
    pub labels: (f32, f32),
    /// The strip section's top, which [`SCALE_LABELS`] count from.
    pub top: f32,
}

impl StripLayout {
    /// The strip's controls in `column` across the strip section's span.
    #[must_use]
    pub fn compute(column: Rect, span: (f32, f32)) -> Self {
        let top = span.0;
        let (travel_top, travel_bottom) = bl_shell::mixer_controls::FADER_TRAVEL;
        Self {
            peak_field: at(column, top, 8.0, 13.0, 42.0, 24.0),
            pan_triangle: at(column, top, 22.0, 27.5, 28.0, 32.5),
            pan: at(column, top, 12.5, 33.5, 37.5, 58.5),
            activator: at(column, top, 15.0, 64.0, 36.0, 85.0),
            solo: at(column, top, 15.0, 91.0, 36.0, 102.5),
            arm: at(column, top, 15.0, 106.0, 36.0, 119.0),
            fader: at(column, top, 46.0, travel_top, 59.0, travel_bottom),
            meter: at(column, top, 60.0, 11.0, 71.0, 119.0),
            labels: (column.left() + 74.0, column.left() + 86.0),
            top,
        }
    }

    /// Every control, for checks.
    #[must_use]
    pub fn parts(&self) -> [Rect; 8] {
        [
            self.peak_field,
            self.pan_triangle,
            self.pan,
            self.activator,
            self.solo,
            self.arm,
            self.fader,
            self.meter,
        ]
    }
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
        let mixer = MixerLayout::compute(session.mixer.unwrap(), 4, 2, Sections::DEFAULT, 0.0);
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
        let many = MixerLayout::compute(session.mixer.unwrap(), 200, 2, Sections::DEFAULT, 940.0);
        assert_eq!(many.columns[0].index, 10);
        assert_eq!(many.columns[0].rect.left(), 320.0);
        assert!(many.columns.len() <= 12);
        assert!(many.columns.last().unwrap().rect.left() < many.returns[0].rect.left());
        assert_eq!(many.returns, mixer.returns);
        assert_eq!(many.drop_area, None);
        // The scroll stays within the tracks.
        let far = MixerLayout::compute(session.mixer.unwrap(), 200, 2, Sections::DEFAULT, 1e9);
        assert_eq!(far.columns.last().unwrap().index, 199);
        let few = MixerLayout::compute(session.mixer.unwrap(), 4, 2, Sections::DEFAULT, 500.0);
        assert_eq!(few.scroll, 0.0);
        // Without the returns, the tracks run to Main.
        let no_returns = Sections {
            returns: false,
            ..Sections::DEFAULT
        };
        let alone = MixerLayout::compute(session.mixer.unwrap(), 4, 2, no_returns, 0.0);
        assert!(alone.returns.is_empty());
        assert_eq!(alone.tracks.right(), 1487.0);
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
        let layout = MixerLayout::compute(mixer, 4, 2, Sections::DEFAULT, 0.0);
        assert_eq!(layout.in_out, Some((465.5, 580.5)));
        assert_eq!(layout.sends, Some((580.5, 636.5)));
        assert_eq!(layout.strip, Some((636.5, 759.5)));
        assert_eq!(layout.colour, (759.5, 766.0));
        let hidden = SessionLayout::compute(view, None);
        assert_eq!(hidden.mixer, None);
        assert_eq!(hidden.status_row.bottom(), 766.0);
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
