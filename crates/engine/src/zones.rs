//! Rack zones (WP-5.3; the racks spec, R30 to R37): the rack's chain
//! selector and, for each chain, the filter at its input.
//!
//! The selector is a node of its own under the rack's key. It works out the
//! selector's value for every frame of a block, from its control, the
//! envelopes and the modulation driving it, and the rack hands that to its
//! chains. A chain's [`ZoneFilter`] passes or stops the notes that reach it
//! by its key, velocity and chain select zones, and in an audio effect rack
//! opens and closes its audio input; it also scales the chain's output by
//! its chain select zone's fades. Every gain glides over 20 ms.

use bl_model::modulation::Travel;
use bl_model::{
    CHAIN_SELECTOR, ChainZones, Device, FILTER_MIDI_CTRL, FadeMode, ParamId, RackKind, Zone,
    ZoneKind, rack_description,
};

use crate::automation::AutomationList;
use crate::mixer::{Ramp, smoothing_frames};
use crate::node::{
    Block, Node, NodeConfig, NodeData, NodeEvent, NoteBuffer, NoteEvent, NoteKind, ParamValue,
    segments,
};

/// The zone parameters of a chain's [`ZoneFilter`]: four numbers for each
/// zone in the order key, velocity, select.
pub(crate) fn zone_param(kind: ZoneKind, field: u32) -> ParamId {
    ParamId(
        4 * match kind {
            ZoneKind::Key => 0,
            ZoneKind::Velocity => 1,
            ZoneKind::Select => 2,
        } + field,
    )
}

/// A chain's zones as the parameters of its filter: each zone's `low`,
/// `high`, `fade_low` and `fade_high`.
pub(crate) fn zone_values(zones: &ChainZones) -> impl Iterator<Item = (ParamId, f64)> {
    [ZoneKind::Key, ZoneKind::Velocity, ZoneKind::Select]
        .into_iter()
        .flat_map(move |kind| {
            let zone = zones.get(kind);
            [zone.low, zone.high, zone.fade_low, zone.fade_high]
                .into_iter()
                .enumerate()
                .map(move |(field, value)| (zone_param(kind, field as u32), f64::from(value)))
        })
}

/// The rack's chain selector and Chain Selector Filters MIDI Ctrl.
#[derive(Debug)]
pub struct ChainSelector {
    /// A drum rack's chains have no zones (R30), so its selector stays 0.
    fixed: bool,
    value: f64,
    filter_ctrl: bool,
    /// The envelopes driving the selector while the song plays.
    automation: Option<NodeData>,
    /// The selector's whole value on each frame of the last block.
    values: Vec<f32>,
}

impl ChainSelector {
    /// A selector set to `rack`'s values, for blocks of up to
    /// `config.max_frames` frames.
    #[must_use]
    pub fn new(rack: &Device, config: &NodeConfig) -> Self {
        // Made here, off the audio thread, where `process` reads it.
        let _ = rack_description();
        let mut selector = Self {
            fixed: rack.rack_kind() == Some(RackKind::Drum),
            value: 0.0,
            filter_ctrl: false,
            automation: None,
            values: vec![0.0; config.max_frames.max(1)],
        };
        for (param, value) in &rack.params {
            selector.set(*param, ParamValue::Set(*value));
        }
        selector
    }

    fn set(&mut self, param: ParamId, value: ParamValue) {
        let value = match value {
            ParamValue::Set(value) if value.is_finite() => Some(value),
            _ => None,
        };
        match param {
            CHAIN_SELECTOR if !self.fixed => {
                self.value = value.unwrap_or(0.0).round().clamp(0.0, 127.0);
            }
            FILTER_MIDI_CTRL => self.filter_ctrl = value.is_some_and(|v| v >= 0.5),
            _ => {}
        }
    }

    /// The selector's value on each frame of the last block.
    pub(crate) fn values(&self, frames: usize) -> &[f32] {
        &self.values[..frames.min(self.values.len())]
    }

    /// Whether the chain select zones filter messages other than notes.
    pub(crate) fn filters_ctrl(&self) -> bool {
        self.filter_ctrl
    }

