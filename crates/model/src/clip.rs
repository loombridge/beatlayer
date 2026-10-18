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
//!
//! A clip can also carry envelopes of its own ([`ClipEnvelope`], WP-3.15),
//! one per parameter of its track they modulate; they play while the clip
//! plays, and the rules are in the clip envelopes spec.

use std::collections::BTreeSet;

use bl_core::time::{TempoMap, Ticks, TimeSignature};
use imbl::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{AutomationTarget, Envelope};
use crate::error::ModelError;
use crate::ids::{ClipId, ContentHash};
use crate::launch::ClipLaunch;
use crate::warp::WarpMarker;

/// The clips whose MIDI Note Editors are folded to the rows that hold notes
/// (WP-2.3): saved with the set, but not part of the document, so folding
/// isn't an undo step.
pub type ClipFolds = BTreeSet<ClipId>;

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
    /// The clip's envelopes (WP-3.15): one per parameter of its track it
    /// modulates, in target order. Empty for a clip with none, which is what
    /// a set saved before this package holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envelopes: Vec<ClipEnvelope>,
    /// The clip's launch settings (WP-3.4), which only a Session clip's
    /// launching reads. The defaults for a clip saved without them.
    #[serde(default, skip_serializing_if = "ClipLaunch::is_default")]
    pub launch: ClipLaunch,
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
            envelopes: Vec::new(),
            launch: ClipLaunch::default(),
        }
    }

    /// The clip's envelopes, in target order.
    pub fn envelopes(&self) -> impl ExactSizeIterator<Item = &ClipEnvelope> + '_ {
        self.envelopes.iter()
    }

    /// The modulation envelope the clip carries for `target`, if it has one.
    #[must_use]
    pub fn envelope(&self, target: &AutomationTarget) -> Option<&ClipEnvelope> {
        self.envelope_of(target, EnvelopeKind::Modulation)
    }

    /// The envelope of `kind` the clip carries for `target`, if it has one.
    #[must_use]
    pub fn envelope_of(
        &self,
        target: &AutomationTarget,
        kind: EnvelopeKind,
    ) -> Option<&ClipEnvelope> {
        self.envelopes
            .iter()
            .find(|envelope| envelope.target == *target && envelope.kind == kind)
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
            ClipData::Audio(audio) => match audio.file_start_beat() {
                // Warped: the file's start falls as far from the Start
                // marker's beat as the map says, in ticks.
                Some(beat) => Ticks(self.start.get() - (audio.start - beat).round() as i64),
                None => tempo.seconds_to_ticks(tempo.ticks_to_seconds(self.start) - audio.start),
            },
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
            ClipData::Midi(midi) => {
                part.offset = self.offset.saturating_add(from - self.start);
                midi.move_by(self.start, from);
            }
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
                let at = match audio.file_start_beat() {
                    Some(beat) => Ticks(self.start.get() - (audio.start - beat.ceil()) as i64),
                    None => {
                        tempo.seconds_to_ticks(tempo.ticks_to_seconds(self.start) - audio.start)
                    }
                };
                let mut at = at.min(self.start);
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
                if let Some(before) = next.seconds_before_file() {
                    return Err(ModelError::BeforeFileStart(before));
                }
                *audio = next;
            }
            ClipData::Midi(midi) => {
                let offset = start
                    .checked_sub(self.start)
                    .and_then(|by| self.offset.checked_add(by))
                    .ok_or(ModelError::PositionOverflow)?;
                if offset.is_negative() {
                    return Err(ModelError::NegativeOffset(offset));
                }
                moved.offset = offset;
                midi.move_by(self.start, start);
            }
        }
        Ok(moved)
    }
}

/// What a clip's envelope does to its parameter (WP-3.12, WP-3.15).
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
    description = "What a clip envelope does: `modulation` (the default) moves the value the \
                   control or the track's automation gives by a share of the parameter's travel; \
                   `automation` (a Session clip's) sets the value outright while the clip plays."
)]
pub enum EnvelopeKind {
    /// Moves the parameter's own value, in modulation's units.
    #[default]
    Modulation,
    /// Sets the parameter's value, in the parameter's own units.
    Automation,
}

