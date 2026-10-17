//! The multicore scheduler's real-time work under RealtimeSanitizer
//! (WP-5.37): the audio thread and a pool of workers run a song of many
//! tracks and a rack of parallel chains, taking ready work from the
//! lock-free queue, joining the chains of the rack, and waking and sleeping
//! on semaphores, while program swaps replace the program under them. RTSan
//! stops the process at the first allocation, free, lock, or blocking call on
//! the audio thread or on any worker while it runs a task or a chain, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees on any of them.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::SampleRate;
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::testing::{self, GAIN, GAIN_PARAM, Probe, SPIN, SPIN_MICROS};
use bl_engine::{
    Collector, Compiler, EngineConfig, Pool, PoolConfig, TransportCommand, channel,
};
use bl_model::{
    Chain, Command, Device, DeviceKind, DeviceRole, Editor, RackKind, TrackId, TrackKind,
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

#[test]
fn a_pool_of_workers_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let probe = Arc::new(Probe::default());
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, testing::registry(&probe), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);
    let pool = Pool::start(&config, &PoolConfig::plain(3)).unwrap();
    let pool_monitor = pool.monitor();
    processor.set_pool(Some(pool));

    // Tracks that run apart, each a tone through a gain, and one that
    // holds a rack of four chains that each keep a thread busy for a
    // while, so the chains are shared among the threads.
    let mut editor = Editor::new();
    for n in 0..12u32 {
        let id = track(&mut editor, TrackKind::Audio);
        for (index, device) in [
            native(
                &mut editor,
                TEST_TONE,
                DeviceRole::AudioEffect,
                &[
                    (test_tone::FREQUENCY.0, 110.0 + 41.0 * f64::from(n)),
                    (test_tone::LEVEL.0, -24.0),
                ],
            ),
            native(
                &mut editor,
                GAIN,
                DeviceRole::AudioEffect,
                &[(GAIN_PARAM.0, 0.5)],
            ),
        ]
        .into_iter()
        .enumerate()
        {
            editor
                .apply(Command::InsertDevice {
                    track: id,
                    index,
                    device,
                })
                .unwrap();
        }
    }
    let racked = track(&mut editor, TrackKind::Audio);
    let mut rack = Device::rack(editor.new_id(), RackKind::AudioEffect);
    for _ in 0..4 {
        let spin = native(
            &mut editor,
            SPIN,
            DeviceRole::AudioEffect,
            &[(SPIN_MICROS.0, 100.0)],
        );
        rack = rack.with_chain(Chain::new(editor.new_id(), "Chain").with_devices(vec![spin]));
    }
    editor
        .apply(Command::InsertDevice {
            track: racked,
            index: 0,
            device: rack,
        })
        .unwrap();

    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    for round in 0..2_000u64 {
        // Now and then a structural edit: a program swap under the workers,
        // in which every node migrates.
        if round % 100 == 50 {
            let id = track(&mut editor, TrackKind::Audio);
            let device = native(
                &mut editor,
                GAIN,
                DeviceRole::AudioEffect,
                &[(GAIN_PARAM.0, 0.25)],
            );
            editor
                .apply(Command::InsertDevice {
                    track: id,
                    index: 0,
                    device,
                })
                .unwrap();
            let update = compiler.sync(editor.document());
            programs += u64::from(update.program.is_some());
            sender.send_update(update);
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process(128);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.migration_mismatches, 0);
    assert_eq!(state.swaps, programs);
    assert!(programs > 5, "{programs} programs");
    assert!(
        probe.threads_spun_on() > 1,
        "the rack's chains were not shared"
    );
    assert_eq!(pool_monitor.heap_operations(), 0);
    assert_eq!(pool_monitor.panics(), 0);
    eprintln!(
        "2,000 blocks of 13 tracks and a rack of 4 chains on 3 workers under RealtimeSanitizer: \
         {} swaps, peak engine load {:.1}%, the rack's chains ran on {} threads",
        state.swaps,
        state.cpu_load_max * 100.0,
        probe.threads_spun_on()
    );
}
