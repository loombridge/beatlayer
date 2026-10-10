//! Commands: the only way to change a document.

use std::collections::BTreeSet;

use bl_core::id::Id;
use bl_core::time::{TempoMap, Ticks, TimeSignatureMap};
use imbl::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::{Clip, ClipData};
use crate::device::Device;
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{ClipId, DeviceId, ParamId, TrackId};
use crate::shared::Shared;
use crate::track::{Track, TrackKind};

/// One edit to a document.
///
/// Commands are plain values: they serialise (tagged by `op`), so a later
/// version can log and replay them. A command that creates an object carries
/// the new object's ID, which the caller takes from
/// [`Editor::new_id`](crate::Editor::new_id).
///
/// [`Document::apply`] checks a command in full before it takes effect, and a
/// rejected command leaves the document as it was. Setting a value to what it
/// already is succeeds and changes nothing.
///
/// Each variant is a command in the registry, with an ID such as
/// `track.create`, so people and agents can run it by name with JSON
/// arguments: see [`crate::registry`]. The variants' docs are the argument
/// descriptions agents read, so they say `null` where Rust says `None`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Command {
    /// Replaces the tempo map.
    SetTempo {
        /// The new tempo map.
        tempo: TempoMap,
    },
    /// Replaces the time-signature map.
    SetTimeSignatures {
        /// The new time-signature map.
        time_signatures: TimeSignatureMap,
    },

    /// Creates an empty track with default mixer settings.
    ///
    /// An audio, MIDI, or group track goes at `index` among the top-level
    /// tracks, or among the tracks in the group `parent`. A return track goes
    /// at `index` among the returns, and `parent` must be empty. Main can't
    /// be created.
    CreateTrack {
        /// The new track's ID.
        id: TrackId,
        /// Its kind.
        kind: TrackKind,
        /// Its name.
        name: String,
        /// The group to put it in, or `null` (or nothing) for the top level.
        parent: Option<TrackId>,
        /// Where it goes in its list, from 0 to the list's length.
        index: usize,
    },
    /// Deletes a track with its clips and devices. Deleting a group deletes
    /// every track inside it. Main can't be deleted.
    DeleteTrack {
        /// The track.
        track: TrackId,
    },
    /// Moves a track to `index` in the top-level list or in the group
    /// `parent`. A return track moves within the returns. `index` counts
    /// positions after the track is taken out of its current place. Main
    /// can't be moved.
    MoveTrack {
        /// The track.
        track: TrackId,
        /// The group to move it into, or `null` (or nothing) for the top
        /// level.
        parent: Option<TrackId>,
        /// Where it goes in that list.
        index: usize,
    },
    /// Renames a track.
    RenameTrack {
        /// The track.
        track: TrackId,
        /// The new name.
        name: String,
    },
    /// Sets a track's volume, as linear gain (finite, at least 0).
    SetVolume {
        /// The track.
        track: TrackId,
        /// The gain: 1 is unity (0 dB), 0.5 about −6 dB, and 0 silence.
        #[schemars(range(min = 0.0))]
        volume: f64,
    },
    /// Sets a track's pan, from −1 to 1.
    SetPan {
        /// The track.
        track: TrackId,
        /// The pan position, from −1 (left) through 0 (centre) to 1
        /// (right).
        #[schemars(range(min = -1.0, max = 1.0))]
        pan: f64,
    },
    /// Mutes or unmutes a track other than Main.
    SetMute {
        /// The track.
        track: TrackId,
        /// Whether it's muted.
        mute: bool,
    },
    /// Solos or unsolos a track other than Main.
    SetSolo {
        /// The track.
        track: TrackId,
        /// Whether it's soloed.
        solo: bool,
    },
    /// Arms or disarms an audio or MIDI track.
    SetArm {
        /// The track.
        track: TrackId,
        /// Whether it's armed.
        arm: bool,
    },

    /// Puts a new clip on an audio or MIDI track. Its data must match the
    /// track.
    CreateClip {
        /// The track.
        track: TrackId,
        /// The clip, with a new ID.
        clip: Clip,
    },
    /// Deletes a clip.
    DeleteClip {
        /// The clip.
        clip: ClipId,
    },
    /// Moves a clip to a new start on the same or another track.
    MoveClip {
        /// The clip.
        clip: ClipId,
        /// The track it moves to.
        track: TrackId,
        /// Its new start.
        start: Ticks,
    },
    /// Moves several clips in time by the same amount, all or none.
    MoveClips {
        /// The clips, each listed once.
        clips: Vec<ClipId>,
        /// How far to move them; negative moves them earlier.
        by: Ticks,
    },
    /// Changes a clip's length, keeping its start.
    SetClipLength {
        /// The clip.
        clip: ClipId,
        /// The new length.
        length: Ticks,
    },
    /// Renames a clip.
    RenameClip {
        /// The clip.
        clip: ClipId,
        /// The new name.
        name: String,
    },
    /// Mutes or unmutes a clip.
    SetClipMute {
        /// The clip.
        clip: ClipId,
        /// Whether it's muted.
        mute: bool,
    },
    /// Replaces what a clip plays. The data must match the clip's track.
    SetClipData {
        /// The clip.
        clip: ClipId,
        /// The new data.
        data: ClipData,
    },

    /// Inserts a new device at `index` in a track's chain.
    InsertDevice {
        /// The track.
        track: TrackId,
        /// Where it goes in the chain, from 0 to the chain's length.
        index: usize,
        /// The device, with a new ID.
        device: Device,
    },
    /// Removes a device from its chain.
    RemoveDevice {
        /// The device.
        device: DeviceId,
    },
    /// Moves a device to `index` in a track's chain, on the same or another
    /// track. `index` counts positions after the device is taken out of its
    /// current place.
    MoveDevice {
        /// The device.
        device: DeviceId,
        /// The track it moves to.
        track: TrackId,
        /// Where it goes in that chain.
        index: usize,
    },
    /// Renames a device.
    RenameDevice {
        /// The device.
        device: DeviceId,
        /// The new name.
        name: String,
    },
    /// Switches a device on or off.
    SetDeviceEnabled {
        /// The device.
        device: DeviceId,
        /// Whether it's on.
        enabled: bool,
    },
    /// Sets one of a device's parameters to a finite value.
    SetDeviceParam {
        /// The device.
        device: DeviceId,
        /// The parameter.
        param: ParamId,
        /// The value, in the parameter's own units.
        value: f64,
    },
}

