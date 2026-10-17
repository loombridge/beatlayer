//! Tracks and their mixer settings.

use core::fmt;
use std::collections::BTreeSet;

use imbl::{OrdMap, Vector};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{Automation, AutomationTarget, TrackAutomation};
use crate::clip::Clip;
use crate::device::Device;
use crate::ids::{ClipId, DeviceId, SceneId, TrackId};
use crate::shared::Shared;

/// The kind of a track, fixed when the track is created.
///
/// The rules for each kind are in the tracks and undo spec.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(description = "The kind of a track, fixed when the track is created.")]
pub enum TrackKind {
    /// Holds audio clips.
    Audio,
    /// Holds MIDI clips.
    Midi,
    /// Holds other tracks and sums their output.
    Group,
    /// Receives sends from other tracks.
    Return,
    /// The song's output. There is exactly one.
    Main,
}

impl TrackKind {
    /// Whether tracks of this kind hold clips: audio and MIDI tracks do.
    #[must_use]
    pub const fn holds_clips(self) -> bool {
        matches!(self, Self::Audio | Self::Midi)
    }

    /// Whether tracks of this kind can go inside a group: audio, MIDI, and
    /// group tracks can.
    #[must_use]
    pub const fn can_be_grouped(self) -> bool {
        matches!(self, Self::Audio | Self::Midi | Self::Group)
    }

    /// Whether tracks of this kind have mute and solo switches: every kind
    /// but Main does.
    #[must_use]
    pub const fn has_mute_and_solo(self) -> bool {
        !matches!(self, Self::Main)
    }

    /// Whether tracks of this kind have an arm switch: audio and MIDI tracks
    /// do.
    #[must_use]
    pub const fn has_arm(self) -> bool {
        matches!(self, Self::Audio | Self::Midi)
    }

    /// Whether tracks of this kind have a send to each return track: audio,
    /// MIDI, and group tracks do. (Live gives return tracks sends too,
    /// switched off until the user enables them; Beatlayer adds those with
    /// the track context menu, whose Enable Send turns them on.)
    #[must_use]
    pub const fn has_sends(self) -> bool {
        matches!(self, Self::Audio | Self::Midi | Self::Group)
    }

    /// Whether tracks of this kind have the crossfader's A/B assign
    /// buttons: every kind but Main, which carries the crossfader itself
    /// (the mixer spec, R23).
    #[must_use]
    pub const fn has_crossfade_assign(self) -> bool {
        !matches!(self, Self::Main)
    }
}

/// Which side of the crossfader a track is assigned to, through its A and B
/// assign buttons (the mixer spec, R23). The two sides are exclusive: a
/// track is on A, on B, or on neither, and one assigned to neither plays
/// through at every crossfader position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Which side of the crossfader a track is assigned to: `a`, `b`, or `none` \
                   (the default), where the track plays through at every crossfader position."
)]
pub enum CrossfadeAssign {
    /// On neither side: the crossfader doesn't change the track's level.
    #[default]
    None,
    /// On the crossfader's A side.
    A,
    /// On the crossfader's B side.
    B,
}

impl CrossfadeAssign {
    /// Whether this is [`CrossfadeAssign::None`], the default.
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
}

impl fmt::Display for TrackKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Audio => "audio",
            Self::Midi => "MIDI",
            Self::Group => "group",
            Self::Return => "return",
            Self::Main => "Main",
        })
    }
}

/// Where a return track's sends take their signal from the tracks that send
/// to it: the return's Pre/Post switch (the mixer spec).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Where a return track's sends take their signal from each sending track: \
                   `post` (the default) after the sending track's pan, volume, and activator, \
                   or `pre` before them, straight after its device chain."
)]
pub enum SendTap {
    /// After the sending track's pan, volume, and activator: the default.
    #[default]
    Post,
    /// Before them, straight after the sending track's device chain.
    Pre,
}

impl SendTap {
    /// Whether this is [`SendTap::Post`], the default.
    #[must_use]
    pub fn is_post(&self) -> bool {
        *self == Self::Post
    }
}

