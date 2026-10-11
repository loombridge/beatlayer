//! Offline rendering: the engine without a device and without threads.

use bl_model::{ContentHash, DeviceId, Document};

use crate::EngineConfig;
use crate::compiler::Compiler;
use crate::engine::{Sender, channel};
use crate::gc::Collector;
use crate::message::Report;
use crate::processor::{Output, Processor};
use crate::registry::Registry;
use crate::state::{EngineState, Monitor};

/// The engine on one thread, for rendering faster than real time: exporting
/// a song, previewing a range, or a golden test.
///
/// It runs the same [`Processor`] a device's audio thread runs, block by
/// block, and does in between what the engine's other threads would: it
/// compiles documents and sends them as the compiler thread does, and frees
/// what each block let go of as the drop thread does. Each block runs as a
/// real-time section ([`bl_rt::realtime`]), as on a device: denormals are
/// flushed the same way, and where the allocation trap is installed, heap
/// use in a block is counted ([`Offline::heap_operations`]).
///
/// Nothing in it depends on timing, so the same document and messages give
/// the same output to the bit, run after run. Nor does the block size: a
/// parameter change at a frame or a scheduled command lands on its exact
/// frame whatever the blocks are. Only [`When::Now`](crate::When::Now)
/// changes, which take effect at the next block, depend on where blocks
/// start.
///
/// ```
/// use bl_core::time::SampleRate;
/// use bl_engine::{EngineConfig, Offline, Registry, TransportCommand};
/// use bl_model::Editor;
///
/// let mut offline = Offline::new(EngineConfig::new(SampleRate::HZ_48000, 512), Registry::new());
/// let editor = Editor::new();
/// offline.sync(editor.document());
/// offline.sender().transport(TransportCommand::Play);
/// let mut frames = 0;
/// offline.render(48_000, |block| frames += block.frames());
/// assert_eq!(frames, 48_000);
/// assert_eq!(offline.state().clock, 48_000);
/// ```
pub struct Offline {
    compiler: Compiler,
    sender: Sender,
    processor: Processor,
    monitor: Monitor,
    collector: Collector,
    reports: Vec<Report>,
    heap_operations: u64,
    missing: Vec<ContentHash>,
}

impl Offline {
    /// An engine with `config` that makes device nodes, and finds clips'
    /// audio, with `registry`.
    #[must_use]
    pub fn new(config: EngineConfig, registry: Registry) -> Self {
        let collector = Collector::new();
        let compiler = Compiler::new(config, registry, collector.handle());
        let (sender, processor, monitor) = channel(&config);
        Self {
            compiler,
            sender,
            processor,
            monitor,
            collector,
            reports: Vec::new(),
            heap_operations: 0,
            missing: Vec::new(),
        }
    }

    /// The engine's settings.
    #[must_use]
    pub fn config(&self) -> &EngineConfig {
        self.processor.config()
    }

    /// Brings the engine up to date with `document`, as
    /// [`Engine::sync`](crate::Engine::sync) does: a new program if its
    /// structure changed, parameter changes if only values did. They take
    /// effect at the start of the next block. Returns the devices no factory
    /// makes a node for, whose tracks' audio passes through where they'd be.
    /// Media that clips name and the registry's media library can't supply
    /// is kept for [`Offline::missing_media`].
    pub fn sync(&mut self, document: &Document) -> Vec<DeviceId> {
        let mut update = self.compiler.sync(document);
        let unhosted = std::mem::take(&mut update.unhosted);
        self.missing = std::mem::take(&mut update.missing);
        self.sender.send_update(update);
        unhosted
    }

    /// The media that clips sent to the engine by the last
    /// [`sync`](Self::sync) name and the registry's media library couldn't
    /// supply. Those clips play silence.
    #[must_use]
    pub fn missing_media(&self) -> &[ContentHash] {
        &self.missing
    }

    /// The queue to the processor, for transport commands, scheduled
    /// commands, parameter changes, and node data. What's sent reaches the
    /// processor at the start of the next block, in order.
    pub fn sender(&mut self) -> &mut Sender {
        &mut self.sender
    }

