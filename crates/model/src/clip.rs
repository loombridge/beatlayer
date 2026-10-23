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

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::{TempoMap, Ticks, TimeSignature};
use imbl::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{AutomationTarget, Breakpoint, Envelope};
use crate::error::ModelError;
use crate::ids::{ClipId, ContentHash, TakeLaneId};
use crate::launch::ClipLaunch;
use crate::scale::Scale;
use crate::warp::{TransientEdits, WarpMarker};
use crate::warp_settings::{
    DETUNE_MAX, DETUNE_MIN, TRANSPOSE_MAX, TRANSPOSE_MIN, WarpMode, WarpSettings,
};

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
    /// The clip's scale (WP-6.7): its Scale Mode switch, its root and its
    /// scale name. It never changes what the clip plays; it tells the rest
    /// of the set which keys belong. A clip off at C, in Major, writes
    /// nothing, so a set saved before the scales loads unchanged.
    #[serde(default, skip_serializing_if = "Scale::is_default")]
    pub scale: Scale,
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
            scale: Scale::default(),
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

/// The exponent of a fade's curve (the audio clips spec, R23): *p* =
/// 4^(−`curve`), so a fade-in's gain at a fraction *x* of the way through it
/// is *x*^*p*. A curve of 0 is 1, the straight line.
#[must_use]
pub fn fade_power(curve: f64) -> f64 {
    4f64.powf(-curve)
}

/// A crossfade between two audio clips on one track (the audio clips spec,
/// R24), as the clips' fade fields hold it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A crossfade: the earlier of two audio clips that meet fades out, and the later \
                   fades in, over a span from `before` seconds before their boundary to `after` \
                   seconds after it, with one curve."
)]
pub struct Crossfade {
    /// The clip that ends at the boundary.
    pub earlier: ClipId,
    /// The clip that starts at the boundary.
    pub later: ClipId,
    /// Where the clips meet.
    pub boundary: Ticks,
    /// How many seconds before the boundary the span starts.
    pub before: f64,
    /// How many seconds after the boundary the span ends.
    pub after: f64,
    /// The curve of both fades, from −1 to 1.
    pub curve: f64,
}