    /// Takes the envelopes driving the selector.
    pub(crate) fn receive_data(&mut self, data: NodeData) {
        if data.downcast_ref::<AutomationList>().is_some() {
            self.automation = Some(data);
        }
    }
}

impl Node for ChainSelector {
    fn process(&mut self, block: &mut Block<'_>) {
        let frames = block.frames().min(self.values.len());
        let transport = block.transport();
        let bus = block.modulation();
        let song = transport.playing.then(|| transport.position.get());
        let automation = self.automation.clone();
        let list = automation
            .as_ref()
            .and_then(|data| data.downcast_ref::<AutomationList>())
            .filter(|list| !list.is_empty());
        let description = rack_description();
        let descriptor = description
            .param(CHAIN_SELECTOR)
            .expect("a rack describes its chain selector");
        let travel = Travel::Param(descriptor);
        let now = list.map(|list| block.session_now(list));
        for segment in segments(block.events(), frames) {
            for event in segment.events {
                self.set(event.param, event.value);
            }
            for at in segment.range {
                let value = match (list, &now) {
                    _ if self.fixed => 0.0,
                    (Some(list), Some(now)) => {
                        let song = song.map(|from| from + at as i64);
                        let base =
                            list.base(travel, CHAIN_SELECTOR, song, now, bus, at, |_| self.value);
                        descriptor.clamp(list.played(travel, CHAIN_SELECTOR, song, base, bus, at))
                    }
                    _ => self.value,
                };
                self.values[at] = value.round() as f32;
            }
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.set(event.param, event.value);
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        ChainSelector::receive_data(self, data);
    }
}

/// How many MIDI channels and keys a chain remembers notes for.
const HELD_WORDS: usize = 16 * 128 / 64;

/// The filter at a chain's input.
#[derive(Debug)]
pub(crate) struct ZoneFilter {
    zones: ChainZones,
    /// The notes it passed that haven't ended, a bit for each channel and
    /// key: their note-offs reach the chain whatever the zones are by then
    /// (R31).
    held: [u64; HELD_WORDS],
    /// The audio effect rack's gate on the chain's input (R33).
    gate: Ramp,
    /// The gain on the chain's output (R34).
    out: Ramp,
    glide: u32,
    /// Whether the chain was processing in the last block (R37).
    processing: bool,
}

impl ZoneFilter {
    pub(crate) fn new(zones: &ChainZones, config: &NodeConfig) -> Self {
        Self {
            zones: *zones,
            held: [0; HELD_WORDS],
            gate: Ramp::at(1.0),
            out: Ramp::at(1.0),
            glide: smoothing_frames(config.sample_rate),
            processing: false,
        }
    }

    /// Takes zone changes: each event is one number of one zone.
    pub(crate) fn apply(&mut self, events: &[NodeEvent]) {
        for event in events {
            let ParamValue::Set(value) = event.value else {
                continue;
            };
            let (kind, field) = match event.param.0 {
                n @ 0..=3 => (ZoneKind::Key, n),
                n @ 4..=7 => (ZoneKind::Velocity, n - 4),
                n @ 8..=11 => (ZoneKind::Select, n - 8),
                _ => continue,
            };
            let mut zone = self.zones.get(kind);
            let value = value.clamp(0.0, 127.0) as u8;
            match field {
                0 => zone.low = value,
                1 => zone.high = value,
                2 => zone.fade_low = value,
                _ => zone.fade_high = value,
            }
            self.zones = self.zones.with(kind, zone);
        }
    }

    /// Whether the chain was processing in the last block (R37): in an
    /// instrument or MIDI effect rack, a note it passed is held; in an
    /// audio effect rack, its chain select zone held the selector.
    pub(crate) fn processing(&self) -> bool {
        self.processing
    }

    /// The chain isn't run: it isn't processing.
    pub(crate) fn idle(&mut self) {
        self.processing = false;
    }

    fn bit(note: &NoteEvent) -> (usize, u64) {
        let at = usize::from(note.channel & 15) * 128 + usize::from(note.key & 127);
        (at / 64, 1 << (at % 64))
    }

