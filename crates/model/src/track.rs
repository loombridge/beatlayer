//! Tracks and their mixer settings.

use core::fmt;

use imbl::{OrdMap, Vector};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::Clip;
use crate::device::Device;
use crate::ids::{ClipId, DeviceId, TrackId};
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

/// Where a track takes its input from: the In/Out section's input type
/// chooser, Audio From on an audio track and MIDI From on a MIDI track, with
/// its channel (the routing spec).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "Where a track takes its input from. Audio tracks: `external` (Ext. In, the \
                   audio interface's inputs; a new audio track's), `resampling` (the Main \
                   output), `track` (another audio track, a return, or Main, tapped at `tap`), \
                   or `none` (No Input). MIDI tracks: `all_ins` (every MIDI input; a new MIDI \
                   track's), `computer_keyboard`, `port` (one MIDI input, by its name as the \
                   MIDI settings list it), `track` (another MIDI track), or `none`."
)]
pub enum Input {
    /// No Input.
    None,
    /// Ext. In: the audio interface's inputs. Audio tracks only; it carries
    /// silence until duplex audio (WP-2.13).
    External,
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
                   its sends), or `external` (Ext. Out)."
)]
pub enum Output {
    /// Main, or the group the track is in: where a new track's audio goes.
    #[default]
    Default,
    /// Main, even from a track inside a group.
    Main,
    /// Ext. Out: the audio interface's outputs. It carries nothing until
    /// external outputs arrive (WP-2.13).
    External,
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
}

impl Routing {
    /// A new track's routing: an audio track takes Ext. In, a MIDI track
    /// All Ins on All Channels, both with Monitor Auto, and both go to Main
    /// (or their group); MIDI To is No Output. Other kinds take no input.
    #[must_use]
    pub const fn new(kind: TrackKind) -> Self {
        let (input, monitor) = match kind {
            TrackKind::Audio => (Input::External, Monitor::Auto),
            TrackKind::Midi => (Input::AllIns, Monitor::Auto),
            _ => (Input::None, Monitor::Off),
        };
        Self {
            input,
            channel: None,
            monitor,
            output: Output::Default,
            midi_output: None,
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
    /// Where sends to this track take their signal from: only a return
    /// track's can be [`SendTap::Pre`]. Saved only when it's `Pre`, so a set
    /// saved before WP-1.6 loads unchanged.
    #[serde(default, skip_serializing_if = "SendTap::is_post")]
    pub tap: SendTap,
}

impl Default for Mixer {
    /// Unity gain, centred, every switch off, and sends taken after the
    /// mixer.
    fn default() -> Self {
        Self {
            volume: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            arm: false,
            tap: SendTap::Post,
        }
    }
}

/// A track: its mixer and sends, its device chain, and either its clips
/// (audio and MIDI tracks) or the tracks inside it (groups).
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
    pub(crate) children: Vector<TrackId>,
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
            children: Vector::new(),
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

    /// The track's clips, ordered by ID. Only audio and MIDI tracks have
    /// clips.
    pub fn clips(&self) -> impl ExactSizeIterator<Item = &Clip> + DoubleEndedIterator {
        self.clips.values().map(|clip| &**clip)
    }

    /// The tracks directly inside this one, in order. Only groups have them.
    pub fn children(&self) -> impl ExactSizeIterator<Item = TrackId> + DoubleEndedIterator + '_ {
        self.children.iter().copied()
    }

    /// Whether the track's own settings differ from another version's:
    /// its name, mixer, sends, routing, children, or the order of its device chain,
    /// but not the clips or devices themselves.
    pub(crate) fn own_settings_differ(&self, other: &Self) -> bool {
        self.kind != other.kind
            || self.name != other.name
            || self.mixer != other.mixer
            || self.routing != other.routing
            || !(self.sends.ptr_eq(&other.sends) || self.sends == other.sends)
            || !(self.children.ptr_eq(&other.children) || self.children == other.children)
            || !(self.devices.ptr_eq(&other.devices)
                || self
                    .devices
                    .iter()
                    .map(|d| d.id)
                    .eq(other.devices.iter().map(|d| d.id)))
    }
}