/// The crossfades among `clips`, a track's clips (R24): each pair of audio
/// clips where one ends exactly where the other starts and whose fade fields
/// agree on a span and a curve, with a span that reaches past the boundary on
/// at least one side. Fields that disagree, or that name a neighbour the clip
/// no longer meets, are no crossfade, and the engine plays the clip's own
/// fades alone. In order of boundary.
#[must_use]
pub fn crossfades<'a>(clips: impl IntoIterator<Item = &'a Clip>) -> Vec<Crossfade> {
    let mut audio: Vec<(&Clip, &AudioData)> = clips
        .into_iter()
        .filter_map(|clip| match &clip.data {
            ClipData::Audio(data) => Some((clip, data)),
            ClipData::Midi(_) => None,
        })
        .collect();
    audio.sort_by_key(|(clip, _)| (clip.start, clip.id));
    let mut found = Vec::new();
    for (earlier, before_data) in &audio {
        let boundary = earlier.end();
        let first = audio.partition_point(|(clip, _)| clip.start < boundary);
        let Some((later, after_data)) = audio[first..]
            .iter()
            .take_while(|(clip, _)| clip.start == boundary)
            .find(|(clip, _)| clip.id != earlier.id)
        else {
            continue;
        };
        let reaches = before_data.fade_out_after > 0.0 || after_data.fade_in_before > 0.0;
        if reaches
            && before_data.fade_out_after == after_data.fade_in
            && before_data.fade_out == after_data.fade_in_before
            && before_data.fade_out_curve == after_data.fade_in_curve
        {
            found.push(Crossfade {
                earlier: earlier.id,
                later: later.id,
                boundary,
                before: after_data.fade_in_before,
                after: after_data.fade_in,
                curve: after_data.fade_in_curve,
            });
        }
    }
    found.sort_by_key(|c| (c.boundary, c.earlier));
    found
}

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
    /// The fade-in's curve (the audio clips spec, R23), from −1 to 1: 0, the
    /// straight line, unless given.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = -1.0, max = 1.0))]
    pub fade_in_curve: f64,
    /// The fade-out's curve, from −1 to 1: 0 unless given.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = -1.0, max = 1.0))]
    pub fade_out_curve: f64,
    /// How many seconds before the clip's start a crossfade (R24) begins its
    /// fade-in, over the clip that ends where this one starts: 0 unless this
    /// clip is the later of two crossfaded ones. The fade-in's length is then
    /// this much before the start and `fade_in` after it.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0))]
    pub fade_in_before: f64,
    /// How many seconds past the clip's end a crossfade (R24) ends its
    /// fade-out, under the clip that starts where this one ends: 0 unless this
    /// clip is the earlier of two crossfaded ones. The fade-out's length is
    /// then `fade_out` before the end and this much after it.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0))]
    pub fade_out_after: f64,
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
    /// The warp mode (the warp modes spec, R1): Beats unless given. An
    /// unwarped clip keeps it for when Warp goes back on.
    #[serde(default, skip_serializing_if = "is_default")]
    pub warp_mode: WarpMode,
    /// Every mode's controls (R2): each at the table's default unless given.
    #[serde(default, skip_serializing_if = "is_default")]
    pub warp_settings: WarpSettings,
    /// The Lead/Follow switch (the warping spec, R43): whether this
    /// Arrangement clip leads the set's tempo instead of following it. Off
    /// unless given, a follower. Only a warped Arrangement clip can lead.
    #[serde(default, skip_serializing_if = "is_false")]
    pub tempo_leader: bool,
    /// The RAM switch (R11): off unless given. On, the clip's whole file is
    /// held in memory instead of streamed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ram: bool,
    /// The HiQ switch (R12): on unless given. On, the clip is resampled by a
    /// band-limited interpolator wherever it is read at another rate than 1.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub high_quality: bool,
    /// The clip's Transpose, in whole semitones, from −48 to 48 (R2): 0
    /// unless given. It shifts the clip's pitch in every mode but Re-Pitch
    /// (R10, R26).
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    #[schemars(range(min = -48, max = 48))]
    pub transpose: i32,
    /// The clip's Detune, in cents, from −50 to 50 (R2): 0 unless given.
    /// It shifts the clip's pitch in every mode but Re-Pitch (R10, R26).
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = -50.0, max = 50.0))]
    pub detune: f64,
    /// The clip's edits to its file's detected transients (the warping spec,
    /// R23): none unless given. They belong to the file, so replacing the
    /// file drops them (R27).
    #[serde(default, skip_serializing_if = "TransientEdits::is_empty")]
    pub transient_edits: TransientEdits,
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
            fade_in_curve: 0.0,
            fade_out_curve: 0.0,
            fade_in_before: 0.0,
            fade_out_after: 0.0,
            warp: false,
            warp_markers: Vec::new(),
            tempo: None,
            signature: None,
            warp_mode: WarpMode::Beats,
            warp_settings: WarpSettings::default(),
            tempo_leader: false,
            ram: false,
            high_quality: true,
            transpose: 0,
            detune: 0.0,
            transient_edits: TransientEdits::default(),
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

    /// Gives this clip's end and the start of `later`, which meets it, a
    /// crossfade over `before` seconds before their boundary and `after`
    /// seconds past it, with `curve` (the audio clips spec, R24): the fade
    /// fields of both clips are set to agree.
    pub fn crossfade_into(&mut self, later: &mut Self, before: f64, after: f64, curve: f64) {
        self.fade_out = before;
        self.fade_out_after = after;
        self.fade_out_curve = curve;
        later.fade_in = after;
        later.fade_in_before = before;
        later.fade_in_curve = curve;
    }

    /// Ends the crossfade between this clip's end and the start of `later`,
    /// leaving neither a fade there (R29, with Create Fades on Clip Edges
    /// off).
    pub fn end_crossfade(&mut self, later: &mut Self) {
        self.crossfade_into(later, 0.0, 0.0, 0.0);
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

    /// The clip's pitch change, in semitones (R25): its Transpose plus its
    /// Detune in semitones, each held to its range. The envelopes and
    /// automation that add to them come with WP-4.8.
    #[must_use]
    pub fn pitch_change(&self) -> f64 {
        let transpose = f64::from(self.transpose.clamp(TRANSPOSE_MIN, TRANSPOSE_MAX));
        let detune = self.detune.clamp(DETUNE_MIN, DETUNE_MAX) / 100.0;
        transpose + detune
    }

    /// The clip's pitch ratio (R25): 2 raised to its pitch change over 12,
    /// so 1 where it isn't transposed.
    #[must_use]
    pub fn pitch_ratio(&self) -> f64 {
        2f64.powf(self.pitch_change() / 12.0)
    }

    /// The data with its file reversed (the audio clip editing spec, R8):
    /// the same stretch of sound played backwards. Its Start and End
    /// markers, its loop brace and its warp markers are mirrored about the
    /// file, so a position *t* seconds into the source becomes *L* − *t* in
    /// the new file, and each warp marker keeps its point in the audio. The
    /// loop switch, gain, transpose, detune, warp mode, fades and the rest
    /// don't change, and the clip's envelopes stay where they are (R9).
    #[must_use]
    pub fn reversed(&self, seconds: f64, media: Option<ContentHash>) -> Self {
        let mirror = |at: f64| (seconds - at).max(0.0);
        let mut next = self.clone();
        if self.warp {
            // A warped clip's markers are ticks of its contents, so they go
            // through its warp map: the time each falls at is mirrored about
            // the file, and the new map says which beat that is.
            let (Ok(map), true) = (self.warp_map(), !self.warp_markers.is_empty()) else {
                return next;
            };
            let markers = self.mirrored_markers(seconds);
            // Each marker keeps its point in the audio: its beat becomes the
            // first marker's beat plus the last marker's less its own, as
            // the warp markers' do.
            let first_beat = self.warp_markers[0].beat.get() as f64;
            let last_beat = self.warp_markers[self.warp_markers.len() - 1].beat.get() as f64;
            let mirror_beat = |beat: f64| (first_beat + last_beat - beat).round();
            // The region from Start *s* to End *e* becomes one from *L* - *e*
            // to *L* - *s*, the file's end standing in for an End marker the
            // clip doesn't set.
            let end = self.end.unwrap_or_else(|| map.time_to_beat(seconds));
            next.start = mirror_beat(end);
            next.end = if map.beat_to_time(self.start).abs() < 1e-9 {
                None
            } else {
                Some(mirror_beat(self.start))
            };
            // The loop brace likewise.
            let loop_end = self.loop_end.unwrap_or_else(|| map.time_to_beat(seconds));
            next.loop_start = mirror_beat(loop_end);
            next.loop_end = if map.beat_to_time(self.loop_start).abs() < 1e-9 {
                None
            } else {
                Some(mirror_beat(self.loop_start))
            };
            next.warp_markers = markers;
            if let Some(media) = media {
                next.media = media;
            }
            return next;
        }
        // The region: from Start *s* to End *e* becomes one from *L* − *e* to
        // *L* − *s*, the file's end standing in for an End marker the clip
        // doesn't set.
        let end = self.end.unwrap_or(seconds);
        next.start = mirror(end);
        next.end = ending(mirror(self.start), seconds);
        // The loop brace likewise.
        let loop_end = self.loop_end.unwrap_or(seconds);
        next.loop_start = mirror(loop_end);
        next.loop_end = ending(mirror(self.loop_start), seconds);
        // Each warp marker keeps its point in the audio: its file position is
        // mirrored, and its beat becomes the first marker's beat plus the
        // last marker's less its own.
        next.warp_markers = self.mirrored_markers(seconds);
        if let Some(media) = media {
            next.media = media;
        }
        next
    }

    /// The clip's warp markers mirrored about the file (R8): each one's file
    /// position is mirrored, and its beat becomes the first marker's beat
    /// plus the last marker's less its own. They run backwards afterwards, so
    /// both their times and their beats keep rising.
    fn mirrored_markers(&self, seconds: f64) -> Vec<WarpMarker> {
        if self.warp_markers.is_empty() {
            return Vec::new();
        }
        let first = self.warp_markers[0].beat;
        let last = self.warp_markers[self.warp_markers.len() - 1].beat;
        let mut markers: Vec<WarpMarker> = self
            .warp_markers
            .iter()
            .map(|marker| WarpMarker {
                time: (seconds - marker.time).max(0.0),
                beat: first + last - marker.beat,
            })
            .collect();
        markers.reverse();
        markers
    }
}

