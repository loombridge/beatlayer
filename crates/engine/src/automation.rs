//! Automation playback (WP-2.12): the envelopes one node reads on the audio
//! thread.
//!
//! The compiler places a track's envelopes in engine frames, through the
//! tempo map, and holds each back by its node's delay, so automation acts on
//! the audio it is meant for (WP-2.11); it sends each node its own list as
//! node data ([`Sender::send_data`](crate::Sender::send_data)), the way it
//! sends a clip player its clips. A node reads the value of each of its
//! automated parameters on each frame; an overridden envelope isn't in the
//! list, so the node follows the control instead.
//!
//! Reading allocates nothing, and follows the block size: the value at a
//! frame is the same whichever block it lands in.
//!
//! The rules, and the Live sources behind them, are in the automation spec.

use std::collections::BTreeMap;

use bl_core::time::{SampleRate, TempoMap, Ticks};
use bl_model::modulation::Travel;
use bl_model::modulator::ModTarget;
use bl_model::{AutomationTarget, ClipData, MidiData, MixerParam, ParamId, TrackId, curve_value};

use crate::mixer;
use crate::modbus::{self, Bus};
use crate::node::NodeKey;

/// One parameter's envelope, placed in engine frames. Its values are
/// read by the node whose parameter it drives.
pub struct PlacedEnvelope {
    /// Its points, in frame order, none at one frame, each with the bend of
    /// the segment that starts at it.
    points: Vec<(i64, f64, f64)>,
}

impl PlacedEnvelope {
    /// `envelope` placed under `tempo` at `rate`, with every point's frame
    /// shifted on by `shift` (the node's delay, WP-2.11).
    fn placed(
        envelope: &bl_model::Envelope,
        tempo: &TempoMap,
        rate: SampleRate,
        shift: i64,
    ) -> Self {
        let points = envelope
            .points()
            .map(|point| {
                (
                    tempo
                        .ticks_to_frames(point.at, rate)
                        .get()
                        .saturating_add(shift),
                    point.value,
                    point.bend,
                )
            })
            .collect();
        Self { points }
    }

    /// Whether it holds no points.
    pub(crate) fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The value it holds at `frame`: exactly the point's own value on the
    /// frames its points fall on, the shape of the segment between the two
    /// around it (a bent one follows its curve), and the nearest point's
    /// value before the first and after the last. Reading allocates nothing
    /// and is the same whatever the block size: the value depends on the
    /// frame alone.
    ///
    /// # Panics
    ///
    /// If the envelope has no points, which the compiler doesn't send.
    pub fn value_at(&self, frame: i64) -> f64 {
        let first = self.points[0];
        let last = self.points[self.points.len() - 1];
        if frame <= first.0 {
            return first.1;
        }
        if frame >= last.0 {
            return last.1;
        }
        let index = self.points.partition_point(|&(at, _, _)| at <= frame) - 1;
        let (start, from, bend) = self.points[index];
        let (end, to, _) = self.points[index + 1];
        let along = (frame - start) as f64 / (end - start) as f64;
        curve_value(from, to, along, bend)
    }
}

/// One clip's envelope, placed in song frames (WP-3.15): the stretches of
/// song time its clip plays, with the envelope's breakpoints placed in the
/// frames the clip's contents reach their positions at. Its value is read
/// frame by frame, the way a track envelope's is.
struct PlacedClipEnvelope {
    /// The stretches the clip plays, in frame order, in song frames.
    segments: Vec<PlacedSegment>,
}

/// One stretch of a clip's playback: the frames it runs between, and the
/// envelope's breakpoints placed in song frames, each with the bend of the
/// segment starting at it.
struct PlacedSegment {
    first_frame: i64,
    end_frame: i64,
    points: Vec<(i64, f64, f64)>,
}

