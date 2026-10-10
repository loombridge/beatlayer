//! Change sets: what differs between two versions of a document.

use std::collections::BTreeMap;

use imbl::ordmap::DiffItem;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::ids::{ClipId, DeviceId, TrackId};
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
    description = "What a command changed: whether the tempo or time-signature maps changed, \
                   whether the track list changed, and which tracks, clips, and devices were \
                   added, removed, or modified, keyed by ID. Every field is empty or false if \
                   nothing changed."
)]
pub struct ChangeSet {
    /// The tempo map or time-signature map changed.
    pub settings: bool,
    /// The track tree changed: a track was added, removed, or moved, so
    /// routing may have changed.
    pub layout: bool,
    /// Tracks whose own settings changed: name, mixer, the tracks inside a
    /// group, or the order of the device chain. Changes to a track's clips or
    /// to its devices' settings are listed under `clips` and `devices`
    /// instead.
    pub tracks: BTreeMap<TrackId, Change>,
    /// Clips that changed, including clips moved to another track.
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
            && self.clips.is_empty()
            && self.devices.is_empty()
    }

    fn whole_track(&mut self, id: TrackId, track: &Track, change: Change) {
        record(&mut self.tracks, id, change);
        for clip in track.clips.keys() {
            record(&mut self.clips, *clip, change);
        }
        for device in &track.devices {
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
        if !old.devices.ptr_eq(&new.devices) {
            let before: BTreeMap<_, _> = old.devices.iter().map(|d| (d.id, d)).collect();
            let after: BTreeMap<_, _> = new.devices.iter().map(|d| (d.id, d)).collect();
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

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, TempoMap, Ticks};

    use super::*;
    use crate::clip::{Clip, ClipData, MidiData};
    use crate::command::Command;
    use crate::device::{Device, DeviceKind};
    use crate::editor::Editor;
    use crate::ids::{ClipId, ParamId};
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