impl Command {
    /// A short description for the undo history, such as "Move clip".
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::SetTempo { .. } => "Change tempo",
            Self::SetTimeSignatures { .. } => "Change time signatures",
            Self::CreateTrack { .. } => "Create track",
            Self::DeleteTrack { .. } => "Delete track",
            Self::MoveTrack { .. } => "Move track",
            Self::RenameTrack { .. } => "Rename track",
            Self::SetVolume { .. } => "Change volume",
            Self::SetPan { .. } => "Change pan",
            Self::SetMute { .. } => "Mute track",
            Self::SetSolo { .. } => "Solo track",
            Self::SetArm { .. } => "Arm track",
            Self::CreateClip { .. } => "Create clip",
            Self::DeleteClip { .. } => "Delete clip",
            Self::MoveClip { .. } => "Move clip",
            Self::MoveClips { .. } => "Move clips",
            Self::SetClipLength { .. } => "Resize clip",
            Self::RenameClip { .. } => "Rename clip",
            Self::SetClipMute { .. } => "Mute clip",
            Self::SetClipData { .. } => "Edit clip",
            Self::InsertDevice { .. } => "Insert device",
            Self::RemoveDevice { .. } => "Remove device",
            Self::MoveDevice { .. } => "Move device",
            Self::RenameDevice { .. } => "Rename device",
            Self::SetDeviceEnabled { .. } => "Switch device",
            Self::SetDeviceParam { .. } => "Change parameter",
        }
    }

    /// The ID of the object this command creates, if it creates one.
    #[must_use]
    pub fn created_id(&self) -> Option<Id> {
        match self {
            Self::CreateTrack { id, .. } => Some(id.0),
            Self::CreateClip { clip, .. } => Some(clip.id.0),
            Self::InsertDevice { device, .. } => Some(device.id.0),
            _ => None,
        }
    }
}

impl Document {
    /// Applies a command, or leaves the document as it was if the command is
    /// rejected.
    ///
    /// # Errors
    ///
    /// If the command refers to an object that doesn't exist, reuses an ID,
    /// breaks a rule about which track kinds allow what, or gives a value out
    /// of range. The variants of [`ModelError`] list each case.
    pub fn apply(&mut self, command: &Command) -> Result<(), ModelError> {
        // Edit a copy, so a command rejected partway leaves `self` untouched.
        // The copy shares every node with `self` until it's edited.
        let mut next = self.clone();
        next.apply_in_place(command)?;
        *self = next;
        Ok(())
    }

    fn apply_in_place(&mut self, command: &Command) -> Result<(), ModelError> {
        match command {
            Command::SetTempo { tempo } => {
                if self.settings.tempo != *tempo {
                    self.settings.tempo = tempo.clone();
                }
                Ok(())
            }
            Command::SetTimeSignatures { time_signatures } => {
                if self.settings.time_signatures != *time_signatures {
                    self.settings.time_signatures = time_signatures.clone();
                }
                Ok(())
            }
            Command::CreateTrack {
                id,
                kind,
                name,
                parent,
                index,
            } => self.create_track(*id, *kind, name, *parent, *index),
            Command::DeleteTrack { track } => self.delete_track(*track),
            Command::MoveTrack {
                track,
                parent,
                index,
            } => self.move_track(*track, *parent, *index),
            Command::RenameTrack { track, name } => {
                if self.track_ref(*track)?.name != *name {
                    self.track_mut(*track)?.name.clone_from(name);
                }
                Ok(())
            }
            Command::SetVolume { track, volume } => {
                check_value("volume", *volume, volume.is_finite() && *volume >= 0.0)?;
                if self.track_ref(*track)?.mixer.volume != *volume {
                    self.track_mut(*track)?.mixer.volume = *volume;
                }
                Ok(())
            }
            Command::SetPan { track, pan } => {
                check_value("pan", *pan, (-1.0..=1.0).contains(pan))?;
                if self.track_ref(*track)?.mixer.pan != *pan {
                    self.track_mut(*track)?.mixer.pan = *pan;
                }
                Ok(())
            }
            Command::SetMute { track, mute } => {
                let current = self.track_ref(*track)?;
                check_kind(current, TrackKind::has_mute_and_solo, NOT_MAIN)?;
                if current.mixer.mute != *mute {
                    self.track_mut(*track)?.mixer.mute = *mute;
                }
                Ok(())
            }
            Command::SetSolo { track, solo } => {
                let current = self.track_ref(*track)?;
                check_kind(current, TrackKind::has_mute_and_solo, NOT_MAIN)?;
                if current.mixer.solo != *solo {
                    self.track_mut(*track)?.mixer.solo = *solo;
                }
                Ok(())
            }
            Command::SetArm { track, arm } => {
                let current = self.track_ref(*track)?;
                check_kind(current, TrackKind::has_arm, AUDIO_OR_MIDI)?;
                if current.mixer.arm != *arm {
                    self.track_mut(*track)?.mixer.arm = *arm;
                }
                Ok(())
            }
            Command::CreateClip { track, clip } => self.create_clip(*track, clip),
            Command::DeleteClip { clip } => {
                let owner = self.clip_owner(*clip)?;
                self.track_mut(owner)?.clips.remove(clip);
                self.index.clip_track.remove(clip);
                Ok(())
            }
            Command::MoveClip { clip, track, start } => self.move_clip(*clip, *track, *start),
            Command::MoveClips { clips, by } => self.move_clips(clips, *by),
            Command::SetClipLength { clip, length } => {
                let current = self.clip_ref(*clip)?;
                check_extent(current.start, *length)?;
                if current.length != *length {
                    self.clip_mut(*clip)?.length = *length;
                }
                Ok(())
            }
            Command::RenameClip { clip, name } => {
                if self.clip_ref(*clip)?.name != *name {
                    self.clip_mut(*clip)?.name.clone_from(name);
                }
                Ok(())
            }
            Command::SetClipMute { clip, mute } => {
                if self.clip_ref(*clip)?.mute != *mute {
                    self.clip_mut(*clip)?.mute = *mute;
                }
                Ok(())
            }
            Command::SetClipData { clip, data } => {
                let owner = self.clip_owner(*clip)?;
                check_clip_fits(self.track_ref(owner)?, data)?;
                check_clip_data(data)?;
                if self.clip_ref(*clip)?.data != *data {
                    self.clip_mut(*clip)?.data = data.clone();
                }
                Ok(())
            }
            Command::InsertDevice {
                track,
                index,
                device,
            } => self.insert_device(*track, *index, device),
            Command::RemoveDevice { device } => {
                let owner = self.device_owner(*device)?;
                let chain = &mut self.track_mut(owner)?.devices;
                let position = position_in(chain, |d| d.id == *device)
                    .ok_or(ModelError::NoSuchDevice(*device))?;
                chain.remove(position);
                self.index.device_track.remove(device);
                Ok(())
            }
            Command::MoveDevice {
                device,
                track,
                index,
            } => self.move_device(*device, *track, *index),
            Command::RenameDevice { device, name } => {
                if self.device_ref(*device)?.name != *name {
                    self.device_mut(*device)?.name.clone_from(name);
                }
                Ok(())
            }
            Command::SetDeviceEnabled { device, enabled } => {
                if self.device_ref(*device)?.enabled != *enabled {
                    self.device_mut(*device)?.enabled = *enabled;
                }
                Ok(())
            }
            Command::SetDeviceParam {
                device,
                param,
                value,
            } => {
                check_value("parameter value", *value, value.is_finite())?;
                if self.device_ref(*device)?.params.get(param) != Some(value) {
                    self.device_mut(*device)?.params.insert(*param, *value);
                }
                Ok(())
            }
        }
    }

