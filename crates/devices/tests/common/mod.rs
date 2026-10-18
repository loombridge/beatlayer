//! What the devices' tests share: a node of a device type, run on its own,
//! each block a real-time section under the allocation trap.

#![allow(dead_code, reason = "each test file uses some")]

pub mod chain;

use bl_core::id::Id;
use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_engine::{
    Block, DeviceType, Node, NodeConfig, NodeEvent, ParamValue, Registry, TransportInfo,
};
use bl_model::{Device, DeviceId, ParamId};

pub const RATE: SampleRate = SampleRate::HZ_48000;
pub const CHANNELS: usize = 2;

pub fn config(max_frames: usize) -> NodeConfig {
    NodeConfig {
        sample_rate: RATE,
        max_frames,
        channels: CHANNELS,
    }
}

/// A device of `device_type` with `params` set.
pub fn device(device_type: &DeviceType, params: &[(ParamId, f64)]) -> Device {
    let mut device = device_type.device(DeviceId(Id::new(1).expect("1 is an ID")));
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    device
}

/// The node `registry` makes for `device`.
pub fn node(registry: &Registry, device: &Device, max_frames: usize) -> Box<dyn Node> {
    registry
        .create(device, &config(max_frames))
        .expect("the registry makes the device")
}

pub fn set(offset: u32, param: ParamId, value: f64) -> NodeEvent {
    NodeEvent {
        offset,
        param,
        value: ParamValue::Set(value),
    }
}

/// Runs one block of `input` (the same on every channel) through `node`
/// with `events`, as a real-time section, and returns each channel.
pub fn run(node: &mut dyn Node, input: &[f32], events: &[NodeEvent]) -> Vec<Vec<f32>> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..CHANNELS).flat_map(|_| input.iter().copied()).collect();
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        );
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// Runs one block of per-channel `input` through `node` at `tempo` and
/// `transport`, as a real-time section, and returns each channel.
pub fn run_at(
    node: &mut dyn Node,
    input: &[Vec<f32>],
    events: &[NodeEvent],
    tempo: &TempoMap,
    transport: TransportInfo,
) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut data: Vec<f32> = input.iter().flat_map(|c| c.iter().copied()).collect();
    {
        let mut block = Block::new(&mut data, CHANNELS, frames, frames, events, transport, RATE)
            .with_tempo(tempo);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// A noise signal, the same every run.
pub fn noise(frames: usize) -> Vec<f32> {
    let mut state = 0x1234_5678u32;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) * 1.6 - 0.8
        })
        .collect()
}

/// The rows a modulator wrote over a run (WP-5.10): one `Vec` of values per
/// row, for `frames` frames.
pub struct Rows(pub Vec<Vec<f32>>);

impl Rows {
    /// Slot `slot`'s contribution row.
    pub fn contribution(&self, slot: usize) -> &[f32] {
        &self.0[slot * 2]
    }

    /// Slot `slot`'s hold row.
    pub fn hold(&self, slot: usize) -> &[f32] {
        &self.0[slot * 2 + 1]
    }
}

/// Runs a modulator's [`Node::control`] over `frames` frames in blocks of
/// `block`, each a real-time section, with `events` (offsets from the run's
/// start) landing in the block they fall in, at `tempo` and under
/// `transport` (whose position moves on with the run while it plays).
pub fn control(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    events: &[NodeEvent],
    tempo: &TempoMap,
    transport: TransportInfo,
) -> Rows {
    use bl_engine::modbus::{Bus, Control, ROWS};

    let mut rows: Vec<Vec<f32>> = (0..ROWS).map(|_| Vec::with_capacity(frames)).collect();
    let mut done = 0;
    while done < frames {
        let length = block.min(frames - done);
        let here: Vec<NodeEvent> = events
            .iter()
            .filter(|e| (e.offset as usize) >= done && (e.offset as usize) < done + length)
            .map(|e| NodeEvent {
                offset: e.offset - done as u32,
                ..*e
            })
            .collect();
        let mut out = vec![0.0f32; ROWS * length];
        {
            let mut ctl = Control {
                frames: length,
                events: &here,
                transport: TransportInfo {
                    playing: transport.playing,
                    position: Frames(
                        transport.position.get() + if transport.playing { done as i64 } else { 0 },
                    ),
                },
                sample_rate: RATE,
                tempo,
                earlier: Bus::EMPTY,
                out: &mut out,
                stride: length,
            };
            let node = &mut *node;
            bl_rt::rt_test!({
                assert!(node.control(&mut ctl), "the node is a modulator");
            });
        }
        for (row, values) in rows.iter_mut().zip(out.chunks(length)) {
            row.extend_from_slice(values);
        }
        done += length;
    }
    Rows(rows)
}

/// A registry holding Beatlayer's native devices.
pub fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

/// A note-on of `key` on frame `offset`, with `id`.
pub fn note_on(offset: u32, key: u8, velocity: f32, id: Option<u32>) -> bl_engine::NoteEvent {
    bl_engine::NoteEvent {
        offset,
        kind: bl_engine::NoteKind::On,
        channel: 0,
        key,
        velocity,
        note_id: id,
    }
}

