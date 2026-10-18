//! The framework's test effects device and Sweep under RealtimeSanitizer
//! (WP-5.42, WP-5.20, WP-5.21). The test effects device runs the effects' shared
//! parts (an LFO that locks to the song's tempo map and position, an
//! envelope follower, and a seeded random source) on a track that plays,
//! while its waveform, rate mode, output, Hold, and menu option change.
//! Sweep runs every filter type, the oversampled path, the LFO's waves,
//! rate modes and quantiser, and Clip while they change. Strip, Triad,
//! Shutter and Bond run with their parameters moving. RTSan stops the process at the first allocation, free, lock, or
//! blocking call, and the allocation trap, installed as in every Beatlayer
//! test binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::SampleRate;
use bl_devices::testing::{
    self, EFFECTS, EFFECTS_DIVISION, EFFECTS_HOLD, EFFECTS_INVERT, EFFECTS_OUTPUT,
    EFFECTS_RATE_MODE, EFFECTS_WAVE,
};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, EngineConfig, Registry, TransportCommand, channel};
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

/// The channel EQ, the three-band EQ, the gate and the bus compressor on a
/// track of a test tone, with the gate's and the compressor's sidechain
/// keyed from a second track, while their parameters move between two
/// settings every few blocks (WP-5.21).
fn the_eqs_and_dynamics_pass(device_type: &str, moves: &[(ParamId, [f64; 2])]) {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let track = |editor: &mut Editor| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        id
    };
    let insert = |editor: &mut Editor, track, kind: &str, sidechain: bool| {
        let id = editor.new_id();
        let mut device = Device::new(
            id,
            "",
            DeviceKind::Native {
                device_type: kind.to_owned(),
            },
        );
        if sidechain {
            device = device.with_sidechain();
        }
        let index = editor.document().track(track).unwrap().devices().len();
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device,
            })
            .unwrap();
        id
    };
    let key = track(&mut editor);
    let victim = track(&mut editor);
    for (on, level) in [(key, -6.0), (victim, -20.0)] {
        let tone = insert(&mut editor, on, TEST_TONE, false);
        editor
            .apply(Command::SetDeviceParam {
                device: tone,
                param: test_tone::LEVEL,
                value: level,
            })
            .unwrap();
    }
    let sidechain = bl_devices::device_types()
        .iter()
        .any(|t| t.name == device_type && t.sidechain);
    let device = insert(&mut editor, victim, device_type, sidechain);
    if sidechain {
        editor
            .apply(Command::SetSidechainInput {
                device,
                input: bl_model::Input::Track {
                    track: key,
                    tap: bl_model::Tap::PostFx,
                    chain: None,
                },
            })
            .unwrap();
        editor
            .apply(Command::SetSidechainEnabled {
                device,
                enabled: true,
            })
            .unwrap();
    }
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    for round in 0..2_000u64 {
        let step = round / 7;
        let command = match round % 7 {
            0 => {
                let (param, values) = moves[(step as usize) % moves.len()];
                Some(Command::SetDeviceParam {
                    device,
                    param,
                    value: values[(step as usize / moves.len()) % 2],
                })
            }
            3 if sidechain => Some(Command::SetSidechainListen {
                device,
                listen: step % 2 == 0,
            }),
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
    assert!(state.counters.events_applied > 50);
}

#[test]
fn the_channel_eq_passes_under_rtsan() {
    use bl_devices::channel_eq::*;
    the_eqs_and_dynamics_pass(
        CHANNEL_EQ,
        &[
            (HIGH_PASS, [0.0, 1.0]),
            (LOW, [-12.0, 12.0]),
            (MID, [12.0, -12.0]),
            (MID_FREQ, [200.0, 6_000.0]),
            (HIGH, [-15.0, 15.0]),
            (OUTPUT, [0.5, 1.0]),
        ],
    );
}

#[test]
fn the_three_band_eq_passes_under_rtsan() {
    use bl_devices::three_band_eq::*;
    the_eqs_and_dynamics_pass(
        THREE_BAND_EQ,
        &[
            (SLOPE, [0.0, 1.0]),
            (GAIN_LOW, [0.0, 1.5]),
            (ON_MID, [0.0, 1.0]),
            (FREQ_LOW, [120.0, 800.0]),
            (FREQ_HIGH, [1_500.0, 9_000.0]),
            (GAIN_HIGH, [0.3, 1.0]),
        ],
    );
}

#[test]
fn the_gate_passes_under_rtsan() {
    use bl_devices::gate::*;
    the_eqs_and_dynamics_pass(
        GATE,
        &[
            (THRESHOLD, [-40.0, -10.0]),
            (LOOKAHEAD, [0.0, 2.0]),
            (FLIP, [0.0, 1.0]),
            (HOLD, [5.0, 200.0]),
            (SC_EQ, [0.0, 1.0]),
            (FLOOR, [0.0, 0.2]),
        ],
    );
}

#[test]
fn the_bus_compressor_passes_under_rtsan() {
    use bl_devices::bus_compressor::*;
    the_eqs_and_dynamics_pass(
        BUS_COMPRESSOR,
        &[
            (THRESHOLD, [-40.0, -6.0]),
            (OVERSAMPLING, [0.0, 1.0]),
            (SOFT_CLIP, [0.0, 1.0]),
            (AUTO_RELEASE, [0.0, 1.0]),
            (RATIO, [2.0, 20.0]),
            (DRY_WET, [100.0, 40.0]),
            (SC_EQ, [0.0, 1.0]),
        ],
    );
}