    fn create_track(
        &mut self,
        id: TrackId,
        kind: TrackKind,
        name: &str,
        parent: Option<TrackId>,
        index: usize,
    ) -> Result<(), ModelError> {
        if kind == TrackKind::Main {
            return Err(ModelError::MainTrack);
        }
        self.check_fresh(id.0)?;
        if kind == TrackKind::Return {
            if parent.is_some() {
                return Err(ModelError::CannotGroup { kind });
            }
            insert_at(&mut self.returns, index, id)?;
        } else {
            self.check_can_hold(id, parent)?;
            insert_at(self.list_mut(parent)?, index, id)?;
            if let Some(parent) = parent {
                self.index.parent.insert(id, parent);
            }
        }
        self.tracks
            .insert(id, Shared::new(Track::new(id, kind, name.to_owned())));
        Ok(())
    }

    fn delete_track(&mut self, id: TrackId) -> Result<(), ModelError> {
        let kind = self.track_ref(id)?.kind;
        if kind == TrackKind::Main {
            return Err(ModelError::MainTrack);
        }
        let parent = self.parent(id);
        let list = self.containing_list_mut(kind, parent)?;
        let position = position_in(list, |t| *t == id).ok_or(ModelError::NoSuchTrack(id))?;
        list.remove(position);

        let mut doomed = vec![id];
        while let Some(id) = doomed.pop() {
            let track = self.tracks.remove(&id).ok_or(ModelError::NoSuchTrack(id))?;
            self.index.parent.remove(&id);
            for clip in track.clips.keys() {
                self.index.clip_track.remove(clip);
            }
            for device in &track.devices {
                self.index.device_track.remove(&device.id);
            }
            doomed.extend(track.children.iter().copied());
        }
        Ok(())
    }

    fn move_track(
        &mut self,
        id: TrackId,
        parent: Option<TrackId>,
        index: usize,
    ) -> Result<(), ModelError> {
        let kind = self.track_ref(id)?.kind;
        match kind {
            TrackKind::Main => return Err(ModelError::MainTrack),
            TrackKind::Return if parent.is_some() => return Err(ModelError::CannotGroup { kind }),
            _ => {}
        }
        self.check_can_hold(id, parent)?;
        let old_parent = self.parent(id);
        let old_list = self.containing_list_mut(kind, old_parent)?;
        let old_position =
            position_in(old_list, |t| *t == id).ok_or(ModelError::NoSuchTrack(id))?;
        if old_parent == parent && old_position == index {
            return Ok(());
        }
        old_list.remove(old_position);
        let new_list = if kind == TrackKind::Return {
            &mut self.returns
        } else {
            self.list_mut(parent)?
        };
        insert_at(new_list, index, id)?;
        match parent {
            Some(parent) => self.index.parent.insert(id, parent),
            None => self.index.parent.remove(&id),
        };
        Ok(())
    }

    fn create_clip(&mut self, track: TrackId, clip: &Clip) -> Result<(), ModelError> {
        self.check_fresh(clip.id.0)?;
        check_clip_fits(self.track_ref(track)?, &clip.data)?;
        check_extent(clip.start, clip.length)?;
        check_clip_data(&clip.data)?;
        self.track_mut(track)?
            .clips
            .insert(clip.id, Shared::new(clip.clone()));
        self.index.clip_track.insert(clip.id, track);
        Ok(())
    }

    fn move_clip(&mut self, id: ClipId, track: TrackId, start: Ticks) -> Result<(), ModelError> {
        let owner = self.clip_owner(id)?;
        let clip = self.clip_ref(id)?;
        check_clip_fits(self.track_ref(track)?, &clip.data)?;
        check_extent(start, clip.length)?;
        if owner == track {
            if clip.start != start {
                self.clip_mut(id)?.start = start;
            }
            return Ok(());
        }
        let mut moved = clip.clone();
        moved.start = start;
        self.track_mut(owner)?.clips.remove(&id);
        self.track_mut(track)?.clips.insert(id, Shared::new(moved));
        self.index.clip_track.insert(id, track);
        Ok(())
    }

