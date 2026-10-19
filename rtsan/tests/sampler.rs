//! A warped Snapshot's audio-thread work under RealtimeSanitizer (WP-5.15b,
//! the sampler spec R18): held notes play a warped sample at the set's
//! tempo while its warp mode, its mode's controls and the sample's tempo
//! change between blocks. RTSan stops the process at the first allocation,
//! free, lock, or blocking call in any of it, and the allocation trap,
//! installed as in every Beatlayer test binary, fails the test at any heap
//! use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::sampler::SAMPLER;
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    BeatsUpdate, Clip, ClipData, Command, ContentHash, Device, DeviceKind, DeviceRole, DeviceWarp,
    Editor, MidiData, Note, Preserve, TextureUpdate, TrackKind, WarpMode, WarpSettings,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A mono sample of `frames` frames at 48 kHz: a quiet sine of its own pitch.
fn hit(frames: usize, hz: f64) -> Arc<SampleData> {
    let samples = (0..frames)
        .map(|i| (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin() as f32 * 0.01)
        .collect();
    Arc::new(SampleData::new(48_000, vec![samples]).unwrap())
}

#[test]
fn a_warped_snapshot_passes_under_rtsan() {
    let hash = ContentHash([7; 32]);
    let mut media = MediaMap::new();
    media.insert(hash, hit(96_000, 220.0));
    let registry = {
        let mut registry = Registry::new().with_media(Arc::new(media));
        bl_devices::register(&mut registry);
        registry
    };
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let id: bl_model::DeviceId = editor.new_id();
    let mut device = Device::new(
        id,
        "Simpler",
        DeviceKind::Native {
            device_type: SAMPLER.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.set_media(0, Some(hash));
    device.warp = DeviceWarp {
        warp: true,
        mode: WarpMode::Beats,
        settings: WarpSettings::default(),
        bpm: Some(90.0),
    };
    device.slices = vec![24_000, 48_000];

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
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    // Held notes that overlap, so voices are stolen and retriggered while
    // the warp settings change.
    let notes: Vec<Note> = (0..8)
        .map(|n| {
            Note::new(
                60 + n,
                Ticks(i64::from(n) * Ticks::QUARTER.0 / 2),
                Ticks(Ticks::QUARTER.0 * 2),
                40 + (n * 9) as u8,
            )
        })
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
    let update = compiler.sync(editor.document());
    assert!(update.missing.is_empty(), "the sample is found");
    sender.send_update(update);
    sender.transport(TransportCommand::Play);

    for round in 0..1_500u64 {
        // The warp mode, its controls and the sample's tempo change between
        // blocks, as automation or a hand would.
        let command = match round % 25 {
            3 => Some(Command::SetDeviceWarpMode {
                device: id,
                mode: WarpMode::Beats,
            }),
            7 => Some(Command::SetDeviceWarpSettings {
                device: id,
                beats: Some(BeatsUpdate {
                    preserve: Some(Preserve::Quarter),
                    transient_loop_mode: None,
                    transient_envelope: Some((round % 101) as u32),
                }),
                tones: None,
                texture: None,
                complex_pro: None,
            }),
            11 => Some(Command::SetDeviceWarpMode {
                device: id,
                mode: WarpMode::Texture,
            }),
            15 => Some(Command::SetDeviceWarpSettings {
                device: id,
                beats: None,
                tones: None,
                texture: Some(TextureUpdate {
                    grain_size: Some(50.0),
                    flux: Some((round % 101) as f64),
                }),
                complex_pro: None,
            }),
            19 => Some(Command::ScaleDeviceTempo {
                device: id,
                factor: if round % 2 == 0 { 2.0 } else { 0.5 },
            }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        // Jump back into the notes now and then, so they retrigger and stop
        // mid-sound.
        if round % 300 == 150 {
            sender.transport(TransportCommand::Locate(Ticks(Ticks::QUARTER.0 * 3)));
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process(128);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }
    let _ = &mut monitor;
}