/// A clip's envelope for one of its track's parameters (WP-3.15): the
/// breakpoints it holds and whether it follows the clip's timeline.
///
/// A linked envelope (the Envelopes tab's Linked switch, on as a clip gets
/// it) runs over the clip's own timeline: it plays what the clip's contents
/// play, repeating with the clip's loop. An unlinked one plays once from the
/// clip's start, one tick per tick, for as long as the clip lasts. The rules
/// are in the clip envelopes spec.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "An envelope a clip carries for a parameter of its track: the parameter it \
                   modulates, its breakpoints, and whether it follows the clip's timeline \
                   (`linked`, the default) or runs once from the clip's start (`false`)."
)]
pub struct ClipEnvelope {
    /// The parameter it modulates: a control of the clip's track's mixer, a
    /// send, or a parameter of a device in its chain.
    pub target: AutomationTarget,
    /// Its breakpoints, in position order: positions are ticks from the
    /// beginning of the clip's contents, like a note's start.
    pub envelope: Envelope,
    /// Whether it follows the clip's region and loop (`true`, the Linked
    /// switch's default) or plays once from the clip's start (`false`).
    #[serde(default = "linked_default", skip_serializing_if = "is_linked")]
    pub linked: bool,
    /// Whether it modulates the parameter or sets it (WP-3.12). A clip can
    /// hold one of each kind for a parameter.
    #[serde(default, skip_serializing_if = "is_modulation")]
    pub kind: EnvelopeKind,
    /// Whether an automation envelope drives its parameter: a control moved
    /// by hand while the clip plays turns it off until the clip launches
    /// again or Re-Enable Automation. Always true for a modulation envelope.
    #[serde(default = "linked_default", skip_serializing_if = "is_linked")]
    pub enabled: bool,
}

impl ClipEnvelope {
    /// An envelope driving `target`, linked to its clip.
    #[must_use]
    pub fn new(target: AutomationTarget, envelope: Envelope) -> Self {
        Self {
            target,
            envelope,
            linked: true,
            kind: EnvelopeKind::Modulation,
            enabled: true,
        }
    }
}

/// The `linked` a clip envelope gets when a set doesn't say.
fn linked_default() -> bool {
    true
}

/// The clip's entry for `target`, adding an empty linked one if there's
/// none: for an edit that gives a parameter its first breakpoint.
pub(crate) fn clip_entry(
    envelopes: &mut Vec<ClipEnvelope>,
    target: AutomationTarget,
    kind: EnvelopeKind,
) -> &mut ClipEnvelope {
    let index = envelopes
        .iter()
        .position(|envelope| envelope.target == target && envelope.kind == kind)
        .unwrap_or_else(|| {
            let mut entry = ClipEnvelope::new(target, Envelope::default());
            entry.kind = kind;
            envelopes.push(entry);
            envelopes.len() - 1
        });
    &mut envelopes[index]
}

/// Whether an envelope is a modulation one, so serde leaves the default out.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_modulation(kind: &EnvelopeKind) -> bool {
    *kind == EnvelopeKind::Modulation
}

/// Whether a clip envelope is linked, so serde leaves the default out.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_linked(linked: &bool) -> bool {
    *linked
}