/// One of the audio interface's channels: a single channel or a stereo
/// pair, counted from 1 as Live's In/Out choosers show them (the routing
/// spec, WP-2.13).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "channel", rename_all = "snake_case")]
#[schemars(
    description = "One of the audio interface's channels: `mono` a single channel, or `pair` a \
                   stereo pair starting at it, counted from 1 (`{ \"type\": \"pair\", \
                   \"channel\": 3 }` is the pair 3/4)."
)]
pub enum Channel {
    /// A single channel, counted from 1.
    Mono(u16),
    /// A stereo pair: its first channel, counted from 1; the second is the
    /// next one.
    Pair(u16),
}

impl Default for Channel {
    /// The first stereo pair, `1/2`: what an interface's first channels are
    /// chosen as.
    fn default() -> Self {
        Self::Pair(1)
    }
}

impl Channel {
    /// Whether this is the default, `pair 1`.
    #[must_use]
    pub fn is_first_pair(&self) -> bool {
        *self == Self::Pair(1)
    }

    /// What Live's choosers show: `1` for one channel, `1/2` for a pair.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Mono(channel) => channel.to_string(),
            Self::Pair(channel) => format!("{channel}/{}", u32::from(channel) + 1),
        }
    }
}

/// Where a track takes its input from: the In/Out section's input type
/// chooser, Audio From on an audio track and MIDI From on a MIDI track, with
/// its channel (the routing spec).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "Where a track takes its input from. Audio tracks: `external` (Ext. In, the \
                   audio interface's inputs, with its `channel`; a new audio track's), \
                   `resampling` (the Main output), `track` (another audio track, a return, or \
                   Main, tapped at `tap`), or `none` (No Input). MIDI tracks: `all_ins` (every \
                   MIDI input; a new MIDI track's), `computer_keyboard`, `port` (one MIDI input, \
                   by its name as the MIDI settings list it), `track` (another MIDI track), or \
                   `none`."
)]
pub enum Input {
    /// No Input.
    None,
    /// Ext. In: the audio interface's inputs, one channel or a stereo pair.
    /// Audio tracks only.
    External {
        /// Which of the interface's inputs, counted from 1: a single channel
        /// or a stereo pair. The first pair (1/2) by default.
        #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
        channel: Channel,
    },
    /// All Ins: every MIDI input port whose Track switch is on, and the
    /// computer MIDI keyboard. MIDI tracks only.
    AllIns,
    /// The computer MIDI keyboard. MIDI tracks only.
    ComputerKeyboard,
    /// One MIDI input port, by the name the MIDI system gives it. MIDI
    /// tracks only. The track keeps the name while the port is away and
    /// receives again when a port of that name returns.
    Port {
        /// The port's name, as the MIDI settings list it.
        name: String,
    },
    /// Resampling: the Main output. Audio tracks only.
    Resampling,
    /// Another track's signal.
    Track {
        /// The track: for an audio track, another audio track, a return
        /// track, or Main; for a MIDI track, another MIDI track.
        track: TrackId,
        /// Where in that track the signal is taken: Post Mixer if it's left
        /// out.
        #[serde(default, skip_serializing_if = "Tap::is_post_mixer")]
        tap: Tap,
    },
}

/// Where a track's signal is taken when another track takes it as its
/// input: the input channel chooser's Pre FX, Post FX, and Post Mixer.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Where a track's signal is taken: `pre_fx` before its devices, `post_fx` \
                   after its devices and before its mixer, or `post_mixer` (the default) after \
                   its pan, volume, and activator."
)]
pub enum Tap {
    /// Before the track's devices and mixer.
    PreFx,
    /// After the track's devices, before its mixer.
    PostFx,
    /// After the track's mixer: its output. The default.
    #[default]
    PostMixer,
}

impl Tap {
    /// Whether this is [`Tap::PostMixer`], the default.
    #[must_use]
    pub fn is_post_mixer(&self) -> bool {
        *self == Self::PostMixer
    }
}

/// A track's Monitor switch: whether its input is heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Whether a track's input is heard: `in` always (its clips are silent), \
                   `auto` while the track is armed, or `off` never."
)]
pub enum Monitor {
    /// The input is always heard, and the track's clips are silent.
    In,
    /// The input is heard while the track is armed.
    Auto,
    /// The input is never heard.
    Off,
}

