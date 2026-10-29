//! MIDI clock out on the audio thread under RealtimeSanitizer (WP-7.11,
//! `docs/specs/midi-clock.md` R13): the engine writes Timing Clock, Start,
//! Continue, Stop and Song Position Pointer for two outputs with Sync on,
//! one of them with a negative sync delay that makes it write ahead, while
//! the transport starts, stops, jumps, loops and resyncs, the tempo changes
//! and a thread plays the output thread's part. RTSan stops the process at
//! the first allocation, free, lock, or blocking call on the audio thread,
//! and the allocation trap fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use bl_core::midi::{ClockType, HardwareResync, PortId, SyncOutput};
use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{
    Collector, Compiler, EngineConfig, LoopRegion, NoInput, TransportCommand, channel,
};
use bl_model::{Command, Editor};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn midi_clock_out_passes_under_rtsan() {
    let mut config = EngineConfig::new(SampleRate::HZ_48000, 256);
    config.output_latency = 240;
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(
        config,
        testing::registry(&Arc::new(Probe::default())),
        collector.handle(),
    );
    let (mut sender, mut processor, mut monitor) = channel(&config);
    let (out, mut output) = rtrb::RingBuffer::new(8192);
    processor.set_midi_output(out);
    let outputs = [
        SyncOutput {
            port: PortId::from_name("Drum machine"),
            delay_ms: 0,
            clock_type: ClockType::Song,
            resync: HardwareResync::StopAndStart,
        },
        SyncOutput {
            port: PortId::from_name("Sequencer"),
            delay_ms: -50,
            clock_type: ClockType::Pattern,
            resync: HardwareResync::StartOnly,
        },
    ];
    sender.sync_outputs(&outputs);

    let mut editor = Editor::new();
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());
    sender.transport(TransportCommand::Play);

    // A thread plays the output thread's part.
    let stop = Arc::new(AtomicBool::new(false));
    let taken = Arc::new(AtomicU64::new(0));
    let reader = {
        let (stop, taken) = (Arc::clone(&stop), Arc::clone(&taken));
        thread::spawn(move || {
            while !stop.load(Relaxed) {
                while let Ok(event) = output.pop() {
                    assert!(outputs.iter().any(|o| o.port == event.port));
                    taken.fetch_add(1, Relaxed);
                }
                thread::sleep(Duration::from_millis(1));
            }
        })
    };

    for round in 0..1200u32 {
        match round % 200 {
            40 => sender.transport(TransportCommand::ResyncExternalHardware),
            80 => sender.transport(TransportCommand::Locate(Ticks::WHOLE * 3 + Ticks::QUARTER)),
            100 => sender.transport(TransportCommand::SetLoop(Some(LoopRegion {
                start: Ticks::ZERO,
                end: Ticks::WHOLE,
            }))),
            140 => {
                sender.transport(TransportCommand::Stop);
                sender.transport(TransportCommand::Locate(Ticks::QUARTER));
            }
            150 => sender.transport(TransportCommand::Play),
            170 => {
                // A new tempo map swaps in while the song plays.
                let bpm = 90.0 + f64::from(round % 400) / 4.0;
                editor
                    .apply(Command::SetTempo {
                        tempo: TempoMap::constant(Bpm::new(bpm).unwrap()),
                    })
                    .unwrap();
                sender.send_update(compiler.sync(editor.document()));
            }
            _ => {}
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process_at(256, &NoInput, Instant::now());
        });
        if round % 50 == 0 {
            collector.collect();
        }
        thread::sleep(Duration::from_micros(200));
    }
    stop.store(true, Relaxed);
    reader.join().unwrap();
    let state = monitor.state().sync_out;
    assert!(state.clocks > 500, "{state:?}");
    eprintln!(
        "1200 blocks of MIDI clock out under RealtimeSanitizer: {} clocks, {} late, {} dropped, \
         the reader took {}",
        state.clocks,
        state.late,
        state.dropped,
        taken.load(Relaxed)
    );
}