    fn move_clips(&mut self, clips: &[ClipId], by: Ticks) -> Result<(), ModelError> {
        let mut seen = BTreeSet::new();
        let mut moves = Vec::with_capacity(clips.len());
        for &id in clips {
            if !seen.insert(id) {
                return Err(ModelError::DuplicateId(id.0));
            }
            let clip = self.clip_ref(id)?;
            let start = clip
                .start
                .checked_add(by)
                .ok_or(ModelError::PositionOverflow)?;
            check_extent(start, clip.length)?;
            moves.push((id, start));
        }
        if by != Ticks::ZERO {
            for (id, start) in moves {
                self.clip_mut(id)?.start = start;
            }
        }
        Ok(())
    }

    fn insert_device(
        &mut self,
        track: TrackId,
        index: usize,
        device: &Device,
    ) -> Result<(), ModelError> {
        self.check_fresh(device.id.0)?;
        check_device(device)?;
        let chain = &mut self.track_mut(track)?.devices;
        insert_at(chain, index, Shared::new(device.clone()))?;
        self.index.device_track.insert(device.id, track);
        Ok(())
    }

    fn move_device(
        &mut self,
        id: DeviceId,
        track: TrackId,
        index: usize,
    ) -> Result<(), ModelError> {
        let owner = self.device_owner(id)?;
        self.track_ref(track)?;
        let old_position = self
            .track_ref(owner)?
            .device_position(id)
            .ok_or(ModelError::NoSuchDevice(id))?;
        if owner == track && old_position == index {
            return Ok(());
        }
        let device = self.track_mut(owner)?.devices.remove(old_position);
        insert_at(&mut self.track_mut(track)?.devices, index, device)?;
        self.index.device_track.insert(id, track);
        Ok(())
    }

    // Lookups that fail with the error a command reports.

    pub(crate) fn track_ref(&self, id: TrackId) -> Result<&Track, ModelError> {
        self.track(id).ok_or(ModelError::NoSuchTrack(id))
    }

    fn track_mut(&mut self, id: TrackId) -> Result<&mut Track, ModelError> {
        self.tracks
            .get_mut(&id)
            .map(Shared::make_mut)
            .ok_or(ModelError::NoSuchTrack(id))
    }

    fn clip_owner(&self, id: ClipId) -> Result<TrackId, ModelError> {
        self.clip_track(id).ok_or(ModelError::NoSuchClip(id))
    }

    fn clip_ref(&self, id: ClipId) -> Result<&Clip, ModelError> {
        self.clip(id).ok_or(ModelError::NoSuchClip(id))
    }

    fn clip_mut(&mut self, id: ClipId) -> Result<&mut Clip, ModelError> {
        let owner = self.clip_owner(id)?;
        self.track_mut(owner)?
            .clips
            .get_mut(&id)
            .map(Shared::make_mut)
            .ok_or(ModelError::NoSuchClip(id))
    }

    fn device_owner(&self, id: DeviceId) -> Result<TrackId, ModelError> {
        self.device_track(id).ok_or(ModelError::NoSuchDevice(id))
    }

    fn device_ref(&self, id: DeviceId) -> Result<&Device, ModelError> {
        self.device(id).ok_or(ModelError::NoSuchDevice(id))
    }

    fn device_mut(&mut self, id: DeviceId) -> Result<&mut Device, ModelError> {
        let owner = self.device_owner(id)?;
        let chain = &mut self.track_mut(owner)?.devices;
        let position = position_in(chain, |d| d.id == id).ok_or(ModelError::NoSuchDevice(id))?;
        chain
            .get_mut(position)
            .map(Shared::make_mut)
            .ok_or(ModelError::NoSuchDevice(id))
    }

    /// The top-level list, or a group's list of tracks.
    fn list_mut(&mut self, parent: Option<TrackId>) -> Result<&mut Vector<TrackId>, ModelError> {
        match parent {
            None => Ok(&mut self.top),
            Some(parent) => Ok(&mut self.track_mut(parent)?.children),
        }
    }

    /// The list a track is in now.
    fn containing_list_mut(
        &mut self,
        kind: TrackKind,
        parent: Option<TrackId>,
    ) -> Result<&mut Vector<TrackId>, ModelError> {
        match kind {
            TrackKind::Main => Err(ModelError::MainTrack),
            TrackKind::Return => Ok(&mut self.returns),
            _ => self.list_mut(parent),
        }
    }

    /// Checks that `parent` (if any) is a group that `track` can go in: not
    /// the track itself, nor a group inside it.
    fn check_can_hold(&self, track: TrackId, parent: Option<TrackId>) -> Result<(), ModelError> {
        let Some(parent) = parent else {
            return Ok(());
        };
        check_kind(
            self.track_ref(parent)?,
            |kind| kind == TrackKind::Group,
            "a group track",
        )?;
        let mut ancestor = Some(parent);
        while let Some(group) = ancestor {
            if group == track {
                return Err(ModelError::GroupCycle { track, parent });
            }
            ancestor = self.parent(group);
        }
        Ok(())
    }

    fn check_fresh(&self, id: Id) -> Result<(), ModelError> {
        if self.contains(id) {
            Err(ModelError::IdInUse(id))
        } else {
            Ok(())
        }
    }
}

const NOT_MAIN: &str = "a track other than Main";
const AUDIO_OR_MIDI: &str = "an audio or MIDI track";

fn check_kind(
    track: &Track,
    allowed: impl Fn(TrackKind) -> bool,
    needs: &'static str,
) -> Result<(), ModelError> {
    if allowed(track.kind) {
        Ok(())
    } else {
        Err(ModelError::WrongTrackKind {
            track: track.id,
            kind: track.kind,
            needs,
        })
    }
}

fn check_value(what: &'static str, value: f64, ok: bool) -> Result<(), ModelError> {
    if ok {
        Ok(())
    } else {
        Err(ModelError::InvalidValue { what, value })
    }
}

/// Checks that a clip's data matches the track it's on.
pub(crate) fn check_clip_fits(track: &Track, data: &ClipData) -> Result<(), ModelError> {
    match (track.kind, data) {
        (TrackKind::Audio, ClipData::Audio(_)) | (TrackKind::Midi, ClipData::Midi(_)) => Ok(()),
        (kind, _) => Err(ModelError::WrongTrackKind {
            track: track.id,
            kind,
            needs: if data.is_audio() {
                "an audio track"
            } else {
                "a MIDI track"
            },
        }),
    }
}

