//! Device chains in the engine (WP-2.4; the device chains spec, R5 to R9):
//! each track's chain runs in signal order before its mixer, a switched-off
//! device passes its input through, and a chain edit makes only the nodes
//! it adds. Every block runs as a real-time section under the allocation
//! trap.

bl_rt::install_trap!();

mod common;

use std::sync::atomic::Ordering::Relaxed;

use bl_engine::testing::{self, GAIN, GAIN_PARAM, THROUGH};
use bl_engine::{DeviceType, Registry};
use bl_model::{Command, DeviceRole, TrackKind};
use common::{Rig, dc, device, track};

const FRAMES: usize = 64;

fn level(rig: &mut Rig) -> f32 {
    rig.block(FRAMES);
    let out = rig.last();
    assert!(out.iter().all(|s| *s == out[0]), "{out:?}");
    out[0]
}

// Spec: device-chains R5
#[test]
fn a_chain_runs_in_signal_order_before_the_mixer() {
    let mut rig = Rig::new(FRAMES);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    let source = dc(&mut rig.editor, audio, 1.0);
    let gain = device(&mut rig.editor, audio, GAIN, &[(GAIN_PARAM, 0.5)]);
    rig.sync();
    assert_eq!(level(&mut rig), 0.5, "DC 1, then halved");

    // The gain first: it halves silence, then DC adds 1.
    rig.apply(Command::MoveDevice {
        device: gain,
        track: audio,
        index: 0,
    });
    assert_eq!(level(&mut rig), 1.0);

    // The mixer comes after the chain: its volume scales the chain's output.
    rig.apply(Command::MoveDevice {
        device: source,
        track: audio,
        index: 0,
    });
    rig.apply(Command::SetVolume {
        track: audio,
        volume: 0.5,
    });
    // The volume glides over 20 ms; run past it.
    rig.render(48_000 / 10, FRAMES);
    assert_eq!(level(&mut rig), 0.25);
}

// Spec: device-chains R6
#[test]
fn a_switched_off_device_passes_its_input_through() {
    let mut rig = Rig::new(FRAMES);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 0.75);
    let mute = device(&mut rig.editor, audio, GAIN, &[(GAIN_PARAM, 0.0)]);
    rig.sync();
    assert_eq!(level(&mut rig), 0.0);
    rig.apply(Command::SetDeviceEnabled {
        device: mute,
        enabled: false,
    });
    assert_eq!(level(&mut rig), 0.75, "the gain's input, untouched");
    // It takes parameter changes while it's off, and comes back with them.
    rig.apply(Command::SetDeviceParam {
        device: mute,
        param: GAIN_PARAM,
        value: 2.0,
    });
    assert_eq!(level(&mut rig), 0.75);
    rig.apply(Command::SetDeviceEnabled {
        device: mute,
        enabled: true,
    });
    assert_eq!(level(&mut rig), 1.5);
}

// Spec: device-chains R7, R8
#[test]
fn a_chain_edit_makes_only_the_nodes_it_adds() {
    let mut rig = Rig::new(FRAMES);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    let first = device(&mut rig.editor, audio, THROUGH, &[]);
    let second = device(&mut rig.editor, audio, GAIN, &[]);
    rig.sync();
    rig.block(FRAMES);
    assert_eq!(rig_made(&rig), 2);

    // Edits that leave the chain as it is send no program.
    for command in [
        Command::RenameDevice {
            device: second,
            name: "Trim".to_owned(),
        },
        Command::SetShowPresetName {
            device: second,
            shown: false,
        },
        Command::SetDeviceParam {
            device: second,
            param: GAIN_PARAM,
            value: 0.5,
        },
        Command::SetDeviceState {
            device: second,
            params: [(GAIN_PARAM, 0.25)].into_iter().collect(),
            name: "Quiet".to_owned(),
            shape: None,
        },
    ] {
        let synced = rig.apply(command.clone());
        assert!(!synced.program, "{command:?} sent a program");
        rig.block(FRAMES);
    }

    // Inserting makes one node; every other node migrates.
    let third = device(&mut rig.editor, audio, THROUGH, &[]);
    assert!(rig.sync().program);
    rig.block(FRAMES);
    assert_eq!(rig_made(&rig), 3);

    // Moving, switching off and on, and removing make none.
    for command in [
        Command::MoveDevice {
            device: third,
            track: audio,
            index: 0,
        },
        Command::SetDeviceEnabled {
            device: first,
            enabled: false,
        },
        Command::SetDeviceEnabled {
            device: first,
            enabled: true,
        },
        Command::RemoveDevice { device: first },
    ] {
        assert!(rig.apply(command).program);
        rig.block(FRAMES);
    }
    assert_eq!(rig_made(&rig), 3);
    rig.collector.collect();
    assert_eq!(
        rig.probe.dropped.load(Relaxed),
        1,
        "only the removed node goes"
    );
}

