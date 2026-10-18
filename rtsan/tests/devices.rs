//! The framework's test effects device under RealtimeSanitizer (WP-5.42):
//! the effects' shared parts (an LFO that locks to the song's tempo map and
//! position, an envelope follower, and a seeded random source) on a track
//! that plays, while its waveform, rate mode, output, Hold, and menu option
//! change. RTSan stops the process at the first allocation, free, lock, or
//! blocking call, and the allocation trap, installed as in every Beatlayer
//! test binary, fails the test at any heap use it sees.
//!
//! The lo-fi effects (WP-5.24: Degrade, Crush and Wax) run after a test tone
//! with every parameter changing the same way.

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

/// Runs `device_type` after a test tone for 2,000 blocks of 256 frames under
/// RTSan, changing a parameter every few blocks to the next value of its
/// list.
fn run_lo_fi_device(device_type: &str, changes: &[(ParamId, &[f64])]) {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut registry = testing::registry();
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
    let mut tone = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    tone.params.insert(test_tone::LEVEL, -20.0);
    let effect = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: device_type.to_owned(),
        },
    );
    let effect_id = effect.id;
    for (index, device) in [tone, effect].into_iter().enumerate() {
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

    for round in 0..2_000u64 {
        // A new setting every few blocks, so changes land mid-stream.
        if round % 4 == 0 {
            let (param, values) = changes[(round / 4) as usize % changes.len()];
            let value = values[(round / 4 / changes.len() as u64) as usize % values.len()];
            editor
                .apply(Command::SetDeviceParam {
                    device: effect_id,
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
        "2,000 blocks of {device_type} under RealtimeSanitizer: {} parameter changes, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}

#[test]
fn degrade_passes_under_rtsan() {
    use bl_devices::degrade::*;
    run_lo_fi_device(
        DEGRADE,
        &[
            (FREQ, &[40.0, 1_000.0, 12_000.0]),
            (AMOUNT, &[0.0, 50.0, 100.0]),
            (NOISE_BLEND, &[0.0, 60.0, 100.0]),
            (FILTER_WIDTH, &[0.0, 50.0, 100.0]),
            (STEREO_WIDTH, &[0.0, 100.0]),
        ],
    );
}

#[test]
fn crush_passes_under_rtsan() {
    use bl_devices::crusher::*;
    run_lo_fi_device(
        CRUSHER,
        &[
            (RATE, &[100.0, 6_000.0, 48_000.0]),
            (JITTER, &[0.0, 100.0]),
            (PRE, &[0.0, 1.0]),
            (POST, &[0.0, 1.0]),
            (POST_OCTAVE, &[-4.0, 0.0, 4.0]),
            (BITS, &[2.0, 8.0, 16.0]),
            (SHAPE, &[0.0, 100.0]),
            (DC_SHIFT, &[-50.0, 0.0, 50.0]),
            (DRY_WET, &[0.0, 100.0]),
        ],
    );
}

#[test]
fn wax_passes_under_rtsan() {
    use bl_devices::wax::*;
    run_lo_fi_device(
        WAX,
        &[
            (TRACING_DRIVE, &[0.0, 100.0]),
            (TRACING_FREQ, &[20.0, 3_000.0, 20_000.0]),
            (PINCH_DRIVE, &[0.0, 100.0]),
            (PINCH_WIDTH, &[0.25, 4.0]),
            (DRIVE, &[0.0, 100.0]),
            (SOFT_HARD, &[0.0, 1.0]),
            (PINCH_MODE, &[0.0, 1.0]),
            (CRACKLE_DENSITY, &[0.0, 250.0]),
            (CRACKLE_VOLUME, &[0.0, 1.0]),
        ],
    );
}
