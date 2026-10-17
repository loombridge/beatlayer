//! Commands: the only way to change a document.

use std::collections::BTreeSet;

use bl_core::id::Id;
use bl_core::time::{
    Bpm, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use imbl::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{
    AutomationTarget, MixerParam, PointError, check_bend, entry, override_target,
};
use crate::clip::{AudioData, Clip, ClipData, ClipEnvelope, MidiData, TakeLane, clip_entry};
use crate::device::{Device, DeviceRole};
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{ChainId, ClipId, ContentHash, DeviceId, ParamId, SceneId, TrackId};
use crate::launch::{ClipLaunch, LaunchMode, LaunchQuantization};
use crate::metronome::{CountIn, MetronomeRhythm, MetronomeSound};
use crate::rack::{CHAIN_COLOURS, Chain};
use crate::scene::{SCENE_TEMPO_MAX, SCENE_TEMPO_MIN, Scene};
use crate::shared::Shared;
use crate::track::{
    Channel, CrossfadeAssign, Input, Mixer, Monitor, Output, SendTap, Track, TrackKind,
};

/// What left a track: devices and rack chains.
#[derive(Default)]
struct Gone {
    devices: BTreeSet<DeviceId>,
    chains: BTreeSet<ChainId>,
}

impl Gone {
    /// A device, and everything inside it.
    fn device(device: &Device) -> Self {
        let mut gone = Self::default();
        gone.add_device(device);
        gone
    }

    /// A chain, and everything inside it.
    fn chain(chain: &Chain) -> Self {
        let mut gone = Self::default();
        gone.add_chain(chain);
        gone
    }

    fn add_device(&mut self, device: &Device) {
        for device in device.walk() {
            self.devices.insert(device.id);
            device.chains.iter().for_each(|chain| {
                self.chains.insert(chain.id);
            });
        }
    }

    fn add_chain(&mut self, chain: &Chain) {
        self.chains.insert(chain.id);
        chain
            .devices
            .iter()
            .for_each(|device| self.add_device(device));
    }
}

/// Forgets everything a track holds for what left it: the envelopes on the
/// track, and the clip envelopes that modulate their parameters (automation
/// R11, WP-3.15).
fn forget(track: &mut Track, gone: &Gone) {
    for target in track.automation.keys().copied().collect::<Vec<_>>() {
        let names_gone = match target {
            AutomationTarget::Device { device, .. } => gone.devices.contains(&device),
            AutomationTarget::Chain { chain, .. } => gone.chains.contains(&chain),
            _ => false,
        };
        if names_gone {
            track.automation.remove(&target);
        }
    }
    let names_gone = |envelope: &ClipEnvelope| match envelope.target {
        AutomationTarget::Device { device, .. } => gone.devices.contains(&device),
        AutomationTarget::Chain { chain, .. } => gone.chains.contains(&chain),
        _ => false,
    };
    let clips: Vec<ClipId> = track
        .clips()
        .chain(track.slots().map(|(_, clip)| clip))
        .filter(|clip| clip.envelopes.iter().any(names_gone))
        .map(|clip| clip.id)
        .collect();
    for id in clips {
        if let Some(clip) = track.clip_mut(id) {
            clip.envelopes.retain(|e| !names_gone(e));
        }
    }
}

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
    /// Switches Punch-In on or off: with it on, recording into the
    /// Arrangement starts at the loop brace's start, whether or not the Loop
    /// switch is on.
    SetPunchIn {
        /// Whether recording starts at the brace's start.
        enabled: bool,
    },
    /// Switches Punch-Out on or off: with it on, recording into the
    /// Arrangement ends at the loop brace's end, whether or not the Loop
    /// switch is on.
    SetPunchOut {
        /// Whether recording ends at the brace's end.
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
    /// Switches MIDI Arrangement Overdub on or off.
    SetMidiOverdub {
        /// When on, MIDI recorded into the Arrangement merges into the clips
        /// already there instead of replacing them.
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
    /// Gives a scene a tempo to set when it is launched, from 20 to 999
    /// BPM, or switches it off with `None` (WP-3.5).
    SetSceneTempo {
        /// The scene.
        scene: SceneId,
        /// The tempo, or `None` for none of its own.
        tempo: Option<Bpm>,
    },
    /// Gives a scene a time signature to set when it is launched, or
    /// switches it off with `None` (WP-3.5).
    SetSceneTimeSignature {
        /// The scene.
        scene: SceneId,
        /// The signature, or `None` for none of its own.
        signature: Option<TimeSignature>,
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
    /// Sets the value of one of a track's automatable parameters at a
    /// position on its envelope, adding a breakpoint there or replacing the
    /// one that's there, as clicking or dragging a breakpoint does. A
    /// parameter with no envelope gets its first breakpoint, at that value,
    /// and its envelope drives it from then on.
    SetAutomationBreakpoint {
        /// The track the parameter belongs to.
        track: TrackId,
        /// The parameter: a mixer control, a send, or a device parameter.
        target: AutomationTarget,
        /// Where the breakpoint sits: 0 (1.1.1) or later.
        at: Ticks,
        /// The value there, in the parameter's own units.
        value: f64,
    },
    /// Curves the segment starting at one of a track's automation
    /// breakpoints, as holding Alt (Windows) or Option (macOS) while
    /// dragging a line segment on an automation lane does: the value between
    /// that breakpoint and the next follows a power curve shaped by `bend`.
    /// A bend of 0 makes the segment a straight line again, as the same drag
    /// with a double-click does.
    SetAutomationCurve {
        /// The track the parameter belongs to.
        track: TrackId,
        /// The parameter.
        target: AutomationTarget,
        /// Where the segment starts: the breakpoint there.
        at: Ticks,
        /// The bend, from −1 to 1: 0 is the straight line, and the sign
        /// chooses which side the segment bows to.
        bend: f64,
    },
    /// Moves one of a track's automation breakpoints to `to` and, with
    /// `value`, to that value: what dragging a breakpoint on an automation
    /// lane does. Without `value` its value stays.
    MoveAutomationBreakpoint {
        /// The track the parameter belongs to.
        track: TrackId,
        /// The parameter.
        target: AutomationTarget,
        /// Where the breakpoint is now.
        at: Ticks,
        /// Where it goes: 0 (1.1.1) or later, and not where another
        /// breakpoint of the same envelope already is.
        to: Ticks,
        /// The value it takes there, in the parameter's own units, or `null`
        /// (or nothing) to keep the value it has.
        #[serde(default)]
        value: Option<f64>,
    },
    /// Deletes one of a track's automation breakpoints, so the segments
    /// either side of it become the one starting at the breakpoint before
    /// it. Deleting the last one takes the parameter's envelope with it.
    DeleteAutomationBreakpoint {
        /// The track the parameter belongs to.
        track: TrackId,
        /// The parameter.
        target: AutomationTarget,
        /// Where the breakpoint is.
        at: Ticks,
    },
    /// Deletes all of a track parameter's automation: its envelope and every
    /// breakpoint on it, as Delete Automation in the control's context menu
    /// does. The control's value stays where it is.
    DeleteAutomation {
        /// The track the parameter belongs to.
        track: TrackId,
        /// The parameter.
        target: AutomationTarget,
    },
    /// Turns every overridden envelope in the song back on, as the Control
    /// Bar's Re-Enable Automation button does: every automated control a
    /// hand move turned off follows its envelope again.
    ReEnableAutomation {},
    /// Turns one parameter's overridden envelope back on, as Re-Enable
    /// Automation in its context menu does.
    ReEnableAutomationFor {
        /// The track the parameter belongs to.
        track: TrackId,
        /// The parameter.
        target: AutomationTarget,
    },
    /// Switches Solo in Place on or off.
    SetSoloInPlace {
        /// Whether soloing a track keeps the return tracks audible.
        enabled: bool,
    },
    /// Sets the Main track's crossfader, from 0 (the A end) through 0.5
    /// (the centre) to 1 (the B end).
    SetCrossfader {
        /// The position: 0 is the A end, 0.5 the centre, 1 the B end.
        #[schemars(range(min = 0.0, max = 1.0))]
        position: f64,
    },
    /// Assigns a track to the crossfader's A or B side, or to neither.
    SetCrossfadeAssign {
        /// The track; every track but Main.
        track: TrackId,
        /// `a`, `b`, or `none`.
        assign: CrossfadeAssign,
    },
    /// Sets where an audio or MIDI track takes its input from: Audio From
    /// or MIDI From, with its channel.
    SetInput {
        /// The track.
        track: TrackId,
        /// The input.
        input: Input,
        /// The MIDI channel to take, 1 to 16, or `null` (or nothing) for
        /// All Channels. Only a MIDI port input has a channel.
        #[serde(default)]
        #[schemars(range(min = 1, max = 16))]
        channel: Option<u8>,
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
    /// Adds a take lane under an audio track's Arrangement lane, after the
    /// lanes it has, holding recorded passes.
    AddTakeLane {
        /// The audio track.
        track: TrackId,
        /// The lane and its passes.
        lane: TakeLane,
    },
    /// Deletes a clip, from the Arrangement or from a track's Session slot.
    DeleteClip {
        /// The clip.
        clip: ClipId,
    },
    /// Moves a clip to a new start on the same or another track, out of the
    /// Session slot it may sit in.
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
    /// Sets a clip's launch mode (WP-3.3).
    SetClipLaunchMode {
        /// The clip.
        clip: ClipId,
        /// What a press and a release of its launch button do.
        mode: LaunchMode,
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
    /// Switches a Session clip's Legato: on, it takes over the play position
    /// of the clip playing on its track when it launches.
    SetClipLegato {
        /// The clip, which sits in a Session slot.
        clip: ClipId,
        /// Whether Legato is on.
        legato: bool,
    },
    /// Sets a Session clip's own launch quantisation.
    SetClipLaunchQuantization {
        /// The clip, which sits in a Session slot.
        clip: ClipId,
        /// The step the clip's launches wait for; `null` (or nothing)
        /// follows the global quantisation, Live's "Global".
        #[serde(default)]
        quantization: Option<LaunchQuantization>,
    },
    /// Sets how much the velocity of the note that launches a Session clip
    /// changes its volume.
    SetClipVelocityAmount {
        /// The clip, which sits in a Session slot.
        clip: ClipId,
        /// 0 for no influence, up to 1 for 100 %.
        #[schemars(range(min = 0.0, max = 1.0))]
        amount: f64,
    },
    /// Sets the value of one of a clip's envelopes at a position, adding a
    /// breakpoint there or replacing the one that's there, as clicking or
    /// dragging a breakpoint in the Envelopes tab does. A parameter with no
    /// clip envelope gets one, linked to the clip, with the breakpoint as
    /// its first.
    SetClipEnvelopeBreakpoint {
        /// The clip.
        clip: ClipId,
        /// The parameter of the clip's track the envelope modulates: a
        /// control of its mixer, one of its sends, or a parameter of a
        /// device in its chain.
        target: AutomationTarget,
        /// Where the breakpoint sits, in ticks from the beginning of the
        /// clip's contents, as a note's start is: 0 or later.
        at: Ticks,
        /// The value there, in the parameter's own units.
        value: f64,
    },
    /// Curves the segment starting at one of a clip envelope's breakpoints,
    /// as holding Alt (Windows) or Option (macOS) while dragging a segment
    /// in the Envelopes tab does: a bend of 0 makes it a straight line
    /// again.
    SetClipEnvelopeCurve {
        /// The clip.
        clip: ClipId,
        /// The parameter of the clip's track.
        target: AutomationTarget,
        /// Where the segment starts: the breakpoint there.
        at: Ticks,
        /// The bend, from −1 to 1: 0 is the straight line, and the sign
        /// chooses which side the segment bows to.
        bend: f64,
    },
    /// Moves one of a clip envelope's breakpoints, keeping its value unless
    /// `value` gives it a new one, as dragging it in the Envelopes tab does.
    MoveClipEnvelopeBreakpoint {
        /// The clip.
        clip: ClipId,
        /// The parameter of the clip's track.
        target: AutomationTarget,
        /// Where the breakpoint is now.
        at: Ticks,
        /// Where it goes: 0 or later, and not where another breakpoint of
        /// the same envelope already is.
        to: Ticks,
        /// The value it takes there, in the parameter's own units, or `null`
        /// (or nothing) to keep the value it has.
        #[serde(default)]
        value: Option<f64>,
    },
    /// Deletes one of a clip envelope's breakpoints, so the segments either
    /// side of it become the one starting at the breakpoint before it.
    /// Deleting the last one takes the envelope with it.
    DeleteClipEnvelopeBreakpoint {
        /// The clip.
        clip: ClipId,
        /// The parameter of the clip's track.
        target: AutomationTarget,
        /// Where the breakpoint is.
        at: Ticks,
    },
    /// Deletes a clip's envelope for a parameter, as Clear Envelope in the
    /// Envelopes tab does: the parameter goes back to what it does without
    /// the clip envelope. Nothing changes if the clip has no envelope for
    /// it.
    DeleteClipEnvelope {
        /// The clip.
        clip: ClipId,
        /// The parameter of the clip's track.
        target: AutomationTarget,
    },
    /// Switches a clip envelope's Linked switch: linked, it follows the
    /// clip's region and loop; unlinked, it plays once from the clip's
    /// start. A parameter with no clip envelope gets an empty linked one.
    SetClipEnvelopeLinked {
        /// The clip.
        clip: ClipId,
        /// The parameter of the clip's track.
        target: AutomationTarget,
        /// Whether the envelope follows the clip's timeline.
        linked: bool,
    },

    /// Inserts a new empty scene at `index` among the scenes: it holds an
    /// empty slot in every audio and MIDI track. A new set opens with eight
    /// scenes (the layout reference's default state).
    CreateScene {
        /// The new scene's ID.
        id: SceneId,
        /// Where it goes among the scenes, from 0 to the number of scenes.
        index: usize,
    },
    /// Deletes a scene with the clips in its slots. The clips in the other
    /// scenes stay where they are.
    DeleteScene {
        /// The scene.
        scene: SceneId,
    },
    /// Moves a scene to `index` among the scenes, with its clips. `index`
    /// counts positions after the scene is taken out of its current place.
    MoveScene {
        /// The scene.
        scene: SceneId,
        /// Where it goes among the scenes.
        index: usize,
    },
    /// Copies a scene and the clips in its slots into a new scene directly
    /// after it. `clips` lists every track that holds a clip in the scene,
    /// each with the ID its copy takes: a copy is an independent clip. A
    /// scene whose slots are all empty copies no clips, so `clips` is empty.
    DuplicateScene {
        /// The scene.
        scene: SceneId,
        /// The new scene's ID.
        id: SceneId,
        /// Each track that holds a clip in the scene, with the new ID its
        /// copy takes, each track once.
        clips: Vec<(TrackId, ClipId)>,
    },
    /// Renames a scene. A name may be empty: a scene without a name shows
    /// its number, which is its position.
    RenameScene {
        /// The scene.
        scene: SceneId,
        /// The new name.
        name: String,
    },

    /// Puts a clip in a track's Session slot at `scene`: what Insert Empty
    /// MIDI Clip (Cmd+Shift+M) and a clip dropped in a slot do. Only audio
    /// and MIDI tracks hold slot clips, the slot must be empty, and, since a
    /// slot has no place in the Arrangement, the clip's `start` must be 0.
    /// Launching a slot clip is WP-3.2's.
    CreateSlotClip {
        /// The track.
        track: TrackId,
        /// The scene whose slot it goes in.
        scene: SceneId,
        /// The clip, with a new ID.
        clip: Clip,
    },
    /// Moves a clip into a track's Session slot at `scene`, out of the slot
    /// or the Arrangement lane it sits in. The target slot must be empty,
    /// unless it's the clip's own slot.
    MoveSlotClip {
        /// The clip.
        clip: ClipId,
        /// The track whose slot it goes in.
        track: TrackId,
        /// The scene whose slot it goes in.
        scene: SceneId,
    },
    /// Adds or removes a slot's Clip Stop button (WP-3.2): the square that
    /// stops the track's playing clip (S-SES-005, M-EDIT-032).
    SetSlotStopButton {
        /// The track whose slot it is.
        track: TrackId,
        /// The scene whose slot it is.
        scene: SceneId,
        /// Whether the slot keeps its Clip Stop button: `true` adds it, and
        /// `false` removes it.
        stop_button: bool,
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

    /// Inserts a new device at `index` in a rack's chain, keeping the chain
    /// in order for its kind of rack (WP-5.1). The chain may be in a rack
    /// at any depth.
    InsertChainDevice {
        /// The chain.
        chain: ChainId,
        /// Where it goes in the chain, from 0 to the chain's length.
        index: usize,
        /// The device, with a new ID.
        device: Device,
    },
    /// Moves a device to `index` in a rack's chain, from a track's chain or
    /// another chain, keeping its settings and running state. `index`
    /// counts positions after the device is taken out of its current place.
    /// A rack can't be moved into its own chains.
    MoveDeviceToChain {
        /// The device.
        device: DeviceId,
        /// The chain it moves to.
        chain: ChainId,
        /// Where it goes in that chain.
        index: usize,
    },
    /// Wraps devices of one chain in a new rack (Group, Cmd-G): the rack
    /// takes the first device's place, holding one chain that holds the
    /// devices in their order, each keeping its settings and running state.
    /// The chain they were in must stay in order for its kind.
    GroupDevices {
        /// The devices, all in one chain of a track or a rack. Not empty.
        devices: Vec<DeviceId>,
        /// The new rack, with a new ID, of a kind that can hold the devices,
        /// with exactly one chain, with a new ID and no devices.
        rack: Device,
    },
    /// Takes a rack with one chain, or none, apart (Ungroup, Cmd-Shift-G):
    /// the chain's devices take the rack's place in order, keeping their
    /// settings and running state, and the rack, its chain and its mixer
    /// go. A rack with no chain is just deleted.
    UngroupDevice {
        /// The rack.
        device: DeviceId,
    },
    /// Switches a device and every device inside it, at any depth, on or
    /// off: Option-click on a rack's Device Activator.
    SetDeviceEnabledAll {
        /// The device, usually a rack.
        device: DeviceId,
        /// Whether they're on.
        enabled: bool,
    },
    /// Adds a chain at `index` in a rack's list of chains, empty or holding
    /// devices.
    AddChain {
        /// The rack.
        rack: DeviceId,
        /// Where the chain goes in the rack's list, from 0 to its length.
        index: usize,
        /// The chain, with a new ID, and new IDs for its devices. An empty
        /// `name` becomes its first device's title, or `Chain` with none.
        chain: Chain,
    },
    /// Deletes a chain from its rack, with its devices.
    RemoveChain {
        /// The chain.
        chain: ChainId,
    },
    /// Moves a chain to `index` in a rack's list of chains, within its rack
    /// or into another, keeping its devices' settings and running state.
    /// `index` counts positions after the chain is taken out of its
    /// current place. A chain can't go into a rack it holds.
    MoveChain {
        /// The chain.
        chain: ChainId,
        /// The rack it moves to.
        rack: DeviceId,
        /// Where it goes in the rack's list of chains.
        index: usize,
    },
    /// Renames a chain. An empty name gives it the name a chain has without
    /// one: its first device's title, or `Chain`.
    RenameChain {
        /// The chain.
        chain: ChainId,
        /// The new name.
        name: String,
    },
    /// Colours a chain with a swatch of the palette of 5 rows of 14
    /// (0 to 69), or `null` to clear its colour.
    SetChainColour {
        /// The chain.
        chain: ChainId,
        /// The swatch, or `null` for none.
        colour: Option<u8>,
    },
    /// Sets a chain's info text (Edit Info Text). Empty clears it.
    SetChainInfo {
        /// The chain.
        chain: ChainId,
        /// The text.
        info: String,
    },
    /// Sets a chain's Volume as linear gain (finite, at least 0; 1 is
    /// 0 dB, 0 is −∞ dB). Chains of a MIDI effect rack have none.
    SetChainVolume {
        /// The chain.
        chain: ChainId,
        /// The gain.
        volume: f64,
    },
    /// Sets a chain's Pan from −1 (left) to 1 (right). Chains of a MIDI
    /// effect rack have none.
    SetChainPan {
        /// The chain.
        chain: ChainId,
        /// The position.
        pan: f64,
    },
    /// Switches a chain's Chain Activator: a chain that's off adds nothing
    /// to its rack's output.
    SetChainEnabled {
        /// The chain.
        chain: ChainId,
        /// Whether it's on.
        enabled: bool,
    },
    /// Solos a chain: while any chain of a rack is soloed, the rack's
    /// unsoloed chains add nothing. It leaves every other rack and every
    /// track as it is.
    SetChainSolo {
        /// The chain.
        chain: ChainId,
        /// Whether it's soloed.
        solo: bool,
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
            Self::SetPunchIn { .. } => "Punch-In",
            Self::SetPunchOut { .. } => "Punch-Out",
            Self::SetMetronomeEnabled { .. } => "Metronome",
            Self::SetMetronomeCountIn { .. } => "Metronome Count-In",
            Self::SetMetronomeSound { .. } => "Metronome Sound",
            Self::SetMetronomeRhythm { .. } => "Metronome Rhythm",
            Self::SetMetronomeOnlyWhileRecording { .. } => "Hear Metronome Only While Recording",
            Self::SetMidiOverdub { .. } => "MIDI Arrangement Overdub",
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
            Self::SetAutomationBreakpoint { .. } => "Add Automation Breakpoint",
            Self::SetAutomationCurve { .. } => "Curve Automation Segment",
            Self::MoveAutomationBreakpoint { .. } => "Move Automation Breakpoint",
            Self::DeleteAutomationBreakpoint { .. } => "Delete Automation Breakpoint",
            Self::DeleteAutomation { .. } => "Delete Automation",
            Self::ReEnableAutomation { .. } | Self::ReEnableAutomationFor { .. } => {
                "Re-Enable Automation"
            }
            Self::SetCrossfader { .. } => "Change crossfader",
            Self::SetCrossfadeAssign { .. } => "Assign crossfade",
            Self::SetSoloInPlace { .. } => "Switch Solo in Place",
            Self::SetInput { .. }
            | Self::SetOutput { .. }
            | Self::SetMidiOutput { .. }
            | Self::SetMainOut { .. }
            | Self::SetCueOut { .. } => "Change \"Routing\"",
            Self::SetMonitor { .. } => "Change monitoring",
            Self::CreateClip { .. } => "Create clip",
            Self::AddTakeLane { .. } => "Add take lane",
            Self::DeleteClip { .. } => "Delete clip",
            Self::MoveClip { .. } => "Move clip",
            Self::MoveClips { .. } => "Move clips",
            Self::SetClipLength { .. } => "Resize clip",
            Self::RenameClip { .. } => "Rename clip",
            Self::SetClipMute { .. } => "Mute clip",
            Self::SetClipLaunchMode { .. } => "Change launch mode",
            Self::SetClipData { .. } => "Edit clip",
            Self::TrimClipStart { .. } => "Resize clip",
            Self::SetClipRegion { .. } => "Change clip region",
            Self::SetClipLoop { .. } => "Change clip loop",
            Self::SetMidiClipRegion { .. } => "Change clip region",
            Self::SetMidiClipLoop { .. } => "Change clip loop",
            Self::SetClipLoopEnabled { .. } => "Switch clip loop",
            Self::SetClipGain { .. } => "Change clip gain",
            Self::SetClipFades { .. } => "Change clip fades",
            // WP-3.4. Live's undo names for the Launch panel's controls are
            // to observe; Beatlayer's name the edit.
            Self::SetClipLegato { .. } => "Switch Legato",
            Self::SetClipLaunchQuantization { .. } => "Change clip quantization",
            Self::SetClipVelocityAmount { .. } => "Change velocity amount",
            // WP-3.15. Live's undo names for clip envelope edits are to
            // observe; Beatlayer's name the edit.
            Self::SetClipEnvelopeBreakpoint { .. } => "Add Envelope Breakpoint",
            Self::SetClipEnvelopeCurve { .. } => "Curve Envelope Segment",
            Self::MoveClipEnvelopeBreakpoint { .. } => "Move Envelope Breakpoint",
            Self::DeleteClipEnvelopeBreakpoint { .. } => "Delete Envelope Breakpoint",
            Self::DeleteClipEnvelope { .. } => "Clear Envelope",
            Self::SetClipEnvelopeLinked { .. } => "Link Envelope",
            Self::CreateScene { .. } => "Create scene",
            Self::DeleteScene { .. } => "Delete scene",
            Self::MoveScene { .. } => "Move scene",
            // WP-3.1. Live's undo name for duplicating a scene is to
            // observe; Beatlayer's names the edit.
            Self::DuplicateScene { .. } => "Duplicate scene",
            Self::RenameScene { .. } => "Rename scene",
            Self::SetSceneTempo { .. } => "Change scene tempo",
            Self::SetSceneTimeSignature { .. } => "Change scene time signature",
            Self::CreateSlotClip { .. } => "Create clip",
            Self::MoveSlotClip { .. } => "Move clip",
            // Spec: launch-quantisation R8. Live 12.4.6's names, from its
            // Edit menu (M-EDIT-032).
            Self::SetSlotStopButton {
                stop_button: true, ..
            } => "Add Stop Button",
            Self::SetSlotStopButton {
                stop_button: false, ..
            } => "Remove Stop Button",
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
            // WP-5.1. Beatlayer's own names, after the device ones.
            Self::InsertChainDevice { .. } => "Insert Device",
            Self::MoveDeviceToChain { .. } => "Move Device",
            Self::GroupDevices { .. } => "Group",
            Self::UngroupDevice { .. } => "Ungroup",
            Self::SetDeviceEnabledAll { .. } => "Change \"Device On\"",
            Self::AddChain { .. } => "Insert Chain",
            Self::RemoveChain { .. } => "Delete Chain",
            Self::MoveChain { .. } => "Move Chain",
            Self::RenameChain { .. } => "Change \"Chain Name\"",
            Self::SetChainColour { .. } => "Change Chain Color",
            Self::SetChainInfo { .. } => "Change Chain Info Text",
            Self::SetChainVolume { .. } => "Change chain volume",
            Self::SetChainPan { .. } => "Change chain pan",
            Self::SetChainEnabled { .. } => "Change \"Chain On\"",
            Self::SetChainSolo { .. } => "Solo chain",
        }
    }

    /// The IDs of the objects this command creates, in the order it creates
    /// them: the track, clip, device, scene, or clip copies.
    #[must_use]
    pub fn created_ids(&self) -> Vec<Id> {
        match self {
            Self::CreateTrack { id, .. } => vec![id.0],
            Self::CreateClip { clip, .. } => vec![clip.id.0],
            Self::CreateSlotClip { clip, .. } => vec![clip.id.0],
            Self::InsertDevice { device, .. } | Self::InsertChainDevice { device, .. } => {
                device.ids()
            }
            Self::AddChain { chain, .. } => chain.ids(),
            Self::GroupDevices { rack, .. } => rack.ids(),
            Self::CreateScene { id, .. } => vec![id.0],
            Self::DuplicateScene { id, clips, .. } => {
                let mut ids = vec![id.0];
                ids.extend(clips.iter().map(|(_, copy)| copy.0));
                ids
            }
            _ => Vec::new(),
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
            Command::SetPunchIn { enabled } => {
                self.settings.punch_in = *enabled;
                Ok(())
            }
            Command::SetPunchOut { enabled } => {
                self.settings.punch_out = *enabled;
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
            Command::SetMidiOverdub { enabled } => {
                self.settings.midi_overdub = *enabled;
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
                    let track_ref = self.track_mut(*track)?;
                    track_ref.mixer.volume = *volume;
                    override_target(
                        &mut track_ref.automation,
                        &AutomationTarget::Mixer {
                            param: MixerParam::Volume,
                        },
                    );
                }
                Ok(())
            }
            Command::SetPan { track, pan } => {
                check_value("pan", *pan, (-1.0..=1.0).contains(pan))?;
                if self.track_ref(*track)?.mixer.pan != *pan {
                    let track_ref = self.track_mut(*track)?;
                    track_ref.mixer.pan = *pan;
                    override_target(
                        &mut track_ref.automation,
                        &AutomationTarget::Mixer {
                            param: MixerParam::Pan,
                        },
                    );
                }
                Ok(())
            }
            Command::SetMute { track, mute } => {
                let current = self.track_ref(*track)?;
                check_kind(current, TrackKind::has_mute_and_solo, NOT_MAIN)?;
                if current.mixer.mute != *mute {
                    let track_ref = self.track_mut(*track)?;
                    track_ref.mixer.mute = *mute;
                    override_target(
                        &mut track_ref.automation,
                        &AutomationTarget::Mixer {
                            param: MixerParam::Mute,
                        },
                    );
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
                    override_target(
                        &mut self.track_mut(*track)?.automation,
                        &AutomationTarget::Send { to: *to },
                    );
                }
                Ok(())
            }
            Command::SetAutomationBreakpoint {
                track,
                target,
                at,
                value,
            } => self.set_automation_breakpoint(*track, *target, *at, *value),
            Command::SetAutomationCurve {
                track,
                target,
                at,
                bend,
            } => self.set_automation_curve(*track, *target, *at, *bend),
            Command::MoveAutomationBreakpoint {
                track,
                target,
                at,
                to,
                value,
            } => self.move_automation_breakpoint(*track, *target, *at, *to, *value),
            Command::DeleteAutomationBreakpoint { track, target, at } => {
                self.delete_automation_breakpoint(*track, *target, *at)
            }
            Command::DeleteAutomation { track, target } => {
                self.check_automation_target(*track, *target)?;
                self.track_mut(*track)?.automation.remove(target);
                Ok(())
            }
            Command::ReEnableAutomation {} => {
                for id in self.track_order() {
                    let track = self.track_mut(id)?;
                    let overridden: Vec<AutomationTarget> = track
                        .automation
                        .iter()
                        .filter(|(_, automation)| !automation.enabled)
                        .map(|(target, _)| *target)
                        .collect();
                    for target in overridden {
                        if let Some(automation) = track.automation.get_mut(&target) {
                            automation.enabled = true;
                        }
                    }
                }
                Ok(())
            }
            Command::ReEnableAutomationFor { track, target } => {
                self.check_automation_target(*track, *target)?;
                if let Some(automation) = self.track_mut(*track)?.automation.get_mut(target) {
                    automation.enabled = true;
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
            Command::SetCrossfader { position } => {
                check_crossfader(*position)?;
                self.settings.crossfader = *position;
                Ok(())
            }
            Command::SetCrossfadeAssign { track, assign } => {
                let current = self.track_ref(*track)?;
                check_kind(current, TrackKind::has_crossfade_assign, NOT_MAIN)?;
                if current.mixer.crossfade != *assign {
                    self.track_mut(*track)?.mixer.crossfade = *assign;
                }
                Ok(())
            }
            Command::SetInput {
                track,
                input,
                channel,
            } => {
                self.check_instrument_source(*track, input)?;
                let input = input.clone();
                let channel = *channel;
                self.edit_routing(*track, |routing| {
                    routing.input = input;
                    routing.channel = channel;
                })
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
            Command::AddTakeLane { track, lane } => self.add_take_lane(*track, lane),
            Command::DeleteClip { clip } => self.remove_clip(*clip),
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
            Command::SetClipLaunchMode { clip, mode } => {
                self.edit_launch(*clip, |launch| launch.mode = *mode)
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
            Command::SetClipLegato { clip, legato } => {
                self.edit_launch(*clip, |launch| launch.legato = *legato)
            }
            Command::SetClipLaunchQuantization { clip, quantization } => {
                self.edit_launch(*clip, |launch| launch.quantization = *quantization)
            }
            Command::SetClipVelocityAmount { clip, amount } => {
                if !(0.0..=1.0).contains(amount) {
                    return Err(ModelError::InvalidInput(
                        "the velocity amount is from 0 to 1",
                    ));
                }
                self.edit_launch(*clip, |launch| launch.velocity_amount = *amount)
            }
            Command::SetClipEnvelopeBreakpoint {
                clip,
                target,
                at,
                value,
            } => self.set_clip_envelope_breakpoint(*clip, *target, *at, *value),
            Command::SetClipEnvelopeCurve {
                clip,
                target,
                at,
                bend,
            } => self.set_clip_envelope_curve(*clip, *target, *at, *bend),
            Command::MoveClipEnvelopeBreakpoint {
                clip,
                target,
                at,
                to,
                value,
            } => self.move_clip_envelope_breakpoint(*clip, *target, *at, *to, *value),
            Command::DeleteClipEnvelopeBreakpoint { clip, target, at } => {
                self.delete_clip_envelope_breakpoint(*clip, *target, *at)
            }
            Command::DeleteClipEnvelope { clip, target } => {
                self.delete_clip_envelope(*clip, *target)
            }
            Command::SetClipEnvelopeLinked {
                clip,
                target,
                linked,
            } => self.set_clip_envelope_linked(*clip, *target, *linked),
            Command::CreateScene { id, index } => self.create_scene(*id, *index),
            Command::DeleteScene { scene } => self.delete_scene(*scene),
            Command::MoveScene { scene, index } => self.move_scene(*scene, *index),
            Command::DuplicateScene { scene, id, clips } => {
                self.duplicate_scene(*scene, *id, clips)
            }
            Command::RenameScene { scene, name } => self.rename_scene(*scene, name),
            Command::SetSceneTempo { scene, tempo } => self.set_scene_tempo(*scene, *tempo),
            Command::SetSceneTimeSignature { scene, signature } => {
                self.set_scene_signature(*scene, *signature)
            }
            Command::CreateSlotClip { track, scene, clip } => {
                self.create_slot_clip(*track, *scene, clip)
            }
            Command::MoveSlotClip { clip, track, scene } => {
                self.move_slot_clip(*clip, *track, *scene)
            }
            Command::SetSlotStopButton {
                track,
                scene,
                stop_button,
            } => self.set_slot_stop_button(*track, *scene, *stop_button),
            Command::InsertDevice {
                track,
                index,
                device,
            } => self.insert_device(*track, *index, device),
            Command::RemoveDevice { device } => {
                let owner = self.device_owner(*device)?;
                let removed = self.take_device(*device)?;
                self.index.remove_device(&removed);
                self.forget_in(owner, &Gone::device(&removed))?;
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
                    let owner = self.device_owner(*device)?;
                    self.device_mut(*device)?.params.insert(*param, *value);
                    override_target(
                        &mut self.track_mut(owner)?.automation,
                        &AutomationTarget::Device {
                            device: *device,
                            param: *param,
                        },
                    );
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
                let input = input.clone();
                self.edit_sidechain(*device, |s| s.input = input)?;
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
            Command::InsertChainDevice {
                chain,
                index,
                device,
            } => self.insert_chain_device(*chain, *index, device),
            Command::MoveDeviceToChain {
                device,
                chain,
                index,
            } => self.move_device_to_chain(*device, *chain, *index),
            Command::GroupDevices { devices, rack } => self.group_devices(devices, rack),
            Command::UngroupDevice { device } => self.ungroup_device(*device),
            Command::SetDeviceEnabledAll { device, enabled } => {
                if self
                    .device_ref(*device)?
                    .walk()
                    .any(|d| d.enabled != *enabled)
                {
                    self.device_mut(*device)?
                        .walk_mut(&mut |device| device.enabled = *enabled);
                }
                Ok(())
            }
            Command::AddChain { rack, index, chain } => self.add_chain(*rack, *index, chain),
            Command::RemoveChain { chain } => {
                let owner = self.chain_owner(*chain)?;
                let removed = self
                    .rack_holding_mut(*chain)?
                    .remove_chain(*chain)
                    .ok_or(ModelError::NoSuchChain(*chain))?;
                self.index.remove_chain(&removed);
                self.forget_in(owner, &Gone::chain(&removed))?;
                Ok(())
            }
            Command::MoveChain { chain, rack, index } => self.move_chain(*chain, *rack, *index),
            Command::RenameChain { chain, name } => {
                let current = self.chain_ref(*chain)?;
                let name = if name.is_empty() {
                    default_chain_name(current)
                } else {
                    name.clone()
                };
                if current.name != name {
                    self.chain_mut(*chain)?.name = name;
                }
                Ok(())
            }
            Command::SetChainColour { chain, colour } => {
                if let Some(colour) = colour {
                    check_value("chain colour", f64::from(*colour), *colour < CHAIN_COLOURS)?;
                }
                if self.chain_ref(*chain)?.colour != *colour {
                    self.chain_mut(*chain)?.colour = *colour;
                }
                Ok(())
            }
            Command::SetChainInfo { chain, info } => {
                if self.chain_ref(*chain)?.info != *info {
                    self.chain_mut(*chain)?.info.clone_from(info);
                }
                Ok(())
            }
            Command::SetChainVolume { chain, volume } => {
                check_value("volume", *volume, volume.is_finite() && *volume >= 0.0)?;
                self.edit_chain_mixer(*chain, MixerParam::Volume, true, |m| {
                    let changed = m.volume != *volume;
                    m.volume = *volume;
                    changed
                })
            }
            Command::SetChainPan { chain, pan } => {
                check_value("pan", *pan, (-1.0..=1.0).contains(pan))?;
                self.edit_chain_mixer(*chain, MixerParam::Pan, true, |m| {
                    let changed = m.pan != *pan;
                    m.pan = *pan;
                    changed
                })
            }
            Command::SetChainEnabled { chain, enabled } => {
                self.edit_chain_mixer(*chain, MixerParam::Mute, false, |m| {
                    let changed = m.enabled != *enabled;
                    m.enabled = *enabled;
                    changed
                })
            }
            Command::SetChainSolo { chain, solo } => {
                if self.chain_ref(*chain)?.mixer.solo != *solo {
                    self.chain_mut(*chain)?.mixer.solo = *solo;
                }
                Ok(())
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
            .clone()
            .ok_or(ModelError::NoSidechain(id))?;
        edit(&mut sidechain);
        if Some(&sidechain) != self.device_ref(id)?.sidechain.as_ref() {
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
            for clip in track.slots.values() {
                self.index.clip_track.remove(&clip.id);
            }
            for device in &track.devices {
                self.index.remove_device(device);
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
        let mut routing = self.track_ref(id)?.routing.clone();
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
        let mut routing = self.track_ref(main)?.routing.clone();
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
        self.check_clip_envelopes(track, clip)?;
        self.track_mut(track)?
            .clips
            .insert(clip.id, Shared::new(clip.clone()));
        self.index.clip_track.insert(clip.id, track);
        Ok(())
    }

    fn add_take_lane(&mut self, track: TrackId, lane: &TakeLane) -> Result<(), ModelError> {
        check_kind(
            self.track_ref(track)?,
            |kind| kind == TrackKind::Audio,
            "an audio track",
        )?;
        check_take_lane(lane)?;
        self.track_mut(track)?.take_lanes.push_back(lane.clone());
        Ok(())
    }

    fn move_clip(&mut self, id: ClipId, track: TrackId, start: Ticks) -> Result<(), ModelError> {
        let owner = self.clip_owner(id)?;
        let clip = self.clip_ref(id)?;
        check_clip_fits(self.track_ref(track)?, &clip.data)?;
        check_extent(start, clip.length)?;
        // A clip already on the track's Arrangement lane just moves; one in
        // a Session slot leaves the slot for the lane.
        if owner == track && self.track_ref(owner)?.clips.contains_key(&id) {
            if clip.start != start {
                self.clip_mut(id)?.start = start;
            }
            return Ok(());
        }
        let mut moved = clip.clone();
        moved.start = start;
        // An envelope names a parameter of the track the clip is on, so one
        // moving to another track keeps only those its new track has
        // (WP-3.15).
        if owner != track {
            self.keep_fitting_envelopes(track, &mut moved);
        }
        self.remove_clip(id)?;
        self.track_mut(track)?.clips.insert(id, Shared::new(moved));
        self.index.clip_track.insert(id, track);
        Ok(())
    }

    /// Drops from `clip` the envelopes naming parameters `track` doesn't
    /// have: what moving a clip to another track does, since a clip envelope
    /// names a parameter of the track it's on (WP-3.15, as automation R11
    /// drops a device's envelopes when it leaves a track).
    fn keep_fitting_envelopes(&self, track: TrackId, clip: &mut Clip) {
        clip.envelopes
            .retain(|envelope| self.check_automation_target(track, envelope.target).is_ok());
    }

    /// Removes a clip from the Arrangement lane or the Session slot it sits
    /// in, and from the lookups.
    fn remove_clip(&mut self, id: ClipId) -> Result<(), ModelError> {
        let owner = self.clip_owner(id)?;
        if self.track_ref(owner)?.clips.contains_key(&id) {
            self.track_mut(owner)?.clips.remove(&id);
        } else {
            let scene = self
                .track_ref(owner)?
                .slots
                .iter()
                .find(|(_, clip)| clip.id == id)
                .map(|(&scene, _)| scene)
                .ok_or(ModelError::NoSuchClip(id))?;
            self.track_mut(owner)?.slots.remove(&scene);
        }
        self.index.clip_track.remove(&id);
        Ok(())
    }

    fn create_scene(&mut self, id: SceneId, index: usize) -> Result<(), ModelError> {
        self.check_fresh(id.0)?;
        insert_at(&mut self.scenes, index, Scene::new(id))
    }

    /// Deletes a scene, and the clips in its slots with it. The clips in
    /// the other scenes stay.
    fn delete_scene(&mut self, id: SceneId) -> Result<(), ModelError> {
        let position = self.scene_position(id).ok_or(ModelError::NoSuchScene(id))?;
        self.scenes.remove(position);
        // Only the tracks that hold a clip in the scene change.
        let holders: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|(_, track)| track.slots.contains_key(&id))
            .map(|(&track, _)| track)
            .collect();
        for track in holders {
            let clip = self
                .track_mut(track)?
                .slots
                .remove(&id)
                .expect("just checked the slot holds a clip");
            self.index.clip_track.remove(&clip.id);
        }
        // A slot's removed Clip Stop button goes with its scene too (WP-3.2):
        // a set names no stop-button state for a scene that's gone, so the
        // tracks that only remember a removal change as well.
        let buttons: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|(_, track)| track.removed_stop_buttons.contains(&id))
            .map(|(&track, _)| track)
            .collect();
        for track in buttons {
            self.track_mut(track)?.removed_stop_buttons.remove(&id);
        }
        Ok(())
    }

    /// Moves a scene to `index` among the scenes, counting positions after
    /// it is taken out. Its clips stay in its slots, which are keyed by the
    /// scene's ID, so they move with it.
    fn move_scene(&mut self, id: SceneId, index: usize) -> Result<(), ModelError> {
        let position = self.scene_position(id).ok_or(ModelError::NoSuchScene(id))?;
        if position == index {
            return Ok(());
        }
        let scene = self.scenes.remove(position);
        insert_at(&mut self.scenes, index, scene)
    }

    fn rename_scene(&mut self, id: SceneId, name: &str) -> Result<(), ModelError> {
        let position = self.scene_position(id).ok_or(ModelError::NoSuchScene(id))?;
        if let Some(scene) = self.scenes.get_mut(position)
            && scene.name != *name
        {
            scene.name = name.to_owned();
        }
        Ok(())
    }

    fn set_scene_tempo(&mut self, id: SceneId, tempo: Option<Bpm>) -> Result<(), ModelError> {
        let position = self.scene_position(id).ok_or(ModelError::NoSuchScene(id))?;
        if let Some(bpm) = tempo {
            let range = SCENE_TEMPO_MIN..=SCENE_TEMPO_MAX;
            check_value("scene tempo", bpm.get(), range.contains(&bpm.get()))?;
        }
        if let Some(scene) = self.scenes.get_mut(position)
            && scene.tempo != tempo
        {
            scene.tempo = tempo;
        }
        Ok(())
    }

    fn set_scene_signature(
        &mut self,
        id: SceneId,
        signature: Option<TimeSignature>,
    ) -> Result<(), ModelError> {
        let position = self.scene_position(id).ok_or(ModelError::NoSuchScene(id))?;
        if let Some(scene) = self.scenes.get_mut(position)
            && scene.signature != signature
        {
            scene.signature = signature;
        }
        Ok(())
    }

    /// Copies a scene and its clips straight after it: one fresh scene, and
    /// an independent copy of each clip the scene holds, under the IDs the
    /// caller lists, every track that holds one listed once.
    fn duplicate_scene(
        &mut self,
        id: SceneId,
        copy: SceneId,
        clips: &[(TrackId, ClipId)],
    ) -> Result<(), ModelError> {
        let position = self.scene_position(id).ok_or(ModelError::NoSuchScene(id))?;
        self.check_fresh(copy.0)?;
        let mut copies = Vec::with_capacity(clips.len());
        let mut tracks = BTreeSet::new();
        for &(track, clip) in clips {
            if !tracks.insert(track) {
                return Err(ModelError::DuplicateId(track.0));
            }
            let source = self
                .track_ref(track)?
                .slot(id)
                .ok_or(ModelError::SceneCopyMismatch { scene: id, track })?
                .clone();
            self.check_fresh(clip.0)?;
            copies.push((track, clip, source));
        }
        for track in self.tracks.values() {
            if track.slots.contains_key(&id) && !tracks.contains(&track.id) {
                return Err(ModelError::SceneCopyMismatch {
                    scene: id,
                    track: track.id,
                });
            }
        }
        let mut copies_seen: BTreeSet<Id> = BTreeSet::new();
        copies_seen.insert(copy.0);
        for &(_, clip, _) in &copies {
            if !copies_seen.insert(clip.0) {
                return Err(ModelError::DuplicateId(clip.0));
            }
        }
        let original = self
            .scene(id)
            .expect("just checked the scene exists")
            .clone();
        for (track, clip, source) in copies {
            let copied = Clip {
                id: clip,
                start: Ticks::ZERO,
                ..source
            };
            self.track_mut(track)?
                .slots
                .insert(copy, Shared::new(copied));
            self.index.clip_track.insert(clip, track);
        }
        insert_at(
            &mut self.scenes,
            position + 1,
            Scene {
                id: copy,
                ..original
            },
        )
    }

    fn create_slot_clip(
        &mut self,
        track: TrackId,
        scene: SceneId,
        clip: &Clip,
    ) -> Result<(), ModelError> {
        self.check_fresh(clip.id.0)?;
        let target = self.track_ref(track)?;
        check_kind(target, TrackKind::holds_clips, AUDIO_OR_MIDI)?;
        self.scene_position(scene)
            .ok_or(ModelError::NoSuchScene(scene))?;
        check_clip_fits(target, &clip.data)?;
        if clip.start != Ticks::ZERO {
            return Err(ModelError::SlotClipPosition(clip.start));
        }
        check_extent(clip.start, clip.length)?;
        check_clip(clip)?;
        self.check_clip_envelopes(track, clip)?;
        if target.slot(scene).is_some() {
            return Err(ModelError::SlotOccupied { track, scene });
        }
        self.track_mut(track)?
            .slots
            .insert(scene, Shared::new(clip.clone()));
        self.index.clip_track.insert(clip.id, track);
        Ok(())
    }

    /// Moves a clip into a slot, out of the slot or lane it sits in. A slot
    /// clip has no position, so the clip arrives with its start at 0.
    fn move_slot_clip(
        &mut self,
        id: ClipId,
        track: TrackId,
        scene: SceneId,
    ) -> Result<(), ModelError> {
        self.clip_owner(id)?;
        let target = self.track_ref(track)?;
        check_kind(target, TrackKind::holds_clips, AUDIO_OR_MIDI)?;
        self.scene_position(scene)
            .ok_or(ModelError::NoSuchScene(scene))?;
        let current = self.clip_ref(id)?.clone();
        check_clip_fits(target, &current.data)?;
        if target.slot(scene).is_some_and(|held| held.id != id) {
            return Err(ModelError::SlotOccupied { track, scene });
        }
        self.remove_clip(id)?;
        let mut moved = Clip {
            start: Ticks::ZERO,
            ..current
        };
        // An envelope names a parameter of the track the clip is on, so one
        // moving to another track keeps only those its new track has
        // (WP-3.15).
        self.keep_fitting_envelopes(track, &mut moved);
        self.track_mut(track)?
            .slots
            .insert(scene, Shared::new(moved));
        self.index.clip_track.insert(id, track);
        Ok(())
    }

    /// Adds or removes a slot's Clip Stop button (WP-3.2, S-SES-005): the
    /// square that stops the track's playing clip. Only audio and MIDI
    /// tracks hold clips, so only they have slots with stop buttons.
    fn set_slot_stop_button(
        &mut self,
        track: TrackId,
        scene: SceneId,
        stop_button: bool,
    ) -> Result<(), ModelError> {
        let target = self.track_ref(track)?;
        check_kind(target, TrackKind::holds_clips, AUDIO_OR_MIDI)?;
        self.scene_position(scene)
            .ok_or(ModelError::NoSuchScene(scene))?;
        let removed = &mut self.track_mut(track)?.removed_stop_buttons;
        if stop_button {
            removed.remove(&scene);
        } else {
            removed.insert(scene);
        }
        Ok(())
    }

    fn move_clips(&mut self, clips: &[ClipId], by: Ticks) -> Result<(), ModelError> {
        let mut seen = BTreeSet::new();
        let mut moves = Vec::with_capacity(clips.len());
        for &id in clips {
            if !seen.insert(id) {
                return Err(ModelError::DuplicateId(id.0));
            }
            if !self.in_arrangement(id)? {
                return Err(ModelError::ClipInSlot(id));
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
        if !self.in_arrangement(id)? {
            return Err(ModelError::ClipInSlot(id));
        }
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

    /// Changes the launch settings of the Session clip `id`.
    fn edit_launch(
        &mut self,
        id: ClipId,
        edit: impl FnOnce(&mut ClipLaunch),
    ) -> Result<(), ModelError> {
        let owner = self.clip_owner(id)?;
        if !self
            .track_ref(owner)?
            .slots()
            .any(|(_, clip)| clip.id == id)
        {
            return Err(ModelError::NotSlotClip(id));
        }
        let mut next = self.clip_ref(id)?.launch;
        edit(&mut next);
        if next != self.clip_ref(id)?.launch {
            self.clip_mut(id)?.launch = next;
        }
        Ok(())
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

    /// Checks that `target` names a parameter of the clip's track: a control
    /// of its mixer, one of its sends, or a parameter of a device in its
    /// chain (WP-3.15).
    fn check_clip_envelope_target(
        &self,
        clip: ClipId,
        target: AutomationTarget,
    ) -> Result<(), ModelError> {
        let track = self.clip_owner(clip)?;
        self.check_automation_target(track, target)
    }

    /// Sets the value of `target` at `at` on the clip's envelope for it,
    /// adding a breakpoint there or replacing the one that's there, and
    /// giving the clip that envelope, linked, if it has none.
    fn set_clip_envelope_breakpoint(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        at: Ticks,
        value: f64,
    ) -> Result<(), ModelError> {
        if at < Ticks::ZERO {
            return Err(ModelError::NegativePosition(at));
        }
        check_value("clip envelope value", value, value.is_finite())?;
        self.check_clip_envelope_target(clip, target)?;
        clip_entry(&mut self.clip_mut(clip)?.envelopes, target)
            .envelope
            .set(at, value);
        Ok(())
    }

    /// Sets the bend of the segment starting at `at` on the clip's envelope
    /// for `target`, leaving the breakpoints where they are.
    fn set_clip_envelope_curve(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        at: Ticks,
        bend: f64,
    ) -> Result<(), ModelError> {
        check_bend(bend)?;
        self.check_clip_envelope_target(clip, target)?;
        let envelope = self
            .clip_ref(clip)?
            .envelope(&target)
            .ok_or(ModelError::NoChange {
                what: "clip envelope",
                at,
            })?;
        let mut envelope = envelope.envelope.clone();
        envelope
            .set_bend(at, bend)
            .map_err(automation_point_error)?;
        self.clip_envelope_mut(clip, target)?.envelope = envelope;
        Ok(())
    }

    /// Moves the breakpoint at `at` on the clip's envelope for `target` to
    /// `to`, keeping its value unless `value` gives it another.
    fn move_clip_envelope_breakpoint(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        at: Ticks,
        to: Ticks,
        value: Option<f64>,
    ) -> Result<(), ModelError> {
        if to < Ticks::ZERO {
            return Err(ModelError::NegativePosition(to));
        }
        if let Some(value) = value {
            check_value("clip envelope value", value, value.is_finite())?;
        }
        self.check_clip_envelope_target(clip, target)?;
        let envelope = self
            .clip_ref(clip)?
            .envelope(&target)
            .ok_or(ModelError::NoChange {
                what: "clip envelope",
                at,
            })?;
        let mut envelope = envelope.envelope.clone();
        envelope
            .move_point(at, to)
            .map_err(automation_point_error)?;
        if let Some(value) = value {
            envelope.set(to, value);
        }
        self.clip_envelope_mut(clip, target)?.envelope = envelope;
        Ok(())
    }

    /// Deletes the breakpoint at `at` on the clip's envelope for `target`.
    /// Deleting the last one takes the envelope with it.
    fn delete_clip_envelope_breakpoint(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        at: Ticks,
    ) -> Result<(), ModelError> {
        self.check_clip_envelope_target(clip, target)?;
        let envelope = self
            .clip_ref(clip)?
            .envelope(&target)
            .ok_or(ModelError::NoChange {
                what: "clip envelope",
                at,
            })?;
        let mut envelope = envelope.envelope.clone();
        envelope.delete(at).map_err(automation_point_error)?;
        if envelope.is_empty() {
            self.delete_clip_envelope(clip, target)
        } else {
            self.clip_envelope_mut(clip, target)?.envelope = envelope;
            Ok(())
        }
    }

    /// Deletes the clip's envelope for `target`, as Clear Envelope does.
    /// Nothing changes if the clip has no envelope for it.
    fn delete_clip_envelope(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
    ) -> Result<(), ModelError> {
        self.check_clip_envelope_target(clip, target)?;
        self.clip_mut(clip)?
            .envelopes
            .retain(|envelope| envelope.target != target);
        Ok(())
    }

    /// Switches the clip's envelope for `target` between linked and
    /// unlinked, giving the clip an empty linked one if it has none.
    fn set_clip_envelope_linked(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        linked: bool,
    ) -> Result<(), ModelError> {
        self.check_clip_envelope_target(clip, target)?;
        clip_entry(&mut self.clip_mut(clip)?.envelopes, target).linked = linked;
        Ok(())
    }

    /// The clip's envelope for `target`, which the command has checked
    /// exists.
    fn clip_envelope_mut(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
    ) -> Result<&mut ClipEnvelope, ModelError> {
        self.clip_mut(clip)?
            .envelopes
            .iter_mut()
            .find(|envelope| envelope.target == target)
            .ok_or(ModelError::NoChange {
                what: "clip envelope",
                at: Ticks::ZERO,
            })
    }

    fn insert_device(
        &mut self,
        track: TrackId,
        index: usize,
        device: &Device,
    ) -> Result<(), ModelError> {
        self.check_all_fresh(device.ids())?;
        check_device(device)?;
        let chain = &mut self.track_mut(track)?.devices;
        insert_at(chain, index, Shared::new(device.clone()))?;
        self.index.add_device(device, track);
        check_chain(self.track_ref(track)?)?;
        self.check_routing(track)
    }

    fn insert_chain_device(
        &mut self,
        chain: ChainId,
        index: usize,
        device: &Device,
    ) -> Result<(), ModelError> {
        let track = self.chain_owner(chain)?;
        self.check_all_fresh(device.ids())?;
        check_device(device)?;
        let devices = &mut self.chain_mut(chain)?.devices;
        check_index(index, devices.len())?;
        devices.insert(index, device.clone());
        self.index.add_device(device, track);
        check_chain(self.track_ref(track)?)?;
        self.check_routing(track)
    }

    fn check_all_fresh(&self, ids: Vec<Id>) -> Result<(), ModelError> {
        ids.into_iter().try_for_each(|id| self.check_fresh(id))
    }

    /// Takes a device out of the chain it sits in, a track's or a rack's,
    /// leaving the lookups and the track's automation as they were.
    fn take_device(&mut self, id: DeviceId) -> Result<Device, ModelError> {
        let owner = self.device_owner(id)?;
        let track = self.track_mut(owner)?;
        if let Some(at) = position_in(&track.devices, |d| d.id == id) {
            return Ok((*track.devices.remove(at)).clone());
        }
        let at = track
            .devices
            .iter()
            .position(|d| d.find(id).is_some())
            .ok_or(ModelError::NoSuchDevice(id))?;
        track
            .devices
            .get_mut(at)
            .map(Shared::make_mut)
            .and_then(|rack| rack.remove_nested(id))
            .ok_or(ModelError::NoSuchDevice(id))
    }

    /// Brings the lookups and the automation up to date with a device that
    /// moved from `from`'s chains to `to`'s.
    fn settle_move(
        &mut self,
        from: TrackId,
        to: TrackId,
        moved: &Device,
    ) -> Result<(), ModelError> {
        if from != to {
            self.index.add_device(moved, to);
            self.forget_in(from, &Gone::device(moved))?;
        }
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
        if owner == track && self.track_ref(owner)?.device_position(id) == Some(index) {
            return Ok(());
        }
        let device = self.take_device(id)?;
        insert_at(
            &mut self.track_mut(track)?.devices,
            index,
            Shared::new(device.clone()),
        )?;
        self.settle_move(owner, track, &device)?;
        check_chain(self.track_ref(track)?)
    }

    /// Puts `device` at `index` in the chain it belongs to: a rack's chain,
    /// or the track's own.
    fn put_device(
        &mut self,
        track: TrackId,
        chain: Option<ChainId>,
        index: usize,
        device: Device,
    ) -> Result<(), ModelError> {
        match chain {
            None => insert_at(
                &mut self.track_mut(track)?.devices,
                index,
                Shared::new(device),
            ),
            Some(chain) => {
                let devices = &mut self.chain_mut(chain)?.devices;
                check_index(index, devices.len())?;
                devices.insert(index, device);
                Ok(())
            }
        }
    }

    fn group_devices(&mut self, devices: &[DeviceId], rack: &Device) -> Result<(), ModelError> {
        let Some(&first) = devices.first() else {
            return Err(ModelError::BadGroup("there are no devices to group"));
        };
        if rack.rack_kind().is_none()
            || rack.chains.len() != 1
            || !rack.chains[0].devices.is_empty()
        {
            return Err(ModelError::BadGroup(
                "the new rack must hold exactly one empty chain",
            ));
        }
        self.check_all_fresh(rack.ids())?;
        check_device(rack)?;
        let place = self
            .device_place(first)
            .ok_or(ModelError::NoSuchDevice(first))?;
        let mut positions = Vec::with_capacity(devices.len());
        for &id in devices {
            let there = self.device_place(id).ok_or(ModelError::NoSuchDevice(id))?;
            if (there.track, there.chain) != (place.track, place.chain) {
                return Err(ModelError::BadGroup(
                    "the devices to group must be in one chain",
                ));
            }
            positions.push((there.index, id));
        }
        positions.sort_unstable();
        positions.dedup();
        if positions.len() != devices.len() {
            return Err(ModelError::BadGroup("a device is listed twice"));
        }
        let mut grouped = rack.clone();
        for &(_, id) in &positions {
            grouped.chains[0].devices.push(self.take_device(id)?);
        }
        self.put_device(place.track, place.chain, positions[0].0, grouped.clone())?;
        self.index.add_device(&grouped, place.track);
        check_chain(self.track_ref(place.track)?)?;
        self.check_routing(place.track)
    }

    fn ungroup_device(&mut self, id: DeviceId) -> Result<(), ModelError> {
        let rack = self.device_ref(id)?;
        if rack.rack_kind().is_none() {
            return Err(ModelError::NotARack(id));
        }
        if rack.chains.len() > 1 {
            return Err(ModelError::BadGroup(
                "a rack of several chains can't be taken apart",
            ));
        }
        let place = self.device_place(id).ok_or(ModelError::NoSuchDevice(id))?;
        let rack = self.take_device(id)?;
        let mut gone = Gone::default();
        gone.devices.insert(id);
        self.index.device_track.remove(&id);
        let mut inside = Vec::new();
        if let Some(chain) = rack.chains.into_iter().next() {
            gone.chains.insert(chain.id);
            self.index.chain_track.remove(&chain.id);
            inside = chain.devices;
        }
        for (offset, device) in inside.into_iter().enumerate() {
            self.put_device(place.track, place.chain, place.index + offset, device)?;
        }
        self.forget_in(place.track, &gone)?;
        check_chain(self.track_ref(place.track)?)
    }

    fn move_device_to_chain(
        &mut self,
        id: DeviceId,
        chain: ChainId,
        index: usize,
    ) -> Result<(), ModelError> {
        let owner = self.device_owner(id)?;
        let track = self.chain_owner(chain)?;
        if self.device_ref(id)?.find_chain(chain).is_some() {
            return Err(ModelError::RackIntoItself(id));
        }
        let device = self.take_device(id)?;
        let devices = &mut self.chain_mut(chain)?.devices;
        check_index(index, devices.len())?;
        devices.insert(index, device.clone());
        self.settle_move(owner, track, &device)?;
        check_chain(self.track_ref(track)?)
    }

    fn add_chain(&mut self, rack: DeviceId, index: usize, chain: &Chain) -> Result<(), ModelError> {
        let owner = self.device_owner(rack)?;
        if self.device_ref(rack)?.rack_kind().is_none() {
            return Err(ModelError::NotARack(rack));
        }
        self.check_all_fresh(chain.ids())?;
        chain.devices.iter().try_for_each(check_device)?;
        let mut chain = chain.clone();
        if chain.name.is_empty() {
            chain.name = default_chain_name(&chain);
        }
        let chains = &mut self.device_mut(rack)?.chains;
        check_index(index, chains.len())?;
        chains.insert(index, chain.clone());
        self.index.add_chain(&chain, owner);
        check_chain(self.track_ref(owner)?)?;
        self.check_routing(owner)
    }

    fn move_chain(&mut self, id: ChainId, rack: DeviceId, index: usize) -> Result<(), ModelError> {
        let from = self.chain_owner(id)?;
        let to = self.device_owner(rack)?;
        if self.device_ref(rack)?.rack_kind().is_none() {
            return Err(ModelError::NotARack(rack));
        }
        if self.chain_ref(id)?.devices_deep().any(|d| d.id == rack) {
            return Err(ModelError::RackIntoItself(rack));
        }
        let moved = self
            .rack_holding_mut(id)?
            .remove_chain(id)
            .ok_or(ModelError::NoSuchChain(id))?;
        let chains = &mut self.device_mut(rack)?.chains;
        check_index(index, chains.len())?;
        chains.insert(index, moved.clone());
        if from != to {
            self.index.add_chain(&moved, to);
            self.forget_in(from, &Gone::chain(&moved))?;
        }
        check_chain(self.track_ref(to)?)
    }

    /// Edits a chain's mixer with `edit`, which says whether it changed
    /// anything; a change moves the control by hand, which turns its
    /// envelope off. `audio`: the control is one a MIDI effect rack's chains
    /// lack.
    fn edit_chain_mixer(
        &mut self,
        chain: ChainId,
        param: MixerParam,
        audio: bool,
        edit: impl FnOnce(&mut crate::rack::ChainMixer) -> bool,
    ) -> Result<(), ModelError> {
        let owner = self.chain_owner(chain)?;
        if audio
            && self
                .chain_rack(chain)
                .and_then(Device::rack_kind)
                .is_some_and(|kind| !kind.chains_have_volume_and_pan())
        {
            return Err(ModelError::ChainHasNoMixer(chain));
        }
        let mut mixer = self.chain_ref(chain)?.mixer;
        if edit(&mut mixer) {
            self.chain_mut(chain)?.mixer = mixer;
            override_target(
                &mut self.track_mut(owner)?.automation,
                &AutomationTarget::Chain { chain, param },
            );
        }
        Ok(())
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

    // A track's automation: one envelope per automatable parameter, as
    // edits on the track's map (WP-2.12).

    /// Checks that `target` names a parameter of `track`: a control of its
    /// mixer, one of its sends to a return track, or a parameter of a device
    /// in its chain.
    pub fn check_automation_target(
        &self,
        track: TrackId,
        target: AutomationTarget,
    ) -> Result<(), ModelError> {
        match target {
            AutomationTarget::Mixer { .. } => {
                self.track_ref(track)?;
            }
            AutomationTarget::Send { to } => {
                check_kind(self.track_ref(track)?, TrackKind::has_sends, SENDER)?;
                check_kind(
                    self.track_ref(to)?,
                    |kind| kind == TrackKind::Return,
                    RETURN,
                )?;
            }
            AutomationTarget::Device { device, .. } => {
                let track_ref = self.track_ref(track)?;
                if track_ref.find_device(device).is_none() {
                    return Err(ModelError::DeviceNotInTrack { device, track });
                }
            }
            AutomationTarget::Chain { chain, param } => {
                let track_ref = self.track_ref(track)?;
                let rack = track_ref
                    .devices
                    .iter()
                    .find_map(|d| d.chain_rack(chain))
                    .ok_or(ModelError::ChainNotInTrack { chain, track })?;
                let no_mixer = rack
                    .rack_kind()
                    .is_some_and(|kind| !kind.chains_have_volume_and_pan());
                if no_mixer && param != MixerParam::Mute {
                    return Err(ModelError::ChainHasNoMixer(chain));
                }
            }
        }
        Ok(())
    }

    /// Sets the value of `target` at `at` on its envelope, adding a
    /// breakpoint there or replacing the one that's there. The envelope
    /// keeps driving the parameter (or starts to, if this is its first
    /// breakpoint): only a hand move or Re-Enable Automation changes that.
    fn set_automation_breakpoint(
        &mut self,
        track: TrackId,
        target: AutomationTarget,
        at: Ticks,
        value: f64,
    ) -> Result<(), ModelError> {
        if at < Ticks::ZERO {
            return Err(ModelError::NegativePosition(at));
        }
        check_value("automation value", value, value.is_finite())?;
        self.check_automation_target(track, target)?;
        entry(&mut self.track_mut(track)?.automation, target)
            .envelope
            .set(at, value);
        Ok(())
    }

    /// Sets the bend of the segment starting at `at` on `target`'s envelope:
    /// 0 makes the segment a straight line again.
    fn set_automation_curve(
        &mut self,
        track: TrackId,
        target: AutomationTarget,
        at: Ticks,
        bend: f64,
    ) -> Result<(), ModelError> {
        check_bend(bend)?;
        self.check_automation_target(track, target)?;
        let automation =
            self.track_mut(track)?
                .automation
                .get_mut(&target)
                .ok_or(ModelError::NoChange {
                    what: "breakpoint",
                    at,
                })?;
        automation
            .envelope
            .set_bend(at, bend)
            .map_err(automation_point_error)
    }

    /// Moves the breakpoint at `at` on `target`'s envelope to `to`, and to
    /// `value` when one is given.
    fn move_automation_breakpoint(
        &mut self,
        track: TrackId,
        target: AutomationTarget,
        at: Ticks,
        to: Ticks,
        value: Option<f64>,
    ) -> Result<(), ModelError> {
        if to < Ticks::ZERO {
            return Err(ModelError::NegativePosition(to));
        }
        if let Some(value) = value {
            check_value("automation value", value, value.is_finite())?;
        }
        self.check_automation_target(track, target)?;
        let automation =
            self.track_mut(track)?
                .automation
                .get_mut(&target)
                .ok_or(ModelError::NoChange {
                    what: "breakpoint",
                    at,
                })?;
        automation
            .envelope
            .move_point(at, to)
            .map_err(automation_point_error)?;
        if let Some(value) = value {
            automation.envelope.set(to, value);
        }
        Ok(())
    }

    /// Deletes the breakpoint at `at` on `target`'s envelope, joining its
    /// neighbours with a straight segment. Deleting the last one takes the
    /// parameter's envelope with it.
    fn delete_automation_breakpoint(
        &mut self,
        track: TrackId,
        target: AutomationTarget,
        at: Ticks,
    ) -> Result<(), ModelError> {
        self.check_automation_target(track, target)?;
        let empty =
            {
                let automation = self.track_mut(track)?.automation.get_mut(&target).ok_or(
                    ModelError::NoChange {
                        what: "breakpoint",
                        at,
                    },
                )?;
                automation
                    .envelope
                    .delete(at)
                    .map_err(automation_point_error)?;
                automation.envelope.is_empty()
            };
        if empty {
            self.track_mut(track)?.automation.remove(&target);
        }
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

    /// Whether a clip sits on its track's Arrangement lane rather than in a
    /// Session slot.
    fn in_arrangement(&self, id: ClipId) -> Result<bool, ModelError> {
        Ok(self
            .track_ref(self.clip_owner(id)?)?
            .clips
            .contains_key(&id))
    }

    fn clip_ref(&self, id: ClipId) -> Result<&Clip, ModelError> {
        self.clip(id).ok_or(ModelError::NoSuchClip(id))
    }

    fn clip_mut(&mut self, id: ClipId) -> Result<&mut Clip, ModelError> {
        let owner = self.clip_owner(id)?;
        let track = self.track_mut(owner)?;
        if track.clips.contains_key(&id) {
            return track
                .clips
                .get_mut(&id)
                .map(Shared::make_mut)
                .ok_or(ModelError::NoSuchClip(id));
        }
        let scene = track
            .slots
            .iter()
            .find(|(_, clip)| clip.id == id)
            .map(|(&scene, _)| scene)
            .ok_or(ModelError::NoSuchClip(id))?;
        track
            .slots
            .get_mut(&scene)
            .map(Shared::make_mut)
            .ok_or(ModelError::NoSuchClip(id))
    }

    /// Forgets what a track held for the devices and chains that left it,
    /// and the inputs that tapped those chains.
    fn forget_in(&mut self, track: TrackId, gone: &Gone) -> Result<(), ModelError> {
        forget(self.track_mut(track)?, gone);
        self.forget_chain_taps(&gone.chains);
        Ok(())
    }

    fn device_owner(&self, id: DeviceId) -> Result<TrackId, ModelError> {
        self.device_track(id).ok_or(ModelError::NoSuchDevice(id))
    }

    fn chain_owner(&self, id: ChainId) -> Result<TrackId, ModelError> {
        self.chain_track(id).ok_or(ModelError::NoSuchChain(id))
    }

    fn chain_ref(&self, id: ChainId) -> Result<&Chain, ModelError> {
        self.chain(id).ok_or(ModelError::NoSuchChain(id))
    }

    /// The chain's track's top-level device that holds the chain, to change
    /// in place.
    fn rack_holding_mut(&mut self, id: ChainId) -> Result<&mut Device, ModelError> {
        let owner = self.chain_owner(id)?;
        let track = self.track_mut(owner)?;
        let at = track
            .devices
            .iter()
            .position(|d| d.find_chain(id).is_some())
            .ok_or(ModelError::NoSuchChain(id))?;
        track
            .devices
            .get_mut(at)
            .map(Shared::make_mut)
            .ok_or(ModelError::NoSuchChain(id))
    }

    fn chain_mut(&mut self, id: ChainId) -> Result<&mut Chain, ModelError> {
        self.rack_holding_mut(id)?
            .find_chain_mut(id)
            .ok_or(ModelError::NoSuchChain(id))
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
        let position = chain
            .iter()
            .position(|d| d.find(id).is_some())
            .ok_or(ModelError::NoSuchDevice(id))?;
        chain
            .get_mut(position)
            .map(Shared::make_mut)
            .and_then(|device| device.find_mut(id))
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

/// An envelope edit's error as the model's, for the command that asked.
fn automation_point_error(error: PointError) -> ModelError {
    match error {
        PointError::Missing(at) => ModelError::NoChange {
            what: "breakpoint",
            at,
        },
        PointError::Taken(at) => ModelError::ChangeInUse {
            what: "breakpoint",
            at,
        },
        PointError::NoSegment(at) => ModelError::NoChange {
            what: "segment",
            at,
        },
    }
}

pub(crate) fn check_value(what: &'static str, value: f64, ok: bool) -> Result<(), ModelError> {
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

/// Checks a take lane's passes: each has a start and length that fit and
/// audio data that follows the audio clips' rules.
pub(crate) fn check_take_lane(lane: &TakeLane) -> Result<(), ModelError> {
    for clip in &lane.clips {
        check_extent(clip.start, clip.length)?;
        check_audio(&clip.data)?;
        check_marker_order(&clip.data)?;
    }
    Ok(())
}

/// Checks a device's parameter values, and, for a rack, its chains and the
/// devices in them.
pub(crate) fn check_device(device: &Device) -> Result<(), ModelError> {
    crate::rack::check_racks(device)?;
    device.walk().try_for_each(check_one_device)
}

fn check_one_device(device: &Device) -> Result<(), ModelError> {
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
    track
        .devices
        .iter()
        .try_for_each(|d| crate::rack::check_racks(d))
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
    if mixer.crossfade != CrossfadeAssign::None {
        check_kind(track, TrackKind::has_crossfade_assign, NOT_MAIN)?;
    }
    Ok(())
}

/// Checks a crossfader position: a finite number from 0 (the A end) to 1
/// (the B end).
pub(crate) fn check_crossfader(position: f64) -> Result<(), ModelError> {
    check_value("crossfader", position, (0.0..=1.0).contains(&position))
}

/// Checks a send's level: above 0 (a send at 0 isn't stored) and at most 1.
pub(crate) fn check_send_level(level: f64) -> Result<(), ModelError> {
    check_value("send level", level, level > 0.0 && level <= 1.0)
}

fn insert_at<T: Clone>(list: &mut Vector<T>, index: usize, value: T) -> Result<(), ModelError> {
    check_index(index, list.len())?;
    list.insert(index, value);
    Ok(())
}

/// The name a chain has without one given (the racks spec, R8): its first
/// device's title, or `Chain` while it holds none.
fn default_chain_name(chain: &Chain) -> String {
    chain
        .devices
        .first()
        .map_or("Chain", |device| device.title())
        .to_owned()
}

fn check_index(index: usize, len: usize) -> Result<(), ModelError> {
    if index > len {
        return Err(ModelError::IndexOutOfRange { index, len });
    }
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
    use crate::automation::Breakpoint;
    use crate::changes::ChangeSet;
    use crate::clip::{EDGE_FADE_SECONDS as EDGE_FADE, MidiData, Note, TakeClip};
    use crate::device::{DeviceKind, DeviceRole, PluginFormat};
    use crate::ids::ContentHash;
    use crate::scene::DEFAULT_SCENE_COUNT;
    use crate::track::Routing;

    /// A document and the allocator for its IDs.
    struct Song {
        doc: Document,
        ids: IdAllocator,
    }

    impl Song {
        fn new() -> Self {
            let mut ids = IdAllocator::new();
            let main = TrackId(ids.allocate());
            let scenes = (0..DEFAULT_SCENE_COUNT).map(|_| SceneId(ids.allocate()));
            let doc = Document::new(main, scenes);
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

    // Spec: recording R12
    #[test]
    fn take_lanes_are_added_in_order_to_audio_tracks_and_survive_a_save() {
        let mut song = Song::new();
        let audio_track = song.track(TrackKind::Audio, None);
        let midi_track = song.track(TrackKind::Midi, None);
        let lane = |name: &str, start: f64| TakeLane {
            name: name.to_owned(),
            clips: vec![TakeClip {
                name: "Take".to_owned(),
                start: Ticks::QUARTER,
                length: Ticks::QUARTER,
                data: AudioData {
                    start,
                    end: Some(start + 0.5),
                    ..AudioData::new(ContentHash([3; 32]))
                },
            }],
        };
        let before = song.doc.clone();
        song.apply(Command::AddTakeLane {
            track: audio_track,
            lane: lane("Take 1", 0.0),
        })
        .unwrap();
        song.apply(Command::AddTakeLane {
            track: audio_track,
            lane: lane("Take 2", 0.5),
        })
        .unwrap();
        let names: Vec<&str> = song
            .doc
            .track(audio_track)
            .unwrap()
            .take_lanes()
            .map(|lane| lane.name.as_str())
            .collect();
        assert_eq!(names, ["Take 1", "Take 2"], "in the order they were added");
        assert!(
            ChangeSet::between(&before, &song.doc)
                .tracks
                .contains_key(&audio_track),
            "the track shows as changed"
        );

        // Only audio tracks have take lanes, and a pass needs a real length
        // and sound data.
        assert!(matches!(
            song.reject(Command::AddTakeLane {
                track: midi_track,
                lane: lane("Take 1", 0.0),
            }),
            ModelError::WrongTrackKind { .. }
        ));
        let mut empty = lane("Take 3", 0.0);
        empty.clips[0].length = Ticks::ZERO;
        assert!(matches!(
            song.reject(Command::AddTakeLane {
                track: audio_track,
                lane: empty,
            }),
            ModelError::EmptyLength(_)
        ));
        let mut bad = lane("Take 3", 0.0);
        bad.clips[0].data.gain = f64::NAN;
        assert!(matches!(
            song.reject(Command::AddTakeLane {
                track: audio_track,
                lane: bad,
            }),
            ModelError::InvalidValue { .. }
        ));

        // A save and load keeps them, and the loaded document follows every
        // rule.
        let saved = serde_json::to_string(&crate::repr::DocumentRepr::from(&song.doc)).unwrap();
        let loaded: crate::repr::DocumentRepr = serde_json::from_str(&saved).unwrap();
        let loaded = Document::try_from(loaded).unwrap();
        assert_eq!(loaded, song.doc);
        loaded.check_invariants().unwrap();
        assert_eq!(loaded.track(audio_track).unwrap().take_lanes().len(), 2);
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

    // Spec: automation R1 to R6
    #[test]
    fn automation_is_one_envelope_per_parameter_and_a_hand_move_overrides_it() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let ret = song.track(TrackKind::Return, None);
        let device = song.device(track);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        let send = AutomationTarget::Send { to: ret };
        let param = AutomationTarget::Device {
            device,
            param: ParamId(2),
        };

        // A first breakpoint makes the envelope, and the envelope drives the
        // control.
        song.apply(Command::SetAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::ZERO,
            value: 1.0,
        })
        .unwrap();
        song.apply(Command::SetAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::WHOLE,
            value: 0.5,
        })
        .unwrap();
        let automation = song
            .doc
            .track(track)
            .unwrap()
            .automation_for(&volume)
            .unwrap();
        assert!(automation.enabled);
        assert_eq!(
            automation.envelope.points().collect::<Vec<_>>(),
            vec![
                Breakpoint {
                    at: Ticks::ZERO,
                    value: 1.0,
                    bend: 0.0
                },
                Breakpoint {
                    at: Ticks::WHOLE,
                    value: 0.5,
                    bend: 0.0
                }
            ]
        );
        // Between two breakpoints the line, and before the first and after
        // the last the nearest one's value.
        assert_eq!(automation.envelope.value_at(Ticks::ZERO), Some(1.0));
        assert_eq!(automation.envelope.value_at(Ticks::WHOLE / 2), Some(0.75));
        assert_eq!(automation.envelope.value_at(Ticks::WHOLE * 2), Some(0.5));

        // Setting the same position replaces the breakpoint's value; a send
        // and a device parameter hold envelopes of their own.
        song.apply(Command::SetAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::WHOLE,
            value: 0.25,
        })
        .unwrap();
        assert_eq!(
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&volume)
                .unwrap()
                .envelope
                .points()
                .count(),
            2
        );
        song.apply(Command::SetAutomationBreakpoint {
            track,
            target: send,
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
        song.apply(Command::SetAutomationBreakpoint {
            track,
            target: param,
            at: Ticks::ZERO,
            value: -6.0,
        })
        .unwrap();
        assert_eq!(song.doc.track(track).unwrap().automation().count(), 3);

        // Moving and deleting a breakpoint.
        song.apply(Command::MoveAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::WHOLE,
            to: Ticks::WHOLE * 2,
            value: None,
        })
        .unwrap();
        assert_eq!(
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&volume)
                .unwrap()
                .envelope
                .points()
                .collect::<Vec<_>>(),
            vec![
                Breakpoint {
                    at: Ticks::ZERO,
                    value: 1.0,
                    bend: 0.0
                },
                Breakpoint {
                    at: Ticks::WHOLE * 2,
                    value: 0.25,
                    bend: 0.0
                }
            ]
        );
        assert_eq!(
            song.reject(Command::MoveAutomationBreakpoint {
                track,
                target: volume,
                at: Ticks::WHOLE,
                to: Ticks::WHOLE * 4,
                value: None,
            }),
            ModelError::NoChange {
                what: "breakpoint",
                at: Ticks::WHOLE,
            }
        );
        assert_eq!(
            song.reject(Command::MoveAutomationBreakpoint {
                track,
                target: volume,
                at: Ticks::ZERO,
                to: Ticks::WHOLE * 2,
                value: None,
            }),
            ModelError::ChangeInUse {
                what: "breakpoint",
                at: Ticks::WHOLE * 2,
            }
        );

        // Moving the control by hand overrides the envelope: the control's
        // value wins, the envelope stays, and Re-Enable brings it back.
        song.apply(Command::SetVolume {
            track,
            volume: 0.75,
        })
        .unwrap();
        let automation = song
            .doc
            .track(track)
            .unwrap()
            .automation_for(&volume)
            .unwrap();
        assert!(!automation.enabled);
        assert_eq!(automation.envelope.points().count(), 2);
        assert!(song.doc.track(track).unwrap().automation_overridden());
        song.apply(Command::ReEnableAutomationFor {
            track,
            target: volume,
        })
        .unwrap();
        assert!(
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&volume)
                .unwrap()
                .enabled
        );
        // A hand move on a control with no envelope changes nothing else.
        song.apply(Command::SetPan { track, pan: 0.5 }).unwrap();
        assert!(!song.doc.track(track).unwrap().automation_overridden());

        // The same for a device parameter and a send, and Re-Enable
        // Automation turns every overridden envelope back on at once.
        song.apply(Command::SetDeviceParam {
            device,
            param: ParamId(2),
            value: -3.0,
        })
        .unwrap();
        assert!(
            !song
                .doc
                .track(track)
                .unwrap()
                .automation_for(&param)
                .unwrap()
                .enabled
        );
        song.apply(Command::SetSend {
            track,
            to: ret,
            level: 0.25,
        })
        .unwrap();
        assert!(
            !song
                .doc
                .track(track)
                .unwrap()
                .automation_for(&send)
                .unwrap()
                .enabled
        );
        song.apply(Command::ReEnableAutomation {}).unwrap();
        assert!(!song.doc.track(track).unwrap().automation_overridden());

        // Deleting a parameter's automation takes the whole envelope; the
        // control keeps its value. Deleting again changes nothing.
        song.apply(Command::DeleteAutomation {
            track,
            target: send,
        })
        .unwrap();
        assert!(
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&send)
                .is_none()
        );
        assert_eq!(song.doc.track(track).unwrap().send(ret), 0.25);
        song.apply(Command::DeleteAutomation {
            track,
            target: send,
        })
        .unwrap();

        // Deleting the last breakpoint takes the envelope with it.
        song.apply(Command::DeleteAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::ZERO,
        })
        .unwrap();
        assert_eq!(
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&volume)
                .unwrap()
                .envelope
                .points()
                .count(),
            1
        );
        song.apply(Command::DeleteAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::WHOLE * 2,
        })
        .unwrap();
        assert!(
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&volume)
                .is_none()
        );

        // Targets the track doesn't have are refused: a send to something
        // that isn't a return track, and a device in another chain.
        let other = song.track(TrackKind::Audio, None);
        let other_device = song.device(other);
        assert!(
            song.doc
                .apply(&Command::SetAutomationBreakpoint {
                    track,
                    target: AutomationTarget::Send { to: other },
                    at: Ticks::ZERO,
                    value: 0.5,
                })
                .is_err()
        );
        assert_eq!(
            song.reject(Command::SetAutomationBreakpoint {
                track,
                target: AutomationTarget::Device {
                    device: other_device,
                    param: ParamId(0),
                },
                at: Ticks::ZERO,
                value: 0.5,
            }),
            ModelError::DeviceNotInTrack {
                device: other_device,
                track,
            }
        );
        // A position before 1.1.1, and a value that isn't a number.
        assert!(
            song.doc
                .apply(&Command::SetAutomationBreakpoint {
                    track,
                    target: param,
                    at: -Ticks(1),
                    value: 0.5,
                })
                .is_err()
        );
        assert!(
            song.doc
                .apply(&Command::SetAutomationBreakpoint {
                    track,
                    target: param,
                    at: Ticks::ZERO,
                    value: f64::NAN,
                })
                .is_err()
        );

        // Deleting a device takes its automation with it.
        song.apply(Command::RemoveDevice { device }).unwrap();
        assert!(
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&param)
                .is_none()
        );
        song.doc.check_invariants().unwrap();
    }

    // Spec: clip-envelopes R5
    #[test]
    fn moving_a_clip_envelope_breakpoint_can_give_it_a_new_value() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.clip(track, 0);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
        song.apply(Command::MoveClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::ZERO,
            to: Ticks::WHOLE,
            value: Some(0.25),
        })
        .unwrap();
        let envelope = &song.doc.clip(clip).unwrap().envelope(&volume).unwrap();
        assert!(envelope.envelope.point(Ticks::ZERO).is_none());
        assert_eq!(envelope.envelope.point(Ticks::WHOLE).unwrap().value, 0.25);
        song.reject(Command::MoveClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::WHOLE,
            to: Ticks::WHOLE * 2,
            value: Some(f64::NAN),
        });
    }

    // Spec: clip-envelopes R1, R5
    #[test]
    fn a_clip_carries_envelopes_and_their_edits_work() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let caller = song.track(TrackKind::Return, None);
        let device = song.device(track);
        let clip = song.clip(track, 0);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        let send = AutomationTarget::Send { to: caller };
        let param = AutomationTarget::Device {
            device,
            param: ParamId(2),
        };

        // A first breakpoint makes the envelope, linked to its clip.
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::ZERO,
            value: 1.0,
        })
        .unwrap();
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::WHOLE,
            value: 0.25,
        })
        .unwrap();
        let envelope = song.doc.clip(clip).unwrap().envelope(&volume).unwrap();
        assert!(envelope.linked, "a new envelope is linked to its clip");
        assert_eq!(
            envelope.envelope.points().collect::<Vec<_>>(),
            vec![
                Breakpoint {
                    at: Ticks::ZERO,
                    value: 1.0,
                    bend: 0.0
                },
                Breakpoint {
                    at: Ticks::WHOLE,
                    value: 0.25,
                    bend: 0.0
                }
            ]
        );
        assert_eq!(envelope.envelope.value_at(Ticks::WHOLE / 2), Some(0.625));

        // A send and a device parameter hold envelopes of their own.
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: send,
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: param,
            at: Ticks::ZERO,
            value: -6.0,
        })
        .unwrap();
        assert_eq!(song.doc.clip(clip).unwrap().envelopes().count(), 3);

        // Setting the same position replaces a breakpoint's value; curving a
        // segment stores its bend on the breakpoint at its start; moving a
        // breakpoint keeps its value; deleting one joins its neighbours.
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::WHOLE,
            value: 0.5,
        })
        .unwrap();
        assert_eq!(
            song.doc
                .clip(clip)
                .unwrap()
                .envelope(&volume)
                .unwrap()
                .envelope
                .points()
                .count(),
            2
        );
        song.apply(Command::SetClipEnvelopeCurve {
            clip,
            target: volume,
            at: Ticks::ZERO,
            bend: -0.5,
        })
        .unwrap();
        let envelope = &song.doc.clip(clip).unwrap().envelope(&volume).unwrap();
        assert_eq!(envelope.envelope.point(Ticks::ZERO).unwrap().bend, -0.5);
        // The segment bows off the straight line by its bend's power curve.
        let half = envelope.envelope.value_at(Ticks::WHOLE / 2).unwrap();
        let want = 1.0 - 0.5 * 0.5f64.powf(2f64.powf(-0.5));
        assert!((half - want).abs() < 1e-12, "{half}");
        song.apply(Command::MoveClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::WHOLE,
            to: Ticks::WHOLE * 2,
            value: None,
        })
        .unwrap();
        let envelope = &song.doc.clip(clip).unwrap().envelope(&volume).unwrap();
        assert!(envelope.envelope.point(Ticks::WHOLE).is_none());
        assert_eq!(
            envelope.envelope.point(Ticks::WHOLE * 2).unwrap().value,
            0.5
        );
        assert_eq!(envelope.envelope.point(Ticks::ZERO).unwrap().bend, -0.5);
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::WHOLE,
            value: 0.75,
        })
        .unwrap();
        song.apply(Command::DeleteClipEnvelopeBreakpoint {
            clip,
            target: volume,
            at: Ticks::WHOLE,
        })
        .unwrap();
        assert_eq!(
            song.doc
                .clip(clip)
                .unwrap()
                .envelope(&volume)
                .unwrap()
                .envelope
                .points()
                .count(),
            2
        );

        // The Linked switch, and the refusals: a position before zero, a
        // move onto a breakpoint, a bend outside −1 to 1, a segment at no
        // breakpoint, and targets the clip's track doesn't have.
        song.apply(Command::SetClipEnvelopeLinked {
            clip,
            target: volume,
            linked: false,
        })
        .unwrap();
        assert!(
            !song
                .doc
                .clip(clip)
                .unwrap()
                .envelope(&volume)
                .unwrap()
                .linked
        );
        assert!(
            song.doc
                .apply(&Command::SetClipEnvelopeBreakpoint {
                    clip,
                    target: volume,
                    at: -Ticks(1),
                    value: 0.5,
                })
                .is_err()
        );
        assert!(
            song.doc
                .apply(&Command::SetClipEnvelopeBreakpoint {
                    clip,
                    target: volume,
                    at: Ticks::ZERO,
                    value: f64::NAN,
                })
                .is_err()
        );
        assert_eq!(
            song.reject(Command::MoveClipEnvelopeBreakpoint {
                clip,
                target: volume,
                at: Ticks::ZERO,
                to: Ticks::WHOLE * 2,
                value: None,
            }),
            ModelError::ChangeInUse {
                what: "breakpoint",
                at: Ticks::WHOLE * 2,
            }
        );
        assert_eq!(
            song.reject(Command::MoveClipEnvelopeBreakpoint {
                clip,
                target: volume,
                at: Ticks::WHOLE,
                to: Ticks::WHOLE * 4,
                value: None,
            }),
            ModelError::NoChange {
                what: "breakpoint",
                at: Ticks::WHOLE,
            }
        );
        assert!(
            song.doc
                .apply(&Command::SetClipEnvelopeCurve {
                    clip,
                    target: volume,
                    at: Ticks::ZERO,
                    bend: 2.0,
                })
                .is_err()
        );
        assert!(
            song.doc
                .apply(&Command::SetClipEnvelopeCurve {
                    clip,
                    target: volume,
                    at: Ticks::WHOLE,
                    bend: 0.0,
                })
                .is_err(),
            "the last breakpoint has no segment after it"
        );
        assert!(
            song.doc
                .apply(&Command::SetClipEnvelopeBreakpoint {
                    clip,
                    target: AutomationTarget::Send { to: track },
                    at: Ticks::ZERO,
                    value: 0.5,
                })
                .is_err(),
            "a send to something that isn't a return track"
        );
        let other = song.track(TrackKind::Audio, None);
        let other_device = song.device(other);
        assert_eq!(
            song.reject(Command::SetClipEnvelopeBreakpoint {
                clip,
                target: AutomationTarget::Device {
                    device: other_device,
                    param: ParamId(0),
                },
                at: Ticks::ZERO,
                value: 0.5,
            }),
            ModelError::DeviceNotInTrack {
                device: other_device,
                track,
            }
        );

        // Deleting the last breakpoint takes the envelope with it; Clear
        // Envelope takes the whole envelope, and deleting it again changes
        // nothing.
        song.apply(Command::DeleteClipEnvelopeBreakpoint {
            clip,
            target: param,
            at: Ticks::ZERO,
        })
        .unwrap();
        assert!(song.doc.clip(clip).unwrap().envelope(&param).is_none());
        song.apply(Command::DeleteClipEnvelope { clip, target: send })
            .unwrap();
        assert!(song.doc.clip(clip).unwrap().envelope(&send).is_none());
        song.apply(Command::DeleteClipEnvelope { clip, target: send })
            .unwrap();
        assert_eq!(song.doc.clip(clip).unwrap().envelopes().count(), 1);
        song.doc.check_invariants().unwrap();
    }

    // Spec: clip-envelopes R7
    #[test]
    fn clip_envelopes_follow_the_parameters_their_track_still_has() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let other = song.track(TrackKind::Midi, None);
        let device = song.device(track);
        let clip = song.clip(track, 0);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        let param = AutomationTarget::Device {
            device,
            param: ParamId(0),
        };
        for target in [volume, param] {
            song.apply(Command::SetClipEnvelopeBreakpoint {
                clip,
                target,
                at: Ticks::ZERO,
                value: 1.0,
            })
            .unwrap();
        }
        assert_eq!(song.doc.clip(clip).unwrap().envelopes().count(), 2);

        // A clip moved to another track keeps only the envelopes naming
        // parameters its new track has: a mixer control of any track fits,
        // the device the clip leaves behind doesn't.
        song.apply(Command::MoveClip {
            clip,
            track: other,
            start: Ticks::ZERO,
        })
        .unwrap();
        let moved = song.doc.clip(clip).unwrap();
        assert!(moved.envelope(&volume).is_some());
        assert!(moved.envelope(&param).is_none());

        // A device that leaves the track's chain takes the clip envelopes
        // that name its parameters with it, as it takes the track's own
        // automation (automation R11).
        let clip = song.clip(track, 0);
        let device = song.device(track);
        let param = AutomationTarget::Device {
            device,
            param: ParamId(0),
        };
        for target in [volume, param] {
            song.apply(Command::SetClipEnvelopeBreakpoint {
                clip,
                target,
                at: Ticks::ZERO,
                value: 1.0,
            })
            .unwrap();
        }
        song.apply(Command::RemoveDevice { device }).unwrap();
        let envelopes = song.doc.clip(clip).unwrap();
        assert!(envelopes.envelope(&param).is_none());
        assert!(envelopes.envelope(&volume).is_some());
        song.doc.check_invariants().unwrap();
    }

    /// Spec: automation R7. A drag moves a breakpoint in time and, with a
    /// value, to that value; without one its value stays.
    #[test]
    fn moving_a_breakpoint_can_change_its_value() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        song.apply(Command::SetAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::ZERO,
            value: 1.0,
        })
        .unwrap();
        song.apply(Command::SetAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::WHOLE,
            value: 0.25,
        })
        .unwrap();
        song.apply(Command::MoveAutomationBreakpoint {
            track,
            target: volume,
            at: Ticks::WHOLE,
            to: Ticks::WHOLE * 2,
            value: Some(0.5),
        })
        .unwrap();
        let envelope = &song
            .doc
            .track(track)
            .unwrap()
            .automation_for(&volume)
            .unwrap()
            .envelope;
        assert_eq!(
            envelope.point(Ticks::WHOLE * 2),
            Some(Breakpoint {
                at: Ticks::WHOLE * 2,
                value: 0.5,
                bend: 0.0,
            })
        );
        assert_eq!(envelope.point(Ticks::WHOLE), None);
        // A value that isn't a number is refused.
        assert!(
            song.reject(Command::MoveAutomationBreakpoint {
                track,
                target: volume,
                at: Ticks::WHOLE * 2,
                to: Ticks::WHOLE * 3,
                value: Some(f64::NAN),
            })
            .to_string()
            .contains("automation value")
        );
        song.doc.check_invariants().unwrap();
    }

    /// Spec: automation R12. A segment's bend lives on the breakpoint at
    /// its start; 0 straightens the segment again, and a bend outside −1 to
    /// 1, a position with no breakpoint, and the last breakpoint are
    /// refused.
    #[test]
    fn curving_a_segment_stores_its_bend_and_straightens_again() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        for (at, value) in [(Ticks::ZERO, 0.0), (Ticks::WHOLE, 1.0)] {
            song.apply(Command::SetAutomationBreakpoint {
                track,
                target: volume,
                at,
                value,
            })
            .unwrap();
        }
        song.apply(Command::SetAutomationCurve {
            track,
            target: volume,
            at: Ticks::ZERO,
            bend: -1.0,
        })
        .unwrap();
        let automation = song
            .doc
            .track(track)
            .unwrap()
            .automation_for(&volume)
            .unwrap();
        assert_eq!(
            automation.envelope.points().collect::<Vec<_>>(),
            vec![
                Breakpoint {
                    at: Ticks::ZERO,
                    value: 0.0,
                    bend: -1.0
                },
                Breakpoint {
                    at: Ticks::WHOLE,
                    value: 1.0,
                    bend: 0.0
                }
            ]
        );
        let half = automation.envelope.value_at(Ticks::WHOLE / 2).unwrap();
        assert!((half - 0.5f64.sqrt()).abs() < 1e-12, "{half}");

        // A bend of 0 straightens the segment again, as the same drag with
        // a double-click does, and the breakpoints stay where they are.
        song.apply(Command::SetAutomationCurve {
            track,
            target: volume,
            at: Ticks::ZERO,
            bend: 0.0,
        })
        .unwrap();
        let automation = song
            .doc
            .track(track)
            .unwrap()
            .automation_for(&volume)
            .unwrap();
        assert_eq!(automation.envelope.point(Ticks::ZERO).unwrap().bend, 0.0);
        assert_eq!(automation.envelope.value_at(Ticks::WHOLE / 2), Some(0.5));

        // Refusals: a bend outside −1 to 1, a position with no breakpoint,
        // and the last breakpoint, with no segment after it.
        assert_eq!(
            song.reject(Command::SetAutomationCurve {
                track,
                target: volume,
                at: Ticks::ZERO,
                bend: 1.5,
            }),
            ModelError::InvalidValue {
                what: "automation bend",
                value: 1.5,
            }
        );
        assert_eq!(
            song.reject(Command::SetAutomationCurve {
                track,
                target: volume,
                at: Ticks::QUARTER,
                bend: 0.5,
            }),
            ModelError::NoChange {
                what: "breakpoint",
                at: Ticks::QUARTER,
            }
        );
        assert_eq!(
            song.reject(Command::SetAutomationCurve {
                track,
                target: volume,
                at: Ticks::WHOLE,
                bend: 0.5,
            }),
            ModelError::NoChange {
                what: "segment",
                at: Ticks::WHOLE,
            }
        );
        assert!(
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&volume)
                .unwrap()
                .envelope
                .points()
                .all(|point| point.bend == 0.0),
            "the refusals changed nothing"
        );
        song.doc.check_invariants().unwrap();
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

    // Spec: routing R1, R2, R16
    #[test]
    fn a_midi_tracks_input_is_all_ins_or_a_port_with_a_channel() {
        let mut song = Song::new();
        let audio = song.track(TrackKind::Audio, None);
        let midi = song.track(TrackKind::Midi, None);
        let group = song.track(TrackKind::Group, None);
        let ret = song.track(TrackKind::Return, None);
        let main = song.main();

        // A new MIDI track takes All Ins on All Channels, an audio track
        // Ext. In, and the other kinds no input.
        assert_eq!(
            *song.doc.track(midi).unwrap().routing(),
            Routing::new(TrackKind::Midi)
        );
        assert_eq!(
            *song.doc.track(audio).unwrap().routing(),
            Routing::new(TrackKind::Audio)
        );
        assert_eq!(song.doc.track(group).unwrap().routing().input, Input::None);

        // A port and its channel change together, as one undo step.
        let port = Input::Port {
            name: "IAC Bus 1".to_owned(),
        };
        song.apply(Command::SetInput {
            track: midi,
            input: port.clone(),
            channel: Some(16),
        })
        .unwrap();
        let routing = song.doc.track(midi).unwrap().routing();
        assert_eq!(
            (routing.input.clone(), routing.channel),
            (port.clone(), Some(16))
        );
        // The same command with no channel means All Channels.
        song.apply(Command::SetInput {
            track: midi,
            input: port,
            channel: None,
        })
        .unwrap();
        assert_eq!(song.doc.track(midi).unwrap().routing().channel, None);

        // Only a MIDI track takes All Ins, the computer keyboard, or a
        // port; a port needs a name, and a channel needs a port.
        for track in [audio, group, ret, main] {
            assert!(matches!(
                song.reject(Command::SetInput {
                    track,
                    input: Input::ComputerKeyboard,
                    channel: None,
                }),
                ModelError::WrongTrackKind { .. }
            ));
        }
        for (input, channel) in [
            (Input::AllIns, Some(3)),
            (Input::ComputerKeyboard, Some(3)),
            (
                Input::Port {
                    name: String::new(),
                },
                None,
            ),
            (
                Input::Port {
                    name: "Keys".to_owned(),
                },
                Some(0),
            ),
            (
                Input::Port {
                    name: "Keys".to_owned(),
                },
                Some(17),
            ),
        ] {
            assert!(matches!(
                song.reject(Command::SetInput {
                    track: midi,
                    input,
                    channel,
                }),
                ModelError::InvalidInput(_)
            ));
        }

        // Monitoring is In, Auto, or Off, on audio and MIDI tracks alone.
        for (track, allowed) in [
            (audio, true),
            (midi, true),
            (group, false),
            (ret, false),
            (main, false),
        ] {
            let command = Command::SetMonitor {
                track,
                monitor: Monitor::In,
            };
            if allowed {
                song.apply(command).unwrap();
                assert_eq!(
                    song.doc.track(track).unwrap().routing().monitor,
                    Monitor::In
                );
            } else {
                assert!(matches!(
                    song.reject(command),
                    ModelError::WrongTrackKind { .. }
                ));
            }
        }
        let json = serde_json::to_value(Command::SetInput {
            track: midi,
            input: Input::Port {
                name: "Keys".to_owned(),
            },
            channel: Some(2),
        })
        .unwrap();
        assert_eq!(json["op"], "set_input");
        assert_eq!(json["input"]["type"], "port");
        assert_eq!(json["channel"], 2);
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
            .created_ids(),
            vec![id.0]
        );
        assert!(Command::DeleteTrack { track }.created_ids().is_empty());
    }

    // Spec: session-view R20
    #[test]
    fn a_scene_tempo_and_signature_are_set_cleared_copied_and_one_undo_step_each() {
        let mut editor = crate::Editor::new();
        let scene = editor.document().scenes().next().unwrap().id;
        let tempo = |editor: &crate::Editor| editor.document().scene(scene).unwrap().tempo;
        let signature = |editor: &crate::Editor| editor.document().scene(scene).unwrap().signature;
        assert_eq!((tempo(&editor), signature(&editor)), (None, None));
        let ninety = Bpm::new(90.0).unwrap();
        let three_four = TimeSignature::new(3, 4).unwrap();
        editor
            .apply(Command::SetSceneTempo {
                scene,
                tempo: Some(ninety),
            })
            .unwrap();
        editor
            .apply(Command::SetSceneTimeSignature {
                scene,
                signature: Some(three_four),
            })
            .unwrap();
        assert_eq!(
            (tempo(&editor), signature(&editor)),
            (Some(ninety), Some(three_four))
        );
        // Duplicating a scene keeps its tempo and signature.
        let copy: SceneId = editor.new_id();
        editor
            .apply(Command::DuplicateScene {
                scene,
                id: copy,
                clips: Vec::new(),
            })
            .unwrap();
        assert_eq!(editor.document().scene(copy).unwrap().tempo, Some(ninety));
        assert_eq!(
            editor.document().scene(copy).unwrap().signature,
            Some(three_four)
        );
        editor.undo().unwrap();
        // Each set is its own undo step, and setting the same value is none.
        editor
            .apply(Command::SetSceneTempo {
                scene,
                tempo: Some(ninety),
            })
            .unwrap();
        editor.undo().unwrap();
        assert_eq!((tempo(&editor), signature(&editor)), (Some(ninety), None));
        editor.undo().unwrap();
        assert_eq!(tempo(&editor), None);
        // On, then off.
        editor
            .apply(Command::SetSceneTempo {
                scene,
                tempo: Some(ninety),
            })
            .unwrap();
        editor
            .apply(Command::SetSceneTempo { scene, tempo: None })
            .unwrap();
        assert_eq!(tempo(&editor), None);
        // A tempo outside 20 to 999 BPM is refused and changes nothing.
        for bpm in [19.9, 999.5] {
            let before = editor.document().clone();
            let error = editor
                .apply(Command::SetSceneTempo {
                    scene,
                    tempo: Some(Bpm::new(bpm).unwrap()),
                })
                .unwrap_err();
            assert!(
                matches!(
                    error,
                    crate::editor::EditError::Command(ModelError::InvalidValue { .. })
                ),
                "{error:?}"
            );
            assert_eq!(editor.document(), &before);
        }
    }

    // Spec: session-view R1, R4, R5, R6, R7, R8
    #[test]
    fn scenes_insert_move_rename_duplicate_and_delete() {
        let mut song = Song::new();
        assert_eq!(song.doc.scene_count(), crate::DEFAULT_SCENE_COUNT);
        assert!(
            song.doc.scenes().all(|scene| scene.name.is_empty()),
            "the default scenes have no names of their own"
        );
        let audio_track = song.track(TrackKind::Audio, None);
        let scene: SceneId = song.fresh();
        song.apply(Command::CreateScene {
            id: scene,
            index: 0,
        })
        .unwrap();
        assert_eq!(song.doc.scene_position(scene), Some(0));
        assert_eq!(song.doc.scene(scene).unwrap().name, "");
        song.apply(Command::RenameScene {
            scene,
            name: "Verse".to_owned(),
        })
        .unwrap();
        assert_eq!(song.doc.scene(scene).unwrap().name, "Verse");

        // A clip in the scene's slot moves with the scene.
        let clip: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track: audio_track,
            scene,
            clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, audio()),
        })
        .unwrap();
        song.apply(Command::MoveScene { scene, index: 3 }).unwrap();
        assert_eq!(song.doc.scene_position(scene), Some(3));
        assert_eq!(song.doc.slot(audio_track, scene).unwrap().id, clip);

        // Duplicating copies the scene, its name, and an independent clip,
        // straight after it.
        let copy: SceneId = song.fresh();
        let copy_clip: ClipId = song.fresh();
        song.apply(Command::DuplicateScene {
            scene,
            id: copy,
            clips: vec![(audio_track, copy_clip)],
        })
        .unwrap();
        assert_eq!(song.doc.scene_position(copy), Some(4));
        assert_eq!(song.doc.scene(copy).unwrap().name, "Verse");
        let copied = song.doc.slot(audio_track, copy).unwrap();
        assert_eq!(copied.id, copy_clip);
        assert_eq!(copied.data, audio());

        // Every clip in the scene must be listed, and only those.
        let other = song.track(TrackKind::Audio, None);
        let missing: SceneId = song.fresh();
        assert_eq!(
            song.reject(Command::DuplicateScene {
                scene,
                id: missing,
                clips: Vec::new(),
            }),
            ModelError::SceneCopyMismatch {
                scene,
                track: audio_track
            }
        );
        let (one, two) = (song.fresh(), song.fresh());
        assert_eq!(
            song.reject(Command::DuplicateScene {
                scene,
                id: missing,
                clips: vec![(audio_track, one), (audio_track, two)],
            }),
            ModelError::DuplicateId(audio_track.0)
        );
        let (one, two) = (song.fresh(), song.fresh());
        assert_eq!(
            song.reject(Command::DuplicateScene {
                scene,
                id: missing,
                clips: vec![(audio_track, one), (other, two)],
            }),
            ModelError::SceneCopyMismatch {
                scene,
                track: other,
            }
        );
        // The scene's copy and the clips' copies are one set of new IDs.
        assert_eq!(
            song.reject(Command::DuplicateScene {
                scene,
                id: missing,
                clips: vec![(audio_track, ClipId(missing.0))],
            }),
            ModelError::DuplicateId(missing.0)
        );

        // Deleting takes the scene's clips with it; the copies stay.
        song.apply(Command::DeleteScene { scene }).unwrap();
        assert!(song.doc.scene(scene).is_none());
        assert!(song.doc.clip(clip).is_none());
        assert_eq!(song.doc.slot(audio_track, copy).unwrap().id, copy_clip);
        assert_eq!(
            song.reject(Command::DeleteScene { scene }),
            ModelError::NoSuchScene(scene)
        );
    }

    // Spec: session-view R2, R3, R9
    #[test]
    fn slot_clips_need_a_track_with_slots_and_an_empty_slot() {
        let mut song = Song::new();
        let audio_track = song.track(TrackKind::Audio, None);
        let midi_track = song.track(TrackKind::Midi, None);
        let group = song.track(TrackKind::Group, None);
        let ret = song.track(TrackKind::Return, None);
        let main = song.main();
        let scene = song.doc.scenes().next().unwrap().id;

        let audio_clip: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track: audio_track,
            scene,
            clip: Clip::new(audio_clip, Ticks::ZERO, Ticks::WHOLE, audio()),
        })
        .unwrap();
        let midi_clip: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track: midi_track,
            scene,
            clip: Clip::new(midi_clip, Ticks::ZERO, Ticks::WHOLE, midi()),
        })
        .unwrap();
        assert_eq!(song.doc.clip_count(), 2);
        assert_eq!(song.doc.clip_track(audio_clip), Some(audio_track));
        assert_eq!(song.doc.clip(audio_clip).unwrap().id, audio_clip);

        // The data must fit the track, and only audio and MIDI tracks hold
        // slot clips.
        for (track, data) in [
            (audio_track, midi()),
            (midi_track, audio()),
            (group, midi()),
            (ret, audio()),
            (main, midi()),
        ] {
            let clip = Clip::new(song.fresh(), Ticks::ZERO, Ticks::WHOLE, data);
            assert!(matches!(
                song.apply(Command::CreateSlotClip { track, scene, clip }),
                Err(ModelError::WrongTrackKind { .. })
            ));
        }
        // A slot holds one clip, and a slot clip has no position.
        let occupied = Clip::new(song.fresh(), Ticks::ZERO, Ticks::WHOLE, audio());
        assert_eq!(
            song.reject(Command::CreateSlotClip {
                track: audio_track,
                scene,
                clip: occupied,
            }),
            ModelError::SlotOccupied {
                track: audio_track,
                scene,
            }
        );
        let placed = Clip::new(song.fresh(), Ticks::WHOLE, Ticks::WHOLE, audio());
        assert!(matches!(
            song.reject(Command::CreateSlotClip {
                track: audio_track,
                scene,
                clip: placed,
            }),
            ModelError::SlotClipPosition(_)
        ));
        // The scene must exist.
        let nowhere: SceneId = song.fresh();
        let clip = Clip::new(song.fresh(), Ticks::ZERO, Ticks::WHOLE, audio());
        assert_eq!(
            song.reject(Command::CreateSlotClip {
                track: audio_track,
                scene: nowhere,
                clip,
            }),
            ModelError::NoSuchScene(nowhere)
        );
    }

    // Spec: launch-quantisation R14
    #[test]
    fn a_session_clips_launch_settings_are_set_and_kept_in_range() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let scene = song.doc.scenes().next().unwrap().id;
        let clip: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, audio()),
        })
        .unwrap();
        assert_eq!(song.doc.clip(clip).unwrap().launch, ClipLaunch::default());

        song.apply(Command::SetClipLegato { clip, legato: true })
            .unwrap();
        song.apply(Command::SetClipLaunchQuantization {
            clip,
            quantization: Some(LaunchQuantization::None),
        })
        .unwrap();
        song.apply(Command::SetClipVelocityAmount { clip, amount: 1.0 })
            .unwrap();
        assert_eq!(
            song.doc.clip(clip).unwrap().launch,
            ClipLaunch {
                legato: true,
                quantization: Some(LaunchQuantization::None),
                velocity_amount: 1.0,
                ..ClipLaunch::default()
            },
            "None is a step of its own, apart from following the global one"
        );
        song.apply(Command::SetClipLaunchQuantization {
            clip,
            quantization: None,
        })
        .unwrap();
        assert_eq!(song.doc.clip(clip).unwrap().launch.quantization, None);

        for amount in [-0.1, 1.1, f64::NAN] {
            assert!(matches!(
                song.reject(Command::SetClipVelocityAmount { clip, amount }),
                ModelError::InvalidInput(_)
            ));
        }

        // An Arrangement clip has no launch settings.
        let placed: ClipId = song.fresh();
        song.apply(Command::CreateClip {
            track,
            clip: Clip::new(placed, Ticks::ZERO, Ticks::WHOLE, audio()),
        })
        .unwrap();
        assert_eq!(
            song.reject(Command::SetClipLegato {
                clip: placed,
                legato: true,
            }),
            ModelError::NotSlotClip(placed)
        );
    }

    // Spec: session-view R10, R11
    #[test]
    fn slot_clips_move_between_slots_and_the_arrangement() {
        let mut song = Song::new();
        let audio_track = song.track(TrackKind::Audio, None);
        let other = song.track(TrackKind::Audio, None);
        let midi_track = song.track(TrackKind::Midi, None);
        let scenes: Vec<SceneId> = song.doc.scenes().map(|scene| scene.id).collect();
        let (first, second) = (scenes[0], scenes[1]);
        let clip: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track: audio_track,
            scene: first,
            clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, audio()),
        })
        .unwrap();

        // Moving a clip into its own slot changes nothing.
        song.apply(Command::MoveSlotClip {
            clip,
            track: audio_track,
            scene: first,
        })
        .unwrap();
        assert_eq!(song.doc.slot(audio_track, first).unwrap().id, clip);

        // To another track, and to another scene.
        song.apply(Command::MoveSlotClip {
            clip,
            track: other,
            scene: first,
        })
        .unwrap();
        assert_eq!(song.doc.slot(other, first).unwrap().id, clip);
        assert!(song.doc.slot(audio_track, first).is_none());
        song.apply(Command::MoveSlotClip {
            clip,
            track: other,
            scene: second,
        })
        .unwrap();
        assert_eq!(song.doc.slot(other, second).unwrap().id, clip);
        assert_eq!(song.doc.clip_track(clip), Some(other));

        // A slot that holds another clip, or a track without slots, is
        // refused.
        let blocker: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track: audio_track,
            scene: second,
            clip: Clip::new(blocker, Ticks::ZERO, Ticks::WHOLE, audio()),
        })
        .unwrap();
        assert_eq!(
            song.reject(Command::MoveSlotClip {
                clip,
                track: audio_track,
                scene: second,
            }),
            ModelError::SlotOccupied {
                track: audio_track,
                scene: second,
            }
        );
        assert!(matches!(
            song.reject(Command::MoveSlotClip {
                clip,
                track: midi_track,
                scene: second,
            }),
            ModelError::WrongTrackKind { .. }
        ));

        // Out of the slot onto the Arrangement lane, and back, arriving at
        // 0.
        song.apply(Command::MoveClip {
            clip,
            track: other,
            start: Ticks::WHOLE * 4,
        })
        .unwrap();
        assert_eq!(song.doc.clip(clip).unwrap().start, Ticks::WHOLE * 4);
        assert!(song.doc.slot(other, second).is_none());
        song.apply(Command::MoveSlotClip {
            clip,
            track: other,
            scene: second,
        })
        .unwrap();
        assert_eq!(song.doc.clip(clip).unwrap().start, Ticks::ZERO);
        assert_eq!(song.doc.slot(other, second).unwrap().id, clip);

        // Deleting a slot clip frees its slot.
        song.apply(Command::DeleteClip { clip }).unwrap();
        assert!(song.doc.clip(clip).is_none());
        assert!(song.doc.slot(other, second).is_none());
        assert_eq!(song.doc.clip_count(), 1);
    }

    // Spec: session-view R11
    #[test]
    fn the_clip_edits_work_on_a_slot_clip() {
        let mut song = Song::new();
        let midi_track = song.track(TrackKind::Midi, None);
        let scene = song.doc.scenes().next().unwrap().id;
        let clip: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track: midi_track,
            scene,
            clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, midi()),
        })
        .unwrap();
        // The clip edits that don't place a clip work on a slot clip too.
        song.apply(Command::RenameClip {
            clip,
            name: "Riff".to_owned(),
        })
        .unwrap();
        song.apply(Command::SetClipMute { clip, mute: true })
            .unwrap();
        song.apply(Command::SetClipData { clip, data: midi() })
            .unwrap();
        let held = song.doc.slot(midi_track, scene).unwrap();
        assert_eq!((held.name.as_str(), held.mute), ("Riff", true));
        assert_eq!(song.doc.clip(clip).unwrap().name, "Riff");
    }
}