/// Where a track's audio goes: the In/Out section's Audio To chooser (the
/// routing spec).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "Where a track's audio goes: `default` (Main, or the group the track is \
                   in), `main` (Main, even from inside a group), `track` (another audio \
                   track's input, or the group the track is in), `sends_only` (only through \
                   its sends), or `external` (Ext. Out, the audio interface's outputs, with its \
                   `channel`: a single channel or a stereo pair, the first pair by default)."
)]
pub enum Output {
    /// Main, or the group the track is in: where a new track's audio goes.
    #[default]
    Default,
    /// Main, even from a track inside a group.
    Main,
    /// Ext. Out: the audio interface's outputs. The track plays on those
    /// device channels and not into Main (WP-2.13).
    External {
        /// Which of the interface's outputs, counted from 1: a single
        /// channel or a stereo pair. The first pair (1/2) by default.
        #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
        channel: Channel,
    },
    /// Nowhere but the track's sends.
    SendsOnly,
    /// Another track: an audio track's input (Track In), or the group the
    /// track is in.
    Track {
        /// The track.
        track: TrackId,
    },
}

impl Output {
    /// Whether this is [`Output::Default`].
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::Default
    }
}

/// A track's In/Out settings: its input and its channel, Monitor, and its
/// output (the routing spec).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Routing {
    /// Audio From or MIDI From.
    pub input: Input,
    /// The input channel: the MIDI channel to take, 1 to 16, or `None` for
    /// All Channels. Only a MIDI port input has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
    /// The Monitor switch.
    pub monitor: Monitor,
    /// Audio To.
    #[serde(default, skip_serializing_if = "Output::is_default")]
    pub output: Output,
    /// MIDI To, on a MIDI track: another MIDI track, or none (No Output,
    /// a new MIDI track's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_output: Option<TrackId>,
    /// Main Out, on Main: the interface's output channels the Main mix goes
    /// to, as a stereo pair. Saved only where it differs from the first
    /// pair.
    #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
    pub main_out: Channel,
    /// Cue Out, on Main: the interface's output channels the cue mix goes
    /// to, as a stereo pair. It carries the cue mix once cue mode is built
    /// (a later package); the choice is kept and saved with the set.
    #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
    pub cue_out: Channel,
}

impl Routing {
    /// A new track's routing: an audio track takes Ext. In, a MIDI track
    /// All Ins on All Channels, both with Monitor Auto, and both go to Main
    /// (or their group); MIDI To is No Output. Other kinds take no input.
    /// Main Out and Cue Out are the interface's first pair.
    #[must_use]
    pub const fn new(kind: TrackKind) -> Self {
        let (input, monitor) = match kind {
            TrackKind::Audio => (
                Input::External {
                    channel: Channel::Pair(1),
                },
                Monitor::Auto,
            ),
            TrackKind::Midi => (Input::AllIns, Monitor::Auto),
            _ => (Input::None, Monitor::Off),
        };
        Self {
            input,
            channel: None,
            monitor,
            output: Output::Default,
            midi_output: None,
            main_out: Channel::Pair(1),
            cue_out: Channel::Pair(1),
        }
    }

    /// Whether the input is heard, given the Arm switch.
    #[must_use]
    pub const fn monitoring(&self, armed: bool) -> bool {
        match self.monitor {
            Monitor::In => true,
            Monitor::Auto => armed,
            Monitor::Off => false,
        }
    }
}

/// A track's mixer settings: its volume, pan, and switches. Its sends are
/// [`Track::sends`]. The rules are in the mixer spec.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mixer {
    /// Linear gain: 1.0 is unity (0 dB) and 0.0 is silence (−∞ dB). Finite
    /// and never negative.
    pub volume: f64,
    /// Stereo position from −1.0 (left) to 1.0 (right).
    pub pan: f64,
    /// Whether the track is muted: its Track Activator is off. Always off on
    /// the Main track.
    pub mute: bool,
    /// Whether the track is soloed. Always off on the Main track.
    pub solo: bool,
    /// Whether the track is armed for recording. Only audio and MIDI tracks
    /// can be armed.
    pub arm: bool,
    /// Which side of the crossfader the track plays on, set by its A and B
    /// assign buttons; [`CrossfadeAssign::None`] (the default) plays through
    /// at every position. Always `None` on the Main track, which carries
    /// the crossfader itself (the mixer spec, R23). Saved only when it's
    /// assigned, so a set saved before WP-3.17 loads unchanged.
    #[serde(default, skip_serializing_if = "CrossfadeAssign::is_none")]
    pub crossfade: CrossfadeAssign,
    /// Where sends to this track take their signal from: only a return
    /// track's can be [`SendTap::Pre`]. Saved only when it's `Pre`, so a set
    /// saved before WP-1.6 loads unchanged.
    #[serde(default, skip_serializing_if = "SendTap::is_post")]
    pub tap: SendTap,
}