    /// Passes `notes`, which reach the chain, to `out` as the zones allow
    /// (R31, R32): a note-on in its key, velocity and chain select zones,
    /// with its velocity scaled by the fades it is in; a note-off, and
    /// whatever else belongs to a note, if its note-on passed; any other
    /// message unless `filter_ctrl`, and then if the chain select zone holds
    /// the selector. In a MIDI effect rack (`scaled_by_select`) the chain
    /// select fades scale the velocity too (R34). `selector` is the
    /// selector's value on each frame.
    pub(crate) fn filter(
        &mut self,
        selector: &[f32],
        notes: &[NoteEvent],
        out: &mut NoteBuffer,
        filter_ctrl: bool,
        scaled_by_select: bool,
    ) {
        let zones = self.zones;
        for note in notes {
            let selected = selector
                .get(note.offset as usize)
                .or(selector.last())
                .map_or(0, |&v| v as u8);
            match note.kind {
                NoteKind::On => {
                    let velocity = ((note.velocity * 127.0).round() as u8).clamp(1, 127);
                    if !(zones.key.contains(note.key)
                        && zones.velocity.contains(velocity)
                        && zones.select.contains(selected))
                    {
                        continue;
                    }
                    let (word, bit) = Self::bit(note);
                    self.held[word] |= bit;
                    let mut share = zones.key.gain(note.key, FadeMode::Lin)
                        * zones.velocity.gain(velocity, FadeMode::Lin);
                    if scaled_by_select {
                        share *= zones.select.gain(selected, FadeMode::Lin);
                    }
                    let mut note = *note;
                    if share < 1.0 {
                        note.velocity = (f32::from(velocity) * share).round().max(1.0) / 127.0;
                    }
                    out.push(note);
                }
                NoteKind::Off | NoteKind::Choke | NoteKind::Expression { .. } => {
                    let (word, bit) = Self::bit(note);
                    if self.held[word] & bit != 0 {
                        if !matches!(note.kind, NoteKind::Expression { .. }) {
                            self.held[word] &= !bit;
                        }
                        out.push(*note);
                    }
                }
                NoteKind::Control { .. }
                | NoteKind::PitchBend { .. }
                | NoteKind::ChannelPressure { .. } => {
                    if !filter_ctrl || zones.select.contains(selected) {
                        out.push(*note);
                    }
                }
            }
        }
        self.processing = self.held.iter().any(|&word| word != 0);
    }

    /// Opens or closes the chain's audio input as the selector enters or
    /// leaves its chain select zone, over 20 ms (R33).
    pub(crate) fn gate(
        &mut self,
        selector: &[f32],
        data: &mut [f32],
        channels: usize,
        stride: usize,
    ) {
        for (at, &value) in selector.iter().enumerate() {
            let open = self.zones.select.contains(value as u8);
            self.gate.toward(f64::from(u8::from(open)), self.glide);
            let gain = self.gate.next() as f32;
            if gain != 1.0 {
                for c in 0..channels {
                    data[c * stride + at] *= gain;
                }
            }
        }
        self.processing = selector
            .last()
            .is_some_and(|&value| self.zones.select.contains(value as u8));
    }

    /// Scales the chain's output by its chain select zone's fades (R34).
    pub(crate) fn scale(
        &mut self,
        selector: &[f32],
        data: &mut [f32],
        channels: usize,
        stride: usize,
    ) {
        for (at, &value) in selector.iter().enumerate() {
            self.out
                .toward(output_gain(&self.zones.select, value as u8), self.glide);
            let gain = self.out.next() as f32;
            if gain != 1.0 {
                for c in 0..channels {
                    data[c * stride + at] *= gain;
                }
            }
        }
    }
}

/// The gain a chain select zone gives the chain's output when the selector
/// is at `value` (R34): the fade's share inside the zone, silence beyond an
/// end that has a fade, and no attenuation beyond one that has none.
fn output_gain(zone: &Zone, value: u8) -> f64 {
    if zone.contains(value) {
        f64::from(zone.gain(value, FadeMode::Lin))
    } else if value < zone.low {
        f64::from(u8::from(zone.fade_low == 0))
    } else {
        f64::from(u8::from(zone.fade_high == 0))
    }
}
