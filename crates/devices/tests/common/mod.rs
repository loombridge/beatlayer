//! What the devices' tests share: a node of a device type, run on its own,
//! each block a real-time section under the allocation trap.

#![allow(dead_code, reason = "each test file uses some")]

use bl_core::id::Id;
use bl_core::time::{SampleRate, TempoMap};
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