/// Checks a clip or note's start and length.
pub(crate) fn check_extent(start: Ticks, length: Ticks) -> Result<(), ModelError> {
    if start.is_negative() {
        return Err(ModelError::NegativePosition(start));
    }
    if length <= Ticks::ZERO {
        return Err(ModelError::EmptyLength(length));
    }
    start
        .checked_add(length)
        .map(|_| ())
        .ok_or(ModelError::PositionOverflow)
}

/// Checks the notes in MIDI data.
pub(crate) fn check_clip_data(data: &ClipData) -> Result<(), ModelError> {
    if let ClipData::Midi(midi) = data {
        for (index, note) in midi.notes.iter().enumerate() {
            if note.key > 127 || !(1..=127).contains(&note.velocity) {
                return Err(ModelError::InvalidNote { index });
            }
            check_extent(note.start, note.length)?;
        }
    }
    Ok(())
}

/// Checks a device's parameter values.
pub(crate) fn check_device(device: &Device) -> Result<(), ModelError> {
    for value in device.params.values() {
        check_value("parameter value", *value, value.is_finite())?;
    }
    Ok(())
}

/// Checks mixer values against the track's kind.
pub(crate) fn check_mixer(track: &Track) -> Result<(), ModelError> {
    let mixer = &track.mixer;
    check_value(
        "volume",
        mixer.volume,
        mixer.volume.is_finite() && mixer.volume >= 0.0,
    )?;
    check_value("pan", mixer.pan, (-1.0..=1.0).contains(&mixer.pan))?;
    if mixer.mute || mixer.solo {
        check_kind(track, TrackKind::has_mute_and_solo, NOT_MAIN)?;
    }
    if mixer.arm {
        check_kind(track, TrackKind::has_arm, AUDIO_OR_MIDI)?;
    }
    Ok(())
}

fn insert_at<T: Clone>(list: &mut Vector<T>, index: usize, value: T) -> Result<(), ModelError> {
    if index > list.len() {
        return Err(ModelError::IndexOutOfRange {
            index,
            len: list.len(),
        });
    }
    list.insert(index, value);
    Ok(())
}

fn position_in<T: Clone>(list: &Vector<T>, matches: impl Fn(&T) -> bool) -> Option<usize> {
    list.iter().position(matches)
}

#[cfg(test)]
mod tests {
    use bl_core::id::IdAllocator;
    use bl_core::time::{Bpm, TempoMap, TimeSignature};

    use super::*;
    use crate::clip::{AudioData, MidiData, Note};
    use crate::device::{DeviceKind, PluginFormat};
    use crate::ids::ContentHash;

    /// A document and the allocator for its IDs.
    struct Song {
        doc: Document,
        ids: IdAllocator,
    }

    impl Song {
        fn new() -> Self {
            let mut ids = IdAllocator::new();
            let doc = Document::new(TrackId(ids.allocate()));
            Self { doc, ids }
        }

        fn main(&self) -> TrackId {
            self.doc.main_id()
        }

        fn track(&mut self, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
            let id = TrackId(self.ids.allocate());
            let index = match (kind, parent) {
                (TrackKind::Return, _) => self.doc.returns.len(),
                (_, None) => self.doc.top.len(),
                (_, Some(parent)) => self.doc.track(parent).unwrap().children.len(),
            };
            self.apply(Command::CreateTrack {
                id,
                kind,
                name: format!("{kind} {}", id.0),
                parent,
                index,
            })
            .unwrap();
            id
        }

        fn clip(&mut self, track: TrackId, start: i64) -> ClipId {
            let id = ClipId(self.ids.allocate());
            let data = match self.doc.track(track).unwrap().kind() {
                TrackKind::Audio => audio(),
                _ => midi(),
            };
            let clip = Clip::new(id, Ticks::QUARTER * start, Ticks::WHOLE, data);
            self.apply(Command::CreateClip { track, clip }).unwrap();
            id
        }

        fn device(&mut self, track: TrackId) -> DeviceId {
            let id = DeviceId(self.ids.allocate());
            let index = self.doc.track(track).unwrap().devices.len();
            self.apply(Command::InsertDevice {
                track,
                index,
                device: utility(id),
            })
            .unwrap();
            id
        }

        fn apply(&mut self, command: Command) -> Result<(), ModelError> {
            let result = self.doc.apply(&command);
            self.doc.check_invariants().unwrap();
            result
        }

        /// Applies a command that must fail, and checks it changed nothing.
        fn reject(&mut self, command: Command) -> ModelError {
            let before = self.doc.clone();
            let error = self.apply(command).unwrap_err();
            assert_eq!(self.doc, before);
            error
        }

        fn fresh<T: From<Id>>(&mut self) -> T {
            T::from(self.ids.allocate())
        }
    }

    fn audio() -> ClipData {
        ClipData::Audio(AudioData {
            media: ContentHash([7; 32]),
        })
    }

    fn midi() -> ClipData {
        ClipData::Midi(MidiData::new([Note {
            key: 60,
            velocity: 100,
            start: Ticks::ZERO,
            length: Ticks::QUARTER,
        }]))
    }

    fn utility(id: DeviceId) -> Device {
        Device::new(
            id,
            "Utility",
            DeviceKind::Native {
                device_type: "beatlayer.utility".to_owned(),
            },
        )
    }

    #[test]
    fn clips_only_on_matching_tracks() {
        let mut song = Song::new();
        let audio_track = song.track(TrackKind::Audio, None);
        let midi_track = song.track(TrackKind::Midi, None);
        let group = song.track(TrackKind::Group, None);
        let ret = song.track(TrackKind::Return, None);
        let main = song.main();

        for (track, data, ok) in [
            (audio_track, audio(), true),
            (audio_track, midi(), false),
            (midi_track, midi(), true),
            (midi_track, audio(), false),
            (group, audio(), false),
            (group, midi(), false),
            (ret, audio(), false),
            (main, midi(), false),
        ] {
            let clip = Clip::new(song.fresh(), Ticks::ZERO, Ticks::WHOLE, data);
            let command = Command::CreateClip { track, clip };
            if ok {
                song.apply(command).unwrap();
            } else {
                assert!(matches!(
                    song.reject(command),
                    ModelError::WrongTrackKind { .. }
                ));
            }
        }

        // Moving a clip or replacing its data follows the same rule.
        let clip = song.clip(audio_track, 0);
        song.reject(Command::MoveClip {
            clip,
            track: midi_track,
            start: Ticks::ZERO,
        });
        song.reject(Command::SetClipData { clip, data: midi() });
        let other_audio = song.track(TrackKind::Audio, None);
        song.apply(Command::MoveClip {
            clip,
            track: other_audio,
            start: Ticks::WHOLE,
        })
        .unwrap();
        assert_eq!(song.doc.clip_track(clip), Some(other_audio));
        assert_eq!(song.doc.clip(clip).unwrap().start, Ticks::WHOLE);
    }

