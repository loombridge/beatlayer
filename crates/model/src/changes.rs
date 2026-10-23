//! Change sets: what differs between two versions of a document.

use std::collections::BTreeMap;

use imbl::ordmap::DiffItem;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::Clip;
use crate::document::Document;
use crate::ids::{ClipId, DeviceId, SceneId, TrackId};
use crate::track::Track;

/// How one object differs between two versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// It's only in the newer version.
    Added,
    /// It's only in the older version.
    Removed,
    /// It's in both, but different.
    Modified,
}

/// What differs between two versions of a document, for the compiler.
///
/// Each map lists only the objects that differ, so the compiler can rebuild
/// the parts of the engine program they feed and keep the rest. Finding
/// them skips every node the two versions share, so the cost follows the
/// size of the change rather than the size of the song.
///
/// It's also what a command run by ID returns (see [`crate::registry`]),
/// serialised with each map keyed by ID.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "What a command changed: whether the song settings (the tempo map, the \
                   time-signature map, the crossfader, the Arrangement loop, or Solo in Place) \
                   changed, whether the track list changed, and which tracks, scenes, clips, and \
                   devices were added, removed, or modified, keyed by ID. Every field is empty or \
                   false if nothing changed."
)]
pub struct ChangeSet {
    /// The song settings changed: the tempo map, the time-signature map, the
    /// crossfader, the Arrangement loop, or Solo in Place.
    pub settings: bool,
    /// The track tree changed: a track was added, removed, or moved, so
    /// routing may have changed.
    pub layout: bool,
    /// Tracks whose own settings changed: name, mixer, sends, the tracks
    /// inside a group, or the order of the device chain. Changes to a track's
    /// clips or to its devices' settings are listed under `clips` and
    /// `devices` instead.
    pub tracks: BTreeMap<TrackId, Change>,
    /// Scenes that were added, removed, renamed, or moved. A scene whose
    /// clips changed only is listed under `clips`, not here.
    pub scenes: BTreeMap<SceneId, Change>,
    /// Clips that changed, including clips moved to another track or
    /// between the Arrangement and a Session slot.
    pub clips: BTreeMap<ClipId, Change>,
    /// Devices whose settings changed, including devices moved to another
    /// track.
    pub devices: BTreeMap<DeviceId, Change>,
}

impl ChangeSet {
    /// What changed from `old` to `new`.
    #[must_use]
    pub fn between(old: &Document, new: &Document) -> Self {
        let mut changes = Self {
            settings: old.settings != new.settings,
            layout: old.main != new.main
                || !(old.top.ptr_eq(&new.top) || old.top == new.top)
                || !(old.returns.ptr_eq(&new.returns) || old.returns == new.returns),
            ..Self::default()
        };
        scene_changes(old, new, &mut changes);
        for item in old.tracks.diff(&new.tracks) {
            match item {
                DiffItem::Add(id, track) => {
                    changes.layout = true;
                    changes.whole_track(*id, track, Change::Added);
                }
                DiffItem::Remove(id, track) => {
                    changes.layout = true;
                    changes.whole_track(*id, track, Change::Removed);
                }
                DiffItem::Update {
                    old: (id, old),
                    new: (_, new),
                } => changes.track_update(*id, old, new),
            }
        }
        changes
    }

