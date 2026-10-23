//! Tuning systems' audio-thread work under RealtimeSanitizer (WP-6.9, the
//! tunings spec R18): held notes play on Drift and Simpler while the set's
//! tuning is loaded, its reference pitch and range change, a track's Bypass
//! Tuning flips and the tuning is removed, between blocks. RTSan stops the
//! process at the first allocation, free, lock, or blocking call in any of
//! it, and the allocation trap, installed as in every Beatlayer test
//! binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::polysynth::POLYSYNTH;
use bl_devices::sampler::SAMPLER;
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    Clip, ClipData, Command, ContentHash, Device, DeviceKind, DeviceRole, Editor, MidiData, Note,
    TrackId, TrackKind, Tuning, TuningNote,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// 19 equal steps to the octave.
fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".to_owned(), String::new(), cents).unwrap()
}

#[test]
fn instruments_playing_through_tuning_changes_pass_under_rtsan() {
    let hash = ContentHash([5; 32]);
    let mut media = MediaMap::new();
    let tone = (0..96_000)
        .map(|i| (std::f64::consts::TAU * 220.0 * f64::from(i) / 48_000.0).sin() as f32 * 0.01)
        .collect();
    media.insert(hash, Arc::new(SampleData::new(48_000, vec![tone]).unwrap()));
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
    let mut tracks: Vec<TrackId> = Vec::new();
    for (name, device_type) in [("Drift", POLYSYNTH), ("Simpler", SAMPLER)] {
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: name.to_owned(),
                parent: None,
                index: tracks.len(),
            })
            .unwrap();
        let id = editor.new_id();
        let mut device = Device::new(
            id,
            name,
            DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        if device_type == SAMPLER {
            device.set_media(0, Some(hash));
        }
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        // Held notes that overlap, so voices are stolen and retriggered
        // while the tuning changes under them.
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
        tracks.push(track);
    }
    let update = compiler.sync(editor.document());
    assert!(update.missing.is_empty(), "the sample is found");
    sender.send_update(update);
    sender.transport(TransportCommand::Play);

    for round in 0..1_500u64 {
        // The tuning, its reference pitch and range, a track's Bypass Tuning
        // and the tuning's removal change between blocks, as a hand or an
        // agent would change them.
        let command = match round % 30 {
            2 => Some(Command::SetTuning {
                tuning: nineteen(),
                retune: false,
            }),
            6 => Some(Command::SetTuningReference {
                octave: None,
                degree: None,
                frequency: Some(250.0 + (round % 40) as f64),
            }),
            10 => Some(Command::SetTuningRange {
                lowest: Some(TuningNote {
                    octave: -1,
                    degree: (round % 19) as u32,
                }),
                highest: None,
            }),
            14 => Some(Command::SetTuningBypass {
                track: tracks[0],
                bypass: true,
            }),
            18 => Some(Command::SetTuningBypass {
                track: tracks[0],
                bypass: false,
            }),
            22 => Some(Command::SetTuningReference {
                octave: Some(4),
                degree: Some(3),
                frequency: None,
            }),
            26 => Some(Command::RemoveTuning { retune: false }),
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
