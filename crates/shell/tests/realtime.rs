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
    shell.audio_mut().stop();
}
