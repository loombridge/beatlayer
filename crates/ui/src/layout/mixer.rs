//! The layout layer's mixer (WP-1.9): the Session View's columns and its
//! mixer, the Arrangement's mixer, and the controls inside a strip, from
//! layout.md's L-SES, L-MIX, and L-ARR-018 rows measured in Live 12.4.3 at
//! 1600 × 1000 points (the mixer view spec, R1 to R3, R5).
//!
//! What layout.md measured is placed exactly: the columns' widths and
//! where they start, the returns and Main at the right, the title bars,
//! the status row, the sections' heights and order, and the Arrangement's
//! split. Inside a strip, layout.md gives only the order (the volume field
//! and pan at the top left, the fader with its meter at the right, then the
//! activator, Solo, and Arm), so the sizes there are Beatlayer's until Live
//! is observed: every such value is in [`unmeasured`], and the mixer view
//! spec lists them under "To observe in Live", item 5.

use egui::{Rect, pos2};

pub use bl_shell::mixer_view_commands::COLUMN_WIDTH;

use super::Row;

/// A Session title bar's height (L-SES-002: y 61 to 78).
pub const TITLE_BAR_HEIGHT: f32 = 17.0;

/// The Session's track status row's height, under the slots and above the
/// mixer (L-SES-008).
pub const STATUS_ROW: f32 = 17.0;

/// The In/Out section's height (L-MIX-001).
pub const IN_OUT_HEIGHT: f32 = 115.0;

/// The Sends section's height (L-MIX-002).
pub const SENDS_HEIGHT: f32 = 56.0;

/// The mixer strip's height (L-MIX-003).
pub const STRIP_HEIGHT: f32 = 123.0;

/// The Main track's column width (L-SES-006: 1487 to 1591).
pub const MAIN_COLUMN: f32 = 104.0;

/// The Arrangement's split at 1600 × 1000 (L-ARR-018): the part above the
/// bottom strip runs from 61 to 729 (668 points), the lanes keep 61 to 397
/// (336) and the mixer takes 407 to 729 (322).
const SPLIT_SPAN: f32 = 668.0;
const SPLIT_LANES: f32 = 336.0;
const SPLIT_MIXER: f32 = 322.0;

/// The Arrangement's bottom strip under the lanes or the mixer (the grid
/// indicator's row, the time ruler, the waveform and Optimize buttons):
/// from 729 to the main view's bottom, 766 (L-ARR-015 to 017, L-ARR-018).
pub const ARRANGEMENT_BOTTOM_STRIP: f32 = 37.0;

/// Sizes inside a strip and its sections that layout.md doesn't give:
/// Beatlayer's until they're observed in Live (the mixer view spec, "To
/// observe in Live", item 5). The meter's width is the Arrangement header's
/// meter column's, which was measured (L-ARR-006: 1580 to 1591).
pub mod unmeasured {
    /// The margin inside a column.
    pub const PAD: f32 = 4.0;
    /// The width of the strip's left part, holding the volume field and pan.
    pub const LEFT_PART: f32 = 40.0;
    /// The volume field's height.
    pub const FIELD: f32 = 14.0;
    /// The pan reset triangle's height, above the knob.
    pub const PAN_TRIANGLE: f32 = 6.0;
    /// The pan knob's height.
    pub const PAN_KNOB: f32 = 24.0;
    /// The row of the activator, Solo, and Arm.
    pub const BUTTONS: f32 = 16.0;
    /// The gap between the fader and the meter.
    pub const FADER_GAP: f32 = 3.0;
    /// An In/Out row's pitch: seven rows over the section.
    pub const IN_OUT_ROW: f32 = 16.0;
}

/// The meter's width (L-ARR-006's meter column, 11 points).
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
    /// The return tracks' columns.
    pub returns: bool,
}

impl Sections {
    /// Every section a new set shows.
    pub const DEFAULT: Self = Self {
        in_out: true,
        sends: true,
        volume: true,
        returns: true,
    };

