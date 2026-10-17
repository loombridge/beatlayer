//! What the engine's integration tests share: a song built through the
//! model's commands, and a rig that compiles, sends, and processes on one
//! thread, with every block a real-time section under the allocation trap.

#![allow(dead_code)] // Each test file uses part of this.

use std::sync::Arc;

use bl_core::time::SampleRate;
use bl_engine::testing::{self, Probe};
use bl_engine::{
    Collector, Compiler, EngineConfig, Monitor, NodeKey, Processor, Registry, Sender, Update,
    channel,
};
use bl_model::{
    Chain, ChainId, Command, Device, DeviceId, DeviceKind, DeviceRole, Editor, ParamId, RackKind,
    TrackId, TrackKind,
};

pub const RATE: SampleRate = SampleRate::HZ_48000;

/// Adds a track at the top level, or at the front of a group.
pub fn track(editor: &mut Editor, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind,
            name: String::new(),
            parent,
            index: 0,
        })
        .unwrap();
    id
}

/// Adds a native device at the end of a track's chain.
pub fn device(
    editor: &mut Editor,
    track: TrackId,
    device_type: &str,
    params: &[(ParamId, f64)],
) -> DeviceId {
    let id = editor.new_id();
    let mut device = Device::new(
        id,
        "",
        DeviceKind::Native {
            device_type: device_type.to_owned(),
        },
    );
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

/// Adds a rack of `kind` with `chains` empty chains at the end of a track's
/// chain (WP-5.1).
pub fn rack(
    editor: &mut Editor,
    track: TrackId,
    kind: RackKind,
    chains: usize,
) -> (DeviceId, Vec<ChainId>) {
    let mut rack = Device::rack(editor.new_id(), kind);
    for n in 0..chains {
        rack = rack.with_chain(Chain::new(editor.new_id(), format!("Chain {n}")));
    }
    let ids = (rack.id, rack.chains.iter().map(|c| c.id).collect());
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device: rack,
        })
        .unwrap();
    ids
}

/// Adds a native device with `role` at the end of a rack's chain.
pub fn in_chain(
    editor: &mut Editor,
    chain: ChainId,
    device_type: &str,
    role: DeviceRole,
    params: &[(ParamId, f64)],
) -> DeviceId {
    let id = editor.new_id();
    let mut device = Device::new(
        id,
        "",
        DeviceKind::Native {
            device_type: device_type.to_owned(),
        },
    )
    .with_role(role);
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    let index = editor.document().chain(chain).unwrap().devices.len();
    editor
        .apply(Command::InsertChainDevice {
            chain,
            index,
            device,
        })
        .unwrap();
    id
}

/// Adds an audio effect at the end of a rack's chain.
pub fn effect_in(
    editor: &mut Editor,
    chain: ChainId,
    device_type: &str,
    params: &[(ParamId, f64)],
) -> DeviceId {
    in_chain(editor, chain, device_type, DeviceRole::AudioEffect, params)
}

/// Adds a rack of `kind` with `chains` empty chains at the end of a rack's
/// chain.
pub fn rack_in(
    editor: &mut Editor,
    chain: ChainId,
    kind: RackKind,
    chains: usize,
) -> (DeviceId, Vec<ChainId>) {
    let mut rack = Device::rack(editor.new_id(), kind);
    for n in 0..chains {
        rack = rack.with_chain(Chain::new(editor.new_id(), format!("Chain {n}")));
    }
    let ids = (rack.id, rack.chains.iter().map(|c| c.id).collect());
    let index = editor.document().chain(chain).unwrap().devices.len();
    editor
        .apply(Command::InsertChainDevice {
            chain,
            index,
            device: rack,
        })
        .unwrap();
    ids
}

/// A DC device with `value`.
pub fn dc(editor: &mut Editor, track: TrackId, value: f64) -> DeviceId {
    device(editor, track, testing::DC, &[(testing::VALUE, value)])
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main and the nightly run. Unset or `quick`, the default for local runs
/// and pull requests, wall-clock results aren't judged.
pub fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Err(_) | Ok("" | "quick") => false,
        Ok(other) => panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full"),
    }
}

/// An engine's three ends and a compiler, all on the test's thread.
pub struct Rig {
    pub editor: Editor,
    pub probe: Arc<Probe>,
    pub collector: Collector,
    pub compiler: Compiler,
    pub sender: Sender,
    pub processor: Processor,
    pub monitor: Monitor,
}