impl PlacedClipEnvelope {
    /// `envelope` placed for `clip` under `tempo` at `rate`, with every
    /// frame shifted on by `shift` (the node's delay, WP-2.11), the way a
    /// track envelope is. `None` for a clip that plays nothing, or an
    /// envelope with no breakpoints: neither modulates anything.
    fn placed(
        clip: &bl_model::Clip,
        envelope: &bl_model::ClipEnvelope,
        tempo: &TempoMap,
        rate: SampleRate,
        shift: i64,
    ) -> Option<Self> {
        let ClipData::Midi(midi) = &clip.data else {
            return None;
        };
        if envelope.envelope.is_empty() {
            return None;
        }
        let mut segments = Vec::new();
        for (song_from, song_to, content_from, _) in played_regions(clip, midi, envelope) {
            let first_frame = tempo
                .ticks_to_frames(song_from, rate)
                .get()
                .saturating_add(shift);
            let end_frame = tempo
                .ticks_to_frames(song_to, rate)
                .get()
                .saturating_add(shift);
            if end_frame <= first_frame {
                continue;
            }
            // Each breakpoint's own frame: its content position maps one
            // tick per tick onto the song, and the tempo map places that
            // frame exactly, so a breakpoint lands where the note beside it
            // plays.
            let points = envelope
                .envelope
                .points()
                .map(|point| {
                    let song = song_from + (point.at - content_from);
                    (
                        tempo
                            .ticks_to_frames(song, rate)
                            .get()
                            .saturating_add(shift),
                        point.value,
                        point.bend,
                    )
                })
                .collect();
            segments.push(PlacedSegment {
                first_frame,
                end_frame,
                points,
            });
        }
        if segments.is_empty() {
            return None;
        }
        Some(Self { segments })
    }

    /// The value it holds at `frame`: the envelope's own value at the
    /// content position that frame sits at, `None` while the clip doesn't
    /// play. Reading allocates nothing and is the same whatever the block
    /// size: the value depends on the frame alone.
    // Spec: clip-envelopes R4
    fn value_at(&self, frame: i64) -> Option<f64> {
        let index = self
            .segments
            .partition_point(|segment| segment.first_frame <= frame);
        let segment = self.segments.get(index.checked_sub(1)?)?;
        if frame >= segment.end_frame {
            return None;
        }
        let points = &segment.points;
        let first = points[0];
        let last = points[points.len() - 1];
        if frame <= first.0 {
            return Some(first.1);
        }
        if frame >= last.0 {
            return Some(last.1);
        }
        let index = points.partition_point(|&(at, _, _)| at <= frame) - 1;
        let (start, from, bend) = points[index];
        let (end, to, _) = points[index + 1];
        let along = (frame - start) as f64 / (end - start) as f64;
        Some(curve_value(from, to, along, bend))
    }
}

/// The stretches of song time a clip plays, each with the part of its
/// contents that plays there, in ticks: `(song start, song end, content
/// start, content end)`, in order. A linked envelope (WP-3.15) runs over
/// what the clip's contents play, which is this mapping, and the player's
/// notes play by the same one (`crate::midi_clips`). An unlinked one runs
/// once from the clip's start, one tick per tick, for as long as the clip
/// lasts.
// Spec: clip-envelopes R2
fn played_regions(
    clip: &bl_model::Clip,
    midi: &MidiData,
    envelope: &bl_model::ClipEnvelope,
) -> Vec<(Ticks, Ticks, Ticks, Ticks)> {
    let (start, length) = (clip.start, clip.length);
    if length <= Ticks::ZERO || start < Ticks::ZERO {
        return Vec::new();
    }
    if !envelope.linked {
        return vec![(start, start + length, Ticks::ZERO, length)];
    }
    let mut regions = Vec::new();
    if !midi.looping {
        let end_marker = midi.end.unwrap_or(midi.start + length);
        if end_marker > midi.start {
            let active = length.min(end_marker - midi.start);
            regions.push((start, start + active, midi.start, midi.start + active));
        }
        return regions;
    }
    let loop_end = midi
        .loop_end
        .unwrap_or_else(|| (midi.loop_start + length).max(midi.loop_start + Ticks::QUARTER));
    if loop_end <= midi.loop_start || midi.start >= loop_end {
        return regions;
    }
    let loop_len = loop_end - midi.loop_start;
    // Repetition 0: from the start marker to the brace's end, cut off where
    // the clip ends. Then the brace repeats.
    let first = (loop_end - midi.start).min(length);
    regions.push((start, start + first, midi.start, midi.start + first));
    let mut at = first;
    while at < length {
        let repeat = loop_len.min(length - at);
        regions.push((
            start + at,
            start + at + repeat,
            midi.loop_start,
            midi.loop_start + repeat,
        ));
        at += repeat;
    }
    regions
}

