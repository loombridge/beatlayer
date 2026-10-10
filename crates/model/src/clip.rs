//! Arrangement clips and their data.
//!
//! The clip data types are stubs: enough to place clips, refer to media, and
//! hold notes. WP-1.4 fills in audio clips (region, loop, gain, fades) and
//! WP-2.1 MIDI clips (velocity, probability, and the rest).
//!
//! A clip shows a window onto its contents: its contents begin
//! [`Clip::offset`] before the clip starts, so splitting a clip or trimming
//! its left edge (WP-1.8b) changes what the clip shows without changing
//! where anything plays.

use bl_core::time::Ticks;
use imbl::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{ClipId, ContentHash};

/// A clip on a track's arrangement lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Clip {
    /// The clip's ID.
    pub id: ClipId,
    /// The clip's name, which may be empty.
    pub name: String,
    /// Where the clip starts in the arrangement. Never negative.
    pub start: Ticks,
    /// How long the clip is. Always positive.
    pub length: Ticks,
    /// How far into its contents the clip starts: its contents begin this
    /// long before the clip, at `start - offset`, and the part before the
    /// clip's start doesn't play. 0 (or left out) plays the contents from
    /// their beginning. Never negative. Splitting a clip or trimming its
    /// left edge changes it, so each part plays what it played before.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: Ticks,
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
            offset: Ticks::ZERO,
            mute: false,
            data,
        }
    }

    /// Where the clip ends: its start plus its length.
    #[must_use]
    pub fn end(&self) -> Ticks {
        self.start.saturating_add(self.length)
    }

    /// Where the clip's contents begin in the arrangement: its start less
    /// its offset. An audio clip's media plays from its first frame at this
    /// position, at its own speed; a MIDI clip's notes are placed from it.
    #[must_use]
    pub fn content_start(&self) -> Ticks {
        self.start.saturating_sub(self.offset)
    }
}

/// Whether a position is zero, so a zero offset is left out of saved sets.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_zero(ticks: &Ticks) -> bool {
    *ticks == Ticks::ZERO
}

/// What a clip plays. An audio clip goes only on an audio track and a MIDI
/// clip only on a MIDI track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "An audio clip's data. The samples stay in the media store; the clip names \
                   them by content hash."
)]
pub struct AudioData {
    /// The media file the clip plays.
    pub media: ContentHash,
}

/// A MIDI clip's data (a stub until WP-2.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "A MIDI clip's data: its notes.")]
pub struct MidiData {
    /// The notes, in no particular order.
    #[schemars(with = "Vec<Note>")]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Note {
    /// The MIDI key, from 0 to 127. Middle C (C3 in Live) is 60.
    #[schemars(range(max = 127))]
    pub key: u8,
    /// The velocity, from 1 to 127.
    #[schemars(range(min = 1, max = 127))]
    pub velocity: u8,
    /// Where the note starts, counted from the beginning of the clip's
    /// contents: the clip's start less its offset. Never negative.
    pub start: Ticks,
    /// How long the note lasts. Always positive.
    pub length: Ticks,
}
