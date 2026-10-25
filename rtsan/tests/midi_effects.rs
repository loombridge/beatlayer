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

// Spec: articulation-map R14. The Articulation Map on the audio thread
// under RealtimeSanitizer: a clip with a trigger key and a chord plays
// through it into a test instrument, and its list changes while it plays.
// RTSan stops the process at the first allocation, free, lock, or blocking
// call on the audio thread, and the allocation trap fails the test at any
// heap use it sees.
#[test]
fn the_articulation_map_passes_under_rtsan() {
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
    let map = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                map,
                "",
                DeviceKind::Native {
                    device_type: bl_devices::articulation_map::ARTICULATION_MAP.to_owned(),
                },
            ),
        })
        .unwrap();
    // The first articulation triggers on key 36 and plays a keyswitch and
    // a controller; the second triggers on key 37.
    editor
        .apply(Command::SetArticulation {
            device: map,
            index: 0,
            name: Some("Staccato".to_owned()),
            trigger: Some(bl_model::ArticulationTrigger::Key { key: 36 }),
            outputs: Some(vec![
                bl_model::ArticulationOutput::Key {
                    key: 24,
                    velocity: 100,
                },
                bl_model::ArticulationOutput::Controller {
                    controller: 1,
                    value: 64,
                },
            ]),
            channel: Some(Some(5)),
        })
        .unwrap();
    let instrument = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 1,
            device: Device::new(
                instrument,
                "",
                DeviceKind::Native {
                    device_type: testing::INSTRUMENT.to_owned(),
                },
            ),
        })
        .unwrap();

    // A bar of sixteenth notes on many keys, with a trigger every fourth.
    let sixteenth = Ticks(TICKS_PER_QUARTER / 4);
    let notes = (0..64u8).flat_map(|i| {
        let key = if i % 4 == 0 { 36 } else { 48 + i };
        [Note::new(key, sixteenth * i64::from(i), sixteenth * 3, 90)]
    });
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

    for round in 0..400u32 {
        if round == 200 {
            // A second articulation, and the list changes while it plays.
            editor
                .apply(Command::AddArticulation {
                    device: map,
                    index: None,
                    name: "Legato".to_owned(),
                })
                .unwrap();
            editor
                .apply(Command::SetArticulation {
                    device: map,
                    index: 1,
                    name: None,
                    trigger: Some(bl_model::ArticulationTrigger::Key { key: 37 }),
                    outputs: Some(vec![bl_model::ArticulationOutput::Key {
                        key: 25,
                        velocity: 100,
                    }]),
                    channel: Some(Some(6)),
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
            assert!(sender.flush());
        }
        bl_rt::rt_test!({
            processor.process(256);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }
    let heard = probe.recorded_notes();
    let keyswitches = heard
        .iter()
        .filter(|n| n.event.kind == NoteKind::On && n.event.key == 24)
        .count();
    assert!(
        keyswitches > 0,
        "the trigger played the keyswitch: {keyswitches} times"
    );
    eprintln!(
        "400 blocks through the Articulation Map under RealtimeSanitizer: {keyswitches} keyswitches"
    );
}

// Spec: scales R15 to R17. Pitch, Random, Chord, Scale and Arpeggiator, each
// counting degrees of the current scale (Use Current Scale on), play a clip
// through changes of its scale, the Session clip taking over, and the set's
// scale on the audio thread.
#[test]
fn scale_aware_devices_pass_a_scale_change_under_rtsan() {
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
    let switches = [
        (bl_devices::LIFT, bl_devices::lift::USE_SCALE),
        (bl_devices::DICE, bl_devices::dice::USE_SCALE),
        (bl_devices::STACK, bl_devices::stack::USE_SCALE),
        (bl_devices::REMAP, bl_devices::remap::USE_SCALE),
        (bl_devices::ARPEGGIATOR, bl_devices::arpeggiator::USE_SCALE),
    ];
    for (index, (device_type, switch)) in switches.iter().enumerate() {
        let id = editor.new_id();
        let mut device = Device::new(
            id,
            "",
            DeviceKind::Native {
                device_type: (*device_type).to_owned(),
            },
        );
        device.params.insert(*switch, 1.0);
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device,
            })
            .unwrap();
    }
    let id = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: switches.len(),
            device: Device::new(
                id,
                "",
                DeviceKind::Native {
                    device_type: testing::INSTRUMENT.to_owned(),
                },
            ),
        })
        .unwrap();
    let sixteenth = Ticks(TICKS_PER_QUARTER / 4);
    let notes = (0..64u8).map(|i| Note::new(36 + i, sixteenth * i64::from(i), sixteenth * 3, 90));
    let mut arranged = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks(TICKS_PER_QUARTER * 16),
        ClipData::Midi(MidiData::new(notes)),
    );
    arranged.scale = bl_model::Scale::new(0, bl_model::ScaleName::Major);
    let arranged_id = arranged.id;
    editor
        .apply(Command::CreateClip {
            track,
            clip: arranged,
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
            100 | 300 | 500 => change(
                &mut editor,
                Command::SetClipScale {
                    clips: vec![arranged_id],
                    enabled: Some(round != 300),
                    root: Some((round / 100) as u8),
                    name: Some("dorian".to_owned()),
                },
            ),
            200 | 400 => change(
                &mut editor,
                Command::SetScale {
                    enabled: Some(true),
                    root: Some((round / 100) as u8),
                    name: Some("minor_pentatonic".to_owned()),
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
    let ons = probe
        .recorded_notes()
        .iter()
        .filter(|n| n.event.kind == NoteKind::On)
        .count();
    // The arpeggiator turns the clip's chords into steps of its own, so fewer
    // notes than the clip holds reach the instrument.
    assert!(ons >= 10, "{ons} notes reached the instrument");
}

// Spec: arpeggiator R19, R20. An Arpeggiator with Swing 16ths 60 at Rate 1/16
// plays a chord through changes of the Global Amount, Sync and the groove,
// and the notes it holds for their grooved frames pass on the audio thread.
#[test]
fn a_swung_arpeggio_passes_under_rtsan() {
    use bl_devices::arpeggiator::{ARPEGGIATOR, GROOVE, RATE_SYNC, SYNC};

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
    let arpeggiator = editor.new_id();
    let mut device = Device::new(
        arpeggiator,
        "",
        DeviceKind::Native {
            device_type: ARPEGGIATOR.to_owned(),
        },
    );
    device.params.insert(RATE_SYNC, 2.0);
    device.params.insert(GROOVE, 7.0);
    let instrument = editor.new_id();
    for (index, device) in [
        device,
        Device::new(
            instrument,
            "",
            DeviceKind::Native {
                device_type: testing::INSTRUMENT.to_owned(),
            },
        ),
    ]
    .into_iter()
    .enumerate()
    {
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device,
            })
            .unwrap();
    }
    let notes =
        [60, 64, 67].map(|key| Note::new(key, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 16), 90));
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
                Command::SetGlobalGrooveAmount { amount: 130.0 },
            ),
            300 => change(&mut editor, Command::SetGlobalGrooveAmount { amount: 0.0 }),
            400 => change(
                &mut editor,
                Command::SetGlobalGrooveAmount { amount: 100.0 },
            ),
            500 | 600 => change(
                &mut editor,
                Command::SetDeviceParam {
                    device: arpeggiator,
                    param: if round == 500 { SYNC } else { GROOVE },
                    value: 0.0,
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
    let ons = probe
        .recorded_notes()
        .iter()
        .filter(|n| n.event.kind == NoteKind::On)
        .count();
    // 800 blocks of 256 frames at 120 BPM hold 34 sixteenth steps.
    assert!(ons >= 30, "{ons} notes reached the instrument");
}