impl Default for Mixer {
    /// Unity gain, centred, every switch off, unassigned on the crossfader,
    /// and sends taken after the mixer.
    fn default() -> Self {
        Self {
            volume: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            arm: false,
            crossfade: CrossfadeAssign::None,
            tap: SendTap::Post,
        }
    }
}

/// A track: its mixer, sends, and automation, its device chain, and either
/// its clips (audio and MIDI tracks) or the tracks inside it (groups).
///
/// Tracks change only through commands, so this type has no setters.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub(crate) id: TrackId,
    pub(crate) kind: TrackKind,
    pub(crate) name: String,
    pub(crate) mixer: Mixer,
    /// The level of each send that isn't at −∞ dB, by return track: a linear
    /// gain above 0 and at most 1. A send that isn't listed sends nothing.
    pub(crate) sends: OrdMap<TrackId, f64>,
    pub(crate) routing: Routing,
    pub(crate) devices: Vector<Shared<Device>>,
    pub(crate) clips: OrdMap<ClipId, Shared<Clip>>,
    /// The clips in the track's Session slots, by scene. A slot that holds
    /// nothing isn't listed: every audio and MIDI track has a slot in every
    /// scene, and a slot with no entry is empty. Return, group, and Main
    /// tracks have none.
    pub(crate) slots: OrdMap<SceneId, Shared<Clip>>,
    /// The scenes whose slot's Clip Stop button was removed (WP-3.2, C-SLOT-006,
    /// S-SES-005): every audio and MIDI slot has one until then. Return,
    /// group, and Main tracks have none.
    pub(crate) removed_stop_buttons: BTreeSet<SceneId>,
    pub(crate) children: Vector<TrackId>,
    /// One envelope per automated parameter of this track (WP-2.12): a
    /// control of its mixer, one of its sends, or a parameter of a device in
    /// its chain.
    pub(crate) automation: TrackAutomation,
}

impl Track {
    pub(crate) fn new(id: TrackId, kind: TrackKind, name: String) -> Self {
        Self {
            id,
            kind,
            name,
            mixer: Mixer::default(),
            sends: OrdMap::new(),
            routing: Routing::new(kind),
            devices: Vector::new(),
            clips: OrdMap::new(),
            slots: OrdMap::new(),
            removed_stop_buttons: BTreeSet::new(),
            children: Vector::new(),
            automation: TrackAutomation::new(),
        }
    }

    /// The track's ID.
    #[must_use]
    pub fn id(&self) -> TrackId {
        self.id
    }

    /// The track's kind.
    #[must_use]
    pub fn kind(&self) -> TrackKind {
        self.kind
    }

