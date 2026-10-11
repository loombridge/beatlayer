//! Why a command or a loaded document was rejected.

use core::fmt;

use bl_core::id::Id;
use bl_core::time::Ticks;

use crate::ids::{ClipId, DeviceId, TrackId};
use crate::track::TrackKind;

/// Why a command was rejected, or why a loaded document isn't valid.
///
/// A rejected command leaves the document as it was.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ModelError {
    /// No track has this ID.
    NoSuchTrack(TrackId),
    /// No clip has this ID.
    NoSuchClip(ClipId),
    /// No device has this ID.
    NoSuchDevice(DeviceId),
    /// Another object in the document already has this ID.
    IdInUse(Id),
    /// The same ID appears twice where each must be different.
    DuplicateId(Id),
    /// The track's kind doesn't allow this.
    WrongTrackKind {
        /// The track.
        track: TrackId,
        /// Its kind.
        kind: TrackKind,
        /// The kinds that would be allowed, such as "an audio or MIDI track".
        needs: &'static str,
    },
    /// The Main track can't be created, deleted, or moved, and a document
    /// has exactly one.
    MainTrack,
    /// Tracks of this kind can't go inside a group: only audio, MIDI, and
    /// group tracks can.
    CannotGroup {
        /// The kind.
        kind: TrackKind,
    },
    /// A group can't go inside itself or inside a group within it.
    GroupCycle {
        /// The group being moved.
        track: TrackId,
        /// The group it would go into.
        parent: TrackId,
    },
    /// An insertion index is past the end of the list.
    IndexOutOfRange {
        /// The index given.
        index: usize,
        /// The length of the list.
        len: usize,
    },
    /// A clip or note would start before zero.
    NegativePosition(Ticks),
    /// A clip or note length isn't positive.
    EmptyLength(Ticks),
    /// A clip would start before its contents begin: its offset into them
    /// would be negative.
    NegativeOffset(Ticks),
    /// An audio clip's left edge would move so far earlier that its Start
    /// marker would go this many seconds before the start of its file.
    BeforeFileStart(f64),
    /// An audio clip has an offset into its contents. Its Start marker says
    /// where it starts playing, so its offset is always 0.
    AudioClipOffset(Ticks),
    /// A position or length is too large to represent.
    PositionOverflow,
    /// A number is out of range or not finite.
    InvalidValue {
        /// What the number is, such as "volume".
        what: &'static str,
        /// The number given.
        value: f64,
    },
    /// A note's key or velocity is outside 0 to 127 (velocity 1 to 127).
    InvalidNote {
        /// The note's index in the clip's note list.
        index: usize,
    },
    /// The command changes an audio clip's settings, and this clip is a MIDI
    /// clip.
    NotAudioClip(ClipId),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchTrack(id) => write!(f, "no {id}"),
            Self::NoSuchClip(id) => write!(f, "no {id}"),
            Self::NoSuchDevice(id) => write!(f, "no {id}"),
            Self::IdInUse(id) => write!(f, "ID {id} is already in use"),
            Self::DuplicateId(id) => write!(f, "ID {id} appears more than once"),
            Self::WrongTrackKind { track, kind, needs } => {
                write!(f, "{track} is a {kind} track; this needs {needs}")
            }
            Self::MainTrack => f.write_str(
                "the Main track can't be created, deleted, or moved, and there must be exactly one",
            ),
            Self::CannotGroup { kind } => write!(f, "a {kind} track can't go inside a group"),
            Self::GroupCycle { track, parent } => {
                write!(f, "{track} can't go inside {parent}, which is inside it")
            }
            Self::IndexOutOfRange { index, len } => {
                write!(f, "index {index} is past the end of a list of {len}")
            }
            Self::NegativePosition(at) => write!(f, "position {at} is before zero"),
            Self::EmptyLength(length) => write!(f, "length {length} isn't positive"),
            Self::NegativeOffset(offset) => write!(
                f,
                "the clip would start {} ticks before its contents begin",
                offset.get().unsigned_abs()
            ),
            Self::BeforeFileStart(seconds) => write!(
                f,
                "the clip would start {seconds} seconds before the start of its file"
            ),
            Self::AudioClipOffset(offset) => write!(
                f,
                "an audio clip's offset must be 0, not {} ticks: its Start marker says where in \
                 its file it starts playing",
                offset.get()
            ),
            Self::PositionOverflow => f.write_str("position is too large"),
            Self::InvalidValue { what, value } => write!(f, "{what} {value} is out of range"),
            Self::InvalidNote { index } => {
                write!(f, "note {index} has a key or velocity out of range")
            }
            Self::NotAudioClip(id) => write!(f, "{id} is a MIDI clip; this needs an audio clip"),
        }
    }
}

impl std::error::Error for ModelError {}
