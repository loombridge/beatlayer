//! The instruments' modulation matrix under RealtimeSanitizer (WP-8.4; the
//! instrument modulation spec R10, R13): the framework's test instrument and
//! Drift play overlapping notes while routes are set and cleared, amounts,
//! choosers, the matrix's Amount and Hi-Quality change, Drift's voice mode,
//! drift, glide, Legato and cycling envelope change, and the engine
//! publishes what the newest voice plays. RTSan stops the process at the
//! first allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_devices::polysynth::{
    CYC_RATE, DRIFT, ENV2_MODE, GLIDE, HI_QUALITY, LEGATO, MOD1_AMOUNT, MOD1_DEST, MOD1_SOURCE,
    MOD2_DEST, POLYSYNTH, VOICE_MODE,
};
use bl_devices::testing::{
    self, INSTRUMENT, INSTRUMENT_CHOSEN_AMOUNT, INSTRUMENT_CHOSEN_DEST, INSTRUMENT_CHOSEN_SOURCE,
    INSTRUMENT_FREE_AMOUNTS, INSTRUMENT_HI_QUALITY, INSTRUMENT_MATRIX_AMOUNT,
};
use bl_devices::wavetable::{
    self, HI_QUALITY, LFO_RETRIGGER, LFO_SYNC, MATRIX_AMOUNT, MATRIX_TIME, UNISON_MODE, WAVETABLE,
    envelope, lfo, slot_amount,
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
            // Drift's voice modes, its drift, glide, Legato, Envelope 2's
            // cycle and its rate as a destination, and Hi-Quality.
            12 => Some(set(drift, VOICE_MODE, (step % 4) as f64)),
            13 => Some(set(drift, DRIFT, (step % 3) as f64 * 50.0)),
            14 => Some(set(drift, LEGATO, (step % 2) as f64)),
            15 => Some(set(drift, GLIDE, (step % 3) as f64 * 200.0)),
            16 => Some(set(drift, ENV2_MODE, (step % 2) as f64)),
            17 => Some(set(drift, CYC_RATE, 1.0 + (step % 5) as f64 * 4.0)),
            18 => Some(set(drift, MOD2_DEST, 10.0 + (step % 2) as f64)),
            19 => Some(set(drift, HI_QUALITY, (step % 2) as f64)),
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

// Spec: wavetable R33
#[test]
fn wavetable_routes_modulators_unison_modes_and_hi_quality_changing_during_playback_pass_under_rtsan()
 {
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut registry = testing::registry();
    wavetable::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let device = instrument(&mut editor, WAVETABLE);
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let set = |param: ParamId, value: f64| Command::SetDeviceParam {
        device,
        param,
        value,
    };
    let sources = [
        "Amp",
        "Env 2",
        "Env 3",
        "LFO 1",
        "LFO 2",
        "Velocity",
        "Note",
        "Pitch Bend",
        "Aftertouch",
        "Mod Wheel",
        "Random",
        "Pressure",
        "Slide",
    ];
    let destinations = [
        "Osc 1 Position",
        "Osc 2 Gain",
        "Osc 1 Detune",
        "Filter 1 Freq",
        "Sub Tone",
        "Env 2 Release",
        "LFO 2 Rate",
        "Unison Amount",
        "Volume",
        "Matrix Time",
        "Matrix Amount",
    ];
    for round in 0..2_000u64 {
        let step = (round / 20) as usize;
        let command = match round % 20 {
            1 => Some(Command::SetModRoute {
                device,
                slot: (step % 64) as u8,
                source: sources[step % sources.len()].to_owned(),
                destination: destinations[step % destinations.len()].to_owned(),
            }),
            2 => Some(set(slot_amount(step % 64), (step % 5) as f64 * 40.0 - 80.0)),
            3 => Some(Command::ClearModRoute {
                device,
                slot: ((step + 7) % 64) as u8,
            }),
            4 => Some(set(UNISON_MODE, (step % 7) as f64)),
            5 => Some(set(HI_QUALITY, (step % 2) as f64)),
            6 => Some(set(lfo(1, LFO_SYNC), (step % 2) as f64)),
            7 => Some(set(lfo(2, LFO_RETRIGGER), (step % 2) as f64)),
            8 => Some(set(envelope(1, wavetable::ENV_MODE), (step % 3) as f64)),
            9 => Some(set(envelope(0, wavetable::ENV_MODE), (step % 3) as f64)),
            10 => Some(set(MATRIX_TIME, (step % 5) as f64 * 40.0 - 80.0)),
            11 => Some(set(MATRIX_AMOUNT, (step % 4) as f64 * 33.0)),
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
        if round % 50 == 0 {
            collector.collect();
        }
    }
    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert!(state.counters.events_applied > 100);
}
