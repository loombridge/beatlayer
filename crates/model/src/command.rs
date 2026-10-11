//! Commands: the only way to change a document.

use std::collections::BTreeSet;

use bl_core::id::Id;
use bl_core::time::{TempoMap, Ticks, TimeSignatureMap};
use imbl::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::{AudioData, Clip, ClipData};
use crate::device::Device;
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{ClipId, DeviceId, ParamId, TrackId};
use crate::shared::Shared;
use crate::track::{Mixer, SendTap, Track, TrackKind};

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
    /// Moves the Arrangement loop brace, leaving the Loop switch as it is.
    SetLoop {
        /// Where the loop starts: 0 (1.1.1) or later.
        start: Ticks,
        /// How long it is: more than 0.
        length: Ticks,
    },
    /// Switches the Arrangement loop on or off, leaving the brace where it
    /// is.
    SetLoopEnabled {
        /// Whether playback loops between the brace's start and end.
        enabled: bool,
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
    /// Solos a track other than Main and unsolos every other track, as one
    /// edit.
    SoloExclusive {
        /// The track.
        track: TrackId,
    },
    /// Arms an audio or MIDI track and disarms every other track, as one
    /// edit.
    ArmExclusive {
        /// The track.
        track: TrackId,
    },
    /// Sets the level of an audio, MIDI, or group track's send to a return
    /// track, as linear gain from 0 (−∞ dB) to 1 (0 dB).
    SetSend {
        /// The sending track.
        track: TrackId,
        /// The return track the send feeds.
        to: TrackId,
        /// The level: 1 is 0 dB, 0.5 about −6 dB, and 0 sends nothing.
        #[schemars(range(min = 0.0, max = 1.0))]
        level: f64,
    },
    /// Sets where a return track's sends take their signal from the tracks
    /// that send to it: after their mixer (`post`) or before it (`pre`).
    SetSendTap {
        /// The return track.
        track: TrackId,
        /// `pre` or `post`.
        tap: SendTap,
    },
    /// Switches Solo in Place on or off.
    SetSoloInPlace {
        /// Whether soloing a track keeps the return tracks audible.
        enabled: bool,
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
    /// Moves a clip's left edge to `start`, keeping its end and where its
    /// contents play: the clip shows more or less of its contents. An audio
    /// clip's Start marker moves by the time the edge moved, as Live's does,
    /// and can't go before the start of its file; a MIDI clip's offset
    /// moves by as many ticks, and its contents can't start after it.
    TrimClipStart {
        /// The clip.
        clip: ClipId,
        /// Where its left edge goes, before its end.
        start: Ticks,
    },
    /// Moves an audio clip's Start and End markers: where in its file it
    /// starts playing, and where it stops while its loop is off.
    SetClipRegion {
        /// The audio clip.
        clip: ClipId,
        /// The Start marker, in seconds into the file: 0 or more. While the
        /// loop is on, it must be before the loop brace's end.
        #[schemars(range(min = 0.0))]
        start: f64,
        /// The End marker, in seconds into the file, after `start`; `null`
        /// (or nothing) for the end of the file.
        #[serde(default)]
        end: Option<f64>,
    },
    /// Moves an audio clip's loop brace, leaving its Loop switch as it is.
    SetClipLoop {
        /// The audio clip.
        clip: ClipId,
        /// Where the brace starts, in seconds into the file: 0 or more.
        #[schemars(range(min = 0.0))]
        start: f64,
        /// Where it ends, in seconds into the file, after `start`; `null`
        /// (or nothing) for the end of the file.
        #[serde(default)]
        end: Option<f64>,
    },
    /// Switches an audio clip's loop on or off, leaving its brace where it
    /// is.
    SetClipLoopEnabled {
        /// The audio clip.
        clip: ClipId,
        /// Whether the clip repeats its loop brace.
        enabled: bool,
    },
    /// Sets an audio clip's gain, as linear gain (finite, at least 0).
    SetClipGain {
        /// The audio clip.
        clip: ClipId,
        /// The gain: 1 is 0 dB, 0.5 about −6 dB, and 0 silence.
        #[schemars(range(min = 0.0))]
        gain: f64,
    },
    /// Sets the lengths of an audio clip's fade-in and fade-out.
    SetClipFades {
        /// The audio clip.
        clip: ClipId,
        /// The fade-in's length in seconds from the clip's start: 0 for none.
        #[schemars(range(min = 0.0))]
        fade_in: f64,
        /// The fade-out's length in seconds up to the clip's end: 0 for none.
        #[schemars(range(min = 0.0))]
        fade_out: f64,
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
            Self::SetLoop { .. } => "Change loop",
            Self::SetLoopEnabled { .. } => "Switch loop",
            Self::CreateTrack { .. } => "Create track",
            Self::DeleteTrack { .. } => "Delete track",
            Self::MoveTrack { .. } => "Move track",
            Self::RenameTrack { .. } => "Rename track",
            Self::SetVolume { .. } => "Change volume",
            Self::SetPan { .. } => "Change pan",
            Self::SetMute { .. } => "Mute track",
            Self::SetSolo { .. } => "Solo track",
            Self::SetArm { .. } => "Arm track",
            Self::SoloExclusive { .. } => "Solo track",
            Self::ArmExclusive { .. } => "Arm track",
            Self::SetSend { .. } => "Change send",
            Self::SetSendTap { .. } => "Switch Pre/Post",
            Self::SetSoloInPlace { .. } => "Switch Solo in Place",
            Self::CreateClip { .. } => "Create clip",
            Self::DeleteClip { .. } => "Delete clip",
            Self::MoveClip { .. } => "Move clip",
            Self::MoveClips { .. } => "Move clips",
            Self::SetClipLength { .. } => "Resize clip",
            Self::RenameClip { .. } => "Rename clip",
            Self::SetClipMute { .. } => "Mute clip",
            Self::SetClipData { .. } => "Edit clip",
            Self::TrimClipStart { .. } => "Resize clip",
            Self::SetClipRegion { .. } => "Change clip region",
            Self::SetClipLoop { .. } => "Change clip loop",
            Self::SetClipLoopEnabled { .. } => "Switch clip loop",
            Self::SetClipGain { .. } => "Change clip gain",
            Self::SetClipFades { .. } => "Change clip fades",
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
            Command::SetLoop { start, length } => {
                check_extent(*start, *length)?;
                let brace = &mut self.settings.arrangement_loop;
                brace.start = *start;
                brace.length = *length;
                Ok(())
            }
            Command::SetLoopEnabled { enabled } => {
                self.settings.arrangement_loop.enabled = *enabled;
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
            Command::SoloExclusive { track } => {
                check_kind(
                    self.track_ref(*track)?,
                    TrackKind::has_mute_and_solo,
                    NOT_MAIN,
                )?;
                self.set_switch_exclusively(*track, |mixer| &mut mixer.solo);
                Ok(())
            }
            Command::ArmExclusive { track } => {
                check_kind(self.track_ref(*track)?, TrackKind::has_arm, AUDIO_OR_MIDI)?;
                self.set_switch_exclusively(*track, |mixer| &mut mixer.arm);
                Ok(())
            }
            Command::SetSend { track, to, level } => {
                check_value("send level", *level, (0.0..=1.0).contains(level))?;
                let sender = self.track_ref(*track)?;
                check_kind(sender, TrackKind::has_sends, SENDER)?;
                check_kind(
                    self.track_ref(*to)?,
                    |kind| kind == TrackKind::Return,
                    RETURN,
                )?;
                if sender.send(*to) != *level {
                    let sends = &mut self.track_mut(*track)?.sends;
                    if *level == 0.0 {
                        sends.remove(to);
                    } else {
                        sends.insert(*to, *level);
                    }
                }
                Ok(())
            }
            Command::SetSendTap { track, tap } => {
                let current = self.track_ref(*track)?;
                check_kind(current, |kind| kind == TrackKind::Return, RETURN)?;
                if current.mixer.tap != *tap {
                    self.track_mut(*track)?.mixer.tap = *tap;
                }
                Ok(())
            }
            Command::SetSoloInPlace { enabled } => {
                self.settings.solo_in_place = *enabled;
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
            Command::TrimClipStart { clip, start } => self.trim_clip_start(*clip, *start),
            Command::SetClipRegion { clip, start, end } => self.edit_markers(*clip, |audio| {
                audio.start = *start;
                audio.end = *end;
            }),
            Command::SetClipLoop { clip, start, end } => self.edit_markers(*clip, |audio| {
                audio.loop_start = *start;
                audio.loop_end = *end;
            }),
            Command::SetClipLoopEnabled { clip, enabled } => {
                self.edit_markers(*clip, |audio| audio.looping = *enabled)
            }
            Command::SetClipGain { clip, gain } => {
                self.edit_audio(*clip, |audio| audio.gain = *gain)
            }
            Command::SetClipFades {
                clip,
                fade_in,
                fade_out,
            } => self.edit_audio(*clip, |audio| {
                audio.fade_in = *fade_in;
                audio.fade_out = *fade_out;
            }),
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
        if kind == TrackKind::Return {
            // Every send to the return goes with it.
            let senders: Vec<TrackId> = self
                .tracks
                .iter()
                .filter(|(_, track)| track.sends.contains_key(&id))
                .map(|(&sender, _)| sender)
                .collect();
            for sender in senders {
                self.track_mut(sender)?.sends.remove(&id);
            }
        }
        Ok(())
    }

    /// Turns a mixer switch (solo or arm) on for `track` and off on every
    /// other track, touching only the tracks whose switch changes.
    fn set_switch_exclusively(&mut self, track: TrackId, switch: fn(&mut Mixer) -> &mut bool) {
        let changes: Vec<(TrackId, bool)> = self
            .tracks
            .iter()
            .filter_map(|(&id, current)| {
                let wanted = id == track;
                let mut mixer = current.mixer;
                (*switch(&mut mixer) != wanted).then_some((id, wanted))
            })
            .collect();
        for (id, wanted) in changes {
            if let Some(current) = self.tracks.get_mut(&id) {
                *switch(&mut Shared::make_mut(current).mixer) = wanted;
            }
        }
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
        check_clip(clip)?;
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

    fn trim_clip_start(&mut self, id: ClipId, start: Ticks) -> Result<(), ModelError> {
        let clip = self.clip_ref(id)?;
        if clip.start == start {
            return Ok(());
        }
        let moved = clip.with_start(start, &self.settings.tempo)?;
        check_extent(moved.start, moved.length)?;
        check_clip(&moved)?;
        *self.clip_mut(id)? = moved;
        Ok(())
    }

    /// Sets an audio clip's markers or loop with `edit`, if the result is
    /// valid and its markers are in order, as Live keeps markers set by hand
    /// (the audio clips spec, R3 and R4).
    fn edit_markers(
        &mut self,
        id: ClipId,
        edit: impl FnOnce(&mut AudioData),
    ) -> Result<(), ModelError> {
        let ClipData::Audio(current) = self.clip_ref(id)?.data else {
            return Err(ModelError::NotAudioClip(id));
        };
        let mut next = current;
        edit(&mut next);
        check_marker_order(&next)?;
        self.edit_audio(id, |audio| *audio = next)
    }

    /// Changes an audio clip's data with `edit`, if the result is valid.
    fn edit_audio(
        &mut self,
        id: ClipId,
        edit: impl FnOnce(&mut AudioData),
    ) -> Result<(), ModelError> {
        let ClipData::Audio(current) = self.clip_ref(id)?.data else {
            return Err(ModelError::NotAudioClip(id));
        };
        let mut next = current;
        edit(&mut next);
        check_audio(&next)?;
        if next != current {
            self.clip_mut(id)?.data = ClipData::Audio(next);
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
const SENDER: &str = "an audio, MIDI, or group track";
const RETURN: &str = "a return track";

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

/// Checks a clip's offset into its contents.
pub(crate) fn check_offset(offset: Ticks) -> Result<(), ModelError> {
    if offset.is_negative() {
        Err(ModelError::NegativeOffset(offset))
    } else {
        Ok(())
    }
}

/// Checks the notes in MIDI data, and an audio clip's markers, loop, gain,
/// and fades.
pub(crate) fn check_clip_data(data: &ClipData) -> Result<(), ModelError> {
    match data {
        ClipData::Midi(midi) => {
            for (index, note) in midi.notes.iter().enumerate() {
                if note.key > 127 || !(1..=127).contains(&note.velocity) {
                    return Err(ModelError::InvalidNote { index });
                }
                check_extent(note.start, note.length)?;
            }
            Ok(())
        }
        ClipData::Audio(audio) => check_audio(audio),
    }
}

/// Checks an audio clip's data (the audio clips spec, R3 to R6): times
/// finite and not negative, each end after its start, the Start marker
/// before the loop's end while the loop is on, and the gain and fades
/// finite and not negative.
pub(crate) fn check_audio(audio: &AudioData) -> Result<(), ModelError> {
    let at_least_zero = |value: f64| value.is_finite() && value >= 0.0;
    check_value("clip start", audio.start, at_least_zero(audio.start))?;
    if let Some(end) = audio.end {
        check_value("clip end", end, end.is_finite() && end > 0.0)?;
    }
    check_value(
        "loop start",
        audio.loop_start,
        at_least_zero(audio.loop_start),
    )?;
    if let Some(end) = audio.loop_end {
        check_value("loop end", end, end.is_finite() && end > audio.loop_start)?;
    }
    check_value("clip gain", audio.gain, at_least_zero(audio.gain))?;
    check_value(
        "fade-in length",
        audio.fade_in,
        at_least_zero(audio.fade_in),
    )?;
    check_value(
        "fade-out length",
        audio.fade_out,
        at_least_zero(audio.fade_out),
    )
}

/// Checks that an audio clip's markers are in order, as setting them by hand
/// keeps them: the End marker after the Start marker, and, while the loop is
/// on, the Start marker before the loop brace's end. Trimming a clip's left
/// edge or splitting it can move the Start marker past either (the audio
/// clips spec, R16), where the clip plays as its markers say.
pub(crate) fn check_marker_order(audio: &AudioData) -> Result<(), ModelError> {
    if let Some(end) = audio.end {
        check_value("clip end", end, end > audio.start)?;
    }
    if let (true, Some(end)) = (audio.looping, audio.loop_end) {
        check_value(
            "clip start (at or after the loop end)",
            audio.start,
            audio.start < end,
        )?;
    }
    Ok(())
}

/// Checks a clip's offset and data: a MIDI clip's offset can't be negative,
/// and an audio clip's is 0, since its Start marker says where it starts
/// playing (the audio clips spec, R16).
pub(crate) fn check_clip(clip: &Clip) -> Result<(), ModelError> {
    check_offset(clip.offset)?;
    if clip.data.is_audio() && clip.offset != Ticks::ZERO {
        return Err(ModelError::AudioClipOffset(clip.offset));
    }
    check_clip_data(&clip.data)
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
    if mixer.tap != SendTap::Post {
        check_kind(track, |kind| kind == TrackKind::Return, RETURN)?;
    }
    Ok(())
}

/// Checks a send's level: above 0 (a send at 0 isn't stored) and at most 1.
pub(crate) fn check_send_level(level: f64) -> Result<(), ModelError> {
    check_value("send level", level, level > 0.0 && level <= 1.0)
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
    use crate::clip::{EDGE_FADE_SECONDS as EDGE_FADE, MidiData, Note};
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
        ClipData::Audio(AudioData::new(ContentHash([7; 32])))
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

    // Spec: mixer R7, R8
    #[test]
    fn audio_midi_and_group_tracks_send_to_returns() {
        let mut song = Song::new();
        let audio = song.track(TrackKind::Audio, None);
        let group = song.track(TrackKind::Group, None);
        let midi = song.track(TrackKind::Midi, Some(group));
        let reverb = song.track(TrackKind::Return, None);
        let delay = song.track(TrackKind::Return, None);
        let main = song.main();

        // Every send starts at -inf dB.
        for track in [audio, group, midi] {
            assert_eq!(song.doc.track(track).unwrap().send(reverb), 0.0);
            assert_eq!(song.doc.track(track).unwrap().sends().len(), 0);
        }
        for (track, to, level) in [
            (audio, reverb, 1.0),
            (group, delay, 0.25),
            (midi, reverb, 0.5),
        ] {
            song.apply(Command::SetSend { track, to, level }).unwrap();
        }
        let sends: Vec<_> = song.doc.track(audio).unwrap().sends().collect();
        assert_eq!(sends, vec![(reverb, 1.0)]);
        // Turning a send back down to -inf dB forgets it.
        song.apply(Command::SetSend {
            track: audio,
            to: reverb,
            level: 0.0,
        })
        .unwrap();
        assert_eq!(song.doc.track(audio).unwrap().sends().len(), 0);

        // Only audio, MIDI, and group tracks send, only to returns, and only
        // between -inf and 0 dB.
        for (track, to, level) in [
            (reverb, delay, 0.5),
            (main, reverb, 0.5),
            (audio, group, 0.5),
        ] {
            assert!(matches!(
                song.reject(Command::SetSend { track, to, level }),
                ModelError::WrongTrackKind { .. }
            ));
        }
        for level in [1.01, -0.1, f64::NAN] {
            assert!(matches!(
                song.reject(Command::SetSend {
                    track: audio,
                    to: reverb,
                    level,
                }),
                ModelError::InvalidValue { .. }
            ));
        }

        // The Pre/Post switch is a return's.
        song.apply(Command::SetSendTap {
            track: delay,
            tap: SendTap::Pre,
        })
        .unwrap();
        assert_eq!(song.doc.track(delay).unwrap().mixer().tap, SendTap::Pre);
        for track in [audio, group, main] {
            assert!(matches!(
                song.reject(Command::SetSendTap {
                    track,
                    tap: SendTap::Pre,
                }),
                ModelError::WrongTrackKind { .. }
            ));
        }

        // Deleting a return deletes the sends to it.
        song.apply(Command::DeleteTrack { track: delay }).unwrap();
        assert_eq!(song.doc.track(group).unwrap().sends().len(), 0);
        assert_eq!(song.doc.track(midi).unwrap().send(reverb), 0.5);
    }

    // Spec: mixer R15
    #[test]
    fn exclusive_solo_and_arm_release_every_other_track() {
        let mut song = Song::new();
        let a = song.track(TrackKind::Audio, None);
        let b = song.track(TrackKind::Midi, None);
        let group = song.track(TrackKind::Group, None);
        let ret = song.track(TrackKind::Return, None);
        for track in [a, b, group, ret] {
            song.apply(Command::SetSolo { track, solo: true }).unwrap();
        }
        song.apply(Command::SetArm {
            track: a,
            arm: true,
        })
        .unwrap();
        song.apply(Command::SetArm {
            track: b,
            arm: true,
        })
        .unwrap();

        song.apply(Command::SoloExclusive { track: group }).unwrap();
        let soloed: Vec<_> = song
            .doc
            .tracks()
            .filter(|t| t.mixer().solo)
            .map(Track::id)
            .collect();
        assert_eq!(soloed, vec![group]);
        song.apply(Command::ArmExclusive { track: b }).unwrap();
        let armed: Vec<_> = song
            .doc
            .tracks()
            .filter(|t| t.mixer().arm)
            .map(Track::id)
            .collect();
        assert_eq!(armed, vec![b]);

        // Main can't be soloed, and only audio and MIDI tracks armed.
        let main = song.main();
        assert!(matches!(
            song.reject(Command::SoloExclusive { track: main }),
            ModelError::WrongTrackKind { .. }
        ));
        assert!(matches!(
            song.reject(Command::ArmExclusive { track: ret }),
            ModelError::WrongTrackKind { .. }
        ));
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

    // Spec: audio-clips R3, R4, R5, R6, R15
    #[test]
    fn audio_clip_settings_change_one_at_a_time_and_are_checked() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.clip(track, 0);
        let audio = |song: &Song| match song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio,
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        // A new clip plays the whole file once, at 0 dB, with no fades.
        assert_eq!(audio(&song), AudioData::new(ContentHash([7; 32])));

        song.apply(Command::SetClipRegion {
            clip,
            start: 0.5,
            end: Some(2.0),
        })
        .unwrap();
        song.apply(Command::SetClipLoop {
            clip,
            start: 1.0,
            end: Some(1.5),
        })
        .unwrap();
        song.apply(Command::SetClipLoopEnabled {
            clip,
            enabled: true,
        })
        .unwrap();
        song.apply(Command::SetClipGain { clip, gain: 0.5 })
            .unwrap();
        song.apply(Command::SetClipFades {
            clip,
            fade_in: 0.25,
            fade_out: EDGE_FADE,
        })
        .unwrap();
        let set = audio(&song);
        assert_eq!(
            (
                set.start,
                set.end,
                set.looping,
                set.loop_start,
                set.loop_end
            ),
            (0.5, Some(2.0), true, 1.0, Some(1.5))
        );
        assert_eq!(
            (set.gain, set.fade_in, set.fade_out),
            (0.5, 0.25, EDGE_FADE)
        );

        // Out of range, each is refused and changes nothing.
        for command in [
            Command::SetClipRegion {
                clip,
                start: -0.1,
                end: None,
            },
            Command::SetClipRegion {
                clip,
                start: 1.0,
                end: Some(1.0),
            },
            // The Start marker must stay before the loop's end while the
            // loop is on.
            Command::SetClipRegion {
                clip,
                start: 1.5,
                end: None,
            },
            Command::SetClipLoop {
                clip,
                start: 0.25,
                end: Some(0.5),
            },
            Command::SetClipLoop {
                clip,
                start: f64::NAN,
                end: None,
            },
            Command::SetClipGain {
                clip,
                gain: f64::INFINITY,
            },
            Command::SetClipGain { clip, gain: -0.5 },
            Command::SetClipFades {
                clip,
                fade_in: -1.0,
                fade_out: 0.0,
            },
            Command::SetClipFades {
                clip,
                fade_in: 0.0,
                fade_out: f64::NAN,
            },
        ] {
            assert!(
                matches!(song.reject(command), ModelError::InvalidValue { .. }),
                "{:?}",
                audio(&song)
            );
        }
        // With the loop off, the Start marker may sit past the brace.
        song.apply(Command::SetClipLoopEnabled {
            clip,
            enabled: false,
        })
        .unwrap();
        song.apply(Command::SetClipRegion {
            clip,
            start: 1.75,
            end: None,
        })
        .unwrap();
        // And then the loop can't come on.
        assert!(matches!(
            song.reject(Command::SetClipLoopEnabled {
                clip,
                enabled: true,
            }),
            ModelError::InvalidValue { .. }
        ));

        // A MIDI clip has none of these settings.
        let midi_track = song.track(TrackKind::Midi, None);
        let midi_clip = song.clip(midi_track, 0);
        assert_eq!(
            song.reject(Command::SetClipGain {
                clip: midi_clip,
                gain: 0.5,
            }),
            ModelError::NotAudioClip(midi_clip)
        );
        // Nor can whole data sneak past the checks.
        let mut bad = AudioData::new(ContentHash([7; 32]));
        bad.fade_in = f64::NAN;
        assert!(matches!(
            song.reject(Command::SetClipData {
                clip,
                data: ClipData::Audio(bad),
            }),
            ModelError::InvalidValue { .. }
        ));
    }

    // Spec: audio-clips R7
    #[test]
    fn create_fades_on_clip_edges_gives_new_clips_4_ms_fades() {
        assert_eq!(EDGE_FADE, 0.004);
        let plain = AudioData::new(ContentHash([1; 32]));
        assert_eq!((plain.fade_in, plain.fade_out), (0.0, 0.0));
        let faded = plain.with_edge_fades();
        assert_eq!((faded.fade_in, faded.fade_out), (EDGE_FADE, EDGE_FADE));
        assert_eq!(
            AudioData {
                fade_in: 0.0,
                fade_out: 0.0,
                ..faded
            },
            plain
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
    fn trimming_a_clip_start_keeps_its_contents_in_place() {
        // Spec: arrangement-editing R7, R16.
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let clip = song.clip(track, 4);
        let content = song.doc.clip(clip).unwrap().content_start();
        assert_eq!(content, Ticks::WHOLE);

        // Later: the clip shows less, its end and contents stay put.
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::QUARTER * 6,
        })
        .unwrap();
        let trimmed = song.doc.clip(clip).unwrap();
        assert_eq!(trimmed.start, Ticks::QUARTER * 6);
        assert_eq!(trimmed.end(), Ticks::WHOLE * 2);
        assert_eq!(trimmed.offset, Ticks::QUARTER * 2);
        assert_eq!(trimmed.content_start(), content);

        // Earlier again, back to where its contents begin, but no further.
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::WHOLE,
        })
        .unwrap();
        assert_eq!(song.doc.clip(clip).unwrap().offset, Ticks::ZERO);
        assert_eq!(
            song.reject(Command::TrimClipStart {
                clip,
                start: Ticks::QUARTER * 3,
            }),
            ModelError::NegativeOffset(-Ticks::QUARTER)
        );
        // Not at or past its end.
        assert_eq!(
            song.reject(Command::TrimClipStart {
                clip,
                start: Ticks::WHOLE * 2,
            }),
            ModelError::EmptyLength(Ticks::ZERO)
        );
        // A new clip can't start before its contents either.
        let mut early = Clip::new(song.fresh(), Ticks::ZERO, Ticks::WHOLE, midi());
        early.offset = -Ticks::QUARTER;
        assert_eq!(
            song.reject(Command::CreateClip { track, clip: early }),
            ModelError::NegativeOffset(-Ticks::QUARTER)
        );
    }

    // Spec: audio-clips R16; arrangement-editing R7, R16
    #[test]
    fn trimming_an_audio_clips_left_edge_moves_its_start_marker() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.clip(track, 4);
        let audio = |song: &Song| match song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio,
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        // At 120 BPM a quarter note is half a second: trimming two quarters
        // off the start moves the Start marker a second into the file, and
        // the offset stays 0.
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::QUARTER * 6,
        })
        .unwrap();
        let trimmed = song.doc.clip(clip).unwrap();
        assert_eq!(
            (trimmed.start, trimmed.end()),
            (Ticks::QUARTER * 6, Ticks::WHOLE * 2)
        );
        assert_eq!(trimmed.offset, Ticks::ZERO);
        assert_eq!(audio(&song).start, 1.0);

        // Under a slower tempo, the same ticks are more seconds.
        song.apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(60.0).unwrap()),
        })
        .unwrap();
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::QUARTER * 5,
        })
        .unwrap();
        assert_eq!(audio(&song).start, 0.0);
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::QUARTER * 7,
        })
        .unwrap();
        assert_eq!(audio(&song).start, 2.0);
        // Back to the file's start, but no further.
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::QUARTER * 5,
        })
        .unwrap();
        assert_eq!(audio(&song).start, 0.0);
        assert_eq!(
            song.reject(Command::TrimClipStart {
                clip,
                start: Ticks::QUARTER * 4,
            }),
            ModelError::BeforeFileStart(1.0)
        );

        // Past the End marker the clip plays silence there, and past the
        // loop's end a looped clip plays on into its repetitions: setting
        // the markers by hand keeps them in order, but trimming may not.
        song.apply(Command::SetClipRegion {
            clip,
            start: 0.0,
            end: Some(1.5),
        })
        .unwrap();
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::QUARTER * 7,
        })
        .unwrap();
        assert_eq!((audio(&song).start, audio(&song).end), (2.0, Some(1.5)));
        song.apply(Command::SetClipGain { clip, gain: 0.5 })
            .unwrap();
        assert!(matches!(
            song.reject(Command::SetClipRegion {
                clip,
                start: 2.0,
                end: Some(1.5),
            }),
            ModelError::InvalidValue { .. }
        ));

        // An audio clip never has an offset.
        let mut offset = Clip::new(
            song.fresh(),
            Ticks::WHOLE * 4,
            Ticks::WHOLE,
            super::tests::audio(),
        );
        offset.offset = Ticks::QUARTER;
        assert_eq!(
            song.reject(Command::CreateClip {
                track,
                clip: offset,
            }),
            ModelError::AudioClipOffset(Ticks::QUARTER)
        );
    }

    // Spec: audio-clips R16; arrangement-editing R8, R16
    #[test]
    fn a_part_of_a_clip_plays_on_from_where_the_clip_was() {
        let tempo = TempoMap::from_breakpoints([
            (Ticks::ZERO, Bpm::new(120.0).unwrap()),
            (Ticks::WHOLE * 2, Bpm::new(90.0).unwrap()),
        ])
        .unwrap();
        let audio = AudioData {
            start: 0.25,
            ..AudioData::new(ContentHash([7; 32]))
        };
        let whole = Clip::new(
            ClipId(Id::new(9).unwrap()),
            Ticks::WHOLE,
            Ticks::WHOLE * 2,
            ClipData::Audio(audio),
        );
        let at = Ticks::WHOLE * 2 + Ticks::QUARTER;
        let part = whole.part(at, whole.end(), &tempo);
        assert_eq!(
            (part.start, part.end(), part.offset),
            (at, whole.end(), Ticks::ZERO)
        );
        let ClipData::Audio(moved) = part.data else {
            unreachable!("an audio clip");
        };
        // A bar and a quarter, slowing from 120 BPM towards 90: longer than
        // the 2.5 s they'd last at 120.
        let by = tempo.ticks_to_seconds(at) - tempo.ticks_to_seconds(Ticks::WHOLE);
        assert!(by > 2.5 && by < 3.5, "{by}");
        assert_eq!(moved.start, 0.25 + by);
        assert_eq!(
            AudioData {
                start: 0.25,
                ..moved
            },
            audio,
            "only the Start marker moves"
        );
        // A MIDI clip's part keeps its notes and takes an offset.
        let notes = Clip {
            data: midi(),
            ..whole
        };
        let part = notes.part(at, notes.end(), &tempo);
        assert_eq!(part.offset, at - Ticks::WHOLE);
        assert_eq!(part.content_start(), notes.content_start());
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
