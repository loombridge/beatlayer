//! Device displays under RealtimeSanitizer (WP-5.43): EQ Eight's spectrum and
//! Compressor's gain reduction are published after every block, in blocks of 1
//! to 4,096 frames, while parameters move and programs swap. RTSan stops the
//! process at the first allocation, free, lock, or blocking call on the
//! audio thread, and the allocation trap fails the test at any heap use it
//! sees.

bl_rt::install_trap!();

use std::collections::BTreeMap;

use bl_core::time::SampleRate;
use bl_devices::compressor::{COMPRESSOR, THRESHOLD};
use bl_devices::eq::{ANALYZE, EQ};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, EngineConfig, Registry, TransportCommand, channel};
use bl_model::{Command, Device, DeviceId, DeviceKind, Editor, ParamId, TrackKind};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

fn insert(editor: &mut Editor, track: bl_model::TrackId, kind: &str) -> DeviceId {
    let id = editor.new_id();
    let device = Device::new(
        id,
        "",
        DeviceKind::Native {
            device_type: kind.to_owned(),
        },
    );
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

fn set(editor: &mut Editor, device: DeviceId, param: ParamId, value: f64) {
    editor
        .apply(Command::SetDeviceParam {
            device,
            param,
            value,
        })
        .unwrap();
}

#[test]
fn displays_publish_after_every_block_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 4096);
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let displays = compiler.displays();
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let tone = insert(&mut editor, track, TEST_TONE);
    set(&mut editor, tone, test_tone::FREQUENCY, 1000.0);
    set(&mut editor, tone, test_tone::LEVEL, -6.0);
    let contour = insert(&mut editor, track, EQ);
    let squeeze = insert(&mut editor, track, COMPRESSOR);
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut read = BTreeMap::new();
    let mut blocks = 0u64;
    let mut extra = None;
    for round in 0..1_200u64 {
        let command = match round % 40 {
            0 => Some((contour, ANALYZE, 0.0)),
            4 => Some((contour, ANALYZE, 1.0)),
            8 => Some((squeeze, THRESHOLD, -40.0)),
            16 => Some((squeeze, THRESHOLD, -10.0)),
            _ => None,
        };
        if let Some((device, param, value)) = command {
            set(&mut editor, device, param, value);
            sender.send_update(compiler.sync(editor.document()));
        }
        // A program swap: a device joins the chain and leaves it, and the
        // displays of the two that stay go on publishing.
        match (round % 100, extra) {
            (30, None) => {
                extra = Some(insert(&mut editor, track, COMPRESSOR));
                sender.send_update(compiler.sync(editor.document()));
            }
            (60, Some(device)) => {
                editor.apply(Command::RemoveDevice { device }).unwrap();
                extra = None;
                sender.send_update(compiler.sync(editor.document()));
            }
            _ => {}
        }
        assert!(sender.flush());

        let frames = [1, 64, 512, 4096][(round % 4) as usize];
        bl_rt::rt_test!({
            processor.process(frames);
        });
        blocks += 1;
        displays.read_all(&mut read);
        for device in [contour, squeeze] {
            assert_eq!(
                read[&device].published, blocks,
                "round {round}: the display of {device} is published after every block"
            );
        }
        if round % 50 == 0 {
            collector.collect();
        }
    }
    let state = monitor.state().clone();
    assert_eq!(state.counters.migration_mismatches, 0);
    assert!(state.swaps > 5, "{} programs", state.swaps);
    eprintln!(
        "{blocks} blocks of device displays under RealtimeSanitizer: {} swaps, peak engine load {:.1}%",
        state.swaps,
        state.cpu_load_max * 100.0
    );
}