/// One node's automation: an envelope per automated parameter, and the
/// clips' envelopes modulating them.
pub struct AutomationList {
    /// One envelope per parameter, ordered by parameter.
    envelopes: Vec<(ParamId, PlacedEnvelope)>,
    /// The clips' envelopes (WP-3.15), one entry per parameter they
    /// modulate, ordered by parameter.
    modulation: Vec<(ParamId, Vec<PlacedClipEnvelope>)>,
    /// The modulator slots (WP-5.10) that map each parameter, as the first
    /// of their two rows on the bus ([`crate::modbus::row`]), ordered by
    /// parameter.
    mapped: Vec<(ParamId, Vec<u32>)>,
}

impl AutomationList {
    /// The envelope driving `param`, if the parameter is automated. The
    /// rest of the node's parameters follow their controls.
    pub fn envelope(&self, param: ParamId) -> Option<&PlacedEnvelope> {
        let index = self
            .envelopes
            .binary_search_by_key(&param, |&(param, _)| param)
            .ok()?;
        Some(&self.envelopes[index].1)
    }

    /// The value the node's `param` plays, `base` being its value before
    /// modulation (its envelope's, a modulator's hold, or its control's):
    /// the clips' envelopes playing at song frame `song` (none while the
    /// song is stopped) and the modulators mapping it, read from `bus` at
    /// frame `at` of the block, each contribute their share of the
    /// parameter's travel, the contributions add, and the sum moves the
    /// base's position on `travel`, clamped to its ends (the modulation
    /// spec, R3, R4 and R14). With nothing modulating the parameter it is
    /// `base`, to the bit. The one place a parameter's played value is worked
    /// out: a macro's hold changes `base`, and a new kind of source adds to
    /// the sum here.
    // Spec: modulation R3
    #[must_use]
    pub fn played(
        &self,
        travel: Travel<'_>,
        param: ParamId,
        song: Option<i64>,
        base: f64,
        bus: Bus<'_>,
        at: usize,
    ) -> f64 {
        let mut sum = 0.0;
        if let (Some(frame), Ok(index)) = (
            song,
            self.modulation
                .binary_search_by_key(&param, |&(param, _)| param),
        ) {
            sum += self.modulation[index]
                .1
                .iter()
                .filter_map(|envelope| envelope.value_at(frame))
                .map(|share| travel.clip_contribution(base, share))
                .sum::<f64>();
        }
        if let Some(rows) = self.rows(param) {
            sum += rows
                .iter()
                .filter_map(|&row| bus.contribution(row).get(at))
                .map(|&share| f64::from(share))
                .sum::<f64>();
        }
        travel.played(base, sum)
    }

    /// The position on `param`'s travel a modulator in Remote Control holds
    /// it at on frame `at` of the block, if one does (R16): the lowest slot
    /// holding it, as a parameter has one holder.
    // Spec: modulation R16
    #[must_use]
    pub fn held(&self, param: ParamId, bus: Bus<'_>, at: usize) -> Option<f64> {
        self.rows(param)?
            .iter()
            .filter_map(|&row| bus.hold(row).get(at))
            .map(|&position| f64::from(position))
            .find(|&position| position >= 0.0)
    }