/// A marker's new place after a reversal: the file's end standing in for a
/// marker the clip doesn't set, so that reversing again gives the clip back
/// exactly as it was (R10).
fn ending(at: f64, seconds: f64) -> Option<f64> {
    ((at - seconds).abs() > 1e-9).then_some(at)
}

/// What a pass handed to `track.add_take_lane` plays: audio data as it was
/// given before take lanes held clips (just the audio's fields), or a clip's
/// data tagged `audio` or `midi`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TakeClipData {
    /// Audio or MIDI data, tagged as a clip's is.
    Tagged(ClipData),
    /// Audio data without the tag.
    Audio(AudioData),
}

impl TakeClipData {
    /// The clip data it stands for.
    #[must_use]
    pub fn into_data(self) -> ClipData {
        match self {
            Self::Tagged(data) => data,
            Self::Audio(audio) => ClipData::Audio(audio),
        }
    }
}

/// A clip handed to `track.add_take_lane`: where it sits, how long it is,
/// and what it plays. Its ID is issued when it's left out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A clip for a new take lane: its `name`, `start` and `length` in ticks \
                   (705,600 to the quarter note), and the `data` it plays, audio as \
                   `clip.create` takes it or tagged `audio` or `midi`. Its `id` is issued when \
                   it's left out."
)]
pub struct TakeClip {
    /// The clip's ID. Left out, the editor issues one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<ClipId>,
    /// The clip's name, which may be empty.
    pub name: String,
    /// Where the clip starts in the arrangement. Never negative.
    pub start: Ticks,
    /// How long the clip is. Always positive.
    pub length: Ticks,
    /// What it plays.
    pub data: TakeClipData,
}

