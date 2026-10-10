//! A locate through the engine's transport during playback, heard through a
//! stream: the source sees the song position jump and crossfades, so the
//! engine's output doesn't click.
//!
//! The clip player is WP-1.4's; this test plays a stream with a stand-in
//! node, which reads the stream at the song position (a clip at the song's
//! start, unwarped) as the clip player will, with
//! [`Transition::Crossfade`]. Every block runs as a real-time section under
//! the allocation trap. The stream reads in blocking mode, as an offline
//! render's do, so the run doesn't depend on the disk thread's timing.

bl_rt::install_trap!();

mod common;

use std::f64::consts::PI;
use std::sync::{Arc, Mutex};

use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_engine::{
    Block, Collector, Compiler, EngineConfig, Node, NodeConfig, Registry, TransportCommand, channel,
};
use bl_media::{AudioSource, Decoder, PlanarMut, StreamConfig, StreamSource, Streamer, Transition};
use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};
use common::TempDir;
use common::wav::{self, Format};

const PLAYER: &str = "test.stream_player";
const RATE: SampleRate = SampleRate::HZ_48000;

/// A stand-in clip player: the stream at the song position.
struct StreamPlayer {
    source: StreamSource,
    transition: Transition,
    scratch: Vec<f32>,
    stride: usize,
}

impl Node for StreamPlayer {
    fn process(&mut self, block: &mut Block<'_>) {
        let transport = block.transport();
        if !transport.playing {
            self.source.stop();
            return;
        }
        let frames = block.frames();
        let position = u64::try_from(transport.position.get()).unwrap_or(0);
        let mut out = PlanarMut::new(&mut self.scratch, 2, frames, self.stride);
        self.source.read(position, &mut out, self.transition);
        for c in 0..block.channels() {
            let from = out.channel(c.min(1));
            for (sample, add) in block.channel_mut(c).iter_mut().zip(from) {
                *sample += add;
            }
        }
    }
}

/// Plays a 1 kHz sine through the engine in 64-frame blocks, locating a
/// quarter of a cycle away at block 300, and returns the left channel.
fn play(path: &std::path::Path, transition: Transition) -> Vec<f32> {
    let streamer = Streamer::start(StreamConfig::default()).unwrap();
    let mut source = streamer
        .open(Decoder::open(path).unwrap(), RATE.hz(), 0)
        .unwrap();
    source.set_blocking(true);
    let handover = Arc::new(Mutex::new(Some(source)));
    let mut registry = Registry::new();
    registry.add_native(PLAYER, move |_, config: &NodeConfig| {
        let source = handover.lock().unwrap().take().expect("one player");
        Box::new(StreamPlayer {
            source,
            transition,
            scratch: vec![0.0; 2 * config.max_frames],
            stride: config.max_frames,
        })
    });

    let config = EngineConfig::new(RATE, 64);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor) = channel(&config);
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: PLAYER.to_owned(),
        },
    );
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);
    assert!(sender.flush());

    // 300,060 frames is a whole number of ticks at 120 BPM, and lands a
    // quarter of a cycle away from where the playhead is at block 300
    // (19,200): from a zero crossing to a peak.
    let target = TempoMap::default().frames_to_ticks(Frames(300_060), RATE);
    let mut out = Vec::new();
    for block in 0..600 {
        if block == 300 {
            sender.transport(TransportCommand::Locate(target));
            assert!(sender.flush());
        }
        bl_rt::rt_test!({
            processor.process(64);
        });
        out.extend_from_slice(processor.output().channel(0));
        collector.collect();
    }
    assert_eq!(processor.position(), Frames(300_060 + 300 * 64));
    out
}

#[test]
fn a_locate_through_the_transport_doesnt_click() {
    // Spec: disk-streaming R4
    let dir = TempDir::new("stream-engine");
    let path = dir.path().join("sine.wav");
    wav::write(&path, 48_000, 2, 400_000, Format::Float32, |f, _| {
        (0.5 * (2.0 * PI * 1000.0 * f as f64 / 48_000.0).sin()) as f32
    });
    let natural = (2.0 * 0.5 * (PI * 1000.0 / 48_000.0).sin()) as f32;
    let step = |samples: &[f32]| {
        samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let crossfaded = step(&play(&path, Transition::Crossfade));
    let cut = step(&play(&path, Transition::Cut));
    eprintln!(
        "a locate through the transport: the largest step is {crossfaded:.5} crossfaded and \
         {cut:.5} cut, against the sine's own {natural:.5}"
    );
    assert!(crossfaded <= natural * 1.2, "crossfaded: {crossfaded}");
    assert!(cut >= natural * 5.0, "the measurement misses clicks: {cut}");
}