    fn rows(&self, param: ParamId) -> Option<&[u32]> {
        let index = self
            .mapped
            .binary_search_by_key(&param, |&(param, _)| param)
            .ok()?;
        Some(&self.mapped[index].1)
    }

    /// Calls `visit` with the id, base and played value at frame `at` of the
    /// block, song frame `song` (none while stopped), of each parameter an
    /// envelope drives, a clip or a modulator modulates or holds, in
    /// parameter order: for a node that plays the values itself, as a
    /// plug-in does. `travel_of` gives a parameter's travel (`None` skips
    /// it) and `control_of` its control's value, the base where nothing
    /// drives it. It allocates nothing.
    pub fn drive<'a>(
        &self,
        song: Option<i64>,
        bus: Bus<'_>,
        at: usize,
        travel_of: impl Fn(ParamId) -> Option<Travel<'a>>,
        control_of: impl Fn(ParamId) -> f64,
        mut visit: impl FnMut(ParamId, f64, f64),
    ) {
        let driven = self
            .envelopes
            .iter()
            .map(|&(param, _)| param)
            .chain(
                self.modulation
                    .iter()
                    .map(|&(param, _)| param)
                    .filter(|&param| self.envelope(param).is_none()),
            )
            .chain(
                self.mapped
                    .iter()
                    .map(|&(param, _)| param)
                    .filter(|&param| {
                        self.envelope(param).is_none() && !self.modulates_by_clip(param)
                    }),
            );
        for param in driven {
            let Some(travel) = travel_of(param) else {
                continue;
            };
            let base = self.base(travel, param, song, bus, at, &control_of);
            visit(param, base, self.played(travel, param, song, base, bus, at));
        }
    }

    /// The value `param` has before modulation on frame `at`: a modulator's
    /// hold, else its envelope's value at song frame `song`, else `control`.
    pub fn base(
        &self,
        travel: Travel<'_>,
        param: ParamId,
        song: Option<i64>,
        bus: Bus<'_>,
        at: usize,
        control_of: impl Fn(ParamId) -> f64,
    ) -> f64 {
        if let Some(position) = self.held(param, bus, at) {
            return travel.value(position);
        }
        match (song, self.envelope(param)) {
            (Some(frame), Some(envelope)) => envelope.value_at(frame),
            _ => control_of(param),
        }
    }

    /// Whether a modulator slot maps `param`.
    #[must_use]
    pub fn maps(&self, param: ParamId) -> bool {
        self.rows(param).is_some()
    }

    /// Whether any modulator slot maps a parameter of the node.
    #[must_use]
    pub fn has_mapped(&self) -> bool {
        !self.mapped.is_empty()
    }

    fn modulates_by_clip(&self, param: ParamId) -> bool {
        self.modulation
            .binary_search_by_key(&param, |&(param, _)| param)
            .is_ok()
    }

    /// Whether any clip envelope or modulator slot modulates or holds
    /// `param`: what a node checks before its fast paths, since a parameter
    /// with neither a track envelope nor modulation follows its control
    /// alone.
    #[must_use]
    pub fn modulates(&self, param: ParamId) -> bool {
        self.modulates_by_clip(param) || self.rows(param).is_some()
    }

    /// Whether neither a track envelope, a clip envelope nor a modulator
    /// drives any of the node's parameters: what a node whose envelopes all
    /// went is sent, so it follows its controls again.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.envelopes.is_empty() && self.modulation.is_empty() && self.mapped.is_empty()
    }
}