/// A take lane handed to `track.add_take_lane`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A new take lane: its `name`, the `clips` it holds, and its `id`, issued when \
                   it's left out."
)]
pub struct NewTakeLane {
    /// The lane's ID. Left out, the editor issues one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<TakeLaneId>,
    /// The lane's name, such as `Take 1`.
    pub name: String,
    /// The clips on the lane.
    #[serde(default)]
    pub clips: Vec<TakeClip>,
}

impl NewTakeLane {
    /// The lane with its clips as the document holds them. Every ID must be
    /// there: the editor issues the ones left out before the command runs.
    pub(crate) fn lane(&self) -> Result<TakeLane, ModelError> {
        let id = self.id.ok_or(ModelError::MissingId)?;
        let clips = self
            .clips
            .iter()
            .map(|clip| {
                let id = clip.id.ok_or(ModelError::MissingId)?;
                let data = clip.data.clone().into_data();
                Ok(Clip {
                    name: clip.name.clone(),
                    ..Clip::new(id, clip.start, clip.length, data)
                })
            })
            .collect::<Result<Vec<_>, ModelError>>()?;
        Ok(TakeLane {
            id,
            name: self.name.clone(),
            clips,
        })
    }
}

/// One take lane under a track's main lane (WP-6.21, the comping spec, R1):
/// an ID that never changes, a name, and clips of its track's kind, which
/// never overlap. The track plays only its main lane, or the lane that is
/// auditioned in its place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "A take lane under a track's main lane: its `id`, `name` and `clips`.")]
pub struct TakeLane {
    /// The lane's ID.
    pub id: TakeLaneId,
    /// The lane's name, such as `Take 1`.
    pub name: String,
    /// The clips on the lane.
    pub clips: Vec<Clip>,
}

