//! The document: the whole song as an immutable, persistent tree.

use bl_core::id::Id;
use bl_core::time::{TempoMap, TimeSignatureMap};
use imbl::{OrdMap, Vector};
use serde::{Deserialize, Serialize};

use crate::clip::Clip;
use crate::device::Device;
use crate::ids::{ClipId, DeviceId, TrackId};
use crate::shared::Shared;
use crate::track::{Track, TrackKind};

/// Song-wide settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The tempo map. The default is 120 BPM throughout.
    pub tempo: TempoMap,
    /// The time-signature map. The default is 4/4 throughout.
    pub time_signatures: TimeSignatureMap,
}

/// A song: its settings, its tracks, and everything on them.
///
/// A `Document` is a value. Cloning it is cheap (a few reference-count
/// increments) and the clone never changes, so the compiler, autosave, and
/// analysis can each hold their own snapshot on other threads while the main
/// thread edits. An edit builds a new version that shares every node it
/// didn't touch with the old one, which is how the undo history stays small.
///
/// The only way to change a document is [`Document::apply`] with a
/// [`Command`](crate::Command), usually through an [`Editor`](crate::Editor).
///
/// The tracks form a tree:
///
/// - the top-level tracks, in order, where each group holds its own ordered
///   list of audio, MIDI, and group tracks;
/// - the return tracks, in order;
/// - the Main track.
///
/// That is also the display order: [`Document::track_order`].
///
/// A document serialises as that tree, with each group's tracks inline.
/// Deserialising checks every rule a command would, and fails with a
/// [`ModelError`](crate::ModelError) message if the input breaks one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    into = "crate::repr::DocumentRepr",
    try_from = "crate::repr::DocumentRepr"
)]
pub struct Document {
    pub(crate) settings: Settings,
    pub(crate) main: TrackId,
    pub(crate) top: Vector<TrackId>,
    pub(crate) returns: Vector<TrackId>,
    pub(crate) tracks: OrdMap<TrackId, Shared<Track>>,
    pub(crate) index: Index,
}

/// Lookups derived from the tree, kept in step with it by every command and
/// rebuilt when a document is loaded. They're never saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Index {
    /// The track that holds each clip.
    pub(crate) clip_track: OrdMap<ClipId, TrackId>,
    /// The track whose chain holds each device.
    pub(crate) device_track: OrdMap<DeviceId, TrackId>,
    /// The group that holds each grouped track.
    pub(crate) parent: OrdMap<TrackId, TrackId>,
}

impl Document {
    /// An empty song: default settings and a Main track with this ID.
    #[must_use]
    pub fn new(main: TrackId) -> Self {
        let mut tracks = OrdMap::new();
        tracks.insert(
            main,
            Shared::new(Track::new(main, TrackKind::Main, "Main".to_owned())),
        );
        Self {
            settings: Settings::default(),
            main,
            top: Vector::new(),
            returns: Vector::new(),
            tracks,
            index: Index::default(),
        }
    }

    /// The song-wide settings.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The Main track's ID.
    #[must_use]
    pub fn main_id(&self) -> TrackId {
        self.main
    }

    /// The Main track.
    #[must_use]
    pub fn main(&self) -> &Track {
        self.tracks
            .get(&self.main)
            .expect("a document always has its Main track")
    }

    /// The track with this ID.
    #[must_use]
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.get(&id).map(|track| &**track)
    }

    /// The tracks that aren't in a group, other than the returns and Main, in
    /// order.
    pub fn top_level(&self) -> impl ExactSizeIterator<Item = TrackId> + DoubleEndedIterator + '_ {
        self.top.iter().copied()
    }

    /// The return tracks, in order.
    pub fn returns(&self) -> impl ExactSizeIterator<Item = TrackId> + DoubleEndedIterator + '_ {
        self.returns.iter().copied()
    }

    /// The group a track is in, if it's in one.
    #[must_use]
    pub fn parent(&self, track: TrackId) -> Option<TrackId> {
        self.index.parent.get(&track).copied()
    }

    /// Every track ID in display order: the top-level tracks with each
    /// group's contents straight after the group, then the returns, then
    /// Main.
    #[must_use]
    pub fn track_order(&self) -> Vec<TrackId> {
        let mut order = Vec::with_capacity(self.tracks.len());
        let mut pending: Vec<TrackId> = self.top.iter().rev().copied().collect();
        while let Some(id) = pending.pop() {
            order.push(id);
            if let Some(track) = self.tracks.get(&id) {
                pending.extend(track.children.iter().rev().copied());
            }
        }
        order.extend(self.returns.iter().copied());
        order.push(self.main);
        order
    }

    /// Every track in display order (see [`Document::track_order`]).
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.track_order()
            .into_iter()
            .filter_map(|id| self.track(id))
    }

    /// How many tracks there are, counting the returns and Main.
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// The clip with this ID.
    #[must_use]
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        let track = self.index.clip_track.get(&id)?;
        self.tracks.get(track)?.clips.get(&id).map(|clip| &**clip)
    }

    /// The track that holds a clip.
    #[must_use]
    pub fn clip_track(&self, id: ClipId) -> Option<TrackId> {
        self.index.clip_track.get(&id).copied()
    }

    /// Every clip ID, in ID order.
    pub fn clip_ids(&self) -> impl ExactSizeIterator<Item = ClipId> + '_ {
        self.index.clip_track.keys().copied()
    }

    /// How many clips there are.
    #[must_use]
    pub fn clip_count(&self) -> usize {
        self.index.clip_track.len()
    }

    /// The device with this ID.
    #[must_use]
    pub fn device(&self, id: DeviceId) -> Option<&Device> {
        let track = self.tracks.get(self.index.device_track.get(&id)?)?;
        track
            .devices
            .iter()
            .find(|device| device.id == id)
            .map(|device| &**device)
    }

    /// The track whose chain holds a device.
    #[must_use]
    pub fn device_track(&self, id: DeviceId) -> Option<TrackId> {
        self.index.device_track.get(&id).copied()
    }

    /// Every device ID, in ID order.
    pub fn device_ids(&self) -> impl ExactSizeIterator<Item = DeviceId> + '_ {
        self.index.device_track.keys().copied()
    }

    /// How many devices there are.
    #[must_use]
    pub fn device_count(&self) -> usize {
        self.index.device_track.len()
    }

    /// Whether any track, clip, or device has this ID.
    #[must_use]
    pub fn contains(&self, id: Id) -> bool {
        self.tracks.contains_key(&TrackId(id))
            || self.index.clip_track.contains_key(&ClipId(id))
            || self.index.device_track.contains_key(&DeviceId(id))
    }

    /// The largest ID of any track, clip, or device.
    #[must_use]
    pub fn max_id(&self) -> Id {
        let track = self.tracks.get_max().map(|(id, _)| id.0);
        let clip = self.index.clip_track.get_max().map(|(id, _)| id.0);
        let device = self.index.device_track.get_max().map(|(id, _)| id.0);
        [track, clip, device]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(self.main.0)
    }

    /// Checks that the document's derived lookups match its tree and that the
    /// tree follows every rule a loaded document must follow.
    ///
    /// A document built through commands or deserialisation always passes;
    /// this is for tests.
    ///
    /// # Errors
    ///
    /// A description of the first inconsistency found.
    pub fn check_invariants(&self) -> Result<(), String> {
        let rebuilt = Self::try_from(crate::repr::DocumentRepr::from(self))
            .map_err(|error| format!("the tree breaks a rule: {error}"))?;
        if rebuilt != *self {
            return Err("the track map holds tracks that aren't in the tree".to_owned());
        }
        if rebuilt.index != self.index {
            return Err("the derived lookups don't match the tree".to_owned());
        }
        Ok(())
    }
}