impl Rig {
    /// A rig with blocks of up to `max_frames` frames at 48 kHz.
    pub fn new(max_frames: usize) -> Self {
        Self::with_registry(max_frames, |_| {})
    }

    /// A rig whose engine has `input_channels` audio-device input channels,
    /// for tracks whose Audio From is Ext. In (WP-2.13).
    pub fn with_input(max_frames: usize, input_channels: usize) -> Self {
        Self::configured(max_frames, input_channels, 0, |_| {})
    }

    /// A rig whose engine has `output_channels` audio-device output
    /// channels, for tracks whose Audio To is Ext. Out (WP-2.13).
    pub fn with_output(max_frames: usize, output_channels: usize) -> Self {
        Self::configured(max_frames, 0, output_channels, |_| {})
    }

    /// A rig with `input_channels` audio-device input channels and whatever
    /// `extend` adds to the registry.
    pub fn with_input_and_registry(
        max_frames: usize,
        input_channels: usize,
        extend: impl FnOnce(&mut Registry),
    ) -> Self {
        Self::configured(max_frames, input_channels, 0, extend)
    }

    /// A rig with blocks of up to `max_frames` frames at 48 kHz.
    pub fn with_registry(max_frames: usize, extend: impl FnOnce(&mut Registry)) -> Self {
        Self::configured(max_frames, 0, 0, extend)
    }

    /// A rig whose registry has the test nodes and whatever `extend` adds.
    fn configured(
        max_frames: usize,
        input_channels: usize,
        output_channels: usize,
        extend: impl FnOnce(&mut Registry),
    ) -> Self {
        let config = EngineConfig {
            input_channels,
            output_channels,
            ..EngineConfig::new(RATE, max_frames)
        };
        let probe = Arc::new(Probe::default());
        let collector = Collector::new();
        let mut registry = testing::registry(&probe);
        extend(&mut registry);
        let compiler = Compiler::new(config, registry, collector.handle());
        let (sender, processor, monitor) = channel(&config);
        Self {
            editor: Editor::new(),
            probe,
            collector,
            compiler,
            sender,
            processor,
            monitor,
        }
    }

    /// Syncs the document, sends the update, and says what it held.
    pub fn sync(&mut self) -> Synced {
        let update: Update = self.compiler.sync(self.editor.document());
        let synced = Synced {
            program: update.program.is_some(),
            params: update.params.len(),
            data: update.data.len(),
        };
        self.sender.send_update(update);
        assert!(self.sender.flush(), "the queue is full");
        synced
    }

    /// Applies a command and syncs.
    pub fn apply(&mut self, command: Command) -> Synced {
        self.editor.apply(command).unwrap();
        self.sync()
    }

    /// Processes one block as a real-time section, and fails the test if it
    /// touches the heap.
    pub fn block(&mut self, frames: usize) {
        let processor = &mut self.processor;
        bl_rt::rt_test!({
            processor.process(frames);
        });
    }

    /// Processes one block with the audio device's input, as a real-time
    /// section under the allocation trap, and fails the test if it touches
    /// the heap.
    pub fn block_with_input(&mut self, frames: usize, input: &dyn bl_engine::DeviceInput) {
        let processor = &mut self.processor;
        bl_rt::rt_test!({
            processor.process_with_input(frames, input);
        });
    }

    /// Processes `total` frames with a device input, in blocks of `frames`,
    /// each a real-time section, and returns the first output channel.
    pub fn render_with_input(
        &mut self,
        total: usize,
        frames: usize,
        input: &dyn bl_engine::DeviceInput,
    ) -> Vec<f32> {
        let mut out = Vec::with_capacity(total);
        while out.len() < total {
            let n = frames.min(total - out.len());
            self.block_with_input(n, input);
            out.extend_from_slice(self.processor.output().channel(0));
        }
        out
    }

    /// Processes `total` frames in blocks of `frames`, each a real-time
    /// section, and returns the first output channel.
    pub fn render(&mut self, total: usize, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(total);
        while out.len() < total {
            let n = frames.min(total - out.len());
            self.block(n);
            out.extend_from_slice(self.processor.output().channel(0));
        }
        out
    }

    /// The left channel of the last block.
    pub fn last(&self) -> Vec<f32> {
        self.processor.output().channel(0).to_vec()
    }
}

/// What a sync sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Synced {
    /// Whether it sent a new program.
    pub program: bool,
    /// How many parameter changes it sent.
    pub params: usize,
    /// How many nodes it sent data.
    pub data: usize,
}

/// The key of a device's node.
pub fn key(device: DeviceId) -> NodeKey {
    NodeKey::Device(device)
}
