//! Where a drag from the browser lands in the Arrangement (spec: browser
//! R13): an audio track's lane, at the snapped position under the pointer,
//! or the drop area under the last track, which makes a new track.

use bl_core::time::Ticks;
use bl_model::{ClipId, Document, TakeLaneId, TrackId, TrackKind};

use crate::arrangement::ArrangementView;

/// Where dropped files land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropTarget {
    /// The audio track they go on (a MIDI track for an SFZ instrument), or
    /// `None` for the drop area under the last track, which makes a new
    /// track.
    pub track: Option<TrackId>,
    /// Where the first clip starts.
    pub at: Ticks,
    /// The top of the highlighted row (or the drop area), in points from the
    /// lanes' top edge as they're scrolled.
    pub top: f32,
    /// Its height in points.
    pub height: f32,
    /// The take lane of the track the pointer is over, whose row is the one
    /// highlighted, or `None` over the track's own lane (the comping spec,
    /// R36).
    pub lane: Option<TakeLaneId>,
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
    drop_target_on(TrackKind::Audio, view, doc, height, x, y, ignore_snap)
}

/// As [`drop_target`], for files that go on tracks of kind `kind`: audio
/// files on audio tracks, an SFZ instrument (multisampler R47) on a MIDI
/// track.
#[must_use]
pub fn drop_target_on(
    kind: TrackKind,
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
            lane: None,
        });
    }
    let row = rows
        .iter()
        .find(|row| row.top <= content_y && content_y < row.bottom())?;
    // Spec: comping R36. Over a take lane, that lane's row.
    let lane = view
        .take_lane_rows(doc, row)
        .find(|lane| content_y >= lane.top && content_y < lane.top + lane.height);
    (row.kind == kind).then_some(DropTarget {
        track: Some(row.track),
        at,
        top: lane.map_or(row.top, |lane| lane.top) - scroll,
        height: lane.map_or(row.height, |lane| lane.height),
        lane: lane.map(|lane| lane.lane),
    })
}

/// What a groove dropped on the Arrangement does (the grooves spec R28,
/// I-DROP-017).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrooveDrop {
    /// It becomes the groove of the clip under the pointer.
    Clip(ClipId),
    /// It becomes a clip of its notes on the MIDI track.
    Track(TrackId),
    /// It becomes a clip of its notes on a new MIDI track, from the drop area
    /// under the tracks.
    NewTrack,
}

/// Where a groove dropped at `(x, y)`, in points from the lanes' top-left
/// corner, lands, with the lanes `height` points tall: on a clip, on a MIDI
/// track's lane, or in the drop area; `None` on an audio track's empty lane
/// and elsewhere. The position of a clip made snaps as [`drop_target`]'s
/// does.
#[must_use]
pub fn groove_drop_target(
    view: &ArrangementView,
    doc: &Document,
    height: f32,
    x: f32,
    y: f32,
    ignore_snap: bool,
) -> Option<(GrooveDrop, DropTarget)> {
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
        let top = last - scroll;
        let target = DropTarget {
            track: None,
            at,
            top,
            height: height - top,
            lane: None,
        };
        return Some((GrooveDrop::NewTrack, target));
    }
    let row = rows
        .iter()
        .find(|row| row.top <= content_y && content_y < row.bottom())?;
    let target = DropTarget {
        track: Some(row.track),
        at,
        top: row.top - scroll,
        height: row.height,
        lane: None,
    };
    let clip = doc
        .track(row.track)?
        .clips()
        .find(|clip| clip.start <= raw && raw < clip.start + clip.length)
        .map(|clip| clip.id);
    match (clip, row.kind) {
        (Some(clip), _) => Some((GrooveDrop::Clip(clip), target)),
        (None, TrackKind::Midi) => Some((GrooveDrop::Track(row.track), target)),
        (None, _) => None,
    }
}