impl TakeLane {
    /// The lane's clip with this ID.
    #[must_use]
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips.iter().find(|clip| clip.id == id)
    }
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
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

    /// The clip cropped to `from`..`to` of its contents (the audio clip
    /// editing spec, R17): the notes that start outside the region go, a note
    /// that starts inside and runs past the region's end is cut to end there,
    /// and the Start and End markers and the loop brace begin at the region's
    /// start. No file is involved, so the notes' own values are kept.
    #[must_use]
    pub fn cropped(&self, from: Ticks, to: Ticks) -> Self {
        let region = to.saturating_sub(from);
        let notes: Vector<Note> = self
            .notes
            .iter()
            .filter(|note| note.start >= from && note.start < to)
            .map(|note| Note {
                start: note.start - from,
                length: note.length.min(region - (note.start - from)),
                ..note.clone()
            })
            .collect();
        Self {
            notes,
            start: (self.start - from).max(Ticks::ZERO),
            end: cropped_end(self.end, from, to),
            loop_start: (self.loop_start - from).max(Ticks::ZERO),
            loop_end: cropped_end(self.loop_end, from, to),
            ..self.clone()
        }
    }
}

/// A MIDI marker's position after a crop, or `None` where the crop's end cuts
/// it off and the clip's end stands in for it.
fn cropped_end(marker: Option<Ticks>, from: Ticks, to: Ticks) -> Option<Ticks> {
    match marker {
        Some(at) if at > from && at < to => Some(at - from),
        _ => None,
    }
}

/// The song stretches the MIDI clip `clip` plays over `start`..`end`, each
/// with the part of its contents that plays there, as `(song start, song
/// end, content start, content end)`, in order: the clip's first pass and
/// each repetition of its loop brace, cut at the range's edges. It mirrors
/// the player's own reading of a clip (the engine's MIDI clip player).
fn played_stretches(
    clip: &Clip,
    midi: &MidiData,
    start: Ticks,
    end: Ticks,
) -> Vec<(Ticks, Ticks, Ticks, Ticks)> {
    if clip.length <= Ticks::ZERO || end <= start {
        return Vec::new();
    }
    let marker = midi.start;
    let mut windows: Vec<(Ticks, Ticks)> = Vec::new();
    if midi.looping {
        let loop_end = midi.loop_end.unwrap_or_else(|| {
            (midi.loop_start + clip.length).max(midi.loop_start + Ticks::QUARTER)
        });
        if loop_end > midi.loop_start && marker < loop_end {
            let brace = loop_end - midi.loop_start;
            let first = (loop_end - marker).min(clip.length);
            windows.push((marker, marker + first));
            let mut at = first;
            while at < clip.length {
                let repeat = brace.min(clip.length - at);
                windows.push((midi.loop_start, midi.loop_start + repeat));
                at += repeat;
            }
        }
    } else {
        let end_marker = midi.end.unwrap_or(marker + clip.length);
        if end_marker > marker {
            let active = clip.length.min(end_marker - marker);
            windows.push((marker, marker + active));
        }
    }
    // Each window's song position: the clip starts at `clip.start`, playing
    // its contents from the Start marker.
    let mut stretches = Vec::with_capacity(windows.len());
    let mut song = clip.start;
    for (content_start, content_end) in windows {
        let song_end = song + (content_end - content_start);
        let from = song.max(start);
        let to = song_end.min(end);
        if to > from {
            let inside = from - song;
            stretches.push((
                from,
                to,
                content_start + inside,
                content_start + inside + (to - from),
            ));
        }
        song = song_end;
    }
    stretches
}