    /// Whether nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.settings
            && !self.layout
            && self.tracks.is_empty()
            && self.scenes.is_empty()
            && self.clips.is_empty()
            && self.devices.is_empty()
    }

    fn whole_track(&mut self, id: TrackId, track: &Track, change: Change) {
        record(&mut self.tracks, id, change);
        for clip in track.clips.keys() {
            record(&mut self.clips, *clip, change);
        }
        for clip in track.slots.values() {
            record(&mut self.clips, clip.id, change);
        }
        for clip in track.take_lanes().flat_map(|lane| lane.clips.iter()) {
            record(&mut self.clips, clip.id, change);
        }
        for device in track.devices_deep() {
            record(&mut self.devices, device.id, change);
        }
    }

    fn track_update(&mut self, id: TrackId, old: &Track, new: &Track) {
        if old.own_settings_differ(new) {
            record(&mut self.tracks, id, Change::Modified);
        }
        if !(old.children.ptr_eq(&new.children) || old.children == new.children) {
            self.layout = true;
        }
        for item in old.clips.diff(&new.clips) {
            match item {
                DiffItem::Add(clip, _) => record(&mut self.clips, *clip, Change::Added),
                DiffItem::Remove(clip, _) => record(&mut self.clips, *clip, Change::Removed),
                DiffItem::Update { old: (clip, _), .. } => {
                    record(&mut self.clips, *clip, Change::Modified);
                }
            }
        }
        for item in old.slots.diff(&new.slots) {
            match item {
                DiffItem::Add(_, clip) => record(&mut self.clips, clip.id, Change::Added),
                DiffItem::Remove(_, clip) => record(&mut self.clips, clip.id, Change::Removed),
                DiffItem::Update {
                    old: (_, before),
                    new: (_, after),
                } => {
                    if before != after {
                        record(&mut self.clips, before.id, Change::Modified);
                    }
                }
            }
        }
        if !old.take_lanes.ptr_eq(&new.take_lanes) {
            let lane_clips = |track: &Track| -> BTreeMap<ClipId, Clip> {
                track
                    .take_lanes()
                    .flat_map(|lane| lane.clips.iter())
                    .map(|clip| (clip.id, clip.clone()))
                    .collect()
            };
            let (before, after) = (lane_clips(old), lane_clips(new));
            for (id, clip) in &before {
                match after.get(id) {
                    None => record(&mut self.clips, *id, Change::Removed),
                    Some(now) if now != clip => record(&mut self.clips, *id, Change::Modified),
                    Some(_) => {}
                }
            }
            for id in after.keys().filter(|id| !before.contains_key(id)) {
                record(&mut self.clips, *id, Change::Added);
            }
        }
        if !old.devices.ptr_eq(&new.devices) {
            let before: BTreeMap<_, _> = old.devices_deep().map(|d| (d.id, d)).collect();
            let after: BTreeMap<_, _> = new.devices_deep().map(|d| (d.id, d)).collect();
            for (device, old_device) in &before {
                match after.get(device) {
                    None => record(&mut self.devices, *device, Change::Removed),
                    Some(new_device) if old_device != new_device => {
                        record(&mut self.devices, *device, Change::Modified);
                    }
                    Some(_) => {}
                }
            }
            for device in after.keys() {
                if !before.contains_key(device) {
                    record(&mut self.devices, *device, Change::Added);
                }
            }
        }
    }
}

/// Records a change, merging an object that left one track and joined
/// another into one modification.
fn record<K: Ord>(changes: &mut BTreeMap<K, Change>, id: K, change: Change) {
    changes
        .entry(id)
        .and_modify(|existing| {
            if *existing != change {
                *existing = Change::Modified;
            }
        })
        .or_insert(change);
}