fn rig_made(rig: &Rig) -> u64 {
    rig.probe.made.load(Relaxed)
}

// Spec: device-chains R4
#[test]
fn a_device_moved_to_another_track_keeps_its_node() {
    let mut rig = Rig::new(FRAMES);
    let one = track(&mut rig.editor, TrackKind::Audio, None);
    let two = track(&mut rig.editor, TrackKind::Audio, None);
    let source = dc(&mut rig.editor, one, 0.5);
    rig.sync();
    rig.block(FRAMES);
    rig.apply(Command::MoveDevice {
        device: source,
        track: two,
        index: 0,
    });
    assert_eq!(level(&mut rig), 0.5);
    assert_eq!(rig.probe.made.load(Relaxed), 1);
}

// Spec: device-chains R9
#[test]
fn factories_offer_device_types_with_their_roles() {
    let probe = std::sync::Arc::new(testing::Probe::default());
    let registry = testing::registry(&probe);
    let types = registry.device_types();
    assert_eq!(
        types,
        vec![
            testing::gain_type(),
            testing::described_gain_type(),
            testing::through_type(),
            testing::key_type(),
            testing::hear_type(),
            testing::instrument_type(),
        ]
    );
    // A type that declares a sidechain input makes devices that have one
    // (WP-2.19b).
    let key = testing::key_type().device(bl_model::DeviceId(bl_core::id::Id::new(8).unwrap()));
    assert_eq!(key.sidechain, Some(bl_model::Sidechain::default()));
    assert!(!testing::gain_type().sidechain);
    // And one that declares a MIDI sidechain input, devices that have one
    // (WP-5.27).
    let hear = testing::hear_type().device(bl_model::DeviceId(bl_core::id::Id::new(7).unwrap()));
    assert_eq!(
        hear.midi_sidechain,
        Some(bl_model::MidiSidechain::default())
    );
    assert_eq!(hear.sidechain, None);
    assert_eq!(key.midi_sidechain, None);
    let gain = registry.device_type(&testing::gain_type().kind).unwrap();
    let device = gain.device(bl_model::DeviceId(bl_core::id::Id::new(9).unwrap()));
    assert_eq!(
        (device.name.as_str(), device.role),
        ("Test Gain", DeviceRole::AudioEffect)
    );
    assert_eq!(device.sidechain, None);
    // The engine's own registry offers no device: Beatlayer ships none
    // until WP-2.5.
    assert!(Registry::new().device_types().is_empty());
    assert_eq!(Registry::new().device_type(&gain.kind), None::<DeviceType>);
}

// Spec: multisampler R31
#[test]
fn a_zone_edit_sends_data_and_no_program() {
    let mut rig = Rig::new(FRAMES);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    let sampler = device(&mut rig.editor, audio, THROUGH, &[]);
    rig.sync();
    rig.block(FRAMES);
    let layer = bl_model::Layer::new(
        bl_model::LayerId(1),
        "Kick",
        bl_model::ContentHash([1; 32]),
        48_000,
        48_000,
        1,
    );
    // Adding a layer, moving its zone and changing its settings each send
    // the node its layers as data; none makes a program or a node.
    for command in [
        Command::InsertLayers {
            device: sampler,
            index: None,
            layers: vec![layer],
        },
        Command::SetLayerZones {
            device: sampler,
            kind: bl_model::ZoneKind::Key,
            zones: vec![bl_model::LayerZone {
                layer: bl_model::LayerId(1),
                zone: bl_model::Zone::new(36, 48),
            }],
        },
        Command::UpdateLayers {
            device: sampler,
            layers: vec![bl_model::LayerId(1)],
            settings: bl_model::LayerSettings {
                root_key: Some(40),
                ..Default::default()
            },
        },
    ] {
        let synced = rig.apply(command.clone());
        assert!(!synced.program, "{command:?} sent a program");
        assert_eq!(synced.data, 1, "{command:?} sent the layers");
        rig.block(FRAMES);
    }
    // An edit of something else sends none.
    let synced = rig.apply(Command::RenameDevice {
        device: sampler,
        name: "Drums".to_owned(),
    });
    assert_eq!((synced.program, synced.data), (false, 0));
    assert_eq!(rig_made(&rig), 1, "no node was made for the edits");
}