/// What Consolidate makes of a MIDI track's clips over the song range
/// `start`..`end` (the audio clip editing spec, R19, R23): one clip named
/// `name`, `length` long from `start`, holding every note the range plays at
/// the position it plays it, with each repetition of a looped clip written
/// out and notes cut at the range's edges, and the clips' own envelopes
/// written in at the positions they played. Its loop is on over its whole
/// contents, so dragging its edge adds repetitions.
#[must_use]
pub fn consolidated_midi(
    id: ClipId,
    name: String,
    clips: &[&Clip],
    start: Ticks,
    end: Ticks,
) -> Clip {
    let mut notes: Vec<Note> = Vec::new();
    let mut envelopes: Vec<(ClipEnvelope, BTreeMap<Ticks, Breakpoint>)> = Vec::new();
    for clip in clips {
        let ClipData::Midi(midi) = &clip.data else {
            continue;
        };
        for (song_start, song_end, content_start, content_end) in
            played_stretches(clip, midi, start, end)
        {
            for note in midi.notes.iter() {
                if note.start < content_start || note.start >= content_end {
                    continue;
                }
                let at = song_start + (note.start - content_start);
                let length = note.length.min(song_end - at);
                if length > Ticks::ZERO {
                    notes.push(Note {
                        start: at - start,
                        length,
                        ..note.clone()
                    });
                }
            }
            for envelope in clip.envelopes() {
                let slot = entry(&mut envelopes, envelope);
                if envelope.linked {
                    // A linked envelope runs over what the clip's contents
                    // play: each of its breakpoints lands where it played,
                    // and each stretch's ends are held.
                    for point in envelope.envelope.points() {
                        if point.at >= content_start && point.at < content_end {
                            let at = song_start + (point.at - content_start) - start;
                            slot.insert(at, Breakpoint { at, ..point });
                        }
                    }
                    for (content, song) in [(content_start, song_start), (content_end, song_end)] {
                        if let Some(value) = envelope.envelope.value_at(content) {
                            slot.entry(song - start).or_insert(Breakpoint {
                                at: song - start,
                                value,
                                bend: 0.0,
                            });
                        }
                    }
                } else {
                    // An unlinked one plays once from the clip's start, one
                    // tick per tick.
                    let from = start - clip.start;
                    let to = end - clip.start;
                    for point in envelope
                        .envelope
                        .points()
                        .filter(|p| p.at >= from && p.at < to)
                    {
                        let at = point.at + clip.start - start;
                        slot.insert(at, Breakpoint { at, ..point });
                    }
                }
            }
        }
    }
    notes.sort_by_key(|note| (note.start, note.key));
    let envelopes = envelopes
        .into_iter()
        .map(|(mut envelope, points)| {
            envelope.envelope =
                Envelope::from_points(points.into_values().collect()).unwrap_or_default();
            envelope
        })
        .collect();
    Clip {
        id,
        name,
        start,
        length: end - start,
        offset: Ticks::ZERO,
        mute: false,
        data: ClipData::Midi(MidiData {
            notes: notes.into(),
            start: Ticks::ZERO,
            end: None,
            looping: true,
            loop_start: Ticks::ZERO,
            loop_end: None,
            signature: None,
        }),
        envelopes,
        launch: ClipLaunch::default(),
        // Spec: scales R7. A consolidated clip takes the scale of the first
        // clip it was made from.
        scale: clips.first().map_or_else(Scale::default, |clip| clip.scale),
    }
}

