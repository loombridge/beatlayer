//! The instruments' voice section under RealtimeSanitizer (WP-8.2; the
//! instrument voices spec R2): the framework's test instrument plays more
//! overlapping notes than it has voices, so voices are stolen, retriggered
//! and returned to, while every part of the section changes between
//! blocks: Voices, Priority, Mono and Legato, Glide and Glissando, unison
//! copies, the pitch bend range, Transpose, Stretch, Error, Hi-Quality and
//! the set's tuning. RTSan stops the process at the first allocation, free,
//! lock, or blocking call in any of it, and the allocation trap, installed
//! as in every Beatlayer test binary, fails the test at any heap use it
//! sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_devices::testing::{
    self, INSTRUMENT, INSTRUMENT_ERROR, INSTRUMENT_GLIDE_MODE, INSTRUMENT_GLIDE_PROP,
    INSTRUMENT_GLIDE_TIME, INSTRUMENT_HI_QUALITY, INSTRUMENT_LEGATO, INSTRUMENT_PB_RANGE,
    INSTRUMENT_PRESSURE_LEVEL, INSTRUMENT_PRIORITY, INSTRUMENT_RETRIGGER, INSTRUMENT_STRETCH,
    INSTRUMENT_TRANSPOSE, INSTRUMENT_UNISON_DELAY, INSTRUMENT_UNISON_DETUNE,
    INSTRUMENT_UNISON_SPREAD, INSTRUMENT_UNISON_VOICES, INSTRUMENT_VOICE_MODE, INSTRUMENT_VOICES,
};
use bl_engine::{Collector, Compiler, EngineConfig, TransportCommand, channel};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceKind, DeviceRole, Editor, MidiData, Note, ParamId,
    TrackKind, Tuning,
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
fn the_test_instrument_stealing_gliding_and_stacking_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, testing::registry(), collector.handle());
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
    let mut instrument = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: INSTRUMENT.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    // Few voices and eight copies of each, so the notes below steal.
    instrument.params.insert(INSTRUMENT_VOICES, 6.0);
    instrument.params.insert(INSTRUMENT_UNISON_VOICES, 8.0);
    let device = instrument.id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: instrument,
        })
        .unwrap();
    // Forty overlapping notes, each held for a bar, across the keyboard.
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
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let set = |param: ParamId, value: f64| Command::SetDeviceParam {
        device,
        param,
        value,
    };
    for round in 0..2_000u64 {
        let step = (round / 20) as f64;
        let command = match round % 20 {
            1 => Some(set(INSTRUMENT_VOICES, 1.0 + step % 8.0)),
            2 => Some(set(INSTRUMENT_PRIORITY, step % 3.0)),
            3 => Some(set(INSTRUMENT_VOICE_MODE, step % 2.0)),
            4 => Some(set(INSTRUMENT_LEGATO, (step / 2.0).floor() % 2.0)),
            5 => Some(set(INSTRUMENT_GLIDE_TIME, (step % 5.0) * 120.0)),
            6 => Some(set(INSTRUMENT_GLIDE_MODE, (step / 3.0).floor() % 2.0)),
            7 => Some(set(INSTRUMENT_GLIDE_PROP, step % 2.0)),
            8 => Some(set(INSTRUMENT_UNISON_VOICES, 1.0 + step % 8.0)),
            9 => Some(set(INSTRUMENT_UNISON_DETUNE, (step % 4.0) * 25.0)),
            10 => Some(set(INSTRUMENT_UNISON_DELAY, (step % 3.0) * 40.0)),
            11 => Some(set(INSTRUMENT_UNISON_SPREAD, (step % 5.0) * 25.0)),
            12 => Some(set(INSTRUMENT_PB_RANGE, 1.0 + step % 24.0)),
            13 => Some(set(INSTRUMENT_TRANSPOSE, (step % 9.0) - 4.0)),
            14 => Some(set(INSTRUMENT_STRETCH, (step % 5.0) * 50.0 - 100.0)),
            15 => Some(set(INSTRUMENT_ERROR, (step % 3.0) * 50.0)),
            16 => Some(set(INSTRUMENT_RETRIGGER, step % 2.0)),
            17 => Some(set(INSTRUMENT_PRESSURE_LEVEL, (step % 3.0) * 40.0)),
            18 => Some(set(INSTRUMENT_HI_QUALITY, step % 2.0)),
            19 if round % 120 == 19 => Some(Command::SetTuning {
                tuning: nineteen(),
                retune: false,
            }),
            19 if round % 120 == 79 => Some(Command::RemoveTuning { retune: false }),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        // Back into the notes now and then, so they steal and stop mid-sound.
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
