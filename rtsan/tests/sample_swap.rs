//! Similar sample swapping on the audio thread under RealtimeSanitizer
//! (WP-9.10; the sound similarity spec, R29): a Simpler and a Drum Sampler
//! play overlapping held notes while their sample is swapped for another
//! every few blocks, as Cmd-Right does, so voices fade out on the sample they
//! began on while new notes play the new one. RTSan stops the process at the
//! first allocation, free, lock, or blocking call on the audio thread, and
//! the allocation trap fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::drum_sampler::drum_sampler_type;
use bl_devices::sampler::sampler_type;
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{Clip, ClipData, Command, ContentHash, DeviceId, Editor, MidiData, Note, TrackKind};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A mono sample of `frames` frames at 48 kHz: a quiet sine of its own pitch.
fn hit(frames: usize, hz: f64) -> Arc<SampleData> {
    let samples = (0..frames)
        .map(|i| (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin() as f32 * 0.2)
        .collect();
    Arc::new(SampleData::new(48_000, vec![samples]).unwrap())
}

#[test]
fn swapping_the_sample_while_playing_passes_under_rtsan() {
    let hashes = [
        ContentHash([1; 32]),
        ContentHash([2; 32]),
        ContentHash([3; 32]),
    ];
    let mut media = MediaMap::new();
    for (n, hash) in hashes.iter().enumerate() {
        // Different lengths, so a voice that outlives its sample is read
        // past the end of the one it began on.
        media.insert(*hash, hit(30_000 + 20_000 * n, 110.0 * (n + 1) as f64));
    }
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(
        config,
        registry.with_media(Arc::new(media)),
        collector.handle(),
    );
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let mut devices: Vec<DeviceId> = Vec::new();
    for device_type in [sampler_type(), drum_sampler_type()] {
        let track = editor.new_id();
        let index = editor.document().top_level().len();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index,
            })
            .unwrap();
        let mut device = device_type.device(editor.new_id());
        device.set_media(0, Some(hashes[0]));
        devices.push(device.id);
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        // Held notes that overlap, so voices are always sounding.
        let notes: Vec<Note> = (0..8)
            .map(|n| {
                Note::new(
                    60 + n,
                    Ticks(i64::from(n) * Ticks::QUARTER.0 / 2),
                    Ticks(Ticks::QUARTER.0 * 2),
                    60 + n * 8,
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
    }
    let update = compiler.sync(editor.document());
    assert!(update.missing.is_empty(), "the samples are found");
    sender.send_update(update);
    sender.transport(TransportCommand::Play);

    let mut swaps = 0;
    for round in 0..1_500u64 {
        // A swap every few blocks, each one transaction of the commands the
        // shell makes: the sample, then the reference it keeps.
        if round % 7 == 3 {
            swaps += 1;
            let next = hashes[swaps % hashes.len()];
            for &device in &devices {
                editor.begin("Swap Sample");
                editor
                    .apply(Command::SetDeviceSample {
                        device,
                        media: Some(next),
                        slot: 0,
                    })
                    .unwrap();
                editor
                    .apply(Command::SetSimilarityReference {
                        device,
                        media: Some(hashes[0]),
                    })
                    .unwrap();
                editor.commit().unwrap();
            }
            let update = compiler.sync(editor.document());
            assert!(update.missing.is_empty(), "the swapped sample is found");
            sender.send_update(update);
        }
        // Jump back into the notes now and then, so they retrigger.
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
    assert!(swaps > 100, "many swaps: {swaps}");
    let _ = &mut monitor;
}