/// Records scenes that were added, removed, renamed, or moved. A scene's
/// clips are compared with the tracks', so a scene with one more or fewer
/// clips isn't a scene change.
fn scene_changes(old: &Document, new: &Document, changes: &mut ChangeSet) {
    for (position, scene) in new.scenes().enumerate() {
        match old.scene(scene.id) {
            None => record(&mut changes.scenes, scene.id, Change::Added),
            Some(before) if before != scene || old.scene_position(scene.id) != Some(position) => {
                record(&mut changes.scenes, scene.id, Change::Modified);
            }
            Some(_) => {}
        }
    }
    for scene in old.scenes() {
        if new.scene(scene.id).is_none() {
            record(&mut changes.scenes, scene.id, Change::Removed);
        }
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, TempoMap, Ticks};

    use super::*;
    use crate::clip::{Clip, ClipData, MidiData};
    use crate::command::Command;
    use crate::device::{Device, DeviceKind};
    use crate::editor::Editor;
    use crate::ids::{ClipId, ParamId, SceneId};
    use crate::track::TrackKind;

    struct Fixture {
        editor: Editor,
        group: TrackId,
        first: TrackId,
        second: TrackId,
        clip: ClipId,
        devices: [DeviceId; 2],
    }

    impl Fixture {
        fn new() -> Self {
            let mut editor = Editor::new();
            let group = track(&mut editor, TrackKind::Group, None);
            let first = track(&mut editor, TrackKind::Midi, Some(group));
            let second = track(&mut editor, TrackKind::Midi, None);
            let clip = editor.new_id();
            editor
                .apply(Command::CreateClip {
                    track: first,
                    clip: Clip::new(
                        clip,
                        Ticks::ZERO,
                        Ticks::WHOLE,
                        ClipData::Midi(MidiData::default()),
                    ),
                })
                .unwrap();
            let devices = [(); 2].map(|()| {
                let id = editor.new_id();
                let kind = DeviceKind::Native {
                    device_type: "beatlayer.utility".to_owned(),
                };
                editor
                    .apply(Command::InsertDevice {
                        track: first,
                        index: 0,
                        device: Device::new(id, "", kind),
                    })
                    .unwrap();
                id
            });
            Self {
                editor,
                group,
                first,
                second,
                clip,
                devices,
            }
        }

        /// Applies a command and returns its change set, after checking it
        /// matches the one between the versions either side.
        fn change(&mut self, command: Command) -> ChangeSet {
            let before = self.editor.document().clone();
            let changes = self.editor.apply(command).unwrap();
            assert_eq!(changes, ChangeSet::between(&before, self.editor.document()));
            changes
        }
    }

    fn track(editor: &mut Editor, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent,
                index: 0,
            })
            .unwrap();
        id
    }

    fn only<K: Ord + Copy>(id: K, change: Change) -> BTreeMap<K, Change> {
        BTreeMap::from([(id, change)])
    }

    #[test]
    fn nothing_changed() {
        let fixture = Fixture::new();
        let doc = fixture.editor.document();
        assert!(ChangeSet::between(doc, doc).is_empty());
        assert!(ChangeSet::between(doc, &doc.clone()).is_empty());
    }

    #[test]
    fn clip_edits() {
        let mut f = Fixture::new();
        let clip = f.clip;
        let changes = f.change(Command::SetClipMute { clip, mute: true });
        assert_eq!(
            changes,
            ChangeSet {
                clips: only(clip, Change::Modified),
                ..ChangeSet::default()
            }
        );
        // A clip moved to another track is modified, not removed and added.
        let changes = f.change(Command::MoveClip {
            clip,
            track: f.second,
            start: Ticks::WHOLE,
        });
        assert_eq!(
            changes,
            ChangeSet {
                clips: only(clip, Change::Modified),
                ..ChangeSet::default()
            }
        );
        let changes = f.change(Command::DeleteClip { clip });
        assert_eq!(changes.clips, only(clip, Change::Removed));
    }

    #[test]
    fn track_edits() {
        let mut f = Fixture::new();
        let changes = f.change(Command::SetVolume {
            track: f.second,
            volume: 0.5,
        });
        assert_eq!(
            changes,
            ChangeSet {
                tracks: only(f.second, Change::Modified),
                ..ChangeSet::default()
            }
        );

        let new: TrackId = f.editor.new_id();
        let changes = f.change(Command::CreateTrack {
            id: new,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        });
        assert!(changes.layout);
        assert_eq!(changes.tracks, only(new, Change::Added));

        // Moving a track into a group changes the layout and the group.
        let changes = f.change(Command::MoveTrack {
            track: new,
            parent: Some(f.group),
            index: 0,
        });
        assert!(changes.layout);
        assert_eq!(changes.tracks, only(f.group, Change::Modified));
    }

    #[test]
    fn deleting_a_group_removes_everything_in_it() {
        let mut f = Fixture::new();
        let changes = f.change(Command::DeleteTrack { track: f.group });
        assert!(changes.layout);
        assert_eq!(
            changes.tracks,
            BTreeMap::from([(f.group, Change::Removed), (f.first, Change::Removed)])
        );
        assert_eq!(changes.clips, only(f.clip, Change::Removed));
        assert_eq!(
            changes.devices,
            f.devices.iter().map(|d| (*d, Change::Removed)).collect()
        );

        // Undo brings them all back.
        let changes = f.editor.undo().unwrap();
        assert_eq!(changes.clips, only(f.clip, Change::Added));
        assert_eq!(changes.tracks.len(), 2);
    }

    // Spec: session-view R4, R6, R8, R12
    #[test]
    fn scene_and_slot_edits() {
        let mut f = Fixture::new();
        let scene = f.editor.document().scenes().next().unwrap().id;

        // Renaming and moving a scene are scene changes, not track or clip
        // changes.
        let changes = f.change(Command::RenameScene {
            scene,
            name: "Verse".to_owned(),
        });
        assert_eq!(
            changes,
            ChangeSet {
                scenes: only(scene, Change::Modified),
                ..ChangeSet::default()
            }
        );
        let order: Vec<SceneId> = f.editor.document().scenes().map(|scene| scene.id).collect();
        let changes = f.change(Command::MoveScene { scene, index: 3 });
        // Its number, and the numbers of the scenes it passed, change.
        let expected: BTreeMap<SceneId, Change> = order[..4]
            .iter()
            .map(|&id| (id, Change::Modified))
            .collect();
        assert_eq!(changes.scenes, expected);

        // Inserting a scene renumbers the ones below it, as observed in
        // Live; deleting it renumbers them back.
        let before: Vec<SceneId> = f.editor.document().scenes().map(|scene| scene.id).collect();
        let new: SceneId = f.editor.new_id();
        let changes = f.change(Command::CreateScene { id: new, index: 0 });
        let expected: BTreeMap<SceneId, Change> = before
            .iter()
            .map(|&id| (id, Change::Modified))
            .chain(std::iter::once((new, Change::Added)))
            .collect();
        assert_eq!(changes.scenes, expected);
        assert!(changes.clips.is_empty());
        let changes = f.change(Command::DeleteScene { scene: new });
        let expected: BTreeMap<SceneId, Change> = before
            .iter()
            .map(|&id| (id, Change::Modified))
            .chain(std::iter::once((new, Change::Removed)))
            .collect();
        assert_eq!(changes.scenes, expected);

        // A clip in a slot is listed under clips; the scene it sits in
        // doesn't change.
        let clip: ClipId = f.editor.new_id();
        let changes = f.change(Command::CreateSlotClip {
            track: f.first,
            scene,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::default()),
            ),
        });
        assert_eq!(changes.clips, only(clip, Change::Added));
        assert!(changes.scenes.is_empty());

        // Duplicating adds the scene and the copies of its clips.
        let copy: SceneId = f.editor.new_id();
        let copy_clip: ClipId = f.editor.new_id();
        let changes = f.change(Command::DuplicateScene {
            scene,
            id: copy,
            clips: vec![(f.first, copy_clip)],
        });
        // The copy goes straight after the scene, so the scenes after it
        // are renumbered too.
        assert_eq!(changes.scenes.get(&copy), Some(&Change::Added));
        assert_eq!(changes.clips, only(copy_clip, Change::Added));

        // Deleting a scene removes the clips in its slots with it, and
        // counts as a scene change.
        let changes = f.change(Command::DeleteScene { scene });
        assert_eq!(changes.scenes.get(&scene), Some(&Change::Removed));
        assert_eq!(changes.clips, only(clip, Change::Removed));
    }

    #[test]
    fn device_edits() {
        let mut f = Fixture::new();
        let [a, b] = f.devices;
        let changes = f.change(Command::SetDeviceParam {
            device: a,
            param: ParamId(1),
            value: 0.5,
        });
        assert_eq!(
            changes,
            ChangeSet {
                devices: only(a, Change::Modified),
                ..ChangeSet::default()
            }
        );
        // Reordering a chain changes the track, not the devices. Both were
        // inserted at the front, so the chain is [b, a].
        let changes = f.change(Command::MoveDevice {
            device: a,
            track: f.first,
            index: 0,
        });
        assert_eq!(
            changes,
            ChangeSet {
                tracks: only(f.first, Change::Modified),
                ..ChangeSet::default()
            }
        );
        // Moving a device to another track changes both chains and the
        // device.
        let changes = f.change(Command::MoveDevice {
            device: b,
            track: f.second,
            index: 0,
        });
        assert_eq!(changes.devices, only(b, Change::Modified));
        assert_eq!(
            changes.tracks,
            BTreeMap::from([(f.first, Change::Modified), (f.second, Change::Modified)])
        );
        assert!(!changes.layout);
    }

    #[test]
    fn settings_edits() {
        let mut f = Fixture::new();
        let changes = f.change(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(90.0).unwrap()),
        });
        assert_eq!(
            changes,
            ChangeSet {
                settings: true,
                ..ChangeSet::default()
            }
        );
    }
}
