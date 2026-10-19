//! A chain of every MIDI effect on the audio thread under RealtimeSanitizer
//! (WP-5.30): a clip of many notes plays through Pitch, Velocity, Random, Scale,
//! Chord and Note Length into a test instrument while the devices are switched off
//! and on, moved and deleted, and their parameters change. RTSan stops the
//! process at the first allocation, free, lock, or blocking call on the audio
//! thread, and the allocation trap fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_core::time::{SampleRate, TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{Collector, Compiler, EngineConfig, NoInput, NoteKind, TransportCommand, channel};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceId, DeviceKind, Editor, Input, MidiData, Monitor, Note,
    ParamId, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn a_chain_of_midi_effects_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let probe = Arc::new(Probe::default());
    let mut collector = Collector::new();
    let mut registry = testing::registry(&probe);
    bl_devices::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor) = channel(&config);

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
    let insert = |editor: &mut Editor, index: usize, device_type: &str| -> DeviceId {
        let id = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device: Device::new(
                    id,
                    "",
                    DeviceKind::Native {
                        device_type: device_type.to_owned(),
                    },
                ),
            })
            .unwrap();
        id
    };
    let types = [
        bl_devices::LIFT,
        bl_devices::TOUCH,
        bl_devices::DICE,
        bl_devices::REMAP,
        bl_devices::STACK,
        bl_devices::HOLD,
        bl_devices::ARPEGGIATOR,
        bl_devices::NOTE_ECHO,
        bl_devices::CC_CONTROL,
        bl_devices::MPE_CONTROL,
        bl_devices::MIDI_MONITOR,
    ];
    let effects: Vec<DeviceId> = types
        .iter()
        .enumerate()
        .map(|(i, t)| insert(&mut editor, i, t))
        .collect();
    insert(&mut editor, types.len(), testing::INSTRUMENT);

    // A bar of sixteenth notes on many keys, looping.
    let sixteenth = Ticks(TICKS_PER_QUARTER / 4);
    let notes = (0..64u8).map(|i| Note::new(36 + i, sixteenth * i64::from(i), sixteenth * 3, 90));
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 16),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());
    sender.transport(TransportCommand::Play);

    let mut change = |editor: &mut Editor, command: Command| {
        editor.apply(command).unwrap();
        sender.send_update(compiler.sync(editor.document()));
        assert!(sender.flush());
    };
    for round in 0..800u32 {
        match round {
            100 => change(
                &mut editor,
                Command::SetDeviceParam {
                    device: effects[0],
                    param: ParamId(0),
                    value: 5.0,
                },
            ),
            200 => change(
                &mut editor,
                Command::SetDeviceEnabled {
                    device: effects[4],
                    enabled: false,
                },
            ),
            300 => change(
                &mut editor,
                Command::SetDeviceEnabled {
                    device: effects[4],
                    enabled: true,
                },
            ),
            400 => change(
                &mut editor,
                Command::MoveDevice {
                    device: effects[0],
                    track,
                    index: 3,
                },
            ),
            500 => change(&mut editor, Command::RemoveDevice { device: effects[5] }),
            600 => change(
                &mut editor,
                Command::SetDeviceParam {
                    device: effects[2],
                    param: ParamId(1),
                    value: 12.0,
                },
            ),
            _ => {}
        }
        bl_rt::rt_test!({
            processor.process(256);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }
    let heard = probe.recorded_notes();
    let ons = heard
        .iter()
        .filter(|n| n.event.kind == NoteKind::On)
        .count();
    // 800 blocks of 256 frames at 120 BPM play 34 of the clip's sixteenths:
    // at least one note-on each reaches the instrument.
    assert!(ons >= 34, "{ons} notes reached the instrument");
    eprintln!("800 blocks through six MIDI effects under RealtimeSanitizer: {ons} note-ons");
}

// Spec: device-framework R39. Control changes, pitch bends and channel
// pressure from monitored input go through two MIDI effects, one of them
// switched off and on again, into a test instrument.
#[test]
fn channel_messages_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let probe = Arc::new(Probe::default());
    let mut collector = Collector::new();
    let mut registry = testing::registry(&probe);
    bl_devices::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor) = channel(&config);
    let (input, engine_end) = bl_midi_io::input_queue(4096);
    processor.set_midi_input(engine_end);

    let mut editor = Editor::new();
    let track = editor.new_id();
    for command in [
        Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        },
        Command::SetInput {
            track,
            input: Input::ComputerKeyboard,
            channel: None,
        },
        Command::SetMonitor {
            track,
            monitor: Monitor::In,
        },
    ] {
        editor.apply(command).unwrap();
    }
    let mut effects = Vec::new();
    for (index, device_type) in [bl_devices::LIFT, bl_devices::STACK, testing::INSTRUMENT]
        .into_iter()
        .enumerate()
    {
        let id = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device: Device::new(
                    id,
                    "",
                    DeviceKind::Native {
                        device_type: device_type.to_owned(),
                    },
                ),
            })
            .unwrap();
        effects.push(id);
    }
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());

    let start = Instant::now();
    let period = Duration::from_nanos(256 * 1_000_000_000 / 48_000);
    let mut messages = 0;
    for round in 0..600u32 {
        let enabled = !(200..400).contains(&round);
        if round == 200 || round == 400 {
            editor
                .apply(Command::SetDeviceEnabled {
                    device: effects[0],
                    enabled,
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
            assert!(sender.flush());
        }
        // A burst of messages stamped inside the previous block: a note,
        // then eighty controller, bend and pressure messages, which
        // fit a track's input.
        let block = start + period * round;
        let stamp = block - period / 2;
        let send = |message: MidiMessage| {
            input.send(MidiInputEvent {
                source: MidiSource::ComputerKeyboard,
                time: stamp,
                message,
            });
        };
        send(MidiMessage::note_on(0, 36 + (round % 48) as u8, 100));
        for i in 0..80u8 {
            let value = i % 128;
            for bytes in [[0xB0, 1, value], [0xE0, value, 64], [0xD0, value, 0]] {
                let len = if bytes[0] == 0xD0 { 2 } else { 3 };
                send(MidiMessage::from_bytes(&bytes[..len]).unwrap());
            }
        }
        bl_rt::rt_test!({
            processor.process_at(256, &NoInput, block);
        });
        // The instrument's record holds 4,096 events: read it every block.
        messages += probe
            .recorded_notes()
            .iter()
            .filter(|n| n.event.kind.is_message())
            .count();
        if round % 50 == 0 {
            collector.collect();
        }
    }
    assert!(
        messages > 100_000,
        "{messages} messages reached the instrument"
    );
    eprintln!("600 blocks of channel messages under RealtimeSanitizer: {messages} heard");
}
