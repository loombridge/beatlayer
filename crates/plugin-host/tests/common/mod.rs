//! What the CLAP host's tests share: the test plug-in built from source
//! (`bl-test-clap`) as a CLAP bundle, and its nodes run block by block, each
//! block a real-time section under the allocation trap.

#![allow(dead_code, unused_imports, reason = "each test file uses some")]

use std::path::Path;

use bl_core::id::Id;
use bl_core::time::SampleRate;
use bl_engine::{
    Block, Node, NodeConfig, NodeEvent, NoteBuffer, NoteEvent, ParamValue, TransportInfo,
};
use bl_model::{Device, DeviceId, DeviceKind, ParamId, PluginFormat};

pub use bl_test_clap::{
    ASK_RESIZE, ASK_RESIZE_HEIGHT, ASK_RESIZE_WIDTH, EDITOR_HEIGHT, EDITOR_SCALE, EDITOR_WIDTH,
    GAIN, LAST_NOTE, LATENCY, PLUGIN_ID, PLUGIN_NAME,
};

pub const RATE: SampleRate = SampleRate::HZ_48000;
pub const CHANNELS: usize = 2;
pub const FRAMES: usize = 64;

pub fn config() -> NodeConfig {
    NodeConfig {
        sample_rate: RATE,
        max_frames: FRAMES,
        channels: CHANNELS,
    }
}

/// The test plug-in as a CLAP bundle.
pub fn bundle() -> &'static Path {
    bl_test_clap::bundle()
}

/// A Test Gain Echo device with `params` set.
pub fn device(id: u64, params: &[(u32, f64)]) -> Device {
    let mut device = Device::new(
        DeviceId(Id::new(id).expect("an ID")),
        PLUGIN_NAME,
        DeviceKind::Plugin {
            format: PluginFormat::Clap,
            plugin_id: PLUGIN_ID.to_owned(),
        },
    );
    for (param, value) in params {
        device.params.insert(ParamId(*param), *value);
    }
    device
}

pub fn set(offset: u32, param: u32, value: f64) -> NodeEvent {
    NodeEvent {
        offset,
        param: ParamId(param),
        value: ParamValue::Set(value),
    }
}

/// What a block put out: each channel, and the notes the node sent on.
pub struct Output {
    pub channels: Vec<Vec<f32>>,
    pub notes: Vec<NoteEvent>,
}

/// Runs one block of `input` on every channel through `node`, with
/// `events` and `notes`, as a real-time section.
pub fn run(
    node: &mut dyn Node,
    input: &[f32],
    events: &[NodeEvent],
    notes: &[NoteEvent],
) -> Output {
    let frames = input.len();
    let mut data: Vec<f32> = (0..CHANNELS).flat_map(|_| input.iter().copied()).collect();
    let mut out = NoteBuffer::with_capacity(64);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    Output {
        channels: data.chunks(frames).map(<[f32]>::to_vec).collect(),
        notes: out.notes().to_vec(),
    }
}

/// The VST3 test plug-in as a `.vst3` bundle (WP-2.7).
pub fn vst3_bundle() -> &'static Path {
    bl_test_vst3::bundle()
}

/// A VST3 Test Gain Echo device with `params` (plain values) set.
pub fn vst3_device(id: u64, params: &[(u32, f64)]) -> Device {
    let mut device = Device::new(
        DeviceId(Id::new(id).expect("an ID")),
        bl_test_vst3::PLUGIN_NAME,
        DeviceKind::Plugin {
            format: PluginFormat::Vst3,
            plugin_id: bl_test_vst3::plugin_id(),
        },
    );
    for (param, value) in params {
        device.params.insert(ParamId(*param), *value);
    }
    device
}
