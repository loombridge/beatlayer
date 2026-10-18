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

/// Runs per-channel `input` through `node` in blocks of `block` frames,
/// each a real-time section, and returns each channel's whole output.
pub fn stream(node: &mut dyn Node, input: &[Vec<f32>], block: usize) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut out = vec![Vec::with_capacity(frames); input.len()];
    for start in (0..frames).step_by(block) {
        let end = (start + block).min(frames);
        let mut data: Vec<f32> = input.iter().flat_map(|c| c[start..end].to_vec()).collect();
        {
            let mut block = Block::new(
                &mut data,
                input.len(),
                end - start,
                end - start,
                &[],
                TransportInfo::default(),
                RATE,
            );
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (channel, values) in out.iter_mut().zip(data.chunks(end - start)) {
            channel.extend_from_slice(values);
        }
    }
    out
}

/// A sine of `hz` and `amplitude`, `frames` long, starting at phase 0.
pub fn sine(hz: f64, amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| amplitude * (std::f64::consts::TAU * hz * i as f64 / RATE.as_f64()).sin() as f32)
        .collect()
}

/// `signal`'s amplitude at each frequency from 0 up to Nyquist, in steps of
/// `RATE / signal.len()` hertz (1 Hz for a second of audio), through a Hann
/// window: a sine of amplitude A on a bin reads A there.
pub fn spectrum(signal: &[f32]) -> Vec<f64> {
    use realfft::RealFftPlanner;

    let n = signal.len();
    let weights: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let mut input: Vec<f64> = signal
        .iter()
        .zip(&weights)
        .map(|(s, w)| f64::from(*s) * w)
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut output = fft.make_output_vec();
    fft.process(&mut input, &mut output)
        .expect("the lengths match");
    let sum: f64 = weights.iter().sum();
    output.iter().map(|c| c.norm() * 2.0 / sum).collect()
}

/// The complex amplitude of `signal` at `hz` (a whole number of cycles in
/// it, for an exact reading): its real and imaginary parts.
pub fn dft_at(signal: &[f32], hz: f64) -> (f64, f64) {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, s) in signal.iter().enumerate() {
        let angle = std::f64::consts::TAU * hz * i as f64 / RATE.as_f64();
        re += f64::from(*s) * angle.cos();
        im -= f64::from(*s) * angle.sin();
    }
    let n = signal.len() as f64;
    (2.0 * re / n, 2.0 * im / n)
}

/// A signal's level in decibels relative to 1.
pub fn db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-12).log10()
}
