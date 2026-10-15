//! Commands: the only way to change a document.

use std::collections::BTreeSet;

use bl_core::id::Id;
use bl_core::time::{
    Bpm, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use imbl::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::{AudioData, Clip, ClipData, MidiData};
use crate::device::{Device, DeviceRole};
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{ClipId, ContentHash, DeviceId, ParamId, TrackId};
use crate::metronome::{CountIn, MetronomeRhythm, MetronomeSound};
use crate::shared::Shared;
use crate::track::{Channel, Input, Mixer, Monitor, Output, SendTap, Track, TrackKind};

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
    /// Sets the tempo at a position on the Main track's Song Tempo envelope,
    /// as dragging a breakpoint there does: the breakpoint at `at` becomes
    /// `bpm`, or a new one is added when there's none, and the segments
    /// either side follow it.
    SetTempoBreakpoint {
        /// Where the breakpoint sits: 0 (1.1.1) or later.
        at: Ticks,
        /// The tempo there, as quarter notes per minute.
        bpm: Bpm,
    },
    /// Moves the tempo breakpoint at `at` to `to`, keeping its tempo. The
    /// first breakpoint, at 1.1.1, can't move.
    MoveTempoBreakpoint {
        /// Where the breakpoint is now.
        at: Ticks,
        /// Where it goes: 0 (1.1.1) or later, and not where another
        /// breakpoint already is.
        to: Ticks,
    },
    /// Deletes the tempo breakpoint at `at`, joining its neighbours with a
    /// straight segment. The first breakpoint, at 1.1.1, can't be deleted.
    DeleteTempoBreakpoint {
        /// Where the breakpoint is.
        at: Ticks,
    },
    /// Inserts or replaces a time-signature marker at `at`: the signature
    /// at a position in the arrangement becomes `signature`, and a new bar
    /// starts there.
    SetTimeSignatureChange {
        /// Where the marker sits: 0 (1.1.1) or later.
        at: Ticks,
        /// The signature from there on.
        signature: TimeSignature,
    },
    /// Moves the time-signature marker at `at` to `to`. The first, at
    /// 1.1.1, can't move.
    MoveTimeSignatureChange {
        /// Where the marker is now.
        at: Ticks,
        /// Where it goes: 0 (1.1.1) or later, and not where another marker
        /// already is.
        to: Ticks,
    },
    /// Deletes the time-signature marker at `at`. The first, at 1.1.1,
    /// can't be deleted.
    DeleteTimeSignatureChange {
        /// Where the marker is.
        at: Ticks,
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
    /// Switches the metronome on or off.
    SetMetronomeEnabled {
        /// Whether the metronome clicks during playback.
        enabled: bool,
    },
    /// Sets the metronome count-in length before playback or recording starts.
    SetMetronomeCountIn {
        /// The count-in duration in bars: none, 1 bar, 2 bars, or 4 bars.
        count_in: CountIn,
    },
    /// Chooses the metronome sound character: Classic, Click, or Wood.
    SetMetronomeSound {
        /// The synthesised sound character.
        sound: MetronomeSound,
    },
    /// Sets the rhythmic division the metronome clicks at: Auto, 1 Bar,
    /// 1/2, 1/2T, 1/4, 1/4T, 1/8, 1/8T, 1/16, or 1/16T.
    SetMetronomeRhythm {
        /// The rhythm division.
        rhythm: MetronomeRhythm,
    },
    /// Sets whether the metronome only clicks while recording.
    SetMetronomeOnlyWhileRecording {
        /// When on, clicks only sound while recording.
        only_while_recording: bool,
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
    /// Sets where an audio or MIDI track takes its input from: Audio From
    /// or MIDI From, with its channel.
    SetInput {
        /// The track.
        track: TrackId,
        /// The input.
        input: Input,
    },
    /// Sets an audio or MIDI track's Monitor switch.
    SetMonitor {
        /// The track.
        track: TrackId,
        /// `in`, `auto`, or `off`.
        monitor: Monitor,
    },
    /// Sets where a track's audio goes: Audio To.
    SetOutput {
        /// The track.
        track: TrackId,
        /// The output.
        output: Output,
    },
    /// Sets where a MIDI track's MIDI goes: MIDI To.
    SetMidiOutput {
        /// The MIDI track.
        track: TrackId,
        /// Another MIDI track, or `null` (or nothing) for No Output.
        to: Option<TrackId>,
    },
    /// Sets Main's Main Out: the audio interface's output channels the Main
    /// mix goes to, as a stereo pair.
    SetMainOut {
        /// The pair: its first channel, counted from 1 (`{ "type": "pair",
        /// "channel": 3 }` is 3/4).
        channel: Channel,
    },
    /// Sets Main's Cue Out: the audio interface's output channels the cue
    /// mix goes to, as a stereo pair.
    SetCueOut {
        /// The pair: its first channel, counted from 1.
        channel: Channel,
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
    /// Moves a MIDI clip's Start and End markers: where in its contents it
    /// starts playing, and where it stops while its loop is off.
    SetMidiClipRegion {
        /// The MIDI clip.
        clip: ClipId,
        /// The Start marker, in ticks from the clip's beginning.
        start: Ticks,
        /// The End marker, in ticks from the clip's beginning, after `start`;
        /// `null` (or nothing) for the end of the clip.
        #[serde(default)]
        end: Option<Ticks>,
    },
    /// Moves a MIDI clip's loop brace, leaving its Loop switch as it is.
    SetMidiClipLoop {
        /// The MIDI clip.
        clip: ClipId,
        /// Where the brace starts, in ticks from the clip's beginning.
        start: Ticks,
        /// Where it ends, in ticks, after `start`; `null` (or nothing) for
        /// the end of the clip.
        #[serde(default)]
        end: Option<Ticks>,
    },
    /// Switches an audio or MIDI clip's loop on or off, leaving its brace where it
    /// is.
    SetClipLoopEnabled {
        /// The clip.
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

    /// Inserts a new device at `index` in a track's chain, keeping the
    /// chain in order: MIDI effects, then the instrument, then audio
    /// effects.
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
    /// current place. The chain must stay in order.
    MoveDevice {
        /// The device.
        device: DeviceId,
        /// The track it moves to.
        track: TrackId,
        /// Where it goes in that chain.
        index: usize,
    },
    /// Renames a device: the name its title bar shows while Show Preset
    /// Name is on. An empty name gives it back its own name.
    RenameDevice {
        /// The device.
        device: DeviceId,
        /// The new name.
        name: String,
    },
    /// Switches a device's Show Preset Name: its title bar shows its
    /// preset's or given name (on) or its own name (off).
    SetShowPresetName {
        /// The device.
        device: DeviceId,
        /// Whether the preset's or given name shows.
        shown: bool,
    },
    /// Replaces a device's state, as loading a preset does: every
    /// parameter value (those not listed go back to their defaults) and the
    /// name its title bar shows.
    SetDeviceState {
        /// The device.
        device: DeviceId,
        /// The parameter values, as `[parameter, value]` pairs, each
        /// finite and each parameter once.
        #[serde(with = "crate::device::param_pairs")]
        #[schemars(with = "Vec<(ParamId, f64)>")]
        params: imbl::OrdMap<ParamId, f64>,
        /// The name the title bar shows while Show Preset Name is on, such
        /// as the preset's; empty for the device's own name.
        name: String,
    },
    /// Floats a device in a window of its own, or docks it back in the
    /// Device View (IMP-14, WP-2.5b). Only native devices float.
    SetDeviceFloating {
        /// The device.
        device: DeviceId,
        /// Float (true) or dock (false).
        floating: bool,
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
    /// Sets the media a device plays, by content hash (WP-2.16): the
    /// sampler's sample, which the media store holds like a clip's audio.
    SetDeviceSample {
        /// The device.
        device: DeviceId,
        /// The media, by content hash, or `null` to play none. The media
        /// store is where the audio comes from; a device given media the
        /// store doesn't hold is silent.
        media: Option<ContentHash>,
    },
    /// Sets where a device's sidechain listens: its sidechain section's
    /// Source, with its channel. The device must have a sidechain input.
    SetSidechainInput {
        /// The device.
        device: DeviceId,
        /// `none` (No Input), or `track` with an audio, group, or return
        /// track or Main (the device's own track too) and a `tap`.
        input: Input,
    },
    /// Switches a device's sidechain on or off (its External switch):
    /// whether its detector listens to the sidechain rather than its own
    /// input.
    SetSidechainEnabled {
        /// The device.
        device: DeviceId,
        /// Whether the sidechain is on.
        enabled: bool,
    },
    /// Sets a device's sidechain gain, as linear gain (finite, at least 0).
    SetSidechainGain {
        /// The device.
        device: DeviceId,
        /// The gain: 1 is 0 dB.
        gain: f64,
    },
    /// Sets how much of a device's detector signal is its sidechain, from
    /// 0 (its own input) to 1 (the sidechain alone).
    SetSidechainMix {
        /// The device.
        device: DeviceId,
        /// The mix, from 0 to 1.
        mix: f64,
    },
    /// Switches a device's Sidechain Listen: whether its output is what
    /// its detector hears.
    SetSidechainListen {
        /// The device.
        device: DeviceId,
        /// Whether to listen.
        listen: bool,
    },
}

impl Command {
    /// A short description for the undo history, such as "Move clip".
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::SetTempo { .. } => "Change tempo",
            Self::SetTimeSignatures { .. } => "Change time signatures",
            // Live's undo history names tempo envelope work as adding,
            // moving, and deleting breakpoints.
            Self::SetTempoBreakpoint { .. } => "Change tempo breakpoint",
            Self::MoveTempoBreakpoint { .. } => "Move tempo breakpoint",
            Self::DeleteTempoBreakpoint { .. } => "Delete tempo breakpoint",
            Self::SetTimeSignatureChange { .. } => "Change time signature",
            Self::MoveTimeSignatureChange { .. } => "Move time signature change",
            Self::DeleteTimeSignatureChange { .. } => "Delete time signature change",
            Self::SetLoop { .. } => "Change loop",
            Self::SetLoopEnabled { .. } => "Switch loop",
            Self::SetMetronomeEnabled { .. } => "Metronome",
            Self::SetMetronomeCountIn { .. } => "Metronome Count-In",
            Self::SetMetronomeSound { .. } => "Metronome Sound",
            Self::SetMetronomeRhythm { .. } => "Metronome Rhythm",
            Self::SetMetronomeOnlyWhileRecording { .. } => "Hear Metronome Only While Recording",
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
            Self::SetInput { .. }
            | Self::SetOutput { .. }
            | Self::SetMidiOutput { .. }
            | Self::SetMainOut { .. }
            | Self::SetCueOut { .. } => "Change \"Routing\"",
            Self::SetMonitor { .. } => "Change monitoring",
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
            Self::SetMidiClipRegion { .. } => "Change clip region",
            Self::SetMidiClipLoop { .. } => "Change clip loop",
            Self::SetClipLoopEnabled { .. } => "Switch clip loop",
            Self::SetClipGain { .. } => "Change clip gain",
            Self::SetClipFades { .. } => "Change clip fades",
            // Spec: device-chains R14. Live 12.4.6's names, where observed.
            Self::InsertDevice { .. } => "Insert Device",
            Self::RemoveDevice { .. } => "Delete Device(s)",
            Self::MoveDevice { .. } => "Move Device",
            Self::RenameDevice { .. } => "Change \"Device Name\"",
            Self::SetShowPresetName { .. } => "Toggle Show Preset Name",
            Self::SetDeviceState { .. } => "Load Preset",
            Self::SetDeviceEnabled { .. } => "Change \"Device On\"",
            // IMP-14: Beatlayer's own, so Live has no name for them.
            Self::SetDeviceFloating { floating: true, .. } => "Float Device",
            Self::SetDeviceFloating {
                floating: false, ..
            } => "Dock Device",
            Self::SetDeviceParam { .. } => "Change parameter",
            // WP-2.16. Live's undo name for loading a Simpler sample is to
            // observe; Beatlayer's names the edit.
            Self::SetDeviceSample { media: Some(_), .. } => "Load Sample",
            Self::SetDeviceSample { media: None, .. } => "Clear Sample",
            // WP-2.19b, as Live 12.4.6 names them (observed); Listen's is to
            // observe.
            Self::SetSidechainInput { .. } => "Change \"Routing\"",
            Self::SetSidechainEnabled { .. } => "Change \"Sidechain On\"",
            Self::SetSidechainGain { .. } => "Change \"SC Gain\"",
            Self::SetSidechainMix { .. } => "Change \"Mix\"",
            Self::SetSidechainListen { .. } => "Change \"Sidechain Listen\"",
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
            Command::SetTempoBreakpoint { at, bpm } => self.set_tempo_breakpoint(*at, *bpm),
            Command::MoveTempoBreakpoint { at, to } => self.move_tempo_breakpoint(*at, *to),
            Command::DeleteTempoBreakpoint { at } => self.delete_tempo_breakpoint(*at),
            Command::SetTimeSignatureChange { at, signature } => {
                self.set_time_signature_change(*at, *signature)
            }
            Command::MoveTimeSignatureChange { at, to } => {
                self.move_time_signature_change(*at, *to)
            }
            Command::DeleteTimeSignatureChange { at } => self.delete_time_signature_change(*at),
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
            Command::SetMetronomeEnabled { enabled } => {
                self.settings.metronome.enabled = *enabled;
                Ok(())
            }
            Command::SetMetronomeCountIn { count_in } => {
                self.settings.metronome.count_in = *count_in;
                Ok(())
            }
            Command::SetMetronomeSound { sound } => {
                self.settings.metronome.sound = *sound;
                Ok(())
            }
            Command::SetMetronomeRhythm { rhythm } => {
                self.settings.metronome.rhythm = *rhythm;
                Ok(())
            }
            Command::SetMetronomeOnlyWhileRecording {
                only_while_recording,
            } => {
                self.settings.metronome.only_while_recording = *only_while_recording;
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
            Command::SetInput { track, input } => {
                self.edit_routing(*track, |routing| routing.input = *input)
            }
            Command::SetMonitor { track, monitor } => {
                check_kind(
                    self.track_ref(*track)?,
                    TrackKind::holds_clips,
                    AUDIO_OR_MIDI,
                )?;
                self.edit_routing(*track, |routing| routing.monitor = *monitor)
            }
            Command::SetOutput { track, output } => {
                let output = self.normal_output(*track, *output);
                self.edit_routing(*track, |routing| routing.output = output)
            }
            Command::SetMidiOutput { track, to } => {
                self.edit_routing(*track, |routing| routing.midi_output = *to)
            }
            Command::SetMainOut { channel } => {
                self.set_main_channel(*channel, |routing| &mut routing.main_out)
            }
            Command::SetCueOut { channel } => {
                self.set_main_channel(*channel, |routing| &mut routing.cue_out)
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
            Command::SetMidiClipRegion { clip, start, end } => {
                self.edit_midi_markers(*clip, |midi| {
                    midi.start = *start;
                    midi.end = *end;
                })
            }
            Command::SetMidiClipLoop { clip, start, end } => {
                self.edit_midi_markers(*clip, |midi| {
                    midi.loop_start = *start;
                    midi.loop_end = *end;
                })
            }
            Command::SetClipLoopEnabled { clip, enabled } => match &self.clip_ref(*clip)?.data {
                ClipData::Audio(_) => self.edit_markers(*clip, |audio| audio.looping = *enabled),
                ClipData::Midi(_) => self.edit_midi_markers(*clip, |midi| midi.looping = *enabled),
            },
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
                let mut renamed = self.device_ref(*device)?.clone();
                renamed.set_name(name);
                if renamed != *self.device_ref(*device)? {
                    *self.device_mut(*device)? = renamed;
                }
                Ok(())
            }
            Command::SetShowPresetName { device, shown } => {
                if self.device_ref(*device)?.show_preset_name != *shown {
                    self.device_mut(*device)?.show_preset_name = *shown;
                }
                Ok(())
            }
            Command::SetDeviceState {
                device,
                params,
                name,
            } => {
                let mut loaded = self.device_ref(*device)?.clone();
                loaded.params = params.clone();
                loaded.set_name(name);
                check_device(&loaded)?;
                if loaded != *self.device_ref(*device)? {
                    *self.device_mut(*device)? = loaded;
                }
                Ok(())
            }
            Command::SetDeviceFloating { device, floating } => {
                let current = self.device_ref(*device)?;
                if *floating && !current.can_float() {
                    return Err(ModelError::CannotFloat(*device));
                }
                if current.floating != *floating {
                    self.device_mut(*device)?.floating = *floating;
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
            Command::SetDeviceSample { device, media } => {
                if self.device_ref(*device)?.media != *media {
                    self.device_mut(*device)?.media = *media;
                }
                Ok(())
            }
            Command::SetSidechainInput { device, input } => {
                self.edit_sidechain(*device, |s| s.input = *input)?;
                self.check_routing(self.device_owner(*device)?)
            }
            Command::SetSidechainEnabled { device, enabled } => {
                self.edit_sidechain(*device, |s| s.enabled = *enabled)
            }
            Command::SetSidechainGain { device, gain } => {
                check_value("sidechain gain", *gain, gain.is_finite() && *gain >= 0.0)?;
                self.edit_sidechain(*device, |s| s.gain = *gain)
            }
            Command::SetSidechainMix { device, mix } => {
                check_value("sidechain mix", *mix, (0.0..=1.0).contains(mix))?;
                self.edit_sidechain(*device, |s| s.mix = *mix)
            }
            Command::SetSidechainListen { device, listen } => {
                self.edit_sidechain(*device, |s| s.listen = *listen)
            }
        }
    }

    /// Changes a device's sidechain settings with `edit`, if it has a
    /// sidechain input.
    fn edit_sidechain(
        &mut self,
        id: DeviceId,
        edit: impl FnOnce(&mut crate::device::Sidechain),
    ) -> Result<(), ModelError> {
        let mut sidechain = self
            .device_ref(id)?
            .sidechain
            .ok_or(ModelError::NoSidechain(id))?;
        edit(&mut sidechain);
        if Some(sidechain) != self.device_ref(id)?.sidechain {
            self.device_mut(id)?.sidechain = Some(sidechain);
        }
        Ok(())
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
        let mut gone = BTreeSet::new();
        while let Some(id) = doomed.pop() {
            gone.insert(id);
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
        self.forget_routes_to(&gone);
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

    /// Changes a track's routing and checks it.
    fn edit_routing(
        &mut self,
        id: TrackId,
        edit: impl FnOnce(&mut crate::track::Routing),
    ) -> Result<(), ModelError> {
        let mut routing = self.track_ref(id)?.routing;
        edit(&mut routing);
        if routing == self.track_ref(id)?.routing {
            return Ok(());
        }
        self.track_mut(id)?.routing = routing;
        self.check_routing(id)
    }

    /// Sets Main's Main Out or Cue Out, as `slot` names it, one of the two.
    fn set_main_channel(
        &mut self,
        channel: Channel,
        slot: fn(&mut crate::track::Routing) -> &mut Channel,
    ) -> Result<(), ModelError> {
        let main = self.main_id();
        let mut routing = self.track_ref(main)?.routing;
        *slot(&mut routing) = channel;
        if routing == self.track_ref(main)?.routing {
            return Ok(());
        }
        self.track_mut(main)?.routing = routing;
        self.check_routing(main)
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
        if kind != TrackKind::Return {
            self.settle_output(id, old_parent);
            self.check_routing(id)?;
        }
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
        let ClipData::Audio(current) = &self.clip_ref(id)?.data else {
            return Err(ModelError::NotAudioClip(id));
        };
        let mut next = *current;
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
        let ClipData::Audio(current) = &self.clip_ref(id)?.data else {
            return Err(ModelError::NotAudioClip(id));
        };
        let mut next = *current;
        edit(&mut next);
        check_audio(&next)?;
        if next != *current {
            self.clip_mut(id)?.data = ClipData::Audio(next);
        }
        Ok(())
    }

    /// Sets a MIDI clip's markers or loop with `edit`, if the result is valid
    /// and its markers are in order, as Live keeps markers set by hand.
    fn edit_midi_markers(
        &mut self,
        id: ClipId,
        edit: impl FnOnce(&mut MidiData),
    ) -> Result<(), ModelError> {
        let ClipData::Midi(current) = &self.clip_ref(id)?.data else {
            return Err(ModelError::NotMidiClip(id));
        };
        let mut next = current.clone();
        edit(&mut next);
        check_midi_marker_order(&next)?;
        self.edit_midi(id, |midi| *midi = next)
    }

    /// Changes a MIDI clip's data with `edit`, if the result is valid.
    fn edit_midi(
        &mut self,
        id: ClipId,
        edit: impl FnOnce(&mut MidiData),
    ) -> Result<(), ModelError> {
        let ClipData::Midi(current) = &self.clip_ref(id)?.data else {
            return Err(ModelError::NotMidiClip(id));
        };
        let mut next = current.clone();
        edit(&mut next);
        check_clip_data(&ClipData::Midi(next.clone()))?;
        if next != *current {
            self.clip_mut(id)?.data = ClipData::Midi(next);
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
        check_chain(self.track_ref(track)?)?;
        self.check_routing(track)
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
        check_chain(self.track_ref(track)?)
    }

    // The Main track's Song Tempo envelope and the arrangement's
    // time-signature markers, as edits on the settings' maps.

    /// Sets the tempo at `at` to `bpm`, adding a breakpoint there or
    /// replacing the one that's there. A breakpoint between two others
    /// splits the segment, keeping the neighbours.
    fn set_tempo_breakpoint(&mut self, at: Ticks, bpm: Bpm) -> Result<(), ModelError> {
        if at < Ticks::ZERO {
            return Err(ModelError::NegativePosition(at));
        }
        let mut points = tempo_breakpoints(&self.settings.tempo);
        points.retain(|&(position, _)| position != at);
        points.push((at, bpm));
        points.sort_by_key(|&(position, _)| position);
        self.settings.tempo = tempo_from_breakpoints(points);
        Ok(())
    }

    /// Moves the tempo breakpoint at `at` to `to`, keeping its tempo.
    fn move_tempo_breakpoint(&mut self, at: Ticks, to: Ticks) -> Result<(), ModelError> {
        if to < Ticks::ZERO {
            return Err(ModelError::NegativePosition(to));
        }
        if at == Ticks::ZERO {
            return Err(ModelError::FirstChange {
                what: "tempo breakpoint",
            });
        }
        let points = tempo_breakpoints(&self.settings.tempo);
        let moved: Vec<(Ticks, Bpm)> = points.iter().copied().filter(|&(p, _)| p == at).collect();
        if moved.is_empty() {
            return Err(ModelError::NoChange {
                what: "tempo breakpoint",
                at,
            });
        }
        if to != at && points.iter().any(|&(p, _)| p == to) {
            return Err(ModelError::ChangeInUse {
                what: "tempo breakpoint",
                at: to,
            });
        }
        let mut next: Vec<(Ticks, Bpm)> = points.into_iter().filter(|&(p, _)| p != at).collect();
        next.extend(moved.into_iter().map(|(_, bpm)| (to, bpm)));
        next.sort_by_key(|&(position, _)| position);
        self.settings.tempo = tempo_from_breakpoints(next);
        Ok(())
    }

    /// Deletes the tempo breakpoint at `at`, joining its neighbours with a
    /// straight segment.
    fn delete_tempo_breakpoint(&mut self, at: Ticks) -> Result<(), ModelError> {
        if at == Ticks::ZERO {
            return Err(ModelError::FirstChange {
                what: "tempo breakpoint",
            });
        }
        let points = tempo_breakpoints(&self.settings.tempo);
        if !points.iter().any(|&(p, _)| p == at) {
            return Err(ModelError::NoChange {
                what: "tempo breakpoint",
                at,
            });
        }
        let next: Vec<(Ticks, Bpm)> = points.into_iter().filter(|&(p, _)| p != at).collect();
        self.settings.tempo = tempo_from_breakpoints(next);
        Ok(())
    }

    /// Sets the time-signature marker at `at`, adding one when there's none.
    fn set_time_signature_change(
        &mut self,
        at: Ticks,
        signature: TimeSignature,
    ) -> Result<(), ModelError> {
        if at < Ticks::ZERO {
            return Err(ModelError::NegativePosition(at));
        }
        let mut changes: Vec<TimeSignatureChange> =
            self.settings.time_signatures.changes().collect();
        match changes.iter_mut().find(|change| change.at == at) {
            Some(change) => change.signature = signature,
            None => {
                changes.push(TimeSignatureChange { at, signature });
                changes.sort_by_key(|change| change.at);
            }
        }
        self.settings.time_signatures = time_signature_map(changes);
        Ok(())
    }

    /// Moves the time-signature marker at `at` to `to`.
    fn move_time_signature_change(&mut self, at: Ticks, to: Ticks) -> Result<(), ModelError> {
        if to < Ticks::ZERO {
            return Err(ModelError::NegativePosition(to));
        }
        if at == Ticks::ZERO {
            return Err(ModelError::FirstChange {
                what: "time signature",
            });
        }
        let mut changes: Vec<TimeSignatureChange> =
            self.settings.time_signatures.changes().collect();
        if !changes.iter().any(|change| change.at == at) {
            return Err(ModelError::NoChange {
                what: "time signature",
                at,
            });
        }
        if to != at && changes.iter().any(|change| change.at == to) {
            return Err(ModelError::ChangeInUse {
                what: "time signature",
                at: to,
            });
        }
        for change in &mut changes {
            if change.at == at {
                change.at = to;
            }
        }
        changes.sort_by_key(|change| change.at);
        self.settings.time_signatures = time_signature_map(changes);
        Ok(())
    }

    /// Deletes the time-signature marker at `at`.
    fn delete_time_signature_change(&mut self, at: Ticks) -> Result<(), ModelError> {
        if at == Ticks::ZERO {
            return Err(ModelError::FirstChange {
                what: "time signature",
            });
        }
        let mut changes: Vec<TimeSignatureChange> =
            self.settings.time_signatures.changes().collect();
        let before = changes.len();
        changes.retain(|change| change.at != at);
        if changes.len() == before {
            return Err(ModelError::NoChange {
                what: "time signature",
                at,
            });
        }
        self.settings.time_signatures = time_signature_map(changes);
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

    /// Sets a plug-in device's saved state (WP-2.6), outside the command
    /// system: [`Editor::amend_plugin_state`](crate::Editor::amend_plugin_state)
    /// calls it.
    pub(crate) fn set_plugin_state(
        &mut self,
        id: DeviceId,
        state: Option<crate::PluginState>,
    ) -> Result<(), ModelError> {
        if self.device_ref(id)?.plugin != state {
            self.device_mut(id)?.plugin = state;
        }
        Ok(())
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

/// The breakpoints that [`TempoMap::from_breakpoints`] rebuilds `tempo`
/// from: each segment's starting tempo, and, where a segment arrives at a
/// tempo the next one doesn't start at (a jump), a second breakpoint at the
/// next segment's start carrying the arrival.
fn tempo_breakpoints(tempo: &TempoMap) -> Vec<(Ticks, Bpm)> {
    let segments: Vec<TempoSegment> = tempo.segments().collect();
    let mut points = Vec::with_capacity(segments.len() + 1);
    for (index, segment) in segments.iter().enumerate() {
        points.push((segment.start(), segment.start_bpm()));
        if let Some(next) = segments.get(index + 1)
            && segment.end_bpm() != next.start_bpm()
        {
            points.push((next.start(), segment.end_bpm()));
        }
    }
    points
}

/// Builds a tempo map from breakpoints this module made, which always start
/// at tick zero and don't move backwards, so the map is valid.
fn tempo_from_breakpoints(points: Vec<(Ticks, Bpm)>) -> TempoMap {
    TempoMap::from_breakpoints(points).expect("a tempo map edited this way stays valid")
}

/// Builds a time-signature map from changes this module made, which always
/// start at tick zero and are in order, so the map is valid.
fn time_signature_map(changes: Vec<TimeSignatureChange>) -> TimeSignatureMap {
    TimeSignatureMap::new(changes).expect("a time-signature map edited this way stays valid")
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
                if note.key > 127
                    || !(1..=127).contains(&note.velocity)
                    || !(1..=127).contains(&note.release_velocity)
                    || !note.probability.is_finite()
                    || !(0.0..=1.0).contains(&note.probability)
                {
                    return Err(ModelError::InvalidNote { index });
                }
                check_extent(note.start, note.length)?;
            }
            check_midi(midi)
        }
        ClipData::Audio(audio) => check_audio(audio),
    }
}

/// Checks a MIDI clip's data: markers and loop brace.
pub(crate) fn check_midi(midi: &MidiData) -> Result<(), ModelError> {
    if midi.start < Ticks::ZERO {
        return Err(ModelError::NegativePosition(midi.start));
    }
    if let Some(end) = midi.end.filter(|&end| end <= Ticks::ZERO) {
        return Err(ModelError::EmptyLength(end));
    }
    if midi.loop_start < Ticks::ZERO {
        return Err(ModelError::NegativePosition(midi.loop_start));
    }
    if let Some(end) = midi.loop_end.filter(|&end| end <= midi.loop_start) {
        return Err(ModelError::EmptyLength(end));
    }
    Ok(())
}

/// Checks that a MIDI clip's markers are in order, as setting them by hand
/// keeps them: the End marker after the Start marker, and, while the loop is
/// on, the Start marker before the loop brace's end.
pub(crate) fn check_midi_marker_order(midi: &MidiData) -> Result<(), ModelError> {
    if let Some(end) = midi.end {
        check_value("clip end", end.get() as f64, end > midi.start)?;
    }
    if let (true, Some(end)) = (midi.looping, midi.loop_end) {
        check_value(
            "clip start (at or after the loop end)",
            midi.start.get() as f64,
            midi.start < end,
        )?;
    }
    Ok(())
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
    if device.floating && !device.can_float() {
        return Err(ModelError::CannotFloat(device.id));
    }
    for value in device.params.values() {
        check_value("parameter value", *value, value.is_finite())?;
    }
    if let Some(sidechain) = &device.sidechain {
        let gain = sidechain.gain;
        check_value("sidechain gain", gain, gain.is_finite() && gain >= 0.0)?;
        check_value(
            "sidechain mix",
            sidechain.mix,
            (0.0..=1.0).contains(&sidechain.mix),
        )?;
    }
    Ok(())
}

/// Checks a chain against Live's rules (the device chains spec, R2 and R3):
/// MIDI effects and instruments only on MIDI tracks, MIDI effects first,
/// then at most one instrument, then audio effects.
pub(crate) fn check_chain(track: &Track) -> Result<(), ModelError> {
    let mut last = None;
    for device in track.devices() {
        if !device.role.fits(track.kind) {
            return Err(ModelError::DeviceDoesNotFit {
                track: track.id,
                kind: track.kind,
                role: device.role,
            });
        }
        let repeated_instrument =
            device.role == DeviceRole::Instrument && last == Some(DeviceRole::Instrument);
        if last.is_some_and(|last| device.role < last) || repeated_instrument {
            return Err(ModelError::DeviceOrder { track: track.id });
        }
        last = Some(device.role);
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
    use crate::device::{DeviceKind, DeviceRole, PluginFormat};
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
        ClipData::Midi(MidiData::new([Note::new(
            60,
            Ticks::ZERO,
            Ticks::QUARTER,
            100,
        )]))
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

        let note = |key, velocity| Note::new(key, Ticks::ZERO, Ticks::QUARTER, velocity);
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

    // Spec: audio-clips R16. As observed in Live 12.4.6: a looped clip
    // trimmed from bar 5 by exactly one loop length keeps its Start marker,
    // and by a bar more moves it a bar on, wrapping inside the loop brace.
    #[test]
    fn trimming_a_looped_clip_wraps_its_start_marker_inside_the_loop() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.fresh();
        // At 120 BPM a bar is two seconds: the brace is bars 1 to 9 of the
        // file, and the clip lasts 24 bars.
        let looped = AudioData {
            looping: true,
            loop_start: 0.0,
            loop_end: Some(16.0),
            ..AudioData::new(ContentHash([7; 32]))
        };
        song.apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE * 24,
                ClipData::Audio(looped),
            ),
        })
        .unwrap();
        let start = |song: &Song| match song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.start,
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        let trim = |song: &mut Song, bar: i64| {
            song.apply(Command::TrimClipStart {
                clip,
                start: Ticks::WHOLE * (bar - 1),
            })
            .unwrap();
        };
        trim(&mut song, 5);
        assert_eq!(start(&song), 8.0, "bar 5 of the file");
        trim(&mut song, 13);
        assert_eq!(start(&song), 8.0, "one loop on: bar 5 again");
        trim(&mut song, 14);
        assert_eq!(start(&song), 10.0, "a bar more: bar 6");
        trim(&mut song, 23);
        assert_eq!(start(&song), 12.0, "past two wraps: bar 7");
        // A brace that ends at the file's end has a length the set doesn't
        // know, so the marker can't wrap.
        let to_end = AudioData {
            loop_end: None,
            ..looped
        };
        let whole = Clip::new(
            clip,
            Ticks::ZERO,
            Ticks::WHOLE * 24,
            ClipData::Audio(to_end),
        );
        let part = whole.part(Ticks::WHOLE * 12, whole.end(), &TempoMap::default());
        let ClipData::Audio(moved) = part.data else {
            unreachable!("an audio clip");
        };
        assert_eq!(moved.start, 24.0);
        // Unlooped, it doesn't wrap either.
        let plain = Clip::new(
            clip,
            Ticks::ZERO,
            Ticks::WHOLE * 24,
            ClipData::Audio(AudioData {
                looping: false,
                ..looped
            }),
        );
        let ClipData::Audio(moved) = plain
            .part(Ticks::WHOLE * 12, plain.end(), &TempoMap::default())
            .data
        else {
            unreachable!("an audio clip");
        };
        assert_eq!(moved.start, 24.0);
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

    /// A device with a role, for the ordering tests.
    fn with_role(song: &mut Song, role: DeviceRole) -> Device {
        Device::new(
            song.fresh(),
            role.label(),
            DeviceKind::Native {
                device_type: format!("test.{role:?}"),
            },
        )
        .with_role(role)
    }

    // Spec: device-chains R2, R3
    #[test]
    fn chains_keep_lives_order() {
        let mut song = Song::new();
        let midi = song.track(TrackKind::Midi, None);
        let audio = song.track(TrackKind::Audio, None);
        let insert = |track, index, device: &Device| Command::InsertDevice {
            track,
            index,
            device: device.clone(),
        };
        let fx = with_role(&mut song, DeviceRole::AudioEffect);
        let synth = with_role(&mut song, DeviceRole::Instrument);
        let arp = with_role(&mut song, DeviceRole::MidiEffect);
        song.apply(insert(midi, 0, &fx)).unwrap();
        // An instrument can't go after an audio effect, a MIDI effect not
        // after the instrument.
        assert_eq!(
            song.reject(insert(midi, 1, &synth)),
            ModelError::DeviceOrder { track: midi }
        );
        song.apply(insert(midi, 0, &synth)).unwrap();
        assert_eq!(
            song.reject(insert(midi, 1, &arp)),
            ModelError::DeviceOrder { track: midi }
        );
        song.apply(insert(midi, 0, &arp)).unwrap();
        // One instrument per chain.
        let second = with_role(&mut song, DeviceRole::Instrument);
        assert_eq!(
            song.reject(insert(midi, 2, &second)),
            ModelError::DeviceOrder { track: midi }
        );
        // Only MIDI tracks hold MIDI effects and instruments.
        for device in [&second, &with_role(&mut song, DeviceRole::MidiEffect)] {
            assert!(matches!(
                song.reject(insert(audio, 0, device)),
                ModelError::DeviceDoesNotFit { .. }
            ));
        }
        // Moves keep the order too.
        assert_eq!(
            song.reject(Command::MoveDevice {
                device: fx.id,
                track: midi,
                index: 0,
            }),
            ModelError::DeviceOrder { track: midi }
        );
        assert!(matches!(
            song.reject(Command::MoveDevice {
                device: synth.id,
                track: audio,
                index: 0,
            }),
            ModelError::DeviceDoesNotFit { .. }
        ));
        song.apply(Command::MoveDevice {
            device: fx.id,
            track: audio,
            index: 0,
        })
        .unwrap();
        let roles: Vec<_> = song
            .doc
            .track(midi)
            .unwrap()
            .devices()
            .map(|d| d.role)
            .collect();
        assert_eq!(roles, [DeviceRole::MidiEffect, DeviceRole::Instrument]);
    }

    // Spec: device-chains R10
    #[test]
    fn renaming_and_show_preset_name() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let device = song.device(track);
        let title = |song: &Song| song.doc.device(device).unwrap().title().to_owned();
        assert_eq!(title(&song), "Utility");
        song.apply(Command::RenameDevice {
            device,
            name: "Trim".to_owned(),
        })
        .unwrap();
        assert_eq!(title(&song), "Trim");
        assert_eq!(song.doc.device(device).unwrap().own_name(), "Utility");
        song.apply(Command::SetShowPresetName {
            device,
            shown: false,
        })
        .unwrap();
        assert_eq!(title(&song), "Utility");
        song.apply(Command::SetShowPresetName {
            device,
            shown: true,
        })
        .unwrap();
        // An empty name gives the device its own name back.
        song.apply(Command::RenameDevice {
            device,
            name: String::new(),
        })
        .unwrap();
        assert_eq!(title(&song), "Utility");
        assert!(song.doc.device(device).unwrap().own_name.is_empty());
    }

    // Spec: device-chains R26
    #[test]
    fn floating_and_docking_are_undo_steps_and_plug_ins_dont_float() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let device = song.device(track);
        let plugin = DeviceId(song.ids.allocate());
        song.apply(Command::InsertDevice {
            track,
            index: 1,
            device: Device::new(
                plugin,
                "Reverb",
                DeviceKind::Plugin {
                    format: crate::PluginFormat::Clap,
                    plugin_id: "org.example.reverb".to_owned(),
                },
            ),
        })
        .unwrap();
        let mut editor =
            crate::Editor::from_document(song.doc.clone(), bl_core::id::IdAllocator::new());
        let floating = |editor: &crate::Editor| editor.document().device(device).unwrap().floating;
        assert!(!floating(&editor));
        editor
            .apply(Command::SetDeviceFloating {
                device,
                floating: true,
            })
            .unwrap();
        assert!(floating(&editor));
        editor
            .apply(Command::SetDeviceFloating {
                device,
                floating: false,
            })
            .unwrap();
        assert!(!floating(&editor));
        assert_eq!(
            editor.undo_history().collect::<Vec<_>>(),
            ["Dock Device", "Float Device"]
        );
        editor.undo().unwrap();
        assert!(floating(&editor));
        editor.undo().unwrap();
        assert!(!floating(&editor));
        // Docking a docked device changes nothing, so it's no step.
        editor
            .apply(Command::SetDeviceFloating {
                device,
                floating: false,
            })
            .unwrap();
        assert!(!editor.can_undo());
        // Plug-ins open their own editor windows (IMP-14).
        assert_eq!(
            song.reject(Command::SetDeviceFloating {
                device: plugin,
                floating: true,
            }),
            ModelError::CannotFloat(plugin)
        );
    }

    // Spec: device-chains R16
    #[test]
    fn device_state_replaces_every_value() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let device = song.device(track);
        song.apply(Command::SetDeviceParam {
            device,
            param: ParamId(1),
            value: 3.0,
        })
        .unwrap();
        let params: imbl::OrdMap<ParamId, f64> = [(ParamId(2), 0.5)].into_iter().collect();
        song.apply(Command::SetDeviceState {
            device,
            params: params.clone(),
            name: "Warm".to_owned(),
        })
        .unwrap();
        let loaded = song.doc.device(device).unwrap();
        assert_eq!(loaded.params, params);
        assert_eq!((loaded.title(), loaded.own_name()), ("Warm", "Utility"));
        let bad: imbl::OrdMap<ParamId, f64> = [(ParamId(2), f64::NAN)].into_iter().collect();
        assert!(matches!(
            song.reject(Command::SetDeviceState {
                device,
                params: bad,
                name: String::new(),
            }),
            ModelError::InvalidValue { .. }
        ));
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

    /// Spec: musical-time R4, R5, R7, R8. Tempo changes and time-signature
    /// changes are edited one breakpoint or marker at a time: a step and a
    /// ramp, a signature change that cuts a bar short, and the refusals.
    #[test]
    fn tempo_breakpoints_and_signature_markers_edit_one_at_a_time() {
        let bpm = |value| Bpm::new(value).unwrap();
        let mut song = Song::new();
        let bar = Ticks::WHOLE;

        // A breakpoint on the Song Tempo envelope: the segments either side
        // run straight to it. 120 BPM at 1.1.1, reaching 60 at bar 1.3.
        song.apply(Command::SetTempoBreakpoint {
            at: bar / 2,
            bpm: bpm(60.0),
        })
        .unwrap();
        let tempo = &song.doc.settings().tempo;
        assert_eq!(tempo.bpm_at(Ticks::ZERO), bpm(120.0));
        assert_eq!(tempo.bpm_at(bar / 4), bpm(90.0));
        assert_eq!(tempo.bpm_at(bar / 2), bpm(60.0));
        assert_eq!(tempo.bpm_at(bar), bpm(60.0), "the last breakpoint holds on");

        // A second breakpoint ramps on from it.
        song.apply(Command::SetTempoBreakpoint {
            at: bar,
            bpm: bpm(90.0),
        })
        .unwrap();
        let tempo = &song.doc.settings().tempo;
        assert_eq!(tempo.bpm_at(bar / 4), bpm(90.0));
        assert_eq!(tempo.bpm_at(bar / 2), bpm(60.0));
        assert_eq!(tempo.bpm_at(bar), bpm(90.0));

        // Moving and deleting a breakpoint.
        song.apply(Command::MoveTempoBreakpoint {
            at: bar / 2,
            to: bar * 2,
        })
        .unwrap();
        let tempo = &song.doc.settings().tempo;
        assert_eq!(tempo.bpm_at(bar / 2), bpm(105.0));
        assert_eq!(tempo.bpm_at(bar * 2), bpm(60.0));
        song.apply(Command::DeleteTempoBreakpoint { at: bar * 2 })
            .unwrap();
        assert_eq!(song.doc.settings().tempo.bpm_at(bar * 2), bpm(90.0));

        // A step: two breakpoints at one position, as the envelope draws
        // when a breakpoint is dropped on another, set through the full map.
        song.apply(Command::SetTempo {
            tempo: TempoMap::from_breakpoints([
                (Ticks::ZERO, bpm(120.0)),
                (bar, bpm(120.0)),
                (bar, bpm(90.0)),
            ])
            .unwrap(),
        })
        .unwrap();
        let tempo = &song.doc.settings().tempo;
        assert_eq!(tempo.bpm_at(bar - Ticks(1)), bpm(120.0));
        assert_eq!(tempo.bpm_at(bar), bpm(90.0));
        assert_eq!(
            tempo.ticks_to_seconds(bar),
            2.0,
            "two seconds at 120 to bar 2"
        );

        // The refusals leave the map as it was.
        for command in [
            Command::SetTempoBreakpoint {
                at: -Ticks(1),
                bpm: bpm(90.0),
            },
            Command::MoveTempoBreakpoint {
                at: Ticks::ZERO,
                to: bar,
            },
            Command::DeleteTempoBreakpoint { at: Ticks::ZERO },
            Command::DeleteTempoBreakpoint { at: bar * 3 },
        ] {
            assert!(song.doc.apply(&command).is_err(), "{command:?}");
        }
        // Moving a breakpoint onto another one is refused.
        song.apply(Command::SetTempoBreakpoint {
            at: bar * 2,
            bpm: bpm(60.0),
        })
        .unwrap();
        let before = song.doc.settings().tempo.clone();
        assert_eq!(
            song.reject(Command::MoveTempoBreakpoint {
                at: bar * 2,
                to: bar
            }),
            ModelError::ChangeInUse {
                what: "tempo breakpoint",
                at: bar,
            }
        );
        assert_eq!(song.doc.settings().tempo, before);
        assert_eq!(
            song.reject(Command::MoveTempoBreakpoint {
                at: bar * 4,
                to: bar * 5,
            }),
            ModelError::NoChange {
                what: "tempo breakpoint",
                at: bar * 4,
            }
        );
        assert_eq!(
            song.reject(Command::SetTempoBreakpoint {
                at: -Ticks(1),
                bpm: bpm(90.0),
            }),
            ModelError::NegativePosition(-Ticks(1))
        );

        // Time-signature markers: 4/4 at 1.1.1, then 7/8 at bar 2. The 7/8
        // bar is 7/8 of a whole note, so bar 3 starts 1⅞ bars in.
        let seven_eight = TimeSignature::new(7, 8).unwrap();
        song.apply(Command::SetTimeSignatureChange {
            at: bar,
            signature: seven_eight,
        })
        .unwrap();
        let signatures = &song.doc.settings().time_signatures;
        assert_eq!(
            signatures.signature_at(Ticks::ZERO),
            TimeSignature::FOUR_FOUR
        );
        assert_eq!(signatures.signature_at(bar), seven_eight);
        assert_eq!(signatures.bar_start(2), bar + bar * 7 / 8);
        assert_eq!(signatures.to_bbs(bar + bar * 7 / 8).to_string(), "3.1.1");
        assert_eq!(signatures.to_bbs(bar + bar / 4).to_string(), "2.3.1");

        // Moving it to bar 2.1.1 (where it already is not) and deleting it.
        song.apply(Command::SetTimeSignatureChange {
            at: bar * 2,
            signature: TimeSignature::new(3, 4).unwrap(),
        })
        .unwrap();
        song.apply(Command::MoveTimeSignatureChange {
            at: bar * 2,
            to: bar * 3,
        })
        .unwrap();
        let signatures = &song.doc.settings().time_signatures;
        assert_eq!(
            signatures.signature_at(bar * 3),
            TimeSignature::new(3, 4).unwrap()
        );
        song.apply(Command::DeleteTimeSignatureChange { at: bar * 3 })
            .unwrap();
        assert_eq!(
            song.doc.settings().time_signatures.signature_at(bar * 3),
            seven_eight
        );
        assert_eq!(
            song.doc.settings().time_signatures.changes().len(),
            2,
            "the 4/4 at 1.1.1 and the 7/8 at bar 2"
        );

        // The same refusals for markers.
        assert_eq!(
            song.reject(Command::SetTimeSignatureChange {
                at: -Ticks(1),
                signature: seven_eight,
            }),
            ModelError::NegativePosition(-Ticks(1))
        );
        assert_eq!(
            song.reject(Command::DeleteTimeSignatureChange { at: Ticks::ZERO }),
            ModelError::FirstChange {
                what: "time signature",
            }
        );
        assert_eq!(
            song.reject(Command::DeleteTimeSignatureChange { at: bar * 5 }),
            ModelError::NoChange {
                what: "time signature",
                at: bar * 5,
            }
        );
        assert_eq!(
            song.reject(Command::MoveTimeSignatureChange {
                at: Ticks::ZERO,
                to: bar * 4,
            }),
            ModelError::FirstChange {
                what: "time signature",
            }
        );
        // Moving a marker onto another one is refused.
        song.apply(Command::SetTimeSignatureChange {
            at: bar * 2,
            signature: TimeSignature::new(3, 4).unwrap(),
        })
        .unwrap();
        assert_eq!(
            song.reject(Command::MoveTimeSignatureChange {
                at: bar,
                to: bar * 2,
            }),
            ModelError::ChangeInUse {
                what: "time signature",
                at: bar * 2,
            }
        );
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
