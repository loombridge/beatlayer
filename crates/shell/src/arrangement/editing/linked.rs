//! What linked tracks add to an edit's targets (WP-6.23; the linked-track
//! editing spec, R7 to R9): the same lane of every track in the group, and
//! on it the clips that start and end where the named ones do.

use bl_model::{ClipId, Document, TrackId};
use bl_shell_spec::arrangement::Lane;

use super::Place;

/// Whether an edit reaches linked tracks: unless its `linked` argument is
/// `false` (R15).
pub(crate) fn on(linked: Option<bool>) -> bool {
    linked != Some(false)
}

/// The place on `other` that sits where `place` does on its track (R7).
fn counterpart(doc: &Document, place: Place, other: TrackId) -> Option<Place> {
    let lane = doc.matching_lane(place.track, place.lane, other)?;
    Some(Place { track: other, lane })
}

/// `places` and their counterparts on the tracks linked with theirs.
pub(crate) fn places(doc: &Document, places: Vec<Place>) -> Vec<Place> {
    let mut all = places.clone();
    for place in places {
        for other in doc.linked_with(place.track) {
            if let Some(same) = counterpart(doc, place, other)
                && !all.contains(&same)
            {
                all.push(same);
            }
        }
    }
    all
}

/// `lanes` and their counterparts; an automation lane has none.
pub(crate) fn lanes(doc: &Document, lanes: Vec<Lane>) -> Vec<Lane> {
    let mut all = lanes.clone();
    for lane in &lanes {
        let Some(place) = Place::of_lane(lane) else {
            continue;
        };
        for same in places(doc, vec![place]) {
            if !all.contains(&same.as_lane()) {
                all.push(same.as_lane());
            }
        }
    }
    all
}

/// `clips` and the clips on the linked tracks' counterpart lanes that start
/// and end where they do.
pub(crate) fn clips(doc: &Document, clips: Vec<ClipId>) -> Vec<ClipId> {
    let mut all = clips.clone();
    for id in clips {
        for linked in doc.linked_clips(id) {
            if !all.contains(&linked) {
                all.push(linked);
            }
        }
    }
    all
}