/// How far a moved Start marker may land before the file's start and still
/// count as its start: a millionth of a frame at 192 kHz, far below what
/// rounding a time to a frame can see. Moving a marker by the time between
/// two ticks and back can land this close to 0 on the wrong side.
pub(crate) const MARKER_SLACK: f64 = 1e-12;

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
/// An unwarped clip plays at its file's own speed, so its markers and loop
/// brace are positions in the file in seconds, as Live shows them. A warped
/// clip (WP-4.1, the warping spec) follows the set's tempo through its warp
/// map, and its markers and brace are beats in its contents, in ticks. A clip starts playing at its Start
/// marker. With the loop off, it plays to its End marker or its own end in
/// the Arrangement, whichever comes first; with the loop on, it plays into
/// the loop brace and repeats it until the clip ends.
///
/// Every field but `media` has a default, a new clip's, and is saved only
/// when it differs from it, so a set saved before WP-1.4, whose audio clips
/// hold only `media`, loads unchanged and plays each file whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// The Warp switch (the warping spec, R1): off unless given. On, the
    /// clip follows the set's tempo through its warp map, and `start`, `end`,
    /// `loop_start` and `loop_end` are whole ticks of its contents, counted
    /// from the clip's 1.1.1, instead of seconds in the file.
    #[serde(default, skip_serializing_if = "is_false")]
    pub warp: bool,
    /// The warp markers, in increasing order of both fields: a warped clip
    /// has at least one. Switching Warp off keeps them (R3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warp_markers: Vec<WarpMarker>,
    /// The clip's tempo in BPM, from 1 to 1,000: the speed of the file after
    /// the last warp marker (R2). Given whenever there are markers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1.0, max = 1000.0))]
    pub tempo: Option<f64>,
    /// A warped clip's own time signature, the Clip View's Signature fields;
    /// `null` (or nothing) means the song's signature at the clip's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<TimeSignature>,
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
            warp: false,
            warp_markers: Vec::new(),
            tempo: None,
            signature: None,
        }
    }

    /// The data of a clip whose left edge moved from `from` to `to`: its
    /// Start marker moves by the time between them under `tempo`, so the
    /// clip plays the same audio at the same moments (the audio clips spec,
    /// R16); a warped clip's moves by the ticks between them (the warping
    /// spec, R5). With the loop on, a marker that moves later to or past the
    /// loop brace's end wraps back inside the brace by whole lengths of it,
    /// as Live's does; a brace that ends at the end of the file, whose length
    /// the set doesn't know, can't wrap it. A marker that lands a hair
    /// before the file's start, as moving back by the same ticks can, is
    /// the file's start; one further before it is left negative, for the
    /// caller to refuse.
    #[must_use]
    pub fn moved_by(&self, from: Ticks, to: Ticks, tempo: &TempoMap) -> Self {
        if from == to {
            return self.clone();
        }
        let by = if self.warp {
            (to - from).get() as f64
        } else {
            tempo.ticks_to_seconds(to) - tempo.ticks_to_seconds(from)
        };
        let mut start = self.start + by;
        if !self.warp && (-MARKER_SLACK..0.0).contains(&start) {
            start = 0.0;
        }
        if let (true, true, Some(end)) = (self.looping, by > 0.0, self.loop_end) {
            let length = end - self.loop_start;
            if length > 0.0 && start >= end {
                let into = (start - end).rem_euclid(length);
                // Rounding can leave a remainder of a whole length.
                start = self.loop_start + if into < length { into } else { 0.0 };
            }
        }
        Self {
            start,
            ..self.clone()
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

/// One recorded pass on a take lane: where it sits, how long it is, and the
/// audio it plays. Unlike an Arrangement [`Clip`] it has no ID and isn't
/// addressed by any command yet; take-lane editing and comping come later
/// (the recording spec, R12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A recorded pass on a take lane: its `name`, `start` and `length` in ticks \
                   (705,600 to the quarter note), and the audio clip `data` it plays."
)]
pub struct TakeClip {
    /// The pass's name, which may be empty.
    pub name: String,
    /// Where the pass starts in the arrangement. Never negative.
    pub start: Ticks,
    /// How long the pass is. Always positive.
    pub length: Ticks,
    /// The audio it plays.
    pub data: AudioData,
}

/// One take lane under a track's main lane: a name and the passes it holds,
/// in start order. The track plays only its main lane; a take lane's clips
/// are kept until comping lets a person audition and pick from them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "A take lane under a track's main lane: its `name` and its `clips`.")]
pub struct TakeLane {
    /// The lane's name, such as `Take 1`.
    pub name: String,
    /// The passes on the lane.
    pub clips: Vec<TakeClip>,
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

fn default_release_velocity() -> u8 {
    64
}

fn is_default_release_velocity(value: &u8) -> bool {
    *value == 64
}

fn default_probability() -> f64 {
    1.0
}

fn is_default_probability(value: &f64) -> bool {
    *value == 1.0
}

fn is_zero_i8(value: &i8) -> bool {
    *value == 0
}

/// A MIDI clip's data: its notes, Start and End markers, and loop brace.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "A MIDI clip's data: its notes, Start and End markers, and loop brace.")]
pub struct MidiData {
    /// The notes, in no particular order.
    #[schemars(with = "Vec<Note>")]
    pub notes: Vector<Note>,
    /// The Start marker: where in its contents the clip starts playing.
    /// 0 unless given.
    #[serde(default, skip_serializing_if = "is_zero_ticks")]
    pub start: Ticks,
    /// The End marker: where a clip whose loop is off stops playing, after
    /// the Start marker; `null` (or nothing) for the end of the clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Ticks>,
    /// The Loop switch: whether the clip repeats its loop brace. Off unless
    /// given.
    #[serde(default, skip_serializing_if = "is_false")]
    pub looping: bool,
    /// Where the loop brace starts, in ticks: 0 or more; 0 unless given.
    #[serde(default, skip_serializing_if = "is_zero_ticks")]
    pub loop_start: Ticks,
    /// Where the loop brace ends, in ticks, after its start; `null` (or nothing)
    /// for the end of the clip. While the loop is on, the Start marker must
    /// be before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_end: Option<Ticks>,
    /// The clip's time signature, the Clip View's Signature fields. `null`
    /// (or nothing) means the song's signature at the clip's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<TimeSignature>,
}

impl MidiData {
    /// MIDI data holding these notes, unlooped, starting at tick zero.
    #[must_use]
    pub fn new(notes: impl IntoIterator<Item = Note>) -> Self {
        Self {
            notes: notes.into_iter().collect(),
            start: Ticks::ZERO,
            end: None,
            looping: false,
            loop_start: Ticks::ZERO,
            loop_end: None,
            signature: None,
        }
    }

