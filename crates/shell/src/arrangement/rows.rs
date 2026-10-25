//! The Arrangement's track rows: which tracks show, in what order, how tall
//! (arrangement-view R2, R14).

use bl_model::{Document, TakeLaneId, TrackId, TrackKind};

use super::ArrangementView;

/// A new track's height, unfolded, in points (layout.md, L-ARR-005).
pub const DEFAULT_TRACK_HEIGHT: f32 = 88.0;

/// A folded track's height: its title row only. The return tracks and Main,
/// pinned under the other tracks, are rows this tall (L-ARR-014).
pub const FOLDED_HEIGHT: f32 = 18.0;

/// The shortest an unfolded track can be made by hand.
pub const MIN_TRACK_HEIGHT: f32 = FOLDED_HEIGHT;

/// The tallest a track can be made.
pub const MAX_TRACK_HEIGHT: f32 = 800.0;

/// The shortest a track gets under Optimize Height, so a very long track
/// list still draws.
pub const MIN_OPTIMIZED_HEIGHT: f32 = 0.5;

/// How tall a lane of a track's own is, added under its lane while
/// Automation Mode is on (the automation spec, R25): Beatlayer's own, as
/// Live's hasn't been measured (arrangement-view, "To observe in Live").
pub const AUTOMATION_LANE_HEIGHT: f32 = 43.0;

/// How tall a take lane is when it's first shown, in points (the comping spec,
/// R23): Beatlayer's own.
pub const DEFAULT_TAKE_LANE_HEIGHT: f32 = 44.0;

/// The drop area under the last track ("Drop Files and Devices Here",
/// L-ARR-013): the lanes scroll this far past the last track.
pub const DROP_AREA: f32 = 44.0;

/// One track's row in the scrolling part of the lanes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    /// The track.
    pub track: TrackId,
    /// Its kind.
    pub kind: TrackKind,
    /// How deep in groups it is: 0 for a top-level track.
    pub depth: u16,
    /// Its top, in points from the top of the first row.
    pub top: f32,
    /// The height of its own lane, in points (the clips' area).
    pub height: f32,
    /// The height of its lanes of its own, added under its lane, in points:
    /// 0 while Automation Mode is off, the track is folded, or it has none
    /// showing (WP-6.57).
    pub automation: f32,
    /// The height of its take lanes together, added under its lane, in
    /// points: 0 while they're hidden, the track is folded or Automation
    /// Mode is on (the comping spec, R23, R24). [`ArrangementView::take_lane_rows`]
    /// gives the lanes.
    pub take: f32,
    /// Whether it's folded.
    pub folded: bool,
}

impl Row {
    /// The top and bottom of lane `lane` of the track, in points from the top
    /// of the first row: 0 is the main lane, which draws over the track's
    /// clips, and 1 and up are the lanes of its own under it (the automation
    /// spec, R24, R25).
    #[must_use]
    pub fn lane_span(&self, lane: usize) -> (f32, f32) {
        if lane == 0 {
            return (self.top, self.top + self.height);
        }
        let top = self.top + self.height + (lane - 1) as f32 * AUTOMATION_LANE_HEIGHT;
        (top, top + AUTOMATION_LANE_HEIGHT)
    }

    /// Its bottom, in points from the top of the first row: its lane, its
    /// automation lane and its take lanes.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.top + self.height + self.automation + self.take
    }
}

/// One take lane's row under its track's lane (the comping spec, R23).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TakeLaneRow {
    /// The take lane.
    pub lane: TakeLaneId,
    /// Its top, in points from the top of the first row.
    pub top: f32,
    /// Its height, in points.
    pub height: f32,
}

/// The tracks in the scrolling part of the lanes, in display order: every
/// track but the returns and Main, skipping those inside a folded group and,
/// while the search field's Show Only Matches is on, those it hides
/// (command-palette R12).
pub fn shown_tracks<'a>(
    doc: &'a Document,
    view: &'a ArrangementView,
) -> impl Iterator<Item = (TrackId, TrackKind, u16)> + 'a {
    let mut stack: Vec<(TrackId, u16)> = doc
        .top_level()
        .rev()
        .filter(|id| view.search_keeps(*id))
        .map(|id| (id, 0))
        .collect();
    std::iter::from_fn(move || {
        let (id, depth) = stack.pop()?;
        let track = doc.track(id)?;
        if track.kind() == TrackKind::Group && !view.is_folded_in(doc, id) {
            stack.extend(
                track
                    .children()
                    .rev()
                    .filter(|child| view.search_keeps(*child))
                    .map(|child| (child, depth + 1)),
            );
        }
        Some((id, track.kind(), depth))
    })
}

/// The pinned rows under the scrolling ones: the return tracks, then Main,
/// with the return tracks left out while they're hidden (arrangement-view
/// R23) or Show Only Matches hides them (command-palette R12).
pub fn pinned_tracks<'a>(
    doc: &'a Document,
    view: &'a ArrangementView,
) -> impl Iterator<Item = TrackId> + 'a {
    let returns = doc
        .returns()
        .filter(move |id| view.show_return_tracks() && view.search_keeps(*id));
    returns.chain(std::iter::once(doc.main_id()))
}

/// The index of the first row whose bottom is below `y`, by binary search:
/// the first row a view scrolled to `y` shows.
#[must_use]
pub fn first_row_at(rows: &[Row], y: f32) -> usize {
    rows.partition_point(|row| row.bottom() <= y)
}

/// The rows `top..bottom` (in row coordinates) overlaps.
#[must_use]
pub fn rows_between(rows: &[Row], top: f32, bottom: f32) -> std::ops::Range<usize> {
    let first = first_row_at(rows, top);
    let end = rows.partition_point(|row| row.top < bottom);
    first..end.max(first)
}
