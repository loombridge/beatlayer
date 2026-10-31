//! Commands: the only way to change a document.

use std::collections::{BTreeMap, BTreeSet};

use bl_core::id::Id;
use bl_core::time::{
    Bpm, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use imbl::{OrdMap, Vector};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::auto_warp::{ClipEdge, FileBeats, LOOP_BARS, WarpFrom, WarpPoint, nearest_bars};
use crate::automation::{
    AutomationTarget, Breakpoint, Envelope, MixerParam, PointError, check_bend, entry,
    override_device,
};
use crate::clip::{
    AudioData, Clip, ClipData, ClipEnvelope, EnvelopeKind, MidiData, NewTakeLane, TakeLane,
    clip_entry, crossfades,
};
use crate::clip_param::ClipParam;
use crate::device::{Device, DeviceKind, DeviceRole};
use crate::document::Document;
use crate::error::ModelError;
use crate::external::{CvCalibration, ExternalPort, HardwareLatency};
use crate::follow::FollowPatch;
use crate::groove::{GLOBAL_AMOUNT_MAX, Groove, GrooveBase, check_percent};
use crate::ids::{
    ChainId, ClipId, ContentHash, DeviceId, GrooveId, LinkGroupId, LocatorId, ParamId, SceneId,
    TakeLaneId, TrackId,
};
use crate::launch::{ClipLaunch, LaunchMode, LaunchQuantization};
use crate::layers::{
    Distribution, Layer, LayerId, LayerSettings, LayerSort, LayerZone, Layers, Zone, ZoneKind,
};
use crate::locator::{Locator, SongStart, next_locator_name};
use crate::metronome::{CountIn, MetronomeRhythm, MetronomeSound};
use crate::rack::{
    CHAIN_COLOURS, Chain, ChainSend, MACRO_STEPS_MAX, MACRO_STEPS_MIN, MACROS, Macro, MacroMapping,
    MacroStyle, MacroTarget, RackKind, Receive, macro_number,
};
use crate::record_quantization::RecordQuantization;
use crate::sample_edits::{SampleEdit, SampleRange, SampleRegion};
use crate::scale::{Scale, ScaleName};
use crate::scene::{SCENE_TEMPO_MAX, SCENE_TEMPO_MIN, Scene};
use crate::shared::Shared;
use crate::track::{
    Channel, CrossfadeAssign, Input, MidiTo, Mixer, Monitor, Output, PanMode, SendTap, Track,
    TrackDelay, TrackKind,
};
use crate::tuning::{Tuning, TuningNote};
use crate::warp::{MarkerAt, TransientEdits};
use crate::warp_settings::{
    BeatsUpdate, ComplexProUpdate, DETUNE_MAX, DETUNE_MIN, TRANSPOSE_MAX, TRANSPOSE_MIN,
    TextureUpdate, TonesUpdate, WarpMode,
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
    forget_where(track, |target| match *target {
        AutomationTarget::Device { device, .. } => gone.devices.contains(&device),
        AutomationTarget::Chain { chain, .. } => gone.chains.contains(&chain),
        AutomationTarget::ChainSend { chain, to } => {
            gone.chains.contains(&chain) || gone.chains.contains(&to)
        }
        _ => false,
    });
}

/// Forgets the track's envelopes, and its clips' envelopes, whose targets
/// `names_gone` picks.
fn forget_where(track: &mut Track, names_gone: impl Fn(&AutomationTarget) -> bool) {
    for target in track.automation.keys().copied().collect::<Vec<_>>() {
        if names_gone(&target) {
            track.automation.remove(&target);
        }
    }
    let clips: Vec<ClipId> = track
        .clips()
        .chain(track.slots().map(|(_, clip)| clip))
        .chain(track.take_lanes().flat_map(|lane| lane.clips.iter()))
        .filter(|clip| clip.envelopes.iter().any(|e| names_gone(&e.target)))
        .map(|clip| clip.id)
        .collect();
    for id in clips {
        if let Some(clip) = track.clip_mut(id) {
            clip.envelopes.retain(|e| !names_gone(&e.target));
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
    /// Adds a locator at a position, as the Set Locator button and Add
    /// Locator do. A locator with no name of its own is named with the next
    /// whole number.
    AddLocator {
        /// The locator, with a new ID.
        locator: Locator,
    },
    /// Deletes a locator. A song start time on it goes with it.
    DeleteLocator {
        /// The locator.
        locator: LocatorId,
    },
    /// Moves a locator to a new position.
    MoveLocator {
        /// The locator.
        locator: LocatorId,
        /// Where it goes: 0 (1.1.1) or later, and not where another locator
        /// already is.
        to: Ticks,
    },
    /// Renames a locator.
    RenameLocator {
        /// The locator.
        locator: LocatorId,
        /// The new name.
        name: String,
    },
    /// Sets a locator's info text, which the Info View shows while the
    /// pointer is over it.
    SetLocatorInfoText {
        /// The locator.
        locator: LocatorId,
        /// The new info text.
        info_text: String,
    },
    /// Sets the Arrangement loop brace from a locator to the next one and
    /// switches the loop on, as Loop Selection does.
    LoopToNextLocator {
        /// The locator.
        locator: LocatorId,
    },
    /// Sets where the song starts: a locator, the loop brace's start, or
    /// nothing, which leaves the song starting as it otherwise does.
    SetSongStart {
        /// Where the song starts; nothing clears it.
        start: Option<SongStart>,
    },
    /// Opens empty time on every track: a clip across `at` is split there,
    /// and every clip, take-lane clip, breakpoint, tempo breakpoint,
    /// time-signature marker and locator at or after it moves later by
    /// `length` (Insert Silence; time editing R17). The envelopes, the
    /// tempo and the signature hold across the new time what they had at
    /// `at`.
    InsertTime {
        /// Where the time opens: 0 (1.1.1) or later.
        at: Ticks,
        /// How much time opens; more than 0.
        length: Ticks,
        /// One new ID for each clip the time opens inside, which the part
        /// after the split takes, in track order; left out, the editor
        /// issues them.
        #[serde(default)]
        clips: Vec<ClipId>,
    },
    /// Closes a span of time on every track (Delete Time; time editing
    /// R15): clips inside it go, a clip across an edge is cut or split
    /// there, and everything at or after `end` moves earlier by the span's
    /// length. Time-signature markers and locators inside it go.
    DeleteTime {
        /// Where the span starts: 0 (1.1.1) or later.
        start: Ticks,
        /// Where it ends, after `start`.
        end: Ticks,
        /// One new ID for each clip the span lies inside, which the part
        /// after the split takes, in track order; left out, the editor
        /// issues them.
        #[serde(default)]
        clips: Vec<ClipId>,
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

    /// Sets Record Quantization, the grid recorded notes are moved onto
    /// (the recording spec, R17): `quantization` chooses a value, `on` turns
    /// record quantising off, or back on at the last value chosen other than
    /// No Quantization. Exactly one of the two is given.
    SetRecordQuantization {
        /// The value to choose.
        #[serde(default)]
        quantization: Option<RecordQuantization>,
        /// Whether record quantising is on.
        #[serde(default)]
        on: Option<bool>,
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
    /// Colours tracks, Main and the returns among them, with a swatch of the
    /// palette of 5 rows of 14 (0 to 69). Their clips keep their colours.
    SetTrackColour {
        /// The tracks, each listed once.
        tracks: Vec<TrackId>,
        /// The swatch.
        colour: u8,
    },
    /// Gives every clip of each track its track's colour: in the
    /// Arrangement, in its Session slots and in its take lanes. Tracks that
    /// hold no clips are left as they are.
    AssignColourToClips {
        /// The tracks, each listed once.
        tracks: Vec<TrackId>,
    },
    /// Gives each group track's colour to every track inside it, at any
    /// depth, and to all their clips, in both views and their take lanes.
    AssignColourToGrouped {
        /// The group tracks, each listed once.
        tracks: Vec<TrackId>,
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
    /// Sets a track's info text (Edit Info Text), which the Info View shows
    /// while the pointer is over the track. Empty clears it.
    SetTrackInfoText {
        /// The track.
        track: TrackId,
        /// The text.
        info_text: String,
    },
    /// Marks tracks frozen with the files a freeze rendered (WP-4.14, R13).
    /// The track's devices, clips, and their settings stay as they are, so
    /// unfreezing plays them again (R10).
    FreezeTracks {
        /// Each track and the audio its freeze rendered.
        frozen: Vec<(TrackId, crate::freeze::FrozenTrack)>,
    },
    /// Plays tracks through their devices and clips again, dropping their
    /// frozen audio (WP-4.14, R10). Their freeze files stay on disk.
    UnfreezeTracks {
        /// The tracks to unfreeze.
        tracks: Vec<TrackId>,
    },
    /// Replaces a track with the audio track a bounce made (WP-4.15, R21,
    /// R23). The track keeps its ID, so its colour and the audio routing
    /// that points at it stay; routes that needed a MIDI track or its devices
    /// are forgotten. Undoing the bounce gives the source track
    /// back exactly, with every device's state.
    BounceTrackInPlace {
        /// The track that was bounced.
        track: TrackId,
        /// What the bounced track holds.
        bounced: crate::bounce::BouncedTrack,
    },
    /// Makes a new audio track beside a bounced one (WP-4.15, R22, R24).
    /// The new track goes directly after its source, in the same group.
    BounceToNewTrack {
        /// The track that was bounced.
        source: TrackId,
        /// The new track's ID.
        track: TrackId,
        /// What the new track holds.
        bounced: crate::bounce::BouncedTrack,
    },
    /// Gives a scene a tempo to set when it is launched, from 20 to 999
    /// BPM, or switches it off with `None` (WP-3.5).
    SetSceneTempo {
        /// The scene.
        scene: SceneId,
        /// The tempo, or `None` for none of its own.
        tempo: Option<Bpm>,
    },
    /// Changes a scene's follow actions: the fields given change and the
    /// others stay (WP-3.10).
    SetSceneFollowActions {
        /// The scene.
        scene: SceneId,
        /// The fields to change.
        #[serde(flatten)]
        patch: FollowPatch,
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
    /// Deletes every envelope of a track, as Clear All Envelopes on an
    /// automation lane's header does (the automation spec, R28). The
    /// controls stay where they are.
    ClearTrackAutomation {
        /// The track.
        track: TrackId,
    },
    /// Deletes every envelope of one device of a track, or of its mixer
    /// (its volume, pan, Track Activator and sends), as Clear Envelopes of the
    /// device an automation lane shows does (the automation spec, R28).
    ClearDeviceAutomation {
        /// The track.
        track: TrackId,
        /// The device, or `null` (or nothing) for the track's mixer.
        #[serde(default)]
        device: Option<DeviceId>,
    },
    /// Replaces one of a track's parameter's envelope with the breakpoints
    /// given, as the automation editing commands do when they write a shape
    /// over a span (the automation editing spec, R5). The envelope keeps
    /// driving the parameter, or goes on driving it if it was overridden.
    SetAutomationEnvelope {
        /// The track the parameter belongs to.
        track: TrackId,
        /// The parameter.
        target: AutomationTarget,
        /// The envelope's breakpoints, in position order. Empty takes the
        /// parameter's automation with it.
        points: Vec<Breakpoint>,
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
    /// Switches Lock Envelopes on or off: while it's on, editing the
    /// Arrangement's clips leaves the tracks' envelopes where they are in the
    /// song (the automation editing spec, R8).
    SetLockEnvelopes {
        /// Whether the envelopes stay where they are.
        enabled: bool,
    },
    /// Replaces the set's MIDI and key mappings (the remote mapping spec,
    /// R1 to R6), each checked against the set: its control is there, and
    /// no two share a source or a control of a kind.
    SetRemoteMappings {
        /// The mappings, in the order the set keeps them.
        mappings: Vec<crate::remote::RemoteMapping>,
    },
    /// Switches Cue mode: the tracks' Solo switches become Cue switches
    /// that feed the Cue Out. Needs a Cue Out that shares no channel with
    /// Main Out to turn on. It is no undo step.
    SetCueMode {
        /// Whether the Solo switches are Cue switches.
        enabled: bool,
    },
    /// Sets the Cue Volume, the Preview/Cue Volume knob on Main's strip.
    SetCueVolume {
        /// The gain: 1 is unity (0 dB, where a new set starts) and 0 silence,
        /// up to 1.995 (+6 dB).
        #[schemars(range(min = 0.0, max = 1.995_262_314_968_879_5))]
        volume: f64,
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
    /// Sets a track's delay (the mixer spec, manual 18.7): positive delays
    /// the track, negative plays it early. Its value names its unit, as
    /// Hardware Latency's does; the unit switch converts the value. The
    /// command is unavailable while delay compensation is off.
    SetTrackDelay {
        /// The track.
        track: TrackId,
        /// The delay and the unit it is in.
        delay: TrackDelay,
    },
    /// Sets a track's pan mode (the mixer spec, manual 18.1): Stereo Pan Mode
    /// or Split Stereo Pan Mode.
    SetPanMode {
        /// The track.
        track: TrackId,
        /// `stereo` or `split`.
        mode: PanMode,
    },
    /// In Split Stereo Pan Mode, sets the left input channel's position, from
    /// −1 (left) to 1 (right), on the constant-power law.
    SetSplitPanLeft {
        /// The track.
        track: TrackId,
        /// The position, from −1 (left) through 0 (centre) to 1 (right).
        #[schemars(range(min = -1.0, max = 1.0))]
        pan: f64,
    },
    /// In Split Stereo Pan Mode, sets the right input channel's position, as
    /// [`Command::SetSplitPanLeft`].
    SetSplitPanRight {
        /// The track.
        track: TrackId,
        /// The position, from −1 (left) through 0 (centre) to 1 (right).
        #[schemars(range(min = -1.0, max = 1.0))]
        pan: f64,
    },
    /// Switches the set's delay compensation on or off (the delay-compensation
    /// spec, manual 23.3). While it's off, nothing is held back, no node's
    /// latency counts, and a track's delay has no effect.
    SetDelayCompensation {
        /// Whether compensation is on.
        enabled: bool,
    },
    /// Switches Reduced Latency When Monitoring (the delay-compensation spec,
    /// manual 23.3), which gives a monitored track the lowest possible
    /// latency. Only enabled while delay compensation is on.
    SetReducedLatencyMonitoring {
        /// Whether a monitored track is given the lowest possible latency.
        enabled: bool,
    },
    /// Sets a track's Keep Monitoring Latency (the delay-compensation spec,
    /// manual 18.8): whether a monitored track's recording keeps its delay
    /// compensation to match what is heard.
    SetKeepMonitoringLatency {
        /// The track.
        track: TrackId,
        /// Whether it keeps its compensation.
        enabled: bool,
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
        /// Another MIDI track (its ID), an output port with its channel
        /// (`{ "name": "Synth", "channel": 1 }`), or `null` (or nothing) for
        /// No Output.
        to: Option<MidiTo>,
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
    /// Adds a take lane under an audio or MIDI track's main lane, holding
    /// clips of the track's kind: after the lanes it has, or at `index`
    /// among them. IDs left out are issued.
    AddTakeLane {
        /// The audio or MIDI track.
        track: TrackId,
        /// The lane and its clips.
        lane: NewTakeLane,
        /// Where among the track's take lanes the new one goes, counted
        /// from 0 for the lane nearest the main lane. Left out, after the
        /// last.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// Puts a new clip on a take lane. Its data must match the lane's
    /// track.
    CreateTakeClip {
        /// The take lane.
        lane: TakeLaneId,
        /// The clip, with a new ID.
        clip: Clip,
    },
    /// Renames a take lane.
    RenameTakeLane {
        /// The take lane.
        lane: TakeLaneId,
        /// Its new name, which can't be empty.
        name: String,
    },
    /// Deletes take lanes with their clips.
    DeleteTakeLanes {
        /// The take lanes.
        lanes: Vec<TakeLaneId>,
    },
    /// Deletes every take lane of the tracks.
    DeleteAllTakeLanes {
        /// The audio or MIDI tracks.
        tracks: Vec<TrackId>,
    },
    /// Deletes each track's take lanes whose material its main lane doesn't
    /// use (the comping spec, R20).
    DeleteUnusedTakeLanes {
        /// The audio or MIDI tracks.
        tracks: Vec<TrackId>,
    },
    /// Moves take lanes, which must be some of one track's, to a new place
    /// among its take lanes, keeping their order.
    MoveTakeLanes {
        /// The take lanes.
        lanes: Vec<TakeLaneId>,
        /// The place they take among the track's other take lanes, counted
        /// from 0 for the lane nearest the main lane.
        index: usize,
    },
    /// Switches the audition of a take lane on or off. A track plays an
    /// auditioned lane in place of its main lane, and has at most one.
    SetAudition {
        /// The take lane.
        lane: TakeLaneId,
        /// Whether it is auditioned.
        on: bool,
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
    /// Moves a clip to a new start on a track's main lane or on one of its
    /// take lanes, out of the lane or Session slot it sits in. The track
    /// must be the take lane's own.
    MoveClipToLane {
        /// The clip.
        clip: ClipId,
        /// The track it moves to.
        track: TrackId,
        /// The take lane it moves to; the track's main lane when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lane: Option<TakeLaneId>,
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
    /// Colours clips, in the Arrangement, a Session slot or a take lane, with
    /// a swatch of the palette of 5 rows of 14 (0 to 69).
    SetClipColour {
        /// The clips, each listed once.
        clips: Vec<ClipId>,
        /// The swatch.
        colour: u8,
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
    /// Sets a clip's info text (Edit Info Text), in the Arrangement, a
    /// Session slot or a take lane. Empty clears it.
    SetClipInfoText {
        /// The clip.
        clip: ClipId,
        /// The text.
        info_text: String,
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
    /// Crops a clip to a region of its contents (the audio clip editing spec,
    /// R14, R15, R17): an audio clip plays `media`, the region's frames of
    /// its file, and a MIDI clip loses the notes that start outside the
    /// region. The clip's Start and End markers, loop brace, and warp markers
    /// move back by the region's start, so it plays exactly what it played,
    /// and the clip stays where it is in the Arrangement.
    CropClip {
        /// The clip.
        clip: ClipId,
        /// The new file: the region's frames, written through the
        /// processed-file writer. `null` (or nothing) for a MIDI clip, which
        /// writes no file; an audio clip's crop must name one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<ContentHash>,
        /// Where the region starts, in the form the clip shows: seconds into
        /// an unwarped audio clip's file, or whole ticks of a warped audio
        /// clip's or a MIDI clip's contents.
        from: f64,
        /// Where it ends, after `from`, in the same form.
        to: f64,
    },
    /// Crops a clip to a span of song time (the audio clip editing spec,
    /// R16): the clip's edges move to the span, and it plays the part of
    /// itself the span covered, at the same moments. An audio clip plays
    /// `media`, the span's frames of its file; a MIDI clip loses the notes
    /// the span didn't hold.
    CropClipToRange {
        /// The clip.
        clip: ClipId,
        /// The new file: the span's frames. `null` (or nothing) for a MIDI
        /// clip, which writes no file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<ContentHash>,
        /// Where the span starts in the song.
        start: Ticks,
        /// Where it ends, after `start`.
        end: Ticks,
    },
    /// Reverses an audio clip (the audio clip editing spec, R7 to R10): it
    /// plays `media`, a reversed copy of its file, and its Start and End
    /// markers, loop brace and warp markers are mirrored about the file, so
    /// it plays the same stretch of sound backwards. Its envelopes stay
    /// where they are, and its loop switch, gain, transpose, detune, warp
    /// mode and fades don't change.
    ReverseClip {
        /// The clip.
        clip: ClipId,
        /// The file it plays now: the reversed copy. `null` (or nothing)
        /// leaves the file as it is, for a clip whose file is already a
        /// reversed copy and goes back to the one it was made from.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<ContentHash>,
        /// The length of the file in seconds, which the markers are
        /// mirrored about.
        seconds: f64,
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
        /// loop is on, it must be before the loop brace's end. A warped
        /// clip's is whole ticks of its contents (`ticks` true).
        start: f64,
        /// The End marker, in seconds into the file, after `start`; `null`
        /// (or nothing) for the end of the file.
        #[serde(default)]
        end: Option<f64>,
        /// Whether `start` and `end` are ticks of a warped clip's contents
        /// instead of seconds in the file of an unwarped one: they must be
        /// the form the clip shows.
        #[serde(default)]
        ticks: bool,
    },
    /// Moves an audio clip's loop brace, leaving its Loop switch as it is.
    SetClipLoop {
        /// The audio clip.
        clip: ClipId,
        /// Where the brace starts, in seconds into the file: 0 or more. A
        /// warped clip's is whole ticks of its contents (`ticks` true).
        start: f64,
        /// Where it ends, in seconds into the file, after `start`; `null`
        /// (or nothing) for the end of the file.
        #[serde(default)]
        end: Option<f64>,
        /// Whether `start` and `end` are ticks of a warped clip's contents
        /// instead of seconds in the file of an unwarped one: they must be
        /// the form the clip shows.
        #[serde(default)]
        ticks: bool,
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
    /// Sets a MIDI clip's or a warped audio clip's time signature, the Clip
    /// View's Signature fields.
    SetClipSignature {
        /// The MIDI or warped audio clip.
        clip: ClipId,
        /// The clip's signature; `null` (or nothing) follows the song's at
        /// the clip's position.
        #[serde(default)]
        signature: Option<TimeSignature>,
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
    /// Sets the lengths and curves of an audio clip's fade-in and fade-out.
    SetClipFades {
        /// The audio clip.
        clip: ClipId,
        /// The fade-in's length in seconds from the clip's start: 0 for none.
        /// Where a crossfade begins at the clip's start, this is how far
        /// past the start it runs.
        #[schemars(range(min = 0.0))]
        fade_in: f64,
        /// The fade-out's length in seconds up to the clip's end: 0 for none.
        /// Where a crossfade ends at the clip's end, this is how far before
        /// the end it begins.
        #[schemars(range(min = 0.0))]
        fade_out: f64,
        /// The fade-in's curve, from −1 to 1: 0 is a straight line, a
        /// positive curve bows towards full level and a negative one towards
        /// silence. Left out, the curve stays as it is.
        #[serde(default)]
        #[schemars(range(min = -1.0, max = 1.0))]
        fade_in_curve: Option<f64>,
        /// The fade-out's curve, from −1 to 1. Left out, the curve stays as
        /// it is.
        #[serde(default)]
        #[schemars(range(min = -1.0, max = 1.0))]
        fade_out_curve: Option<f64>,
    },
    /// Makes or changes the crossfade between two audio clips on a track
    /// that meet at `boundary`.
    SetCrossfade {
        /// The track.
        track: TrackId,
        /// Where the earlier clip ends and the later one starts.
        boundary: Ticks,
        /// How many seconds before the boundary the crossfade starts.
        #[schemars(range(min = 0.0))]
        before: f64,
        /// How many seconds after the boundary the crossfade ends.
        #[schemars(range(min = 0.0))]
        after: f64,
        /// The curve of both fades, from −1 to 1: 0 is a straight line, so
        /// the two gains add to 1.
        #[serde(default)]
        #[schemars(range(min = -1.0, max = 1.0))]
        curve: f64,
    },
    /// Ends the crossfade between the two audio clips on a track that meet
    /// at `boundary`, leaving neither clip a fade there.
    DeleteCrossfade {
        /// The track.
        track: TrackId,
        /// Where the earlier clip ends and the later one starts.
        boundary: Ticks,
    },
    /// Switches an audio clip's Warp on or off (WP-4.1): its Start and End
    /// markers and loop brace convert through the warp map and it keeps its
    /// place and length.
    SetClipWarp {
        /// The audio clip.
        clip: ClipId,
        /// Whether the clip follows the set's tempo through its warp map.
        warp: bool,
    },
    /// Adds a warp marker to each of the warped clips, at the same place
    /// (WP-4.1): at a beat of the clip's contents, or at a time in its file.
    InsertWarpMarker {
        /// The warped audio clips: their files must be as long as each
        /// other's.
        clips: Vec<ClipId>,
        /// The beat, in ticks from the clip's 1.1.1, to put the marker on,
        /// at the time in the file the map already places there. Give this
        /// or `time`.
        #[serde(default)]
        beat: Option<Ticks>,
        /// The time in the file, in seconds, to put the marker at, on the
        /// beat the map already places it. Give this or `beat`.
        #[serde(default)]
        time: Option<f64>,
        /// Also put markers at the transients either side of the new one
        /// (WP-4.3): Cmd (Ctrl) held on a pseudo-warp marker. Needs one clip.
        #[serde(default)]
        pin_neighbours: bool,
        /// The clip's transients, as times in the file, that `pin_neighbours`
        /// looks among. Left out, the app uses the clip's current ones.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transients: Option<Vec<f64>>,
    },
    /// Puts warp markers in a range of a warped clip's contents (WP-4.3): at
    /// every transient in it, or at its start and its end when it holds none.
    InsertWarpMarkers {
        /// The warped audio clip.
        clip: ClipId,
        /// The start of the range, in ticks from the clip's 1.1.1.
        from: Ticks,
        /// The end of the range, in ticks, after `from`.
        to: Ticks,
        /// The clip's transients, as times in the file, that the range is
        /// searched for. Left out, the app uses the clip's current ones.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transients: Option<Vec<f64>>,
    },
    /// Puts a transient into an audio clip's file at a time (WP-4.3).
    InsertTransient {
        /// The audio clip.
        clip: ClipId,
        /// The time in the file, in seconds.
        #[schemars(range(min = 0.0))]
        time: f64,
    },
    /// Takes transients out of an audio clip (WP-4.3).
    DeleteTransients {
        /// The audio clip.
        clip: ClipId,
        /// The transients to take out, by their times in the file as
        /// `clip.transients` lists them.
        times: Vec<f64>,
    },
    /// Takes away an audio clip's transient edits (WP-4.3): the ones made by
    /// hand go, and the detected ones taken away come back.
    ResetTransients {
        /// The audio clip.
        clip: ClipId,
    },
    /// Moves a transient to another time in the file (WP-4.3); no audio
    /// changes.
    MoveTransient {
        /// The audio clip.
        clip: ClipId,
        /// The transient's time in the file, in seconds, as `clip.transients`
        /// lists it.
        #[schemars(range(min = 0.0))]
        from: f64,
        /// Where it goes, in seconds.
        #[schemars(range(min = 0.0))]
        to: f64,
    },
    /// Moves warp markers by the same ticks, each keeping its time in the
    /// file (WP-4.1).
    MoveWarpMarkers {
        /// The warped audio clips: their files must be as long as each
        /// other's.
        clips: Vec<ClipId>,
        /// The markers to move, by their place in the map, the first being 0.
        markers: Vec<usize>,
        /// How far to move them, in ticks: negative moves them earlier.
        by: Ticks,
    },
    /// Moves one warp marker's time in the file, keeping its beat (WP-4.1).
    ShiftWarpMarker {
        /// The warped audio clips: their files must be as long as each
        /// other's.
        clips: Vec<ClipId>,
        /// The marker, by its place in the map, the first being 0.
        marker: usize,
        /// How far to move its time in the file, in seconds: negative moves
        /// it earlier.
        by: f64,
    },
    /// Deletes warp markers (WP-4.1).
    DeleteWarpMarkers {
        /// The warped audio clips: their files must be as long as each
        /// other's.
        clips: Vec<ClipId>,
        /// The markers to delete, by their place in the map, the first being 0.
        markers: Vec<usize>,
    },
    /// Sets a warped clip's BPM field (WP-4.1).
    SetSegmentBpm {
        /// The warped audio clip.
        clip: ClipId,
        /// The marker whose segment to set, by its place in the map, the
        /// first being 0: `null` (or nothing) for the first segment's tempo,
        /// which scales every segment.
        #[serde(default)]
        marker: Option<usize>,
        /// The tempo in BPM: positive, and from 1 to 1,000 for the clip's.
        bpm: f64,
    },
    /// Multiplies every tempo of a warped clip by a factor, Live's ×2 and ÷2
    /// (WP-4.1).
    ScaleTempo {
        /// The warped audio clip.
        clip: ClipId,
        /// What to multiply by: 2 for ×2, 0.5 for ÷2.
        factor: f64,
    },
    /// Makes a point of a warped clip's file its 1.1.1 (WP-4.9, the warping
    /// spec R38): a warp marker is there, made if there was none, and every
    /// marker, the End marker and the loop brace move by the same ticks, so
    /// no segment's tempo changes; the Start marker goes to 1.1.1.
    SetOneOneOneHere {
        /// The warped audio clip.
        clip: ClipId,
        /// Where: the selected marker or the insert marker.
        #[serde(flatten)]
        at: WarpPoint,
    },
    /// Warps a warped clip from a point on (WP-4.9, R39): the markers up to
    /// it stay and those after it are replaced as `method` says.
    WarpFromHere {
        /// The warped audio clip.
        clip: ClipId,
        /// Where: the selected marker or the insert marker.
        #[serde(flatten)]
        at: WarpPoint,
        /// How the audio after the point is warped.
        method: WarpFrom,
        /// The tempo for `start_at` and `bpm`: the set's tempo at the point
        /// unless given.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bpm: Option<f64>,
        /// The file's tempo and beats. Left out, the app uses the file's
        /// analysis, and refuses while it is not ready.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        analysis: Option<FileBeats>,
    },
    /// Warps a warped clip's whole file as a loop of whole bars (WP-4.9,
    /// R40): a marker at the file's start on 1.1.1 and one at its end, the
    /// loop brace over the file, the loop on. The bar count is the one of 1,
    /// 2, 4, 8, 16 and 32 whose tempo is nearest the set's at the clip's
    /// start.
    WarpAsLoop {
        /// The warped audio clip.
        clip: ClipId,
        /// The file's length. Left out, the app reads it from the file, and
        /// refuses while the file's analysis is not ready.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        analysis: Option<FileBeats>,
    },
    /// Warps the part of a warped clip's file from `from` to `to` as a loop
    /// of whole bars (WP-4.9, R40): markers at its ends, the loop brace on
    /// it, the loop on.
    WarpSelectionAsLoop {
        /// The warped audio clip.
        clip: ClipId,
        /// The start of the part, in seconds in the file.
        #[schemars(range(min = 0.0))]
        from: f64,
        /// The end of the part, in seconds, after `from`.
        #[schemars(range(min = 0.0))]
        to: f64,
    },
    /// Switches an Arrangement clip's Lead/Follow switch (the warping spec,
    /// R43): a warped Arrangement audio clip can lead the set's tempo, so the
    /// set takes its tempo from the clip's warp markers and the clip plays as
    /// if unwarped. Refused for a Session clip and for an unwarped clip;
    /// switching a leader's Warp off makes it a follower in the same step.
    SetTempoLeader {
        /// The warped Arrangement audio clip.
        clip: ClipId,
        /// Whether the clip leads, or follows, the set's tempo.
        leader: bool,
    },
    /// Makes every leader a follower and writes the tempo they set into the
    /// Main track's Song Tempo envelope as ordinary breakpoints (R46), so the
    /// song plays exactly as it did. One undo step.
    UnfollowTempoAutomation {},
    /// Stretches a warped Arrangement clip (R47): the dragged edge goes to
    /// `at`, the other stays put, and every beat in its contents — its warp
    /// markers, its Start and End markers and its loop brace — scales about
    /// the beat at the fixed edge, so the same audio fills the new length.
    Stretch {
        /// The warped audio clip.
        clip: ClipId,
        /// The edge being dragged.
        edge: ClipEdge,
        /// Where that edge goes, in ticks: 0 (1.1.1) or later, and past the
        /// other edge.
        at: Ticks,
    },
    /// Slides a clip's contents inside the clip, which stays where it is
    /// (R54): its Start marker, its End marker and its loop brace each move
    /// the other way through its contents by `by`, and its warp markers keep
    /// their beats and their times in the file, so the same audio — its
    /// markers and its waveform with it — plays `by` ticks later (or earlier)
    /// while the clip keeps its place and length.
    SlideContents {
        /// The audio or MIDI clip.
        clip: ClipId,
        /// How far the contents move, in ticks: negative moves them earlier.
        by: Ticks,
    },
    /// Sets a warped or unwarped audio clip's warp mode (WP-4.2).
    SetClipWarpMode {
        /// The audio clip.
        clip: ClipId,
        /// The mode: Beats, Tones, Texture, Re-Pitch, Complex or Complex Pro.
        mode: WarpMode,
    },
    /// Sets controls of an audio clip's warp modes (WP-4.2): any of them, by
    /// mode, in one step.
    SetClipWarpSettings {
        /// The audio clip.
        clip: ClipId,
        /// Beats' controls to change.
        #[serde(default)]
        beats: Option<BeatsUpdate>,
        /// Tones' controls to change.
        #[serde(default)]
        tones: Option<TonesUpdate>,
        /// Texture's controls to change.
        #[serde(default)]
        texture: Option<TextureUpdate>,
        /// Complex Pro's controls to change.
        #[serde(default)]
        complex_pro: Option<ComplexProUpdate>,
    },
    /// Switches an audio clip's RAM mode (WP-4.2).
    SetClipRamMode {
        /// The audio clip.
        clip: ClipId,
        /// Whether the clip's whole file is held in memory.
        ram: bool,
    },
    /// Switches an audio clip's high-quality interpolation (WP-4.2).
    SetClipHighQuality {
        /// The audio clip.
        clip: ClipId,
        /// Whether the clip is resampled by the band-limited interpolator.
        high_quality: bool,
    },
    /// Sets an audio clip's Transpose and Detune (WP-4.7): either of them,
    /// or both, in one step.
    SetClipPitch {
        /// The audio clip.
        clip: ClipId,
        /// The Transpose, in whole semitones, from −48 to 48.
        #[serde(default)]
        transpose: Option<i32>,
        /// The Detune, in cents, from −50 to 50.
        #[serde(default)]
        detune: Option<f64>,
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
    /// Changes a Session clip's follow actions: the fields given change and
    /// the others stay (WP-3.10).
    SetClipFollowActions {
        /// The clip, which sits in a Session slot.
        clip: ClipId,
        /// The fields to change.
        #[serde(flatten)]
        patch: FollowPatch,
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
        /// device in its chain; or, in a MIDI clip, a MIDI controller
        /// (`midi.cc`, `midi.pitch_bend` or `midi.channel_pressure`).
        target: AutomationTarget,
        /// Which of the clip's envelopes for the parameter: `modulation`
        /// (the default) or a Session clip's `automation`.
        #[serde(default)]
        kind: EnvelopeKind,
        /// Where the breakpoint sits, in ticks from the beginning of the
        /// clip's contents, as a note's start is: 0 or later.
        at: Ticks,
        /// The value there, in the parameter's own units; a MIDI
        /// controller's own range (0 to 127, pitch bend -8192 to 8191).
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
        /// Which of the clip's envelopes for the parameter: `modulation`
        /// (the default) or a Session clip's `automation`.
        #[serde(default)]
        kind: EnvelopeKind,
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
        /// Which of the clip's envelopes for the parameter: `modulation`
        /// (the default) or a Session clip's `automation`.
        #[serde(default)]
        kind: EnvelopeKind,
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
        /// Which of the clip's envelopes for the parameter: `modulation`
        /// (the default) or a Session clip's `automation`.
        #[serde(default)]
        kind: EnvelopeKind,
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
        /// Which of the clip's envelopes for the parameter: `modulation`
        /// (the default) or a Session clip's `automation`.
        #[serde(default)]
        kind: EnvelopeKind,
    },
    /// Switches a clip envelope's Linked switch: linked, it follows the
    /// clip's region and loop; unlinked, it plays once from the clip's
    /// start. A parameter with no clip envelope gets an empty linked one.
    SetClipEnvelopeLinked {
        /// The clip.
        clip: ClipId,
        /// The parameter of the clip's track.
        target: AutomationTarget,
        /// Which of the clip's envelopes for the parameter: `modulation`
        /// (the default) or a Session clip's `automation`.
        #[serde(default)]
        kind: EnvelopeKind,
        /// Whether the envelope follows the clip's timeline.
        linked: bool,
    },
    /// Replaces one of a clip's envelopes with the breakpoints given, as the
    /// clip envelope editing commands do when they write a shape, a stretch or
    /// a paste over a span (the clip envelopes spec, R12, R13). The envelope
    /// keeps its Linked switch.
    SetClipEnvelope {
        /// The clip.
        clip: ClipId,
        /// The parameter of the clip's track.
        target: AutomationTarget,
        /// Which of the clip's envelopes for the parameter: `modulation`
        /// (the default) or a Session clip's `automation`.
        #[serde(default)]
        kind: EnvelopeKind,
        /// The envelope's breakpoints, in position order, each with its
        /// position in ticks from the beginning of the clip's contents, its
        /// value in the parameter's own units and the bend of the segment
        /// starting at it. Empty takes the envelope with it.
        points: Vec<Breakpoint>,
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
    /// Sets a scene's info text (Edit Info Text). Empty clears it.
    SetSceneInfoText {
        /// The scene.
        scene: SceneId,
        /// The text.
        info_text: String,
    },

    /// Colours scenes with a swatch of the palette of 5 rows of 14 (0 to
    /// 69), or clears their colour with `null`.
    SetSceneColour {
        /// The scenes, each listed once.
        scenes: Vec<SceneId>,
        /// The swatch, or `null` for none.
        colour: Option<u8>,
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
    /// Sets a device's info text (Edit Info Text). Empty clears it.
    SetDeviceInfoText {
        /// The device.
        device: DeviceId,
        /// The text.
        info_text: String,
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
        /// A Shaper's breakpoint shape (the modulation spec, R23), which
        /// a preset of one holds; `null` for the default shape.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shape: Option<Vec<crate::modulator::ShapePoint>>,
        /// An Articulation Map's articulations (the articulation map spec,
        /// R16), which a preset of one holds; `null` to keep the ones the
        /// device has.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        articulations: Option<Vec<crate::articulation::Articulation>>,
        /// A plug-in's MPE Mode and settings (the MPE spec, R14), which a
        /// preset of one holds; `null` to keep the ones the device has.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mpe: Option<crate::PluginMpe>,
    },
    /// Switches a plug-in device's MPE Mode (the MPE spec, R14): on, the
    /// plug-in gets each note's pitch, slide and pressure as its format takes
    /// them; off, only its pressure as polyphonic key pressure.
    SetPluginMpeMode {
        /// The device, which must be a plug-in.
        device: DeviceId,
        /// Whether MPE Mode is on.
        on: bool,
    },
    /// Switches a plug-in device's Apply Tuning to Per-Note Pitch Bend (the
    /// tunings spec, R20): on, a plug-in with MPE Mode on gets each note on
    /// the nearest twelve-step key of its tuned pitch with a per-note bend
    /// for the rest.
    SetPluginTuning {
        /// The device, which must be a plug-in.
        device: DeviceId,
        /// Whether the switch is on.
        on: bool,
    },
    /// Sets the MPE/Multi-channel Settings a plug-in device's MPE MIDI goes
    /// out on (the MPE spec, R18).
    SetPluginMpeSettings {
        /// The device, which must be a plug-in.
        device: DeviceId,
        /// The settings.
        settings: crate::MpeSettings,
    },
    /// Floats a device in a window of its own, or docks it back in the
    /// Device View (IMP-14, WP-2.5b). Only native devices float.
    SetDeviceFloating {
        /// The device.
        device: DeviceId,
        /// Float (true) or dock (false).
        floating: bool,
    },
    /// Places a device in another position of its chain (IMP-24): a plug-in
    /// that sends notes on can sit in the MIDI-effect position, before the
    /// instrument, and go back where its type goes. The placement is saved
    /// with the set.
    SetDeviceRole {
        /// The device.
        device: DeviceId,
        /// Where it goes in the chain; the other position if left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<DeviceRole>,
    },
    /// Switches a device on or off.
    SetDeviceEnabled {
        /// The device.
        device: DeviceId,
        /// Whether it's on.
        enabled: bool,
    },
    /// Makes the other of a device's A and B states the active one
    /// (device compare R3). Only native devices have two states.
    CompareSwitch {
        /// The device.
        device: DeviceId,
    },
    /// Copies the active one of a device's A and B states over the other
    /// (device compare R4). Only native devices have two states.
    CompareCopy {
        /// The device.
        device: DeviceId,
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
    /// Replaces a plug-in device's configured parameter list (clap-hosting
    /// R5, configure mode): the parameters its panel knobs and the choosers
    /// show, in order. Native devices ignore it.
    SetConfiguredParams {
        /// The device.
        device: DeviceId,
        /// The parameters in their new order.
        params: Vec<ParamId>,
    },
    /// Maps a slot of a modulator to a parameter (the modulation spec, R15):
    /// the Map button's gesture. The slot's earlier mapping goes.
    MapModulator {
        /// The modulator device.
        device: DeviceId,
        /// The slot, from 0 to 7.
        slot: u8,
        /// The parameter to map: a mixer control or send of any track, or a
        /// parameter of any device but the modulator's own.
        target: crate::modulator::ModTarget,
    },
    /// Removes the mapping of a modulator's slot (R15): the Unmap button.
    UnmapModulator {
        /// The modulator device.
        device: DeviceId,
        /// The slot, from 0 to 7.
        slot: u8,
    },
    /// Replaces the breakpoint shape of a Shaper (R23): every edit of
    /// its display, Clear and the presets make a new list.
    SetShape {
        /// The Shaper device.
        device: DeviceId,
        /// The breakpoints, in order along the cycle (at most 64), each with
        /// its place and output from 0 to 1 and the bend of the segment
        /// starting there, from −1 to 1.
        points: Vec<crate::modulator::ShapePoint>,
    },
    /// Adds an articulation to an Articulation Map's list (WP-6.47; the
    /// articulation map spec, R16).
    AddArticulation {
        /// The device.
        device: DeviceId,
        /// Where in the list it goes, or `null` for the end.
        #[serde(default)]
        index: Option<u8>,
        /// Its name. An empty name is the numbered one a new articulation
        /// gets.
        name: String,
    },
    /// Removes an articulation from an Articulation Map's list (the
    /// articulation map spec, R16).
    DeleteArticulation {
        /// The device.
        device: DeviceId,
        /// Its place in the list, from 0.
        index: u8,
    },
    /// Moves an articulation in an Articulation Map's list (the articulation
    /// map spec, R16).
    MoveArticulation {
        /// The device.
        device: DeviceId,
        /// Its place in the list, from 0.
        index: u8,
        /// Where it goes: the articulation there takes its place, or it goes
        /// last when `null`.
        #[serde(default)]
        to: Option<u8>,
    },
    /// Changes an articulation of an Articulation Map's list: its name, its
    /// trigger, its outputs or its channel (the articulation map spec, R16).
    /// What isn't given stays as it is.
    SetArticulation {
        /// The device.
        device: DeviceId,
        /// Its place in the list, from 0.
        index: u8,
        /// Its new name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Its new trigger.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger: Option<crate::articulation::ArticulationTrigger>,
        /// Its new outputs, in order.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outputs: Option<Vec<crate::articulation::ArticulationOutput>>,
        /// Its new channel: a channel from 1 to 16, or `null` for none, so
        /// its notes keep the channel they come in on.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::articulation::double_option"
        )]
        channel: Option<Option<u8>>,
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
        /// The sample slot to set: 0, the only one the sampler has, unless
        /// given; the eight-slot drum sampler's are 0 to 7 (WP-5.17).
        #[serde(default)]
        slot: u8,
    },
    /// Switches a device's Warp on or off (WP-5.15b).
    SetDeviceWarp {
        /// The device.
        device: DeviceId,
        /// Whether Warp is on.
        warp: bool,
    },
    /// Sets a device's warp mode (WP-5.15b).
    SetDeviceWarpMode {
        /// The device.
        device: DeviceId,
        /// The mode: Beats, Tones, Texture, Re-Pitch, Complex or Complex Pro.
        mode: WarpMode,
    },
    /// Sets controls of a device's warp modes (WP-5.15b).
    SetDeviceWarpSettings {
        /// The device.
        device: DeviceId,
        /// Beats' controls to change.
        #[serde(default)]
        beats: Option<BeatsUpdate>,
        /// Tones' controls to change.
        #[serde(default)]
        tones: Option<TonesUpdate>,
        /// Texture's controls to change.
        #[serde(default)]
        texture: Option<TextureUpdate>,
        /// Complex Pro's controls to change.
        #[serde(default)]
        complex_pro: Option<ComplexProUpdate>,
    },
    /// Sets a device's sample tempo in BPM (WP-5.15b).
    SetDeviceSampleBpm {
        /// The device.
        device: DeviceId,
        /// The tempo in BPM: positive, from 1 to 1,000.
        bpm: f64,
    },
    /// Multiplies a device's sample tempo by a factor (Live's ×2 and ÷2, WP-5.15b).
    ScaleDeviceTempo {
        /// The device.
        device: DeviceId,
        /// What to multiply by: 2 for ×2, 0.5 for ÷2.
        factor: f64,
    },
    /// Adds a manual slice at `frame` in the sample (WP-5.15b).
    AddSlice {
        /// The device.
        device: DeviceId,
        /// Frame into the sample.
        frame: u64,
    },
    /// Moves a manual slice from `from` to `to` frame (WP-5.15b).
    MoveSlice {
        /// The device.
        device: DeviceId,
        /// The existing slice's frame.
        from: u64,
        /// The new frame.
        to: u64,
    },
    /// Deletes a manual slice at `frame` (WP-5.15b).
    DeleteSlice {
        /// The device.
        device: DeviceId,
        /// Frame of the slice to delete.
        frame: u64,
    },
    /// Sets the region of a device's sample, between its Start and End flags
    /// (the sampler spec, R22): `None` is the whole sample. Start must be
    /// before End; whether a flag is past the sample's end is for the caller
    /// that knows the sample, `device.set_sample_region`, which clamps it.
    SetDeviceRegion {
        /// The device.
        device: DeviceId,
        /// The flags, or `null` for the whole sample.
        region: Option<SampleRegion>,
    },
    /// Adds layers to a multisampler (WP-5.13; the multisampler spec, R3), at
    /// `index` or at the end. Each layer's own `id` is ignored: the device
    /// gives each the next of its layer IDs, in order, and they read back
    /// from the device's layers. Every setting must be in range.
    InsertLayers {
        /// The device.
        device: DeviceId,
        /// Where in the layer list the first goes, or `null` for the end.
        index: Option<usize>,
        /// The layers.
        layers: Vec<Layer>,
    },
    /// Removes layers from a multisampler.
    DeleteLayers {
        /// The device.
        device: DeviceId,
        /// The layers to remove.
        layers: Vec<LayerId>,
    },
    /// Copies layers, each copy straight after its original with a new ID.
    DuplicateLayers {
        /// The device.
        device: DeviceId,
        /// The layers to copy.
        layers: Vec<LayerId>,
    },
    /// Renames a layer.
    RenameLayer {
        /// The device.
        device: DeviceId,
        /// The layer.
        layer: LayerId,
        /// Its new name.
        name: String,
    },
    /// Moves layers, keeping their order, so the first sits at `index`
    /// among the layers that stay.
    MoveLayers {
        /// The device.
        device: DeviceId,
        /// The layers to move.
        layers: Vec<LayerId>,
        /// Where the first goes among the layers left.
        index: usize,
    },
    /// Sorts a multisampler's layers.
    SortLayers {
        /// The device.
        device: DeviceId,
        /// What to sort by.
        by: LayerSort,
        /// Whether the largest comes first.
        descending: bool,
    },
    /// Sets one kind of zone of layers (R10): the key, velocity or sample
    /// select zone, with its fades. Every zone must hold at least one value
    /// of its scale (keys 0 to 127, velocities 1 to 127, select 0 to 127)
    /// and its fades stay inside it.
    SetLayerZones {
        /// The device.
        device: DeviceId,
        /// Which zone.
        kind: ZoneKind,
        /// Each layer's new zone.
        zones: Vec<LayerZone>,
    },
    /// Changes settings of layers, exactly as given with no snapping to
    /// zero crossings (`device.set_layer` snaps while Snap is on): Root Key,
    /// Detune, Volume, Pan, the markers, Link, the loop modes, the loop's
    /// Crossfade and Detune. Settings left out stay as they were; one
    /// setting out of range refuses the whole change.
    UpdateLayers {
        /// The device.
        device: DeviceId,
        /// The layers to change.
        layers: Vec<LayerId>,
        /// The settings to set.
        settings: LayerSettings,
    },
    /// Replaces a layer's sample, keeping its zones and settings, and its
    /// markers in the same places relative to the sample's length. A layer
    /// whose media the store doesn't hold plays silence.
    SetLayerMedia {
        /// The device.
        device: DeviceId,
        /// The layer.
        layer: LayerId,
        /// The new sample, by content hash.
        media: ContentHash,
        /// Its length in frames at its own rate.
        length: u64,
        /// Its own rate in hertz.
        sample_rate: u32,
        /// Its channels.
        channels: u8,
    },
    /// Adds an edit to the end of a sample's edit list (WP-5.15, R55): a
    /// layer's, or with no `layer` the sample in slot 0 of a device such as
    /// Simpler. A layer's length and markers follow the audio (R56). The
    /// selection must be a span of the sample as the earlier edits left it;
    /// a layer's is checked here, a device's by `device.edit_sample`, which
    /// knows the sample.
    AddSampleEdit {
        /// The device.
        device: DeviceId,
        /// The layer, or `null` for the device's own sample.
        layer: Option<LayerId>,
        /// The edit.
        edit: SampleEdit,
    },
    /// Clears a sample's edit list (R58). The sample is `length` frames long
    /// again, which a layer's markers keep their share of; a device's own
    /// sample ignores `length`.
    ClearSampleEdits {
        /// The device.
        device: DeviceId,
        /// The layer, or `null` for the device's own sample.
        layer: Option<LayerId>,
        /// The unedited sample's length in frames at its own rate.
        length: u64,
    },
    /// Replaces a layer's sample with a processed copy of its edited sample
    /// (R59), which holds the edits already: the list empties. `crop` is the
    /// span of the edited sample the copy keeps, which moves the markers as
    /// a Trim does; a copy of the whole sample leaves them in place.
    SetProcessedLayerSample {
        /// The device.
        device: DeviceId,
        /// The layer.
        layer: LayerId,
        /// The copy, by content hash.
        media: ContentHash,
        /// Its length in frames at its own rate.
        length: u64,
        /// The edited sample's span the copy keeps, or `null` for all of it.
        crop: Option<SampleRange>,
    },
    /// Spreads layers' ranges (R12): equally over the range of the zone
    /// editor in view (`kind`), or key zones around the layers' root keys,
    /// without fades. With one layer or none listed, every layer.
    DistributeLayers {
        /// The device.
        device: DeviceId,
        /// The layers to spread.
        layers: Vec<LayerId>,
        /// How to spread them.
        how: Distribution,
        /// The zone editor in view: which zone `equally` spreads.
        kind: ZoneKind,
    },
    /// Sets the options of a multisampler's layer list that aren't
    /// parameters: RAM Mode, Snap, and the title bar's Use Constant Power
    /// Fade for Loops (`linear_loop_fades`: true when it's off). Options
    /// left out stay as they were.
    SetLayerOptions {
        /// The device.
        device: DeviceId,
        /// RAM Mode: decode every layer whole into memory.
        ram_mode: Option<bool>,
        /// Snap: markers land on zero crossings.
        snap: Option<bool>,
        /// Whether loop crossfades are linear rather than constant-power.
        linear_loop_fades: Option<bool>,
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
    /// Sets the MIDI track a device's MIDI sidechain listens to, or none.
    /// The device must have a MIDI sidechain input.
    SetMidiSidechainSource {
        /// The device.
        device: DeviceId,
        /// The MIDI track whose notes and channel messages the device
        /// hears (its own track too), or `None` for No Input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackId>,
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
    /// Sets an external device's routing (WP-5.34): the settings given
    /// change, the others stay. The device must declare routing to external
    /// hardware.
    SetExternal {
        /// The device.
        device: DeviceId,
        /// MIDI To: `none` for No Output, or `port` with the MIDI output
        /// port's name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        midi_to: Option<ExternalPort>,
        /// The MIDI channel, 1 to 16.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(range(min = 1, max = 16))]
        channel: Option<u8>,
        /// MPE, chosen in place of a channel, with the MPE/Multi-channel
        /// Settings the notes go out on (the MPE spec, R19). Giving a
        /// `channel` instead sends on that channel again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mpe: Option<crate::MpeSettings>,
        /// Audio From: the audio interface's inputs it comes back on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio_from: Option<Channel>,
        /// Audio To: the audio interface's outputs an external effect sends
        /// its signal to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio_to: Option<Channel>,
        /// Hardware Latency with its unit: the unit switch is the unit
        /// given, so switching the unit sends the converted value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hardware_latency: Option<HardwareLatency>,
        /// A CV device's outputs (`docs/specs/cv-tools.md`, R4): each named
        /// by its name in the device (Pitch, Gate, Mod 1 and so on) and set
        /// to an interface channel, counted from 1, or to `null` for No
        /// Output. The outputs left out stay as they are.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cv_outputs: Option<Vec<crate::external::CvPort>>,
        /// A CV device's inputs, as `cv_outputs`; `null` is No Input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cv_inputs: Option<Vec<crate::external::CvPort>>,
    },
    /// Stores a CV Instrument's calibration, or takes it away with `null`
    /// (`docs/specs/cv-tools.md`, R14).
    SetCvCalibration {
        /// The device.
        device: DeviceId,
        /// The calibration, or none to clear it.
        calibration: Option<CvCalibration>,
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
    /// Sets one of a chain's zones (the racks spec, R30): the key zone
    /// (notes 0 to 127), the velocity zone (1 to 127) or the chain select
    /// zone (0 to 127), with its two fades. The zone holds at least one
    /// value of its scale and its fades stay inside it. A chain of an audio
    /// effect rack has only a chain select zone, and a drum rack's none.
    SetChainZone {
        /// The chain.
        chain: ChainId,
        /// Which zone.
        kind: ZoneKind,
        /// The new zone.
        zone: Zone,
    },
    /// Sets the note a drum chain answers (drum racks R1): one note, or all
    /// notes, which pass as they came.
    SetChainReceive {
        /// The drum chain.
        chain: ChainId,
        /// What it answers.
        receive: Receive,
    },
    /// Sets the note a drum chain passes on to its devices, from 0 to 127.
    SetChainPlay {
        /// The drum chain.
        chain: ChainId,
        /// The note.
        play: u8,
    },
    /// Puts a drum chain in a choke group, from 1 to 16, or in none.
    SetChainChoke {
        /// The drum chain.
        chain: ChainId,
        /// The group, or `null` for none.
        choke: Option<u8>,
    },
    /// Sets a drum chain's send to one of its rack's return chains as
    /// linear gain (finite, at least 0; 0 is −∞ dB). Moving it by hand turns
    /// off its automation envelope.
    SetChainSend {
        /// The drum chain.
        chain: ChainId,
        /// The return chain.
        to: ChainId,
        /// The gain.
        level: f64,
    },
    /// Sets where a return chain sends its output: one of the set's return
    /// tracks, or `null` for the rack's own output.
    SetChainAudioTo {
        /// The return chain.
        chain: ChainId,
        /// The return track, or `null`.
        track: Option<TrackId>,
    },
    /// Maps a parameter inside a rack to one of its macros (macros spec,
    /// R5), moving a mapping it already has to this macro. A new mapping
    /// runs the whole of the parameter's travel.
    MapMacro {
        /// The rack.
        rack: DeviceId,
        /// The macro, numbered from 1.
        macro_number: u8,
        /// The parameter to map.
        target: MacroTarget,
    },
    /// Unmaps a parameter from its rack's macro (macros spec, R9).
    UnmapMacro {
        /// The rack.
        rack: DeviceId,
        /// The parameter to unmap.
        target: MacroTarget,
    },
    /// Sets a mapping's range as positions on the parameter's travel, 0 to 1
    /// (macros spec, R9). Min above Max inverts the mapping.
    SetMappingRange {
        /// The rack.
        rack: DeviceId,
        /// The mapped parameter.
        target: MacroTarget,
        /// The bottom of the range.
        min: f64,
        /// The top of the range.
        max: f64,
    },
    /// Swaps a mapping's Min and Max (macros spec, R9).
    InvertMapping {
        /// The rack.
        rack: DeviceId,
        /// The mapped parameter.
        target: MacroTarget,
    },
    /// Gives a macro a name of its own; empty gives it its default name
    /// (macros spec, R14).
    RenameMacro {
        /// The rack.
        rack: DeviceId,
        /// The macro, numbered from 1.
        macro_number: u8,
        /// The name, or empty.
        name: String,
    },
    /// Colours a macro with a swatch of the palette of 5 rows of 14 (0 to
    /// 69), or `null` to clear it (macros spec, R14).
    SetMacroColour {
        /// The rack.
        rack: DeviceId,
        /// The macro, numbered from 1.
        macro_number: u8,
        /// The swatch, or `null` for none.
        colour: Option<u8>,
    },
    /// Sets a macro's info text (macros spec, R14). Empty clears it.
    SetMacroInfo {
        /// The rack.
        rack: DeviceId,
        /// The macro, numbered from 1.
        macro_number: u8,
        /// The text.
        info: String,
    },
    /// Sets every mapped macro that isn't excluded from Randomize to a random
    /// position (macros spec, R15). With a `seed` the same values come back
    /// each time.
    RandomizeMacros {
        /// The rack.
        rack: DeviceId,
        /// The seed; without one a new seed is picked.
        seed: Option<u64>,
    },
    /// Leaves a macro out of Randomize, or puts it back (macros spec, R16).
    SetMacroRandomizeExcluded {
        /// The rack.
        rack: DeviceId,
        /// The macro, numbered from 1.
        macro_number: u8,
        /// Whether it's left out.
        excluded: bool,
    },
    /// Sets how a macro is drawn and which positions it takes (macros spec,
    /// R21 and R22): `knob`, Live's continuous control, `stepped`, a knob
    /// that takes `steps` steps, or `radio_buttons`, a stepped macro drawn
    /// as its steps' buttons. A macro that becomes stepped moves to its
    /// nearest step.
    SetMacroStyle {
        /// The rack.
        rack: DeviceId,
        /// The macro, numbered from 1.
        macro_number: u8,
        /// The style.
        style: MacroStyle,
        /// How many steps a stepped macro takes, from 2 to 16; the macro's
        /// own number of steps when left out.
        steps: Option<u8>,
    },
    /// Sets a parameter to the value a macro gives it (macros spec, R7),
    /// without overriding the parameter's own automation envelope, which
    /// waits while it's mapped (R11). The macro commands run it inside their
    /// undo step.
    SetMappedValue {
        /// The mapped parameter.
        target: MacroTarget,
        /// The value, in the parameter's own units.
        value: f64,
    },
    /// Stores every macro of a rack's values as a new variation at the end of
    /// its list (macros spec, R26).
    StoreVariation {
        /// The rack.
        rack: DeviceId,
    },
    /// Sets every macro of a rack that isn't excluded from variations to the
    /// value a variation holds, at once and on one frame, as a hand move on
    /// each (macros spec, R27).
    LaunchVariation {
        /// The rack.
        rack: DeviceId,
        /// The variation's place in the rack's list, from 0.
        index: usize,
    },
    /// Replaces every value a variation holds with the macros' values now,
    /// keeping its name and place (macros spec, R28).
    OverwriteVariation {
        /// The rack.
        rack: DeviceId,
        /// The variation's place in the rack's list, from 0.
        index: usize,
    },
    /// Gives a variation a name of its own (macros spec, R29).
    RenameVariation {
        /// The rack.
        rack: DeviceId,
        /// The variation's place in the rack's list, from 0.
        index: usize,
        /// The name.
        name: String,
    },
    /// Copies a variation, with its name and values, straight after it
    /// (macros spec, R29).
    DuplicateVariation {
        /// The rack.
        rack: DeviceId,
        /// The variation's place in the rack's list, from 0.
        index: usize,
    },
    /// Deletes a variation (macros spec, R29).
    DeleteVariation {
        /// The rack.
        rack: DeviceId,
        /// The variation's place in the rack's list, from 0.
        index: usize,
    },
    /// Leaves a macro out of variations, or puts it back (macros spec, R30).
    SetMacroVariationExcluded {
        /// The rack.
        rack: DeviceId,
        /// The macro, numbered from 1.
        macro_number: u8,
        /// Whether it's left out.
        excluded: bool,
    },
    /// Sets a clip's scale (the scales spec, R5): its Scale Mode switch, its
    /// root and its scale name. It leaves the values it isn't given as they
    /// were, and it is one undo step however many clips it sets.
    SetClipScale {
        /// The clips.
        clips: Vec<ClipId>,
        /// Whether the clips' Scale Mode is on.
        #[serde(default)]
        enabled: Option<bool>,
        /// The root note: 0 (C) to 11 (B).
        #[serde(default)]
        root: Option<u8>,
        /// The scale's name, from the scale list.
        #[serde(default)]
        name: Option<String>,
    },
    /// Sets the set's current scale (the scales spec, R9): its Scale Mode
    /// switch, its root and its scale name, which the Control Bar's scale
    /// controls show and the clips made next take.
    SetScale {
        /// Whether the set's Scale Mode is on.
        #[serde(default)]
        enabled: Option<bool>,
        /// The root note: 0 (C) to 11 (B).
        #[serde(default)]
        root: Option<u8>,
        /// The scale's name, from the scale list.
        #[serde(default)]
        name: Option<String>,
    },
    /// Makes a tuning the set's, in place of any other (the tunings spec,
    /// R2 and R15). With `retune`, every note of the MIDI clips on tracks
    /// that play the tuning moves to the key whose new pitch is nearest the
    /// pitch it had, in the same undo step.
    SetTuning {
        /// The tuning.
        tuning: Tuning,
        /// Whether the notes are retuned.
        #[serde(default)]
        retune: bool,
    },
    /// Removes the set's tuning, which returns it to twelve equal steps
    /// (the tunings spec, R9 and R15), retuning the notes as `retune` says.
    RemoveTuning {
        /// Whether the notes are retuned.
        #[serde(default)]
        retune: bool,
    },
    /// Changes the set's tuning's reference pitch (the tunings spec, R7): its
    /// octave, its degree, its frequency in Hz. Changing the octave or the
    /// degree names another note as the reference and leaves every pitch as
    /// it was; changing the frequency moves every pitch by one ratio. A
    /// value left out stays as it was.
    SetTuningReference {
        /// The reference note's octave.
        #[serde(default)]
        octave: Option<i32>,
        /// The reference note's degree.
        #[serde(default)]
        degree: Option<u32>,
        /// The reference note's frequency in Hz.
        #[serde(default)]
        frequency: Option<f64>,
    },
    /// Changes the set's tuning's note range (the tunings spec, R8): the
    /// lowest note, which key 0 plays, and the highest. Given one, the
    /// other moves by as many notes, so the count stays; given both, they
    /// are the range, which holds at most 128 notes.
    SetTuningRange {
        /// The lowest note.
        #[serde(default)]
        lowest: Option<TuningNote>,
        /// The highest note.
        #[serde(default)]
        highest: Option<TuningNote>,
    },
    /// Switches a MIDI track's Bypass Tuning on or off (the tunings spec,
    /// R14): on, the track plays twelve equal steps whatever tuning the set
    /// holds.
    SetTuningBypass {
        /// The MIDI track.
        track: TrackId,
        /// Whether it bypasses the tuning.
        bypass: bool,
    },
    /// Sets a MIDI track's MIDI Controller Layout and its custom table (the
    /// tunings spec, R22, R23); what is left out stays.
    SetTuningLayout {
        /// The MIDI track.
        track: TrackId,
        /// The mapping to use.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<crate::LayoutKind>,
        /// The custom table: 128 entries, the key of the tuning each
        /// incoming key plays or `null` for none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Option<u8>>>,
    },
    /// Puts audio and MIDI tracks into the link group `group`, or takes them
    /// out of their groups when it is left out (the linked-track editing
    /// spec, R2, R3, R5). A track leaves the group it was in, and a group
    /// left with one track ends.
    SetTrackLinks {
        /// The audio or MIDI tracks.
        tracks: Vec<TrackId>,
        /// The group they join; an existing group to add to it, or a new
        /// ID for a new one. Left out, the tracks are unlinked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<LinkGroupId>,
    },
    /// Adds a groove to the end of the Groove Pool (the grooves spec, R1):
    /// what loading a groove file and extracting a groove make.
    AddGroove {
        /// The groove, with a new ID.
        groove: Groove,
    },
    /// Takes a groove out of the pool, sets every clip that used it to none
    /// and clears the auto-load groove if it was that one (R5).
    DeleteGroove {
        /// The groove.
        groove: GrooveId,
    },
    /// Renames a groove (R5).
    RenameGroove {
        /// The groove.
        groove: GrooveId,
        /// The new name.
        name: String,
    },
    /// Sets any of a groove's parameters, keeping those left out (R5).
    SetGroove {
        /// The groove.
        groove: GrooveId,
        /// The grid its notes are measured against.
        #[serde(default)]
        base: Option<GrooveBase>,
        /// Quantize, from 0 to 100 percent.
        #[serde(default)]
        quantize: Option<f64>,
        /// Timing, from 0 to 100 percent.
        #[serde(default)]
        timing: Option<f64>,
        /// Random, from 0 to 100 percent.
        #[serde(default)]
        random: Option<f64>,
        /// Velocity, from -100 to +100 percent.
        #[serde(default)]
        velocity: Option<f64>,
    },
    /// Sets the Global Amount, from 0 to 130 percent (R5).
    SetGlobalGrooveAmount {
        /// The amount, in percent.
        amount: f64,
    },
    /// Sets the Auto Load Groove switch and its groove (R20), keeping what
    /// is left out.
    SetAutoLoadGroove {
        /// Whether a new MIDI clip takes the auto-load groove.
        #[serde(default)]
        enabled: Option<bool>,
        /// The auto-load groove, one of the pool's.
        #[serde(default)]
        groove: Option<GrooveId>,
    },
    /// Sets the groove of clips to one of the pool's, or to none (R3).
    SetClipGroove {
        /// The clips; left out, the ones selected.
        #[serde(default)]
        clips: Vec<ClipId>,
        /// The groove; left out, none.
        #[serde(default)]
        groove: Option<GrooveId>,
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
            Self::AddLocator { .. } => "Add Locator",
            Self::DeleteLocator { .. } => "Delete Locator",
            Self::MoveLocator { .. } => "Move Locator",
            Self::RenameLocator { .. } => "Rename Locator",
            Self::SetLocatorInfoText { .. } => "Edit Info Text",
            Self::LoopToNextLocator { .. } => "Loop to Next Locator",
            Self::SetSongStart { .. } => "Set Song Start Time",
            Self::InsertTime { .. } => "Insert Silence",
            Self::DeleteTime { .. } => "Delete Time",
            Self::SetPunchIn { .. } => "Punch-In",
            Self::SetPunchOut { .. } => "Punch-Out",
            Self::SetMetronomeEnabled { .. } => "Metronome",
            Self::SetMetronomeCountIn { .. } => "Metronome Count-In",
            Self::SetMetronomeSound { .. } => "Metronome Sound",
            Self::SetMetronomeRhythm { .. } => "Metronome Rhythm",
            Self::SetMetronomeOnlyWhileRecording { .. } => "Hear Metronome Only While Recording",
            Self::SetMidiOverdub { .. } => "MIDI Arrangement Overdub",
            Self::SetRecordQuantization { .. } => "Record Quantization",
            Self::CreateTrack { .. } => "Create track",
            Self::DeleteTrack { .. } => "Delete track",
            Self::MoveTrack { .. } => "Move track",
            Self::RenameTrack { .. } => "Rename track",
            // Spec: info-view R10. Live's name for the menu item.
            Self::SetTrackInfoText { .. }
            | Self::SetClipInfoText { .. }
            | Self::SetSceneInfoText { .. }
            | Self::SetDeviceInfoText { .. } => "Change Info Text",
            // Spec: themes R16, R17. Beatlayer's own names.
            Self::SetTrackColour { .. } => "Change Track Color",
            Self::AssignColourToClips { .. } => "Assign Track Color to Clips",
            Self::AssignColourToGrouped { .. } => "Assign Track Color to Grouped Tracks and Clips",
            Self::SetClipColour { .. } => "Change Clip Color",
            Self::SetSceneColour { .. } => "Change Scene Color",
            Self::FreezeTracks { .. } => "Freeze tracks",
            Self::UnfreezeTracks { .. } => "Unfreeze tracks",
            Self::BounceTrackInPlace { .. } => "Bounce track in place",
            Self::BounceToNewTrack { .. } => "Bounce to new track",
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
            Self::SetAutomationEnvelope { .. } => "Set Automation Envelope",
            Self::DeleteAutomation { .. } => "Delete Automation",
            Self::ClearTrackAutomation { .. } => "Clear All Envelopes",
            Self::ClearDeviceAutomation { .. } => "Clear Envelopes",
            Self::ReEnableAutomation { .. } | Self::ReEnableAutomationFor { .. } => {
                "Re-Enable Automation"
            }
            Self::SetCrossfader { .. } => "Change crossfader",
            Self::SetCrossfadeAssign { .. } => "Assign crossfade",
            Self::SetTrackDelay { .. } => "Change track delay",
            Self::SetPanMode { .. } => "Change track pan mode",
            Self::SetSplitPanLeft { .. } | Self::SetSplitPanRight { .. } => "Change split pan",
            Self::SetDelayCompensation { .. } => "Switch Delay Compensation",
            Self::SetReducedLatencyMonitoring { .. } => "Switch Reduced Latency When Monitoring",
            Self::SetKeepMonitoringLatency { .. } => "Switch Keep Monitoring Latency",
            Self::SetSoloInPlace { .. } => "Switch Solo in Place",
            Self::SetLockEnvelopes { .. } => "Switch Lock Envelopes",
            Self::SetRemoteMappings { .. } => "Change MIDI and key mappings",
            Self::SetCueMode { .. } => "Switch Cue mode",
            Self::SetCueVolume { .. } => "Change \"Preview Volume\"",
            Self::SetInput { .. }
            | Self::SetOutput { .. }
            | Self::SetMidiOutput { .. }
            | Self::SetMainOut { .. }
            | Self::SetCueOut { .. } => "Change \"Routing\"",
            Self::SetMonitor { .. } => "Change monitoring",
            Self::CreateClip { .. } => "Create clip",
            Self::AddTakeLane { .. } => "Add take lane",
            Self::CreateTakeClip { .. } => "Create clip",
            Self::RenameTakeLane { .. } => "Rename Take Lane",
            Self::DeleteTakeLanes { .. } => "Delete Take Lane(s)",
            Self::DeleteAllTakeLanes { .. } => "Delete All Take Lanes",
            Self::DeleteUnusedTakeLanes { .. } => "Delete All Unused Take Lanes",
            Self::MoveTakeLanes { .. } => "Move Take Lane(s)",
            Self::SetAudition { .. } => "Audition Take Lane",
            Self::DeleteClip { .. } => "Delete clip",
            Self::MoveClip { .. } | Self::MoveClipToLane { .. } => "Move clip",
            Self::MoveClips { .. } => "Move clips",
            Self::SetClipLength { .. } => "Resize clip",
            Self::RenameClip { .. } => "Rename clip",
            Self::SetClipMute { .. } => "Mute clip",
            Self::SetClipLaunchMode { .. } => "Change launch mode",
            Self::SetClipData { .. } => "Edit clip",
            Self::CropClip { .. } | Self::CropClipToRange { .. } => "Crop clip",
            Self::ReverseClip { .. } => "Reverse clip",
            Self::TrimClipStart { .. } => "Resize clip",
            Self::SetClipRegion { .. } => "Change clip region",
            Self::SetClipLoop { .. } => "Change clip loop",
            Self::SetMidiClipRegion { .. } => "Change clip region",
            Self::SetMidiClipLoop { .. } => "Change clip loop",
            Self::SetClipSignature { .. } => "Change clip signature",
            Self::SetClipLoopEnabled { .. } => "Switch clip loop",
            Self::SetClipGain { .. } => "Change clip gain",
            Self::SetClipFades { .. } => "Change Fade",
            Self::SetCrossfade { .. } => "Change Fade",
            Self::DeleteCrossfade { .. } => "Delete Fade",
            // WP-4.1. Live's undo names for the warp edits are to observe;
            // Beatlayer's name the edit.
            Self::SetClipWarp { .. } => "Switch Warp",
            Self::InsertWarpMarker { .. } => "Insert Warp Marker",
            Self::InsertWarpMarkers { .. } => "Insert Warp Markers",
            Self::InsertTransient { .. } => "Insert Transient",
            Self::DeleteTransients { .. } => "Delete Transient",
            Self::ResetTransients { .. } => "Reset Transients",
            Self::MoveTransient { .. } => "Move Transient",
            Self::MoveWarpMarkers { .. } | Self::ShiftWarpMarker { .. } => "Move Warp Marker",
            Self::DeleteWarpMarkers { .. } => "Delete Warp Marker",
            Self::SetSegmentBpm { .. } | Self::ScaleTempo { .. } => "Change clip tempo",
            // WP-4.9. Live's undo names are to observe.
            Self::SetOneOneOneHere { .. } => "Set 1.1.1 Here",
            Self::WarpFromHere { .. } => "Warp From Here",
            Self::WarpAsLoop { .. } | Self::WarpSelectionAsLoop { .. } => "Warp as Loop",
            // WP-4.2. Live's names for these are to observe.
            Self::SetClipWarpMode { .. } => "Change Warp Mode",
            Self::SetClipWarpSettings { .. } => "Change Warp Settings",
            Self::SetClipRamMode { .. } => "Switch RAM Mode",
            Self::SetClipHighQuality { .. } => "Switch High Quality",
            // WP-4.7. Live's name for the pitch controls is the Clip View's.
            Self::SetClipPitch { .. } => "Change Clip Pitch",
            // WP-4.11. Live's undo names for these are to observe.
            Self::SetTempoLeader { .. } => "Switch Lead/Follow",
            Self::UnfollowTempoAutomation {} => "Unfollow Tempo Automation",
            Self::Stretch { .. } => "Stretch clip",
            Self::SlideContents { .. } => "Slide clip contents",
            // WP-3.4. Live's undo names for the Launch panel's controls are
            // to observe; Beatlayer's name the edit.
            Self::SetClipLegato { .. } => "Switch Legato",
            Self::SetClipLaunchQuantization { .. } => "Change clip quantization",
            Self::SetClipVelocityAmount { .. } => "Change velocity amount",
            // WP-3.10. Live's undo names are to observe.
            Self::SetClipFollowActions { .. } => "Change follow actions",
            // WP-3.15. Live's undo names for clip envelope edits are to
            // observe; Beatlayer's name the edit.
            Self::SetClipEnvelopeBreakpoint { .. } => "Add Envelope Breakpoint",
            Self::SetClipEnvelopeCurve { .. } => "Curve Envelope Segment",
            Self::MoveClipEnvelopeBreakpoint { .. } => "Move Envelope Breakpoint",
            Self::DeleteClipEnvelopeBreakpoint { .. } => "Delete Envelope Breakpoint",
            Self::DeleteClipEnvelope { .. } => "Clear Envelope",
            Self::SetClipEnvelopeLinked { .. } => "Link Envelope",
            Self::SetClipEnvelope { .. } => "Set Envelope",
            Self::CreateScene { .. } => "Create scene",
            Self::DeleteScene { .. } => "Delete scene",
            Self::MoveScene { .. } => "Move scene",
            // WP-3.1. Live's undo name for duplicating a scene is to
            // observe; Beatlayer's names the edit.
            Self::DuplicateScene { .. } => "Duplicate scene",
            Self::RenameScene { .. } => "Rename scene",
            Self::SetSceneTempo { .. } => "Change scene tempo",
            Self::SetSceneFollowActions { .. } => "Change follow actions",
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
            // Spec: mpe R14, R18. Beatlayer's own names.
            Self::SetPluginMpeMode { .. } => "Change MPE Mode",
            Self::SetPluginMpeSettings { .. } => "Change MPE Settings",
            // Spec: tunings R20. The menu item's label.
            Self::SetPluginTuning { .. } => "Apply Tuning to Per-Note Pitch Bend",
            Self::SetDeviceState { .. } => "Load Preset",
            Self::SetDeviceEnabled { .. } => "Change \"Device On\"",
            // IMP-14: Beatlayer's own, so Live has no name for them.
            Self::SetDeviceFloating { floating: true, .. } => "Float Device",
            Self::SetDeviceFloating {
                floating: false, ..
            } => "Dock Device",
            // IMP-24: Beatlayer's own, so Live has no name for it.
            Self::SetDeviceRole { .. } => "Change Device Role",
            // Spec: device-compare R3, R4; `label_in` names the direction.
            Self::CompareSwitch { .. } => "Compare: Switch",
            Self::CompareCopy { .. } => "Compare: Copy",
            Self::SetDeviceParam { .. } => "Change parameter",
            // WP-5.40: Beatlayer's own name; Live's undo history says
            // "Configure Plug-In".
            Self::SetConfiguredParams { .. } => "Configure Plug-In",
            // WP-2.16. Live's undo name for loading a Simpler sample is to
            // observe; Beatlayer's names the edit.
            // WP-5.10: Beatlayer's own names.
            Self::MapModulator { .. } => "Map Modulator",
            Self::UnmapModulator { .. } => "Unmap Modulator",
            Self::SetShape { .. } => "Edit Shape",
            // WP-6.47: Beatlayer's own names.
            Self::AddArticulation { .. } => "Add Articulation",
            Self::DeleteArticulation { .. } => "Delete Articulation",
            Self::MoveArticulation { .. } => "Move Articulation",
            Self::SetArticulation { .. } => "Change Articulation",
            Self::SetDeviceSample { media: Some(_), .. } => "Load Sample",
            Self::SetDeviceSample { media: None, .. } => "Clear Sample",
            // WP-5.15b: Warp controls and slices.
            Self::SetDeviceWarp { .. } => "Switch Warp",
            Self::SetDeviceWarpMode { .. } => "Change Warp Mode",
            Self::SetDeviceWarpSettings { .. } => "Change Warp Settings",
            Self::SetDeviceSampleBpm { .. } | Self::ScaleDeviceTempo { .. } => {
                "Change sample tempo"
            }
            Self::AddSlice { .. } => "Add Slice",
            Self::MoveSlice { .. } => "Move Slice",
            Self::DeleteSlice { .. } => "Delete Slice",
            Self::SetDeviceRegion { .. } => "Change Sample Region",
            // WP-5.13: Live's names for the layer list's items where it has
            // them (S6); the rest are Beatlayer's own.
            Self::InsertLayers { .. } => "Add Samples",
            Self::DeleteLayers { .. } => "Delete",
            Self::DuplicateLayers { .. } => "Duplicate",
            Self::RenameLayer { .. } => "Rename",
            Self::MoveLayers { .. } => "Move Samples",
            Self::SortLayers { .. } => "Sort Samples",
            Self::SetLayerZones { .. } => "Change Zones",
            Self::UpdateLayers { .. } => "Change Sample Settings",
            Self::SetLayerMedia { .. } => "Replace Sample",
            Self::AddSampleEdit { edit, .. } => edit.kind.name(),
            Self::ClearSampleEdits { .. } => "Revert Edits",
            Self::SetProcessedLayerSample { crop: Some(_), .. } => "Crop",
            Self::SetProcessedLayerSample { crop: None, .. } => "Reverse",
            Self::DistributeLayers {
                how: Distribution::Equally,
                ..
            } => "Distribute Ranges Equally",
            Self::DistributeLayers {
                how: Distribution::AroundRootKey,
                ..
            } => "Distribute Ranges Around Root Key",
            Self::SetLayerOptions { .. } => "Change Sample Options",
            // WP-2.19b, as Live 12.4.6 names them (observed); Listen's is to
            // observe.
            Self::SetSidechainInput { .. } | Self::SetMidiSidechainSource { .. } => {
                "Change \"Routing\""
            }
            Self::SetSidechainEnabled { .. } => "Change \"Sidechain On\"",
            Self::SetSidechainGain { .. } => "Change \"SC Gain\"",
            Self::SetSidechainMix { .. } => "Change \"Mix\"",
            Self::SetSidechainListen { .. } => "Change \"Sidechain Listen\"",
            // WP-5.34: a routing change is named as Live names one; Hardware
            // Latency's is to observe.
            Self::SetExternal {
                midi_to: None,
                channel: None,
                mpe: None,
                audio_from: None,
                cv_outputs: None,
                cv_inputs: None,
                ..
            } => "Change \"Hardware Latency\"",
            Self::SetExternal { .. } => "Change \"Routing\"",
            // WP-7.17: Beatlayer's own names.
            Self::SetCvCalibration {
                calibration: Some(_),
                ..
            } => "Calibrate CV Instrument",
            Self::SetCvCalibration { .. } => "Clear CV Calibration",
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
            Self::SetChainZone { .. } => "Change Chain Zone",
            Self::SetChainReceive { .. } => "Change Receive",
            Self::SetChainPlay { .. } => "Change Play",
            Self::SetChainChoke { .. } => "Change Choke",
            Self::SetChainSend { .. } => "Change chain send",
            Self::SetChainAudioTo { .. } => "Change Audio To",
            // WP-5.4: Beatlayer's own names; the law and the list are ours.
            Self::MapMacro { .. } => "Map to Macro",
            Self::UnmapMacro { .. } => "Unmap Macro",
            Self::SetMappingRange { .. } => "Change Macro Mapping",
            Self::InvertMapping { .. } => "Invert Macro Mapping",
            Self::RenameMacro { .. } => "Rename Macro",
            Self::SetMacroColour { .. } => "Change Macro Color",
            Self::SetMacroInfo { .. } => "Change Macro Info Text",
            Self::RandomizeMacros { .. } => "Randomize Macros",
            Self::SetMacroRandomizeExcluded { .. } => "Exclude Macro from Randomization",
            Self::SetMacroStyle { .. } => "Change Macro Style",
            Self::StoreVariation { .. } => "Store Macro Variation",
            Self::LaunchVariation { .. } => "Launch Macro Variation",
            Self::OverwriteVariation { .. } => "Overwrite Macro Variation",
            Self::RenameVariation { .. } => "Rename Macro Variation",
            Self::DuplicateVariation { .. } => "Duplicate Macro Variation",
            Self::DeleteVariation { .. } => "Delete Macro Variation",
            Self::SetMacroVariationExcluded { .. } => "Exclude Macro from Variations",
            Self::SetMappedValue { .. } => "Change parameter",
            // Spec: scales R5, R12. Live's undo names for the scale edits
            // are to observe; Beatlayer's name the edit.
            Self::SetClipScale { .. } => "Change Clip Scale",
            Self::SetScale { .. } => "Change Scale",
            // Spec: tunings R2, R7, R8, R9, R14. Live's names for these
            // edits are to observe; `Load Tuning` is Beatlayer's own.
            Self::SetTuning { .. } => "Load Tuning",
            Self::RemoveTuning { .. } => "Remove Tuning",
            Self::SetTuningReference { .. } => "Change Reference Pitch",
            Self::SetTuningRange { .. } => "Change Tuning Range",
            Self::SetTuningBypass { .. } => "Bypass Tuning",
            // Spec: tunings R22. Beatlayer's own name.
            Self::SetTuningLayout { .. } => "Change MIDI Controller Layout",
            Self::SetTrackLinks { group: None, .. } => "Unlink Tracks",
            Self::SetTrackLinks { .. } => "Link Tracks",
            // Spec: grooves R5, R16, R17, R32. Live's undo names for the
            // groove edits are to observe; Beatlayer's name the edit.
            Self::AddGroove { .. } => "Add Groove",
            Self::DeleteGroove { .. } => "Delete Groove",
            Self::RenameGroove { .. } => "Rename Groove",
            Self::SetGroove { .. } => "Change Groove",
            Self::SetGlobalGrooveAmount { .. } => "Change Global Amount",
            Self::SetAutoLoadGroove { .. } => "Change Auto Load Groove",
            Self::SetClipGroove { .. } => "Change Clip Groove",
        }
    }

    /// Whether running the command outside a transaction is an undo step.
    /// Cue mode is not: Live's Solo/Cue button and Options items add none.
    #[must_use]
    pub fn records_undo(&self) -> bool {
        !matches!(self, Self::SetCueMode { .. })
    }

    /// The undo step's name for this command run on `before`: the same as
    /// [`label`](Self::label), but a Compare command says which way it went
    /// (device compare R3, R4).
    #[must_use]
    pub fn label_in(&self, before: &Document) -> &'static str {
        let b = |device: &DeviceId| before.device(*device).is_some_and(|d| d.b_active);
        match self {
            Self::CompareSwitch { device } if b(device) => "Compare: Switch to A",
            Self::CompareSwitch { .. } => "Compare: Switch to B",
            Self::CompareCopy { device } if b(device) => "Compare: Copy B to A",
            Self::CompareCopy { .. } => "Compare: Copy A to B",
            // Spec: grooves R5. A change to one parameter is named for it.
            Self::SetGroove {
                base,
                quantize,
                timing,
                random,
                velocity,
                ..
            } => match (
                base.is_some(),
                quantize.is_some(),
                timing.is_some(),
                random.is_some(),
                velocity.is_some(),
            ) {
                (true, false, false, false, false) => "Change Base",
                (false, true, false, false, false) => "Change Quantize",
                (false, false, true, false, false) => "Change Timing",
                (false, false, false, true, false) => "Change Random",
                (false, false, false, false, true) => "Change Velocity",
                _ => self.label(),
            },
            _ => self.label(),
        }
    }

    /// Fills the IDs a command leaves out with ones from `issue`: a take
    /// lane and clips handed to `AddTakeLane` without them.
    pub fn issue_missing_ids(&mut self, doc: &Document, issue: &mut dyn FnMut() -> Id) {
        match self {
            Self::InsertTime { at, clips, .. } if clips.is_empty() => {
                *clips = (doc.clips_split_by_insert(*at).iter())
                    .map(|_| issue().into())
                    .collect();
            }
            Self::DeleteTime {
                start, end, clips, ..
            } if clips.is_empty() => {
                *clips = (doc.clips_split_by_delete(*start, *end).iter())
                    .map(|_| issue().into())
                    .collect();
            }
            _ => {}
        }
        if let Self::AddTakeLane { lane, .. } = self {
            if lane.id.is_none() {
                lane.id = Some(issue().into());
            }
            for clip in &mut lane.clips {
                if clip.id.is_none() {
                    clip.id = Some(issue().into());
                }
            }
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
            Self::CreateTakeClip { clip, .. } => vec![clip.id.0],
            Self::AddTakeLane { lane, .. } => lane
                .id
                .map(|id| id.0)
                .into_iter()
                .chain(lane.clips.iter().filter_map(|clip| clip.id.map(|id| id.0)))
                .collect(),
            Self::InsertDevice { device, .. } | Self::InsertChainDevice { device, .. } => {
                device.ids()
            }
            Self::AddChain { chain, .. } => chain.ids(),
            Self::GroupDevices { rack, .. } => rack.ids(),
            Self::CreateScene { id, .. } => vec![id.0],
            Self::AddLocator { locator } => vec![locator.id.0],
            Self::InsertTime { clips, .. } | Self::DeleteTime { clips, .. } => {
                clips.iter().map(|clip| clip.0).collect()
            }
            Self::AddGroove { groove } => vec![groove.id.0],
            Self::BounceTrackInPlace { bounced, .. } => {
                bounced.clip_ids().iter().map(|clip| clip.0).collect()
            }
            Self::BounceToNewTrack { track, bounced, .. } => {
                let mut ids = vec![track.0];
                ids.extend(bounced.clip_ids().iter().map(|clip| clip.0));
                ids
            }
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
        // Spec: macros R18. A device or chain that left its rack takes its
        // mappings with it, in the same undo step.
        next.prune_macro_mappings();
        *self = next;
        Ok(())
    }

    fn apply_in_place(&mut self, command: &Command) -> Result<(), ModelError> {
        match command {
            Command::SetTempo { tempo } => {
                // Spec: warping R45. While any clip leads, the set's tempo is
                // the clip's: the command is refused.
                if self.any_tempo_leader() {
                    return Err(ModelError::InvalidInput(
                        "a clip leads the set's tempo: make it a follower, or use Unfollow Tempo \
                         Automation, first",
                    ));
                }
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
            Command::SetTempoBreakpoint { at, bpm } => {
                self.check_outside_tempo_leaders(&[*at])?;
                self.set_tempo_breakpoint(*at, *bpm)
            }
            Command::MoveTempoBreakpoint { at, to } => {
                self.check_outside_tempo_leaders(&[*at, *to])?;
                self.move_tempo_breakpoint(*at, *to)
            }
            Command::DeleteTempoBreakpoint { at } => {
                self.check_outside_tempo_leaders(&[*at])?;
                self.delete_tempo_breakpoint(*at)
            }
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
            Command::InsertTime { at, length, clips } => self.insert_time(*at, *length, clips),
            Command::DeleteTime { start, end, clips } => self.delete_time(*start, *end, clips),
            Command::AddLocator { locator } => self.add_locator(locator.clone()),
            Command::DeleteLocator { locator } => self.delete_locator(*locator),
            Command::MoveLocator { locator, to } => self.move_locator(*locator, *to),
            Command::RenameLocator { locator, name } => self.rename_locator(*locator, name.clone()),
            Command::SetLocatorInfoText { locator, info_text } => {
                self.set_locator_info_text(*locator, info_text.clone())
            }
            Command::LoopToNextLocator { locator } => self.loop_to_next_locator(*locator),
            Command::SetSongStart { start } => {
                self.settings.song_start = *start;
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
            Command::SetRecordQuantization { quantization, on } => {
                let settings = &mut self.settings;
                let value = match (quantization, on) {
                    (Some(value), None) => *value,
                    (None, Some(true)) if settings.record_quantization.is_none() => {
                        settings.record_quantization_last
                    }
                    (None, Some(true)) => settings.record_quantization,
                    (None, Some(false)) => RecordQuantization::None,
                    _ => {
                        return Err(ModelError::InvalidInput(
                            "give either `quantization` or `on`",
                        ));
                    }
                };
                settings.record_quantization = value;
                if !value.is_none() {
                    settings.record_quantization_last = value;
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
            Command::DeleteTrack { track } => {
                self.delete_track(*track)?;
                self.end_single_links();
                self.prune_mappings();
                Ok(())
            }
            Command::MoveTrack {
                track,
                parent,
                index,
            } => self.move_track(*track, *parent, *index),
            Command::SetTrackInfoText { track, info_text } => {
                if self.track_ref(*track)?.info_text != *info_text {
                    self.track_mut(*track)?.info_text.clone_from(info_text);
                }
                Ok(())
            }
            Command::RenameTrack { track, name } => {
                if self.track_ref(*track)?.name != *name {
                    self.track_mut(*track)?.name.clone_from(name);
                }
                Ok(())
            }
            Command::SetTrackColour { tracks, colour } => {
                crate::colour::check_swatch("track colour", *colour)?;
                for track in each_once(tracks, |t| t.0)? {
                    self.track_ref(track)?;
                    self.colour_track(track, *colour)?;
                }
                Ok(())
            }
            Command::AssignColourToClips { tracks } => {
                for track in each_once(tracks, |t| t.0)? {
                    let colour = self.track_ref(track)?.colour;
                    self.colour_clips_of(track, colour)?;
                }
                Ok(())
            }
            Command::AssignColourToGrouped { tracks } => {
                for group in each_once(tracks, |t| t.0)? {
                    let group_track = self.track_ref(group)?;
                    check_kind(group_track, |k| k == TrackKind::Group, "a group track")?;
                    let colour = group_track.colour;
                    let mut inside: Vec<TrackId> = group_track.children.iter().copied().collect();
                    while let Some(track) = inside.pop() {
                        inside.extend(self.track_ref(track)?.children.iter().copied());
                        self.colour_track(track, colour)?;
                        self.colour_clips_of(track, colour)?;
                    }
                }
                Ok(())
            }
            Command::FreezeTracks { frozen } => {
                for (track, audio) in frozen {
                    let held = self.track_ref(*track)?;
                    if held.kind == TrackKind::Main {
                        return Err(ModelError::MainTrack);
                    }
                    // A second freeze would replace the stored originals.
                    if held.is_frozen() {
                        return Err(ModelError::InvalidInput("the track is already frozen"));
                    }
                    // The clips play the freeze files: their own data is
                    // kept in the frozen record and restored on unfreezing
                    // (R6, R10).
                    let mut played: Vec<(crate::ids::ClipId, Option<SceneId>, crate::clip::Clip)> =
                        Vec::new();
                    for region in &audio.clips {
                        let Some(clip) = (match region.scene {
                            None => held.clips.get(&region.clip),
                            Some(scene) => held.slots.get(&scene),
                        }) else {
                            continue;
                        };
                        let mut clip = (**clip).clone();
                        clip.data = audio.audio_for(region);
                        played.push((region.clip, region.scene, clip));
                    }
                    let held = self.track_mut(*track)?;
                    held.frozen = Some(audio.clone());
                    for (clip, scene, played) in played {
                        match scene {
                            None => {
                                held.clips.insert(clip, Shared::new(played));
                            }
                            Some(scene) => {
                                held.slots.insert(scene, Shared::new(played));
                            }
                        }
                    }
                }
                Ok(())
            }
            Command::UnfreezeTracks { tracks } => {
                for track in tracks {
                    let Some(frozen) = self.track_ref(*track)?.frozen.clone() else {
                        continue;
                    };
                    // Each clip gets back the data of the region it plays: by
                    // its ID, or, for a copy made while frozen, by the
                    // stretch of the freeze file it starts at (R10).
                    let original = |clip: &crate::clip::Clip| {
                        frozen
                            .clip(clip.id)
                            .or_else(|| {
                                let ClipData::Audio(audio) = &clip.data else {
                                    return None;
                                };
                                let first = audio.warp_markers.first()?.time;
                                frozen
                                    .clips
                                    .iter()
                                    .find(|r| r.media == audio.media && r.start == first)
                            })
                            .map(|region| region.original.clone())
                    };
                    let back = |clip: &Shared<crate::clip::Clip>| {
                        original(clip).map(|data| {
                            let mut back = (**clip).clone();
                            back.data = data;
                            Shared::new(back)
                        })
                    };
                    let held = self.track_mut(*track)?;
                    let lanes: Vec<_> = held
                        .clips
                        .iter()
                        .filter_map(|(id, clip)| Some((*id, back(clip)?)))
                        .collect();
                    let slots: Vec<_> = held
                        .slots
                        .iter()
                        .filter_map(|(scene, clip)| Some((*scene, back(clip)?)))
                        .collect();
                    held.clips.extend(lanes);
                    held.slots.extend(slots);
                    held.frozen = None;
                }
                Ok(())
            }
            Command::BounceTrackInPlace { track, bounced } => {
                // A group's tracks go with it (R23).
                let children: Vec<TrackId> =
                    self.track_ref(*track)?.children.iter().copied().collect();
                for child in children {
                    self.delete_track(child)?;
                }
                self.replace_with_bounced(*track, bounced.clone())?;
                self.end_single_links();
                self.prune_mappings();
                Ok(())
            }
            Command::BounceToNewTrack {
                source,
                track,
                bounced,
            } => {
                // The new track goes directly after its source, in the same
                // group (R22).
                let parent = self.parent(*source);
                let kind = self.track_ref(*source)?.kind;
                if kind == TrackKind::Main || kind == TrackKind::Return {
                    return Err(ModelError::MainTrack);
                }
                let list = self.containing_list(kind, parent)?;
                let position = position_in(list, |id| *id == *source)
                    .ok_or(ModelError::NoSuchTrack(*source))?;
                self.create_track(
                    *track,
                    TrackKind::Audio,
                    &bounced.name,
                    parent,
                    position + 1,
                )?;
                self.fill_bounced(*track, bounced.clone())
            }
            Command::SetVolume { track, volume } => {
                self.check_not_held(crate::modulator::ModTarget::Mixer {
                    track: *track,
                    param: MixerParam::Volume,
                })?;
                check_value("volume", *volume, volume.is_finite() && *volume >= 0.0)?;
                if self.track_ref(*track)?.mixer.volume != *volume {
                    let track_ref = self.track_mut(*track)?;
                    track_ref.mixer.volume = *volume;
                    track_ref.override_target(&AutomationTarget::Mixer {
                        param: MixerParam::Volume,
                    });
                }
                Ok(())
            }
            Command::SetPan { track, pan } => {
                self.check_not_held(crate::modulator::ModTarget::Mixer {
                    track: *track,
                    param: MixerParam::Pan,
                })?;
                check_value("pan", *pan, (-1.0..=1.0).contains(pan))?;
                if self.track_ref(*track)?.mixer.pan != *pan {
                    let track_ref = self.track_mut(*track)?;
                    track_ref.mixer.pan = *pan;
                    track_ref.override_target(&AutomationTarget::Mixer {
                        param: MixerParam::Pan,
                    });
                }
                Ok(())
            }
            Command::SetMute { track, mute } => {
                self.check_not_held(crate::modulator::ModTarget::Mixer {
                    track: *track,
                    param: MixerParam::Mute,
                })?;
                let current = self.track_ref(*track)?;
                check_kind(current, TrackKind::has_mute_and_solo, NOT_MAIN)?;
                if current.mixer.mute != *mute {
                    let track_ref = self.track_mut(*track)?;
                    track_ref.mixer.mute = *mute;
                    track_ref.override_target(&AutomationTarget::Mixer {
                        param: MixerParam::Mute,
                    });
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
                self.check_not_held(crate::modulator::ModTarget::Send {
                    track: *track,
                    to: *to,
                })?;
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
                    self.track_mut(*track)?
                        .override_target(&AutomationTarget::Send { to: *to });
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
            Command::ClearTrackAutomation { track } => {
                self.track_mut(*track)?.automation.clear();
                Ok(())
            }
            Command::ClearDeviceAutomation { track, device } => {
                let owner = self.track_mut(*track)?;
                if let Some(device) = device
                    && owner.find_device(*device).is_none()
                {
                    return Err(ModelError::DeviceNotInTrack {
                        device: *device,
                        track: *track,
                    });
                }
                // Spec: automation R28. A device's envelopes name it; the
                // mixer's are the track's own controls, its sends and its
                // rack chains' controls.
                let cleared: Vec<AutomationTarget> = owner
                    .automation
                    .keys()
                    .copied()
                    .filter(|target| match (target, device) {
                        (AutomationTarget::Device { device: of, .. }, Some(device)) => of == device,
                        (AutomationTarget::Device { .. }, None) | (_, Some(_)) => false,
                        (_, None) => true,
                    })
                    .collect();
                for target in cleared {
                    owner.automation.remove(&target);
                }
                Ok(())
            }
            Command::SetAutomationEnvelope {
                track,
                target,
                points,
            } => {
                self.check_automation_target(*track, *target)?;
                let envelope = Envelope::from_points(points.clone())?;
                if envelope.is_empty() {
                    self.track_mut(*track)?.automation.remove(target);
                    return Ok(());
                }
                let automation = entry(&mut self.track_mut(*track)?.automation, *target);
                automation.envelope = envelope;
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
                    track.set_clip_envelopes_enabled(|_| true, true);
                }
                Ok(())
            }
            Command::ReEnableAutomationFor { track, target } => {
                self.check_automation_target(*track, *target)?;
                let track = self.track_mut(*track)?;
                if let Some(automation) = track.automation.get_mut(target) {
                    automation.enabled = true;
                }
                track.set_clip_envelopes_enabled(|t| t == target, true);
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
            Command::SetLockEnvelopes { enabled } => {
                self.settings.lock_envelopes = *enabled;
                Ok(())
            }
            Command::SetRemoteMappings { mappings } => {
                crate::remote::check_all(mappings)?;
                if mappings.iter().any(|m| !self.has_remote_target(&m.target)) {
                    return Err(ModelError::InvalidInput(
                        "a mapped control isn't in the set",
                    ));
                }
                self.settings.remote.clone_from(mappings);
                Ok(())
            }
            Command::SetCueMode { enabled } => {
                if *enabled && !self.cue_available() {
                    return Err(ModelError::CueOverlapsMain);
                }
                self.settings.cue_mode = *enabled;
                Ok(())
            }
            Command::SetCueVolume { volume } => {
                check_value(
                    "cue volume",
                    *volume,
                    (0.0..=crate::MAX_CUE_VOLUME).contains(volume),
                )?;
                self.settings.cue_volume = *volume;
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
            Command::SetTrackDelay { track, delay } => {
                // Spec: delay-compensation R7. While compensation is off the
                // Track Delay is unavailable: the command is refused and the
                // value stays as it was.
                if !self.settings.delay_compensation {
                    return Err(ModelError::TrackDelayNeedsCompensation);
                }
                check_value(
                    "track delay",
                    match *delay {
                        TrackDelay::Milliseconds { value } => value,
                        TrackDelay::Samples { value } => value as f64,
                    },
                    delay.is_valid(),
                )?;
                if self.track_ref(*track)?.delay != *delay {
                    self.track_mut(*track)?.delay = *delay;
                }
                Ok(())
            }
            Command::SetPanMode { track, mode } => {
                if self.track_ref(*track)?.mixer.pan_mode != *mode {
                    self.track_mut(*track)?.mixer.pan_mode = *mode;
                }
                Ok(())
            }
            Command::SetSplitPanLeft { track, pan } => {
                self.check_not_held(crate::modulator::ModTarget::Mixer {
                    track: *track,
                    param: MixerParam::SplitLeft,
                })?;
                check_value("split pan", *pan, (-1.0..=1.0).contains(pan))?;
                if self.track_ref(*track)?.mixer.split_left != *pan {
                    let track_ref = self.track_mut(*track)?;
                    track_ref.mixer.split_left = *pan;
                    track_ref.override_target(&AutomationTarget::Mixer {
                        param: MixerParam::SplitLeft,
                    });
                }
                Ok(())
            }
            Command::SetSplitPanRight { track, pan } => {
                self.check_not_held(crate::modulator::ModTarget::Mixer {
                    track: *track,
                    param: MixerParam::SplitRight,
                })?;
                check_value("split pan", *pan, (-1.0..=1.0).contains(pan))?;
                if self.track_ref(*track)?.mixer.split_right != *pan {
                    let track_ref = self.track_mut(*track)?;
                    track_ref.mixer.split_right = *pan;
                    track_ref.override_target(&AutomationTarget::Mixer {
                        param: MixerParam::SplitRight,
                    });
                }
                Ok(())
            }
            Command::SetDelayCompensation { enabled } => {
                self.settings.delay_compensation = *enabled;
                Ok(())
            }
            Command::SetReducedLatencyMonitoring { enabled } => {
                // Spec: delay-compensation R8. It can be on only while
                // compensation is on; turning it off is always allowed.
                if *enabled && !self.settings.delay_compensation {
                    return Err(ModelError::ReducedLatencyNeedsCompensation);
                }
                self.settings.reduced_latency_monitoring = *enabled;
                Ok(())
            }
            Command::SetKeepMonitoringLatency { track, enabled } => {
                if self.track_ref(*track)?.keep_monitoring_latency != *enabled {
                    self.track_mut(*track)?.keep_monitoring_latency = *enabled;
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
                self.edit_routing(*track, |routing| routing.midi_output = to.clone())
            }
            Command::SetMainOut { channel } => {
                self.set_main_channel(*channel, |routing| &mut routing.main_out)
            }
            Command::SetCueOut { channel } => {
                self.set_main_channel(*channel, |routing| &mut routing.cue_out)
            }
            Command::CreateClip { track, clip } => self.create_clip(*track, clip),
            Command::AddTakeLane { track, lane, index } => self.add_take_lane(*track, lane, *index),
            Command::CreateTakeClip { lane, clip } => self.create_take_clip(*lane, clip),
            Command::RenameTakeLane { lane, name } => self.rename_take_lane(*lane, name),
            Command::DeleteTakeLanes { lanes } => self.remove_take_lanes(lanes),
            Command::DeleteAllTakeLanes { tracks } => self.delete_all_take_lanes(tracks),
            Command::DeleteUnusedTakeLanes { tracks } => self.delete_unused_take_lanes(tracks),
            Command::MoveTakeLanes { lanes, index } => self.move_take_lanes(lanes, *index),
            Command::SetAudition { lane, on } => self.set_audition(*lane, *on),
            Command::DeleteClip { clip } => self.remove_clip(*clip),
            Command::MoveClip { clip, track, start } => self.move_clip(*clip, *track, *start),
            Command::MoveClipToLane {
                clip,
                track,
                lane,
                start,
            } => self.move_clip_to_lane(*clip, *track, *lane, *start),
            Command::MoveClips { clips, by } => self.move_clips(clips, *by),
            Command::SetClipColour { clips, colour } => {
                crate::colour::check_swatch("clip colour", *colour)?;
                for clip in each_once(clips, |c| c.0)? {
                    if self.clip_ref(clip)?.colour != Some(*colour) {
                        self.clip_mut(clip)?.colour = Some(*colour);
                    }
                }
                Ok(())
            }
            Command::SetClipLength { clip, length } => {
                let current = self.clip_ref(*clip)?;
                check_extent(current.start, *length)?;
                if current.length != *length {
                    self.clip_mut(*clip)?.length = *length;
                }
                Ok(())
            }
            Command::SetClipInfoText { clip, info_text } => {
                if self.clip_ref(*clip)?.info_text != *info_text {
                    self.clip_mut(*clip)?.info_text.clone_from(info_text);
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
            // Spec: scales R5. A scale name the list doesn't hold, or a
            // root outside C to B, is refused, and nothing changes.
            Command::SetClipScale {
                clips,
                enabled,
                root,
                name,
            } => {
                let mut scales = Vec::with_capacity(clips.len());
                for clip in clips {
                    let current = self.clip_ref(*clip)?.scale;
                    scales.push((*clip, scaled(current, *enabled, *root, name.as_deref())?));
                }
                for (clip, next) in scales {
                    if self.clip_ref(clip)?.scale != next {
                        self.clip_mut(clip)?.scale = next;
                    }
                }
                Ok(())
            }
            // Spec: scales R9, R12. The set's current scale, which the
            // Control Bar's scale controls show and a new clip takes.
            Command::SetScale {
                enabled,
                root,
                name,
            } => {
                let current = self.settings.scale;
                self.settings.scale = scaled(current, *enabled, *root, name.as_deref())?;
                Ok(())
            }
            Command::AddGroove { groove } => self.add_groove(groove.clone()),
            Command::DeleteGroove { groove } => self.delete_groove(*groove),
            Command::RenameGroove { groove, name } => {
                self.groove_mut(*groove)?.name.clone_from(name);
                Ok(())
            }
            Command::SetGroove {
                groove,
                base,
                quantize,
                timing,
                random,
                velocity,
            } => self.set_groove(*groove, *base, [*quantize, *timing, *random, *velocity]),
            Command::SetGlobalGrooveAmount { amount } => {
                check_percent("the Global Amount", *amount, 0.0, GLOBAL_AMOUNT_MAX)?;
                self.settings.grooves.global_amount = *amount;
                Ok(())
            }
            Command::SetAutoLoadGroove { enabled, groove } => {
                if let Some(groove) = groove {
                    self.groove_mut(*groove)?;
                }
                let pool = &mut self.settings.grooves;
                if let Some(enabled) = enabled {
                    pool.auto_load = *enabled;
                }
                if groove.is_some() {
                    pool.auto_load_groove = *groove;
                }
                Ok(())
            }
            Command::SetClipGroove { clips, groove } => {
                if let Some(groove) = groove {
                    self.groove_mut(*groove)?;
                }
                for clip in clips {
                    self.clip_ref(*clip)?;
                }
                for clip in clips {
                    if self.clip_ref(*clip)?.groove != *groove {
                        self.clip_mut(*clip)?.groove = *groove;
                    }
                }
                Ok(())
            }
            // Spec: tunings R2, R9, R15. A tuning that breaks R3's limits is
            // refused and the set's stays as it was.
            Command::SetTuning { tuning, retune } => {
                tuning.check()?;
                let before = self.settings.tuning.replace(tuning.clone());
                if *retune {
                    self.retune_notes(before.as_ref());
                }
                Ok(())
            }
            Command::RemoveTuning { retune } => {
                let before = self
                    .settings
                    .tuning
                    .take()
                    .ok_or(ModelError::InvalidInput("the set has no tuning to remove"))?;
                if *retune {
                    self.retune_notes(Some(&before));
                }
                Ok(())
            }
            Command::SetTuningReference {
                octave,
                degree,
                frequency,
            } => {
                let tuning = self.held_tuning()?;
                self.settings.tuning = Some(tuning.with_reference(*octave, *degree, *frequency)?);
                Ok(())
            }
            Command::SetTuningRange { lowest, highest } => {
                let tuning = self.held_tuning()?;
                self.settings.tuning = Some(tuning.with_range(*lowest, *highest)?);
                Ok(())
            }
            Command::SetTuningBypass { track, bypass } => {
                if self.track_ref(*track)?.kind() != TrackKind::Midi {
                    return Err(ModelError::InvalidInput(
                        "only a MIDI track has Bypass Tuning",
                    ));
                }
                if self.track_ref(*track)?.tuning_bypass != *bypass {
                    self.track_mut(*track)?.tuning_bypass = *bypass;
                }
                Ok(())
            }
            Command::SetTuningLayout {
                track,
                layout,
                keys,
            } => {
                if self.track_ref(*track)?.kind() != TrackKind::Midi {
                    return Err(ModelError::InvalidInput(
                        "only a MIDI track has a MIDI Controller Layout",
                    ));
                }
                if let Some(keys) = keys {
                    crate::TuningLayout::check_keys(keys)?;
                }
                let mut next = self.track_ref(*track)?.tuning_layout.clone();
                if let Some(layout) = layout {
                    next.layout = *layout;
                }
                if let Some(keys) = keys {
                    // A table where every key plays itself is the new one.
                    next.keys = if keys
                        .iter()
                        .enumerate()
                        .all(|(at, &to)| to == Some(at as u8))
                    {
                        Vec::new()
                    } else {
                        keys.clone()
                    };
                }
                if next != self.track_ref(*track)?.tuning_layout {
                    self.track_mut(*track)?.tuning_layout = next;
                }
                Ok(())
            }
            Command::SetTrackLinks { tracks, group } => {
                for track in tracks {
                    if !matches!(
                        self.track_ref(*track)?.kind(),
                        TrackKind::Audio | TrackKind::Midi
                    ) {
                        return Err(ModelError::InvalidInput(
                            "only audio and MIDI tracks can be linked",
                        ));
                    }
                }
                for track in tracks {
                    self.track_mut(*track)?.link = *group;
                }
                self.end_single_links();
                Ok(())
            }
            Command::SetClipLaunchMode { clip, mode } => {
                self.edit_launch(*clip, |launch| launch.mode = *mode)
            }
            Command::ReverseClip {
                clip,
                media,
                seconds,
            } => {
                let current = self.clip_ref(*clip)?.clone();
                let ClipData::Audio(audio) = &current.data else {
                    return Err(ModelError::NotAudioClip(*clip));
                };
                if !seconds.is_finite() || *seconds <= 0.0 {
                    return Err(ModelError::InvalidValue {
                        what: "the file's length",
                        value: *seconds,
                    });
                }
                let mut data = audio.reversed(*seconds, *media);
                // The transient edits belong to the file they were made on
                // (the warping spec, R27).
                if data.media != audio.media {
                    data.transient_edits = TransientEdits::default();
                }
                let mut next = current;
                next.data = ClipData::Audio(data);
                check_clip_data(&next.data)?;
                check_clip_fits(self.track_ref(self.clip_owner(*clip)?)?, &next.data)?;
                *self.clip_mut(*clip)? = next;
                Ok(())
            }
            Command::SetClipData { clip, data } => {
                let owner = self.clip_owner(*clip)?;
                check_clip_fits(self.track_ref(owner)?, data)?;
                check_clip_data(data)?;
                let mut data = data.clone();
                // Transient edits belong to the file they were made on
                // (the warping spec, R27).
                if let (ClipData::Audio(next), ClipData::Audio(now)) =
                    (&mut data, &self.clip_ref(*clip)?.data)
                    && next.media != now.media
                {
                    next.transient_edits = TransientEdits::default();
                }
                if self.clip_ref(*clip)?.data != data {
                    self.clip_mut(*clip)?.data = data;
                }
                Ok(())
            }
            // Spec: audio-clip-editing R14, R15, R17.
            Command::CropClip {
                clip,
                media,
                from,
                to,
            } => self.crop_clip(*clip, *media, *from, *to, None),
            // Spec: audio-clip-editing R16.
            Command::CropClipToRange {
                clip,
                media,
                start,
                end,
            } => self.crop_clip_to_range(*clip, *media, *start, *end),
            Command::TrimClipStart { clip, start } => self.trim_clip_start(*clip, *start),
            Command::SetClipRegion {
                clip,
                start,
                end,
                ticks,
            } => {
                self.check_form(*clip, *ticks)?;
                self.edit_markers(*clip, |audio| {
                    audio.start = *start;
                    audio.end = *end;
                })
            }
            Command::SetClipLoop {
                clip,
                start,
                end,
                ticks,
            } => {
                self.check_form(*clip, *ticks)?;
                self.edit_markers(*clip, |audio| {
                    audio.loop_start = *start;
                    audio.loop_end = *end;
                })
            }
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
            // Spec: clip-properties R10. An unwarped audio clip's positions
            // read in time, so it has no signature to set.
            Command::SetClipSignature { clip, signature } => match &self.clip_ref(*clip)?.data {
                ClipData::Audio(audio) if !audio.warp => Err(ModelError::InvalidInput(
                    "only a warped audio clip has a time signature: switch Warp on first",
                )),
                ClipData::Audio(_) => self.edit_audio(*clip, |audio| audio.signature = *signature),
                ClipData::Midi(_) => self.edit_midi(*clip, |midi| midi.signature = *signature),
            },
            Command::SetClipLoopEnabled { clip, enabled } => match &self.clip_ref(*clip)?.data {
                // Spec: warping R14. Only a warped audio clip can switch its
                // loop on; an older set's looped unwarped clip can switch it
                // off.
                ClipData::Audio(audio) if *enabled && !audio.warp => Err(ModelError::InvalidInput(
                    "only a warped audio clip can loop: switch Warp on first",
                )),
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
                fade_in_curve,
                fade_out_curve,
            } => self.set_clip_fades(*clip, *fade_in, *fade_out, *fade_in_curve, *fade_out_curve),
            Command::SetCrossfade {
                track,
                boundary,
                before,
                after,
                curve,
            } => {
                if *before <= 0.0 && *after <= 0.0 {
                    return Err(ModelError::InvalidInput(
                        "a crossfade must reach before or after the boundary; use \
                         `arrangement.delete_crossfade` to end one",
                    ));
                }
                self.set_crossfade(*track, *boundary, *before, *after, *curve)
            }
            Command::DeleteCrossfade { track, boundary } => {
                let (earlier, later) = self.meeting_audio_clips(*track, *boundary)?;
                if !crossfades(self.track(*track).iter().flat_map(|t| t.clips()))
                    .iter()
                    .any(|c| c.earlier == earlier && c.later == later)
                {
                    return Err(ModelError::InvalidInput(
                        "there is no crossfade where those clips meet",
                    ));
                }
                self.set_crossfade(*track, *boundary, 0.0, 0.0, 0.0)
            }
            Command::SetClipWarp { clip, warp } => {
                let at = self.clip_ref(*clip)?.start;
                let set_tempo = self.settings.tempo.bpm_at(at);
                self.edit_markers_with(*clip, |audio| audio.with_warp(*warp, set_tempo))
            }
            Command::InsertWarpMarker {
                clips,
                beat,
                time,
                pin_neighbours,
                transients,
            } => {
                let at = match (beat, time) {
                    (Some(beat), None) => MarkerAt::Beat(*beat),
                    (None, Some(time)) => MarkerAt::Time(*time),
                    _ => {
                        return Err(ModelError::InvalidInput(
                            "give the warp marker's `beat` or its `time`, not both or neither",
                        ));
                    }
                };
                if !*pin_neighbours {
                    return self.edit_each(clips, |audio| audio.with_marker_inserted(at));
                }
                if clips.len() != 1 {
                    return Err(ModelError::InvalidInput(
                        "pin_neighbours works on one clip, whose transients it reads",
                    ));
                }
                let transients = transients.as_deref().unwrap_or_default();
                self.edit_each(clips, |audio| {
                    audio.with_marker_inserted_pinned(at, transients)
                })
            }
            Command::InsertWarpMarkers {
                clip,
                from,
                to,
                transients,
            } => {
                let transients = transients.as_deref().unwrap_or_default();
                self.edit_markers_with(*clip, |audio| {
                    audio.with_markers_in_range(*from, *to, transients)
                })
            }
            Command::InsertTransient { clip, time } => {
                self.edit_markers_with(*clip, |audio| audio.with_transient_inserted(*time))
            }
            Command::DeleteTransients { clip, times } => {
                self.edit_markers_with(*clip, |audio| audio.with_transients_deleted(times))
            }
            Command::ResetTransients { clip } => {
                self.edit_markers_with(*clip, |audio| Ok(audio.with_transients_reset()))
            }
            Command::MoveTransient { clip, from, to } => {
                self.edit_markers_with(*clip, |audio| audio.with_transient_moved(*from, *to))
            }
            Command::MoveWarpMarkers { clips, markers, by } => {
                self.edit_each(clips, |audio| audio.with_markers_moved(markers, *by))
            }
            Command::ShiftWarpMarker { clips, marker, by } => {
                self.edit_each(clips, |audio| audio.with_marker_shifted(*marker, *by))
            }
            Command::DeleteWarpMarkers { clips, markers } => {
                self.edit_each(clips, |audio| audio.with_markers_deleted(markers))
            }
            Command::SetClipWarpMode { clip, mode } => {
                self.edit_audio(*clip, |audio| audio.warp_mode = *mode)
            }
            Command::SetClipWarpSettings {
                clip,
                beats,
                tones,
                texture,
                complex_pro,
            } => {
                let ClipData::Audio(audio) = &self.clip_ref(*clip)?.data else {
                    return Err(ModelError::NotAudioClip(*clip));
                };
                let settings =
                    audio
                        .warp_settings
                        .updated(*beats, *tones, *texture, *complex_pro)?;
                self.edit_audio(*clip, |audio| audio.warp_settings = settings)
            }
            Command::SetClipRamMode { clip, ram } => {
                self.edit_audio(*clip, |audio| audio.ram = *ram)
            }
            Command::SetClipHighQuality { clip, high_quality } => {
                self.edit_audio(*clip, |audio| audio.high_quality = *high_quality)
            }
            Command::SetClipPitch {
                clip,
                transpose,
                detune,
            } => {
                let ClipData::Audio(audio) = &self.clip_ref(*clip)?.data else {
                    return Err(ModelError::NotAudioClip(*clip));
                };
                if transpose.is_none() && detune.is_none() {
                    return Err(ModelError::InvalidInput(
                        "give `transpose`, `detune`, or both",
                    ));
                }
                let transpose = transpose.unwrap_or(audio.transpose);
                let detune = detune.unwrap_or(audio.detune);
                if !(TRANSPOSE_MIN..=TRANSPOSE_MAX).contains(&transpose) {
                    return Err(ModelError::InvalidValue {
                        what: "Transpose",
                        value: f64::from(transpose),
                    });
                }
                if !detune.is_finite() || !(DETUNE_MIN..=DETUNE_MAX).contains(&detune) {
                    return Err(ModelError::InvalidValue {
                        what: "Detune",
                        value: detune,
                    });
                }
                self.edit_audio(*clip, |audio| {
                    audio.transpose = transpose;
                    audio.detune = detune;
                })
            }
            Command::SetSegmentBpm { clip, marker, bpm } => {
                self.edit_markers_with(*clip, |audio| audio.with_segment_bpm(*marker, *bpm))
            }
            Command::ScaleTempo { clip, factor } => {
                self.edit_markers_with(*clip, |audio| audio.with_tempo_scaled(*factor))
            }
            Command::SetTempoLeader { clip, leader } => self.set_tempo_leader(*clip, *leader),
            Command::UnfollowTempoAutomation {} => self.unfollow_tempo_automation(),
            Command::Stretch { clip, edge, at } => self.stretch_clip(*clip, *edge, *at),
            Command::SlideContents { clip, by } => self.slide_contents(*clip, *by),
            Command::SetOneOneOneHere { clip, at } => {
                self.edit_markers_with(*clip, |audio| audio.with_1_1_1_here(at))
            }
            Command::WarpFromHere {
                clip,
                at,
                method,
                bpm,
                analysis,
            } => {
                let set_bpm = self.set_bpm_at(*clip, Some(at))?;
                self.edit_markers_with(*clip, |audio| {
                    audio.with_warp_from_here(at, *method, *bpm, set_bpm, analysis.as_ref())
                })
            }
            Command::WarpAsLoop { clip, analysis } => {
                let seconds =
                    analysis
                        .as_ref()
                        .map(|file| file.seconds)
                        .ok_or(ModelError::InvalidInput(
                            "give the file's length in `analysis`",
                        ))?;
                let set_bpm = self.set_bpm_at(*clip, None)?;
                self.edit_markers_with(*clip, |audio| {
                    audio.require_warp()?;
                    let bars = nearest_bars(&LOOP_BARS, seconds, set_bpm);
                    audio.warped_as_loop(0.0, seconds, bars, true)
                })
            }
            Command::WarpSelectionAsLoop { clip, from, to } => {
                let set_bpm = self.set_bpm_at(*clip, None)?;
                self.edit_markers_with(*clip, |audio| {
                    audio.require_warp()?;
                    let bars = nearest_bars(&LOOP_BARS, to - from, set_bpm);
                    audio.warped_as_loop(*from, *to, bars, true)
                })
            }
            Command::SetClipLegato { clip, legato } => {
                self.edit_launch(*clip, |launch| launch.legato = *legato)
            }
            Command::SetClipLaunchQuantization { clip, quantization } => {
                self.edit_launch(*clip, |launch| launch.quantization = *quantization)
            }
            Command::SetClipFollowActions { clip, patch } => {
                let targets = self.scenes.len();
                let mut result = Ok(());
                self.edit_launch(*clip, |launch| result = launch.follow.apply(patch, targets))?;
                result
            }
            Command::SetClipVelocityAmount { clip, amount } => {
                check_velocity_amount(*amount)?;
                self.edit_launch(*clip, |launch| launch.velocity_amount = *amount)
            }
            Command::SetClipEnvelopeBreakpoint {
                clip,
                target,
                kind,
                at,
                value,
            } => self.set_clip_envelope_breakpoint(*clip, *target, *kind, *at, *value),
            Command::SetClipEnvelopeCurve {
                clip,
                target,
                kind,
                at,
                bend,
            } => self.set_clip_envelope_curve(*clip, *target, *kind, *at, *bend),
            Command::MoveClipEnvelopeBreakpoint {
                clip,
                target,
                kind,
                at,
                to,
                value,
            } => self.move_clip_envelope_breakpoint(*clip, *target, *kind, *at, *to, *value),
            Command::DeleteClipEnvelopeBreakpoint {
                clip,
                target,
                kind,
                at,
            } => self.delete_clip_envelope_breakpoint(*clip, *target, *kind, *at),
            Command::DeleteClipEnvelope { clip, target, kind } => {
                self.delete_clip_envelope(*clip, *target, *kind)
            }
            Command::SetClipEnvelopeLinked {
                clip,
                target,
                kind,
                linked,
            } => self.set_clip_envelope_linked(*clip, *target, *kind, *linked),
            Command::SetClipEnvelope {
                clip,
                target,
                kind,
                points,
            } => self.set_clip_envelope(*clip, *target, *kind, points),
            Command::CreateScene { id, index } => self.create_scene(*id, *index),
            Command::DeleteScene { scene } => {
                self.delete_scene(*scene)?;
                self.prune_mappings();
                Ok(())
            }
            Command::MoveScene { scene, index } => self.move_scene(*scene, *index),
            Command::DuplicateScene { scene, id, clips } => {
                self.duplicate_scene(*scene, *id, clips)
            }
            Command::RenameScene { scene, name } => self.rename_scene(*scene, name),
            Command::SetSceneInfoText { scene, info_text } => {
                self.set_scene_info_text(*scene, info_text)
            }
            Command::SetSceneColour { scenes, colour } => {
                if let Some(colour) = colour {
                    crate::colour::check_swatch("scene colour", *colour)?;
                }
                for id in each_once(scenes, |s| s.0)? {
                    let position = self.scene_position(id).ok_or(ModelError::NoSuchScene(id))?;
                    if let Some(scene) = self.scenes.get_mut(position)
                        && scene.colour != *colour
                    {
                        scene.colour = *colour;
                    }
                }
                Ok(())
            }
            Command::SetSceneTempo { scene, tempo } => self.set_scene_tempo(*scene, *tempo),
            Command::SetSceneFollowActions { scene, patch } => {
                let targets = self.scenes.len();
                let position = self
                    .scene_position(*scene)
                    .ok_or(ModelError::NoSuchScene(*scene))?;
                let mut next = self.scenes[position].follow;
                next.apply(patch, targets)?;
                if let Some(scene) = self.scenes.get_mut(position)
                    && scene.follow != next
                {
                    scene.follow = next;
                }
                Ok(())
            }
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
                self.prune_mappings();
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
            Command::SetDeviceInfoText { device, info_text } => {
                if self.device_ref(*device)?.info_text != *info_text {
                    self.device_mut(*device)?.info_text.clone_from(info_text);
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
                shape,
                articulations,
                mpe,
            } => {
                let mut loaded = self.device_ref(*device)?.clone();
                loaded.set_both(params.clone());
                loaded.shape.clone_from(shape);
                if let Some(articulations) = articulations {
                    loaded.set_articulations(articulations.clone());
                }
                if let Some(mpe) = mpe {
                    mpe.settings.check()?;
                    if matches!(loaded.kind, DeviceKind::Plugin { .. }) {
                        loaded.mpe = *mpe;
                    }
                }
                loaded.set_name(name);
                // The preset the device holds is the one its name names (the
                // plug-in list spec, R5).
                loaded.preset_name = (!name.is_empty()).then(|| name.clone());
                check_device(&loaded)?;
                if loaded != *self.device_ref(*device)? {
                    *self.device_mut(*device)? = loaded;
                }
                Ok(())
            }
            Command::SetPluginMpeMode { device, on } => {
                self.edit_plugin_mpe(*device, |mpe| mpe.on = *on)
            }
            Command::SetPluginTuning { device, on } => {
                self.edit_plugin_mpe(*device, |mpe| mpe.tuning = *on)
            }
            Command::SetPluginMpeSettings { device, settings } => {
                settings.check()?;
                self.edit_plugin_mpe(*device, |mpe| mpe.settings = *settings)
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
            Command::SetDeviceRole { device, role } => self.set_device_role(*device, *role),
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
                // Spec: macros R22. A stepped macro takes only its steps.
                let stepped = self.stepped_macro_value(*device, *param, *value);
                self.check_not_held(crate::modulator::ModTarget::Device {
                    device: *device,
                    param: *param,
                })?;
                // Spec: modulation R16. Remote Control holds what a
                // modulator maps; one parameter has one holder.
                if *param == crate::modulator::MODE && stepped >= 0.5 {
                    let current = self.device_ref(*device)?;
                    if current
                        .mappings
                        .iter()
                        .any(|m| self.held_by(m.target, *device).is_some())
                    {
                        return Err(ModelError::InvalidInput(
                            "another modulator in Remote Control already holds a parameter this \
                             one maps",
                        ));
                    }
                }
                if self.device_ref(*device)?.params.get(param) != Some(&stepped) {
                    let owner = self.device_owner(*device)?;
                    self.device_mut(*device)?.set_param(*param, stepped);
                    self.track_mut(owner)?
                        .override_target(&AutomationTarget::Device {
                            device: *device,
                            param: *param,
                        });
                }
                Ok(())
            }
            Command::SetConfiguredParams { device, params } => {
                self.device_mut(*device)?.set_configured(params.clone());
                Ok(())
            }
            Command::MapModulator {
                device,
                slot,
                target,
            } => self.map_modulator(*device, *slot, *target),
            Command::UnmapModulator { device, slot } => self.unmap_modulator(*device, *slot),
            Command::SetShape { device, points } => self.set_shape(*device, points),
            Command::AddArticulation {
                device,
                index,
                name,
            } => self.add_articulation(*device, *index, name),
            Command::DeleteArticulation { device, index } => {
                self.delete_articulation(*device, *index)
            }
            Command::MoveArticulation { device, index, to } => {
                self.move_articulation(*device, *index, *to)
            }
            Command::SetArticulation {
                device,
                index,
                name,
                trigger,
                outputs,
                channel,
            } => self.set_articulation(
                *device,
                *index,
                name.as_deref(),
                *trigger,
                outputs.as_deref(),
                *channel,
            ),
            Command::CompareSwitch { device } => {
                if !self.device_ref(*device)?.can_compare() {
                    return Err(ModelError::NoCompare(*device));
                }
                let owner = self.device_owner(*device)?;
                self.device_mut(*device)?.compare_switch();
                override_device(&mut self.track_mut(owner)?.automation, *device);
                Ok(())
            }
            Command::CompareCopy { device } => {
                if !self.device_ref(*device)?.can_compare() {
                    return Err(ModelError::NoCompare(*device));
                }
                if self.device_ref(*device)?.other_params.is_some() {
                    self.device_mut(*device)?.compare_copy();
                }
                Ok(())
            }
            Command::SetDeviceSample {
                device,
                media,
                slot,
            } => {
                check_value(
                    "sample slot",
                    f64::from(*slot),
                    usize::from(*slot) < crate::device::MAX_MEDIA_SLOTS,
                )?;
                if self.device_ref(*device)?.media_at(usize::from(*slot)) != *media {
                    self.device_mut(*device)?
                        .set_media(usize::from(*slot), *media);
                }
                Ok(())
            }
            Command::SetDeviceWarp { device, warp } => {
                self.device_mut(*device)?.warp.warp = *warp;
                Ok(())
            }
            Command::SetDeviceWarpMode { device, mode } => {
                self.device_mut(*device)?.warp.mode = *mode;
                Ok(())
            }
            Command::SetDeviceWarpSettings {
                device,
                beats,
                tones,
                texture,
                complex_pro,
            } => {
                let settings = self.device_ref(*device)?.warp.settings.updated(
                    *beats,
                    *tones,
                    *texture,
                    *complex_pro,
                )?;
                self.device_mut(*device)?.warp.settings = settings;
                Ok(())
            }
            Command::SetDeviceSampleBpm { device, bpm } => {
                check_value("sample bpm", *bpm, (1.0..=1000.0).contains(bpm))?;
                self.device_mut(*device)?.warp.bpm = Some(*bpm);
                Ok(())
            }
            Command::ScaleDeviceTempo { device, factor } => {
                check_value("scale factor", *factor, factor.is_finite() && *factor > 0.0)?;
                let current = self.device_ref(*device)?.warp.bpm.unwrap_or(120.0);
                self.device_mut(*device)?.warp.bpm = Some((current * factor).clamp(1.0, 1000.0));
                Ok(())
            }
            Command::AddSlice { device, frame } => {
                let d = self.device_mut(*device)?;
                if !d.slices.contains(frame) {
                    d.slices.push(*frame);
                    d.slices.sort_unstable();
                }
                Ok(())
            }
            Command::MoveSlice { device, from, to } => {
                let d = self.device_mut(*device)?;
                d.slices.retain(|f| f != from);
                if !d.slices.contains(to) {
                    d.slices.push(*to);
                    d.slices.sort_unstable();
                }
                Ok(())
            }
            Command::DeleteSlice { device, frame } => {
                self.device_mut(*device)?.slices.retain(|f| f != frame);
                Ok(())
            }
            Command::SetDeviceRegion { device, region } => {
                match region {
                    Some(SampleRegion::Frames { start, end }) => {
                        check_value("region start", *start as f64, start < end)?;
                    }
                    Some(SampleRegion::Shares {
                        start_share,
                        end_share,
                    }) => check_value(
                        "region start",
                        *start_share,
                        (0.0..*end_share).contains(start_share) && *end_share <= 1.0,
                    )?,
                    None => {}
                }
                self.device_mut(*device)?.region = *region;
                Ok(())
            }
            Command::InsertLayers {
                device,
                index,
                layers,
            } => self.edit_layers(*device, |l| l.insert(*index, layers).map(drop)),
            Command::DeleteLayers { device, layers } => {
                self.edit_layers(*device, |l| l.remove(layers))
            }
            Command::DuplicateLayers { device, layers } => {
                self.edit_layers(*device, |l| l.duplicate(layers).map(drop))
            }
            Command::RenameLayer {
                device,
                layer,
                name,
            } => self.edit_layers(*device, |l| l.rename(*layer, name)),
            Command::MoveLayers {
                device,
                layers,
                index,
            } => self.edit_layers(*device, |l| l.moved(layers, *index)),
            Command::SortLayers {
                device,
                by,
                descending,
            } => self.edit_layers(*device, |l| {
                l.sort(*by, *descending);
                Ok(())
            }),
            Command::SetLayerZones {
                device,
                kind,
                zones,
            } => {
                let zones: Vec<_> = zones.iter().map(|z| (z.layer, z.zone)).collect();
                self.edit_layers(*device, |l| l.set_zones(*kind, &zones))
            }
            Command::UpdateLayers {
                device,
                layers,
                settings,
            } => self.edit_layers(*device, |l| l.update(layers, settings)),
            Command::SetLayerMedia {
                device,
                layer,
                media,
                length,
                sample_rate,
                channels,
            } => self.edit_layers(*device, |l| {
                l.replace_media(*layer, *media, *length, *sample_rate, *channels)
            }),
            Command::AddSampleEdit {
                device,
                layer: Some(layer),
                edit,
            } => self.edit_layers(*device, |l| l.add_edit(*layer, *edit)),
            Command::AddSampleEdit {
                device,
                layer: None,
                edit,
            } => {
                // Whether the selection fits the sample is for the caller
                // that knows it; this checks the edit itself.
                edit.length_after(u64::MAX)?;
                let d = self.device_mut(*device)?;
                d.sample_edits.push(*edit);
                d.region = d.region.and_then(|region| region.after(edit));
                Ok(())
            }
            Command::ClearSampleEdits {
                device,
                layer: Some(layer),
                length,
            } => self.edit_layers(*device, |l| l.clear_edits(*layer, *length)),
            Command::ClearSampleEdits {
                device,
                layer: None,
                ..
            } => {
                let d = self.device_mut(*device)?;
                d.sample_edits.clear();
                // The unedited sample is another length: the flags mark
                // nothing in it, so the region is the whole of it again.
                d.region = None;
                Ok(())
            }
            Command::SetProcessedLayerSample {
                device,
                layer,
                media,
                length,
                crop,
            } => self.edit_layers(*device, |l| l.set_processed(*layer, *media, *length, *crop)),
            Command::DistributeLayers {
                device,
                layers,
                how,
                kind,
            } => self.edit_layers(*device, |l| l.distribute(layers, *how, *kind)),
            Command::SetLayerOptions {
                device,
                ram_mode,
                snap,
                linear_loop_fades,
            } => self.edit_layers(*device, |l| {
                l.ram_mode = ram_mode.unwrap_or(l.ram_mode);
                l.snap = snap.unwrap_or(l.snap);
                l.linear_loop_fades = linear_loop_fades.unwrap_or(l.linear_loop_fades);
                Ok(())
            }),
            Command::SetSidechainInput { device, input } => {
                let input = input.clone();
                self.edit_sidechain(*device, |s| s.input = input)?;
                self.check_routing(self.device_owner(*device)?)
            }
            Command::SetMidiSidechainSource { device, track } => {
                let mut sidechain = self
                    .device_ref(*device)?
                    .midi_sidechain
                    .clone()
                    .ok_or(ModelError::NoMidiSidechain(*device))?;
                sidechain.source = *track;
                self.device_mut(*device)?.midi_sidechain = Some(sidechain);
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
            Command::SetExternal {
                device,
                midi_to,
                channel,
                mpe,
                audio_from,
                audio_to,
                hardware_latency,
                cv_outputs,
                cv_inputs,
            } => {
                let mut external = self
                    .device_ref(*device)?
                    .external
                    .clone()
                    .ok_or(ModelError::NoExternal(*device))?;
                match midi_to {
                    Some(ExternalPort::None) => external.port = None,
                    Some(ExternalPort::Port { name }) => external.port = Some(name.clone()),
                    None => {}
                }
                // Spec: mpe R19. A channel chosen takes MPE's place.
                match (channel, mpe) {
                    (Some(_), Some(_)) => {
                        return Err(ModelError::InvalidInput(
                            "MIDI To is a channel or MPE, not both",
                        ));
                    }
                    (Some(channel), None) => {
                        external.channel = *channel;
                        external.mpe = None;
                    }
                    (None, Some(mpe)) => {
                        mpe.check()?;
                        external.mpe = Some(*mpe);
                    }
                    (None, None) => {}
                }
                external.audio_from = audio_from.unwrap_or(external.audio_from);
                if let Some(channel) = audio_to {
                    external.audio_to = Some(*channel);
                }
                if let Some(latency) = hardware_latency {
                    check_value(
                        "hardware latency",
                        match *latency {
                            HardwareLatency::Milliseconds { value } => value,
                            HardwareLatency::Samples { value } => f64::from(value),
                        },
                        latency.is_valid(),
                    )?;
                    external.hardware_latency = *latency;
                }
                // Spec: cv-tools R4. Each port is named as the device names
                // it; the ones given change, the others stay.
                for (given, kept) in [
                    (cv_outputs, &mut external.cv_outputs),
                    (cv_inputs, &mut external.cv_inputs),
                ] {
                    for port in given.iter().flatten() {
                        if port.channel == Some(0) {
                            return Err(ModelError::InvalidInput(
                                "interface channels count from 1",
                            ));
                        }
                        let Some(held) = kept.iter_mut().find(|p| p.name == port.name) else {
                            return Err(ModelError::InvalidInput(
                                "the device has no CV port of that name",
                            ));
                        };
                        held.channel = port.channel;
                    }
                }
                crate::routing::check_port(
                    external.port.as_deref().unwrap_or("-"),
                    external.channel,
                )?;
                if Some(&external) != self.device_ref(*device)?.external.as_ref() {
                    self.device_mut(*device)?.external = Some(external);
                }
                Ok(())
            }
            Command::SetCvCalibration {
                device,
                calibration,
            } => {
                // The node plays every pitch through the calibration on the
                // audio thread, so only a rising line and a calibration's
                // steps, by rising voltage, are taken.
                if calibration.as_ref().is_some_and(|c| !c.is_valid()) {
                    return Err(ModelError::InvalidInput(
                        "a calibration rises by a positive number of octaves a volt from a \
                         positive frequency, over at most eight steps by rising voltage",
                    ));
                }
                let mut external = self
                    .device_ref(*device)?
                    .external
                    .clone()
                    .ok_or(ModelError::NoExternal(*device))?;
                external.cv_calibration.clone_from(calibration);
                self.device_mut(*device)?.external = Some(external);
                Ok(())
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
                let rack = self.rack_holding_mut(*chain)?;
                let removed = rack
                    .remove_chain(*chain)
                    .ok_or(ModelError::NoSuchChain(*chain))?;
                rack.prune_sends();
                self.index.remove_chain(&removed);
                self.forget_in(owner, &Gone::chain(&removed))?;
                self.prune_mappings();
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
            Command::SetChainZone { chain, kind, zone } => {
                let rack = self.chain_rack(*chain).and_then(Device::rack_kind);
                if !rack.is_some_and(|rack| rack.zone_kinds().contains(kind)) {
                    return Err(ModelError::ChainHasNoZone {
                        chain: *chain,
                        kind: *kind,
                    });
                }
                if !zone.is_valid(*kind) {
                    return Err(ModelError::InvalidInput(
                        "a chain's zone must hold a value of its scale, with its fades inside it",
                    ));
                }
                let zones = self.chain_ref(*chain)?.zones;
                if zones.get(*kind) != *zone {
                    self.chain_mut(*chain)?.zones = zones.with(*kind, *zone);
                }
                Ok(())
            }
            Command::SetChainReceive { chain, receive } => {
                if let Receive::Note(note) = receive {
                    check_value("receive note", f64::from(*note), *note < 128)?;
                }
                self.edit_drum_chain(*chain, false, |c| c.receive = *receive)
            }
            Command::SetChainPlay { chain, play } => {
                check_value("play note", f64::from(*play), *play < 128)?;
                self.edit_drum_chain(*chain, false, |c| c.play = *play)
            }
            Command::SetChainChoke { chain, choke } => {
                if let Some(group) = choke {
                    check_value("choke group", f64::from(*group), (1..=16).contains(group))?;
                }
                self.edit_drum_chain(*chain, false, |c| c.choke = *choke)
            }
            Command::SetChainSend { chain, to, level } => {
                check_value("send level", *level, level.is_finite() && *level >= 0.0)?;
                let owner = self.chain_owner(*chain)?;
                let rack = self
                    .chain_rack(*chain)
                    .ok_or(ModelError::NoSuchChain(*chain))?;
                if !rack.chains.iter().any(|c| c.is_return && c.id == *to) {
                    return Err(ModelError::NoSuchChain(*to));
                }
                self.edit_drum_chain(*chain, false, |c| {
                    match c.sends.iter_mut().find(|s| s.to == *to) {
                        Some(send) => send.level = *level,
                        None => c.sends.push(ChainSend {
                            to: *to,
                            level: *level,
                        }),
                    }
                })?;
                self.track_mut(owner)?
                    .override_target(&AutomationTarget::ChainSend {
                        chain: *chain,
                        to: *to,
                    });
                Ok(())
            }
            Command::SetChainAudioTo { chain, track } => {
                if let Some(track) = track {
                    check_kind(self.track_ref(*track)?, |k| k == TrackKind::Return, RETURN)?;
                }
                self.edit_drum_chain(*chain, true, |c| c.audio_to = *track)?;
                self.check_routing(self.chain_owner(*chain)?)
            }
            Command::MapMacro {
                rack,
                macro_number,
                target,
            } => self.map_macro(*rack, *macro_number, *target),
            Command::UnmapMacro { rack, target } => {
                let current = self.device_ref(*rack)?;
                if current.rack_kind().is_none() {
                    return Err(ModelError::InvalidInput("only a rack has macros"));
                }
                let mut macros = current.macros.clone();
                macros.mappings.retain(|m| m.target != *target);
                if macros != self.device_ref(*rack)?.macros {
                    self.device_mut(*rack)?.macros = macros;
                }
                Ok(())
            }
            Command::SetMappingRange {
                rack,
                target,
                min,
                max,
            } => {
                check_value(
                    "mapping min",
                    *min,
                    min.is_finite() && (0.0..=1.0).contains(min),
                )?;
                check_value(
                    "mapping max",
                    *max,
                    max.is_finite() && (0.0..=1.0).contains(max),
                )?;
                self.edit_mapping(*rack, *target, |m| (m.min, m.max) = (*min, *max))
            }
            Command::InvertMapping { rack, target } => {
                self.edit_mapping(*rack, *target, |m| std::mem::swap(&mut m.min, &mut m.max))
            }
            Command::RenameMacro {
                rack,
                macro_number,
                name,
            } => self.edit_macro(*rack, *macro_number, |m| m.name.clone_from(name)),
            Command::SetMacroColour {
                rack,
                macro_number,
                colour,
            } => {
                if let Some(colour) = colour {
                    check_value("macro colour", f64::from(*colour), *colour < CHAIN_COLOURS)?;
                }
                self.edit_macro(*rack, *macro_number, |m| m.colour = *colour)
            }
            Command::SetMacroInfo {
                rack,
                macro_number,
                info,
            } => self.edit_macro(*rack, *macro_number, |m| m.info.clone_from(info)),
            Command::RandomizeMacros { rack, seed } => self.randomize_macros(*rack, *seed),
            Command::SetMacroRandomizeExcluded {
                rack,
                macro_number,
                excluded,
            } => self.edit_macro(*rack, *macro_number, |m| m.randomize_excluded = *excluded),
            Command::SetMacroStyle {
                rack,
                macro_number,
                style,
                steps,
            } => self.set_macro_style(*rack, *macro_number, *style, *steps),
            Command::SetMappedValue { target, value } => {
                check_value("mapped value", *value, value.is_finite())?;
                match target {
                    MacroTarget::Device { device, param } => {
                        // Spec: macros R22. A stepped macro takes only its
                        // steps.
                        let stepped = self.stepped_macro_value(*device, *param, *value);
                        if self.device_ref(*device)?.params.get(param) != Some(&stepped) {
                            self.device_mut(*device)?.set_param(*param, stepped);
                        }
                    }
                    MacroTarget::Chain { chain, param } => {
                        self.set_chain_mixer_quietly(*chain, *param, *value)?;
                    }
                }
                Ok(())
            }
            Command::StoreVariation { rack } => {
                let values = self.rack_macro_values(*rack)?;
                self.device_mut(*rack)?.macros.store_variation(values);
                Ok(())
            }
            Command::LaunchVariation { rack, index } => self.launch_variation(*rack, *index),
            Command::OverwriteVariation { rack, index } => {
                let values = self.rack_macro_values(*rack)?;
                self.device_mut(*rack)?
                    .macros
                    .overwrite_variation(*index, values)
            }
            Command::RenameVariation { rack, index, name } => {
                self.variation_mut(*rack, *index)?.name.clone_from(name);
                Ok(())
            }
            Command::DuplicateVariation { rack, index } => {
                let variation = self.variation(*rack, *index)?.clone();
                self.device_mut(*rack)?
                    .macros
                    .variations
                    .insert(index + 1, variation);
                Ok(())
            }
            Command::DeleteVariation { rack, index } => {
                self.variation(*rack, *index)?;
                self.device_mut(*rack)?.macros.variations.remove(*index);
                Ok(())
            }
            Command::SetMacroVariationExcluded {
                rack,
                macro_number,
                excluded,
            } => self.edit_macro(*rack, *macro_number, |m| m.variation_excluded = *excluded),
        }
    }

    /// Sets a chain's mixer control without overriding its automation
    /// envelope (macros spec, R11).
    fn set_chain_mixer_quietly(
        &mut self,
        chain: ChainId,
        param: MixerParam,
        value: f64,
    ) -> Result<(), ModelError> {
        if self
            .chain_rack(chain)
            .and_then(Device::rack_kind)
            .is_some_and(|kind| !kind.chains_have_volume_and_pan())
            && matches!(param, MixerParam::Volume | MixerParam::Pan)
        {
            return Err(ModelError::ChainHasNoMixer(chain));
        }
        let mut mixer = self.chain_ref(chain)?.mixer;
        let changed = match param {
            MixerParam::Volume => {
                if mixer.volume != value {
                    mixer.volume = value;
                    true
                } else {
                    false
                }
            }
            MixerParam::Pan => {
                if mixer.pan != value {
                    mixer.pan = value;
                    true
                } else {
                    false
                }
            }
            MixerParam::Mute => {
                let enabled = value < 0.5;
                let changed = mixer.enabled != enabled;
                mixer.enabled = enabled;
                changed
            }
            MixerParam::SplitLeft | MixerParam::SplitRight => {
                return Err(ModelError::ChainHasNoSplitPan(chain));
            }
        };
        if changed {
            self.chain_mut(chain)?.mixer = mixer;
        }
        Ok(())
    }

    /// Maps `target` inside `rack` to macro `number` (macros spec, R5),
    /// moving a mapping it already has.
    fn map_macro(
        &mut self,
        rack: DeviceId,
        number: u8,
        target: MacroTarget,
    ) -> Result<(), ModelError> {
        check_macro_number(number)?;
        let current = self.device_ref(rack)?;
        if current.rack_kind().is_none() {
            return Err(ModelError::InvalidInput("only a rack has macros"));
        }
        match target {
            MacroTarget::Device { device, param } => {
                if device == rack && macro_number(param).is_some() {
                    return Err(ModelError::InvalidInput(
                        "a macro can't map its own rack's macros",
                    ));
                }
                if current.find(device).is_none() {
                    return Err(ModelError::InvalidInput(
                        "a macro can only map a device inside its rack",
                    ));
                }
            }
            MacroTarget::Chain { chain, param } => {
                if current.find_chain(chain).is_none() {
                    return Err(ModelError::InvalidInput(
                        "a macro can only map a chain inside its rack",
                    ));
                }
                if matches!(param, MixerParam::SplitLeft | MixerParam::SplitRight) {
                    return Err(ModelError::ChainHasNoSplitPan(chain));
                }
            }
        }
        let mut macros = current.macros.clone();
        macros.mappings.retain(|m| m.target != target);
        macros.mappings.push(MacroMapping {
            macro_number: number,
            target,
            min: 0.0,
            max: 1.0,
        });
        self.device_mut(rack)?.macros = macros;
        Ok(())
    }

    /// Changes the mapping of `target` in `rack` with `edit`.
    fn edit_mapping(
        &mut self,
        rack: DeviceId,
        target: MacroTarget,
        edit: impl FnOnce(&mut MacroMapping),
    ) -> Result<(), ModelError> {
        let current = self.device_ref(rack)?;
        let mut macros = current.macros.clone();
        let mapping = macros
            .mappings
            .iter_mut()
            .find(|m| m.target == target)
            .ok_or(ModelError::InvalidInput("no such macro mapping"))?;
        edit(mapping);
        self.device_mut(rack)?.macros = macros;
        Ok(())
    }

    /// Changes macro `number`'s settings with `edit`.
    fn edit_macro(
        &mut self,
        rack: DeviceId,
        number: u8,
        edit: impl FnOnce(&mut Macro),
    ) -> Result<(), ModelError> {
        check_macro_number(number)?;
        let current = self.device_ref(rack)?;
        if current.rack_kind().is_none() {
            return Err(ModelError::InvalidInput("only a rack has macros"));
        }
        let mut macros = current.macros.clone();
        edit(macros.setting_mut(number));
        self.device_mut(rack)?.macros = macros;
        Ok(())
    }

    /// Sets a macro's style and, for a stepped one, how many steps it takes
    /// (macros spec, R21 and R22). A macro that becomes stepped moves to its
    /// nearest step, in the same step.
    fn set_macro_style(
        &mut self,
        rack: DeviceId,
        number: u8,
        style: MacroStyle,
        steps: Option<u8>,
    ) -> Result<(), ModelError> {
        check_macro_number(number)?;
        let current = self.device_ref(rack)?;
        if current.rack_kind().is_none() {
            return Err(ModelError::InvalidInput("only a rack has macros"));
        }
        let steps = steps.unwrap_or_else(|| current.macros.setting(number).steps);
        if !(MACRO_STEPS_MIN..=MACRO_STEPS_MAX).contains(&steps) {
            return Err(ModelError::InvalidInput(
                "a stepped macro takes 2 to 16 steps",
            ));
        }
        let mut macros = current.macros.clone();
        let setting = macros.setting_mut(number);
        setting.style = style;
        setting.steps = steps;
        let stepped = setting.is_stepped();
        self.device_mut(rack)?.macros = macros;
        if stepped {
            // Spec: macros R22. A macro that becomes stepped takes its
            // nearest step.
            let param = crate::macro_param(number);
            let value = self
                .device_ref(rack)?
                .params
                .get(&param)
                .copied()
                .unwrap_or(0.0);
            let stepped_value = crate::macro_step_position(steps, value);
            if stepped_value != value {
                self.device_mut(rack)?.set_param(param, stepped_value);
            }
        }
        Ok(())
    }

    /// The value a stepped macro takes for `value`: its nearest step
    /// (macros spec, R22). Any other parameter keeps the value it is given.
    fn stepped_macro_value(&self, device: DeviceId, param: ParamId, value: f64) -> f64 {
        let Some(number) = crate::macro_number(param) else {
            return value;
        };
        let Some(device) = self.device_ref(device).ok() else {
            return value;
        };
        if device.rack_kind().is_none() {
            return value;
        }
        device.macros.setting(number).position_of(value)
    }

    /// Every macro of `rack`'s values, in number order (macros spec, R26).
    fn rack_macro_values(&self, rack: DeviceId) -> Result<Vec<f64>, ModelError> {
        let device = self.device_ref(rack)?;
        if device.rack_kind().is_none() {
            return Err(ModelError::InvalidInput("only a rack has macros"));
        }
        Ok(device.macro_values())
    }

    /// The variation at `index` of `rack`.
    fn variation(
        &self,
        rack: DeviceId,
        index: usize,
    ) -> Result<&crate::rack::Variation, ModelError> {
        self.device_ref(rack)?
            .macros
            .variations
            .get(index)
            .ok_or(ModelError::InvalidInput("no such macro variation"))
    }

    /// The variation at `index` of `rack`, to change.
    fn variation_mut(
        &mut self,
        rack: DeviceId,
        index: usize,
    ) -> Result<&mut crate::rack::Variation, ModelError> {
        self.device_mut(rack)?
            .macros
            .variations
            .get_mut(index)
            .ok_or(ModelError::InvalidInput("no such macro variation"))
    }

    /// Sets every macro of `rack` that isn't excluded from variations to the
    /// value the variation at `index` holds, at once, as a hand move on each
    /// (macros spec, R27).
    fn launch_variation(&mut self, rack: DeviceId, index: usize) -> Result<(), ModelError> {
        let current = self.device_ref(rack)?;
        if current.rack_kind().is_none() {
            return Err(ModelError::InvalidInput("only a rack has macros"));
        }
        let variation = current
            .macros
            .variations
            .get(index)
            .ok_or(ModelError::InvalidInput("no such macro variation"))?;
        let moves: Vec<(u8, f64)> = (1..=MACROS)
            .filter(|number| !current.macros.setting(*number).variation_excluded)
            .filter_map(|number| {
                variation
                    .values
                    .get(usize::from(number - 1))
                    // Spec: macros R22. A stepped macro takes only its steps.
                    .map(|value| {
                        let value = value.clamp(0.0, 1.0);
                        (number, current.macros.setting(number).position_of(value))
                    })
            })
            .collect();
        let owner = self.device_owner(rack)?;
        for (number, value) in &moves {
            self.device_mut(rack)?
                .set_param(crate::macro_param(*number), *value);
        }
        for (number, _) in &moves {
            self.track_mut(owner)?
                .override_target(&AutomationTarget::Device {
                    device: rack,
                    param: crate::macro_param(*number),
                });
        }
        Ok(())
    }

    /// Sets every mapped macro of `rack` that isn't excluded to a random
    /// position (macros spec, R15), as a hand move on each.
    fn randomize_macros(&mut self, rack: DeviceId, seed: Option<u64>) -> Result<(), ModelError> {
        let current = self.device_ref(rack)?;
        if current.rack_kind().is_none() {
            return Err(ModelError::InvalidInput("only a rack has macros"));
        }
        let mut state = seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos() as u64)
        });
        let values: Vec<(u8, f64)> = (1..=MACROS)
            .filter(|number| current.macros.has_mappings(*number))
            .filter(|number| !current.macros.setting(*number).randomize_excluded)
            .map(|number| {
                let setting = current.macros.setting(number);
                let value = next_unit(&mut state);
                // Spec: macros R24. A stepped macro is randomised to a step.
                let value = if setting.is_stepped() {
                    let steps = f64::from(setting.steps);
                    let step = (value * steps).min(steps - 1.0).floor();
                    step / (steps - 1.0)
                } else {
                    value
                };
                (number, value)
            })
            .collect();
        let owner = self.device_owner(rack)?;
        for (number, value) in &values {
            self.device_mut(rack)?
                .set_param(crate::macro_param(*number), *value);
        }
        for (number, _) in &values {
            self.track_mut(owner)?
                .override_target(&AutomationTarget::Device {
                    device: rack,
                    param: crate::macro_param(*number),
                });
        }
        Ok(())
    }

    /// Changes a multisampler's layers with `edit`, which either succeeds or
    /// leaves them as they were.
    fn edit_layers(
        &mut self,
        id: DeviceId,
        edit: impl FnOnce(&mut Layers) -> Result<(), ModelError>,
    ) -> Result<(), ModelError> {
        let mut layers = self.device_ref(id)?.layers.clone();
        edit(&mut layers)?;
        if self.device_ref(id)?.layers != layers {
            self.device_mut(id)?.layers = layers;
        }
        Ok(())
    }

    /// Changes a device's sidechain settings with `edit`, if it has a
    /// sidechain input.
    /// Changes a plug-in device's MPE Mode and settings with `change`.
    fn edit_plugin_mpe(
        &mut self,
        device: DeviceId,
        change: impl FnOnce(&mut crate::PluginMpe),
    ) -> Result<(), ModelError> {
        let current = self.device_ref(device)?;
        if !matches!(current.kind, DeviceKind::Plugin { .. }) {
            return Err(ModelError::InvalidInput(
                "only a plug-in device has an MPE Mode",
            ));
        }
        let mut mpe = current.mpe;
        change(&mut mpe);
        if mpe != current.mpe {
            self.device_mut(device)?.mpe = mpe;
        }
        Ok(())
    }

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

    /// Replaces a track's contents with the audio track a bounce made
    /// (WP-4.15, R21, R23): the track keeps its place in the tree and its
    /// ID, and loses its devices, its clips, and its take lanes.
    fn replace_with_bounced(
        &mut self,
        id: TrackId,
        bounced: crate::bounce::BouncedTrack,
    ) -> Result<(), ModelError> {
        let old: Vec<crate::ids::ClipId> = self
            .track_ref(id)?
            .clips
            .keys()
            .copied()
            .chain(self.track_ref(id)?.slots.values().map(|clip| clip.id))
            .chain(
                self.track_ref(id)?
                    .take_lanes()
                    .flat_map(|lane| lane.clips.iter().map(|clip| clip.id)),
            )
            .collect();
        let lanes: Vec<TakeLaneId> = self.track_ref(id)?.take_lanes().map(|l| l.id).collect();
        for clip in old {
            self.index.clip_track.remove(&clip);
        }
        for lane in lanes {
            self.index.lane_track.remove(&lane);
        }
        // The dropped devices and chains leave the lookups, and so does every
        // route that named them. A MIDI track's own routes go with its kind:
        // audio routes to the track stay valid (R21).
        let mut gone = Gone::default();
        for device in self.track_ref(id)?.devices.clone() {
            gone.add_device(&device);
            self.index.remove_device(&device);
        }
        self.forget_chain_taps(&gone.chains);
        self.forget_device_outputs(&gone.devices);
        self.forget_midi_routes_to(id);
        {
            let track = self.track_mut(id)?;
            track.devices = Vector::new();
            track.clips = OrdMap::new();
            track.slots = OrdMap::new();
            track.removed_stop_buttons = BTreeSet::new();
            track.take_lanes = Vector::new();
            track.audition = None;
            track.automation = OrdMap::new();
            track.frozen = None;
            track.delay = TrackDelay::default();
            track.keep_monitoring_latency = true;
        }
        self.fill_bounced(id, bounced)
    }

    /// Gives a track the contents of the audio track a bounce made
    /// (WP-4.15, R21): its name, its mixer, its sends, its output routing,
    /// its mixer automation, and its clips.
    fn fill_bounced(
        &mut self,
        id: TrackId,
        bounced: crate::bounce::BouncedTrack,
    ) -> Result<(), ModelError> {
        let crate::bounce::BouncedTrack {
            name,
            mixer,
            sends,
            output,
            automation,
            clips,
            slots,
        } = bounced;
        for clip in &clips {
            check_clip(clip)?;
            check_extent(clip.start, clip.length)?;
            self.check_fresh(clip.id.0)?;
        }
        for (_, clip) in &slots {
            check_clip(clip)?;
            check_extent(clip.start, clip.length)?;
            self.check_fresh(clip.id.0)?;
        }
        let clip_ids: Vec<crate::ids::ClipId> = clips.iter().map(|clip| clip.id).collect();
        let slot_ids: Vec<crate::ids::ClipId> = slots.iter().map(|(_, clip)| clip.id).collect();
        {
            let track = self.track_mut(id)?;
            track.kind = TrackKind::Audio;
            track.name = name;
            track.mixer = mixer;
            track.sends = sends.into_iter().collect();
            track.routing = crate::track::Routing::new(TrackKind::Audio);
            track.routing.output = output;
            track.automation = automation
                .into_iter()
                .map(|entry| {
                    (
                        entry.target,
                        crate::automation::Automation {
                            envelope: entry.envelope,
                            enabled: entry.enabled,
                        },
                    )
                })
                .collect();
            let colour = track.colour;
            track.clips = clips
                .into_iter()
                .map(|mut clip| {
                    clip.colour.get_or_insert(colour);
                    (clip.id, Shared::new(clip))
                })
                .collect();
            track.slots = slots
                .into_iter()
                .map(|(scene, mut clip)| {
                    clip.colour.get_or_insert(colour);
                    (scene, Shared::new(clip))
                })
                .collect();
        }
        for clip in clip_ids.into_iter().chain(slot_ids) {
            self.index.clip_track.insert(clip, id);
        }
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
            for lane in &track.take_lanes {
                self.index.lane_track.remove(&lane.id);
                for clip in &lane.clips {
                    self.index.clip_track.remove(&clip.id);
                }
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
            // So do their Send envelopes and the clip envelopes on them.
            let sending: Vec<TrackId> = self
                .tracks
                .iter()
                .filter(|(_, track)| track.kind.has_sends())
                .map(|(&sender, _)| sender)
                .collect();
            for sender in sending {
                forget_where(
                    self.track_mut(sender)?,
                    |target| matches!(*target, AutomationTarget::Send { to } if to == id),
                );
            }
            // A drum rack's return chain sent to it goes back to the rack's
            // own output (drum racks R8).
            let holders: Vec<TrackId> = self
                .tracks
                .iter()
                .filter(|(_, track)| {
                    track
                        .devices_deep()
                        .any(|d| d.chains.iter().any(|c| c.audio_to == Some(id)))
                })
                .map(|(&holder, _)| holder)
                .collect();
            for holder in holders {
                for device in &mut self.track_mut(holder)?.devices {
                    Shared::make_mut(device).walk_mut(&mut |rack| {
                        for chain in &mut rack.chains {
                            if chain.audio_to == Some(id) {
                                chain.audio_to = None;
                            }
                        }
                    });
                }
            }
        }
        Ok(())
    }

    /// Takes a track out of its link group when it is the group's only
    /// member left (the linked-track editing spec, R5).
    fn end_single_links(&mut self) {
        let mut members: BTreeMap<LinkGroupId, Vec<TrackId>> = BTreeMap::new();
        for (id, track) in &self.tracks {
            if let Some(group) = track.link {
                members.entry(group).or_default().push(*id);
            }
        }
        for alone in members.into_values().filter(|m| m.len() == 1).flatten() {
            if let Some(track) = self.tracks.get_mut(&alone) {
                Shared::make_mut(track).link = None;
            }
        }
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
        self.check_clip_groove(clip)?;
        check_clip_fits(self.track_ref(track)?, &clip.data)?;
        check_extent(clip.start, clip.length)?;
        check_clip(clip)?;
        self.check_clip_envelopes(track, clip)?;
        // Spec: scales R7. A clip made from nothing takes the set's current
        // scale. A caller that named a scale keeps it, so the field the
        // command reference lists isn't silently dropped.
        let mut clip = clip.clone();
        if clip.scale.is_default() {
            clip.scale = self.settings.scale;
        }
        clip.colour.get_or_insert(self.track_ref(track)?.colour);
        let id = clip.id;
        self.track_mut(track)?.clips.insert(id, Shared::new(clip));
        self.index.clip_track.insert(id, track);
        Ok(())
    }

    fn add_take_lane(
        &mut self,
        track: TrackId,
        new: &NewTakeLane,
        index: Option<usize>,
    ) -> Result<(), ModelError> {
        check_kind(
            self.track_ref(track)?,
            TrackKind::holds_clips,
            AUDIO_OR_MIDI,
        )?;
        let mut lane = new.lane()?;
        let track_colour = self.track_ref(track)?.colour;
        for clip in &mut lane.clips {
            clip.colour.get_or_insert(track_colour);
        }
        self.check_fresh(lane.id.0)?;
        let mut seen = BTreeSet::from([lane.id.0]);
        for clip in &lane.clips {
            self.check_fresh(clip.id.0)?;
            if !seen.insert(clip.id.0) {
                return Err(ModelError::DuplicateId(clip.id.0));
            }
            self.check_take_clip(track, clip)?;
        }
        self.index.lane_track.insert(lane.id, track);
        for clip in &lane.clips {
            self.index.clip_track.insert(clip.id, track);
        }
        let lanes = &mut self.track_mut(track)?.take_lanes;
        let at = index.map_or(lanes.len(), |index| index.min(lanes.len()));
        lanes.insert(at, lane);
        Ok(())
    }

    /// Checks a clip bound for one of `track`'s take lanes as one bound for
    /// its main lane is checked.
    fn check_take_clip(&self, track: TrackId, clip: &Clip) -> Result<(), ModelError> {
        check_clip_fits(self.track_ref(track)?, &clip.data)?;
        check_extent(clip.start, clip.length)?;
        check_clip(clip)?;
        self.check_clip_envelopes(track, clip)
    }

    fn take_lane_owner(&self, lane: TakeLaneId) -> Result<TrackId, ModelError> {
        self.take_lane_track(lane)
            .ok_or(ModelError::NoSuchTakeLane(lane))
    }

    fn lane_mut(&mut self, lane: TakeLaneId) -> Result<&mut TakeLane, ModelError> {
        let owner = self.take_lane_owner(lane)?;
        self.track_mut(owner)?
            .take_lanes
            .iter_mut()
            .find(|held| held.id == lane)
            .ok_or(ModelError::NoSuchTakeLane(lane))
    }

    fn create_take_clip(&mut self, lane: TakeLaneId, clip: &Clip) -> Result<(), ModelError> {
        let owner = self.take_lane_owner(lane)?;
        self.check_fresh(clip.id.0)?;
        self.check_take_clip(owner, clip)?;
        let mut clip = clip.clone();
        if clip.scale.is_default() {
            clip.scale = self.settings.scale;
        }
        clip.colour.get_or_insert(self.track_ref(owner)?.colour);
        let id = clip.id;
        self.lane_mut(lane)?.clips.push(clip);
        self.index.clip_track.insert(id, owner);
        Ok(())
    }

    fn rename_take_lane(&mut self, lane: TakeLaneId, name: &str) -> Result<(), ModelError> {
        if name.is_empty() {
            return Err(ModelError::EmptyTakeLaneName);
        }
        let held = self.lane_mut(lane)?;
        if held.name != name {
            name.clone_into(&mut held.name);
        }
        Ok(())
    }

    /// Takes the lanes out of their tracks, and out of the lookups.
    fn remove_take_lanes(&mut self, lanes: &[TakeLaneId]) -> Result<(), ModelError> {
        // A lane listed twice goes once.
        let lanes: BTreeSet<TakeLaneId> = lanes.iter().copied().collect();
        for lane in lanes {
            let owner = self.take_lane_owner(lane)?;
            let track = self.track_mut(owner)?;
            let at = track
                .take_lanes
                .iter()
                .position(|held| held.id == lane)
                .ok_or(ModelError::NoSuchTakeLane(lane))?;
            let gone = track.take_lanes.remove(at);
            if track.audition == Some(lane) {
                track.audition = None;
            }
            self.index.lane_track.remove(&lane);
            for clip in gone.clips {
                self.index.clip_track.remove(&clip.id);
            }
        }
        Ok(())
    }

    fn delete_all_take_lanes(&mut self, tracks: &[TrackId]) -> Result<(), ModelError> {
        let mut lanes = Vec::new();
        for &track in tracks {
            let track = self.track_ref(track)?;
            check_kind(track, TrackKind::holds_clips, AUDIO_OR_MIDI)?;
            lanes.extend(track.take_lanes().map(|lane| lane.id));
        }
        self.remove_take_lanes(&lanes)
    }

    fn delete_unused_take_lanes(&mut self, tracks: &[TrackId]) -> Result<(), ModelError> {
        let mut lanes = Vec::new();
        for &track in tracks {
            let track = self.track_ref(track)?;
            check_kind(track, TrackKind::holds_clips, AUDIO_OR_MIDI)?;
            lanes.extend(crate::comp::unused_lanes(track, &self.settings.tempo));
        }
        self.remove_take_lanes(&lanes)
    }

    fn move_take_lanes(&mut self, lanes: &[TakeLaneId], index: usize) -> Result<(), ModelError> {
        let Some(&first) = lanes.first() else {
            return Ok(());
        };
        let owner = self.take_lane_owner(first)?;
        for &lane in lanes {
            if self.take_lane_owner(lane)? != owner {
                return Err(ModelError::TakeLanesNotOnOneTrack);
            }
        }
        let track = self.track_mut(owner)?;
        let all: Vec<TakeLane> = track.take_lanes.iter().cloned().collect();
        let (moving, rest): (Vec<TakeLane>, Vec<TakeLane>) =
            all.into_iter().partition(|lane| lanes.contains(&lane.id));
        let mut next = rest;
        let at = index.min(next.len());
        next.splice(at..at, moving);
        track.take_lanes = next.into_iter().collect();
        Ok(())
    }

    fn set_audition(&mut self, lane: TakeLaneId, on: bool) -> Result<(), ModelError> {
        let owner = self.take_lane_owner(lane)?;
        let track = self.track_mut(owner)?;
        if on {
            track.audition = Some(lane);
        } else if track.audition == Some(lane) {
            track.audition = None;
        }
        Ok(())
    }

    fn move_clip(&mut self, id: ClipId, track: TrackId, start: Ticks) -> Result<(), ModelError> {
        let owner = self.clip_owner(id)?;
        let clip = self.clip_ref(id)?;
        check_clip_fits(self.track_ref(track)?, &clip.data)?;
        check_extent(start, clip.length)?;
        // A clip already on the track's Arrangement lane just moves; one in
        // a Session slot leaves the slot for the lane.
        if owner == track && self.in_arrangement(id)? {
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

    /// Moves a clip to `start` on `track`'s main lane, or on its take lane
    /// `lane`, out of wherever it sits (the comping spec, R27).
    fn move_clip_to_lane(
        &mut self,
        id: ClipId,
        track: TrackId,
        lane: Option<TakeLaneId>,
        start: Ticks,
    ) -> Result<(), ModelError> {
        let owner = self.clip_owner(id)?;
        let clip = self.clip_ref(id)?;
        check_clip_fits(self.track_ref(track)?, &clip.data)?;
        check_extent(start, clip.length)?;
        if let Some(lane) = lane
            && self.take_lane_owner(lane)? != track
        {
            return Err(ModelError::TakeLanesNotOnOneTrack);
        }
        let mut moved = clip.clone();
        moved.start = start;
        if owner != track {
            self.keep_fitting_envelopes(track, &mut moved);
        }
        self.remove_clip(id)?;
        match lane {
            Some(lane) => self.lane_mut(lane)?.clips.push(moved),
            None => {
                self.track_mut(track)?.clips.insert(id, Shared::new(moved));
            }
        }
        self.index.clip_track.insert(id, track);
        Ok(())
    }

    /// Drops from `clip` the envelopes naming parameters `track` doesn't
    /// have: what moving a clip to another track does, since a clip envelope
    /// names a parameter of the track it's on (WP-3.15, as automation R11
    /// drops a device's envelopes when it leaves a track).
    fn keep_fitting_envelopes(&self, track: TrackId, clip: &mut Clip) {
        let envelopes = std::mem::take(&mut clip.envelopes);
        clip.envelopes = envelopes
            .into_iter()
            .filter(|envelope| self.check_clip_target(track, clip, envelope.target).is_ok())
            .collect();
    }

    /// Removes a clip from the Arrangement lane or the Session slot it sits
    /// in, and from the lookups.
    fn remove_clip(&mut self, id: ClipId) -> Result<(), ModelError> {
        let owner = self.clip_owner(id)?;
        if self.track_ref(owner)?.clips.contains_key(&id) {
            self.track_mut(owner)?.clips.remove(&id);
        } else if let Some(lane) = self.track_ref(owner)?.clip_lane(id) {
            self.lane_mut(lane)?.clips.retain(|clip| clip.id != id);
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

    fn set_scene_info_text(&mut self, id: SceneId, info_text: &str) -> Result<(), ModelError> {
        let position = self.scene_position(id).ok_or(ModelError::NoSuchScene(id))?;
        if let Some(scene) = self.scenes.get_mut(position)
            && scene.info_text != *info_text
        {
            scene.info_text = info_text.to_owned();
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
        self.check_clip_groove(clip)?;
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
        // Spec: scales R7. A clip made from nothing takes the set's current
        // scale. A caller that named a scale keeps it, so the field the
        // command reference lists isn't silently dropped.
        let mut clip = clip.clone();
        if clip.scale.is_default() {
            clip.scale = self.settings.scale;
        }
        clip.colour.get_or_insert(target.colour);
        let id = clip.id;
        self.track_mut(track)?
            .slots
            .insert(scene, Shared::new(clip));
        self.index.clip_track.insert(id, track);
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

    /// Refuses positions that fall inside a leading clip's span (the warping
    /// spec, R45), where the clip sets the tempo and the Song Tempo envelope
    /// can't be edited.
    fn check_outside_tempo_leaders(&self, at: &[Ticks]) -> Result<(), ModelError> {
        if at.iter().any(|&at| self.tempo_leader_covers(at)) {
            return Err(ModelError::InvalidInput(
                "a clip leads the tempo here: make it a follower, or use Unfollow Tempo \
                 Automation, first",
            ));
        }
        Ok(())
    }

    /// Switches an Arrangement clip's Lead/Follow switch (R43).
    fn set_tempo_leader(&mut self, id: ClipId, leader: bool) -> Result<(), ModelError> {
        if !self.in_arrangement(id)? {
            return Err(ModelError::InvalidInput(
                "only an Arrangement clip can lead the tempo: a Session clip follows",
            ));
        }
        self.edit_markers_with(id, |audio| audio.with_tempo_leader(leader))
    }

    /// Makes every leader a follower and writes the tempo they set into the
    /// Main track's Song Tempo envelope as ordinary breakpoints (R46), so the
    /// song plays exactly as it did.
    fn unfollow_tempo_automation(&mut self) -> Result<(), ModelError> {
        let leaders: Vec<ClipId> = self
            .tempo_leaders()
            .into_iter()
            .map(|(clip, _, _)| clip.id)
            .collect();
        if leaders.is_empty() {
            return Ok(());
        }
        let map = self.tempo_map();
        self.settings.tempo = map;
        for id in leaders {
            self.edit_markers(id, |audio| audio.tempo_leader = false)?;
        }
        Ok(())
    }

    /// Stretches a warped clip (R47): the dragged edge goes to `at`, the
    /// other stays put, and every beat of its contents scales about the beat
    /// at the fixed edge, so the same audio fills the new length. An unwarped
    /// clip resizes as a plain drag does.
    fn stretch_clip(&mut self, id: ClipId, edge: ClipEdge, at: Ticks) -> Result<(), ModelError> {
        if !self.in_arrangement(id)? {
            return Err(ModelError::ClipInSlot(id));
        }
        let clip = self.clip_ref(id)?.clone();
        let ClipData::Audio(audio) = &clip.data else {
            return Err(ModelError::NotAudioClip(id));
        };
        let audio = audio.clone();
        if !audio.warp {
            return match edge {
                ClipEdge::Left => self.trim_clip_start(id, at),
                ClipEdge::Right => {
                    let length = at.saturating_sub(clip.start);
                    check_extent(clip.start, length)?;
                    let mut next = clip;
                    next.length = length;
                    check_clip(&next)?;
                    *self.clip_mut(id)? = next;
                    Ok(())
                }
            };
        }
        let (start, length, anchor) = match edge {
            // The left edge stays: every beat scales about the Start marker's
            // beat, the clip's 1.1.1.
            ClipEdge::Right => (clip.start, at.saturating_sub(clip.start), audio.start),
            // The right edge stays: about the beat at the clip's end.
            ClipEdge::Left => (
                at,
                clip.end().saturating_sub(at),
                audio.start + clip.length.get() as f64,
            ),
        };
        check_extent(start, length)?;
        let factor = length.get() as f64 / clip.length.get() as f64;
        let audio = audio.with_beats_scaled(anchor, factor)?;
        let mut next = clip;
        next.start = start;
        next.length = length;
        next.data = ClipData::Audio(audio);
        check_clip(&next)?;
        *self.clip_mut(id)? = next;
        Ok(())
    }

    /// Crops `clip` (the audio clip editing spec, R14 to R17): its data to
    /// `from`..`to` of its contents, in the form the clip shows, and, when
    /// `span` is given, its edges to that span of the song. An audio clip's
    /// crop names the new file it plays; a MIDI clip's writes none.
    fn crop_clip(
        &mut self,
        id: ClipId,
        media: Option<ContentHash>,
        from: f64,
        to: f64,
        span: Option<(Ticks, Ticks)>,
    ) -> Result<(), ModelError> {
        let current = self.clip_ref(id)?.clone();
        let mut next = current.clone();
        match &current.data {
            ClipData::Audio(audio) => {
                let Some(media) = media else {
                    return Err(ModelError::InvalidInput(
                        "an audio clip's crop names the new file its region is written to",
                    ));
                };
                let mut data = audio.cropped(from, to)?;
                data.media = media;
                next.data = ClipData::Audio(data);
            }
            ClipData::Midi(midi) => {
                if media.is_some() {
                    return Err(ModelError::InvalidInput(
                        "a MIDI clip's crop writes no file",
                    ));
                }
                let data = midi.cropped(whole_tick(from)?, whole_tick(to)?);
                // The clip's contents begin at its Start marker (the clip
                // module's own rule), which the crop moved back.
                next.offset = data.start;
                next.data = ClipData::Midi(data);
            }
        }
        if let Some((start, end)) = span {
            if end <= start {
                return Err(ModelError::EmptyLength(end - start));
            }
            next.start = start;
            next.length = end - start;
        }
        check_clip_data(&next.data)?;
        check_clip_fits(self.track_ref(self.clip_owner(id)?)?, &next.data)?;
        check_extent(next.start, next.length)?;
        *self.clip_mut(id)? = next;
        Ok(())
    }

    /// Slides a warped clip's contents inside it, which stays put (R54): the
    /// Start marker, the End marker and the loop brace move the other way
    /// through its contents by `by`, so the same audio — its warp markers
    /// included — plays `by` ticks later (or earlier).
    fn slide_contents(&mut self, id: ClipId, by: Ticks) -> Result<(), ModelError> {
        if !self.in_arrangement(id)? {
            return Err(ModelError::ClipInSlot(id));
        }
        if by == Ticks::ZERO {
            return Ok(());
        }
        self.edit_markers_with(id, |audio| {
            audio.require_warp()?;
            let shift = |value: f64| value - by.get() as f64;
            Ok(AudioData {
                start: shift(audio.start),
                end: audio.end.map(shift),
                loop_start: shift(audio.loop_start),
                loop_end: audio.loop_end.map(shift),
                ..audio.clone()
            })
        })
    }

    /// Crops `clip` to the span of song time `start`..`end` (R16): its edges
    /// move to the span, and it plays the part of itself the span covered, at
    /// the same moments.
    fn crop_clip_to_range(
        &mut self,
        id: ClipId,
        media: Option<ContentHash>,
        start: Ticks,
        end: Ticks,
    ) -> Result<(), ModelError> {
        if !self.in_arrangement(id)? {
            return Err(ModelError::ClipInSlot(id));
        }
        if end <= start {
            return Err(ModelError::EmptyLength(end - start));
        }
        let clip = self.clip_ref(id)?.clone();
        // The span in the form the clip shows: a warped audio clip's and a
        // MIDI clip's contents advance a tick per tick of the song, and an
        // unwarped audio clip's by the time between the ticks.
        let (from, to) = match &clip.data {
            ClipData::Audio(audio) if !audio.warp => {
                let tempo = &self.settings.tempo;
                let at = |ticks: Ticks| tempo.ticks_to_seconds(ticks);
                (
                    audio.start + (at(start) - at(clip.start)),
                    audio.start + (at(end) - at(clip.start)),
                )
            }
            ClipData::Audio(audio) => (
                audio.start + (start - clip.start).get() as f64,
                audio.start + (end - clip.start).get() as f64,
            ),
            ClipData::Midi(midi) => (
                midi.start.get() as f64 + (start - clip.start).get() as f64,
                midi.start.get() as f64 + (end - clip.start).get() as f64,
            ),
        };
        self.crop_clip(id, media, from, to, Some((start, end)))
    }

    /// Sets an audio clip's markers or loop with `edit`, if the result is
    /// valid and its markers are in order, as Live keeps markers set by hand
    /// (the audio clips spec, R3 and R4).
    fn edit_markers(
        &mut self,
        id: ClipId,
        edit: impl FnOnce(&mut AudioData),
    ) -> Result<(), ModelError> {
        self.edit_markers_with(id, |audio| {
            let mut next = audio.clone();
            edit(&mut next);
            Ok(next)
        })
    }

    /// Replaces an audio clip's data with what `edit` makes of it, if the
    /// result is valid and its markers are in order.
    fn edit_markers_with(
        &mut self,
        id: ClipId,
        edit: impl FnOnce(&AudioData) -> Result<AudioData, ModelError>,
    ) -> Result<(), ModelError> {
        let ClipData::Audio(current) = &self.clip_ref(id)?.data else {
            return Err(ModelError::NotAudioClip(id));
        };
        let next = edit(current)?;
        check_marker_order(&next)?;
        self.edit_audio(id, |audio| *audio = next)
    }

    /// The set's tempo, in BPM, where a warped clip plays the point `at`
    /// (the clip's start without one): the warping spec, R39 and R40.
    fn set_bpm_at(&self, id: ClipId, at: Option<&WarpPoint>) -> Result<f64, ModelError> {
        let clip = self.clip_ref(id)?;
        let ClipData::Audio(audio) = &clip.data else {
            return Err(ModelError::NotAudioClip(id));
        };
        let beat = at.and_then(|at| match (at.marker, at.beat, at.time) {
            (Some(marker), None, None) => audio.warp_markers.get(marker).map(|m| m.beat),
            (None, Some(beat), None) => Some(beat),
            (None, None, Some(time)) => audio
                .warp_map()
                .ok()
                .map(|map| Ticks(map.time_to_beat(time).round() as i64)),
            _ => None,
        });
        let played = beat.map_or(Ticks::ZERO, |beat| Ticks(beat.get() - audio.start as i64));
        Ok(self.settings.tempo.bpm_at(clip.start + played).get())
    }

    /// Makes the same edit to each of `clips`, or to none if it's refused for
    /// one (the warping spec, R11): the files must be as long as each other's,
    /// which the app shell checks, since the document doesn't hold the media.
    fn edit_each(
        &mut self,
        clips: &[ClipId],
        edit: impl Fn(&AudioData) -> Result<AudioData, ModelError>,
    ) -> Result<(), ModelError> {
        if clips.is_empty() {
            return Err(ModelError::InvalidInput("name at least one clip"));
        }
        let mut edited = Vec::with_capacity(clips.len());
        for &id in clips {
            let ClipData::Audio(current) = &self.clip_ref(id)?.data else {
                return Err(ModelError::NotAudioClip(id));
            };
            let next = edit(current)?;
            check_marker_order(&next)?;
            check_audio(&next)?;
            edited.push((id, next));
        }
        for (id, next) in edited {
            self.edit_audio(id, |audio| *audio = next)?;
        }
        Ok(())
    }

    /// Checks that a command's positions are in the form the audio clip
    /// shows: ticks if it is warped, seconds if not (the warping spec, R4).
    fn check_form(&self, id: ClipId, ticks: bool) -> Result<(), ModelError> {
        match &self.clip_ref(id)?.data {
            ClipData::Audio(audio) if audio.warp != ticks => {
                Err(ModelError::InvalidInput(if audio.warp {
                    "this clip is warped: its positions are ticks of its contents, so pass `ticks` true"
                } else {
                    "this clip isn't warped: its positions are seconds in its file, so leave `ticks` false"
                }))
            }
            ClipData::Audio(_) => Ok(()),
            ClipData::Midi(_) => Err(ModelError::NotAudioClip(id)),
        }
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

    /// The two audio clips on `track` that meet at `boundary`: the one that
    /// ends there and the one that starts there.
    fn meeting_audio_clips(
        &self,
        track: TrackId,
        boundary: Ticks,
    ) -> Result<(ClipId, ClipId), ModelError> {
        let lane = self.track(track).ok_or(ModelError::NoSuchTrack(track))?;
        let audio_at = |ends: bool| {
            lane.clips().find(|clip| {
                clip.data.is_audio() && if ends { clip.end() } else { clip.start } == boundary
            })
        };
        match (audio_at(true), audio_at(false)) {
            (Some(earlier), Some(later)) if earlier.id != later.id => Ok((earlier.id, later.id)),
            _ => Err(ModelError::InvalidInput(
                "no two audio clips on the track meet at that position",
            )),
        }
    }

    /// Checks that the fades of `audio`, the data clip `id` would have, stay
    /// within the audio clips spec's R25 limits.
    fn check_fades_of(&self, id: ClipId, audio: &AudioData) -> Result<(), ModelError> {
        crate::fades::check_fades(self.clip_ref(id)?, audio, &self.settings.tempo)
    }

    /// Sets a clip's fades (R30), keeping a crossfade that ends or begins at
    /// the clip consistent on its other clip.
    fn set_clip_fades(
        &mut self,
        clip: ClipId,
        fade_in: f64,
        fade_out: f64,
        fade_in_curve: Option<f64>,
        fade_out_curve: Option<f64>,
    ) -> Result<(), ModelError> {
        let ClipData::Audio(current) = &self.clip_ref(clip)?.data else {
            return Err(ModelError::NotAudioClip(clip));
        };
        let mut next = current.clone();
        next.fade_in = fade_in;
        next.fade_out = fade_out;
        next.fade_in_curve = fade_in_curve.unwrap_or(next.fade_in_curve);
        next.fade_out_curve = fade_out_curve.unwrap_or(next.fade_out_curve);
        let track = self.clip_owner(clip)?;
        let existing = self
            .track(track)
            .map(|t| crossfades(t.clips()))
            .unwrap_or_default();
        let at_start = existing.iter().find(|c| c.later == clip);
        let at_end = existing.iter().find(|c| c.earlier == clip);
        // The crossfade at an edge keeps the clip's length there as its own
        // and the other clip follows. An edge without one is a plain fade,
        // whatever an earlier crossfade left.
        let mut partners = Vec::new();
        next.fade_in_before = at_start.map_or(0.0, |c| c.before);
        next.fade_out_after = at_end.map_or(0.0, |c| c.after);
        if let Some(c) = at_start {
            partners.push((c.earlier, true));
        }
        if let Some(c) = at_end {
            partners.push((c.later, false));
        }
        let mut others = Vec::with_capacity(partners.len());
        for (id, is_earlier) in partners {
            let ClipData::Audio(data) = &self.clip_ref(id)?.data else {
                continue;
            };
            let mut other = data.clone();
            if is_earlier {
                other.fade_out_after = next.fade_in;
                other.fade_out_curve = next.fade_in_curve;
            } else {
                other.fade_in_before = next.fade_out;
                other.fade_in_curve = next.fade_out_curve;
            }
            if check_audio(&other)
                .and_then(|()| self.check_fades_of(id, &other))
                .is_err()
            {
                // The other clip can't follow (its file doesn't reach back
                // that far, say): the crossfade ends here, and each clip keeps
                // its own fade.
                other = data.clone();
                if is_earlier {
                    other.fade_out_after = 0.0;
                    next.fade_in_before = 0.0;
                } else {
                    other.fade_in_before = 0.0;
                    next.fade_out_after = 0.0;
                }
            }
            others.push((id, other));
        }
        check_audio(&next)?;
        self.check_fades_of(clip, &next)?;
        self.edit_audio(clip, |audio| *audio = next)?;
        for (id, other) in others {
            self.edit_audio(id, |audio| *audio = other)?;
        }
        Ok(())
    }

    /// Gives the audio clips on `track` that meet at `boundary` a crossfade
    /// of the span and curve, or none if `before` and `after` are 0 (R30).
    fn set_crossfade(
        &mut self,
        track: TrackId,
        boundary: Ticks,
        before: f64,
        after: f64,
        curve: f64,
    ) -> Result<(), ModelError> {
        let (earlier, later) = self.meeting_audio_clips(track, boundary)?;
        let (ClipData::Audio(first), ClipData::Audio(second)) =
            (&self.clip_ref(earlier)?.data, &self.clip_ref(later)?.data)
        else {
            return Err(ModelError::InvalidInput(
                "a crossfade is between audio clips",
            ));
        };
        let (mut first, mut second) = (first.clone(), second.clone());
        first.crossfade_into(&mut second, before, after, curve);
        for (id, audio) in [(earlier, &first), (later, &second)] {
            check_audio(audio)?;
            self.check_fades_of(id, audio)?;
        }
        self.edit_audio(earlier, |audio| *audio = first)?;
        self.edit_audio(later, |audio| *audio = second)
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
        let mut next = current.clone();
        edit(&mut next);
        check_audio(&next)?;
        if next != *current {
            // Spec: warp-modes R31. Moving a clip's setting by hand turns off
            // the track's automation of it, as automation R5 says.
            let moved: Vec<ClipParam> = ClipParam::ALL
                .into_iter()
                .filter(|param| param.base(&next) != param.base(current))
                .collect();
            self.clip_mut(id)?.data = ClipData::Audio(next);
            if !moved.is_empty() {
                let track = self.track_mut(self.clip_owner(id)?)?;
                for param in moved {
                    track.override_target(&AutomationTarget::Clip { param });
                }
            }
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
    /// chain (WP-3.15), or a MIDI controller of a MIDI clip (WP-2.29, R1 and
    /// R6).
    fn check_clip_envelope_target(
        &self,
        clip: ClipId,
        target: AutomationTarget,
    ) -> Result<(), ModelError> {
        let track = self.clip_owner(clip)?;
        self.check_clip_target(track, self.clip_ref(clip)?, target)
    }

    /// Checks `target` for an envelope of `clip`, which is on `track`: a
    /// parameter of the track, or a MIDI controller (a number a clip holds,
    /// on a MIDI clip).
    pub fn check_clip_target(
        &self,
        track: TrackId,
        clip: &Clip,
        target: AutomationTarget,
    ) -> Result<(), ModelError> {
        if target.is_clip() && !clip.data.is_audio() {
            return Err(ModelError::NotAudioClip(clip.id));
        }
        if !target.is_midi() {
            return self.check_automation_target(track, target);
        }
        if clip.data.is_audio() {
            return Err(ModelError::NotMidiClip(clip.id));
        }
        if let AutomationTarget::MidiCc { number } = target {
            check_value(
                "MIDI controller number",
                f64::from(number),
                AutomationTarget::is_clip_controller(number),
            )?;
        }
        Ok(())
    }

    /// Refuses an automation envelope on a clip that sits in no Session
    /// slot: an Arrangement clip's envelopes only modulate, the automation
    /// being the track's (WP-3.12).
    fn check_envelope_kind(&self, clip: ClipId, kind: EnvelopeKind) -> Result<(), ModelError> {
        if kind == EnvelopeKind::Automation
            && !self
                .track_ref(self.clip_owner(clip)?)?
                .slots()
                .any(|(_, slotted)| slotted.id == clip)
        {
            return Err(ModelError::NotSlotClip(clip));
        }
        Ok(())
    }

    /// Sets the value of `target` at `at` on the clip's envelope for it,
    /// adding a breakpoint there or replacing the one that's there, and
    /// giving the clip that envelope, linked, if it has none.
    fn set_clip_envelope_breakpoint(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        kind: EnvelopeKind,
        at: Ticks,
        value: f64,
    ) -> Result<(), ModelError> {
        if at < Ticks::ZERO {
            return Err(ModelError::NegativePosition(at));
        }
        self.check_clip_envelope_target(clip, target)?;
        check_clip_envelope_value(target, value)?;
        // A MIDI controller's envelope only sends messages; it doesn't set a
        // parameter as a Session clip's automation does (WP-2.29).
        if kind == EnvelopeKind::Automation && target.is_midi() {
            return Err(ModelError::MidiControllerNotAParameter);
        }
        check_clip_setting_kind(target, kind)?;
        self.check_envelope_kind(clip, kind)?;
        clip_entry(&mut self.clip_mut(clip)?.envelopes, target, kind)
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
        kind: EnvelopeKind,
        at: Ticks,
        bend: f64,
    ) -> Result<(), ModelError> {
        check_bend(bend)?;
        self.check_clip_envelope_target(clip, target)?;
        let envelope =
            self.clip_ref(clip)?
                .envelope_of(&target, kind)
                .ok_or(ModelError::NoChange {
                    what: "clip envelope",
                    at,
                })?;
        let mut envelope = envelope.envelope.clone();
        envelope
            .set_bend(at, bend)
            .map_err(automation_point_error)?;
        self.clip_envelope_mut(clip, target, kind)?.envelope = envelope;
        Ok(())
    }

    /// Moves the breakpoint at `at` on the clip's envelope for `target` to
    /// `to`, keeping its value unless `value` gives it another.
    fn move_clip_envelope_breakpoint(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        kind: EnvelopeKind,
        at: Ticks,
        to: Ticks,
        value: Option<f64>,
    ) -> Result<(), ModelError> {
        if to < Ticks::ZERO {
            return Err(ModelError::NegativePosition(to));
        }
        self.check_clip_envelope_target(clip, target)?;
        if let Some(value) = value {
            check_clip_envelope_value(target, value)?;
        }
        let envelope =
            self.clip_ref(clip)?
                .envelope_of(&target, kind)
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
        self.clip_envelope_mut(clip, target, kind)?.envelope = envelope;
        Ok(())
    }

    /// Deletes the breakpoint at `at` on the clip's envelope for `target`.
    /// Deleting the last one takes the envelope with it.
    fn delete_clip_envelope_breakpoint(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        kind: EnvelopeKind,
        at: Ticks,
    ) -> Result<(), ModelError> {
        self.check_clip_envelope_target(clip, target)?;
        let envelope =
            self.clip_ref(clip)?
                .envelope_of(&target, kind)
                .ok_or(ModelError::NoChange {
                    what: "clip envelope",
                    at,
                })?;
        let mut envelope = envelope.envelope.clone();
        envelope.delete(at).map_err(automation_point_error)?;
        if envelope.is_empty() {
            self.delete_clip_envelope(clip, target, kind)
        } else {
            self.clip_envelope_mut(clip, target, kind)?.envelope = envelope;
            Ok(())
        }
    }

    /// Deletes the clip's envelope for `target`, as Clear Envelope does.
    /// Nothing changes if the clip has no envelope for it.
    fn delete_clip_envelope(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        kind: EnvelopeKind,
    ) -> Result<(), ModelError> {
        self.check_clip_envelope_target(clip, target)?;
        self.clip_mut(clip)?
            .envelopes
            .retain(|envelope| envelope.target != target || envelope.kind != kind);
        Ok(())
    }

    /// Switches the clip's envelope for `target` between linked and
    /// unlinked, giving the clip an empty linked one if it has none.
    fn set_clip_envelope_linked(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        kind: EnvelopeKind,
        linked: bool,
    ) -> Result<(), ModelError> {
        self.check_clip_envelope_target(clip, target)?;
        check_clip_setting_kind(target, kind)?;
        self.check_envelope_kind(clip, kind)?;
        clip_entry(&mut self.clip_mut(clip)?.envelopes, target, kind).linked = linked;
        Ok(())
    }

    /// Replaces the breakpoints of the clip's envelope for `target`, giving
    /// the clip that envelope, linked, if it has none. An empty list takes
    /// the envelope with it.
    fn set_clip_envelope(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        kind: EnvelopeKind,
        points: &[Breakpoint],
    ) -> Result<(), ModelError> {
        self.check_clip_envelope_target(clip, target)?;
        check_clip_setting_kind(target, kind)?;
        self.check_envelope_kind(clip, kind)?;
        for point in points {
            check_clip_envelope_value(target, point.value)?;
        }
        let envelope = Envelope::from_points(points.to_vec())?;
        if envelope.is_empty() {
            return self.delete_clip_envelope(clip, target, kind);
        }
        clip_entry(&mut self.clip_mut(clip)?.envelopes, target, kind).envelope = envelope;
        Ok(())
    }

    /// The clip's envelope for `target`, which the command has checked
    /// exists.
    fn clip_envelope_mut(
        &mut self,
        clip: ClipId,
        target: AutomationTarget,
        kind: EnvelopeKind,
    ) -> Result<&mut ClipEnvelope, ModelError> {
        self.clip_mut(clip)?
            .envelopes
            .iter_mut()
            .find(|envelope| envelope.target == target && envelope.kind == kind)
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
        // Spec: articulation-map R2. A new Articulation Map holds one
        // articulation, named "Articulation 1".
        let mut device = device.clone();
        if device.is_articulation_map() && device.articulations.is_empty() {
            device.set_articulations(vec![crate::articulation::Articulation::new(
                crate::articulation::FIRST_NAME,
            )]);
        }
        let device = &device;
        check_device(device)?;
        let chain = &mut self.track_mut(track)?.devices;
        insert_at(chain, index, Shared::new(device.clone()))?;
        self.index.add_device(device, track);
        check_chain(self.track_ref(track)?)?;
        // Spec: modulation R19. A mapping to a parameter the set lacks goes.
        self.prune_mappings();
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

    /// Places `device` where its new `role` goes in the chain it sits in
    /// (IMP-24): after the devices of the same role or a smaller one,
    /// before those of a greater one. The chain keeps its order (R3), so a
    /// role that can't fit the track or its chain is refused.
    fn set_device_role(
        &mut self,
        device: DeviceId,
        role: Option<DeviceRole>,
    ) -> Result<(), ModelError> {
        let current = self.device_ref(device)?.role;
        // Left out, the other position is taken: back where the type goes
        // from the MIDI-effect position, and into it from anywhere else.
        let role = role.unwrap_or(if current == DeviceRole::MidiEffect {
            DeviceRole::AudioEffect
        } else {
            DeviceRole::MidiEffect
        });
        if current == role {
            return Ok(());
        }
        // Spec: device-chains R2 (IMP-24). A native device keeps the role
        // its type gives it; only a plug-in takes another position.
        if matches!(self.device_ref(device)?.kind, DeviceKind::Native { .. }) {
            return Err(ModelError::InvalidInput(
                "a native device keeps the role its type gives it",
            ));
        }
        let place = self
            .device_place(device)
            .ok_or(ModelError::NoSuchDevice(device))?;
        let mut taken = self.take_device(device)?;
        taken.role = role;
        let index = match place.chain {
            None => self
                .track_ref(place.track)?
                .devices
                .iter()
                .take_while(|d| d.role <= role)
                .count(),
            Some(chain) => self
                .chain_ref(chain)?
                .devices
                .iter()
                .take_while(|d| d.role <= role)
                .count(),
        };
        self.put_device(place.track, place.chain, index, taken)?;
        check_chain(self.track_ref(place.track)?)
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
        // Only a drum rack's chains keep their notes, sends and outputs.
        chain.fit_to(self.device_ref(rack)?);
        if let Some(track) = chain.audio_to {
            check_kind(self.track_ref(track)?, |k| k == TrackKind::Return, RETURN)?;
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
        let holder = self.rack_holding_mut(id)?;
        let holder_id = holder.id;
        let mut moved = holder.remove_chain(id).ok_or(ModelError::NoSuchChain(id))?;
        // A chain keeps its notes and sends only in a drum rack, and a
        // return chain only among the return chains.
        moved.fit_to(self.device_ref(rack)?);
        let chains = &mut self.device_mut(rack)?.chains;
        check_index(index, chains.len())?;
        chains.insert(index, moved.clone());
        self.device_mut(holder_id)?.prune_sends();
        self.device_mut(rack)?.prune_sends();
        // Only instrument and audio effect racks' chains are tapped (the
        // racks spec, R38).
        if !matches!(
            (self.device_ref(rack)?.rack_kind(), moved.is_return),
            (Some(RackKind::Instrument | RackKind::AudioEffect), _) | (Some(RackKind::Drum), true)
        ) {
            self.forget_chain_taps(&BTreeSet::from([id]));
        }
        if from != to {
            self.index.add_chain(&moved, to);
            self.forget_in(from, &Gone::chain(&moved))?;
        } else if holder_id != rack {
            // Its sends, and those to it, stayed in the rack it left.
            forget_where(
                self.track_mut(from)?,
                |target| matches!(*target, AutomationTarget::ChainSend { chain, to } if chain == id || to == id),
            );
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
            self.track_mut(owner)?
                .override_target(&AutomationTarget::Chain { chain, param });
        }
        Ok(())
    }

    /// Edits a drum rack's chain with `edit`: a drum chain, or with
    /// `returns` a return chain. Leaves the set as it was if the rack
    /// wouldn't hold the result.
    fn edit_drum_chain(
        &mut self,
        chain: ChainId,
        returns: bool,
        edit: impl FnOnce(&mut Chain),
    ) -> Result<(), ModelError> {
        let rack = self
            .chain_rack(chain)
            .ok_or(ModelError::NoSuchChain(chain))?;
        let current = self.chain_ref(chain)?;
        if rack.rack_kind() != Some(RackKind::Drum) || current.is_return != returns {
            return Err(ModelError::NotADrumChain(chain));
        }
        let mut edited = current.clone();
        edit(&mut edited);
        if edited != *current {
            *self.chain_mut(chain)? = edited;
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
            AutomationTarget::Mixer { param } => {
                let track_ref = self.track_ref(track)?;
                // Main has no Track Activator, as SetMute has it.
                if param == MixerParam::Mute {
                    check_kind(track_ref, TrackKind::has_mute_and_solo, NOT_MAIN)?;
                }
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
                if matches!(param, MixerParam::SplitLeft | MixerParam::SplitRight) {
                    return Err(ModelError::ChainHasNoSplitPan(chain));
                }
            }
            AutomationTarget::ChainSend { chain, to } => {
                let track_ref = self.track_ref(track)?;
                let rack = track_ref
                    .devices
                    .iter()
                    .find_map(|d| d.chain_rack(chain))
                    .ok_or(ModelError::ChainNotInTrack { chain, track })?;
                let sends = rack.rack_kind() == Some(RackKind::Drum)
                    && rack.chains.iter().any(|c| c.id == chain && !c.is_return)
                    && rack.chains.iter().any(|c| c.id == to && c.is_return);
                if !sends {
                    return Err(ModelError::NotADrumChain(chain));
                }
            }
            // Spec: warp-modes R28, R31
            AutomationTarget::Clip { .. } => {
                check_kind(
                    self.track_ref(track)?,
                    |kind| kind == TrackKind::Audio,
                    AUDIO,
                )?;
            }
            AutomationTarget::MidiCc { .. }
            | AutomationTarget::MidiPitchBend
            | AutomationTarget::MidiChannelPressure => {
                return Err(ModelError::MidiControllerNotAParameter);
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
        // Spec: warp-modes R28. A clip setting's value is inside its range.
        let in_range = match target {
            AutomationTarget::Clip { param } => {
                let descriptor = param.descriptor();
                (descriptor.min()..=descriptor.max()).contains(&value)
            }
            _ => value.is_finite(),
        };
        check_value("automation value", value, in_range)?;
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

    /// Adds a locator at its position, naming it with the next whole number
    /// when it has no name of its own.
    fn add_locator(&mut self, mut locator: Locator) -> Result<(), ModelError> {
        if locator.position < Ticks::ZERO {
            return Err(ModelError::NegativePosition(locator.position));
        }
        self.check_fresh(locator.id.0)?;
        if self
            .settings
            .locators
            .iter()
            .any(|other| other.position == locator.position)
        {
            return Err(ModelError::ChangeInUse {
                what: "locator",
                at: locator.position,
            });
        }
        if locator.name.is_empty() {
            locator.name = next_locator_name(&self.settings.locators);
        }
        self.settings.locators.push(locator);
        self.settings
            .locators
            .sort_by_key(|locator| locator.position);
        Ok(())
    }

    /// Deletes a locator. A song start time on it goes with it.
    fn delete_locator(&mut self, id: LocatorId) -> Result<(), ModelError> {
        let position = self
            .settings
            .locators
            .iter()
            .position(|locator| locator.id == id)
            .ok_or(ModelError::NoSuchLocator(id))?;
        self.settings.locators.remove(position);
        if self.settings.song_start == Some(SongStart::Locator(id)) {
            self.settings.song_start = None;
        }
        Ok(())
    }

    /// Moves a locator to `to`.
    fn move_locator(&mut self, id: LocatorId, to: Ticks) -> Result<(), ModelError> {
        if to < Ticks::ZERO {
            return Err(ModelError::NegativePosition(to));
        }
        if self.locator(id).is_none() {
            return Err(ModelError::NoSuchLocator(id));
        }
        if self
            .settings
            .locators
            .iter()
            .any(|other| other.id != id && other.position == to)
        {
            return Err(ModelError::ChangeInUse {
                what: "locator",
                at: to,
            });
        }
        let locator = self
            .settings
            .locators
            .iter_mut()
            .find(|locator| locator.id == id)
            .expect("just found the locator");
        locator.position = to;
        self.settings
            .locators
            .sort_by_key(|locator| locator.position);
        Ok(())
    }

    /// Renames a locator. An empty name keeps the old one (the time editing
    /// spec, R8).
    fn rename_locator(&mut self, id: LocatorId, name: String) -> Result<(), ModelError> {
        let locator = self
            .settings
            .locators
            .iter_mut()
            .find(|locator| locator.id == id)
            .ok_or(ModelError::NoSuchLocator(id))?;
        if !name.is_empty() {
            locator.name = name;
        }
        Ok(())
    }

    /// Sets a locator's info text.
    fn set_locator_info_text(
        &mut self,
        id: LocatorId,
        info_text: String,
    ) -> Result<(), ModelError> {
        let locator = self
            .settings
            .locators
            .iter_mut()
            .find(|locator| locator.id == id)
            .ok_or(ModelError::NoSuchLocator(id))?;
        locator.info_text = info_text;
        Ok(())
    }

    /// Sets the loop brace from a locator to the next one and switches the
    /// loop on.
    fn loop_to_next_locator(&mut self, id: LocatorId) -> Result<(), ModelError> {
        let locator = self.locator(id).ok_or(ModelError::NoSuchLocator(id))?;
        let start = locator.position;
        let next = self
            .settings
            .locators
            .iter()
            .map(|locator| locator.position)
            .filter(|position| *position > start)
            .min()
            .ok_or(ModelError::NoChange {
                what: "next locator",
                at: start,
            })?;
        let brace = &mut self.settings.arrangement_loop;
        brace.start = start;
        brace.length = next - start;
        brace.enabled = true;
        Ok(())
    }

    /// The groove with this ID, to change.
    fn groove_mut(&mut self, id: GrooveId) -> Result<&mut Groove, ModelError> {
        self.settings
            .grooves
            .pool
            .iter_mut()
            .find(|groove| groove.id == id)
            .ok_or(ModelError::NoSuchGroove(id))
    }

    /// Whether the groove a new clip names is in the pool (R3).
    fn check_clip_groove(&self, clip: &Clip) -> Result<(), ModelError> {
        match clip.groove {
            Some(groove) if self.settings.grooves.get(groove).is_none() => {
                Err(ModelError::NoSuchGroove(groove))
            }
            _ => Ok(()),
        }
    }

    /// Adds a groove to the end of the pool (the grooves spec, R1).
    fn add_groove(&mut self, groove: Groove) -> Result<(), ModelError> {
        self.check_fresh(groove.id.0)?;
        groove.check()?;
        self.settings.grooves.pool.push(groove);
        Ok(())
    }

    /// Takes a groove out of the pool and everything that used it (R5, R20).
    fn delete_groove(&mut self, id: GrooveId) -> Result<(), ModelError> {
        self.groove_mut(id)?;
        self.settings.grooves.pool.retain(|groove| groove.id != id);
        if self.settings.grooves.auto_load_groove == Some(id) {
            self.settings.grooves.auto_load_groove = None;
        }
        self.forget_grooves(|used| used == id);
        Ok(())
    }

    /// Sets a groove's parameters: Base, then Quantize, Timing, Random and
    /// Velocity. A value out of range refuses them all (R2, R5).
    fn set_groove(
        &mut self,
        id: GrooveId,
        base: Option<GrooveBase>,
        [quantize, timing, random, velocity]: [Option<f64>; 4],
    ) -> Result<(), ModelError> {
        let mut next = self.groove_mut(id)?.clone();
        next.base = base.unwrap_or(next.base);
        next.quantize = quantize.unwrap_or(next.quantize);
        next.timing = timing.unwrap_or(next.timing);
        next.random = random.unwrap_or(next.random);
        next.velocity = velocity.unwrap_or(next.velocity);
        next.check()?;
        *self.groove_mut(id)? = next;
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

    /// The set's tuning, for a command that changes it.
    fn held_tuning(&self) -> Result<&Tuning, ModelError> {
        self.settings
            .tuning
            .as_ref()
            .ok_or(ModelError::InvalidInput("the set has no tuning"))
    }

    /// Moves every note of every MIDI clip on a track that plays the tuning
    /// from the key it had under `before` (None: twelve equal steps) to the
    /// key nearest its pitch under the set's tuning now (the tunings spec,
    /// R15).
    fn retune_notes(&mut self, before: Option<&Tuning>) {
        let from = crate::tuning::key_pitches(before);
        let to = crate::tuning::key_pitches(self.settings.tuning.as_ref());
        let tracks: Vec<TrackId> = self
            .tracks()
            .filter(|track| track.plays_tuning())
            .map(Track::id)
            .collect();
        for id in tracks {
            let Some(track) = self.tracks.get_mut(&id).map(Shared::make_mut) else {
                continue;
            };
            let clips: Vec<ClipId> = track
                .clips
                .keys()
                .copied()
                .chain(track.slots.values().map(|clip| clip.id))
                .collect();
            for clip in clips {
                if let Some(ClipData::Midi(midi)) = track.clip_mut(clip).map(|clip| &mut clip.data)
                {
                    crate::tuning::retune(&mut midi.notes, &from, &to);
                }
            }
        }
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
        let track = self.track_ref(self.clip_owner(id)?)?;
        Ok(track.clips.contains_key(&id) || track.clip_lane(id).is_some())
    }

    fn clip_ref(&self, id: ClipId) -> Result<&Clip, ModelError> {
        self.clip(id).ok_or(ModelError::NoSuchClip(id))
    }

    fn clip_mut(&mut self, id: ClipId) -> Result<&mut Clip, ModelError> {
        let owner = self.clip_owner(id)?;
        self.track_mut(owner)?
            .clip_mut(id)
            .ok_or(ModelError::NoSuchClip(id))
    }

    fn colour_track(&mut self, id: TrackId, colour: u8) -> Result<(), ModelError> {
        if self.track_ref(id)?.colour != colour {
            self.track_mut(id)?.colour = colour;
        }
        Ok(())
    }

    /// Colours every clip a track holds: its Arrangement lane, its Session
    /// slots and its take lanes.
    fn colour_clips_of(&mut self, id: TrackId, colour: u8) -> Result<(), ModelError> {
        let clips: Vec<ClipId> = self
            .track_ref(id)?
            .all_clips()
            .filter(|clip| clip.colour != Some(colour))
            .map(|clip| clip.id)
            .collect();
        for clip in clips {
            self.clip_mut(clip)?.colour = Some(colour);
        }
        Ok(())
    }

    /// Forgets what a track held for the devices and chains that left it,
    /// and the inputs that tapped those chains.
    fn forget_in(&mut self, track: TrackId, gone: &Gone) -> Result<(), ModelError> {
        forget(self.track_mut(track)?, gone);
        self.forget_chain_taps(&gone.chains);
        self.forget_device_outputs(&gone.devices);
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

    pub(crate) fn device_ref(&self, id: DeviceId) -> Result<&Device, ModelError> {
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

    /// Sets the name of the preset a device holds (the plug-in list spec,
    /// R5), outside the command system:
    /// [`Editor::amend_device_preset_name`](crate::Editor::amend_device_preset_name)
    /// calls it.
    pub(crate) fn set_device_preset_name(
        &mut self,
        id: DeviceId,
        name: Option<String>,
    ) -> Result<(), ModelError> {
        if self.device_ref(id)?.preset_name != name {
            self.device_mut(id)?.preset_name = name;
        }
        Ok(())
    }

    /// Takes every macro mapping whose target its rack no longer holds out
    /// of that rack (macros spec, R18): a device or chain that left it takes
    /// its mappings with it, in the same undo step.
    pub(crate) fn prune_macro_mappings(&mut self) {
        let racks: Vec<DeviceId> = self
            .tracks()
            .flat_map(|t| t.devices_deep())
            .filter(|d| d.rack_kind().is_some())
            .map(|d| d.id)
            .collect();
        for rack_id in racks {
            let (kept, total) = {
                let Some(rack) = self.device(rack_id) else {
                    continue;
                };
                let kept: Vec<crate::MacroMapping> = rack
                    .macros
                    .mappings
                    .iter()
                    .filter(|mapping| match mapping.target {
                        crate::MacroTarget::Device { device, param } => {
                            !(device == rack_id && crate::macro_number(param).is_some())
                                && rack.find(device).is_some()
                        }
                        crate::MacroTarget::Chain { chain, .. } => rack.find_chain(chain).is_some(),
                    })
                    .cloned()
                    .collect();
                (kept, rack.macros.mappings.len())
            };
            if kept.len() != total
                && let Ok(device) = self.device_mut(rack_id)
            {
                device.macros.mappings = kept;
            }
        }
    }

    pub(crate) fn device_mut(&mut self, id: DeviceId) -> Result<&mut Device, ModelError> {
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

    /// The list a track is in now, to look at it.
    fn containing_list(
        &self,
        kind: TrackKind,
        parent: Option<TrackId>,
    ) -> Result<&Vector<TrackId>, ModelError> {
        match kind {
            TrackKind::Main => Err(ModelError::MainTrack),
            TrackKind::Return => Ok(&self.returns),
            _ => match parent {
                None => Ok(&self.top),
                Some(parent) => Ok(&self.track_ref(parent)?.children),
            },
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
const AUDIO: &str = "an audio track";
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

/// The items of a list that names each object once, or the duplicated ID's
/// error.
fn each_once<T: Copy>(
    items: &[T],
    raw: impl Fn(T) -> Id,
) -> Result<impl Iterator<Item = T> + '_, ModelError> {
    let mut seen = BTreeSet::new();
    for &item in items {
        if !seen.insert(raw(item)) {
            return Err(ModelError::DuplicateId(raw(item)));
        }
    }
    Ok(items.iter().copied())
}

pub(crate) fn check_value(what: &'static str, value: f64, ok: bool) -> Result<(), ModelError> {
    if ok {
        Ok(())
    } else {
        Err(ModelError::InvalidValue { what, value })
    }
}

/// Checks that `number` is one of a rack's macros, 1 to 16.
fn check_macro_number(number: u8) -> Result<(), ModelError> {
    if (1..=MACROS).contains(&number) {
        Ok(())
    } else {
        Err(ModelError::InvalidInput(
            "a rack's macros are numbered 1 to 16",
        ))
    }
}

/// The scale a command's arguments give over `scale`: the values it gives
/// replace the ones it has, and the ones it doesn't give stay as they were
/// (the scales spec, R5). A root outside C to B, or a name the scale list
/// doesn't hold, is refused.
fn scaled(
    scale: Scale,
    enabled: Option<bool>,
    root: Option<u8>,
    name: Option<&str>,
) -> Result<Scale, ModelError> {
    let root = match root {
        Some(root) if root > 11 => {
            return Err(ModelError::InvalidValue {
                what: "root note",
                value: f64::from(root),
            });
        }
        Some(root) => root,
        None => scale.root,
    };
    let name = match name {
        Some(name) => ScaleName::parse(name).ok_or(ModelError::InvalidInput(
            "the scale's name is not one of the scale list's names",
        ))?,
        None => scale.name,
    };
    Ok(Scale {
        enabled: enabled.unwrap_or(scale.enabled),
        root,
        name,
    })
}

/// The next number from a SplitMix64 generator, from 0 (inclusive) to 1
/// (exclusive): Randomize's values, the same for the same seed (macros spec,
/// R15).
fn next_unit(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Checks the value of a clip envelope's breakpoint: a number, and for a MIDI
/// controller inside the controller's range (the MIDI controller data spec,
/// R1 and R6).
pub(crate) fn check_clip_envelope_value(
    target: AutomationTarget,
    value: f64,
) -> Result<(), ModelError> {
    let in_range = target
        .midi_range()
        .or_else(|| target.clip_range())
        .map_or(value.is_finite(), |(min, max)| (min..=max).contains(&value));
    check_value("clip envelope value", value, in_range)
}

/// Refuses an Automation envelope for a clip sound setting: its envelopes
/// modulate, and the track's automation drives the setting (the warp modes
/// spec, R29 and R31).
pub(crate) fn check_clip_setting_kind(
    target: AutomationTarget,
    kind: EnvelopeKind,
) -> Result<(), ModelError> {
    if kind == EnvelopeKind::Automation && target.is_clip() {
        return Err(ModelError::InvalidInput(
            "a clip setting's envelope only modulates; the track's automation drives the setting",
        ));
    }
    Ok(())
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

/// Builds a tempo map from breakpoints this module made, which don't move
/// backwards, so the map is valid. Unlike `TempoMap::from_breakpoints` it
/// keeps a breakpoint on a flat stretch, so one set at the tempo already in
/// force stays where it was put.
fn tempo_from_breakpoints(mut points: Vec<(Ticks, Bpm)>) -> TempoMap {
    // The first breakpoint's tempo holds before it.
    if let Some(&(at, bpm)) = points.first()
        && at > Ticks::ZERO
    {
        points.insert(0, (Ticks::ZERO, bpm));
    }
    let mut segments: Vec<TempoSegment> = points
        .windows(2)
        .filter(|pair| pair[0].0 != pair[1].0)
        .map(|pair| {
            let ((start, from), (_, to)) = (pair[0], pair[1]);
            if from == to {
                TempoSegment::Constant { start, bpm: from }
            } else {
                TempoSegment::Ramp { start, from, to }
            }
        })
        .collect();
    if let Some(&(start, bpm)) = points.last() {
        segments.push(TempoSegment::Constant { start, bpm });
    }
    TempoMap::new(segments).expect("a tempo map edited this way stays valid")
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
        // A frozen MIDI track's clips play their freeze files (R6).
        (TrackKind::Midi, ClipData::Audio(audio))
            if track
                .frozen
                .as_ref()
                .is_some_and(|frozen| frozen.clips.iter().any(|r| r.media == audio.media)) =>
        {
            Ok(())
        }
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
/// A position in a clip's contents as whole ticks, which is the form a MIDI
/// clip's contents and a warped audio clip's take.
fn whole_tick(value: f64) -> Result<Ticks, ModelError> {
    if !value.is_finite() || value < 0.0 || value > i64::MAX as f64 {
        return Err(ModelError::InvalidValue {
            what: "crop position",
            value,
        });
    }
    Ok(Ticks(value.round() as i64))
}

pub(crate) fn check_clip_data(data: &ClipData) -> Result<(), ModelError> {
    match data {
        ClipData::Midi(midi) => {
            for (index, note) in midi.notes.iter().enumerate() {
                if note.key > 127
                    || !(1..=127).contains(&note.velocity)
                    || !(1..=127).contains(&note.release_velocity)
                    || !note.probability.is_finite()
                    || !(0.0..=1.0).contains(&note.probability)
                    || !note.expressions_fit()
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
    if !midi.groups_hold() {
        return Err(ModelError::BrokenGroups);
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
    if audio.warp {
        // A warped clip's markers are whole ticks of its contents, which may
        // be negative (the warping spec, R2 and R4).
        let tick = |value: f64| value.is_finite() && value.fract() == 0.0;
        check_value("clip start", audio.start, tick(audio.start))?;
        if let Some(end) = audio.end {
            check_value("clip end", end, tick(end))?;
        }
        check_value("loop start", audio.loop_start, tick(audio.loop_start))?;
        if let Some(end) = audio.loop_end {
            check_value("loop end", end, tick(end) && end > audio.loop_start)?;
        }
    } else {
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
    }
    if audio.warp || !audio.warp_markers.is_empty() {
        audio.warp_map()?;
    }
    audio.warp_settings.check()?;
    check_value(
        "Transpose",
        f64::from(audio.transpose),
        (TRANSPOSE_MIN..=TRANSPOSE_MAX).contains(&audio.transpose),
    )?;
    check_value(
        "Detune",
        audio.detune,
        audio.detune.is_finite() && (DETUNE_MIN..=DETUNE_MAX).contains(&audio.detune),
    )?;
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
    )?;
    check_value(
        "fade-in start before the clip",
        audio.fade_in_before,
        at_least_zero(audio.fade_in_before),
    )?;
    check_value(
        "fade-out end after the clip",
        audio.fade_out_after,
        at_least_zero(audio.fade_out_after),
    )?;
    let curve = |value: f64| value.is_finite() && (-1.0..=1.0).contains(&value);
    check_value(
        "fade-in curve",
        audio.fade_in_curve,
        curve(audio.fade_in_curve),
    )?;
    check_value(
        "fade-out curve",
        audio.fade_out_curve,
        curve(audio.fade_out_curve),
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
    if let Some(colour) = clip.colour {
        crate::colour::check_swatch("clip colour", colour)?;
    }
    check_offset(clip.offset)?;
    if clip.data.is_audio() && clip.offset != Ticks::ZERO {
        return Err(ModelError::AudioClipOffset(clip.offset));
    }
    check_velocity_amount(clip.launch.velocity_amount)?;
    check_clip_data(&clip.data)
}

/// Checks a clip's velocity amount: from 0 to 1.
fn check_velocity_amount(amount: f64) -> Result<(), ModelError> {
    if (0.0..=1.0).contains(&amount) {
        Ok(())
    } else {
        Err(ModelError::InvalidInput(
            "the velocity amount is from 0 to 1",
        ))
    }
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
    for value in device.params.values().chain(device.other_params().values()) {
        check_value("parameter value", *value, value.is_finite())?;
    }
    device.layers.check()?;
    // Spec: mpe R18. Settings out of range don't load.
    device.mpe.settings.check()?;
    crate::modulator::check_mappings(device)?;
    if let Some(shape) = &device.shape {
        crate::modulator::check_shape(shape)?;
    }
    // Spec: articulation-map R20. A saved device whose list is empty or
    // holds more than 32 articulations, whose triggers break R3, or whose
    // channel or values fall outside R3 to R5's ranges doesn't load.
    if device.is_articulation_map() {
        crate::articulation::check(&device.articulations).map_err(ModelError::InvalidInput)?;
    }
    if !device.can_compare() && (device.b_active || device.other_params.is_some()) {
        return Err(ModelError::NoCompare(device.id));
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
    if let Some(bpm) = device.warp.bpm {
        check_value("sample bpm", bpm, (1.0..=1000.0).contains(&bpm))?;
    }
    device.warp.settings.check()?;
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
    check_value(
        "split pan",
        mixer.split_left,
        (-1.0..=1.0).contains(&mixer.split_left),
    )?;
    check_value(
        "split pan",
        mixer.split_right,
        (-1.0..=1.0).contains(&mixer.split_right),
    )?;
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

mod time_edit;
pub use time_edit::{tempo_shape, write_tempo_shape};

#[cfg(test)]
mod tests {
    use bl_core::id::IdAllocator;
    use bl_core::time::{Bpm, TempoMap, TimeSignature};

    use super::*;
    use crate::automation::Breakpoint;
    use crate::changes::ChangeSet;
    use crate::clip::{EDGE_FADE_SECONDS as EDGE_FADE, MidiData, Note, TakeClip, TakeClipData};
    use crate::device::{DeviceKind, DeviceRole, PluginFormat};
    use crate::follow::FollowAction;
    use crate::ids::ContentHash;
    use crate::scene::DEFAULT_SCENE_COUNT;
    use crate::track::{DeviceOutput, Routing};
    use crate::warp::WarpMarker;

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

    fn track_input(track: TrackId, chain: Option<ChainId>, output: Option<DeviceOutput>) -> Input {
        Input::Track {
            track,
            tap: crate::track::Tap::default(),
            chain,
            output,
        }
    }

    // Spec: freeze-and-bounce R21. The bounced track forgets the routes that
    // needed a MIDI track or its devices, and keeps the audio ones.
    #[test]
    fn bouncing_a_midi_track_forgets_the_routes_to_its_devices_and_to_it_as_midi() {
        let mut song = Song::new();
        let source = song.track(TrackKind::Midi, None);
        let rack_id: DeviceId = song.fresh();
        let chain: ChainId = song.fresh();
        let mut rack = Device::rack(rack_id, crate::rack::RackKind::Instrument)
            .with_chain(crate::rack::Chain::new(chain, "Chain"));
        rack.role = DeviceRole::Instrument;
        song.apply(Command::InsertDevice {
            track: source,
            index: 0,
            device: rack,
        })
        .unwrap();
        let layered = song.track(TrackKind::Midi, None);
        song.apply(Command::SetInput {
            track: layered,
            input: track_input(source, None, None),
            channel: None,
        })
        .unwrap();
        let forwarding = song.track(TrackKind::Midi, None);
        song.apply(Command::SetMidiOutput {
            track: forwarding,
            to: Some(MidiTo::Track(source)),
        })
        .unwrap();
        let listener = DeviceId(song.ids.allocate());
        song.apply(Command::InsertDevice {
            track: forwarding,
            index: 0,
            device: utility(listener).with_midi_sidechain(),
        })
        .unwrap();
        song.apply(Command::SetMidiSidechainSource {
            device: listener,
            track: Some(source),
        })
        .unwrap();
        let tapping = song.track(TrackKind::Audio, None);
        song.apply(Command::SetInput {
            track: tapping,
            input: track_input(source, Some(chain), None),
            channel: None,
        })
        .unwrap();
        let taking_output = song.track(TrackKind::Audio, None);
        let output = DeviceOutput {
            device: rack_id,
            output: 0,
        };
        song.apply(Command::SetInput {
            track: taking_output,
            input: track_input(source, None, Some(output)),
            channel: None,
        })
        .unwrap();
        let hearing = song.track(TrackKind::Audio, None);
        song.apply(Command::SetInput {
            track: hearing,
            input: track_input(source, None, None),
            channel: None,
        })
        .unwrap();

        let bounced = crate::bounce::BouncedTrack {
            clips: vec![Clip::new(song.fresh(), Ticks::ZERO, Ticks::WHOLE, audio())],
            ..Default::default()
        };
        song.apply(Command::BounceTrackInPlace {
            track: source,
            bounced,
        })
        .unwrap();

        let doc = &song.doc;
        Document::try_from(crate::repr::DocumentRepr::from(doc)).unwrap();
        assert_eq!(doc.track(source).unwrap().kind(), TrackKind::Audio);
        assert!(doc.device(rack_id).is_none());
        assert!(doc.device_ids().all(|device| device != rack_id));
        assert_eq!(doc.device_count(), 1, "only the listener is left");
        let routing = |track| doc.track(track).unwrap().routing().clone();
        assert_eq!(routing(layered).input, Input::None);
        assert_eq!(routing(forwarding).midi_output, None);
        assert_eq!(
            doc.device(listener)
                .unwrap()
                .midi_sidechain
                .as_ref()
                .unwrap()
                .source,
            None
        );
        assert_eq!(routing(tapping).input, Input::None);
        assert_eq!(routing(taking_output).input, Input::None);
        assert_eq!(
            routing(hearing).input,
            track_input(source, None, None),
            "an audio route to the track stays"
        );
    }

    fn frozen_region(clip: &Clip, scene: Option<SceneId>) -> crate::freeze::FrozenClip {
        crate::freeze::FrozenClip {
            clip: clip.id,
            media: ContentHash([9; 32]),
            start: 0.0,
            end: 1.0,
            tempo: 120.0,
            markers: Vec::new(),
            length: clip.length,
            original: clip.data.clone(),
            scene,
            cycle: None,
        }
    }

    /// A song with a frozen MIDI track holding a lane clip and a slot clip.
    fn frozen_midi_song() -> (Song, TrackId, crate::freeze::FrozenTrack) {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let lane = song.clip(track, 0);
        let scene = song.doc.scenes().next().unwrap().id;
        let slot = Clip::new(song.fresh(), Ticks::ZERO, Ticks::WHOLE, midi());
        song.apply(Command::CreateSlotClip {
            track,
            scene,
            clip: slot.clone(),
        })
        .unwrap();
        let regions = vec![
            frozen_region(song.doc.clip(lane).unwrap(), None),
            frozen_region(&slot, Some(scene)),
        ];
        let frozen = crate::freeze::FrozenTrack {
            clips: regions,
            ..Default::default()
        };
        song.apply(Command::FreezeTracks {
            frozen: vec![(track, frozen.clone())],
        })
        .unwrap();
        (song, track, frozen)
    }

    // Spec: freeze-and-bounce R1, R6. A frozen MIDI track holds its freeze
    // audio in its clips, and its saved set loads.
    #[test]
    fn a_frozen_midi_track_saves_and_loads_and_cannot_be_frozen_again() {
        let (mut song, track, frozen) = frozen_midi_song();

        let saved = serde_json::to_string(&crate::repr::DocumentRepr::from(&song.doc)).unwrap();
        let loaded: crate::repr::DocumentRepr = serde_json::from_str(&saved).unwrap();
        assert_eq!(Document::try_from(loaded).unwrap(), song.doc);
        song.doc.check_invariants().unwrap();
        assert!(
            song.doc
                .track(track)
                .unwrap()
                .clips()
                .all(|c| c.data.is_audio())
        );

        let error = song.reject(Command::FreezeTracks {
            frozen: vec![(track, frozen)],
        });
        assert!(matches!(error, ModelError::InvalidInput(_)));
    }

    // Spec: freeze-and-bounce R7, R10. A frozen MIDI clip trims and moves
    // between slots keeping its freeze audio, and unfreezing gives each clip
    // its MIDI back where it now is.
    #[test]
    fn a_frozen_midi_clip_can_be_trimmed_and_moved_and_unfreezes_to_midi() {
        let (mut song, track, _) = frozen_midi_song();
        let held = song.doc.track(track).unwrap();
        let lane = held.clips().next().unwrap().id;
        let (scene, slot) = held.slots().next().map(|(s, c)| (s, c.id)).unwrap();
        let other = song.doc.scenes().nth(1).unwrap().id;

        song.apply(Command::TrimClipStart {
            clip: lane,
            start: Ticks::QUARTER,
        })
        .unwrap();
        song.apply(Command::MoveSlotClip {
            clip: slot,
            track,
            scene: other,
        })
        .unwrap();
        assert_ne!(scene, other);
        song.doc.check_invariants().unwrap();

        song.apply(Command::UnfreezeTracks {
            tracks: vec![track],
        })
        .unwrap();
        song.doc.check_invariants().unwrap();
        let held = song.doc.track(track).unwrap();
        assert!(held.clips().all(|c| !c.data.is_audio()));
        assert!(held.slots().all(|(_, c)| !c.data.is_audio()));
        assert_eq!(held.slots().next().map(|(s, _)| s), Some(other));
    }

    // Spec: freeze-and-bounce R7, R10. A copy made of a frozen MIDI clip
    // unfreezes to the MIDI of the clip it was copied from.
    #[test]
    fn a_copy_of_a_frozen_midi_clip_unfreezes_to_midi() {
        let (mut song, track, frozen) = frozen_midi_song();
        let held = song.doc.track(track).unwrap();
        let mut copy = held.clips().next().unwrap().clone();
        copy.id = song.fresh();
        copy.start = Ticks::WHOLE * 4;
        let id = copy.id;
        song.apply(Command::CreateClip { track, clip: copy })
            .unwrap();

        song.apply(Command::UnfreezeTracks {
            tracks: vec![track],
        })
        .unwrap();
        assert_eq!(song.doc.clip(id).unwrap().data, frozen.clips[0].original);
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

    // Spec: automation R11
    #[test]
    fn deleting_a_return_takes_the_send_envelopes_to_it_along() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let ret = song.track(TrackKind::Return, None);
        let other = song.track(TrackKind::Return, None);
        let clip = song.clip(track, 0);
        let send = AutomationTarget::Send { to: ret };
        let kept = AutomationTarget::Send { to: other };
        for target in [send, kept] {
            song.apply(Command::SetAutomationBreakpoint {
                track,
                target,
                at: Ticks::ZERO,
                value: 0.5,
            })
            .unwrap();
            song.apply(Command::SetClipEnvelopeBreakpoint {
                kind: EnvelopeKind::Modulation,
                clip,
                target,
                at: Ticks::ZERO,
                value: 0.5,
            })
            .unwrap();
        }

        song.apply(Command::DeleteTrack { track: ret }).unwrap();
        let track = song.doc.track(track).unwrap();
        assert!(track.automation_for(&send).is_none());
        assert!(track.automation_for(&kept).is_some());
        let clip = song.doc.clip(clip).unwrap();
        assert!(clip.envelope(&send).is_none());
        assert!(clip.envelope(&kept).is_some());
        song.doc.check_invariants().unwrap();
    }

    // Spec: tempo-map
    #[test]
    fn a_tempo_breakpoint_at_the_tempo_in_force_stays_where_it_was_put() {
        let bpm = |value| Bpm::new(value).unwrap();
        let bar = Ticks::WHOLE;
        let mut song = Song::new();
        song.apply(Command::SetTempoBreakpoint {
            at: bar * 5,
            bpm: bpm(120.0),
        })
        .unwrap();
        song.apply(Command::SetTempoBreakpoint {
            at: bar * 9,
            bpm: bpm(140.0),
        })
        .unwrap();
        let tempo = &song.doc.settings().tempo;
        assert_eq!(
            tempo.bpm_at(bar * 5),
            bpm(120.0),
            "the ramp starts at bar 5"
        );
        assert_eq!(tempo.bpm_at(bar * 7), bpm(130.0));

        // Deleting the far end leaves two equal breakpoints, both kept.
        song.apply(Command::DeleteTempoBreakpoint { at: bar * 9 })
            .unwrap();
        song.apply(Command::MoveTempoBreakpoint {
            at: bar * 5,
            to: bar * 6,
        })
        .unwrap();
    }

    // Spec: automation R1
    #[test]
    fn the_main_track_has_no_activator_to_automate() {
        let mut song = Song::new();
        let main = song.main();
        let audio = song.track(TrackKind::Audio, None);
        let mute = AutomationTarget::Mixer {
            param: MixerParam::Mute,
        };
        let point = |track| Command::SetAutomationBreakpoint {
            track,
            target: mute,
            at: Ticks::ZERO,
            value: 1.0,
        };
        assert!(matches!(
            song.reject(point(main)),
            ModelError::WrongTrackKind { .. }
        ));
        song.apply(point(audio)).unwrap();
        // The Main track's volume is still drawn.
        song.apply(Command::SetAutomationBreakpoint {
            track: main,
            target: AutomationTarget::Mixer {
                param: MixerParam::Volume,
            },
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
    }

    // Spec: arrangement-editing R7, R16
    #[test]
    fn trimming_a_midi_clip_keeps_what_the_engine_plays_where_it_is() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let clip = song.clip(track, 4);
        let region = |start| Command::SetMidiClipRegion {
            clip,
            start,
            end: None,
        };
        let marker = |song: &Song| match &song.doc.clip(clip).unwrap().data {
            ClipData::Midi(midi) => midi.start,
            ClipData::Audio(_) => unreachable!(),
        };

        // The Start marker is behind the offset: a trim past the marker
        // would shift every note, so it's refused.
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::QUARTER * 6,
        })
        .unwrap();
        song.apply(region(Ticks::ZERO)).unwrap();
        assert_eq!(
            song.reject(Command::TrimClipStart {
                clip,
                start: Ticks::QUARTER * 5,
            }),
            ModelError::NegativeOffset(-Ticks::QUARTER)
        );

        // Contents before the Start marker can be trimmed back into view.
        song.apply(region(Ticks::QUARTER)).unwrap();
        assert_eq!(
            song.doc
                .clip(clip)
                .unwrap()
                .earliest_start(&TempoMap::default()),
            Ticks::QUARTER * 5
        );
        song.apply(Command::TrimClipStart {
            clip,
            start: Ticks::QUARTER * 5,
        })
        .unwrap();
        assert_eq!(marker(&song), Ticks::ZERO);
        assert_eq!(song.doc.clip(clip).unwrap().end(), Ticks::QUARTER * 8);
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

    // Spec: note-editing R21, R30. A clip's groups hold two or more notes of
    // one chance, all listed; cropping ends a group left with one note.
    #[test]
    fn probability_groups_are_checked_and_end_below_two_notes() {
        use crate::{GroupKind, NoteGroup};
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let clip = song.clip(track, 0);
        let grouped = |chance: [f64; 2], listed: bool, third: bool| {
            let notes = chance.iter().enumerate().map(|(place, chance)| Note {
                probability: *chance,
                group: Some(1),
                ..Note::new(
                    60 + place as u8,
                    Ticks::QUARTER * place as i64,
                    Ticks::QUARTER,
                    100,
                )
            });
            let mut data = MidiData::new(
                notes.chain(third.then(|| Note::new(70, Ticks::QUARTER * 2, Ticks::QUARTER, 100))),
            );
            if listed {
                data.groups = vec![NoteGroup {
                    id: 1,
                    kind: GroupKind::PlayOne,
                }];
            }
            ClipData::Midi(data)
        };
        for data in [
            grouped([0.5, 0.25], true, false),
            grouped([0.5, 0.5], false, false),
        ] {
            assert_eq!(
                song.reject(Command::SetClipData { clip, data }),
                ModelError::BrokenGroups
            );
        }
        let whole = grouped([0.5, 0.5], true, true);
        song.apply(Command::SetClipData {
            clip,
            data: whole.clone(),
        })
        .unwrap();
        // A group of one note is refused too.
        let ClipData::Midi(full) = whole else {
            unreachable!()
        };
        let mut one = full.clone();
        one.notes.truncate(1);
        assert_eq!(
            song.reject(Command::SetClipData {
                clip,
                data: ClipData::Midi(one),
            }),
            ModelError::BrokenGroups
        );
        // Cropping to the first note leaves it alone in its group.
        let cropped = full.cropped(Ticks::ZERO, Ticks::QUARTER);
        assert_eq!(cropped.notes.len(), 1);
        assert!(cropped.groups.is_empty() && cropped.notes[0].group.is_none());
    }

    // Spec: audio-clips R23, R25, R30
    #[test]
    fn fades_take_curves_and_stop_at_their_limits() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        // A clip of a bar at 120 BPM: two seconds.
        let clip = song.clip(track, 0);
        let audio = |song: &Song| match &song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        let fades = |fade_in, fade_out, fade_in_curve, fade_out_curve| Command::SetClipFades {
            clip,
            fade_in,
            fade_out,
            fade_in_curve,
            fade_out_curve,
        };
        song.apply(fades(0.25, 0.5, Some(0.5), Some(-1.0))).unwrap();
        let set = audio(&song);
        assert_eq!((set.fade_in_curve, set.fade_out_curve), (0.5, -1.0));
        // A curve left out stays as it was.
        song.apply(fades(0.5, 0.5, None, Some(0.0))).unwrap();
        let set = audio(&song);
        assert_eq!((set.fade_in_curve, set.fade_out_curve), (0.5, 0.0));
        for bad in [
            fades(0.25, 0.5, Some(1.5), None),
            fades(0.25, 0.5, None, Some(f64::NAN)),
        ] {
            assert!(matches!(song.reject(bad), ModelError::InvalidValue { .. }));
        }
        // The fade-in must end no later than the fade-out begins.
        assert!(matches!(
            song.reject(fades(1.6, 0.5, None, None)),
            ModelError::InvalidInput(_)
        ));
        song.apply(fades(1.5, 0.5, None, None)).unwrap();

        // Spec: audio-clips R25. A fade takes in no point where the clip
        // goes round its loop brace: this clip starts half a second into its
        // file and goes round at 1.5 seconds, after one second.
        let looped = AudioData {
            start: 0.5,
            looping: true,
            loop_start: 1.0,
            loop_end: Some(1.5),
            ..audio(&song)
        };
        song.apply(Command::SetClipData {
            clip,
            data: ClipData::Audio(looped),
        })
        .unwrap();
        song.apply(fades(1.0, 0.0, None, None)).unwrap();
        assert!(matches!(
            song.reject(fades(1.1, 0.0, None, None)),
            ModelError::InvalidInput(_)
        ));
        // The brace repeats every half second from then, so the clip goes
        // round at 1, 1.5 and 2: a fade-out of 0.25 takes in none, 0.75 does.
        song.apply(fades(0.0, 0.5, None, None)).unwrap();
        assert!(matches!(
            song.reject(fades(0.0, 0.75, None, None)),
            ModelError::InvalidInput(_)
        ));
        let limits = song.doc.fade_limits(clip).unwrap();
        assert_eq!(
            (limits.start.inside, limits.start.outside),
            (1.0, 0.0),
            "no clip meets it"
        );
        assert_eq!(limits.end.inside, 0.5);
    }

    // Spec: audio-clips R24, R25, R30
    #[test]
    fn a_crossfade_is_kept_the_same_on_both_clips_and_ends_when_they_part() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let earlier = song.clip(track, 0);
        let later = song.clip(track, 4);
        let boundary = Ticks::WHOLE;
        let audio = |song: &Song, clip| match &song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        let found = |song: &Song| crate::clip::crossfades(song.doc.track(track).unwrap().clips());
        // The later clip starts half a second into its file, so it can play
        // that long before its start.
        song.apply(Command::SetClipRegion {
            clip: later,
            start: 0.5,
            end: None,
            ticks: false,
        })
        .unwrap();
        song.apply(Command::SetCrossfade {
            track,
            boundary,
            before: 0.01,
            after: 0.02,
            curve: 0.25,
        })
        .unwrap();
        let (first, second) = (audio(&song, earlier), audio(&song, later));
        assert_eq!(
            (first.fade_out, first.fade_out_after, first.fade_out_curve),
            (0.01, 0.02, 0.25)
        );
        assert_eq!(
            (second.fade_in, second.fade_in_before, second.fade_in_curve),
            (0.02, 0.01, 0.25)
        );
        assert_eq!(
            found(&song),
            [crate::clip::Crossfade {
                earlier,
                later,
                boundary,
                before: 0.01,
                after: 0.02,
                curve: 0.25
            }]
        );
        // The room each side: the later clip's file reaches back half a
        // second, and the earlier clip's fade-in leaves it its whole length.
        let limits = song.doc.fade_limits(later).unwrap();
        assert_eq!(limits.start.outside, 0.5);
        assert_eq!(song.doc.fade_limits(earlier).unwrap().end.outside, 2.0);

        // Changing the fade at one clip's edge changes the crossfade, and the
        // other clip follows.
        song.apply(Command::SetClipFades {
            clip: later,
            fade_in: 0.05,
            fade_out: 0.0,
            fade_in_curve: Some(-0.5),
            fade_out_curve: None,
        })
        .unwrap();
        let first = audio(&song, earlier);
        assert_eq!(
            (first.fade_out, first.fade_out_after, first.fade_out_curve),
            (0.01, 0.05, -0.5)
        );
        song.apply(Command::SetClipFades {
            clip: earlier,
            fade_in: 0.0,
            fade_out: 0.03,
            fade_in_curve: None,
            fade_out_curve: None,
        })
        .unwrap();
        let second = audio(&song, later);
        assert_eq!(
            (second.fade_in, second.fade_in_before, second.fade_in_curve),
            (0.05, 0.03, -0.5)
        );
        assert_eq!(found(&song).len(), 1);

        // Past a limit, where no clips meet, or with no span, it is refused.
        for bad in [
            // The later clip's file has half a second before its start.
            Command::SetCrossfade {
                track,
                boundary,
                before: 0.6,
                after: 0.0,
                curve: 0.0,
            },
            Command::SetCrossfade {
                track,
                boundary: boundary * 2,
                before: 0.01,
                after: 0.01,
                curve: 0.0,
            },
            Command::SetCrossfade {
                track,
                boundary,
                before: 0.0,
                after: 0.0,
                curve: 0.0,
            },
            Command::DeleteCrossfade {
                track,
                boundary: boundary * 2,
            },
        ] {
            assert!(matches!(song.reject(bad), ModelError::InvalidInput(_)));
        }
        assert!(matches!(
            song.reject(Command::SetCrossfade {
                track,
                boundary,
                before: 0.01,
                after: 0.01,
                curve: 2.0,
            }),
            ModelError::InvalidValue { .. }
        ));

        // Moving the later clip away leaves the earlier one's fields, but
        // they name a crossfade no longer.
        song.apply(Command::MoveClip {
            clip: later,
            track,
            start: Ticks::WHOLE * 3,
        })
        .unwrap();
        assert!(found(&song).is_empty());
        song.apply(Command::MoveClip {
            clip: later,
            track,
            start: boundary,
        })
        .unwrap();
        assert_eq!(found(&song).len(), 1);

        song.apply(Command::DeleteCrossfade { track, boundary })
            .unwrap();
        assert!(found(&song).is_empty());
        let (first, second) = (audio(&song, earlier), audio(&song, later));
        assert_eq!(
            (
                first.fade_out,
                first.fade_out_after,
                second.fade_in,
                second.fade_in_before
            ),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert!(matches!(
            song.reject(Command::DeleteCrossfade { track, boundary }),
            ModelError::InvalidInput(_)
        ));
    }

    // Spec: audio-clips R25, R30
    #[test]
    fn a_fade_the_other_clip_cant_follow_ends_the_crossfade() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let earlier = song.clip(track, 0);
        let later = song.clip(track, 4);
        let audio = |song: &Song, clip| match &song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        // The later clip starts at the start of its file: a crossfade can only
        // run past the boundary.
        song.apply(Command::SetCrossfade {
            track,
            boundary: Ticks::WHOLE,
            before: 0.0,
            after: 0.004,
            curve: 0.0,
        })
        .unwrap();
        // A longer fade-out for the earlier clip would need the later clip
        // to play before its start, so the crossfade ends, and each clip
        // keeps its own fade.
        song.apply(Command::SetClipFades {
            clip: earlier,
            fade_in: 0.0,
            fade_out: 0.01,
            fade_in_curve: None,
            fade_out_curve: None,
        })
        .unwrap();
        let (first, second) = (audio(&song, earlier), audio(&song, later));
        assert_eq!((first.fade_out, first.fade_out_after), (0.01, 0.0));
        assert_eq!((second.fade_in, second.fade_in_before), (0.004, 0.0));
        assert!(crate::clip::crossfades(song.doc.track(track).unwrap().clips()).is_empty());
    }

    // Spec: audio-clips R3, R4, R5, R6, R15
    #[test]
    fn audio_clip_settings_change_one_at_a_time_and_are_checked() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.clip(track, 0);
        let audio = |song: &Song| match &song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        // A new clip plays the whole file once, at 0 dB, with no fades.
        assert_eq!(audio(&song), AudioData::new(ContentHash([7; 32])));

        song.apply(Command::SetClipRegion {
            clip,
            start: 0.5,
            end: Some(2.0),
            ticks: false,
        })
        .unwrap();
        song.apply(Command::SetClipLoop {
            clip,
            start: 1.0,
            end: Some(1.5),
            ticks: false,
        })
        .unwrap();
        // Spec: warping R14. The Loop switch can't come on for an unwarped
        // clip; an older set's looped clip arrives with it on, through its
        // data, which is what this checks the markers of.
        assert!(matches!(
            song.reject(Command::SetClipLoopEnabled {
                clip,
                enabled: true,
            }),
            ModelError::InvalidInput(_)
        ));
        let looped = AudioData {
            looping: true,
            ..audio(&song)
        };
        song.apply(Command::SetClipData {
            clip,
            data: ClipData::Audio(looped),
        })
        .unwrap();
        song.apply(Command::SetClipGain { clip, gain: 0.5 })
            .unwrap();
        song.apply(Command::SetClipFades {
            clip,
            fade_in: 0.25,
            fade_out: EDGE_FADE,
            fade_in_curve: None,
            fade_out_curve: None,
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
                ticks: false,
            },
            Command::SetClipRegion {
                clip,
                start: 1.0,
                end: Some(1.0),
                ticks: false,
            },
            // The Start marker must stay before the loop's end while the
            // loop is on.
            Command::SetClipRegion {
                clip,
                start: 1.5,
                end: None,
                ticks: false,
            },
            Command::SetClipLoop {
                clip,
                start: 0.25,
                end: Some(0.5),
                ticks: false,
            },
            Command::SetClipLoop {
                clip,
                start: f64::NAN,
                end: None,
                ticks: false,
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
                fade_in_curve: None,
                fade_out_curve: None,
            },
            Command::SetClipFades {
                clip,
                fade_in: 0.0,
                fade_out: f64::NAN,
                fade_in_curve: None,
                fade_out_curve: None,
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
            ticks: false,
        })
        .unwrap();
        // And then the loop can't come on (an unwarped clip can't loop, R14,
        // and for a warped one the Start marker must be before the brace's
        // end, which `warped_clips_loop_only_inside_their_brace` checks).
        assert!(matches!(
            song.reject(Command::SetClipLoopEnabled {
                clip,
                enabled: true,
            }),
            ModelError::InvalidInput(_)
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

    // Spec: recording R12; comping R1, R3
    #[test]
    fn take_lanes_are_added_in_order_to_audio_and_midi_tracks_and_survive_a_save() {
        let mut song = Song::new();
        let audio_track = song.track(TrackKind::Audio, None);
        let midi_track = song.track(TrackKind::Midi, None);
        let lane = |song: &mut Song, name: &str, start: f64, data: Option<ClipData>| {
            let data = data.map_or_else(
                || {
                    TakeClipData::Audio(AudioData {
                        start,
                        end: Some(start + 0.5),
                        ..AudioData::new(ContentHash([3; 32]))
                    })
                },
                TakeClipData::Tagged,
            );
            NewTakeLane {
                id: Some(TakeLaneId(song.ids.allocate())),
                name: name.to_owned(),
                clips: vec![TakeClip {
                    id: Some(ClipId(song.ids.allocate())),
                    name: "Take".to_owned(),
                    start: Ticks::QUARTER,
                    length: Ticks::QUARTER,
                    data,
                }],
            }
        };
        let before = song.doc.clone();
        let first = lane(&mut song, "Take 1", 0.0, None);
        let second = lane(&mut song, "Take 2", 0.5, None);
        for lane in [first, second] {
            song.apply(Command::AddTakeLane {
                track: audio_track,
                lane,
                index: None,
            })
            .unwrap();
        }
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

        // A MIDI track takes MIDI clips on a lane of its own, and a clip of
        // the wrong kind is refused.
        let midi_lane = lane(&mut song, "Take 1", 0.0, Some(midi()));
        song.apply(Command::AddTakeLane {
            track: midi_track,
            lane: midi_lane,
            index: None,
        })
        .unwrap();
        let wrong = lane(&mut song, "Take 2", 0.0, None);
        assert!(matches!(
            song.reject(Command::AddTakeLane {
                track: midi_track,
                lane: wrong,
                index: None,
            }),
            ModelError::WrongTrackKind { .. }
        ));
        let group = song.track(TrackKind::Group, None);
        let none = lane(&mut song, "Take 1", 0.0, None);
        assert!(matches!(
            song.reject(Command::AddTakeLane {
                track: group,
                lane: none,
                index: None,
            }),
            ModelError::WrongTrackKind { .. }
        ));

        // A clip needs a real length and sound data.
        let mut empty = lane(&mut song, "Take 3", 0.0, None);
        empty.clips[0].length = Ticks::ZERO;
        assert!(matches!(
            song.reject(Command::AddTakeLane {
                track: audio_track,
                lane: empty,
                index: None,
            }),
            ModelError::EmptyLength(_)
        ));
        let mut bad = lane(&mut song, "Take 3", 0.0, None);
        bad.clips[0].data = TakeClipData::Audio(AudioData {
            gain: f64::NAN,
            ..AudioData::new(ContentHash([3; 32]))
        });
        assert!(matches!(
            song.reject(Command::AddTakeLane {
                track: audio_track,
                lane: bad,
                index: None,
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
        assert_eq!(loaded.track(midi_track).unwrap().take_lanes().len(), 1);
    }

    // Spec: mixer R28, R29, R30; delay-compensation R7, R8, R9
    #[test]
    fn the_track_delay_the_pan_mode_and_the_switches_save_and_undo() {
        let mut editor = crate::Editor::new();
        let track: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();

        editor
            .apply(Command::SetTrackDelay {
                track,
                delay: TrackDelay::Milliseconds { value: 10.0 },
            })
            .unwrap();
        assert_eq!(editor.undo_history().next(), Some("Change track delay"));
        editor
            .apply(Command::SetPanMode {
                track,
                mode: PanMode::Split,
            })
            .unwrap();
        assert_eq!(editor.undo_history().next(), Some("Change track pan mode"));
        editor
            .apply(Command::SetSplitPanLeft { track, pan: -0.5 })
            .unwrap();
        assert_eq!(editor.undo_history().next(), Some("Change split pan"));
        editor
            .apply(Command::SetSplitPanRight { track, pan: 0.5 })
            .unwrap();
        editor
            .apply(Command::SetKeepMonitoringLatency {
                track,
                enabled: false,
            })
            .unwrap();
        assert_eq!(
            editor.undo_history().next(),
            Some("Switch Keep Monitoring Latency")
        );
        editor
            .apply(Command::SetReducedLatencyMonitoring { enabled: true })
            .unwrap();
        assert_eq!(
            editor.undo_history().next(),
            Some("Switch Reduced Latency When Monitoring")
        );
        editor
            .apply(Command::SetDelayCompensation { enabled: false })
            .unwrap();
        assert_eq!(
            editor.undo_history().next(),
            Some("Switch Delay Compensation")
        );
        // While compensation is off neither can be set: the delay is refused
        // and Reduced Latency When Monitoring can't be turned on (the delay
        // compensation spec, R7 and R8). No undo step is made.
        assert!(matches!(
            editor.apply(Command::SetTrackDelay {
                track,
                delay: TrackDelay::default(),
            }),
            Err(crate::editor::EditError::Command(
                crate::error::ModelError::TrackDelayNeedsCompensation
            ))
        ));
        assert!(matches!(
            editor.apply(Command::SetReducedLatencyMonitoring { enabled: true }),
            Err(crate::editor::EditError::Command(
                crate::error::ModelError::ReducedLatencyNeedsCompensation
            ))
        ));
        assert_eq!(
            editor.undo_history().next(),
            Some("Switch Delay Compensation"),
            "a refused command leaves no step"
        );

        // The set keeps them and reads them back.
        let saved =
            serde_json::to_string(&crate::repr::DocumentRepr::from(editor.document())).unwrap();
        let loaded: crate::repr::DocumentRepr = serde_json::from_str(&saved).unwrap();
        let loaded = Document::try_from(loaded).unwrap();
        let loaded_track = loaded.track(track).unwrap();
        assert_eq!(
            loaded_track.delay(),
            TrackDelay::Milliseconds { value: 10.0 }
        );
        assert_eq!(loaded_track.mixer().pan_mode, PanMode::Split);
        assert_eq!(
            (
                loaded_track.mixer().split_left,
                loaded_track.mixer().split_right
            ),
            (-0.5, 0.5)
        );
        assert!(!loaded_track.keep_monitoring_latency());
        assert!(!loaded.settings().delay_compensation);
        assert!(loaded.settings().reduced_latency_monitoring);

        // Each command is one undo step and restores what it replaced.
        for _ in 0..7 {
            editor.undo().unwrap();
        }
        let undone = editor.document().track(track).unwrap();
        assert_eq!(undone.delay(), TrackDelay::default());
        assert_eq!(undone.mixer().pan_mode, PanMode::Stereo);
        assert_eq!(
            (undone.mixer().split_left, undone.mixer().split_right),
            (0.0, 0.0)
        );
        assert!(undone.keep_monitoring_latency());
        assert!(editor.document().settings().delay_compensation);
        assert!(!editor.document().settings().reduced_latency_monitoring);
    }

    // Spec: audio-clips R7
    #[test]
    fn create_fades_on_clip_edges_gives_new_clips_4_ms_fades() {
        assert_eq!(EDGE_FADE, 0.004);
        let plain = AudioData::new(ContentHash([1; 32]));
        assert_eq!((plain.fade_in, plain.fade_out), (0.0, 0.0));
        let faded = plain.clone().with_edge_fades();
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
        let audio = |song: &Song| match &song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
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
            ticks: false,
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
                ticks: false,
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

    // Spec: clip-properties R10. A warped audio clip has a signature of its
    // own that sets and clears; an unwarped clip has none to set.
    #[test]
    fn a_warped_audio_clips_signature_sets_and_clears_and_an_unwarped_clip_has_none() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.fresh();
        let data = AudioData::new(ContentHash([7; 32]));
        song.apply(Command::CreateClip {
            track,
            clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, ClipData::Audio(data)),
        })
        .unwrap();
        let signature = |song: &Song| match &song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.signature,
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        let seven_eight = TimeSignature::new(7, 8).ok();
        let _ = song.reject(Command::SetClipSignature {
            clip,
            signature: seven_eight,
        });
        assert_eq!(signature(&song), None);
        song.apply(Command::SetClipWarp { clip, warp: true })
            .unwrap();
        song.apply(Command::SetClipSignature {
            clip,
            signature: seven_eight,
        })
        .unwrap();
        assert_eq!(signature(&song), seven_eight);
        song.apply(Command::SetClipSignature {
            clip,
            signature: None,
        })
        .unwrap();
        assert_eq!(signature(&song), None);
    }

    // Spec: warping R5. A warped clip's markers are ticks of its contents,
    // so a trim or a split moves the Start marker by the ticks the edge
    // moved and a loop wraps it exactly.
    #[test]
    fn trimming_and_splitting_a_warped_clip_move_its_start_marker_in_ticks() {
        let q = Ticks::QUARTER.get();
        let tempo = TempoMap::default();
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.fresh();
        // The file's start falls a quarter note before 1.1.1: one marker at
        // half a second, at 120 BPM.
        let warped = AudioData {
            warp: true,
            warp_markers: vec![WarpMarker {
                time: 0.5,
                beat: Ticks::ZERO,
            }],
            tempo: Some(120.0),
            looping: true,
            loop_start: 0.0,
            loop_end: Some(4.0 * q as f64),
            ..AudioData::new(ContentHash([7; 32]))
        };
        let start = Ticks::WHOLE;
        song.apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                start,
                Ticks::WHOLE * 12,
                ClipData::Audio(warped.clone()),
            ),
        })
        .unwrap();
        let marker = |song: &Song| match &song.doc.clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.start,
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        let trim = |song: &mut Song, to: Ticks| {
            song.apply(Command::TrimClipStart { clip, start: to })
                .unwrap();
        };
        // Within the brace: the marker moves by the ticks, exactly.
        trim(&mut song, start + Ticks(q + 12_345));
        assert_eq!(marker(&song), (q + 12_345) as f64);
        // Past the brace's end, it wraps by whole lengths of it.
        trim(&mut song, start + Ticks(4 * q + 3));
        assert_eq!(marker(&song), 3.0, "one loop on");
        trim(&mut song, start + Ticks(9 * q + 7));
        assert_eq!(marker(&song), (q + 7) as f64, "two loops and a quarter on");
        // Back to the left, down to the start of the file and no further:
        // that is a quarter note before the Start marker's beat 0.
        let earliest = song.doc.clip(clip).unwrap().earliest_start(&tempo);
        trim(&mut song, earliest);
        assert_eq!(marker(&song), -(q as f64));
        song.reject(Command::TrimClipStart {
            clip,
            start: earliest - Ticks(1),
        });
        // A split plays what the whole did: the part's marker is the whole's
        // moved by the ticks, with no float error however far it goes.
        let whole = Clip::new(clip, start, Ticks::WHOLE * 12, ClipData::Audio(warped));
        for at in [1, 4 * q + 5, 7 * q + 1, 40 * q + 123_457] {
            let part = whole.part(start + Ticks(at), whole.end(), &tempo);
            let ClipData::Audio(moved) = &part.data else {
                unreachable!("an audio clip");
            };
            let length = 4 * q;
            let expected = if at < length {
                at
            } else {
                (at - length) % length
            };
            assert_eq!(moved.start, expected as f64, "split {at} ticks in");
            // The waveform carries on, until a wrap moves the marker round:
            // the file starts where it did.
            if at < length {
                assert_eq!(part.origin(&tempo), whole.origin(&tempo));
            }
        }
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
                ClipData::Audio(looped.clone()),
            ),
        })
        .unwrap();
        let start = |song: &Song| match &song.doc.clip(clip).unwrap().data {
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
            ..looped.clone()
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
            ClipData::Audio(audio.clone()),
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

    // Spec: device-chains R2, R3; IMP-24
    #[test]
    fn a_native_device_keeps_its_role_and_a_plug_in_takes_another_position() {
        let mut song = Song::new();
        let midi = song.track(TrackKind::Midi, None);
        let native = song.device(midi);
        // A native device keeps the role its type gives it.
        assert_eq!(
            song.reject(Command::SetDeviceRole {
                device: native,
                role: Some(DeviceRole::MidiEffect),
            }),
            ModelError::InvalidInput("a native device keeps the role its type gives it")
        );
        // A plug-in takes the MIDI-effect position, before the instrument.
        let plugin = DeviceId(song.ids.allocate());
        song.apply(Command::InsertDevice {
            track: midi,
            index: 0,
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
        song.apply(Command::SetDeviceRole {
            device: plugin,
            role: Some(DeviceRole::MidiEffect),
        })
        .unwrap();
        let roles: Vec<_> = song
            .doc
            .track(midi)
            .unwrap()
            .devices()
            .map(|d| (d.id, d.role))
            .collect();
        assert_eq!(
            roles,
            [
                (plugin, DeviceRole::MidiEffect),
                (native, DeviceRole::AudioEffect)
            ]
        );
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
            shape: None,
            articulations: None,
            mpe: None,
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
                shape: None,
                articulations: None,
                mpe: None,
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

    // Spec: automation R28
    #[test]
    fn clearing_a_tracks_or_a_devices_envelopes_leaves_the_rest() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let ret = song.track(TrackKind::Return, None);
        let device = song.device(track);
        let other_device = song.device(track);
        let targets = [
            AutomationTarget::Mixer {
                param: MixerParam::Volume,
            },
            AutomationTarget::Send { to: ret },
            AutomationTarget::Device {
                device,
                param: ParamId(2),
            },
            AutomationTarget::Device {
                device: other_device,
                param: ParamId(2),
            },
        ];
        let has = |song: &Song, target: &AutomationTarget| {
            song.doc
                .track(track)
                .unwrap()
                .automation_for(target)
                .is_some()
        };
        let draw = |song: &mut Song| {
            for target in targets {
                song.apply(Command::SetAutomationBreakpoint {
                    track,
                    target,
                    at: Ticks::ZERO,
                    value: 0.5,
                })
                .unwrap();
            }
        };
        draw(&mut song);

        // A device's envelopes go; the other device's and the mixer's stay.
        song.apply(Command::ClearDeviceAutomation {
            track,
            device: Some(device),
        })
        .unwrap();
        assert_eq!(targets.map(|t| has(&song, &t)), [true, true, false, true]);
        // The mixer's go: the volume and the send.
        song.apply(Command::ClearDeviceAutomation {
            track,
            device: None,
        })
        .unwrap();
        assert_eq!(targets.map(|t| has(&song, &t)), [false, false, false, true]);
        // A device the track doesn't have is refused.
        let elsewhere = song.track(TrackKind::Audio, None);
        let stranger = song.device(elsewhere);
        assert_eq!(
            song.reject(Command::ClearDeviceAutomation {
                track,
                device: Some(stranger),
            }),
            ModelError::DeviceNotInTrack {
                device: stranger,
                track,
            }
        );
        // Every envelope of the track.
        draw(&mut song);
        song.apply(Command::ClearTrackAutomation { track }).unwrap();
        assert_eq!(targets.map(|t| has(&song, &t)), [false; 4]);
        // Clearing again changes nothing.
        song.apply(Command::ClearTrackAutomation { track }).unwrap();
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
            kind: EnvelopeKind::Modulation,
            clip,
            target: volume,
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
        song.apply(Command::MoveClipEnvelopeBreakpoint {
            kind: EnvelopeKind::Modulation,
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
            kind: EnvelopeKind::Modulation,
            clip,
            target: volume,
            at: Ticks::WHOLE,
            to: Ticks::WHOLE * 2,
            value: Some(f64::NAN),
        });
    }

    // Spec: session-automation R10
    #[test]
    fn a_session_clip_holds_an_automation_envelope_beside_its_modulation_one() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let scene = song.doc.scenes().next().unwrap().id;
        let slot: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(slot, Ticks::ZERO, Ticks::WHOLE, midi()),
        })
        .unwrap();
        let arrangement = song.clip(track, 0);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        let set = |clip, kind, value| Command::SetClipEnvelopeBreakpoint {
            clip,
            target: volume,
            kind,
            at: Ticks::ZERO,
            value,
        };
        song.apply(set(slot, EnvelopeKind::Modulation, 0.5))
            .unwrap();
        song.apply(set(slot, EnvelopeKind::Automation, 0.25))
            .unwrap();
        let clip = song.doc.clip(slot).unwrap();
        assert_eq!(clip.envelopes().count(), 2, "one of each kind");
        let value = |kind| {
            clip.envelope_of(&volume, kind)
                .unwrap()
                .envelope
                .value_at(Ticks::ZERO)
        };
        assert_eq!(value(EnvelopeKind::Modulation), Some(0.5));
        assert_eq!(value(EnvelopeKind::Automation), Some(0.25));
        assert_eq!(
            clip.envelope(&volume).unwrap().kind,
            EnvelopeKind::Modulation,
            "the plain lookup is the modulation envelope"
        );

        // An Arrangement clip's envelopes only modulate: the automation is
        // the track's.
        song.reject(set(arrangement, EnvelopeKind::Automation, 0.5));
        song.apply(set(arrangement, EnvelopeKind::Modulation, 0.5))
            .unwrap();

        // Deleting one kind leaves the other.
        song.apply(Command::DeleteClipEnvelope {
            clip: slot,
            target: volume,
            kind: EnvelopeKind::Automation,
        })
        .unwrap();
        let clip = song.doc.clip(slot).unwrap();
        assert!(
            clip.envelope_of(&volume, EnvelopeKind::Automation)
                .is_none()
        );
        assert!(
            clip.envelope_of(&volume, EnvelopeKind::Modulation)
                .is_some()
        );
    }

    // Spec: session-automation R9
    #[test]
    fn a_hand_move_turns_a_session_clips_automation_off_until_it_is_re_enabled() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let scene = song.doc.scenes().next().unwrap().id;
        let slot: ClipId = song.fresh();
        song.apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(slot, Ticks::ZERO, Ticks::WHOLE, midi()),
        })
        .unwrap();
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        let pan = AutomationTarget::Mixer {
            param: MixerParam::Pan,
        };
        for target in [volume, pan] {
            song.apply(Command::SetClipEnvelopeBreakpoint {
                clip: slot,
                target,
                kind: EnvelopeKind::Automation,
                at: Ticks::ZERO,
                value: 0.5,
            })
            .unwrap();
        }
        let enabled = |song: &Song, target| {
            song.doc
                .clip(slot)
                .unwrap()
                .envelope_of(&target, EnvelopeKind::Automation)
                .unwrap()
                .enabled
        };
        assert!(!song.doc.track(track).unwrap().automation_overridden());

        // The control moved by hand takes its parameter from the clip's
        // envelope, and only that one.
        song.apply(Command::SetVolume { track, volume: 0.3 })
            .unwrap();
        assert!(!enabled(&song, volume));
        assert!(enabled(&song, pan));
        assert!(song.doc.track(track).unwrap().automation_overridden());

        // Re-enabling the one parameter, or all of them, gives it back.
        song.apply(Command::ReEnableAutomationFor {
            track,
            target: volume,
        })
        .unwrap();
        assert!(enabled(&song, volume));
        assert!(!song.doc.track(track).unwrap().automation_overridden());
        song.apply(Command::SetPan { track, pan: 0.5 }).unwrap();
        song.apply(Command::SetVolume { track, volume: 0.4 })
            .unwrap();
        assert!(!enabled(&song, pan) && !enabled(&song, volume));
        song.apply(Command::ReEnableAutomation {}).unwrap();
        assert!(enabled(&song, pan) && enabled(&song, volume));
    }

    // Spec: clip-envelopes R12, R13
    #[test]
    fn a_whole_clip_envelope_replaces_the_breakpoints_and_keeps_the_linked_switch() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.clip(track, 0);
        let pan = AutomationTarget::Mixer {
            param: MixerParam::Pan,
        };
        let set = |points: Vec<Breakpoint>| Command::SetClipEnvelope {
            kind: EnvelopeKind::Modulation,
            clip,
            target: pan,
            points,
        };
        let point = |at: Ticks, value: f64| Breakpoint {
            at,
            value,
            bend: 0.0,
        };
        song.apply(Command::SetClipEnvelopeLinked {
            kind: EnvelopeKind::Modulation,
            clip,
            target: pan,
            linked: false,
        })
        .unwrap();
        song.apply(set(vec![
            point(Ticks::ZERO, -1.0),
            point(Ticks::WHOLE, 1.0),
        ]))
        .unwrap();
        let envelope = song.doc.clip(clip).unwrap().envelope(&pan).unwrap();
        assert!(!envelope.linked, "the switch stays as it was");
        assert_eq!(envelope.envelope.value_at(Ticks::WHOLE / 2), Some(0.0));
        // A value that isn't a number, a position out of order and a
        // position before zero are refused, and change nothing.
        for broken in [
            vec![point(Ticks::ZERO, f64::NAN)],
            vec![point(Ticks::WHOLE, 0.0), point(Ticks::ZERO, 0.0)],
            vec![point(Ticks(-1), 0.0)],
        ] {
            let before = song.doc.clone();
            song.reject(set(broken));
            assert_eq!(song.doc, before);
        }
        // An empty list takes the envelope with it.
        song.apply(set(Vec::new())).unwrap();
        assert!(song.doc.clip(clip).unwrap().envelope(&pan).is_none());
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
            kind: EnvelopeKind::Modulation,
            clip,
            target: volume,
            at: Ticks::ZERO,
            value: 1.0,
        })
        .unwrap();
        song.apply(Command::SetClipEnvelopeBreakpoint {
            kind: EnvelopeKind::Modulation,
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
            kind: EnvelopeKind::Modulation,
            clip,
            target: send,
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
        song.apply(Command::SetClipEnvelopeBreakpoint {
            kind: EnvelopeKind::Modulation,
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
            kind: EnvelopeKind::Modulation,
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
            kind: EnvelopeKind::Modulation,
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
            kind: EnvelopeKind::Modulation,
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
            kind: EnvelopeKind::Modulation,
            clip,
            target: volume,
            at: Ticks::WHOLE,
            value: 0.75,
        })
        .unwrap();
        song.apply(Command::DeleteClipEnvelopeBreakpoint {
            kind: EnvelopeKind::Modulation,
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
            kind: EnvelopeKind::Modulation,
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
                    kind: EnvelopeKind::Modulation,
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
                    kind: EnvelopeKind::Modulation,
                    clip,
                    target: volume,
                    at: Ticks::ZERO,
                    value: f64::NAN,
                })
                .is_err()
        );
        assert_eq!(
            song.reject(Command::MoveClipEnvelopeBreakpoint {
                kind: EnvelopeKind::Modulation,
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
                kind: EnvelopeKind::Modulation,
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
                    kind: EnvelopeKind::Modulation,
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
                    kind: EnvelopeKind::Modulation,
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
                    kind: EnvelopeKind::Modulation,
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
                kind: EnvelopeKind::Modulation,
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
            kind: EnvelopeKind::Modulation,
            clip,
            target: param,
            at: Ticks::ZERO,
        })
        .unwrap();
        assert!(song.doc.clip(clip).unwrap().envelope(&param).is_none());
        song.apply(Command::DeleteClipEnvelope {
            clip,
            target: send,
            kind: EnvelopeKind::Modulation,
        })
        .unwrap();
        assert!(song.doc.clip(clip).unwrap().envelope(&send).is_none());
        song.apply(Command::DeleteClipEnvelope {
            clip,
            target: send,
            kind: EnvelopeKind::Modulation,
        })
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
                kind: EnvelopeKind::Modulation,
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
                kind: EnvelopeKind::Modulation,
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

    // Spec: follow-actions R1, R3, R4, R7, R17
    #[test]
    fn follow_actions_are_set_per_clip_and_scene_and_only_on_a_slot_clip() {
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
        let patch = FollowPatch {
            enabled: Some(true),
            action_a: Some(FollowAction::Jump),
            jump_a: Some(99),
            linked: Some(false),
            time: Some(Ticks::ZERO),
            ..FollowPatch::default()
        };
        song.apply(Command::SetClipFollowActions { clip, patch })
            .unwrap();
        let follow = song.doc.clip(clip).unwrap().launch.follow;
        assert_eq!(
            (
                follow.enabled,
                follow.action_a,
                follow.jump_a,
                follow.linked
            ),
            (true, FollowAction::Jump, DEFAULT_SCENE_COUNT as u32, false),
            "a jump target is held at the number of scenes"
        );
        assert_eq!(follow.time, crate::FOLLOW_TIME_MIN);
        song.apply(Command::SetSceneFollowActions { scene, patch })
            .unwrap();
        assert_eq!(song.doc.scene(scene).unwrap().follow, follow);

        let placed: ClipId = song.fresh();
        song.apply(Command::CreateClip {
            track,
            clip: Clip::new(placed, Ticks::ZERO, Ticks::WHOLE, audio()),
        })
        .unwrap();
        assert_eq!(
            song.reject(Command::SetClipFollowActions {
                clip: placed,
                patch
            }),
            ModelError::NotSlotClip(placed)
        );
        assert!(matches!(
            song.reject(Command::SetClipFollowActions {
                clip,
                patch: FollowPatch {
                    chance_a: Some(101),
                    ..FollowPatch::default()
                }
            }),
            ModelError::InvalidValue { .. }
        ));
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

    fn param_of(song: &Song, device: DeviceId, active: bool) -> Option<f64> {
        let d = song.doc.device(device).unwrap();
        let params = if active { &d.params } else { d.other_params() };
        params.get(&ParamId(2)).copied()
    }

    fn set_param(song: &mut Song, device: DeviceId, value: f64) {
        song.apply(Command::SetDeviceParam {
            device,
            param: ParamId(2),
            value,
        })
        .unwrap();
    }

    // Spec: device-compare R1
    #[test]
    fn a_new_device_has_two_equal_states_with_a_active() {
        let mut song = Song::new();
        let device = utility(song.fresh());
        assert!(device.can_compare());
        assert!(!device.b_active);
        assert_eq!(device.other_params(), &device.params);
    }

    // Spec: device-compare R2
    #[test]
    fn a_change_goes_to_the_active_state_only() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let device = song.device(track);
        set_param(&mut song, device, -3.0);
        // A changed, B kept the default (not set).
        assert_eq!(param_of(&song, device, true), Some(-3.0));
        assert_eq!(param_of(&song, device, false), None);
        song.apply(Command::CompareSwitch { device }).unwrap();
        set_param(&mut song, device, 5.0);
        assert_eq!(param_of(&song, device, true), Some(5.0));
        assert_eq!(param_of(&song, device, false), Some(-3.0));
        // A preset loads into both states, with A active (R9).
        song.apply(Command::SetDeviceState {
            device,
            params: [(ParamId(2), 1.0)].into_iter().collect(),
            name: String::new(),
            shape: None,
            articulations: None,
            mpe: None,
        })
        .unwrap();
        let d = song.doc.device(device).unwrap();
        assert!(!d.b_active && d.other_params.is_none());
        assert_eq!(param_of(&song, device, false), Some(1.0));
    }

    // Spec: device-compare R3, R4, R7
    #[test]
    fn a_switch_swaps_the_states_and_overrides_the_envelopes_and_a_copy_does_not() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let device = song.device(track);
        let target = AutomationTarget::Device {
            device,
            param: ParamId(7),
        };
        song.apply(Command::SetAutomationBreakpoint {
            track,
            target,
            at: Ticks::ZERO,
            value: 1.0,
        })
        .unwrap();
        set_param(&mut song, device, -3.0);
        let enabled = |song: &Song| {
            song.doc
                .track(track)
                .unwrap()
                .automation_for(&target)
                .unwrap()
                .enabled
        };
        // Setting another parameter by hand leaves this envelope on; a copy
        // overrides nothing.
        assert!(enabled(&song));
        song.apply(Command::CompareCopy { device }).unwrap();
        assert!(enabled(&song));
        assert_eq!(param_of(&song, device, false), Some(-3.0));
        // Equal states: a switch still overrides every envelope of the
        // device (R7), and the other state keeps its values.
        song.apply(Command::CompareSwitch { device }).unwrap();
        assert!(!enabled(&song));
        assert!(song.doc.device(device).unwrap().b_active);
        assert_eq!(param_of(&song, device, true), Some(-3.0));
        // Re-Enable Automation, then a switch back overrides it again.
        song.apply(Command::ReEnableAutomationFor { track, target })
            .unwrap();
        assert!(enabled(&song));
        song.apply(Command::CompareSwitch { device }).unwrap();
        assert!(!enabled(&song));
        assert!(!song.doc.device(device).unwrap().b_active);
    }

    // Spec: device-compare R4
    #[test]
    fn a_copy_overwrites_the_other_state() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let device = song.device(track);
        set_param(&mut song, device, -3.0);
        song.apply(Command::CompareSwitch { device }).unwrap();
        set_param(&mut song, device, 5.0);
        // B is active: Copy B to A.
        song.apply(Command::CompareCopy { device }).unwrap();
        assert_eq!(param_of(&song, device, true), Some(5.0));
        assert_eq!(param_of(&song, device, false), Some(5.0));
        assert!(song.doc.device(device).unwrap().b_active);
    }

    // Spec: device-compare R6, R8
    #[test]
    fn plug_ins_have_no_states_and_a_moved_device_keeps_both() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let other = song.track(TrackKind::Audio, None);
        let plugin = DeviceId(song.ids.allocate());
        song.apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                plugin,
                "Reverb",
                DeviceKind::Plugin {
                    format: PluginFormat::Clap,
                    plugin_id: "org.example.reverb".to_owned(),
                },
            ),
        })
        .unwrap();
        assert_eq!(
            song.reject(Command::CompareSwitch { device: plugin }),
            ModelError::NoCompare(plugin)
        );
        assert_eq!(
            song.reject(Command::CompareCopy { device: plugin }),
            ModelError::NoCompare(plugin)
        );
        let device = song.device(track);
        set_param(&mut song, device, -3.0);
        song.apply(Command::CompareSwitch { device }).unwrap();
        set_param(&mut song, device, 5.0);
        song.apply(Command::MoveDevice {
            device,
            track: other,
            index: 0,
        })
        .unwrap();
        assert!(song.doc.device(device).unwrap().b_active);
        assert_eq!(param_of(&song, device, true), Some(5.0));
        assert_eq!(param_of(&song, device, false), Some(-3.0));
    }

    // Spec: audio-clip-editing R17.
    #[test]
    fn cropping_a_midi_clip_deletes_the_notes_outside_its_region() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let clip = song.clip(track, 1);
        let quarter = Ticks::QUARTER;
        let notes = vec![
            // Outside the region, before it.
            Note::new(60, Ticks::ZERO, quarter, 100),
            // Starts inside and runs past the region's end.
            Note::new(62, quarter + quarter / 2, quarter * 2, 100),
            // Wholly inside.
            Note::new(64, quarter * 2, quarter / 2, 100),
            // Outside the region, after it.
            Note::new(65, quarter * 3 + quarter / 2, quarter, 100),
        ];
        song.apply(Command::SetClipData {
            clip,
            data: ClipData::Midi(MidiData::new(notes)),
        })
        .unwrap();
        song.apply(Command::CropClip {
            clip,
            media: None,
            from: quarter.get() as f64,
            to: (quarter * 3).get() as f64,
        })
        .unwrap();

        let ClipData::Midi(midi) = &song.doc.clip(clip).unwrap().data else {
            panic!("a MIDI clip");
        };
        let kept: Vec<(i64, i64)> = midi
            .notes
            .iter()
            .map(|note| (note.start.get(), note.length.get()))
            .collect();
        assert_eq!(
            kept,
            vec![
                (quarter.get() / 2, quarter.get() * 3 / 2),
                (quarter.get(), quarter.get() / 2)
            ],
            "the notes outside went, and the one running past the end is cut there"
        );
        assert_eq!(
            midi.start,
            Ticks::ZERO,
            "the markers begin at the region's start"
        );
        assert_eq!(song.doc.clip(clip).unwrap().offset, Ticks::ZERO);
    }

    // Spec: audio-clip-editing R15.
    #[test]
    fn cropping_a_warped_clip_moves_its_warp_map_back_with_it() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.clip(track, 0);
        let quarter = Ticks::QUARTER;
        // Two bars of audio, a marker on every beat.
        let markers: Vec<WarpMarker> = (0..8)
            .map(|beat| WarpMarker {
                time: beat as f64 * 0.5,
                beat: quarter * beat,
            })
            .collect();
        let audio = AudioData {
            start: Ticks::ZERO.get() as f64,
            tempo: Some(120.0),
            warp: true,
            warp_markers: markers,
            ..AudioData::new(ContentHash([9; 32]))
        };
        song.apply(Command::SetClipData {
            clip,
            data: ClipData::Audio(audio),
        })
        .unwrap();
        // Crop the second half.
        song.apply(Command::CropClip {
            clip,
            media: Some(ContentHash([8; 32])),
            from: (quarter * 4).get() as f64,
            to: (quarter * 8).get() as f64,
        })
        .unwrap();
        let ClipData::Audio(audio) = &song.doc.clip(clip).unwrap().data else {
            panic!("an audio clip");
        };
        assert_eq!(audio.media, ContentHash([8; 32]));
        let first = audio.warp_markers.first().unwrap();
        assert_eq!((first.time, first.beat), (0.0, Ticks::ZERO));
        let last = audio.warp_markers.last().unwrap();
        assert_eq!(
            (last.time, last.beat),
            (2.0, quarter * 4),
            "the map moved back by the region's start and holds the file's end"
        );
    }

    // Spec: audio-clip-editing R23.
    #[test]
    fn consolidating_midi_clips_writes_out_every_note_and_envelope_they_played() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Midi, None);
        let clip = song.clip(track, 1);
        let quarter = Ticks::QUARTER;
        song.apply(Command::SetClipData {
            clip,
            data: ClipData::Midi(MidiData::new([Note::new(60, Ticks::ZERO, quarter, 100)])),
        })
        .unwrap();
        // A one-quarter loop, on, so the clip plays the note four times over
        // the whole.
        song.apply(Command::SetMidiClipLoop {
            clip,
            start: Ticks::ZERO,
            end: Some(quarter),
        })
        .unwrap();
        song.apply(Command::SetClipLoopEnabled {
            clip,
            enabled: true,
        })
        .unwrap();
        let target = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        song.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target,
            kind: EnvelopeKind::Modulation,
            at: Ticks::ZERO,
            value: 0.25,
        })
        .unwrap();

        let start = song.doc.clip(clip).unwrap().start;
        let end = song.doc.clip(clip).unwrap().end();
        let new_id: ClipId = song.fresh();
        let source = song.doc.clip(clip).unwrap().clone();
        let made =
            crate::clip::consolidated_midi(new_id, "Merged".to_owned(), &[&source], start, end);

        assert_eq!(made.id, new_id);
        assert_eq!((made.start, made.length), (start, end - start));
        assert_eq!(made.name, "Merged");
        let ClipData::Midi(midi) = &made.data else {
            panic!("a MIDI clip");
        };
        assert_eq!(
            midi.notes.len(),
            4,
            "each repetition of the loop is written out"
        );
        assert_eq!(midi.notes[1].start, quarter);
        assert!(midi.looping, "its loop is on over its whole length");
        assert_eq!(midi.loop_start, Ticks::ZERO);
        assert_eq!(midi.loop_end, None);
        let envelope = made.envelopes.first().expect("the envelope came with it");
        assert_eq!(envelope.target, target);
        assert_eq!(envelope.kind, EnvelopeKind::Modulation);
        assert_eq!(envelope.envelope.value_at(Ticks::ZERO), Some(0.25));
    }
    // Spec: audio-clip-editing R9, R13. Reversing a clip mirrors its
    // markers and leaves its envelopes where they are, in one undo step.
    #[test]
    fn reversing_a_clip_leaves_its_envelopes_in_place() {
        let mut song = Song::new();
        let track = song.track(TrackKind::Audio, None);
        let clip = song.clip(track, 0);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        song.apply(Command::SetClipEnvelopeBreakpoint {
            kind: EnvelopeKind::Modulation,
            clip,
            target: volume,
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
        let envelopes = song.doc.clip(clip).unwrap().envelopes.clone();
        let before = song.doc.clip(clip).unwrap().clone();
        song.apply(Command::ReverseClip {
            clip,
            media: None,
            seconds: 4.0,
        })
        .unwrap();
        let after = song.doc.clip(clip).unwrap().clone();
        // Spec: R9. The envelopes stay where they are in the clip's time.
        assert_eq!(after.envelopes, envelopes);
        // Spec: R8. The markers are mirrored about the file.
        let ClipData::Audio(audio) = &after.data else {
            panic!("an audio clip");
        };
        assert_eq!(audio.start, 0.0);
        assert_eq!(audio.end, None);
        // The clip's other settings don't change.
        assert_eq!(after.name, before.name);
        assert_eq!(after.length, before.length);
    }
}
