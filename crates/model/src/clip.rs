//! Arrangement clips and their data.
//!
//! An audio clip's data is its file, its Start and End markers, its loop,
//! its gain, and its edge fades (WP-1.4, the audio clips spec). MIDI
//! clip data is still a stub, enough to hold notes, until WP-2.1 adds
//! velocity, probability, and the rest.
//!
//! A clip shows a window onto its contents. Splitting a clip or trimming its
//! left edge (WP-1.8b) changes what the clip shows without changing where
//! anything plays, and how it does that depends on the clip, as in Live:
//!
//! - An audio clip's Start marker moves (WP-1.4b): by the time its left edge
//!   moved, under the tempo map, since an unwarped clip's markers are times
//!   in its file. Its [`Clip::offset`] is always 0.
//! - A MIDI clip has no markers yet (WP-2.1), so its contents begin
//!   [`Clip::offset`] before the clip starts, and the offset changes.
//!
//! [`Clip::part`] and [`Clip::with_start`] make those changes.

use bl_core::time::{TempoMap, Ticks};
use imbl::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
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
    /// How far into its contents a MIDI clip starts: its notes are placed
    /// from `start - offset`, and the part before the clip's start doesn't
    /// play. 0 (or left out) plays the contents from their beginning. Never
    /// negative. Splitting a MIDI clip or trimming its left edge changes it,
    /// so each part plays what it played before. An audio clip's is always
    /// 0: its Start marker moves instead, as Live's does.
    #[serde(default, skip_serializing_if = "is_zero_ticks")]
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
    /// its offset. A MIDI clip's notes are placed from it. An audio clip's
    /// offset is 0, so this is its start, where it plays its file from its
    /// Start marker.
    #[must_use]
    pub fn content_start(&self) -> Ticks {
        self.start.saturating_sub(self.offset)
    }

    /// Where the clip's contents begin in the arrangement under `tempo`, to
    /// the nearest tick, perhaps before 0: for an audio clip, where the
    /// start of its file falls, the time of its Start marker before the
    /// clip's start (an unwarped file runs at its own speed); for a MIDI
    /// clip, [`Clip::content_start`]. Every part of a split clip has the
    /// same origin as the whole, which is how a waveform carries on.
    #[must_use]
    pub fn origin(&self, tempo: &TempoMap) -> Ticks {
        match &self.data {
            ClipData::Audio(audio) => {
                tempo.seconds_to_ticks(tempo.ticks_to_seconds(self.start) - audio.start)
            }
            ClipData::Midi(_) => self.content_start(),
        }
    }

    /// The part of the clip from `from` to `to`, which must lie inside it,
    /// playing what the clip played there: what a split, or a clip covered
    /// at an end, leaves (the arrangement editing spec, R8 and R16). An
    /// audio clip's Start marker moves on by the time from the clip's start
    /// to `from` under `tempo`, as Live moves it; a MIDI clip's offset grows
    /// by as much.
    #[must_use]
    pub fn part(&self, from: Ticks, to: Ticks, tempo: &TempoMap) -> Self {
        let mut part = Self {
            start: from,
            length: to.saturating_sub(from),
            ..self.clone()
        };
        match &mut part.data {
            ClipData::Audio(audio) => *audio = audio.moved_by(self.start, from, tempo),
            ClipData::Midi(_) => part.offset = self.offset.saturating_add(from - self.start),
        }
        part
    }

    /// The earliest its left edge can go while it plays what it plays: where
    /// an audio clip's file starts (its Start marker at 0), or where a MIDI
    /// clip's contents begin, and never before 0.
    #[must_use]
    pub fn earliest_start(&self, tempo: &TempoMap) -> Ticks {
        let earliest = match &self.data {
            ClipData::Audio(audio) => {
                let at = tempo.ticks_to_seconds(self.start) - audio.start;
                let mut at = tempo.seconds_to_ticks(at).min(self.start);
                // The nearest tick can be a hair before the file's start.
                while at < self.start && self.with_start(at, tempo).is_err() {
                    at = at.saturating_add(Ticks(1));
                }
                at
            }
            ClipData::Midi(_) => self.content_start(),
        };
        earliest.max(Ticks::ZERO)
    }

    /// The clip with its left edge at `start` and its end where it was,
    /// still playing what it played: an audio clip's Start marker moves by
    /// the time the edge moved under `tempo`, as Live's does, and a MIDI
    /// clip's offset by as many ticks.
    ///
    /// # Errors
    ///
    /// [`ModelError::BeforeFileStart`] if an audio clip's Start marker would
    /// go before the start of its file, [`ModelError::NegativeOffset`] if a
    /// MIDI clip would start before its contents begin, and
    /// [`ModelError::PositionOverflow`] if the numbers don't fit. The length
    /// isn't checked.
    pub fn with_start(&self, start: Ticks, tempo: &TempoMap) -> Result<Self, ModelError> {
        let length = self
            .end()
            .checked_sub(start)
            .ok_or(ModelError::PositionOverflow)?;
        let mut moved = Self {
            start,
            length,
            ..self.clone()
        };
        match &mut moved.data {
            ClipData::Audio(audio) => {
                let next = audio.moved_by(self.start, start, tempo);
                if next.start < 0.0 {
                    return Err(ModelError::BeforeFileStart(-next.start));
                }
                *audio = next;
            }
            ClipData::Midi(_) => {
                let offset = start
                    .checked_sub(self.start)
                    .and_then(|by| self.offset.checked_add(by))
                    .ok_or(ModelError::PositionOverflow)?;
                if offset.is_negative() {
                    return Err(ModelError::NegativeOffset(offset));
                }
                moved.offset = offset;
            }
        }
        Ok(moved)
    }
}

/// How far a moved Start marker may land before the file's start and still
/// count as its start: a millionth of a frame at 192 kHz, far below what
/// rounding a time to a frame can see. Moving a marker by the time between
/// two ticks and back can land this close to 0 on the wrong side.
const MARKER_SLACK: f64 = 1e-12;

/// Whether a position is zero, so a zero offset is left out of saved sets.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_zero_ticks(ticks: &Ticks) -> bool {
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

    /// The data of a clip whose left edge moved from `from` to `to`: its
    /// Start marker moves by the time between them under `tempo`, so the
    /// clip plays the same audio at the same moments (the audio clips spec,
    /// R16). A marker that lands a hair before the file's start, as moving
    /// back by the same ticks can, is the file's start; one further before
    /// it is left negative, for the caller to refuse.
    #[must_use]
    pub fn moved_by(self, from: Ticks, to: Ticks, tempo: &TempoMap) -> Self {
        if from == to {
            return self;
        }
        let by = tempo.ticks_to_seconds(to) - tempo.ticks_to_seconds(from);
        let start = self.start + by;
        Self {
            start: if (-MARKER_SLACK..0.0).contains(&start) {
                0.0
            } else {
                start
            },
            ..self
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
    /// Where the note starts, counted from the beginning of the clip's
    /// contents: the clip's start less its offset. Never negative.
    pub start: Ticks,
    /// How long the note lasts. Always positive.
    pub length: Ticks,
}
