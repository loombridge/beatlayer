//! The audio callback the shell runs on the audio thread
//! ([`EngineCallback`]) follows the real-time rules (the architecture,
//! "Real-time rules"): no allocation, no lock, no blocking call. Checked two
//! ways, as the architecture asks: block by block under `rt_test!`, which
//! fails on any heap use, and through the null device's stream, whose
//! statistics count heap operations inside every audio block the device
//! thread runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_audio_io::{AudioBuffer, AudioBufferMut, AudioCallback as _, ProcessContext};
use bl_shell::Shell;
use bl_shell::testing;
use bl_shell::audio::{EngineCallback, EngineShared, ToneSettings};
use serde_json::json;

bl_rt::install_trap!();

fn context(frames: usize) -> ProcessContext {
    ProcessContext {
        sample_rate: 48_000,
        frames,
        position: 0,
        timestamp: None,
    }
}

bl_rt::rt_test! {
    #[should_panic(expected = "in a real-time section")]
    fn the_trap_catches_an_allocating_block() {
        // The control: the harness below would notice an allocation.
        std::hint::black_box(vec![0.0f32; 512]);
    }
}

#[test]
fn the_callback_stays_off_the_heap_with_the_tone_off_on_and_changing() {
    let shared = Arc::new(EngineShared::default());
    let mut engine = EngineCallback::new(Arc::clone(&shared));
    let mut data = vec![0.0f32; 2 * 512];
    let input = AudioBuffer::empty(512);
    let ctx = context(512);
    // Set-up (above) may allocate; the blocks may not.
    bl_rt::rt_test!({
        for block in 0..256u32 {
            // The UI thread's writes, interleaved with the audio thread's
            // reads: switching the tone and moving its level and frequency.
            shared.set_tone_on(block % 3 != 0);
            shared.set_tone(ToneSettings {
                volume_db: -60.0 + f64::from(block % 25),
                frequency_hz: 110.0 * f64::from(1 + block % 8),
            });
            let mut output = AudioBufferMut::new(&mut data, 2, 512, 512);
            engine.process(&ctx, &input, &mut output);
        }
    });
    let peak = data.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    assert!(peak > 0.0, "the tone played in the last block");
}

// Spec: browser R12. The callback with the preview voice: starting,
// playing, replacing, and stopping previews, block by block under the trap.
#[test]
fn the_callback_plays_previews_without_touching_the_heap() {
    use bl_engine::preview::{PreviewBuffer, PreviewShared, preview_channel};

    let collector = bl_engine::Collector::new();
    let (mut control, voice) =
        preview_channel(collector.handle(), Arc::new(PreviewShared::default()));
    let shared = Arc::new(EngineShared::default());
    let mut engine = EngineCallback::new(Arc::clone(&shared)).with_preview(voice);
    let mut data = vec![0.0f32; 2 * 256];
    let input = AudioBuffer::empty(256);
    let ctx = context(256);
    let buffers: Vec<_> = (0..8)
        .map(|i| PreviewBuffer::new(vec![vec![0.001 * (i + 1) as f32; 2_000]; 2]).unwrap())
        .collect();
    let mut peak = 0.0f32;
    for (i, buffer) in buffers.into_iter().enumerate() {
        // Wrapping the audio for the collector allocates, on this thread.
        control.play(Box::new(buffer)).unwrap();
        bl_rt::rt_test!({
            for block in 0..12 {
                if i % 3 == 2 && block == 4 {
                    control.stop();
                }
                data.fill(0.0);
                let mut output = AudioBufferMut::new(&mut data, 2, 256, 256);
                engine.process(&ctx, &input, &mut output);
                peak = data.iter().fold(peak, |peak, s| peak.max(s.abs()));
            }
        });
    }
    assert!(peak > 0.0, "the previews played");
    assert!(control.status().playing.is_none(), "each ended or stopped");
}

