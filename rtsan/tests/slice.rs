//! A sliced drum rack's audio-thread work under RealtimeSanitizer (WP-4.16;
//! the slice-to-midi spec R14): a Drum Rack of One-Shot samplers, one chain
//! per slice, plays its MIDI clip while the slices' regions and modes move
//! between blocks. RTSan stops the process at the first allocation, free,
//! lock, or blocking call in any of it, and the allocation trap, installed
//! as in every Beatlayer test binary, fails the test at any heap use it
//! sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::sampler::{
    LENGTH, MODE, PLAYBACK, SAMPLER, SLICE_BY, SLICE_REGIONS, START, TRIGGER,
};
use bl_engine::{Collector, Compiler, EngineConfig, MediaMap, Registry, TransportCommand, channel};
use bl_media::SampleData;
use bl_model::{
    Chain, Clip, ClipData, Command, ContentHash, Device, DeviceKind, DeviceRole, Editor, MidiData,
    Note, RackKind, SampleRegion, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A mono sample of `frames` frames at 48 kHz: a quiet sine of its own
/// pitch, so a slice of it is recognisable.
fn hit(frames: usize, hz: f64) -> Arc<SampleData> {
    let samples = (0..frames)
        .map(|i| (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin() as f32 * 0.01)
        .collect();
    Arc::new(SampleData::new(48_000, vec![samples]).unwrap())
}

#[test]
fn a_sliced_drum_rack_plays_under_rtsan() {
    let hash = ContentHash([9; 32]);
    let frames = 48_000u64;
    let mut media = MediaMap::new();
    media.insert(hash, hit(frames as usize, 220.0));
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
    let midi = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: midi,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    // Four slices of the sample, one chain each, from C1 up, playing C3.
    let mut rack = Device::rack(editor.new_id(), RackKind::Drum);
    let slices = [0u64, 12_000, 24_000, 36_000, 48_000];
    for at in 0..4usize {
        let mut sampler = Device::new(
            editor.new_id(),
            "Simpler",
            DeviceKind::Native {
                device_type: SAMPLER.to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        sampler.set_media(0, Some(hash));
        let (start, end) = (slices[at], slices[at + 1]);
        sampler.params.insert(MODE, 1.0);
        sampler.params.insert(TRIGGER, 1.0);
        sampler
            .params
            .insert(START, (start as f64 + 0.5) * 100.0 / frames as f64);
        sampler
            .params
            .insert(LENGTH, (end - start) as f64 * 100.0 / frames as f64);
        let mut chain = Chain::for_pad(
            editor.new_id(),
            format!("Slice {}", at + 1),
            36 + u8::try_from(at).unwrap(),
        );
        chain.devices.push(sampler);
        rack.chains.push(chain);
    }
    let samplers: Vec<bl_model::DeviceId> = rack
        .chains
        .iter()
        .flat_map(|chain| chain.devices.iter().map(|device| device.id))
        .collect();
    editor
        .apply(Command::InsertDevice {
            track: midi,
            index: 0,
            device: rack,
        })
        .unwrap();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: midi,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new((0..16u8).map(|n| {
                    Note::new(
                        36 + n % 4,
                        Ticks(Ticks::QUARTER.get() / 4 * i64::from(n)),
                        Ticks(Ticks::QUARTER.get() / 2),
                        127,
                    )
                }))),
            ),
        })
        .unwrap();

    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    for round in 0..1_000u64 {
        // Now and then a slice's region and mode move, as the dialog's OK
        // would move them.
        if round % 50 == 0 {
            let at = (round / 50) as usize % 4;
            editor
                .apply(Command::SetDeviceParam {
                    device: samplers[at],
                    param: START,
                    value: (round % 7) as f64 / 100.0,
                })
                .unwrap();
            editor
                .apply(Command::SetDeviceParam {
                    device: samplers[at],
                    param: LENGTH,
                    value: 20.0 + (round % 5) as f64,
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
        if round % 100 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert_eq!(state.counters.migration_mismatches, 0);
    assert_eq!(state.swaps, programs);
    eprintln!(
        "1,000 blocks of a sliced drum rack under RealtimeSanitizer: {} parameter changes, \
         {} swaps, peak engine load {:.1}%",
        state.counters.events_applied,
        state.swaps,
        state.cpu_load_max * 100.0
    );
}

/// Spec: slice-to-midi R15, R17; sampler R23. A Simpler in Slice mode, whose
/// Playback changes between blocks, and the Drum Rack Slice to Drum Rack
/// makes from one (chains in a choke group, One-Shot copies with their flags
/// on their slices) play under RTSan.
#[test]
fn a_slice_mode_simpler_and_the_rack_made_from_it_play_under_rtsan() {
    let hash = ContentHash([9; 32]);
    let frames = 48_000u64;
    let mut media = MediaMap::new();
    media.insert(hash, hit(frames as usize, 220.0));
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
    let simpler_track = editor.new_id();
    let rack_track = editor.new_id();
    for (index, track) in [simpler_track, rack_track].into_iter().enumerate() {
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index,
            })
            .unwrap();
    }
    let sampler = |editor: &mut Editor, mode: f64| {
        let mut sampler = Device::new(
            editor.new_id(),
            "Simpler",
            DeviceKind::Native {
                device_type: SAMPLER.to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        sampler.set_media(0, Some(hash));
        sampler.params.insert(MODE, mode);
        sampler
    };
    // Slice mode: four equal slices (Slice By Region, Regions 4).
    let mut slicer = sampler(&mut editor, 2.0);
    slicer.params.insert(SLICE_BY, 2.0);
    slicer.params.insert(SLICE_REGIONS, 4.0);
    let slicer_id = slicer.id;
    editor
        .apply(Command::InsertDevice {
            track: simpler_track,
            index: 0,
            device: slicer,
        })
        .unwrap();
    // The rack Slice to Drum Rack makes: one chain per slice on C3 up, in
    // choke group 1.
    let mut rack = Device::rack(editor.new_id(), RackKind::Drum);
    let bounds = [0u64, 12_000, 24_000, 36_000, 48_000];
    for at in 0..4usize {
        let mut copy = sampler(&mut editor, 1.0);
        copy.region = Some(SampleRegion::Frames {
            start: bounds[at],
            end: bounds[at + 1],
        });
        let mut chain = Chain::for_pad(
            editor.new_id(),
            format!("Slice {}", at + 1),
            60 + u8::try_from(at).unwrap(),
        );
        chain.choke = Some(1);
        chain.devices.push(copy);
        rack.chains.push(chain);
    }
    editor
        .apply(Command::InsertDevice {
            track: rack_track,
            index: 0,
            device: rack,
        })
        .unwrap();
    for track in [simpler_track, rack_track] {
        let clip = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(MidiData::new((0..16u8).map(|n| {
                        Note::new(
                            60 + n % 4,
                            Ticks(Ticks::QUARTER.get() / 4 * i64::from(n)),
                            Ticks(Ticks::QUARTER.get() / 2),
                            127,
                        )
                    }))),
                ),
            })
            .unwrap();
    }

    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    for round in 0..1_000u64 {
        // Playback moves through Mono, Poly and Thru, as a person could
        // change it while the clip plays.
        if round % 50 == 0 {
            editor
                .apply(Command::SetDeviceParam {
                    device: slicer_id,
                    param: PLAYBACK,
                    value: (round / 50 % 3) as f64,
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process(128);
        });
        if round % 100 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert_eq!(state.counters.migration_mismatches, 0);
    eprintln!(
        "1,000 blocks of a Slice-mode Simpler and its Drum Rack under RealtimeSanitizer: \
         {} parameter changes, peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}
