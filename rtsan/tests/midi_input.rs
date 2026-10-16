//! MIDI input on the audio thread under RealtimeSanitizer (WP-2.2): the
//! engine takes messages off the lock-free queue, places them on samples
//! through its block clock, and hands them to MIDI tracks' inputs, while a
//! MIDI thread keeps sending through `bl-midi-io`'s sender, whose mutex
//! only the sending side takes. RTSan stops the process at the first
//! allocation, free, lock, or blocking call on the audio thread, and the
//! allocation trap fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource, PortId};
use bl_core::time::SampleRate;
use bl_engine::{Collector, Compiler, EngineConfig, NoInput, Registry, channel};
use bl_model::{Command, Editor, Input, Monitor, TrackKind};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn midi_input_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (mut sender, mut processor, _monitor) = channel(&config);
    let (input, engine_end) = bl_midi_io::input_queue(4096);
    processor.set_midi_input(engine_end);

    let mut editor = Editor::new();
    let mut tracks = Vec::new();
    for (i, (track_input, channel)) in [
        (Input::AllIns, None),
        (Input::ComputerKeyboard, None),
        (Input::None, None),
        (
            Input::Port {
                name: "Keys".to_owned(),
            },
            Some(1),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: i,
            })
            .unwrap();
        editor
            .apply(Command::SetInput {
                track,
                input: track_input,
                channel,
            })
            .unwrap();
        tracks.push(track);
    }
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());

    // A MIDI thread sends a note every 0.4 ms, through the mutex.
    let stop = Arc::new(AtomicBool::new(false));
    let midi = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let mut sent = 0u64;
            while !stop.load(Relaxed) {
                let key = (sent % 128) as u8;
                input.send(MidiInputEvent {
                    source: if sent.is_multiple_of(3) {
                        MidiSource::ComputerKeyboard
                    } else {
                        MidiSource::Port(PortId::from_name("Keys"))
                    },
                    time: Instant::now(),
                    message: MidiMessage::note_on(0, key, 100),
                });
                sent += 1;
                thread::sleep(Duration::from_micros(400));
            }
            sent
        })
    };

    // Blocks paced like a device's, 5.33 ms apart, each called at the time
    // it runs, as a callback is.
    let start = Instant::now();
    let period = Duration::from_nanos(256 * 1_000_000_000 / 48_000);
    for round in 0..600u32 {
        if round % 200 == 100 {
            // A monitoring edit during input: a program swap.
            let monitor = if round % 400 == 100 {
                Monitor::In
            } else {
                Monitor::Off
            };
            editor
                .apply(Command::SetMonitor {
                    track: tracks[0],
                    monitor,
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
            assert!(sender.flush());
        }
        if let Some(wait) = (start + period * round).checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        bl_rt::rt_test!({
            processor.process_at(256, &NoInput, Instant::now());
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }
    stop.store(true, Relaxed);
    let sent = midi.join().unwrap();
    let counters = processor.midi_counters();
    assert!(counters.received > 100, "{counters:?}");
    assert!(counters.delivered >= counters.received);
    eprintln!(
        "600 blocks of MIDI input under RealtimeSanitizer: {sent} sent, {} taken, {} \
         delivered to tracks, {} late, {} relocks",
        counters.received, counters.delivered, counters.late, counters.relocks
    );
}
