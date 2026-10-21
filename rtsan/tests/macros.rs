//! A macro variation launch under RealtimeSanitizer (WP-5.5; macros spec,
//! R33): a rack's macro drives a device inside it, and launching a variation
//! moves every mapped parameter on one frame while the engine plays. RTSan
//! stops the process at the first allocation, free, lock, or blocking call on
//! the audio thread, and the allocation trap fails the test at any heap use it
//! sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::SampleRate;
use bl_engine::testing::{self, DC, GAIN_DESCRIBED, GAIN_PARAM, Probe};
use bl_engine::{Collector, Compiler, EngineConfig, TransportCommand, channel};
use bl_model::{
    Chain, Command, Device, DeviceId, DeviceKind, Editor, MacroTarget, RackKind, TrackId,
    TrackKind, macro_param,
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

fn device(editor: &mut Editor, kind: &str) -> Device {
    Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: kind.to_owned(),
        },
    )
}

#[test]
fn a_variation_launch_runs_clean_under_the_sanitizer() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let probe = Arc::new(Probe::default());
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, testing::registry(&probe), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let audio = track(&mut editor, TrackKind::Audio);
    // A DC source and an audio effect rack whose chain holds a described gain.
    let source = device(&mut editor, DC);
    editor
        .apply(Command::InsertDevice {
            track: audio,
            index: 0,
            device: source,
        })
        .unwrap();
    let rack: DeviceId = editor.new_id();
    let gain = device(&mut editor, GAIN_DESCRIBED);
    let gain_id = gain.id;
    let chain = Chain::new(editor.new_id(), "Chain").with_devices(vec![gain]);
    editor
        .apply(Command::InsertDevice {
            track: audio,
            index: 1,
            device: Device::rack(rack, RackKind::AudioEffect).with_chain(chain),
        })
        .unwrap();
    editor
        .apply(Command::MapMacro {
            rack,
            macro_number: 1,
            target: MacroTarget::Device {
                device: gain_id,
                param: GAIN_PARAM,
            },
        })
        .unwrap();
    editor.apply(Command::StoreVariation { rack }).unwrap();
    editor
        .apply(Command::SetDeviceParam {
            device: rack,
            param: macro_param(1),
            value: 1.0,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    for round in 0..600u64 {
        if round % 100 == 50 {
            // The launch moves every mapped parameter on one frame, and the
            // audio thread allocates, locks and blocks nothing.
            editor
                .apply(Command::LaunchVariation { rack, index: 0 })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
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
    assert_eq!(state.counters.migration_mismatches, 0);
    eprintln!("600 blocks with variation launches under RealtimeSanitizer");
}