    /// Moves the clip's Start marker when its left edge moves from `from`
    /// to `to`. With the loop on, a marker that moves later to or past the
    /// loop brace's end wraps back inside the brace by whole lengths of it.
    pub fn move_by(&mut self, from: Ticks, to: Ticks) {
        if from == to {
            return;
        }
        let by = to - from;
        let mut start = self.start + by;
        if start < Ticks::ZERO {
            start = Ticks::ZERO;
        }
        if let (true, true, Some(end)) = (self.looping, by > Ticks::ZERO, self.loop_end) {
            let length = end - self.loop_start;
            if length > Ticks::ZERO && start >= end {
                let into = (start - end).get().rem_euclid(length.get());
                start = self.loop_start
                    + if into < length.get() {
                        Ticks(into)
                    } else {
                        Ticks::ZERO
                    };
            }
        }
        self.start = start;
    }

    /// The data of a clip whose left edge moved from `from` to `to`: its
    /// Start marker moves by the ticks between them. With the loop on, a
    /// marker that moves later to or past the loop brace's end wraps back
    /// inside the brace by whole lengths of it, as Live's does.
    #[must_use]
    pub fn moved_by(mut self, from: Ticks, to: Ticks) -> Self {
        self.move_by(from, to);
        self
    }
}

/// One point of a note's pressure (WP-2.29): from `at` after the note's
/// start the note's key is pressed to `value`, until the next point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PressurePoint {
    /// Where it takes effect, counted from the note's start: 0 or more and
    /// before the note's length.
    pub at: Ticks,
    /// The pressure, from 0 to 127.
    #[schemars(range(max = 127))]
    pub value: u8,
}

/// A note in a MIDI clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Note {
    /// The MIDI key, from 0 to 127. Middle C (C3 in Live) is 60.
    #[schemars(range(max = 127))]
    pub key: u8,
    /// The velocity, from 1 to 127.
    #[schemars(range(min = 1, max = 127))]
    pub velocity: u8,
    /// The release velocity, from 1 to 127; 64 unless given.
    #[serde(
        default = "default_release_velocity",
        skip_serializing_if = "is_default_release_velocity"
    )]
    #[schemars(range(min = 1, max = 127))]
    pub release_velocity: u8,
    /// Where the note starts, counted from the beginning of the clip's
    /// contents: the clip's start less its offset. Never negative.
    pub start: Ticks,
    /// How long the note lasts. Always positive.
    pub length: Ticks,
    /// The chance the note will play, from 0 to 1: 1 (100 %) unless given.
    #[serde(
        default = "default_probability",
        skip_serializing_if = "is_default_probability"
    )]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub probability: f64,
    /// How far velocity may deviate randomly, from -127 to +127: 0 unless given.
    #[serde(default, skip_serializing_if = "is_zero_i8")]
    #[schemars(range(min = -127, max = 127))]
    pub velocity_deviation: i8,
    /// Whether the note is switched off, so it stays in place but doesn't play.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mute: bool,
    /// The note's own pressure, played as polyphonic pressure over its
    /// length: points in order of `at`, none at the same place. A note with
    /// none plays no pressure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pressure: Vec<PressurePoint>,
}

impl Note {
    /// A new unmuted note with velocity 100, release velocity 64, probability 1.0, and velocity deviation 0.
    #[must_use]
    pub fn new(key: u8, start: Ticks, length: Ticks, velocity: u8) -> Self {
        Self {
            key,
            velocity,
            release_velocity: 64,
            start,
            length,
            probability: 1.0,
            velocity_deviation: 0,
            mute: false,
            pressure: Vec::new(),
        }
    }
}

impl Default for Note {
    fn default() -> Self {
        Self {
            key: 60,
            velocity: 100,
            release_velocity: 64,
            start: Ticks::ZERO,
            length: Ticks::QUARTER,
            probability: 1.0,
            velocity_deviation: 0,
            mute: false,
            pressure: Vec::new(),
        }
    }
}
