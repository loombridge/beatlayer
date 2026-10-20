//! What the CLAP host's tests share: the test plug-in built from source
//! (`bl-test-clap`) as a CLAP bundle, and its nodes run block by block, each
//! block a real-time section under the allocation trap.

#![allow(dead_code, unused_imports, reason = "each test file uses some")]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bl_core::id::Id;
use bl_core::time::{SampleRate, Ticks};
use bl_engine::{
    Block, Node, NodeConfig, NodeData, NodeEvent, NoteBuffer, NoteEvent, ParamValue, TransportInfo,
};
use bl_model::{Device, DeviceId, DeviceKind, ParamId, PluginFormat};

pub use bl_test_clap::{
    ASK_RESIZE, ASK_RESIZE_HEIGHT, ASK_RESIZE_WIDTH, DEPTH, DEPTH_MOD, EDITOR_HEIGHT, EDITOR_SCALE,
    EDITOR_WIDTH, GAIN, LAST_NOTE, LATENCY, PLUGIN_ID, PLUGIN_NAME,
};

pub const RATE: SampleRate = SampleRate::HZ_48000;
pub const CHANNELS: usize = 2;
pub const FRAMES: usize = 64;

pub fn config() -> NodeConfig {
    NodeConfig {
        round_trip_latency: 0,
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
    run_at(node, input, events, notes, TransportInfo::default())
}

/// The same, with the song at `transport` as the block starts.
pub fn run_at(
    node: &mut dyn Node,
    input: &[f32],
    events: &[NodeEvent],
    notes: &[NoteEvent],
    transport: TransportInfo,
) -> Output {
    let frames = input.len();
    let mut data: Vec<f32> = (0..CHANNELS).flat_map(|_| input.iter().copied()).collect();
    let mut out = NoteBuffer::with_capacity(64);
    {
        let mut block = Block::new(&mut data, CHANNELS, frames, frames, events, transport, RATE)
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

/// The engine's node for a device, running block after block on its own
/// thread with an input of 1, keeping the last block's first sample.
pub struct Engine {
    stop: Arc<AtomicBool>,
    last: Arc<Mutex<Option<f32>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn start(mut node: Box<dyn Node>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let last = Arc::new(Mutex::new(None));
        let thread = {
            let (stop, last) = (Arc::clone(&stop), Arc::clone(&last));
            std::thread::spawn(move || {
                while !stop.load(Relaxed) {
                    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
                    *last.lock().unwrap() = Some(out.channels[0][FRAMES - 1]);
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
        };
        Self {
            stop,
            last,
            thread: Some(thread),
        }
    }

    /// Waits for the node's output to settle at `level`.
    pub fn settles_at(&self, level: f32) {
        let until = Instant::now() + Duration::from_secs(5);
        while *self.last.lock().unwrap() != Some(level) {
            assert!(
                Instant::now() < until,
                "the engine's node never put out {level}: {:?}",
                self.last.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the compiler sends a plug-in's node for a clip envelope on one of
/// its parameters (WP-5.9): a one-bar clip at the start of a song, on a
/// track holding `device`, carrying an envelope through `points` (ticks
/// into the clip, modulation share).
pub fn modulation_for(device: Device, param: u32, points: &[(Ticks, f64)]) -> NodeData {
    use bl_engine::{Collector, Compiler, EngineConfig, NodeKey, Registry};
    use bl_model::{
        AutomationTarget, Clip, ClipData, ClipId, Command, Editor, MidiData, TrackKind, Vector,
    };

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let id = editor.new_id();
    let mut device = device;
    device.id = id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData {
                    notes: Vector::new(),
                    start: Ticks::ZERO,
                    end: None,
                    looping: false,
                    loop_start: Ticks::ZERO,
                    loop_end: None,
                    signature: None,
                }),
            ),
        })
        .unwrap();
    let target = AutomationTarget::Device {
        device: id,
        param: ParamId(param),
    };
    for &(at, value) in points {
        editor
            .apply(Command::SetClipEnvelopeBreakpoint {
                kind: bl_model::EnvelopeKind::Modulation,
                clip,
                target,
                at,
                value,
            })
            .unwrap();
    }
    let collector = Collector::new();
    let mut compiler = Compiler::new(
        EngineConfig::new(RATE, FRAMES),
        Registry::empty(),
        collector.handle(),
    );
    compiler
        .sync(editor.document())
        .data
        .into_iter()
        .find(|(key, _)| matches!(key, NodeKey::Device(device) if *device == id))
        .expect("the device's node is sent its envelopes")
        .1
}

/// One bar at 120 BPM and 48 kHz, in frames: a clip's envelope from 0 to 1
/// over it climbs by 1 / `BAR` a frame.
pub const BAR: i64 = 96_000;