    #[test]
    fn groups_nest() {
        let mut song = Song::new();
        let outer = song.track(TrackKind::Group, None);
        let inner = song.track(TrackKind::Group, Some(outer));
        let deepest = song.track(TrackKind::Group, Some(inner));
        let audio = song.track(TrackKind::Audio, Some(deepest));
        let midi = song.track(TrackKind::Midi, Some(inner));
        assert_eq!(song.doc.parent(audio), Some(deepest));
        assert_eq!(song.doc.parent(deepest), Some(inner));
        assert_eq!(song.doc.parent(inner), Some(outer));
        assert_eq!(
            song.doc.track_order(),
            vec![outer, inner, deepest, audio, midi, song.main()]
        );

        // Only groups hold tracks.
        let next_id = song.fresh();
        let error = song.reject(Command::CreateTrack {
            id: next_id,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: Some(audio),
            index: 0,
        });
        assert!(matches!(error, ModelError::WrongTrackKind { .. }));

        // Moving a track out to the top level and back in.
        song.apply(Command::MoveTrack {
            track: midi,
            parent: None,
            index: 0,
        })
        .unwrap();
        assert_eq!(song.doc.parent(midi), None);
        assert_eq!(song.doc.top_level().collect::<Vec<_>>(), vec![midi, outer]);
        song.apply(Command::MoveTrack {
            track: midi,
            parent: Some(deepest),
            index: 0,
        })
        .unwrap();
        assert_eq!(song.doc.parent(midi), Some(deepest));
        assert_eq!(
            song.doc
                .track(deepest)
                .unwrap()
                .children()
                .collect::<Vec<_>>(),
            vec![midi, audio]
        );
        // The index counts positions after the track is taken out.
        song.apply(Command::MoveTrack {
            track: midi,
            parent: Some(deepest),
            index: 1,
        })
        .unwrap();
        assert_eq!(
            song.doc
                .track(deepest)
                .unwrap()
                .children()
                .collect::<Vec<_>>(),
            vec![audio, midi]
        );
        let error = song.reject(Command::MoveTrack {
            track: midi,
            parent: Some(deepest),
            index: 2,
        });
        assert_eq!(error, ModelError::IndexOutOfRange { index: 2, len: 1 });
    }

    #[test]
    fn group_cannot_contain_itself() {
        let mut song = Song::new();
        let outer = song.track(TrackKind::Group, None);
        let inner = song.track(TrackKind::Group, Some(outer));
        for parent in [outer, inner] {
            let error = song.reject(Command::MoveTrack {
                track: outer,
                parent: Some(parent),
                index: 0,
            });
            assert_eq!(
                error,
                ModelError::GroupCycle {
                    track: outer,
                    parent
                }
            );
        }
    }

    #[test]
    fn returns_stay_out_of_groups() {
        let mut song = Song::new();
        let group = song.track(TrackKind::Group, None);
        let first = song.track(TrackKind::Return, None);
        let second = song.track(TrackKind::Return, None);

        let next_id = song.fresh();
        let error = song.reject(Command::CreateTrack {
            id: next_id,
            kind: TrackKind::Return,
            name: String::new(),
            parent: Some(group),
            index: 0,
        });
        assert_eq!(
            error,
            ModelError::CannotGroup {
                kind: TrackKind::Return
            }
        );
        song.reject(Command::MoveTrack {
            track: first,
            parent: Some(group),
            index: 0,
        });

        // Returns move among the returns.
        song.apply(Command::MoveTrack {
            track: second,
            parent: None,
            index: 0,
        })
        .unwrap();
        assert_eq!(song.doc.returns().collect::<Vec<_>>(), vec![second, first]);
        assert_eq!(song.doc.top_level().collect::<Vec<_>>(), vec![group]);
    }

    #[test]
    fn deleting_a_group_deletes_its_contents() {
        let mut song = Song::new();
        let keep = song.track(TrackKind::Audio, None);
        let kept_clip = song.clip(keep, 0);
        let group = song.track(TrackKind::Group, None);
        let group_device = song.device(group);
        let inner = song.track(TrackKind::Group, Some(group));
        let audio = song.track(TrackKind::Audio, Some(inner));
        let clip = song.clip(audio, 0);
        let device = song.device(audio);
        let midi = song.track(TrackKind::Midi, Some(group));
        let midi_clip = song.clip(midi, 4);

        song.apply(Command::DeleteTrack { track: group }).unwrap();
        for track in [group, inner, audio, midi] {
            assert!(song.doc.track(track).is_none());
            assert!(!song.doc.contains(track.0));
        }
        for clip in [clip, midi_clip] {
            assert!(song.doc.clip(clip).is_none());
        }
        for device in [device, group_device] {
            assert!(song.doc.device(device).is_none());
        }
        assert_eq!(song.doc.track_order(), vec![keep, song.main()]);
        assert!(song.doc.clip(kept_clip).is_some());
        assert_eq!(song.doc.clip_count(), 1);
        assert_eq!(song.doc.device_count(), 0);
    }

