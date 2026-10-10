//! Arrangement clips and their data.
//!
//! The clip data types are stubs: enough to place clips, refer to media, and
//! hold notes. WP-1.4 fills in audio clips (region, loop, gain, fades) and
//! WP-2.1 MIDI clips (velocity, probability, and the rest).

use bl_core::time::Ticks;
use imbl::Vector;
use serde::{Deserialize, Serialize};

use crate::ids::{ClipId, ContentHash};

/// A clip on a track's arrangement lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// The clip's ID.
    pub id: ClipId,
    /// The clip's name, which may be empty.
    pub name: String,
    /// Where the clip starts in the arrangement. Never negative.
    pub start: Ticks,
    /// How long the clip is. Always positive.
    pub length: Ticks,
    /// Whether the clip is switched off, so it stays in place but doesn't
    /// play.
    pub mute: bool,
    /// What the clip plays.
    pub data: ClipData,
}

impl Clip {
    /// An unnamed, unmuted clip.
    #[must_use]
    pub fn new(id: ClipId, start: Ticks, length: Ticks, data: ClipData) -> Self {
        Self {
            id,
            name: String::new(),
            start,
            length,
            mute: false,
            data,
        }
    }

    /// Where the clip ends: its start plus its length.
    #[must_use]
    pub fn end(&self) -> Ticks {
        self.start.saturating_add(self.length)
    }
}

/// What a clip plays. An audio clip goes only on an audio track and a MIDI
/// clip only on a MIDI track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipData {
    /// Audio from the media store.
    Audio(AudioData),
    /// MIDI notes.
    Midi(MidiData),
}

impl ClipData {
    /// Whether this is audio.
    #[must_use]
    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Audio(_))
    }
}

/// An audio clip's data (a stub until WP-1.4).
///
/// The samples stay in the media store; the clip holds only their hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioData {
    /// The media file the clip plays.
    pub media: ContentHash,
}

/// A MIDI clip's data (a stub until WP-2.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiData {
    /// The notes, in no particular order.
    pub notes: Vector<Note>,
}

impl MidiData {
    /// MIDI data holding these notes.
    pub fn new(notes: impl IntoIterator<Item = Note>) -> Self {
        Self {
            notes: notes.into_iter().collect(),
        }
    }
}

/// A note in a MIDI clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// The MIDI key, from 0 to 127.
    pub key: u8,
    /// The velocity, from 1 to 127.
    pub velocity: u8,
    /// Where the note starts, counted from the start of the clip. Never
    /// negative.
    pub start: Ticks,
    /// How long the note lasts. Always positive.
    pub length: Ticks,
}
