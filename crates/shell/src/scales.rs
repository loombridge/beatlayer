//! The set's current scale (WP-6.7): the scale the Control Bar's scale
//! controls show, which follows the selection and the clips that play.
//!
//! The rules are in the scales spec.

use bl_core::time::{Frames, Ticks};
use bl_engine::{SlotState, SlotStatus};
use bl_model::{ClipId, Document, Scale, ScaleName, SceneId, TrackId};

/// A Session clip that started playing: the track and the scene whose slot
/// it is, and the song frame it launched on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The track whose slot it is.
    pub track: TrackId,
    /// The scene whose slot it is.
    pub scene: SceneId,
    /// The song frame it launched on.
    pub frame: Frames,
}

/// What the Control Bar's scale controls show (the scales spec, R6): the
/// selected clips' scale, where every selected clip has it, and the set's
/// current scale when no clip is selected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    /// Whether the Scale Mode switch is on: `None` where the selected clips
    /// differ, so the switch is partly filled.
    pub enabled: Option<bool>,
    /// The root note, where every selected clip has it.
    pub root: Option<u8>,
    /// The scale's name, where every selected clip has it.
    pub name: Option<ScaleName>,
}

impl Display {
    /// The scale one clip has.
    fn of(scale: Scale) -> Self {
        Self {
            enabled: Some(scale.enabled),
            root: Some(scale.root),
            name: Some(scale.name),
        }
    }

    /// What several clips' scales show: the value where every clip has it,
    /// and nothing where they differ.
    fn of_many(scales: &[Scale]) -> Self {
        let first = scales.first().copied().unwrap_or_default();
        let same_enabled = scales.iter().all(|scale| scale.enabled == first.enabled);
        let same_root = scales.iter().all(|scale| scale.root == first.root);
        let same_name = scales.iter().all(|scale| scale.name == first.name);
        Self {
            enabled: same_enabled.then_some(first.enabled),
            root: same_root.then_some(first.root),
            name: same_name.then_some(first.name),
        }
    }
}

/// The scale the Control Bar's scale controls show (the scales spec, R6
/// and R9): the selected clips' scale, where they agree, and the set's
/// current scale when no clip is selected.
#[must_use]
pub fn display(doc: &Document, clips: &[ClipId]) -> Display {
    let scales: Vec<Scale> = clips
        .iter()
        .filter_map(|clip| doc.clip(*clip))
        .map(|clip| clip.scale)
        .collect();
    if scales.is_empty() {
        return Display::of(doc.settings().scale);
    }
    Display::of_many(&scales)
}

/// The scale of the clip the Clip View shows (the scales spec, R10).
#[must_use]
pub fn shown_scale(doc: &Document, shown: Option<ClipId>) -> Option<Scale> {
    shown.and_then(|clip| doc.clip(clip)).map(|clip| clip.scale)
}

/// The Session clips that started playing since the shell last looked, and
/// the frames the slots that play launched on, as the shell remembers them.
#[must_use]
pub fn new_launches(
    states: &[SlotStatus],
    seen: &std::collections::BTreeMap<(TrackId, SceneId), Frames>,
) -> (
    Vec<Launch>,
    std::collections::BTreeMap<(TrackId, SceneId), Frames>,
) {
    let mut launches = Vec::new();
    let mut now = std::collections::BTreeMap::new();
    for slot in states {
        if slot.state != SlotState::Playing {
            continue;
        }
        let Some(frame) = slot.started else {
            continue;
        };
        let key = (slot.track, slot.scene);
        if seen.get(&key) != Some(&frame) {
            launches.push(Launch {
                track: slot.track,
                scene: slot.scene,
                frame,
            });
        }
        now.insert(key, frame);
    }
    (launches, now)
}

/// The scale of the Session clip that started playing (the scales spec,
/// R11): the one that started last, and among those that started on the
/// same frame the one on the track furthest left in the Session View.
#[must_use]
pub fn launched_scale(doc: &Document, launches: &[Launch], order: &[TrackId]) -> Option<Scale> {
    let latest = launches.iter().map(|launch| launch.frame).max()?;
    let mut winner: Option<&Launch> = None;
    for launch in launches.iter().filter(|launch| launch.frame == latest) {
        let better = match winner {
            None => true,
            Some(current) => place(order, launch.track) < place(order, current.track),
        };
        if better {
            winner = Some(launch);
        }
    }
    let launch = winner?;
    doc.slot(launch.track, launch.scene).map(|clip| clip.scale)
}

/// The Arrangement clip the playhead is in (the scales spec, R11): the
/// first one, on the track furthest left, on a track that plays the
/// Arrangement rather than a Session clip.
#[must_use]
pub fn arrangement_clip(doc: &Document, at: Ticks, taken_over: &[TrackId]) -> Option<ClipId> {
    for track in doc.track_order() {
        if taken_over.contains(&track) {
            continue;
        }
        let Some(lane) = doc.track(track) else {
            continue;
        };
        if let Some(clip) = lane
            .clips()
            .find(|clip| clip.start <= at && at < clip.end())
        {
            return Some(clip.id);
        }
    }
    None
}

/// The track's place in the display order, or the last place when it isn't
/// in it.
fn place(order: &[TrackId], track: TrackId) -> usize {
    order
        .iter()
        .position(|id| *id == track)
        .unwrap_or(usize::MAX)
}
