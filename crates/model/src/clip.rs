//! Arrangement clips and their data.
//!
//! An audio clip's data is its file, its Start and End markers, its loop,
//! its gain, and its edge fades (WP-1.4, the audio clips spec). MIDI
//! clip data is still a stub, enough to hold notes, until WP-2.1 adds
//! velocity, probability, and the rest.

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

/// The length of the fades Create Fades on Clip Edges gives the edges of a
/// new audio clip, and the length a deleted fade goes back to while that
/// setting is on: 4 ms (the audio clips spec, R7).
pub const EDGE_FADE_SECONDS: f64 = 0.004;

/// An audio clip's data: the file it plays and how it plays it
/// (the audio clips spec).
///
/// The samples stay in the media store; the clip names them by content hash.
/// Until warping (Phase 4), a clip plays unwarped, at its file's own speed,
/// so its markers and loop brace are positions in the file in seconds, as
/// Live shows them for an unwarped clip. A clip starts playing at its Start
/// marker. With the loop off, it plays to its End marker or its own end in
/// the Arrangement, whichever comes first; with the loop on, it plays into
/// the loop brace and repeats it until the clip ends.
///
/// Every field but `media` has a default, a new clip's, and is saved only
/// when it differs from it, so a set saved before WP-1.4, whose audio clips
/// hold only `media`, loads unchanged and plays each file whole.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "An audio clip's data: the audio file it plays, named by content hash, and \
                   how it plays it. Times are seconds into the file. Every field but `media` \
                   is optional: a new clip plays the whole file once, at 0 dB, with no fades."
)]
pub struct AudioData {
    /// The media file the clip plays, by content hash.
    pub media: ContentHash,
    /// The Start marker: where in the file, in seconds, the clip starts
    /// playing. 0 or more; 0 unless given.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0))]
    pub start: f64,
    /// The End marker: where in the file, in seconds, a clip whose loop is
    /// off stops playing, after the Start marker; `null` (or nothing) for
    /// the end of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    /// The Loop switch: whether the clip repeats its loop brace. Off unless
    /// given.
    #[serde(default, skip_serializing_if = "is_false")]
    pub looping: bool,
    /// Where the loop brace starts in the file, in seconds. 0 or more; 0
    /// unless given.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0))]
    pub loop_start: f64,
    /// Where the loop brace ends in the file, in seconds, after its start;
    /// `null` (or nothing) for the end of the file. While the loop is on,
    /// the Start marker must be before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_end: Option<f64>,
    /// The clip's gain, as a linear gain: 1 (0 dB) unless given, 0.5 about
    /// −6 dB, and 0 silence (−∞ dB). Finite and 0 or more.
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    #[schemars(range(min = 0.0))]
    pub gain: f64,
    /// The fade-in's length in seconds, from the clip's start in the
    /// Arrangement: 0 (no fade) unless given.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0))]
    pub fade_in: f64,
    /// The fade-out's length in seconds, up to the clip's end in the
    /// Arrangement: 0 (no fade) unless given.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0))]
    pub fade_out: f64,
}

impl AudioData {
    /// A new clip of the whole file `media`: from the file's start to its
    /// end, the loop off with its brace on the whole file, at 0 dB, with no
    /// fades.
    #[must_use]
    pub fn new(media: ContentHash) -> Self {
        Self {
            media,
            start: 0.0,
            end: None,
            looping: false,
            loop_start: 0.0,
            loop_end: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
        }
    }

    /// The same data with [`EDGE_FADE_SECONDS`] fades at both edges: what a
    /// new clip gets while Create Fades on Clip Edges is on
    /// (the audio clips spec, R7).
    #[must_use]
    pub fn with_edge_fades(self) -> Self {
        Self {
            fade_in: EDGE_FADE_SECONDS,
            fade_out: EDGE_FADE_SECONDS,
            ..self
        }
    }
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn unity() -> f64 {
    1.0
}

fn is_unity(value: &f64) -> bool {
    *value == 1.0
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
    /// Where the note starts, counted from the start of the clip. Never
    /// negative.
    pub start: Ticks,
    /// How long the note lasts. Always positive.
    pub length: Ticks,
}