    #[test]
    fn main_is_fixed() {
        let mut song = Song::new();
        let main = song.main();
        let group = song.track(TrackKind::Group, None);
        let next_id = song.fresh();
        assert_eq!(
            song.reject(Command::CreateTrack {
                id: next_id,
                kind: TrackKind::Main,
                name: String::new(),
                parent: None,
                index: 0,
            }),
            ModelError::MainTrack
        );
        assert_eq!(
            song.reject(Command::DeleteTrack { track: main }),
            ModelError::MainTrack
        );
        for parent in [None, Some(group)] {
            assert_eq!(
                song.reject(Command::MoveTrack {
                    track: main,
                    parent,
                    index: 0,
                }),
                ModelError::MainTrack
            );
        }
        // Main can still be renamed and mixed.
        song.apply(Command::RenameTrack {
            track: main,
            name: "Master".to_owned(),
        })
        .unwrap();
        assert_eq!(song.doc.main().name(), "Master");
    }

    #[test]
    fn devices_on_every_track() {
        let mut song = Song::new();
        let tracks = [
            song.track(TrackKind::Audio, None),
            song.track(TrackKind::Midi, None),
            song.track(TrackKind::Group, None),
            song.track(TrackKind::Return, None),
            song.main(),
        ];
        for track in tracks {
            let device = song.device(track);
            assert_eq!(song.doc.device_track(device), Some(track));
            assert_eq!(song.doc.device(device).unwrap().name, "Utility");
        }
        assert_eq!(song.doc.device_count(), 5);
    }

    #[test]
    fn mixer_switches_by_kind() {
        let mut song = Song::new();
        let cases = [
            (song.track(TrackKind::Audio, None), true, true),
            (song.track(TrackKind::Midi, None), true, true),
            (song.track(TrackKind::Group, None), true, false),
            (song.track(TrackKind::Return, None), true, false),
            (song.main(), false, false),
        ];
        for (track, mute_and_solo, arm) in cases {
            for (command, allowed) in [
                (Command::SetVolume { track, volume: 0.5 }, true),
                (Command::SetPan { track, pan: -1.0 }, true),
                (Command::SetMute { track, mute: true }, mute_and_solo),
                (Command::SetSolo { track, solo: true }, mute_and_solo),
                (Command::SetArm { track, arm: true }, arm),
            ] {
                if allowed {
                    song.apply(command).unwrap();
                } else {
                    assert!(matches!(
                        song.reject(command),
                        ModelError::WrongTrackKind { .. }
                    ));
                }
            }
            let mixer = song.doc.track(track).unwrap().mixer();
            assert_eq!(mixer.volume, 0.5);
            assert_eq!(mixer.pan, -1.0);
            assert_eq!(mixer.mute, mute_and_solo);
            assert_eq!(mixer.solo, mute_and_solo);
            assert_eq!(mixer.arm, arm);
        }
    }

