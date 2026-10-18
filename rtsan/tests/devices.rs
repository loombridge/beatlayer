//! The framework's test effects device, Sweep and Stutter under
//! RealtimeSanitizer (WP-5.42, WP-5.20, WP-5.26). The test effects device runs the effects' shared
//! parts (an LFO that locks to the song's tempo map and position, an
//! envelope follower, and a seeded random source) on a track that plays,
//! while its waveform, rate mode, output, Hold, and menu option change.
//! Sweep runs every filter type, the oversampled path, the LFO's waves,
//! rate modes and quantiser, and Clip while they change. Stutter runs its
//! intervals, grids, Chance, Variation and its modes, output modes, filter
//! and Repeat button while they change. RTSan stops the process at the first
//! allocation, free, lock, or blocking call, and the allocation trap, installed as in every Beatlayer
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

#[test]
fn sweep_passes_under_rtsan() {
    use bl_devices::auto_filter::{
        AUTO_FILTER, CLIP, ENV_AMOUNT, HI_QUALITY, LFO_AMOUNT, LFO_DIVISION, LFO_MODE,
        LFO_QUANTIZE, LFO_WAVE, TYPE,
    };

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
    let mut sweep = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: AUTO_FILTER.to_owned(),
        },
    );
    sweep.params.insert(LFO_AMOUNT, 60.0);
    sweep.params.insert(ENV_AMOUNT, 40.0);
    let sweep_id = sweep.id;
    for (index, device) in [tone, sweep].into_iter().enumerate() {
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
        device: sweep_id,
        param,
        value,
    };
    for round in 0..2_000u64 {
        // A new setting every few blocks, so changes land mid-stream: the
        // type (every filter and the comb, resampler and vowel among them),
        // the oversampled path, the LFO's wave, rate mode and quantiser, and
        // Clip.
        let step = round / 12;
        let command = match round % 12 {
            0 => Some(set(TYPE, (step % 10) as f64)),
            2 => Some(set(HI_QUALITY, (step % 2) as f64)),
            4 => Some(set(LFO_WAVE, (step % 8) as f64)),
            6 => Some(set(LFO_MODE, (step % 4) as f64)),
            8 => Some(set(LFO_QUANTIZE, (step % 3) as f64)),
            9 => Some(set(LFO_DIVISION, (step % 17) as f64)),
            10 => Some(set(CLIP, (step % 2) as f64)),
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
        "2,000 blocks of Sweep under RealtimeSanitizer: {} parameter changes, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}

#[test]
fn stutter_passes_under_rtsan() {
    use bl_devices::beat_repeat::{
        BEAT_REPEAT, CHANCE, FILTER, GRID, INTERVAL, MODE, PITCH, REPEAT, VARIATION, VARIATION_MODE,
    };

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
    let mut stutter = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: BEAT_REPEAT.to_owned(),
        },
    );
    stutter.params.insert(INTERVAL, 1.0);
    stutter.params.insert(VARIATION, 70.0);
    stutter.params.insert(PITCH, 7.0);
    stutter.params.insert(FILTER, 1.0);
    let stutter_id = stutter.id;
    for (index, device) in [tone, stutter].into_iter().enumerate() {
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
        device: stutter_id,
        param,
        value,
    };
    for round in 0..2_000u64 {
        // A new setting every few blocks, so changes land mid-stream: the
        // interval, the grid, Chance, Variation and its mode, the output
        // mode, and the Repeat button.
        let step = round / 12;
        let command = match round % 12 {
            0 => Some(set(INTERVAL, (step % 8) as f64)),
            2 => Some(set(GRID, (step % 7) as f64)),
            4 => Some(set(CHANCE, ((step * 37) % 101) as f64)),
            5 => Some(set(VARIATION_MODE, (step % 5) as f64)),
            7 => Some(set(MODE, (step % 3) as f64)),
            9 => Some(set(REPEAT, (step % 2) as f64)),
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
        "2,000 blocks of Stutter under RealtimeSanitizer: {} parameter changes, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}