impl PartialEq for Document {
    /// Compares the songs, skipping any part both versions share.
    fn eq(&self, other: &Self) -> bool {
        self.main == other.main
            && self.settings == other.settings
            && (self.top.ptr_eq(&other.top) || self.top == other.top)
            && (self.returns.ptr_eq(&other.returns) || self.returns == other.returns)
            && self.tracks == other.tracks
    }
}

#[cfg(test)]
mod tests {
    use bl_core::id::IdAllocator;

    use super::*;
    use crate::command::Command;

    fn track_ids(n: usize) -> (TrackId, Vec<TrackId>) {
        let mut ids = IdAllocator::new();
        let main = TrackId(ids.allocate());
        (main, (0..n).map(|_| TrackId(ids.allocate())).collect())
    }

    fn create(id: TrackId, kind: TrackKind, parent: Option<TrackId>, index: usize) -> Command {
        Command::CreateTrack {
            id,
            kind,
            name: String::new(),
            parent,
            index,
        }
    }

    #[test]
    fn new_document_has_only_main() {
        let (main, _) = track_ids(0);
        let doc = Document::new(main);
        assert_eq!(doc.track_count(), 1);
        assert_eq!(doc.main().kind(), TrackKind::Main);
        assert_eq!(doc.main().name(), "Main");
        assert_eq!(doc.track_order(), vec![main]);
        assert_eq!(doc.max_id(), main.0);
        assert!(doc.contains(main.0));
        assert_eq!(doc.settings(), &Settings::default());
        doc.check_invariants().unwrap();
    }

    #[test]
    fn display_order() {
        let (main, t) = track_ids(6);
        let mut doc = Document::new(main);
        // A return created first still comes after every other track.
        doc.apply(&create(t[0], TrackKind::Return, None, 0))
            .unwrap();
        doc.apply(&create(t[1], TrackKind::Audio, None, 0)).unwrap();
        doc.apply(&create(t[2], TrackKind::Group, None, 1)).unwrap();
        doc.apply(&create(t[3], TrackKind::Midi, Some(t[2]), 0))
            .unwrap();
        doc.apply(&create(t[4], TrackKind::Group, Some(t[2]), 1))
            .unwrap();
        doc.apply(&create(t[5], TrackKind::Audio, Some(t[4]), 0))
            .unwrap();
        assert_eq!(
            doc.track_order(),
            vec![t[1], t[2], t[3], t[4], t[5], t[0], main]
        );
        assert_eq!(doc.top_level().collect::<Vec<_>>(), vec![t[1], t[2]]);
        assert_eq!(doc.returns().collect::<Vec<_>>(), vec![t[0]]);
        assert_eq!(doc.parent(t[5]), Some(t[4]));
        assert_eq!(doc.parent(t[4]), Some(t[2]));
        assert_eq!(doc.parent(t[2]), None);
        assert_eq!(
            doc.tracks().map(Track::id).collect::<Vec<_>>(),
            doc.track_order()
        );
        assert_eq!(doc.max_id(), t[5].0);
        doc.check_invariants().unwrap();
    }

    #[test]
    fn clones_share_and_stay_put() {
        let (main, t) = track_ids(1);
        let mut doc = Document::new(main);
        let before = doc.clone();
        doc.apply(&create(t[0], TrackKind::Audio, None, 0)).unwrap();
        assert_eq!(before.track_count(), 1);
        assert_eq!(doc.track_count(), 2);
        assert_ne!(before, doc);
        // The untouched Main track is the same node in both versions.
        assert!(Shared::ptr_eq(
            before.tracks.get(&main).unwrap(),
            doc.tracks.get(&main).unwrap()
        ));
    }

    #[test]
    fn documents_cross_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Document>();
    }
}
