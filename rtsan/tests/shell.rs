//! The app's audio callback under RealtimeSanitizer (WP-1.18): `bl-shell`'s
//! `EngineCallback` takes the engine's processor from its ring at its
//! first block, runs it in blocks of the engine's size, copies its output
//! to the device's buffer, and plays the test tone over it, while the
//! transport plays, stops, jumps, loops, and moves the insert marker, and
//! programs swap in during playback. RTSan stops the process at the first
//! allocation, free, lock, or blocking call in any block, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_audio_io::{AudioBuffer, AudioBufferMut, AudioCallback as _, ProcessContext};
use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::nodes::{BEAT_TONE, beat_tone};
use bl_engine::{
    Collector, Compiler, EngineConfig, LoopRegion, Registry, TransportCommand, channel,
};
use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};
use bl_shell::audio::{EngineCallback, EngineShared, ToneSettings};

// Without `--cfg bl_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "bl-rt wasn't built with --cfg bl_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn the_apps_audio_callback_passes_under_rtsan() {
    // A device buffer of 300 frames over an engine of 128: blocks split.
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (mut sender, processor, mut monitor) = channel(&config);

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
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: BEAT_TONE.to_owned(),
        },
    );
    device.params.insert(beat_tone::LEVEL, -60.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));

    let (mut handoff, incoming) = rtrb::RingBuffer::new(1);
    let shared = Arc::new(EngineShared::default());
    let mut callback = EngineCallback::with_engine(Arc::clone(&shared), incoming);
    assert!(handoff.push(processor).is_ok());
    let frames = 300;
    let mut data = vec![0.0f32; 2 * frames];
    let input = AudioBuffer::empty(frames);
    let ctx = ProcessContext {
        sample_rate: 48_000,
        frames,
        position: 0,
        timestamp: None,
    };

    let mut peak = 0.0f32;
    for round in 0..1_000u32 {
        let command = match round % 50 {
            0 => Some(TransportCommand::Play),
            7 => Some(TransportCommand::SetInsertMarker(Ticks::WHOLE * 2)),
            10 => Some(TransportCommand::Locate(Ticks::QUARTER * 3)),
            12 => Some(TransportCommand::SetLoop(Some(LoopRegion {
                start: Ticks::QUARTER * 3,
                end: Ticks::WHOLE,
            }))),
            20 => Some(TransportCommand::Stop),
            21 => Some(TransportCommand::Continue),
            30 => Some(TransportCommand::PlayStop),
            31 => Some(TransportCommand::PlayStop),
            40 => Some(TransportCommand::InsertMarkerToPlayhead),
            45 => Some(TransportCommand::SetLoop(None)),
            _ => None,
        };
        if let Some(command) = command {
            sender.transport(command);
        }
        if round % 200 == 100 {
            // A new tempo map during playback: a program swap.
            let bpm = Bpm::new(90.0 + f64::from(round % 400) / 10.0).unwrap();
            editor
                .apply(Command::SetTempo {
                    tempo: TempoMap::constant(bpm),
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        // The Settings window's test tone, at its quietest, now and then.
        shared.set_tone_on(round % 100 >= 90);
        shared.set_tone(ToneSettings {
            volume_db: -60.0,
            frequency_hz: 440.0,
        });
        assert!(sender.flush());

        bl_rt::rt_test!({
            let mut output = AudioBufferMut::new(&mut data, 2, frames, frames);
            callback.process(&ctx, &input, &mut output);
        });
        peak = data.iter().fold(peak, |p, s| p.max(s.abs()));
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert!(state.program.is_some(), "the engine played");
    assert!(state.swaps >= 5, "{} swaps", state.swaps);
    assert!(peak > 0.0 && peak <= 0.001 + 1e-6, "at -60 dBFS: {peak}");
    eprintln!(
        "1,000 device blocks of the app's callback under RealtimeSanitizer: {} programs, peak \
         engine load {:.1}%",
        state.swaps,
        state.cpu_load_max * 100.0
    );
}
