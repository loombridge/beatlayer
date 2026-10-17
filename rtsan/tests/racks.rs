//! Racks' audio-thread work under RealtimeSanitizer (WP-5.1): a nested rack
//! of parallel chains, each on its own buffer and through its own mixer,
//! summed in place, with chains of mismatched latencies held back at the
//! sum; chain Volume, Pan, activator and Solo gliding as they change;
//! program swaps as chains are added, devices are grouped, moved between
//! racks and ungrouped, with every node migrating; and a MIDI effect rack
//! merging its chains' notes. RTSan stops the process at the first
//! allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, DELAY, DELAY_FRAMES, GAIN, GAIN_PARAM, Probe};
use bl_engine::{Collector, Compiler, EngineConfig, TransportCommand, channel};
use bl_model::{
    Chain, ChainId, Command, Device, DeviceId, DeviceKind, DeviceRole, Editor, RackKind, TrackId,
    TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

fn track(editor: &mut Editor, kind: TrackKind) -> TrackId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    id
}

fn native(
    editor: &mut Editor,
    device_type: &str,
    role: DeviceRole,
    params: &[(u32, f64)],
) -> Device {
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: device_type.to_owned(),
        },
    )
    .with_role(role);
    for &(param, value) in params {
        device.params.insert(bl_model::ParamId(param), value);
    }
    device
}

fn rack(editor: &mut Editor, kind: RackKind, chains: Vec<Chain>) -> Device {
    let mut rack = Device::rack(editor.new_id(), kind);
    rack.chains = chains;
    rack
}

fn chain(editor: &mut Editor, devices: Vec<Device>) -> Chain {
    Chain::new(editor.new_id(), "Chain").with_devices(devices)
}

#[test]
fn a_nested_rack_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let probe = std::sync::Arc::new(Probe::default());
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, testing::registry(&probe), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let audio = track(&mut editor, TrackKind::Audio);
    let source = native(
        &mut editor,
        testing::DC,
        DeviceRole::AudioEffect,
        &[(0, 0.25)],
    );
    editor
        .apply(Command::InsertDevice {
            track: audio,
            index: 0,
            device: source,
        })
        .unwrap();
    // An outer rack of three chains: a dry path, a delayed one, and one
    // holding a rack of its own with mismatched latencies.
    let slow = native(
        &mut editor,
        DELAY,
        DeviceRole::AudioEffect,
        &[(DELAY_FRAMES.0, 300.0)],
    );
    let slower = native(
        &mut editor,
        DELAY,
        DeviceRole::AudioEffect,
        &[(DELAY_FRAMES.0, 500.0)],
    );
    let trim = native(
        &mut editor,
        GAIN,
        DeviceRole::AudioEffect,
        &[(GAIN_PARAM.0, 0.5)],
    );
    let inner_dry = chain(&mut editor, vec![]);
    let inner_slow = chain(&mut editor, vec![slower]);
    let inner = rack(
        &mut editor,
        RackKind::AudioEffect,
        vec![inner_dry.clone(), inner_slow],
    );
    let dry = chain(&mut editor, vec![]);
    let delayed = chain(&mut editor, vec![slow]);
    let nested = chain(&mut editor, vec![trim.clone(), inner]);
    let chains: Vec<ChainId> = [&dry, &delayed, &nested].iter().map(|c| c.id).collect();
    let outer = rack(
        &mut editor,
        RackKind::AudioEffect,
        vec![dry, delayed, nested],
    );
    let outer_id = outer.id;
    editor
        .apply(Command::InsertDevice {
            track: audio,
            index: 1,
            device: outer,
        })
        .unwrap();

    // A MIDI track with a MIDI effect rack of two chains and an instrument.
    let midi = track(&mut editor, TrackKind::Midi);
    let arp_a = native(
        &mut editor,
        testing::TRANSPOSE,
        DeviceRole::MidiEffect,
        &[(0, 1.0)],
    );
    let arp_b = native(
        &mut editor,
        testing::TRANSPOSE,
        DeviceRole::MidiEffect,
        &[(0, 7.0)],
    );
    let (first, second) = (
        chain(&mut editor, vec![arp_a]),
        chain(&mut editor, vec![arp_b]),
    );
    let midi_chains = [first.id, second.id];
    let notes = rack(&mut editor, RackKind::MidiEffect, vec![first, second]);
    let instrument = native(
        &mut editor,
        testing::INSTRUMENT,
        DeviceRole::Instrument,
        &[],
    );
    for (index, device) in [notes, instrument].into_iter().enumerate() {
        editor
            .apply(Command::InsertDevice {
                track: midi,
                index,
                device,
            })
            .unwrap();
    }
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: midi,
            clip: bl_model::Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 8),
                bl_model::ClipData::Midi(bl_model::MidiData::new((0..16u8).map(|n| {
                    bl_model::Note::new(
                        48 + n,
                        Ticks(TICKS_PER_QUARTER / 2 * i64::from(n)),
                        Ticks(TICKS_PER_QUARTER / 4),
                        100,
                    )
                }))),
            ),
        })
        .unwrap();

    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    let mut grouped: Option<DeviceId> = None;
    for round in 0..2_000u64 {
        let command = match round % 30 {
            0 => Some(Command::SetChainVolume {
                chain: chains[(round / 30) as usize % 3],
                volume: (round % 7) as f64 / 6.0,
            }),
            4 => Some(Command::SetChainPan {
                chain: chains[1],
                pan: ((round % 5) as f64 - 2.0) / 2.0,
            }),
            8 => Some(Command::SetChainEnabled {
                chain: chains[2],
                enabled: round % 60 != 8,
            }),
            12 => Some(Command::SetChainSolo {
                chain: chains[0],
                solo: round % 120 == 12,
            }),
            14 => Some(Command::SetChainEnabled {
                chain: midi_chains[1],
                enabled: round % 60 != 14,
            }),
            // Now and then a structural edit: a program swap in which every
            // node migrates.
            20 if grouped.is_none() => {
                let rack: DeviceId = editor.new_id();
                let wrapper: ChainId = editor.new_id();
                grouped = Some(rack);
                Some(Command::GroupDevices {
                    devices: vec![outer_id],
                    rack: Device::rack(rack, RackKind::AudioEffect)
                        .with_chain(Chain::new(wrapper, "Group")),
                })
            }
            20 => grouped
                .take()
                .map(|device| Command::UngroupDevice { device }),
            26 if round % 90 == 26 => Some(Command::SetDeviceEnabled {
                device: trim.id,
                enabled: round % 180 == 26,
            }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            let update = compiler.sync(editor.document());
            programs += u64::from(update.program.is_some());
            sender.send_update(update);
        }
        assert!(sender.flush());

        bl_rt::rt_test!({
            processor.process(256);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert_eq!(state.counters.events_unrouted, 0);
    assert_eq!(state.counters.migration_mismatches, 0);
    assert_eq!(state.swaps, programs);
    assert!(programs > 5, "{programs} programs");
    eprintln!(
        "2,000 blocks of a nested rack under RealtimeSanitizer: {} parameter changes, {} swaps, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.swaps,
        state.cpu_load_max * 100.0
    );
}
