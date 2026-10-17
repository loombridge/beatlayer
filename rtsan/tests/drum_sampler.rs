//! The eight-slot drum sampler's audio-thread work under
//! RealtimeSanitizer (WP-5.17): a MIDI clip plays a note on every slot, at
//! several velocities and overlapping, through slots that stretch in both
//! modes, filter, saturate, pan at random and decay in Gate mode, while the
//! link cuts slot 7 and slot 8 against each other and parameters change
//! between blocks. RTSan stops the process at the first allocation, free,
//! lock, or blocking call in any of it, and the allocation trap, installed as
//! in every Beatlayer test binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::{SampleRate, Ticks};
use bl_devices::kitbag::{KEYS, KITBAG, LINK, SLOTS, TIME, param, slot};
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

/// A mono sample of `frames` frames at 48 kHz: a quiet sine of its own pitch.
fn hit(frames: usize, hz: f64) -> Arc<SampleData> {
    let samples = (0..frames)
        .map(|i| (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin() as f32 * 0.01)
        .collect();
    Arc::new(SampleData::new(48_000, vec![samples]).unwrap())
}

#[test]
fn the_drum_samplers_notes_on_every_slot_pass_under_rtsan() {
    let mut media = MediaMap::new();
    let mut editor = Editor::new();
    let id: bl_model::DeviceId = editor.new_id();
    let mut device = Device::new(
        id,
        "Kitbag",
        DeviceKind::Native {
            device_type: KITBAG.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    for s in 0..SLOTS {
        let hash = ContentHash([s as u8 + 1; 32]);
        media.insert(hash, hit(9_000 + 1_500 * s, 150.0 + 70.0 * s as f64));
        device.set_media(s, Some(hash));
        // Every slot does something different with its sample.
        let set = |device: &mut Device, local, value: f64| {
            device.params.insert(param(s, local), value);
        };
        set(
            &mut device,
            slot::STRETCH,
            if s % 2 == 0 { 60.0 } else { -50.0 },
        );
        set(&mut device, slot::STRETCH_MODE, (s / 2 % 2) as f64);
        set(&mut device, slot::FILTER, (s % 3 != 2) as u8 as f64);
        set(&mut device, slot::FILTER_TYPE, (s % 8) as f64);
        set(&mut device, slot::SATURATOR, (s % 2) as f64);
        set(&mut device, slot::DRIVE, 70.0);
        set(&mut device, slot::PAN_RANDOM, 100.0);
        set(&mut device, slot::TRANSPOSE_RANDOM, 15.0);
        set(&mut device, slot::TRANSPOSE_VELOCITY, 30.0);
        set(&mut device, slot::FILTER_RANDOM, 40.0);
        set(&mut device, slot::DECAY, 400.0);
        set(&mut device, slot::DECAY_MODE, (s % 3 == 0) as u8 as f64);
    }
    device.params.insert(LINK, 1.0);

    let registry = {
        let mut registry = Registry::new().with_media(Arc::new(media));
        bl_devices::register(&mut registry);
        registry
    };
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

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
    // A note on every slot's key, a sixteenth apart, three times over at
    // different velocities, long enough to overlap and retrigger.
    let sixteenth = Ticks::QUARTER.0 / 4;
    let notes: Vec<Note> = (0..3 * SLOTS as i64)
        .map(|n| {
            Note::new(
                KEYS[(n % SLOTS as i64) as usize],
                Ticks(n * sixteenth),
                Ticks(sixteenth * 3),
                30 + (n * 11 % 98) as u8,
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
    assert!(update.missing.is_empty(), "the samples are found");
    sender.send_update(update);
    sender.transport(TransportCommand::Play);

    for round in 0..1_500u64 {
        // Parameters change between blocks: a pan, a time, a stretch, a
        // filter cutoff, and the link, as automation or a hand would.
        let command = match round % 25 {
            3 => Some(Command::SetDeviceParam {
                device: id,
                param: param((round % 8) as usize, slot::PAN),
                value: (round % 100) as f64 - 50.0,
            }),
            9 => Some(Command::SetDeviceParam {
                device: id,
                param: TIME,
                value: 50.0 + (round % 300) as f64,
            }),
            15 => Some(Command::SetDeviceParam {
                device: id,
                param: param((round % 8) as usize, slot::STRETCH),
                value: (round % 200) as f64 - 100.0,
            }),
            21 => Some(Command::SetDeviceParam {
                device: id,
                param: param((round % 8) as usize, slot::FILTER_FREQ),
                value: 200.0 + (round % 90) as f64 * 100.0,
            }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        // Jump back into the pattern now and then, so notes retrigger and
        // stop mid-sound.
        if round % 300 == 150 {
            sender.transport(TransportCommand::Locate(Ticks(sixteenth * 5)));
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