/// The automation `track` plays, as one list per node that can be
/// automated: the track's mixer, each of its sends, and each device in its
/// chain. `delay` gives each node's held-back frames (WP-2.11). An
/// overridden envelope is left out, so its parameter follows the control,
/// and a node whose envelopes all went gets an empty list, which clears the
/// ones it had.
pub(crate) fn lists(
    document: &bl_model::Document,
    track: TrackId,
    rate: SampleRate,
    delay: impl Fn(NodeKey) -> i64,
    rows: &modbus::ModRows,
) -> Vec<(NodeKey, AutomationList)> {
    let Some(track) = document.track(track) else {
        return Vec::new();
    };
    let tempo = &document.settings().tempo;
    let mut lists: BTreeMap<NodeKey, Entries> = BTreeMap::new();
    lists.insert(NodeKey::Mixer(track.id()), Entries::default());
    if track.kind().has_sends() {
        for to in document.returns() {
            lists.insert(
                NodeKey::Send {
                    track: track.id(),
                    to,
                },
                Entries::default(),
            );
        }
    }
    for device in track.devices_deep() {
        lists.insert(NodeKey::Device(device.id), Entries::default());
        for chain in &device.chains {
            lists.insert(NodeKey::ChainMixer(chain.id), Entries::default());
        }
    }
    for (target, automation) in track.automation() {
        if !automation.enabled {
            continue;
        }
        let (key, param) = target_node(track.id(), target);
        let envelope = PlacedEnvelope::placed(&automation.envelope, tempo, rate, delay(key));
        if !envelope.is_empty() {
            lists
                .entry(key)
                .or_default()
                .envelopes
                .push((param, envelope));
        }
    }
    // The clips' envelopes (WP-3.15): every clip on the track's Arrangement
    // lane that plays and carries one modulates the parameter it names
    // while the clip plays.
    for clip in track.clips() {
        if clip.mute {
            continue;
        }
        for envelope in clip.envelopes() {
            let (key, param) = target_node(track.id(), &envelope.target);
            let placed = PlacedClipEnvelope::placed(clip, envelope, tempo, rate, delay(key));
            if let Some(placed) = placed {
                lists
                    .entry(key)
                    .or_default()
                    .modulation
                    .push((param, placed));
            }
        }
    }
    // The modulators' slots (WP-5.10) that map this track's nodes' parameters,
    // from any track.
    for owner in document.tracks() {
        for device in owner.devices_deep() {
            for mapping in &device.mappings {
                let Some(row) = rows.row_of(document, owner, device, mapping) else {
                    continue;
                };
                let (key, param) = match mapping.target {
                    ModTarget::Mixer { track: t, param } if t == track.id() => {
                        target_node(t, &AutomationTarget::Mixer { param })
                    }
                    ModTarget::Send { track: t, to } if t == track.id() => {
                        target_node(t, &AutomationTarget::Send { to })
                    }
                    ModTarget::Device { device: d, param }
                        if track.devices_deep().any(|x| x.id == d) =>
                    {
                        (NodeKey::Device(d), param)
                    }
                    _ => continue,
                };
                if let Some(entries) = lists.get_mut(&key) {
                    entries.mapped.push((param, row + mapping.slot as u32 * 2));
                }
            }
        }
    }
    lists
        .into_iter()
        .map(|(key, mut entries)| {
            entries.envelopes.sort_by_key(|&(param, _)| param);
            entries.modulation.sort_by_key(|&(param, _)| param);
            // However many clips modulate one parameter, the node reads
            // them together.
            let mut modulation: Vec<(ParamId, Vec<PlacedClipEnvelope>)> = Vec::new();
            for (param, envelope) in entries.modulation {
                match modulation.last_mut() {
                    Some((last, envelopes)) if *last == param => envelopes.push(envelope),
                    _ => modulation.push((param, vec![envelope])),
                }
            }
            entries.mapped.sort_unstable();
            let mut mapped: Vec<(ParamId, Vec<u32>)> = Vec::new();
            for (param, row) in entries.mapped {
                match mapped.last_mut() {
                    Some((last, rows)) if *last == param => rows.push(row),
                    _ => mapped.push((param, vec![row])),
                }
            }
            (
                key,
                AutomationList {
                    envelopes: entries.envelopes,
                    modulation,
                    mapped,
                },
            )
        })
        .collect()
}