// Spec: browser R12. A preview through the null device's stream, whose
// statistics count heap operations inside every audio block.
#[test]
fn a_preview_through_the_null_stream_stays_off_the_heap() {
    let dir = std::env::temp_dir().join(format!("bl-shell-rt-preview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("tone.wav");
    let frames = 24_000u32;
    let mut bytes = Vec::new();
    for chunk in [
        &b"RIFF"[..],
        &(36 + frames * 2).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &48_000u32.to_le_bytes(),
        &96_000u32.to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &(frames * 2).to_le_bytes(),
    ] {
        bytes.extend_from_slice(chunk);
    }
    for i in 0..frames {
        bytes.extend_from_slice(&((i % 64) as i16 * 4).to_le_bytes());
    }
    std::fs::write(&file, bytes).unwrap();

    let (mut shell, control) = testing::rig();
    shell.start_audio();
    // The null device makes no sound; the volume is low anyway.
    shell
        .run("browser.set_preview", json!({ "volume": 0.001 }))
        .unwrap();
    for _ in 0..3 {
        shell
            .run("browser.preview", json!({ "path": file.to_string_lossy() }))
            .unwrap();
        // The preview's buffer is loaded and pushed on another thread, and
        // the callback renders it: poll and drive the blocks until it plays.
        for _ in 0..2_000 {
            shell.poll();
            if shell.browser_preview().position_seconds > 0.0 {
                break;
            }
            testing::advance(&control, &mut shell, testing::STEP_MS);
        }
        assert!(
            shell.browser_preview().position_seconds > 0.0,
            "the preview never played"
        );
    }
    shell.run("browser.stop_preview", json!({})).unwrap();
    // The null device is paced by hand: run the blocks the statistics need.
    control.run_blocks(40);
    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
    shell.audio_mut().stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_callback_plays_the_engine_off_the_heap_through_the_transport() {
    // Spec: control-bar-transport R13. The engine's processor arrives
    // through the callback's ring, then plays a beat tone (at -60 dBFS;
    // nothing reaches a device here) while the transport plays, stops,
    // jumps, loops, and changes tempo between blocks.
    use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
    use bl_engine::nodes::{BEAT_TONE, beat_tone};
    use bl_engine::{Collector, Compiler, EngineConfig, LoopRegion, Registry, TransportCommand};
    use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};

    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (mut sender, processor, mut monitor) = bl_engine::channel(&config);
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
    let mut data = vec![0.0f32; 2 * 512];
    let input = AudioBuffer::empty(512);
    let ctx = context(512);
    let commands = [
        TransportCommand::Play,
        TransportCommand::Locate(Ticks::WHOLE * 3),
        TransportCommand::SetInsertMarker(Ticks::WHOLE),
        TransportCommand::SetLoop(Some(LoopRegion {
            start: Ticks::ZERO,
            end: Ticks::QUARTER,
        })),
        TransportCommand::Stop,
        TransportCommand::Continue,
        TransportCommand::PlayStop,
        TransportCommand::PlayStop,
        TransportCommand::InsertMarkerToPlayhead,
    ];
    let mut peak = 0.0f32;
    for (round, command) in commands.into_iter().cycle().take(36).enumerate() {
        // Off the audio thread: the UI's commands and an edit's program.
        sender.transport(command);
        if round % 9 == 4 {
            let bpm = Bpm::new(90.0 + round as f64).unwrap();
            editor
                .apply(Command::SetTempo {
                    tempo: TempoMap::constant(bpm),
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        // On it: the blocks, which may not touch the heap.
        bl_rt::rt_test!({
            for _ in 0..8 {
                let mut output = AudioBufferMut::new(&mut data, 2, 512, 512);
                callback.process(&ctx, &input, &mut output);
            }
        });
        peak = data.iter().fold(peak, |p, s| p.max(s.abs()));
        collector.collect();
    }
    let state = monitor.state();
    assert!(state.program.is_some(), "the engine played");
    assert!(state.swaps >= 4, "programs swapped in during playback");
    assert!(
        peak > 0.0 && peak <= 0.001 + 1e-6,
        "the beat tone at -60 dBFS: {peak}"
    );
}

/// Waits until the stream has run at least `blocks` blocks.
fn wait_for_blocks(shell: &Shell, blocks: u64) -> bl_audio_io::StreamStats {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let stats = shell.audio().stats().expect("a stream is open");
        if stats.blocks >= blocks || Instant::now() > deadline {
            return stats;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_null_stream_runs_the_callback_without_touching_the_heap() {
    assert!(bl_rt::trap::is_installed());
    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().is_running(), "{:?}", shell.audio().status());
    shell
        .run("app.settings", json!({ "page": "audio" }))
        .unwrap();
    // The null device makes no sound; the level is the quietest anyway.
    shell
        .run(
            "audio.set_test_tone",
            json!({ "on": true, "volume_db": -60.0 }),
        )
        .unwrap();
    let stats = wait_for_blocks(&shell, 40);
    assert!(stats.blocks >= 40, "{stats:?}");
    let thread = stats.audio_thread.as_ref().expect("a set-up report");
    assert!(thread.trap_installed, "the trap watched the audio thread");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");

    // A buffer change reopens the stream, and the new one is as clean.
    shell
        .run("audio.configure", json!({ "buffer_size": 64 }))
        .unwrap();
    shell
        .run("audio.set_test_tone", json!({ "frequency_hz": 1000.0 }))
        .unwrap();
    let stats = wait_for_blocks(&shell, 40);
    assert!(stats.blocks >= 40, "{stats:?}");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
    shell
        .run("audio.set_test_tone", json!({ "on": false }))
        .unwrap();

    // The engine on the stream, through the transport and edits while it
    // plays (WP-1.18): still no heap use in any block.
    shell.run("transport.play", json!({})).unwrap();
    let before = stats.blocks;
    let tempo = bl_core::time::TempoMap::constant(bl_core::time::Bpm::new(97.0).unwrap());
    for (id, args) in [
        (
            "transport.set_loop",
            json!({ "start": 0, "length": 705_600 }),
        ),
        ("transport.set_loop_enabled", json!({ "enabled": true })),
        ("song.set_tempo", json!({ "tempo": tempo })),
        ("transport.locate", json!({ "position": 2_822_400 })),
        ("track.create", json!({ "kind": "midi" })),
        ("transport.stop", json!({})),
        ("transport.continue", json!({})),
    ] {
        shell
            .invoke(id, args)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let stats = wait_for_blocks(&shell, shell.audio().stats().unwrap().blocks + 4);
        assert_eq!(stats.heap_operations, 0, "{id}: {stats:?}");
    }
    let stats = wait_for_blocks(&shell, before + 60);
    assert!(stats.blocks >= before + 60, "{stats:?}");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
    assert!(shell.poll_engine().playing);
    shell.audio_mut().stop();
}
