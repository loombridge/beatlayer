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
/// The rules for each kind are in `docs/specs/tracks-and-undo.md`.
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
    /// WP-2.19.)
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
/// to it: the return's Pre/Post switch (`docs/specs/mixer.md`).
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

/// A track's mixer settings: its volume, pan, and switches. Its sends are
/// [`Track::sends`]. The rules are in `docs/specs/mixer.md`.
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
    /// its name, mixer, sends, children, or the order of its device chain,
    /// but not the clips or devices themselves.
    pub(crate) fn own_settings_differ(&self, other: &Self) -> bool {
        self.kind != other.kind
            || self.name != other.name
            || self.mixer != other.mixer
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
