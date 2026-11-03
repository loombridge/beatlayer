//! The Drum Synths' audio-thread work under RealtimeSanitizer (WP-8.24): a
//! MIDI clip on each of four tracks plays DS Kick, DS Snare, DS Tom and DS
//! Clap at several velocities, retriggering well inside each hit's length so
//! the 2 ms cut runs, while parameters change between blocks and a preview
//! arrives as node data. RTSan stops the process at the first allocation,
//! free, lock, or blocking call in any of it, and the allocation trap,
//! installed as in every Beatlayer test binary, fails the test at any heap
//! use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_devices::drum_synth::{DECAY, PreviewHit};
use bl_devices::{ds_clap, ds_kick, ds_snare, ds_tom};
use bl_engine::{Collector, Compiler, EngineConfig, NodeKey, Registry, TransportCommand, channel};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceKind, DeviceRole, Editor, MidiData, Note, ParamId,
    TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn the_drum_synths_hits_and_cuts_pass_under_rtsan() {
    let mut editor = Editor::new();
    // Each device type, and the parameter a hand moves between blocks.
    let synths: [(&str, ParamId); 4] = [
        (ds_kick::DS_KICK, ds_kick::PITCH),
        (ds_snare::DS_SNARE, ds_snare::COLOR),
        (ds_tom::DS_TOM, ds_tom::BEND),
        (ds_clap::DS_CLAP, ds_clap::SPREAD),
    ];
    let mut devices = Vec::new();
    for (device_type, _) in synths {
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
        let id: bl_model::DeviceId = editor.new_id();
        let mut device = Device::new(
            id,
            "",
            DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        device.params.insert(DECAY, 600.0);
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        // A note an eighth of a beat apart, softer and louder, so each one
        // arrives while the last still sounds.
        let step = Ticks::QUARTER.0 / 8;
        let notes: Vec<Note> = (0..24)
            .map(|n| Note::new(36, Ticks(n * step), Ticks(step), 30 + (n * 17 % 98) as u8))
            .collect();
        let clip = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE * 2,
                    ClipData::Midi(MidiData::new(notes)),
                ),
            })
            .unwrap();
        devices.push(id);
    }

    let registry = {
        let mut registry = Registry::new();
        bl_devices::register(&mut registry);
        registry
    };
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor) = channel(&config);
    let update = compiler.sync(editor.document());
    assert!(update.missing.is_empty());
    sender.send_update(update);
    sender.transport(TransportCommand::Play);

    for round in 0..1_500u64 {
        // Parameters change between blocks, as automation or a hand would,
        // and a preview arrives now and then.
        if round % 10 == 3 {
            let n = (round / 10 % 4) as usize;
            let (_, param) = synths[n];
            editor
                .apply(Command::SetDeviceParam {
                    device: devices[n],
                    param,
                    value: 30.0 + (round % 70) as f64,
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        if round % 40 == 7 {
            let data = collector.handle().data(PreviewHit);
            sender.send_data(NodeKey::Device(devices[(round / 40 % 4) as usize]), data);
        }
        if round % 300 == 150 {
            sender.transport(TransportCommand::Locate(Ticks::ZERO));
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process(128);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }
}
