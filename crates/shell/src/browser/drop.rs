//! Where a drag from the browser lands in the Arrangement (spec: browser
//! R13): an audio track's lane, at the snapped position under the pointer,
//! or the drop area under the last track, which makes a new track.

use bl_core::time::Ticks;
use bl_model::{Document, TrackId, TrackKind};

use crate::arrangement::ArrangementView;

/// Where dropped files land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropTarget {
    /// The audio track they go on, or `None` for the drop area under the
    /// last track, which makes a new audio track.
    pub track: Option<TrackId>,
    /// Where the first clip starts.
    pub at: Ticks,
    /// The top of the highlighted row (or the drop area), in points from the
    /// lanes' top edge as they're scrolled.
    pub top: f32,
    /// Its height in points.
    pub height: f32,
}

/// Where files dropped at `(x, y)`, in points from the lanes' top-left
/// corner, land, with the lanes `height` points tall; `None` where they
/// can't (a MIDI, group, or return track, or outside the lanes). The
/// position snaps to the editing grid while snapping is on, unless
/// `ignore_snap` (Cmd on macOS, Alt on Windows, held during the drag).
#[must_use]
pub fn drop_target(
    view: &ArrangementView,
    doc: &Document,
    height: f32,
    x: f32,
    y: f32,
    ignore_snap: bool,
) -> Option<DropTarget> {
    if x < 0.0 || y < 0.0 || y > height {
        return None;
    }
    let raw = view.horizontal(doc).at(f64::from(x)).max(Ticks::ZERO);
    let at = if ignore_snap {
        raw
    } else {
        view.snap(doc, raw)
    };
    let rows = view.rows(doc);
    let scroll = view.scroll_y() as f32;
    let content_y = y + scroll;
    let last = rows.last().map_or(0.0, |row| row.bottom());
    if content_y >= last {
        // The drop area (L-ARR-013).
        let top = last - scroll;
        return Some(DropTarget {
            track: None,
            at,
            top,
            height: height - top,
        });
    }
    let row = rows
        .iter()
        .find(|row| row.top <= content_y && content_y < row.bottom())?;
    (row.kind == TrackKind::Audio).then_some(DropTarget {
        track: Some(row.track),
        at,
        top: row.top - scroll,
        height: row.height,
    })
}
