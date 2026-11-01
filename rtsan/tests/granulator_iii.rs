//! Granulator III's audio-thread work under RealtimeSanitizer (WP-8.26, the
//! Granulator III spec R16): overlapping notes play grains in every mode
//! while the mode, the grain controls, the filter and the voices change
//! between blocks, and the song jumps about under Loop's grid. RTSan stops
//! the process at the first allocation, free, lock, or blocking call in any
//! of it, and the allocation trap, installed as in every Beatlayer test
//! binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::granulator_iii::{
    DENSITY, FILTER, FILTER_FREQ, GRANULATOR_III, MODE, POSITION, RATE, SHAPE, SIZE, SPRAY, VOICES,
};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    Clip, ClipData, Command, ContentHash, Device, DeviceKind, DeviceRole, Editor, MidiData, Note,
    TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn granulator_iii_passes_under_rtsan() {
    let hash = ContentHash([9; 32]);
    let mut media = MediaMap::new();
    let samples = (0..96_000)
        .map(|i| (std::f64::consts::TAU * 220.0 * f64::from(i) / 48_000.0).sin() as f32 * 0.1)
        .collect();
    media.insert(
        hash,
        Arc::new(SampleData::new(48_000, vec![samples]).unwrap()),
    );
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
        "Granulator III",
        DeviceKind::Native {
            device_type: GRANULATOR_III.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.set_media(0, Some(hash));
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
    // Held notes that overlap, so voices are stolen and grains outlive them.
    let notes: Vec<Note> = (0..8)
        .map(|n| {
            Note::new(
                60 + n,
                Ticks(i64::from(n) * Ticks::QUARTER.0 / 2),
                Ticks(Ticks::QUARTER.0 * 2),
                40 + n * 9,
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
        // Between blocks, as automation or a hand would: the mode, the grain
        // controls, the filter and the voices.
        let change = match round % 20 {
            2 => Some((MODE, (round / 20 % 3) as f64)),
            5 => Some((SIZE, 1.0 + (round % 997) as f64)),
            7 => Some((DENSITY, 1.0 + (round % 16) as f64)),
            9 => Some((SPRAY, (round % 101) as f64)),
            11 => Some((POSITION, (round % 101) as f64)),
            13 => Some((SHAPE, (round * 7 % 101) as f64)),
            15 => Some((RATE, (round % 7) as f64)),
            17 => Some((FILTER, (round / 20 % 2) as f64)),
            18 => Some((FILTER_FREQ, 100.0 + (round % 50) as f64 * 300.0)),
            19 => Some((VOICES, 1.0 + (round % 8) as f64)),
            _ => None,
        };
        if let Some((param, value)) = change {
            editor
                .apply(Command::SetDeviceParam {
                    device: id,
                    param,
                    value,
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        // Jump back into the notes now and then, so the grid is found again
        // and notes retrigger mid-sound.
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