    #[test]
    fn values_out_of_range_are_rejected() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let device = song.device(track);
        for command in [
            Command::SetVolume {
                track,
                volume: -0.1,
            },
            Command::SetVolume {
                track,
                volume: f64::INFINITY,
            },
            Command::SetVolume {
                track,
                volume: f64::NAN,
            },
            Command::SetPan { track, pan: 1.01 },
            Command::SetPan {
                track,
                pan: f64::NAN,
            },
            Command::SetDeviceParam {
                device,
                param: ParamId(0),
                value: f64::NAN,
            },
        ] {
            assert!(matches!(
                song.reject(command),
                ModelError::InvalidValue { .. }
            ));
        }
        let mut bad = utility(song.fresh());
        bad.params.insert(ParamId(1), f64::NEG_INFINITY);
        song.reject(Command::InsertDevice {
            track,
            index: 0,
            device: bad,
        });
    }

    #[test]
    fn clip_extents_and_notes_are_checked() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let clip = song.clip(track, 0);
        let new_clip =
            |song: &mut Song, start: Ticks, length: Ticks, data: ClipData| Command::CreateClip {
                track,
                clip: Clip::new(song.fresh(), start, length, data),
            };
        let command = new_clip(&mut song, -Ticks::QUARTER, Ticks::WHOLE, midi());
        assert_eq!(
            song.reject(command),
            ModelError::NegativePosition(-Ticks::QUARTER)
        );
        let command = new_clip(&mut song, Ticks::ZERO, Ticks::ZERO, midi());
        assert_eq!(song.reject(command), ModelError::EmptyLength(Ticks::ZERO));
        let command = new_clip(&mut song, Ticks::MAX - Ticks::QUARTER, Ticks::WHOLE, midi());
        assert_eq!(song.reject(command), ModelError::PositionOverflow);
        assert_eq!(
            song.reject(Command::SetClipLength {
                clip,
                length: -Ticks::WHOLE,
            }),
            ModelError::EmptyLength(-Ticks::WHOLE)
        );

        let note = |key, velocity| Note {
            key,
            velocity,
            start: Ticks::ZERO,
            length: Ticks::QUARTER,
        };
        for (notes, index) in [
            (vec![note(60, 100), note(128, 100)], 1),
            (vec![note(60, 0)], 0),
            (vec![note(0, 128)], 0),
        ] {
            let data = ClipData::Midi(MidiData::new(notes));
            assert_eq!(
                song.reject(Command::SetClipData { clip, data }),
                ModelError::InvalidNote { index }
            );
        }
        let data = ClipData::Midi(MidiData::new([Note {
            length: Ticks::ZERO,
            ..note(60, 100)
        }]));
        assert_eq!(
            song.reject(Command::SetClipData { clip, data }),
            ModelError::EmptyLength(Ticks::ZERO)
        );
    }

    #[test]
    fn ids_are_unique_across_kinds() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.clip(track, 0);
        let device = song.device(track);
        // A clip can't take a track's ID, a device can't take a clip's, and
        // so on.
        assert_eq!(
            song.reject(Command::CreateClip {
                track,
                clip: Clip::new(ClipId(track.0), Ticks::ZERO, Ticks::WHOLE, audio()),
            }),
            ModelError::IdInUse(track.0)
        );
        assert_eq!(
            song.reject(Command::InsertDevice {
                track,
                index: 0,
                device: utility(DeviceId(clip.0)),
            }),
            ModelError::IdInUse(clip.0)
        );
        assert_eq!(
            song.reject(Command::CreateTrack {
                id: TrackId(device.0),
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            }),
            ModelError::IdInUse(device.0)
        );
    }

    #[test]
    fn move_clips_is_all_or_nothing() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let early = song.clip(track, 0);
        let late = song.clip(track, 8);
        // Moving both a bar earlier would put `early` before zero.
        assert_eq!(
            song.reject(Command::MoveClips {
                clips: vec![late, early],
                by: -Ticks::WHOLE,
            }),
            ModelError::NegativePosition(-Ticks::WHOLE)
        );
        assert_eq!(
            song.reject(Command::MoveClips {
                clips: vec![late, late],
                by: Ticks::WHOLE,
            }),
            ModelError::DuplicateId(late.0)
        );
        song.apply(Command::MoveClips {
            clips: vec![late, early],
            by: Ticks::QUARTER,
        })
        .unwrap();
        assert_eq!(song.doc.clip(early).unwrap().start, Ticks::QUARTER);
        assert_eq!(song.doc.clip(late).unwrap().start, Ticks::QUARTER * 9);
    }

    #[test]
    fn devices_move_between_chains() {
        let mut song = Song::new();
        let first = song.track(TrackKind::Audio, None);
        let second = song.track(TrackKind::Return, None);
        let a = song.device(first);
        let b = song.device(first);
        let c = song.device(first);
        let chain = |song: &Song, track| {
            song.doc
                .track(track)
                .unwrap()
                .devices()
                .map(|d| d.id)
                .collect::<Vec<_>>()
        };

        song.apply(Command::MoveDevice {
            device: a,
            track: first,
            index: 2,
        })
        .unwrap();
        assert_eq!(chain(&song, first), vec![b, c, a]);
        song.apply(Command::MoveDevice {
            device: c,
            track: second,
            index: 0,
        })
        .unwrap();
        assert_eq!(chain(&song, first), vec![b, a]);
        assert_eq!(chain(&song, second), vec![c]);
        assert_eq!(song.doc.device_track(c), Some(second));
        assert_eq!(
            song.reject(Command::MoveDevice {
                device: b,
                track: second,
                index: 2,
            }),
            ModelError::IndexOutOfRange { index: 2, len: 1 }
        );

        song.apply(Command::SetDeviceEnabled {
            device: b,
            enabled: false,
        })
        .unwrap();
        song.apply(Command::SetDeviceParam {
            device: b,
            param: ParamId(3),
            value: -12.5,
        })
        .unwrap();
        song.apply(Command::RenameDevice {
            device: b,
            name: "Trim".to_owned(),
        })
        .unwrap();
        let device = song.doc.device(b).unwrap();
        assert!(!device.enabled);
        assert_eq!(device.params.get(&ParamId(3)), Some(&-12.5));
        assert_eq!(device.name, "Trim");

        song.apply(Command::RemoveDevice { device: b }).unwrap();
        assert_eq!(chain(&song, first), vec![a]);
        assert_eq!(
            song.reject(Command::RemoveDevice { device: b }),
            ModelError::NoSuchDevice(b)
        );
    }

    #[test]
    fn missing_objects_are_reported() {
        let mut song = Song::new();
        let track: TrackId = song.fresh();
        let clip: ClipId = song.fresh();
        let device: DeviceId = song.fresh();
        assert_eq!(
            song.reject(Command::DeleteTrack { track }),
            ModelError::NoSuchTrack(track)
        );
        assert_eq!(
            song.reject(Command::DeleteClip { clip }),
            ModelError::NoSuchClip(clip)
        );
        assert_eq!(
            song.reject(Command::SetDeviceEnabled {
                device,
                enabled: false
            }),
            ModelError::NoSuchDevice(device)
        );
        let next_id = song.fresh();
        assert_eq!(
            song.reject(Command::CreateTrack {
                id: next_id,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: Some(track),
                index: 0,
            }),
            ModelError::NoSuchTrack(track)
        );
    }

    #[test]
    fn settings_change() {
        let mut song = Song::new();
        let tempo = TempoMap::constant(Bpm::new(96.0).unwrap());
        let signatures = TimeSignatureMap::constant(TimeSignature::new(7, 8).unwrap());
        song.apply(Command::SetTempo {
            tempo: tempo.clone(),
        })
        .unwrap();
        song.apply(Command::SetTimeSignatures {
            time_signatures: signatures.clone(),
        })
        .unwrap();
        assert_eq!(song.doc.settings().tempo, tempo);
        assert_eq!(song.doc.settings().time_signatures, signatures);
    }

    #[test]
    fn commands_serialise() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let commands = [
            Command::CreateTrack {
                id: song.fresh(),
                kind: TrackKind::Group,
                name: "Drums".to_owned(),
                parent: None,
                index: 0,
            },
            Command::CreateClip {
                track,
                clip: Clip::new(song.fresh(), Ticks::WHOLE, Ticks::WHOLE, midi()),
            },
            Command::InsertDevice {
                track,
                index: 0,
                device: Device {
                    params: [(ParamId(2), 0.25)].into_iter().collect(),
                    ..Device::new(
                        song.fresh(),
                        "Synth",
                        DeviceKind::Plugin {
                            format: PluginFormat::Clap,
                            plugin_id: "org.example.synth".to_owned(),
                        },
                    )
                },
            },
            Command::MoveClips {
                clips: vec![song.fresh()],
                by: -Ticks::QUARTER,
            },
            Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(140.0).unwrap()),
            },
            Command::SetClipData {
                clip: song.fresh(),
                data: audio(),
            },
        ];
        for command in commands {
            let json = serde_json::to_string(&command).unwrap();
            assert!(json.starts_with("{\"op\":"), "{json}");
            let back: Command = serde_json::from_str(&json).unwrap();
            assert_eq!(back, command, "{json}");
        }
        let json = serde_json::to_string(&Command::DeleteTrack { track }).unwrap();
        assert_eq!(
            json,
            format!("{{\"op\":\"delete_track\",\"track\":{}}}", track.0.get())
        );
    }

    #[test]
    fn every_command_has_a_label() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        assert_eq!(Command::DeleteTrack { track }.label(), "Delete track");
        let id = song.fresh();
        assert_eq!(
            Command::CreateClip {
                track,
                clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE, audio()),
            }
            .created_id(),
            Some(id.0)
        );
        assert_eq!(Command::DeleteTrack { track }.created_id(), None);
    }
}
