//! Snapping a drag to clip edges, locators and time-signature markers as
//! well as the grid (arrangement-editing R21 to R23).
//!
//! The behaviour is in the arrangement editing spec.

use bl_core::time::Ticks;
use bl_model::{ClipId, Document, LocatorId};

use super::ArrangementView;

/// How near, in points, an item must be for a drag to snap to it rather than
/// the grid (arrangement-editing R21).
pub const REACH: f64 = 6.0;

/// What a drag carries, which it never snaps to (arrangement-editing R22).
#[derive(Clone, Copy, Debug, Default)]
pub struct Dragged<'a> {
    /// The clips being moved, copied or resized.
    pub clips: &'a [ClipId],
    /// The locators being moved.
    pub locators: &'a [LocatorId],
    /// The time-signature marker being moved, by its position.
    pub marker: Option<Ticks>,
}

/// Where a drag lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapped {
    /// Where the dragged position lands.
    pub at: Ticks,
    /// The item it snapped to, which the lanes show as a line.
    pub line: Option<Ticks>,
}

impl ArrangementView {
    /// Where a drag with its position at `at` lands, `ticks_per_point` being
    /// the zoom. With snapping on (or off and inverted by Cmd, `invert`), an
    /// item's edge within [`REACH`] points of `at`, or of `at + span` (a
    /// clip's end, `span` its length), wins over the grid line under `at`;
    /// otherwise the grid's holds.
    #[must_use]
    pub fn snap_to_items(
        &self,
        doc: &Document,
        at: Ticks,
        span: Ticks,
        dragged: Dragged<'_>,
        ticks_per_point: f64,
        invert: bool,
    ) -> Snapped {
        if self.grid.snap == invert {
            return Snapped { at, line: None };
        }
        let reach = Ticks((REACH * ticks_per_point).round() as i64);
        let probes = [Ticks::ZERO, span];
        let nearest = items(doc, dragged)
            .flat_map(|item| probes.iter().map(move |by| (item, *by)))
            .map(|(item, by)| ((item - (at + by)).abs(), item, by))
            .filter(|(distance, ..)| *distance <= reach)
            .min_by_key(|(distance, ..)| *distance);
        match nearest {
            Some((_, item, by)) => Snapped {
                at: item - by,
                line: Some(item),
            },
            None => Snapped {
                at: self.snap_drag(doc, at, invert),
                line: None,
            },
        }
    }
}

/// Every edge a drag may snap to: the ends of the clips on every track's
/// Arrangement lane and take lanes, the locators and the time-signature
/// markers, less what the drag carries.
fn items<'a>(doc: &'a Document, dragged: Dragged<'a>) -> impl Iterator<Item = Ticks> + 'a {
    let clips = doc
        .tracks()
        .flat_map(|track| {
            track
                .clips()
                .chain(track.take_lanes().flat_map(|lane| lane.clips.iter()))
        })
        .filter(move |clip| !dragged.clips.contains(&clip.id))
        .flat_map(|clip| [clip.start, clip.end()]);
    let locators = doc
        .locators()
        .iter()
        .filter(move |locator| !dragged.locators.contains(&locator.id))
        .map(|locator| locator.position);
    let markers = doc
        .settings()
        .time_signatures
        .changes()
        .map(|change| change.at)
        .filter(move |at| dragged.marker != Some(*at));
    clips.chain(locators).chain(markers)
}