    /// Processes one block of `frames` frames (at most
    /// [`EngineConfig::max_frames`]) and returns its output.
    pub fn process(&mut self, frames: usize) -> Output<'_> {
        // The queue holds 4,096 messages; anything sent beyond that waited in
        // the sender, and goes now, ahead of the block.
        self.sender.flush();
        // First touches of the thread's real-time state can allocate (on
        // macOS, for thread-local storage), so they happen outside the
        // section.
        bl_rt::sanitize::ensure_initialized();
        bl_rt::trap::prepare_thread();
        let before = bl_rt::trap::thread_violations();
        let processor = &mut self.processor;
        bl_rt::realtime(|| {
            processor.process(frames);
        });
        self.heap_operations += bl_rt::trap::thread_violations() - before;
        // What the block let go of (an old program, replaced node data) is
        // freed here, between blocks, as the drop thread would.
        self.collector.collect();
        self.reports.extend(self.monitor.reports());
        self.processor.output()
    }

    /// Processes `frames` frames in blocks of [`EngineConfig::max_frames`]
    /// (the last one shorter if it doesn't divide), handing each block's
    /// output to `sink`.
    pub fn render(&mut self, frames: u64, mut sink: impl FnMut(Output<'_>)) {
        let block = self.config().max_frames as u64;
        let mut done = 0;
        while done < frames {
            let length = block.min(frames - done);
            sink(self.process(length as usize));
            done += length;
        }
    }

    /// The state after the last block: clock, playhead, meters, counters.
    pub fn state(&mut self) -> &EngineState {
        self.monitor.state()
    }

    /// Reports since the last call, oldest first: scheduled commands that
    /// fired or were rejected.
    pub fn reports(&mut self) -> Vec<Report> {
        std::mem::take(&mut self.reports)
    }

    /// Heap operations inside blocks so far. The real-time rules allow none.
    /// Always 0 unless the program installed the allocation trap
    /// ([`bl_rt::install_trap!`]), as test and debug builds do.
    #[must_use]
    pub fn heap_operations(&self) -> u64 {
        self.heap_operations
    }
}

