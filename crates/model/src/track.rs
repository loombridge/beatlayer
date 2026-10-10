//! Tracks and their mixer settings.

use core::fmt;

use imbl::{OrdMap, Vector};
use serde::{Deserialize, Serialize};

use crate::clip::Clip;
use crate::device::Device;
use crate::ids::{ClipId, DeviceId, TrackId};
use crate::shared::Shared;

/// The kind of a track, fixed when the track is created.
///
/// The rules for each kind are in `docs/specs/tracks-and-undo.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// A track's mixer settings (a stub until WP-1.6).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mixer {
    /// Linear gain: 1.0 is unity (0 dB) and 0.0 is silence. Finite and never
    /// negative.
    pub volume: f64,
    /// Stereo position from −1.0 (left) to 1.0 (right).
    pub pan: f64,
    /// Whether the track is muted. Always off on the Main track.
    pub mute: bool,
    /// Whether the track is soloed. Always off on the Main track.
    pub solo: bool,
    /// Whether the track is armed for recording. Only audio and MIDI tracks
    /// can be armed.
    pub arm: bool,
}

impl Default for Mixer {
    /// Unity gain, centred, and every switch off.
    fn default() -> Self {
        Self {
            volume: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            arm: false,
        }
    }
}

/// A track: its mixer, its device chain, and either its clips (audio and
/// MIDI tracks) or the tracks inside it (groups).
///
/// Tracks change only through commands, so this type has no setters.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub(crate) id: TrackId,
    pub(crate) kind: TrackKind,
    pub(crate) name: String,
    pub(crate) mixer: Mixer,
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
    /// its name, mixer, children, or the order of its device chain, but not
    /// the clips or devices themselves.
    pub(crate) fn own_settings_differ(&self, other: &Self) -> bool {
        self.kind != other.kind
            || self.name != other.name
            || self.mixer != other.mixer
            || !(self.children.ptr_eq(&other.children) || self.children == other.children)
            || !(self.devices.ptr_eq(&other.devices)
                || self
                    .devices
                    .iter()
                    .map(|d| d.id)
                    .eq(other.devices.iter().map(|d| d.id)))
    }
}
