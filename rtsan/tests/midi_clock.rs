//! MIDI clock on the audio thread under RealtimeSanitizer (WP-7.11 and
//! WP-7.12, `docs/specs/midi-clock.md` R13 and R26): the engine writes Timing
//! Clock, Start, Continue, Stop and Song Position Pointer for two outputs
//! with Sync on, one of them with a negative sync delay that makes it write
//! ahead, while the transport starts, stops, jumps, loops and resyncs, the
//! tempo changes and a thread plays the output thread's part; and it follows
//! the clock a thread sends to an input with Sync on, with its Start,
//! Continue, Stop and pointers, while EXT switches on and off. RTSan stops
//! the process at the first allocation, free, lock, or blocking call on the
//! audio thread, and the allocation trap fails the test at any heap use it
//! sees.

bl_rt::install_trap!();

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use bl_core::midi::{
    ClockType, HardwareResync, MidiInputEvent, MidiMessage, MidiSource, PortId, SyncInput,
    SyncOutput,
};
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

#[test]
fn midi_clock_in_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(
        config,
        testing::registry(&Arc::new(Probe::default())),
        collector.handle(),
    );
    let (mut sender, mut processor, mut monitor) = channel(&config);
    let (input, engine_end) = bl_midi_io::input_queue(4096);
    processor.set_midi_input(engine_end);
    let port = PortId::from_name("Sequencer");
    sender.sync_inputs(&[SyncInput { port, delay_ms: 5 }]);
    sender.external_sync(true);
    sender.transport(TransportCommand::SetLoop(Some(LoopRegion {
        start: Ticks::ZERO,
        end: Ticks::WHOLE * 2,
    })));
    let editor = Editor::new();
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());

    // A thread plays the MIDI input thread's part: clocks at about 500 BPM
    // with a sleep's jitter, a Start, a Stop, a pointer and a Continue, and
    // 600 ms of silence, round and round.
    let stop = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicU64::new(0));
    let writer = {
        let (stop, sent) = (Arc::clone(&stop), Arc::clone(&sent));
        thread::spawn(move || {
            let message = |message| {
                input.send(MidiInputEvent {
                    source: MidiSource::Sync(port),
                    time: Instant::now(),
                    message,
                });
                sent.fetch_add(1, Relaxed);
            };
            let mut clock = 0u32;
            while !stop.load(Relaxed) {
                match clock % 600 {
                    0 => message(MidiMessage::START),
                    300 => message(MidiMessage::STOP),
                    320 => message(MidiMessage::song_position(40)),
                    321 => message(MidiMessage::CONTINUE),
                    // A silence of 600 ms.
                    500..=535 => {
                        clock += 1;
                        thread::sleep(Duration::from_millis(17));
                        continue;
                    }
                    _ => {}
                }
                message(MidiMessage::CLOCK);
                clock += 1;
                thread::sleep(Duration::from_micros(5_000));
            }
        })
    };
    for round in 0..2000u32 {
        match round {
            // EXT off and on again, and a Play of its own, while it plays.
            800 => sender.external_sync(false),
            900 => sender.external_sync(true),
            1200 => sender.transport(TransportCommand::Play),
            _ => {}
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process_at(256, &NoInput, Instant::now());
        });
        if round % 50 == 0 {
            collector.collect();
        }
        thread::sleep(Duration::from_micros(2_000));
    }
    stop.store(true, Relaxed);
    writer.join().unwrap();
    let state = monitor.state().sync_in;
    assert!(state.clocks > 300, "{state:?}");
    eprintln!(
        "2000 blocks of MIDI clock in under RealtimeSanitizer: {} clocks, {} glitches, {} dropped, \
         {} messages sent",
        state.clocks,
        state.glitches,
        state.dropped,
        sent.load(Relaxed)
    );
}