impl std::fmt::Debug for Offline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Offline")
            .field("compiler", &self.compiler)
            .field("processor", &self.processor)
            .field("heap_operations", &self.heap_operations)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
    use bl_model::{Command, Device, DeviceKind, Editor, TrackId, TrackKind};

    use super::*;
    use crate::message::{Action, ScheduledCommand, TransportCommand};
    use crate::mixer;
    use crate::node::{NodeKey, ParamValue};
    use crate::nodes::{TEST_TONE, test_tone};

    /// A group holding a tone, a tone on its own, and, from beat 3, a ramp
    /// from 120 to 180 BPM over four beats.
    fn song() -> (Editor, TrackId, bl_model::DeviceId) {
        let mut editor = Editor::new();
        let track = |editor: &mut Editor, kind, parent| {
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
        };
        let tone = |editor: &mut Editor, track, hz: f64| {
            let id = editor.new_id();
            let mut device = Device::new(
                id,
                "",
                DeviceKind::Native {
                    device_type: TEST_TONE.to_owned(),
                },
            );
            device.params.insert(test_tone::FREQUENCY, hz);
            device.params.insert(test_tone::LEVEL, -30.0);
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device,
                })
                .unwrap();
            id
        };
        let group = track(&mut editor, TrackKind::Group, None);
        let inner = track(&mut editor, TrackKind::Midi, Some(group));
        tone(&mut editor, inner, 330.0);
        let outer = track(&mut editor, TrackKind::Midi, None);
        let device = tone(&mut editor, outer, 440.0);
        let tempo = TempoMap::from_breakpoints([
            (Ticks::ZERO, Bpm::new(120.0).unwrap()),
            (Ticks::QUARTER * 2, Bpm::new(120.0).unwrap()),
            (Ticks::QUARTER * 6, Bpm::new(180.0).unwrap()),
        ])
        .unwrap();
        editor.apply(Command::SetTempo { tempo }).unwrap();
        (editor, outer, device)
    }

    /// Renders the song for three seconds in blocks of `block` frames, with
    /// commands scheduled through the ramp, and returns both channels
    /// interleaved.
    fn render(block: usize) -> (Vec<f32>, Offline) {
        let (editor, track, device) = song();
        let config = EngineConfig::new(SampleRate::HZ_48000, block);
        let mut offline = Offline::new(config, Registry::new());
        assert!(offline.sync(editor.document()).is_empty());
        offline.sender().transport(TransportCommand::Play);
        for (i, eighth) in (0..12).enumerate() {
            let action = if i % 2 == 0 {
                Action::Param {
                    key: NodeKey::Device(device),
                    param: test_tone::FREQUENCY,
                    value: ParamValue::Set(440.0 + 55.0 * f64::from(eighth)),
                }
            } else {
                Action::Param {
                    key: NodeKey::Mixer(track),
                    param: mixer::track::VOLUME,
                    value: ParamValue::Set(1.0 / f64::from(eighth)),
                }
            };
            offline.sender().schedule(ScheduledCommand {
                id: i as u64,
                at: Ticks::QUARTER / 2 * i64::from(eighth),
                action,
            });
        }
        let mut out = Vec::new();
        offline.render(144_000, |output| {
            for frame in 0..output.frames() {
                out.extend((0..output.channels()).map(|c| output.channel(c)[frame]));
            }
        });
        (out, offline)
    }

    fn bits(samples: &[f32]) -> Vec<u32> {
        samples.iter().map(|s| s.to_bits()).collect()
    }

    #[test]
    fn a_render_is_the_same_to_the_bit_every_time() {
        let (first, mut offline) = render(512);
        assert_eq!(first.len(), 144_000 * 2);
        assert!(first.iter().any(|&s| s != 0.0));
        assert_eq!(offline.reports().len(), 12, "every command fired");
        assert_eq!(offline.heap_operations(), 0);
        let (second, _) = render(512);
        assert_eq!(bits(&first), bits(&second));
    }

    #[test]
    fn the_block_size_changes_nothing() {
        let (reference, _) = render(512);
        for block in [1, 61, 4096] {
            let (other, mut offline) = render(block);
            assert_eq!(bits(&reference), bits(&other), "blocks of {block}");
            assert_eq!(offline.reports().len(), 12);
            assert_eq!(offline.heap_operations(), 0);
        }
    }

    #[test]
    fn heap_use_inside_a_block_is_counted() {
        /// Allocates in every block, as no node may.
        struct Leaky;
        impl crate::Node for Leaky {
            fn process(&mut self, _: &mut crate::Block<'_>) {
                std::hint::black_box(vec![0u8; 64]);
            }
        }
        let mut registry = Registry::new();
        registry.add_native("test.leaky", |_, _| Box::new(Leaky));
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
        let device = Device::new(
            editor.new_id(),
            "",
            DeviceKind::Native {
                device_type: "test.leaky".to_owned(),
            },
        );
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        let mut offline = Offline::new(EngineConfig::new(SampleRate::HZ_48000, 64), registry);
        offline.sync(editor.document());
        offline.render(640, |_| {});
        // An allocation and a free in each of ten blocks.
        assert_eq!(offline.heap_operations(), 20);
    }

    #[test]
    fn edits_between_blocks_recompile_and_free_the_old_program() {
        let (mut editor, track, _) = song();
        let mut offline = Offline::new(
            EngineConfig::new(SampleRate::HZ_48000, 256),
            Registry::new(),
        );
        offline.sync(editor.document());
        offline.render(1024, |_| {});
        let held = offline.collector.held();
        for _ in 0..10 {
            let id = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind: TrackKind::Audio,
                    name: String::new(),
                    parent: None,
                    index: 0,
                })
                .unwrap();
            offline.sync(editor.document());
            offline.process(256);
        }
        assert_eq!(offline.state().program, Some(11));
        assert_eq!(offline.collector.held(), held, "old programs are freed");
        assert_eq!(offline.heap_operations(), 0);
        // A mixer edit needs no program, only a parameter change.
        editor
            .apply(Command::SetMute { track, mute: true })
            .unwrap();
        offline.sync(editor.document());
        let peak = offline
            .process(256)
            .channel(0)
            .iter()
            .fold(0.0f32, |p, s| p.max(s.abs()));
        assert!(peak > 0.0, "the group's tone still plays");
        assert_eq!(offline.state().program, Some(11));
    }
}