/// The node and parameter an automation target names on `track`: its mixer
/// for a mixer control, its send to a return track, or a device in its
/// chain.
pub fn target_node(track: TrackId, target: &AutomationTarget) -> (NodeKey, ParamId) {
    match *target {
        AutomationTarget::Mixer { param } => (
            NodeKey::Mixer(track),
            match param {
                MixerParam::Volume => mixer::track::VOLUME,
                MixerParam::Pan => mixer::track::PAN,
                MixerParam::Mute => mixer::track::MUTE,
            },
        ),
        AutomationTarget::Send { to } => (NodeKey::Send { track, to }, mixer::route::LEVEL),
        AutomationTarget::Device { device, param } => (NodeKey::Device(device), param),
        AutomationTarget::Chain { chain, param } => (
            NodeKey::ChainMixer(chain),
            match param {
                MixerParam::Volume => mixer::track::VOLUME,
                MixerParam::Pan => mixer::track::PAN,
                MixerParam::Mute => mixer::track::MUTE,
            },
        ),
    }
}

/// The value `clip`'s `envelope` holds at song position `at`, in the
/// envelope's own units, if the clip plays there (the modulation spec, R11):
/// what a reader that has no engine frame asks for, by the same mapping of
/// the clip's contents onto the song as the engine's read path.
#[must_use]
pub fn clip_envelope_value_at(
    clip: &bl_model::Clip,
    envelope: &bl_model::ClipEnvelope,
    at: Ticks,
) -> Option<f64> {
    let ClipData::Midi(midi) = &clip.data else {
        return None;
    };
    played_regions(clip, midi, envelope)
        .into_iter()
        .find(|&(from, to, ..)| at >= from && at < to)
        .and_then(|(from, _, content, _)| envelope.envelope.value_at(content + (at - from)))
}