    /// The height of the sections that show, stacked.
    #[must_use]
    pub fn height(self) -> f32 {
        let mut height = 0.0;
        if self.in_out {
            height += IN_OUT_HEIGHT;
        }
        if self.sends {
            height += SENDS_HEIGHT;
        }
        if self.volume {
            height += STRIP_HEIGHT;
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
    /// The strip's, if it shows.
    pub strip: Option<(f32, f32)>,
    /// How far the track columns are scrolled, within what the set allows.
    pub scroll: f32,
}

impl MixerLayout {
    /// Lays out a mixer in `area` with `tracks` track columns and `returns`
    /// return columns, scrolled `scroll` points (kept within the tracks).
    /// The sections stack from the area's bottom in the order In/Out,
    /// Sends, the strip; any that don't fit are cut at the area's top.
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

        let mut bottom = area.bottom();
        let mut stack = |shown: bool, height: f32| {
            shown.then(|| {
                let span = (bottom - height, bottom);
                bottom -= height;
                span
            })
        };
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
            strip,
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
        if let Some(span) = self.strip {
            rows.push(super::row("L-MIX-003", "Mixer Strip", across(span)));
        }
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

/// The Arrangement's split while its mixer shows (L-ARR-018): the bottom
/// of the part the lanes keep, and the mixer's rectangle, for a main view
/// `view`. The part above the bottom strip splits in Live's shares.
#[must_use]
pub fn arrangement_split(view: Rect) -> (f32, Rect) {
    let span_bottom = view.bottom() - ARRANGEMENT_BOTTOM_STRIP;
    let span = (span_bottom - view.top()).max(0.0);
    let lanes_bottom = view.top() + span * SPLIT_LANES / SPLIT_SPAN;
    let mixer_top = span_bottom - span * SPLIT_MIXER / SPLIT_SPAN;
    (
        lanes_bottom,
        Rect::from_min_max(
            pos2(view.left(), mixer_top),
            pos2(view.right(), span_bottom),
        ),
    )
}

/// The controls inside one column's strip and sections. Their order is
/// layout.md's (L-MIX-003); their sizes are [`unmeasured`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripLayout {
    /// The volume field, top left.
    pub volume_field: Rect,
    /// The small triangle above the pan knob that resets it.
    pub pan_triangle: Rect,
    /// The pan knob, under the volume field.
    pub pan: Rect,
    /// The fader, at the right.
    pub fader: Rect,
    /// The meter beside it, at the column's right edge.
    pub meter: Rect,
    /// The row under them: the activator, Solo, Arm (or, on Main, the cue
    /// volume and the Solo/Cue button in the first two).
    pub buttons: [Rect; 3],
}

impl StripLayout {
    /// The strip's controls in `column` across the strip's span.
    #[must_use]
    pub fn compute(column: Rect, span: (f32, f32)) -> Self {
        use unmeasured::{BUTTONS, FADER_GAP, FIELD, LEFT_PART, PAD, PAN_KNOB, PAN_TRIANGLE};
        let (top, bottom) = (span.0 + PAD, span.1 - PAD);
        let left = column.left() + PAD;
        let right = column.right() - PAD;
        let buttons_top = bottom - BUTTONS;
        let volume_field = Rect::from_min_max(pos2(left, top), pos2(left + LEFT_PART, top + FIELD));
        let pan_triangle = Rect::from_min_max(
            pos2(left, volume_field.bottom() + PAD),
            pos2(left + LEFT_PART, volume_field.bottom() + PAD + PAN_TRIANGLE),
        );
        let pan = Rect::from_min_max(
            pos2(left, pan_triangle.bottom()),
            pos2(left + LEFT_PART, pan_triangle.bottom() + PAN_KNOB),
        );
        let meter = Rect::from_min_max(
            pos2(right - METER_WIDTH, top),
            pos2(right, buttons_top - PAD),
        );
        let fader = Rect::from_min_max(
            pos2(left + LEFT_PART + PAD, top),
            pos2(meter.left() - FADER_GAP, buttons_top - PAD),
        );
        let third = (right - left) / 3.0;
        let button = |i: f32| {
            Rect::from_min_max(
                pos2(left + third * i + 1.0, buttons_top),
                pos2(left + third * (i + 1.0) - 1.0, bottom),
            )
        };
        Self {
            volume_field,
            pan_triangle,
            pan,
            fader,
            meter,
            buttons: [button(0.0), button(1.0), button(2.0)],
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{REFERENCE, ShellLayout, TITLE_BAR};
    use egui::vec2;

    fn main_view() -> Rect {
        ShellLayout::compute(REFERENCE, true)
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
        assert_eq!((mixer.top(), mixer.bottom()), (472.0, 766.0));
        assert_eq!(session.status_row.bottom(), 472.0);
        assert_eq!(session.status_row.height(), 17.0);
        let layout = MixerLayout::compute(mixer, 4, 2, Sections::DEFAULT, 0.0);
        assert_eq!(layout.in_out, Some((472.0, 587.0)));
        assert_eq!(layout.sends, Some((587.0, 643.0)));
        assert_eq!(layout.strip, Some((643.0, 766.0)));
        let hidden = SessionLayout::compute(view, None);
        assert_eq!(hidden.mixer, None);
        assert_eq!(hidden.status_row.bottom(), 766.0);
    }

    // Spec: mixer-view R2.
    #[test]
    fn the_arrangement_split_keeps_its_shares_at_every_height() {
        let (lanes_bottom, mixer) = arrangement_split(main_view());
        assert_eq!(lanes_bottom, 397.0);
        assert_eq!((mixer.top(), mixer.bottom()), (407.0, 729.0));
        for extra in [-200.0, 0.0, 300.0] {
            let view = main_view().with_max_y(main_view().bottom() + extra);
            let (lanes_bottom, mixer) = arrangement_split(view);
            assert!(lanes_bottom < mixer.top());
            assert_eq!(mixer.bottom(), view.bottom() - ARRANGEMENT_BOTTOM_STRIP);
            let span = mixer.bottom() - view.top();
            assert!(((lanes_bottom - view.top()) / span - 336.0 / 668.0).abs() < 1e-4);
        }
    }

    // Spec: mixer-view R3, R5.
    #[test]
    fn a_strip_holds_its_controls_inside_its_column() {
        let column = Rect::from_min_max(pos2(320.0, 472.0), pos2(414.0, 766.0));
        let strip = StripLayout::compute(column, (643.0, 766.0));
        let parts = [
            strip.volume_field,
            strip.pan_triangle,
            strip.pan,
            strip.fader,
            strip.meter,
            strip.buttons[0],
            strip.buttons[1],
            strip.buttons[2],
        ];
        for part in parts {
            assert!(column.contains_rect(part), "{part:?}");
            assert!(part.top() >= 643.0 && part.bottom() <= 766.0, "{part:?}");
            assert!(part.width() > 0.0 && part.height() > 0.0, "{part:?}");
        }
        // The volume field and pan at the top left, pan under the field; the
        // fader at the right with the meter at the column's right edge; the
        // buttons in a row under everything, left to right.
        assert!(strip.pan.top() >= strip.volume_field.bottom());
        assert!(strip.fader.left() >= strip.volume_field.right());
        assert!(strip.meter.left() >= strip.fader.right());
        assert_eq!(strip.meter.width(), 11.0);
        assert!(strip.buttons[0].top() >= strip.fader.bottom());
        assert!(strip.buttons[0].right() <= strip.buttons[1].left());
        assert!(strip.buttons[1].right() <= strip.buttons[2].left());
        for i in 0..7 {
            let row = in_out_row(column, (472.0, 587.0), i);
            assert!(row.bottom() <= 587.0 && column.contains_rect(row));
        }
        let a = send_knob(column, (587.0, 643.0), 0, 2);
        let b = send_knob(column, (587.0, 643.0), 1, 2);
        assert!(a.right() <= b.left() && a.bottom() <= b.top());
    }
}
