//! The engine's audio-thread work under RealtimeSanitizer: 1,000 program
//! swaps with node migration, parameter changes on exact frames, scheduled
//! commands, node data, transport commands, and the state published for the
//! UI, with every block a real-time section. RTSan stops the process at the
//! first allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_engine::testing::{self, Probe};
use bl_engine::{
    Action, Collector, Compiler, EngineConfig, NodeKey, ParamEvent, ParamValue, Report,
    ScheduledCommand, TransportCommand, channel,
};
use bl_model::{Command, Device, DeviceId, DeviceKind, Editor, TrackId, TrackKind};

// Without `--cfg bl_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "bl-rt wasn't built with --cfg bl_rtsan; run `cargo xtask rtsan`"
);

fn native(editor: &mut Editor, track: TrackId, device_type: &str) -> DeviceId {
    let id = editor.new_id();
    let device = Device::new(
        id,
        "",
        DeviceKind::Native {
            device_type: device_type.to_owned(),
        },
    );
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    id
}

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

#[test]
fn a_thousand_swaps_and_everything_else_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let probe = Arc::new(Probe::default());
    let mut collector = Collector::new();
    let gc = collector.handle();
    let mut compiler = Compiler::new(config, testing::registry(&probe), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let tempo = TempoMap::default();
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Midi);
    let b = track(&mut editor, TrackKind::Audio);
    native(&mut editor, a, bl_engine::nodes::TEST_TONE);
    let level = native(&mut editor, a, testing::DC);
    let data = native(&mut editor, a, testing::DATA);
    native(&mut editor, a, testing::COUNTER);
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut spare = None;
    for round in 0..1000u64 {
        // A structural edit each round: add a device, switch it off, remove it.
        match (round % 3, spare) {
            (0, _) => spare = Some(native(&mut editor, b, testing::DC)),
            (1, Some(device)) => {
                editor
                    .apply(Command::SetDeviceEnabled {
                        device,
                        enabled: false,
                    })
                    .unwrap();
            }
            (_, Some(device)) => {
                editor.apply(Command::RemoveDevice { device }).unwrap();
            }
            _ => unreachable!(),
        }
        let update = compiler.sync(editor.document());
        assert!(update.program.is_some(), "round {round}");
        sender.send_update(update);
        editor
            .apply(Command::SetVolume {
                track: a,
                volume: if round % 2 == 0 { 0.5 } else { 1.0 },
            })
            .unwrap();
        sender.send_update(compiler.sync(editor.document()));
        sender.send_param(ParamEvent::at_frame(
            NodeKey::Device(level),
            testing::VALUE,
            round as f64 / 1000.0,
            processor.clock() + 64,
        ));
        sender.send_data(NodeKey::Device(data), gc.data(round as f32));
        sender.schedule(ScheduledCommand {
            id: round,
            // A block and a bit ahead of the playhead.
            at: tempo.frames_to_ticks(processor.position() + Frames(200), config.sample_rate),
            action: Action::Param {
                key: NodeKey::Device(level),
                param: testing::VALUE,
                value: ParamValue::Default,
            },
        });
        assert!(sender.flush());

        bl_rt::rt_test!({
            processor.process(128);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.swaps, 1001);
    assert_eq!(state.counters.migration_mismatches, 0);
    assert_eq!(state.counters.data_delivered, 1000);
    assert_eq!(probe.counter.load(Relaxed), 1);
    assert_eq!(probe.counted.load(Relaxed), 128_000);
    let fired = monitor
        .reports()
        .into_iter()
        .filter(|report| matches!(report, Report::Fired { .. }))
        .count();
    assert_eq!(fired, 999, "every command but the last fired");
    eprintln!(
        "1,000 swaps under RealtimeSanitizer: {} scheduled commands fired, {} parameter changes applied, peak engine load {:.1}%",
        fired,
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}
