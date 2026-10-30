//! CV Instrument and CV Triggers on the audio thread under
//! RealtimeSanitizer (WP-7.17; the CV tools spec, R8): a MIDI track plays a
//! phrase through each, their CV outputs go to the interface's channels, the
//! interface's inputs come back as the instrument's sound, a calibration runs
//! against it, and the devices are switched off, removed and put back while
//! it plays. RTSan stops the process at the first allocation, free, lock, or
//! blocking call on the audio thread, and the allocation trap fails the test
//! at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_devices::{CvCommand, cv_instrument_type, cv_triggers_type};
use bl_engine::testing;
use bl_engine::{
    Collector, Compiler, DeviceInput, EngineConfig, NodeKey, TransportCommand, channel,
};
use bl_model::{
    Channel, Clip, ClipData, Command, DeviceId, Editor, HardwareLatency, MidiData, Note, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// An interface whose input channel 4 holds a sine, the oscillator's sound.
struct Interface(Vec<Vec<f32>>);

impl DeviceInput for Interface {
    fn channels(&self) -> usize {
        self.0.len()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.0[index]
    }
}

#[test]
fn cv_instrument_and_cv_triggers_pass_under_rtsan() {
    let mut config = EngineConfig::new(SampleRate::HZ_48000, 256);
    config.input_channels = 4;
    config.output_channels = 4;
    config.round_trip_latency = 480;
    let mut collector = Collector::new();
    let mut registry = testing::registry(&std::sync::Arc::new(testing::Probe::default()));
    bl_devices::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor) = channel(&config);

    let mut editor = Editor::new();
    let add = |editor: &mut Editor, index: usize, key: u8| {
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index,
            })
            .unwrap();
        let clip = editor.new_id();
        let notes: Vec<Note> = (0..24)
            .map(|n| {
                Note::new(
                    key + (n % 5) as u8,
                    Ticks(Ticks::QUARTER.0 * n / 3),
                    Ticks(Ticks::QUARTER.0 * 2 / 3),
                    60 + (n * 7 % 60) as u8,
                )
            })
            .collect();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE * 8,
                    ClipData::Midi(MidiData::new(notes)),
                ),
            })
            .unwrap();
        track
    };
    let keys = add(&mut editor, 0, 48);
    let drums = add(&mut editor, 1, 36);
    let instrument = editor.new_id();
    let mut device = cv_instrument_type().device(instrument);
    let external = device.external.as_mut().unwrap();
    for (port, channel) in external.cv_outputs.iter_mut().zip([1, 2, 3, 4]) {
        port.channel = Some(channel);
    }
    external.audio_from = Channel::Mono(4);
    external.hardware_latency = HardwareLatency::Samples { value: 100 };
    editor
        .apply(Command::InsertDevice {
            track: keys,
            index: 0,
            device,
        })
        .unwrap();
    let triggers = editor.new_id();
    let mut device = cv_triggers_type().device(triggers);
    let external = device.external.as_mut().unwrap();
    for (port, channel) in external.cv_outputs.iter_mut().zip([1, 2, 3, 4, 1, 2, 3, 4]) {
        port.channel = Some(channel);
    }
    editor
        .apply(Command::InsertDevice {
            track: drums,
            index: 0,
            device,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    assert!(sender.flush());
    sender.transport(TransportCommand::Play);

    let mut phase = 0.0f32;
    let sine: Vec<f32> = (0..256)
        .map(|_| {
            phase = (phase + 440.0 / 48_000.0).fract();
            (phase * std::f32::consts::TAU).sin() * 0.5
        })
        .collect();
    let heard = Interface(
        (0..4)
            .map(|c| if c == 3 { sine.clone() } else { vec![0.0; 256] })
            .collect(),
    );
    let switch = |editor: &mut Editor, device: DeviceId, enabled: bool| {
        editor
            .apply(Command::SetDeviceEnabled { device, enabled })
            .unwrap();
    };
    for round in 0..1_600u32 {
        match round {
            // A calibration runs against the sound for eight steps.
            100 => {
                sender.send_data(
                    NodeKey::Device(instrument),
                    collector.handle().data(CvCommand::Calibrate),
                );
            }
            // Switched off, on again, and removed while they play.
            900 | 1100 => {
                switch(&mut editor, instrument, round == 1100);
                sender.send_update(compiler.sync(editor.document()));
                assert!(sender.flush());
            }
            1300 => {
                switch(&mut editor, triggers, false);
                editor
                    .apply(Command::RemoveDevice { device: triggers })
                    .unwrap();
                sender.send_update(compiler.sync(editor.document()));
                assert!(sender.flush());
            }
            _ => {}
        }
        bl_rt::rt_test!({
            processor.process_with_input(256, &heard);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }
}
