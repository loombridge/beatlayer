//! WP-0.9's acceptance test: 1,000 recompiles during playback, with the
//! engine on a real-time audio thread (the null device, paced like a sound
//! card) whose every block runs under the allocation trap. No block may
//! touch the heap or drop out, and a counter node must keep its state
//! through every swap.
//!
//! `cargo test -p bl-engine --test stress -- --nocapture` prints the run's
//! figures.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, NullBackend, NullOptions, Pacing,
    PortRequest, ProcessContext, StreamRequest,
};
use bl_engine::testing::{self, Probe};
use bl_engine::{Engine, EngineConfig, Processor};
use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};
use common::{RATE, dc, device, track};

/// What every output sample must be: track A's DC.
const LEVEL: f32 = 0.25;

/// The engine as a stream's callback, checking every sample it plays.
struct Play {
    processor: Processor,
    wrong_samples: Arc<AtomicU64>,
}

impl AudioCallback for Play {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let mut done = 0;
        while done < ctx.frames {
            let n = (ctx.frames - done).min(self.processor.config().max_frames);
            let block = self.processor.process(n);
            let wrong = block.channel(0).iter().filter(|&&s| s != LEVEL).count();
            if wrong > 0 {
                self.wrong_samples.fetch_add(wrong as u64, Relaxed);
            }
            for c in 0..output.channels() {
                let from = block.channel(c.min(block.channels() - 1));
                output.channel_mut(c)[done..done + n].copy_from_slice(from);
            }
            done += n;
        }
    }
}

/// Waits until `done` holds, or fails the test after five seconds.
fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        thread::sleep(Duration::from_micros(200));
    }
}

#[test]
fn a_thousand_recompiles_during_playback_touch_no_heap_and_drop_no_audio() {
    let probe = Arc::new(Probe::default());
    let config = EngineConfig::new(RATE, 4096);
    let (mut engine, processor) = Engine::start(config, testing::registry(&probe)).unwrap();

    // Track A runs throughout: a counter and a DC. Track B takes the edits.
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Midi, None);
    let b = track(&mut editor, TrackKind::Audio, None);
    device(&mut editor, a, testing::COUNTER, &[]);
    dc(&mut editor, a, f64::from(LEVEL));
    engine.sync(editor.document());
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));

    let wrong_samples = Arc::new(AtomicU64::new(0));
    let backend = NullBackend::with_options(NullOptions {
        pacing: Pacing::RealTime,
        frame_limit: None,
    });
    let request = StreamRequest {
        output: Some(PortRequest::default()),
        input: None,
        sample_rate: Some(RATE.hz()),
        buffer_size: Some(128),
    };
    let callback = Play {
        processor,
        wrong_samples: Arc::clone(&wrong_samples),
    };
    let mut stream = backend.open_stream(&request, Box::new(callback)).unwrap();
    let began = Instant::now();
    stream.start().unwrap();
    wait("the first program", || engine.state().swaps == 1);

    let mut spare = None;
    for round in 0..1000u64 {
        // An edit to B's structure, so each one compiles.
        let command = match (round % 4, spare) {
            (0, _) => {
                let id = editor.new_id();
                spare = Some(id);
                Command::InsertDevice {
                    track: b,
                    index: 0,
                    device: Device::new(
                        id,
                        "",
                        DeviceKind::Native {
                            device_type: testing::DC.to_owned(),
                        },
                    ),
                }
            }
            (1, _) => Command::MoveTrack {
                track: b,
                parent: None,
                index: if round % 8 == 1 { 1 } else { 0 },
            },
            (2, Some(device)) => Command::SetDeviceEnabled {
                device,
                enabled: false,
            },
            (_, Some(device)) => Command::RemoveDevice { device },
            _ => unreachable!(),
        };
        editor.apply(command).unwrap();
        engine.sync(editor.document());
        // Every tenth round, a mixer edit too, which mustn't compile.
        if round % 10 == 0 {
            let volume = if round % 20 == 0 { 0.5 } else { 1.0 };
            editor
                .apply(Command::SetVolume { track: b, volume })
                .unwrap();
            engine.sync(editor.document());
        }
        // Each program reaches the audio thread before the next edit, so
        // none is skipped.
        wait("the swap", || engine.state().swaps == round + 2);
    }
    let played_for = began.elapsed();
    stream.stop().unwrap();
    let stats = stream.stats();
    let state = engine.state().clone();
    let counts = engine.counts();

    eprintln!(
        "{} recompiles in {:.2} s of playback: {} blocks of 128 frames, {} dropouts, \
         {} heap operations on the audio thread, longest callback {:.3} ms of 2.667 ms, \
         peak engine load {:.1}%; {} mixer edits needed no program; counter {} counted {} \
         of {} frames",
        state.swaps - 1,
        played_for.as_secs_f64(),
        stats.blocks,
        stats.dropouts,
        stats.heap_operations,
        stats.longest_block.as_secs_f64() * 1000.0,
        state.cpu_load_max * 100.0,
        counts.without_program,
        probe.counter.load(Relaxed),
        probe.counted.load(Relaxed),
        state.clock,
    );

    // The audio thread ran under the trap, which saw nothing.
    assert!(stats.audio_thread.unwrap().trap_installed);
    assert_eq!(stats.heap_operations, 0);
    assert_eq!(stats.dropouts, 0, "{stats:?}");
    assert_eq!(wrong_samples.load(Relaxed), 0, "a swap glitched the audio");

    // 1,000 programs swapped in, each taking over the last one's nodes.
    assert_eq!(state.swaps, 1001);
    assert_eq!(state.program, Some(1001));
    assert_eq!(state.counters.migration_mismatches, 0);
    // A mixer edit synced right behind a structural one may be folded into
    // that edit's program; on its own, it compiles nothing.
    assert_eq!(counts.syncs, 1101);
    assert_eq!(counts.programs, 1001);
    assert_eq!(counts.without_program + counts.coalesced, 100);

    // The counter is the one made first, and it counted every frame.
    assert_eq!(probe.counter.load(Relaxed), 1);
    assert_eq!(probe.counted.load(Relaxed), state.clock);

    // Everything the audio thread let go of was freed elsewhere.
    drop(stream);
    engine.collect();
    wait("the drop thread", || {
        engine.collect();
        probe.dropped.load(Relaxed) == probe.made.load(Relaxed)
    });
    assert_eq!(probe.made.load(Relaxed), 2 + 250);
}
