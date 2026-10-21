//! MIDI out and the external instrument on the audio thread under
//! RealtimeSanitizer (WP-5.34): a MIDI track sends what reaches the end of
//! its chain to a port, an external instrument sends its notes and brings
//! back the interface's inputs, and a thread plays the output thread's part,
//! reading the queue. RTSan stops the process at the first allocation, free,
//! lock, or blocking call on the audio thread, and the allocation trap fails
//! the test at any heap use it sees. The routing changes mid-run, so notes
//! are ended on a swap, and the transport stops with notes held.

bl_rt::install_trap!();

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource, PortId};
use bl_core::time::SampleRate;
use bl_engine::testing::{self, Probe};
use bl_engine::{Collector, Compiler, DeviceInput, EngineConfig, TransportCommand, channel};
use bl_model::{
    Channel, Command, Device, DeviceKind, DeviceRole, Editor, External, HardwareLatency, Input,
    MidiTo, Monitor, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// An interface with four input channels of a steady level.
struct Interface(Vec<Vec<f32>>);

impl DeviceInput for Interface {
    fn channels(&self) -> usize {
        self.0.len()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.0[index]
    }
}

#[test]
fn midi_out_and_the_external_instrument_pass_under_rtsan() {
    let mut config = EngineConfig::new(SampleRate::HZ_48000, 256);
    config.input_channels = 4;
    config.output_latency = 480;
    config.round_trip_latency = 960;
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(
        config,
        testing::registry(&Arc::new(Probe::default())),
        collector.handle(),
    );
    let (mut sender, mut processor, _monitor) = channel(&config);
    let (input, engine_end) = bl_midi_io::input_queue(4096);
    processor.set_midi_input(engine_end);
    let (out, mut output) = rtrb::RingBuffer::new(4096);
    processor.set_midi_output(out);

    let mut editor = Editor::new();
    let track = |editor: &mut Editor, index: usize| {
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index,
            })
            .unwrap();
        editor
            .apply(Command::SetInput {
                track,
                input: Input::ComputerKeyboard,
                channel: None,
            })
            .unwrap();
        editor
            .apply(Command::SetMonitor {
                track,
                monitor: Monitor::In,
            })
            .unwrap();
        track
    };
    // One track sends to a port; another has the external instrument.
    let direct = track(&mut editor, 0);
    editor
        .apply(Command::SetMidiOutput {
            track: direct,
            to: Some(MidiTo::Port {
                name: "Synth".to_owned(),
                channel: 3,
            }),
        })
        .unwrap();
    let hardware = track(&mut editor, 1);
    let instrument = editor.new_id();
    let mut device = Device::new(
        instrument,
        "",
        DeviceKind::Native {
            device_type: testing::EXTERNAL.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument)
    .with_external();
    device.external = Some(External {
        port: Some("Rack".to_owned()),
        channel: 2,
        audio_from: Channel::Pair(3),
        audio_to: None,
        hardware_latency: HardwareLatency::Milliseconds { value: 3.0 },
    });
    editor
        .apply(Command::InsertDevice {
            track: hardware,
            index: 0,
            device,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());
    sender.transport(TransportCommand::Play);

    // A MIDI thread sends notes; another plays the output thread's part.
    let stop = Arc::new(AtomicBool::new(false));
    let midi = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let mut sent = 0u64;
            while !stop.load(Relaxed) {
                let key = (sent % 100) as u8 + 20;
                input.send(MidiInputEvent {
                    source: MidiSource::ComputerKeyboard,
                    time: Instant::now(),
                    message: if sent % 7 == 6 {
                        MidiMessage::from_bytes(&[0xE0, 0, 96]).unwrap()
                    } else if sent.is_multiple_of(2) {
                        MidiMessage::note_on(0, key, 100)
                    } else {
                        MidiMessage::note_off(0, key - 1)
                    },
                });
                sent += 1;
                thread::sleep(Duration::from_micros(400));
            }
            sent
        })
    };
    let taken = Arc::new(AtomicU64::new(0));
    let reader = {
        let (stop, taken) = (Arc::clone(&stop), Arc::clone(&taken));
        thread::spawn(move || {
            while !stop.load(Relaxed) {
                while let Ok(event) = output.pop() {
                    assert!(
                        [
                            PortId::from_name("Synth"),
                            PortId::from_name("Rack"),
                            PortId::from_name("Elsewhere")
                        ]
                        .contains(&event.port)
                    );
                    taken.fetch_add(1, Relaxed);
                }
                thread::sleep(Duration::from_millis(1));
            }
        })
    };

    // Blocks paced like a device's, with the interface's inputs.
    let levels = Interface((0..4).map(|c| vec![0.1 * (c + 1) as f32; 256]).collect());
    let start = Instant::now();
    let period = Duration::from_nanos(256 * 1_000_000_000 / 48_000);
    for round in 0..600u32 {
        if round % 150 == 75 {
            // A routing change during output: a program swap that ends the
            // notes held on the old port.
            let port = if round % 300 == 75 {
                "Elsewhere"
            } else {
                "Synth"
            };
            editor
                .apply(Command::SetMidiOutput {
                    track: direct,
                    to: Some(MidiTo::Port {
                        name: port.to_owned(),
                        channel: 3,
                    }),
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
            assert!(sender.flush());
        }
        if round % 200 == 100 {
            // Stopping with notes held ends them.
            sender.transport(TransportCommand::Stop);
            sender.transport(TransportCommand::Play);
            assert!(sender.flush());
        }
        if let Some(wait) = (start + period * round).checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        bl_rt::rt_test!({
            processor.process_at(256, &levels, Instant::now());
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }
    stop.store(true, Relaxed);
    let sent = midi.join().unwrap();
    reader.join().unwrap();
    let counters = processor.midi_counters();
    assert!(counters.sent > 100, "{counters:?}");
    eprintln!(
        "600 blocks of MIDI out under RealtimeSanitizer: {sent} played, {} sent to ports, {} \
         dropped, the reader took {}",
        counters.sent,
        counters.out_dropped,
        taken.load(Relaxed)
    );
}
