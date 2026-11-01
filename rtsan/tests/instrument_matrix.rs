//! The instruments' modulation matrix under RealtimeSanitizer (WP-8.4; the
//! instrument modulation spec R10, R13): the framework's test instrument and
//! Drift play overlapping notes while routes are set and cleared, amounts,
//! choosers, the matrix's Amount and Hi-Quality change, and the engine
//! publishes what the newest voice plays. RTSan stops the process at the
//! first allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_devices::polysynth::{MOD1_AMOUNT, MOD1_DEST, MOD1_SOURCE, POLYSYNTH};
use bl_devices::testing::{
    self, INSTRUMENT, INSTRUMENT_CHOSEN_AMOUNT, INSTRUMENT_CHOSEN_DEST, INSTRUMENT_CHOSEN_SOURCE,
    INSTRUMENT_FREE_AMOUNTS, INSTRUMENT_HI_QUALITY, INSTRUMENT_MATRIX_AMOUNT,
};
use bl_engine::{Collector, Compiler, EngineConfig, TransportCommand, channel};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceKind, DeviceRole, Editor, MidiData, Note, ParamId,
    TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A track holding `device_type`, playing forty overlapping notes: the
/// device's ID.
fn instrument(editor: &mut Editor, device_type: &str) -> bl_model::DeviceId {
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
    let device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: device_type.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    let id = device.id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let notes: Vec<Note> = (0..40u32)
        .map(|n| {
            Note::new(
                (36 + (n * 7) % 48) as u8,
                Ticks(i64::from(n) * Ticks::QUARTER.0 / 4),
                Ticks::WHOLE,
                (30 + (n * 11) % 90) as u8,
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
    id
}

#[test]
fn routes_amounts_and_choosers_changing_during_playback_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut registry = testing::registry();
    bl_devices::polysynth::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let test = instrument(&mut editor, INSTRUMENT);
    let drift = instrument(&mut editor, POLYSYNTH);
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let set = |device, param: ParamId, value: f64| Command::SetDeviceParam {
        device,
        param,
        value,
    };
    let route = |slot: u8, source: &str, destination: &str| Command::SetModRoute {
        device: test,
        slot,
        source: source.to_owned(),
        destination: destination.to_owned(),
    };
    let pairs = [
        ("Velocity", "Note Level"),
        ("Key", "Fine Tune"),
        ("Pressure", "Master"),
        ("Mod Wheel", "Note Level"),
    ];
    let mut published = 0;
    for round in 0..2_000u64 {
        let step = (round / 20) as usize;
        let command = match round % 20 {
            1 => {
                let (source, destination) = pairs[step % 4];
                Some(route((step % 4) as u8, source, destination))
            }
            2 => Some(set(
                test,
                INSTRUMENT_FREE_AMOUNTS[step % 4],
                (step % 5) as f64 * 40.0 - 80.0,
            )),
            3 => Some(Command::ClearModRoute {
                device: test,
                slot: ((step + 2) % 4) as u8,
            }),
            4 => Some(set(test, INSTRUMENT_CHOSEN_SOURCE, (step % 6) as f64)),
            5 => Some(set(test, INSTRUMENT_CHOSEN_DEST, (step % 4) as f64)),
            6 => Some(set(
                test,
                INSTRUMENT_CHOSEN_AMOUNT,
                (step % 3) as f64 * 50.0,
            )),
            7 => Some(set(
                test,
                INSTRUMENT_MATRIX_AMOUNT,
                (step % 4) as f64 * 33.0,
            )),
            8 => Some(set(test, INSTRUMENT_HI_QUALITY, (step % 2) as f64)),
            9 => Some(set(drift, MOD1_SOURCE, 1.0 + (step % 8) as f64)),
            10 => Some(set(drift, MOD1_DEST, 1.0 + (step % 10) as f64)),
            11 => Some(set(drift, MOD1_AMOUNT, (step % 5) as f64 * 40.0 - 80.0)),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        if round % 400 == 399 {
            sender.transport(TransportCommand::Locate(Ticks::ZERO));
        }
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process(128);
        });
        published = published.max(monitor.state().matrix_played.len());
        if round % 50 == 0 {
            collector.collect();
        }
    }
    assert!(published > 0, "the engine published the matrix's movement");
    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert!(state.counters.events_applied > 100);
}
