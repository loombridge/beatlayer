//! Analog under RealtimeSanitizer (WP-8.15; the Analog spec, R30): forty
//! overlapping notes play through both LFOs, the vibrato, unison with its
//! Delay, glide, the keyboard, voices and priority and the MPE routings while
//! every one of those parameters and Quick Routing's settings change. RTSan
//! stops the process at the first allocation, free, lock, or blocking call
//! on the audio thread, and the allocation trap, installed as in every
//! Beatlayer test binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_devices::analog::{self, ANALOG, DESTINATIONS, MPE, amp, filter, lfo, noise, osc};
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

/// A track holding an Analog, playing forty overlapping notes: the device's
/// ID.
fn instrument(editor: &mut Editor) -> bl_model::DeviceId {
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
            device_type: ANALOG.to_owned(),
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
fn the_lfos_vibrato_unison_glide_voices_and_mpe_routings_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 128);
    let mut collector = Collector::new();
    let mut registry = bl_devices::testing::registry();
    analog::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let device = instrument(&mut editor);
    for (param, value) in [
        (lfo::id(0, lfo::ON), 1.0),
        (lfo::id(1, lfo::ON), 1.0),
        (osc::id(0, osc::PITCH_MOD), 30.0),
        (filter::id(0, filter::FREQ_LFO), 40.0),
        (analog::VIB_ON, 1.0),
        (analog::VIB_AMOUNT, 40.0),
        (analog::UNI_ON, 1.0),
        (analog::UNI_VOICES, 1.0),
        (analog::UNI_DETUNE, 50.0),
        (analog::UNI_DELAY, 80.0),
        (analog::GLIDE_ON, 1.0),
        (analog::GLIDE_TIME, 80.0),
        (analog::MPE_ON, 1.0),
        (MPE[0].1, 60.0),
        (MPE[2].1, -60.0),
    ] {
        editor
            .apply(Command::SetDeviceParam {
                device,
                param,
                value,
            })
            .unwrap();
    }
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let set = |param: ParamId, value: f64| Command::SetDeviceParam {
        device,
        param,
        value,
    };
    let mut published = 0;
    for round in 0..2_000u64 {
        let step = (round / 20) as usize;
        let command = match round % 24 {
            1 => Some(set(lfo::id(step % 2, lfo::WAVE), (step % 5) as f64)),
            2 => Some(set(lfo::id(step % 2, lfo::RATE), 0.5 + (step % 7) as f64)),
            3 => Some(set(lfo::id(step % 2, lfo::SYNC), (step % 2) as f64)),
            4 => Some(set(lfo::id(step % 2, lfo::DIVISION), (step % 10) as f64)),
            5 => Some(set(lfo::id(step % 2, lfo::RETRIG), (step % 2) as f64)),
            6 => Some(set(analog::VIB_ERROR, (step % 4) as f64 * 30.0)),
            7 => Some(set(analog::UNI_VOICES, (step % 2) as f64)),
            8 => Some(set(analog::UNI_DELAY, (step % 5) as f64 * 25.0)),
            9 => Some(set(analog::VOICE_COUNT, 1.0 + (step % 12) as f64 * 3.0)),
            10 => Some(set(analog::PRIORITY, (step % 3) as f64)),
            11 => Some(set(analog::GLIDE_LEGATO, (step % 2) as f64)),
            12 => Some(set(analog::GLIDE_MODE, (step % 2) as f64)),
            13 => Some(set(analog::OCTAVE, (step % 3) as f64 - 1.0)),
            14 => Some(set(analog::ERROR, (step % 4) as f64 * 30.0)),
            15 => Some(set(
                MPE[step % 4].0,
                1.0 + ((step * 3) % DESTINATIONS.len()) as f64,
            )),
            16 => Some(set(MPE[step % 4].1, (step % 5) as f64 * 40.0 - 80.0)),
            17 => Some(set(analog::MPE_ON, (step % 3 != 0) as u8 as f64)),
            18 => Some(set(analog::NOTE_PB, (step % 5) as f64 * 12.0)),
            19 => Some(set(
                osc::id(step % 2, osc::BALANCE),
                (step % 5) as f64 * 25.0,
            )),
            20 => Some(set(noise::id(noise::ON), (step % 2) as f64)),
            21 => Some(set(
                amp::id(step % 2, amp::ON),
                (step % 3 != 0) as u8 as f64,
            )),
            22 => Some(set(filter::id(1, filter::ON), (step % 2) as f64)),
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