/// A media library over a store that counts what a sampler asks of it.
struct Counting {
    store: std::sync::Arc<bl_media::MediaStore>,
    whole: std::sync::atomic::AtomicUsize,
    progressive: std::sync::atomic::AtomicUsize,
}

impl bl_engine::MediaLibrary for Counting {
    fn open(
        &self,
        media: bl_model::ContentHash,
        rate: bl_core::time::SampleRate,
        start: u64,
    ) -> Option<bl_engine::OpenedSource> {
        bl_engine::MediaLibrary::open(&*self.store, media, rate, start)
    }

    fn sample(
        &self,
        media: bl_model::ContentHash,
        rate: bl_core::time::SampleRate,
    ) -> Option<std::sync::Arc<bl_media::SampleData>> {
        self.whole.fetch_add(1, Relaxed);
        bl_engine::MediaLibrary::sample(&*self.store, media, rate)
    }

    fn progressive(
        &self,
        media: bl_model::ContentHash,
        rate: bl_core::time::SampleRate,
        head: usize,
    ) -> Option<std::sync::Arc<bl_media::ProgressiveSample>> {
        self.progressive.fetch_add(1, Relaxed);
        bl_engine::MediaLibrary::progressive(&*self.store, media, rate, head)
    }
}

// Spec: multisampler R23
#[test]
fn a_long_layer_is_read_in_the_background_with_ram_mode_off() {
    let dir = std::env::temp_dir().join(format!("bl-engine-long-layer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Long.wav");
    let samples = vec![0.25f32; 12 * 48_000];
    std::fs::write(
        &path,
        bl_media::wav_bytes(
            &samples,
            1,
            48_000,
            bl_media::BitDepth::Int16,
            bl_media::Dither::None,
        )
        .unwrap(),
    )
    .unwrap();
    let store = std::sync::Arc::new(bl_media::MediaStore::new());
    let hash = store.import(&path).unwrap();
    let media = bl_model::ContentHash(*hash.as_bytes());
    let library = std::sync::Arc::new(Counting {
        store,
        whole: Default::default(),
        progressive: Default::default(),
    });
    let mut rig = Rig::with_registry(FRAMES, |registry| registry.set_media(library.clone()));
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    let sampler = device(&mut rig.editor, audio, THROUGH, &[]);
    rig.sync();
    let layer = bl_model::Layer::new(bl_model::LayerId(1), "Long", media, 12 * 48_000, 48_000, 1);
    let insert = Command::InsertLayers {
        device: sampler,
        index: None,
        layers: vec![layer],
    };
    rig.apply(insert);
    assert_eq!(
        library.progressive.load(Relaxed),
        1,
        "a layer over ten seconds is read behind its start"
    );
    assert_eq!(library.whole.load(Relaxed), 0);
    // An edit that sends the layers again shares what's being read.
    rig.apply(Command::SetLayerZones {
        device: sampler,
        kind: bl_model::ZoneKind::Key,
        zones: vec![bl_model::LayerZone {
            layer: bl_model::LayerId(1),
            zone: bl_model::Zone::new(10, 20),
        }],
    });
    assert_eq!(library.progressive.load(Relaxed), 1, "not read again");
    // RAM Mode decodes it whole.
    rig.apply(Command::SetLayerOptions {
        device: sampler,
        ram_mode: Some(true),
        snap: None,
        linear_loop_fades: None,
    });
    assert_eq!(library.progressive.load(Relaxed), 1);
    assert_eq!(library.whole.load(Relaxed), 1, "RAM Mode on holds it whole");
    std::fs::remove_dir_all(&dir).ok();
}
