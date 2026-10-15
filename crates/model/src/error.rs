//! Why a command or a loaded document was rejected.

use core::fmt;

use bl_core::id::Id;
use bl_core::time::Ticks;

use crate::device::DeviceRole;
use crate::ids::{ClipId, DeviceId, SceneId, TrackId};
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
    /// No scene has this ID.
    NoSuchScene(SceneId),
    /// The device has no sidechain input to set.
    NoSidechain(DeviceId),
    /// The device is a plug-in, which opens its own editor window rather
    /// than floating (IMP-14).
    CannotFloat(DeviceId),
    /// Another object in the document already has this ID.
    IdInUse(Id),
    /// The same ID appears twice where each must be different.
    DuplicateId(Id),
    /// A track's Session slot already holds a clip, and the command would
    /// put another one there.
    SlotOccupied {
        /// The track.
        track: TrackId,
        /// The scene whose slot it is.
        scene: SceneId,
    },
    /// A clip meant for a Session slot has a position in the Arrangement. A
    /// slot clip has none: its start is 0.
    SlotClipPosition(Ticks),
    /// The command places a clip in the Arrangement, and this clip sits in a
    /// Session slot, which has no place there.
    ClipInSlot(ClipId),
    /// The copies a scene duplicate lists don't match the clips the scene
    /// holds: this track either holds no clip in it, or holds one that no
    /// copy is listed for.
    SceneCopyMismatch {
        /// The scene being duplicated.
        scene: SceneId,
        /// The track whose clip doesn't match.
        track: TrackId,
    },
    /// A loaded document's IDs leave no room for the objects a later version
    /// adds, such as the default scenes.
    IdsExhausted,
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
    /// Main Out and Cue Out take a stereo pair of the audio interface's
    /// outputs, not a single channel.
    NotStereoPair,
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
    /// The command changes a MIDI clip's settings, and this clip is an audio
    /// clip.
    NotMidiClip(ClipId),
    /// A track can't take its input from, or send its output to, this
    /// track: itself, or a track of a kind its chooser doesn't list.
    CannotRoute {
        /// The track whose routing changes.
        track: TrackId,
        /// The track it would route from or to.
        to: TrackId,
    },
    /// A track can't hold a device with this role: MIDI effects and
    /// instruments go only on MIDI tracks.
    DeviceDoesNotFit {
        /// The track.
        track: TrackId,
        /// Its kind.
        kind: TrackKind,
        /// The device's role.
        role: DeviceRole,
    },
    /// A chain's devices would be out of Live's order (MIDI effects, then
    /// the instrument, then audio effects), or it would hold a second
    /// instrument.
    DeviceOrder {
        /// The track whose chain it is.
        track: TrackId,
    },
    /// There's no tempo breakpoint or time-signature change at this
    /// position.
    NoChange {
        /// What's missing, such as "tempo breakpoint".
        what: &'static str,
        /// The position asked about.
        at: Ticks,
    },
    /// Another tempo breakpoint or time-signature change is already at this
    /// position.
    ChangeInUse {
        /// What would collide, such as "tempo breakpoint".
        what: &'static str,
        /// The position asked for.
        at: Ticks,
    },
    /// The first tempo breakpoint or time-signature change sits at 1.1.1
    /// and can't be moved or deleted.
    FirstChange {
        /// What can't move, such as "tempo breakpoint".
        what: &'static str,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchTrack(id) => write!(f, "no {id}"),
            Self::NoSuchClip(id) => write!(f, "no {id}"),
            Self::NoSuchDevice(id) => write!(f, "no {id}"),
            Self::NoSuchScene(id) => write!(f, "no {id}"),
            Self::SlotOccupied { track, scene } => {
                write!(f, "{track}'s slot in {scene} already holds a clip")
            }
            Self::SlotClipPosition(at) => write!(
                f,
                "a slot clip has no position in the Arrangement: its start must be 0, not {at}"
            ),
            Self::ClipInSlot(id) => write!(
                f,
                "{id} sits in a Session slot, which has no place in the Arrangement"
            ),
            Self::SceneCopyMismatch { scene, track } => write!(
                f,
                "the copies listed for {scene} don't match the clip {track} holds in it"
            ),
            Self::IdsExhausted => {
                f.write_str("the document's object IDs leave no room for the objects it needs")
            }
            Self::NoSidechain(id) => write!(f, "{id} has no sidechain input"),
            Self::CannotFloat(id) => write!(
                f,
                "{id} is a plug-in, which opens its own editor window; only native devices float"
            ),
            Self::IdInUse(id) => write!(f, "ID {id} is already in use"),
            Self::DuplicateId(id) => write!(f, "ID {id} appears more than once"),
            Self::WrongTrackKind { track, kind, needs } => {
                write!(f, "{track} is a {kind} track; this needs {needs}")
            }
            Self::MainTrack => f.write_str(
                "the Main track can't be created, deleted, or moved, and there must be exactly one",
            ),
            Self::NotStereoPair => {
                f.write_str("Main Out and Cue Out take a stereo pair of the interface's outputs")
            }
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
            Self::NotMidiClip(id) => write!(f, "{id} is an audio clip; this needs a MIDI clip"),
            Self::CannotRoute { track, to } => write!(
                f,
                "{track} can't take its input from or send its output to {to}: its choosers \
                 don't list it"
            ),
            Self::DeviceDoesNotFit { track, kind, role } => write!(
                f,
                "{track} is a {kind} track, which can't hold a{} {}; only MIDI tracks hold \
                 MIDI effects and instruments",
                if *role == DeviceRole::AudioEffect || *role == DeviceRole::Instrument {
                    "n"
                } else {
                    ""
                },
                role.label().to_lowercase().replace("midi", "MIDI")
            ),
            Self::DeviceOrder { track } => write!(
                f,
                "{track}'s chain must hold its MIDI effects first, then at most one instrument, \
                 then its audio effects"
            ),
            Self::NoChange { what, at } => {
                write!(f, "there's no {what} at {at}")
            }
            Self::ChangeInUse { what, at } => {
                write!(f, "there's already a {what} at {at}")
            }
            Self::FirstChange { what } => write!(
                f,
                "the first {what} sits at 1.1.1 and can't be moved or deleted"
            ),
        }
    }
}

impl std::error::Error for ModelError {}
