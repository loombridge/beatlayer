//! The framework's test effects device under RealtimeSanitizer (WP-5.42):
//! the effects' shared parts (an LFO that locks to the song's tempo map and
//! position, an envelope follower, and a seeded random source) on a track
//! that plays, while its waveform, rate mode, output, Hold, and menu option
//! change. RTSan stops the process at the first allocation, free, lock, or
//! blocking call, and the allocation trap, installed as in every Beatlayer
//! test binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::SampleRate;
use bl_devices::testing::{
    self, EFFECTS, EFFECTS_DIVISION, EFFECTS_HOLD, EFFECTS_INVERT, EFFECTS_OUTPUT,
    EFFECTS_RATE_MODE, EFFECTS_WAVE,
};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, EngineConfig, TransportCommand, channel};
use bl_model::{Command, Device, DeviceKind, Editor, ParamId, TrackKind};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn the_test_effects_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, testing::registry(), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut tone = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    tone.params.insert(test_tone::LEVEL, -20.0);
    let effects = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: EFFECTS.to_owned(),
        },
    );
    let effects_id = effects.id;
    for (index, device) in [tone, effects].into_iter().enumerate() {
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device,
            })
            .unwrap();
    }
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let set = |param: ParamId, value: f64| Command::SetDeviceParam {
        device: effects_id,
        param,
        value,
    };
    for round in 0..2_000u64 {
        // A new setting every few blocks, so changes land mid-stream.
        let command = match round % 12 {
            0 => Some(set(EFFECTS_WAVE, (round / 12 % 12) as f64)),
            3 => Some(set(EFFECTS_RATE_MODE, (round / 12 % 3) as f64)),
            5 => Some(set(EFFECTS_DIVISION, (round / 12 % 6) as f64)),
            7 => Some(set(EFFECTS_OUTPUT, (round / 12 % 3) as f64)),
            9 => Some(set(EFFECTS_HOLD, (round / 12 % 2) as f64)),
            11 => Some(set(EFFECTS_INVERT, (round / 12 % 2) as f64)),
            _ => None,
        };
        if let Some(command) = command {
            editor.apply(command).unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        assert!(sender.flush());

        bl_rt::rt_test!({
            processor.process(256);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert_eq!(state.counters.events_unrouted, 0);
    assert!(state.counters.events_applied > 100);
    eprintln!(
        "2,000 blocks of the test effects under RealtimeSanitizer: {} parameter changes, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}

/// The three delays (WP-5.25) on a track that plays, while their times,
/// pitch, spray, feedback and modes change mid-stream.
#[test]
fn the_three_delays_pass_under_rtsan() {
    use bl_devices::alignment_delay::{self, ALIGNMENT_DELAY};
    use bl_devices::filter_delay::{self, FILTER_DELAY};
    use bl_devices::grain_delay::{self, GRAIN_DELAY};
    use bl_engine::nodes::TEST_TONE;

    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut registry = bl_engine::Registry::new();
    bl_devices::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut ids = Vec::new();
    for (index, kind) in [TEST_TONE, FILTER_DELAY, GRAIN_DELAY, ALIGNMENT_DELAY]
        .into_iter()
        .enumerate()
    {
        let device = Device::new(
            editor.new_id(),
            "",
            DeviceKind::Native {
                device_type: kind.to_owned(),
            },
        );
        ids.push(device.id);
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device,
            })
            .unwrap();
    }
    let (trident, scatter, plumb) = (ids[1], ids[2], ids[3]);
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    for round in 0..2_000u64 {
        let step = (round / 12) as f64;
        let command = match round % 12 {
            0 => Some((
                trident,
                filter_delay::line_param(0, filter_delay::FREQ),
                200.0 + step * 37.0 % 8_000.0,
            )),
            2 => Some((trident, filter_delay::FEEDBACK, step * 7.0 % 90.0)),
            4 => Some((
                trident,
                filter_delay::line_param(1, filter_delay::SYNC),
                step % 2.0,
            )),
            5 => Some((scatter, grain_delay::PITCH, step * 5.0 % 24.0 - 12.0)),
            7 => Some((scatter, grain_delay::SPRAY, step * 31.0 % 400.0)),
            8 => Some((scatter, grain_delay::FREQUENCY, 5.0 + step * 3.0 % 100.0)),
            9 => Some((plumb, alignment_delay::MODE, step % 3.0)),
            11 => Some((
                plumb,
                alignment_delay::side_param(0, alignment_delay::TIME),
                step * 13.0 % 200.0,
            )),
            _ => None,
        };
        if let Some((device, param, value)) = command {
            editor
                .apply(Command::SetDeviceParam {
                    device,
                    param,
                    value,
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        assert!(sender.flush());

        bl_rt::rt_test!({
            processor.process(256);
        });
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert_eq!(state.counters.events_unrouted, 0);
    assert!(state.counters.events_applied > 100);
    eprintln!(
        "2,000 blocks of the three delays under RealtimeSanitizer: {} parameter changes",
        state.counters.events_applied
    );
}
