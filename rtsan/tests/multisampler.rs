//! The multisampler's audio-thread work under RealtimeSanitizer (WP-5.13;
//! the multisampler spec, R31): notes on many layers through zone fades,
//! Round Robin, loops with crossfades, a volume envelope that loops, voice
//! stealing, a long layer read in the background, and layer lists and zone
//! edits sent as node data while it plays. RTSan stops the process at the
//! first allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::multisampler::{ENV_LOOP, MULTISAMPLER, ROUND_ROBIN, RR_MODE, VOICES};
use bl_engine::{Collector, Compiler, EngineConfig, Registry, TransportCommand, channel};
use bl_media::{BitDepth, Dither, MediaStore, wav_bytes};
use bl_model::{
    Clip, ClipData, Command, ContentHash, Device, DeviceKind, DeviceRole, Editor, Layer, LayerId,
    LayerSettings, LayerZone, MidiData, Note, SustainMode, TrackKind, Zone, ZoneKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A mono WAV of `seconds` of a quiet tone, written to `dir`.
fn tone(dir: &std::path::Path, name: &str, seconds: usize, hz: f32) -> ContentHash {
    let samples: Vec<f32> = (0..seconds * 48_000)
        .map(|i| (i as f32 * hz / 48_000.0 * std::f32::consts::TAU).sin() * 0.01)
        .collect();
    let path = dir.join(format!("{name}.wav"));
    std::fs::write(
        &path,
        wav_bytes(&samples, 1, 48_000, BitDepth::Int16, Dither::None).unwrap(),
    )
    .unwrap();
    ContentHash([hz as u8; 32])
}

#[test]
fn a_multisampler_passes_under_rtsan() {
    let dir = std::env::temp_dir().join(format!("bl-rtsan-multisampler-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(MediaStore::new());
    let mut media = Vec::new();
    for (name, seconds, hz) in [("A", 1, 220.0), ("B", 1, 330.0), ("C", 12, 440.0)] {
        tone(&dir, name, seconds, hz);
        let hash = store.import(dir.join(format!("{name}.wav"))).unwrap();
        media.push((ContentHash(*hash.as_bytes()), seconds));
    }
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let registry = registry.with_media(store);
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

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
    let mut device = Device::new(
        editor.new_id(),
        "Mosaic",
        DeviceKind::Native {
            device_type: MULTISAMPLER.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.params.insert(ROUND_ROBIN, 1.0);
    device.params.insert(RR_MODE, 2.0);
    device.params.insert(VOICES, 8.0);
    device.params.insert(ENV_LOOP, 1.0);
    let id = device.id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let layers: Vec<Layer> = media
        .iter()
        .enumerate()
        .flat_map(|(i, (hash, seconds))| {
            (0..4).map(move |copy| {
                let mut layer = Layer::new(
                    LayerId(0),
                    format!("Layer {i}-{copy}"),
                    *hash,
                    (*seconds as u64) * 48_000,
                    48_000,
                    1,
                );
                layer.key = Zone {
                    low: 36 + copy * 8,
                    high: 60 + copy * 8,
                    fade_low: 4,
                    fade_high: 4,
                };
                layer.sustain = SustainMode::Loop;
                layer.loop_start = 4_000;
                layer.loop_end = 40_000;
                layer.release_loop = 40_000;
                layer.crossfade_ms = 5.0;
                layer
            })
        })
        .collect();
    editor
        .apply(Command::InsertLayers {
            device: id,
            index: None,
            layers,
        })
        .unwrap();
    // Notes on every beat, overlapping, on a clip that loops.
    let notes: Vec<Note> = (0..32u8)
        .map(|n| {
            Note::new(
                36 + (n * 5) % 40,
                Ticks::QUARTER * i64::from(n) / 2,
                Ticks::QUARTER,
                40 + (n * 7) % 80,
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
                Ticks::WHOLE * 4,
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    editor
        .apply(Command::SetLoop {
            start: Ticks::ZERO,
            length: Ticks::WHOLE * 4,
        })
        .unwrap();
    editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let ids: Vec<LayerId> = editor.document().device(id).unwrap().layers.list.iter().map(|l| l.id).collect();
    for round in 0..2_000u64 {
        // A zone edit or a layer setting every few blocks: node data, and
        // never a new program.
        let command = match round % 10 {
            0 => Some(Command::SetLayerZones {
                device: id,
                kind: ZoneKind::Key,
                zones: vec![LayerZone {
                    layer: ids[(round / 10) as usize % ids.len()],
                    zone: Zone {
                        low: 30 + (round % 20) as u8,
                        high: 70 + (round % 30) as u8,
                        fade_low: (round % 6) as u8,
                        fade_high: (round % 4) as u8,
                    },
                }],
            }),
            5 => Some(Command::UpdateLayers {
                device: id,
                layers: vec![ids[(round / 10) as usize % ids.len()]],
                settings: LayerSettings {
                    root_key: Some(48 + (round % 24) as u8),
                    ..LayerSettings::default()
                },
            }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            let update = compiler.sync(editor.document());
            assert!(update.program.is_none(), "an edit of the layers is node data");
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
    assert!(state.meter(track).is_some_and(|[left, _]| left > 0.0), "it plays");
    eprintln!(
        "2,000 blocks of multisampler playback under RealtimeSanitizer: {} data lists, peak \
         engine load {:.1}%",
        state.counters.data_delivered,
        state.cpu_load_max * 100.0
    );
    std::fs::remove_dir_all(&dir).ok();
}
