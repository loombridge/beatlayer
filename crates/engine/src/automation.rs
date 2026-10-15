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

use bl_core::time::{SampleRate, TempoMap};
use bl_model::{AutomationTarget, MixerParam, ParamId, TrackId};

use crate::mixer;
use crate::node::NodeKey;

/// One parameter's envelope, placed in engine frames. Its values are
/// read by the node whose parameter it drives.
pub struct PlacedEnvelope {
    /// Its points, in frame order, none at one frame.
    points: Vec<(i64, f64)>,
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
            .map(|(at, value)| {
                (
                    tempo.ticks_to_frames(at, rate).get().saturating_add(shift),
                    value,
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
    /// frames its points fall on, the straight line between the two around
    /// it, and the nearest point's value before the first and after the
    /// last. Reading allocates nothing and is the same whatever the block
    /// size: the value depends on the frame alone.
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
        let index = self.points.partition_point(|&(at, _)| at <= frame) - 1;
        let (start, from) = self.points[index];
        let (end, to) = self.points[index + 1];
        let along = (frame - start) as f64 / (end - start) as f64;
        from + (to - from) * along
    }
}

/// One node's automation: an envelope per automated parameter.
pub struct AutomationList {
    /// One envelope per parameter, ordered by parameter.
    envelopes: Vec<(ParamId, PlacedEnvelope)>,
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

    /// Whether no parameter of the node is automated: what a node whose
    /// envelopes all went is sent, so it follows its controls again.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.envelopes.is_empty()
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
) -> Vec<(NodeKey, AutomationList)> {
    let Some(track) = document.track(track) else {
        return Vec::new();
    };
    let tempo = &document.settings().tempo;
    let mut lists: BTreeMap<NodeKey, Vec<(ParamId, PlacedEnvelope)>> = BTreeMap::new();
    lists.insert(NodeKey::Mixer(track.id()), Vec::new());
    if track.kind().has_sends() {
        for to in document.returns() {
            lists.insert(
                NodeKey::Send {
                    track: track.id(),
                    to,
                },
                Vec::new(),
            );
        }
    }
    for device in track.devices() {
        lists.insert(NodeKey::Device(device.id), Vec::new());
    }
    for (target, automation) in track.automation() {
        if !automation.enabled {
            continue;
        }
        let (key, param) = match *target {
            AutomationTarget::Mixer { param } => (
                NodeKey::Mixer(track.id()),
                match param {
                    MixerParam::Volume => mixer::track::VOLUME,
                    MixerParam::Pan => mixer::track::PAN,
                    MixerParam::Mute => mixer::track::MUTE,
                },
            ),
            AutomationTarget::Send { to } => (
                NodeKey::Send {
                    track: track.id(),
                    to,
                },
                mixer::route::LEVEL,
            ),
            AutomationTarget::Device { device, param } => (NodeKey::Device(device), param),
        };
        let envelope = PlacedEnvelope::placed(&automation.envelope, tempo, rate, delay(key));
        if !envelope.is_empty() {
            lists.entry(key).or_default().push((param, envelope));
        }
    }
    lists
        .into_iter()
        .map(|(key, mut envelopes)| {
            envelopes.sort_by_key(|&(param, _)| param);
            (key, AutomationList { envelopes })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, Ticks};

    use super::*;

    fn envelope(points: &[(Ticks, f64)]) -> bl_model::Envelope {
        bl_model::Envelope::from_points(points.to_vec()).unwrap()
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
