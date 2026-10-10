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
use bl_model::{Command, Device, DeviceId, DeviceKind, Editor, ParamId, TrackId, TrackKind};

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

/// A DC device with `value`.
pub fn dc(editor: &mut Editor, track: TrackId, value: f64) -> DeviceId {
    device(editor, track, testing::DC, &[(testing::VALUE, value)])
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

    /// A rig whose registry has the test nodes and whatever `extend` adds.
    pub fn with_registry(max_frames: usize, extend: impl FnOnce(&mut Registry)) -> Self {
        let config = EngineConfig::new(RATE, max_frames);
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
}

/// The key of a device's node.
pub fn key(device: DeviceId) -> NodeKey {
    NodeKey::Device(device)
}