/// What one node is sent: the track envelopes driving its parameters and
/// the clips' envelopes modulating them, each to sort by parameter before
/// they're sent.
#[derive(Default)]
struct Entries {
    envelopes: Vec<(ParamId, PlacedEnvelope)>,
    modulation: Vec<(ParamId, PlacedClipEnvelope)>,
    mapped: Vec<(ParamId, u32)>,
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, Ticks};

    use super::*;

    fn envelope(points: &[(Ticks, f64)]) -> bl_model::Envelope {
        bl_model::Envelope::from_points(
            points
                .iter()
                .map(|&(at, value)| bl_model::Breakpoint {
                    at,
                    value,
                    bend: 0.0,
                })
                .collect(),
        )
        .unwrap()
    }

    fn curved(points: &[(Ticks, f64, f64)]) -> bl_model::Envelope {
        bl_model::Envelope::from_points(
            points
                .iter()
                .map(|&(at, value, bend)| bl_model::Breakpoint { at, value, bend })
                .collect(),
        )
        .unwrap()
    }

    // Spec: modulation R3
    #[test]
    fn a_list_without_clips_plays_its_base() {
        let envelope = PlacedEnvelope::placed(
            &envelope(&[(Ticks::ZERO, 0.25), (Ticks::QUARTER, 0.75)]),
            &TempoMap::default(),
            SampleRate::HZ_48000,
            0,
        );
        let list = AutomationList {
            envelopes: vec![(ParamId(1), envelope)],
            modulation: Vec::new(),
            mapped: Vec::new(),
        };
        let mut seen = Vec::new();
        list.drive(
            Some(12_000),
            Bus::EMPTY,
            0,
            |_| Some(Travel::Pan),
            |_| 0.0,
            |param, base, played| seen.push((param, base, played)),
        );
        assert_eq!(seen, [(ParamId(1), 0.5, 0.5)], "no modulation, no change");
        assert_eq!(
            list.played(Travel::Pan, ParamId(2), Some(0), 0.3, Bus::EMPTY, 0),
            0.3
        );
    }

    // Spec: automation R12
    #[test]
    fn a_curved_segment_follows_its_bend() {
        let bent = curved(&[(Ticks::ZERO, 0.0, -1.0), (Ticks::QUARTER, 1.0, 0.0)]);
        let placed = PlacedEnvelope::placed(&bent, &TempoMap::default(), SampleRate::HZ_48000, 0);
        assert_eq!(placed.value_at(0), 0.0, "the breakpoint's own value");
        assert_eq!(placed.value_at(24_000), 1.0, "the far breakpoint's");
        let half = placed.value_at(12_000);
        assert!((half - 0.5f64.sqrt()).abs() < 1e-12, "{half}");
        assert!(half > 0.5, "the curve leads the straight line");
        // The other sign bows the other way, and 0 is the exact straight
        // line.
        let mirrored = curved(&[(Ticks::ZERO, 0.0, 1.0), (Ticks::QUARTER, 1.0, 0.0)]);
        let placed =
            PlacedEnvelope::placed(&mirrored, &TempoMap::default(), SampleRate::HZ_48000, 0);
        assert!((placed.value_at(12_000) - 0.25).abs() < 1e-12);
        let straight = PlacedEnvelope::placed(
            &envelope(&[(Ticks::ZERO, 0.0), (Ticks::QUARTER, 1.0)]),
            &TempoMap::default(),
            SampleRate::HZ_48000,
            0,
        );
        assert_eq!(straight.value_at(12_000), 0.5);
    }

    // Spec: automation R2
    #[test]
    fn a_ramp_reaches_its_target_on_the_exact_frame() {
        // 120 BPM throughout: a quarter note is 24,000 frames at 48 kHz.
        let quarter = Ticks::QUARTER;
        let ramp = PlacedEnvelope::placed(
            &envelope(&[(Ticks::ZERO, 1.0), (quarter, 0.0)]),
            &TempoMap::default(),
            SampleRate::HZ_48000,
            0,
        );
        assert_eq!(ramp.value_at(0), 1.0);
        assert_eq!(ramp.value_at(24_000), 0.0, "the target on its exact frame");
        assert_eq!(ramp.value_at(12_000), 0.5);
        // Before the first point and after the last, the nearest one holds.
        assert_eq!(ramp.value_at(-5), 1.0);
        assert_eq!(ramp.value_at(100_000), 0.0);
        // The value is the frame's alone: reading out of order gives the
        // same answer.
        let shuffled = [7, 23_999, 0, 24_000, 1, 12_000];
        let once: Vec<f64> = shuffled.iter().map(|&frame| ramp.value_at(frame)).collect();
        let twice: Vec<f64> = shuffled
            .iter()
            .rev()
            .map(|&frame| ramp.value_at(frame))
            .collect();
        assert_eq!(once, twice.into_iter().rev().collect::<Vec<_>>());
    }

    // Spec: automation R2
    #[test]
    fn an_envelope_lands_on_account_of_the_tempo_and_the_delay() {
        let tempo = TempoMap::from_breakpoints([
            (Ticks::ZERO, Bpm::new(120.0).unwrap()),
            (Ticks::QUARTER * 4, Bpm::new(240.0).unwrap()),
        ])
        .unwrap();
        let envelope = envelope(&[(Ticks::ZERO, 0.0), (Ticks::QUARTER * 8, 1.0)]);
        let rate = SampleRate::HZ_48000;
        let placed = PlacedEnvelope::placed(&envelope, &tempo, rate, 0);
        let end = tempo.ticks_to_frames(Ticks::QUARTER * 8, rate).get();
        assert!(end > 0);
        assert_eq!(placed.value_at(0), 0.0);
        assert_eq!(
            placed.value_at(end),
            1.0,
            "the frame the tempo map puts it on"
        );
        assert_eq!(placed.value_at(end / 2), 0.5);
        // Held back by a delay, the same value lands later.
        let held = PlacedEnvelope::placed(&envelope, &tempo, rate, 1_024);
        assert_eq!(held.value_at(1_024), 0.0);
        assert_eq!(held.value_at(end + 1_024), 1.0);
    }
}