/// The note-off that ends `on`, on frame `offset`.
pub fn note_off(offset: u32, on: &bl_engine::NoteEvent, velocity: f32) -> bl_engine::NoteEvent {
    bl_engine::NoteEvent {
        offset,
        kind: bl_engine::NoteKind::Off,
        velocity,
        ..*on
    }
}

/// Runs one block of `frames` frames through `node` with `notes` to play, as
/// a real-time section, and returns the notes it sends on.
pub fn notes_through(
    node: &mut dyn Node,
    notes: &[bl_engine::NoteEvent],
    frames: usize,
) -> Vec<bl_engine::NoteEvent> {
    let mut out = bl_engine::NoteBuffer::with_capacity(8192);
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut block = Block::new(
        &mut data,
        CHANNELS,
        frames,
        frames,
        &[],
        TransportInfo::default(),
        RATE,
    )
    .with_notes(notes, &mut out);
    bl_rt::rt_test!({ node.process(&mut block) });
    out.notes().to_vec()
}

/// What a signal modulator wrote over a run (WP-5.11): the rows it wrote for
/// its own frames, the rows of what it wrote 32 frames before, which
/// [`Node::control`] gives before each part, and the notes it sent on.
pub struct Signal {
    pub rows: Rows,
    pub late: Rows,
    pub notes: Vec<bl_engine::NoteEvent>,
    /// The audio, each channel.
    pub audio: Vec<Vec<f32>>,
}

/// Runs a modulator driven by the signal over `frames` frames in parts of
/// `block` (at most 32, where a late row is read): each part is `control`
/// then `process`, each a real-time section. `input` is each channel's
/// audio (silence past its end), `sidechain` what its detector hears if it
/// has one, `notes` what it plays (offsets from the run's start) and
/// `events` its parameter changes.
#[allow(clippy::too_many_arguments)]
pub fn signal(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    input: &[Vec<f32>],
    sidechain: Option<&[Vec<f32>]>,
    notes: &[bl_engine::NoteEvent],
    events: &[NodeEvent],
    tempo: &TempoMap,
    transport: impl Fn(usize) -> TransportInfo,
) -> Signal {
    use bl_engine::modbus::{Bus, Control, ROWS};

    let mut rows: Vec<Vec<f32>> = (0..ROWS).map(|_| Vec::with_capacity(frames)).collect();
    let mut late: Vec<Vec<f32>> = (0..ROWS).map(|_| Vec::with_capacity(frames)).collect();
    let mut sent = Vec::new();
    let mut audio: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(frames)).collect();
    let mut done = 0;
    while done < frames {
        let length = block.min(frames - done);
        let at = transport(done);
        let here: Vec<NodeEvent> = events
            .iter()
            .filter(|e| (e.offset as usize) >= done && (e.offset as usize) < done + length)
            .map(|e| NodeEvent {
                offset: e.offset - done as u32,
                ..*e
            })
            .collect();
        let played: Vec<bl_engine::NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= done && (n.offset as usize) < done + length)
            .map(|n| bl_engine::NoteEvent {
                offset: n.offset - done as u32,
                ..*n
            })
            .collect();
        // The part's late rows, before it runs.
        let mut out = vec![0.0f32; ROWS * length];
        {
            let mut ctl = Control {
                frames: length,
                events: &[],
                transport: at,
                sample_rate: RATE,
                tempo,
                earlier: Bus::EMPTY,
                out: &mut out,
                stride: length,
            };
            let node = &mut *node;
            bl_rt::rt_test!({
                assert!(node.control(&mut ctl), "the node writes late rows");
            });
        }
        for (row, values) in late.iter_mut().zip(out.chunks(length)) {
            row.extend_from_slice(values);
        }
        let slice = |source: &[Vec<f32>], c: usize| -> Vec<f32> {
            (done..done + length)
                .map(|i| {
                    source
                        .get(c)
                        .and_then(|ch| ch.get(i))
                        .copied()
                        .unwrap_or(0.0)
                })
                .collect()
        };
        let mut data: Vec<f32> = (0..CHANNELS).flat_map(|c| slice(input, c)).collect();
        let key: Option<Vec<f32>> =
            sidechain.map(|s| (0..CHANNELS).flat_map(|c| slice(s, c)).collect());
        let mut rows_out = vec![0.0f32; ROWS * length];
        let mut notes_out = bl_engine::NoteBuffer::with_capacity(8192);
        {
            let mut block = Block::new(&mut data, CHANNELS, length, length, &here, at, RATE)
                .with_tempo(tempo)
                .with_notes(&played, &mut notes_out)
                .with_rows(&mut rows_out);
            if let Some(key) = &key {
                block = block.with_sidechain(key);
            }
            let node = &mut *node;
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (row, values) in rows.iter_mut().zip(rows_out.chunks(length)) {
            row.extend_from_slice(values);
        }
        sent.extend(notes_out.notes().iter().map(|n| bl_engine::NoteEvent {
            offset: n.offset + done as u32,
            ..*n
        }));
        for (channel, values) in audio.iter_mut().zip(data.chunks(length)) {
            channel.extend_from_slice(values);
        }
        done += length;
    }
    Signal {
        rows: Rows(rows),
        late: Rows(late),
        notes: sent,
        audio,
    }
}