    /// The track's name, which may be empty.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The track's mixer settings.
    #[must_use]
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// The track's sends that aren't at −∞ dB, as (return track, level) in
    /// the order of the returns' IDs. Each level is a linear gain above 0
    /// and at most 1 (0 dB). Audio, MIDI, and group tracks have a send to
    /// every return track; one that isn't listed is at −∞ dB, where every
    /// send starts.
    pub fn sends(&self) -> impl ExactSizeIterator<Item = (TrackId, f64)> + '_ {
        self.sends.iter().map(|(&to, &level)| (to, level))
    }

    /// The level of the track's send to the return track `to`, as a linear
    /// gain: 0 (−∞ dB) unless it has been turned up.
    #[must_use]
    pub fn send(&self, to: TrackId) -> f64 {
        self.sends.get(&to).copied().unwrap_or(0.0)
    }

    /// The track's In/Out settings.
    #[must_use]
    pub fn routing(&self) -> &Routing {
        &self.routing
    }

    /// The devices in the track's chain, in signal order.
    pub fn devices(&self) -> impl ExactSizeIterator<Item = &Device> + DoubleEndedIterator {
        self.devices.iter().map(|device| &**device)
    }

    /// Where a device sits in the chain, counting from 0.
    #[must_use]
    pub fn device_position(&self, device: DeviceId) -> Option<usize> {
        self.devices.iter().position(|d| d.id == device)
    }

    /// The track's clips, ordered by ID: those on its Arrangement lane.
    /// Only audio and MIDI tracks have clips.
    pub fn clips(&self) -> impl ExactSizeIterator<Item = &Clip> + DoubleEndedIterator {
        self.clips.values().map(|clip| &**clip)
    }

    /// The clips in the track's Session slots, as `(scene, clip)` pairs
    /// ordered by scene ID. Only audio and MIDI tracks hold them.
    pub fn slots(&self) -> impl ExactSizeIterator<Item = (SceneId, &Clip)> + '_ {
        self.slots.iter().map(|(&scene, clip)| (scene, &**clip))
    }

    /// The clip in the track's slot at `scene`, if the slot holds one.
    #[must_use]
    pub fn slot(&self, scene: SceneId) -> Option<&Clip> {
        self.slots.get(&scene).map(|clip| &**clip)
    }

    /// Whether the slot at `scene` has a Clip Stop button (WP-3.2,
    /// S-SES-005): every audio and MIDI slot has one until it's removed.
    #[must_use]
    pub fn has_stop_button(&self, scene: SceneId) -> bool {
        self.kind.holds_clips() && !self.removed_stop_buttons.contains(&scene)
    }

    /// The scenes whose slots' Clip Stop buttons were removed, in order.
    pub fn removed_stop_buttons(&self) -> impl ExactSizeIterator<Item = SceneId> + '_ {
        self.removed_stop_buttons.iter().copied()
    }

    /// The clip with this ID, whether it sits on the Arrangement lane or in
    /// a Session slot.
    #[must_use]
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips
            .get(&id)
            .or_else(|| self.slots.values().find(|clip| clip.id == id))
            .map(|clip| &**clip)
    }

    /// The clip with this ID, to change in place, whether it sits on the
    /// Arrangement lane or in a Session slot. The track's share of the clip
    /// is copied out of the document it came from first, so editing it
    /// leaves that version as it was.
    pub(crate) fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        if let Some(clip) = self.clips.get_mut(&id) {
            return Some(Shared::make_mut(clip));
        }
        let scene = self
            .slots
            .iter()
            .find(|(_, clip)| clip.id == id)
            .map(|(&scene, _)| scene)?;
        self.slots.get_mut(&scene).map(Shared::make_mut)
    }

    /// The tracks directly inside this one, in order. Only groups have them.
    pub fn children(&self) -> impl ExactSizeIterator<Item = TrackId> + DoubleEndedIterator + '_ {
        self.children.iter().copied()
    }

    /// The track's automation: one entry per automated parameter, in target
    /// order.
    pub fn automation(
        &self,
    ) -> impl ExactSizeIterator<Item = (&AutomationTarget, &Automation)> + '_ {
        self.automation.iter()
    }

    /// The track's automation, keyed by the parameter it drives.
    #[must_use]
    pub fn automation_map(&self) -> &TrackAutomation {
        &self.automation
    }

    /// The automation of one parameter of the track, if it has any.
    #[must_use]
    pub fn automation_for(&self, target: &AutomationTarget) -> Option<&Automation> {
        self.automation.get(target)
    }

    /// Whether any of the track's envelopes is turned off: a control was
    /// moved by hand and Re-Enable Automation hasn't run since.
    #[must_use]
    pub fn automation_overridden(&self) -> bool {
        self.automation
            .values()
            .any(|automation| !automation.enabled)
    }

    /// Whether the track's own settings differ from another version's:
    /// its name, mixer, sends, routing, children, automation, or the order
    /// of its device chain, but not the clips or devices themselves.
    pub(crate) fn own_settings_differ(&self, other: &Self) -> bool {
        self.kind != other.kind
            || self.name != other.name
            || self.mixer != other.mixer
            || self.routing != other.routing
            || self.removed_stop_buttons != other.removed_stop_buttons
            || !(self.sends.ptr_eq(&other.sends) || self.sends == other.sends)
            || !(self.children.ptr_eq(&other.children) || self.children == other.children)
            || !(self.automation.ptr_eq(&other.automation) || self.automation == other.automation)
            || !(self.devices.ptr_eq(&other.devices)
                || self
                    .devices
                    .iter()
                    .map(|d| d.id)
                    .eq(other.devices.iter().map(|d| d.id)))
    }
}