/// The merged envelope `envelope`'s points go into, making its entry the
/// first time a clip carries one for its parameter.
fn entry<'a>(
    merged: &'a mut Vec<(ClipEnvelope, BTreeMap<Ticks, Breakpoint>)>,
    envelope: &ClipEnvelope,
) -> &'a mut BTreeMap<Ticks, Breakpoint> {
    let at = merged
        .iter()
        .position(|(held, _)| held.target == envelope.target && held.kind == envelope.kind);
    let at = at.unwrap_or_else(|| {
        merged.push((envelope.clone(), std::collections::BTreeMap::new()));
        merged.len() - 1
    });
    &mut merged[at].1
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

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: warp-modes R25. A clip's pitch change is its Transpose plus its
    // Detune in semitones, each held to its range.
    #[test]
    fn the_pitch_change_sums_and_holds_its_parts() {
        let mut audio = AudioData::new(ContentHash([1; 32]));
        assert_eq!(audio.pitch_change(), 0.0);
        assert_eq!(audio.pitch_ratio(), 1.0);
        audio.transpose = 12;
        audio.detune = 50.0;
        assert_eq!(audio.pitch_change(), 12.5);
        assert!((audio.pitch_ratio() - 2f64.powf(12.5 / 12.0)).abs() < 1e-12);
        // Each part is held to its own range.
        audio.transpose = 60;
        audio.detune = 80.0;
        assert_eq!(audio.pitch_change(), 48.5);
        audio.transpose = -60;
        audio.detune = -80.0;
        assert_eq!(audio.pitch_change(), -48.5);
        // An octave up is twice the speed.
        audio.transpose = 12;
        audio.detune = 0.0;
        assert!((audio.pitch_ratio() - 2.0).abs() < 1e-12);
    }

    // Spec: audio-clip-editing R8. A reversal mirrors the markers, the loop
    // brace and the warp markers about the file.
    #[test]
    fn reversing_mirrors_the_markers_the_loop_and_the_warp_markers() {
        // An unwarped clip's markers are seconds into its file.
        let mut audio = AudioData {
            start: 1.0,
            end: Some(3.0),
            loop_start: 1.5,
            loop_end: Some(2.5),
            ..AudioData::new(ContentHash([1; 32]))
        };
        let reversed = audio.reversed(4.0, None);
        // The region from 1 to 3 becomes one from 1 to 3: L - e to L - s.
        assert_eq!(reversed.start, 1.0);
        assert_eq!(reversed.end, Some(3.0));
        // The loop brace likewise.
        assert_eq!(reversed.loop_start, 1.5);
        assert_eq!(reversed.loop_end, Some(2.5));
        // Reversing again gives the clip back exactly as it was.
        assert_eq!(reversed.reversed(4.0, None), audio);
        // A clip with no End marker keeps none.
        audio.end = None;
        audio.loop_end = None;
        let reversed = audio.reversed(4.0, None);
        assert_eq!(reversed.start, 0.0);
        assert_eq!(reversed.end, Some(3.0));
        assert_eq!(reversed.loop_start, 0.0);
        assert_eq!(reversed.loop_end, Some(2.5));
        assert_eq!(reversed.reversed(4.0, None), audio);

        // A warped clip's markers are ticks of its contents, and its warp
        // markers keep their points in the audio.
        let audio = AudioData {
            warp: true,
            warp_markers: vec![
                WarpMarker {
                    time: 0.0,
                    beat: Ticks::ZERO,
                },
                WarpMarker {
                    time: 2.0,
                    beat: Ticks::QUARTER * 4,
                },
            ],
            tempo: Some(120.0),
            ..AudioData::new(ContentHash([1; 32]))
        };
        let reversed = audio.reversed(4.0, None);
        // Each warp marker's file position is mirrored, and its beat becomes
        // the first marker's beat plus the last marker's less its own.
        assert_eq!(reversed.warp_markers.len(), 2);
        assert_eq!(reversed.warp_markers[0].time, 2.0);
        assert_eq!(reversed.warp_markers[0].beat, Ticks::ZERO);
        assert_eq!(reversed.warp_markers[1].time, 4.0);
        assert_eq!(reversed.warp_markers[1].beat, Ticks::QUARTER * 4);
        // Reversing again gives the clip back exactly as it was.
        assert_eq!(reversed.reversed(4.0, None), audio);
    }
}
