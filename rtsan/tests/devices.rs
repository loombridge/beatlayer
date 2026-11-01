//! The framework's test effects device, Auto Filter and Beat Repeat under
//! RealtimeSanitizer (WP-5.42, WP-5.20, WP-5.21, WP-5.26). The test effects device runs the effects' shared
//! parts (an LFO that locks to the song's tempo map and position, an
//! envelope follower, and a seeded random source) on a track that plays,
//! while its waveform, rate mode, output, Hold, and menu option change.
//! Auto Filter runs every filter type, the oversampled path, the LFO's waves,
//! rate modes and quantiser, and Clip while they change. Channel EQ, EQ Three,
//! Gate and Glue Compressor run with their parameters moving. Beat Repeat runs its
//! intervals, grids, Chance, Variation and its modes, output modes, filter
//! and Repeat button while they change. RTSan stops the process at the first allocation, free, lock, or
//! blocking call, and the allocation trap, installed as in every Beatlayer
//! test binary, fails the test at any heap use it sees.
//!
//! The lo-fi effects (WP-5.24: Erosion, Redux and Vinyl Distortion) run after a test tone
//! with every parameter changing the same way, as do Saturator, Overdrive and Dynamic Tube
//! (WP-5.23) with their curves, Hi-Quality and envelopes changing.

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
        "2,000 blocks of Auto Filter under RealtimeSanitizer: {} parameter changes, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}

#[test]
fn veer_passes_under_rtsan() {
    use bl_devices::shifter::{
        BEND_RANGE, COARSE, DELAY_DIVISION, DELAY_MODE, DELAY_ON, DRIVE, ENV_ON, FEEDBACK, GLIDE,
        LFO_AMOUNT, LFO_MODE, LFO_WAVE, MODE, PITCH_SOURCE, SHIFT, SHIFTER, SPREAD, TONE, WIDE,
        WINDOW,
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
    let mut veer = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: SHIFTER.to_owned(),
        },
    )
    .with_midi_sidechain();
    for (param, value) in [
        (COARSE, 7.0),
        (SPREAD, 50.0),
        (DELAY_ON, 1.0),
        (FEEDBACK, 60.0),
        (TONE, 40.0),
        (LFO_AMOUNT, 40.0),
        (ENV_ON, 1.0),
    ] {
        veer.params.insert(param, value);
    }
    let veer_id = veer.id;
    for (index, device) in [tone, veer].into_iter().enumerate() {
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
        device: veer_id,
        param,
        value,
    };
    for round in 0..2_000u64 {
        // A new setting every few blocks, so changes land mid-stream: the
        // mode (Pitch, Freq and Ring), the shift, the window, Spread and
        // Wide, the delay and its modes, the LFO's wave and rate mode, Drive,
        // and the MIDI sidechain's source, glide and range.
        let step = round / 14;
        let command = match round % 14 {
            0 => Some(set(MODE, (step % 3) as f64)),
            1 => Some(set(COARSE, (step % 25) as f64 - 12.0)),
            2 => Some(set(SHIFT, (step % 9) as f64 * 150.0 - 600.0)),
            3 => Some(set(WINDOW, 2.0 + (step % 10) as f64 * 2.0)),
            4 => Some(set(WIDE, (step % 2) as f64)),
            5 => Some(set(DELAY_MODE, (step % 2) as f64)),
            6 => Some(set(DELAY_DIVISION, (step % 14) as f64)),
            7 => Some(set(DELAY_ON, (step % 3 != 0) as u8 as f64)),
            8 => Some(set(LFO_WAVE, (step % 10) as f64)),
            9 => Some(set(LFO_MODE, (step % 2) as f64)),
            10 => Some(set(DRIVE, (step % 5) as f64 * 25.0)),
            11 => Some(set(PITCH_SOURCE, (step % 2) as f64)),
            12 => Some(set(GLIDE, (step % 4) as f64 * 100.0)),
            13 => Some(set(BEND_RANGE, (step % 25) as f64)),
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
        "2,000 blocks of Shifter under RealtimeSanitizer: {} parameter changes, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}

#[test]
fn corpus_passes_under_rtsan() {
    use bl_core::time::{TICKS_PER_QUARTER, Ticks};
    use bl_devices::corpus::{
        CORPUS, DECAY, FILTER_ON, FREQUENCY, INHARM, LFO_AMOUNT, LFO_WAVE, OFF_DECAY, OPENING,
        PB_RANGE, PRIORITY, QUALITY, RATIO, SPREAD, TYPE,
    };
    use bl_model::{Clip, ClipData, MidiData, Note};

    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut registry = testing::registry();
    bl_devices::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let new_track = |editor: &mut Editor, kind| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        id
    };
    let (audio, keys) = (
        new_track(&mut editor, TrackKind::Audio),
        new_track(&mut editor, TrackKind::Midi),
    );
    // A note every beat for the length of the run: ten seconds at 120 BPM.
    let beat = Ticks(TICKS_PER_QUARTER);
    let notes = (0..20).map(|n| Note::new(48 + (n % 24) as u8, beat * n, beat, 100));
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: keys,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                beat * 20,
                ClipData::Midi(MidiData::new(notes)),
            ),
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
    let mut corpus = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: CORPUS.to_owned(),
        },
    )
    .with_midi_sidechain_tap();
    for (param, value) in [
        (FREQUENCY, 1.0),
        (OFF_DECAY, 1.0),
        (SPREAD, 50.0),
        (LFO_AMOUNT, 40.0),
        (DECAY, 60.0),
    ] {
        corpus.params.insert(param, value);
    }
    let corpus_id = corpus.id;
    for (index, device) in [tone, corpus].into_iter().enumerate() {
        editor
            .apply(Command::InsertDevice {
                track: audio,
                index,
                device,
            })
            .unwrap();
    }
    editor
        .apply(Command::SetMidiSidechainSource {
            device: corpus_id,
            track: Some(keys),
            tap: None,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let set = |param: ParamId, value: f64| Command::SetDeviceParam {
        device: corpus_id,
        param,
        value,
    };
    for round in 0..2_000u64 {
        // A new setting every few blocks, so changes land mid-stream: the
        // type (a waveguide and a modal body in turn), Ratio, Inharm and
        // Quality, which make a new table, Opening, the LFO, the filter and
        // the sidechain's choices.
        let step = round / 12;
        let command = match round % 12 {
            0 => Some(set(TYPE, (step % 7) as f64)),
            1 => Some(set(RATIO, (step % 11) as f64 * 10.0)),
            2 => Some(set(INHARM, (step % 9) as f64 * 25.0 - 100.0)),
            3 => Some(set(QUALITY, (step % 2) as f64)),
            4 => Some(set(OPENING, (step % 5) as f64 * 25.0)),
            5 => Some(set(LFO_WAVE, (step % 7) as f64)),
            6 => Some(set(FILTER_ON, (step % 2) as f64)),
            7 => Some(set(OFF_DECAY, (step % 3 != 0) as u8 as f64)),
            8 => Some(set(PRIORITY, (step % 2) as f64)),
            9 => Some(set(PB_RANGE, (step % 25) as f64)),
            10 => Some(set(DECAY, (step % 11) as f64 * 10.0)),
            11 => Some(set(FREQUENCY, (step % 4 != 0) as u8 as f64)),
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
        "2,000 blocks of Corpus under RealtimeSanitizer: {} parameter changes, \
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
                    output: None,
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

#[test]
fn color_limiter_passes_under_rtsan() {
    use bl_devices::color_limiter::*;
    run_lo_fi_device(
        COLOR_LIMITER,
        &[
            (LOUDNESS, &[1.0, 15.8, 0.0]),
            (CEILING, &[-0.3, -12.0, -24.0]),
            (LOOKAHEAD, &[0.0, 1.0, 2.0]),
            (RELEASE, &[1.0, 100.0, 1_000.0]),
            (SATURATION, &[0.0, 50.0, 100.0]),
            (COLOR, &[-100.0, 0.0, 100.0]),
        ],
    );
}

#[test]
fn re_enveloper_passes_under_rtsan() {
    use bl_devices::re_enveloper::*;
    run_lo_fi_device(
        RE_ENVELOPER,
        &[
            (LOW_CROSSOVER, &[50.0, 200.0, 2_000.0]),
            (HIGH_CROSSOVER, &[500.0, 2_500.0, 16_000.0]),
            (factor(LOW), &[-100.0, 0.0, 100.0]),
            (factor(MID), &[100.0, -100.0, 30.0]),
            (factor(HIGH), &[-50.0, 100.0]),
            (gain(LOW), &[1.0, 0.0, 2.0]),
            (gain(MID), &[1.0, 0.5]),
            (OUTPUT, &[1.0, 0.0, 2.0]),
        ],
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
        "2,000 blocks of Beat Repeat under RealtimeSanitizer: {} parameter changes, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}

// Spec: saturator R14, overdrive R8, tube-distortion R11
#[test]
fn the_saturation_devices_pass_under_rtsan() {
    use bl_devices::{overdrive as grit, saturator as singe, tube as valve};
    run_lo_fi_device(
        singe::SATURATOR,
        &[
            (singe::CURVE, &[0.0, 2.0, 5.0, 7.0]),
            (singe::DRIVE, &[-12.0, 12.0, 30.0]),
            (singe::COLOR, &[1.0, 0.0]),
            (singe::AMT_LO, &[80.0, -60.0]),
            (singe::AMT_HI, &[-70.0, 90.0]),
            (singe::WS_DEPTH, &[0.0, 70.0]),
            (singe::POST_CLIP, &[0.0, 1.0, 2.0]),
            (singe::HI_QUALITY, &[1.0, 0.0]),
            (singe::PRE_DC, &[1.0, 0.0]),
        ],
    );
    run_lo_fi_device(
        grit::OVERDRIVE,
        &[
            (grit::DRIVE, &[0.0, 60.0, 100.0]),
            (grit::TONE, &[0.0, 100.0]),
            (grit::FREQUENCY, &[200.0, 3_000.0]),
            (grit::WIDTH, &[0.5, 3.0]),
            (grit::DYNAMICS, &[0.0, 100.0]),
        ],
    );
    run_lo_fi_device(
        valve::TUBE,
        &[
            (valve::MODEL, &[0.0, 1.0, 2.0]),
            (valve::DRIVE, &[0.0, 24.0]),
            (valve::BIAS, &[0.0, 80.0]),
            (valve::ENVELOPE, &[-80.0, 80.0]),
            (valve::ATTACK, &[1.0, 200.0]),
            (valve::HI_QUALITY, &[1.0, 0.0]),
        ],
    );
}

// Spec: roar R14
#[test]
fn roar_passes_under_rtsan() {
    use bl_devices::roar::{
        AMOUNT, COLOR_COMPENSATION, DRIVE, DRY_WET, FILTER, FREQUENCY, HIGH_CROSSOVER, LEVEL,
        LOW_CROSSOVER, OUTPUT_GAIN, PRE, RESONANCE, ROAR, ROUTING_MODE, SHAPER, TONE_AMOUNT,
        stage_param,
    };
    run_lo_fi_device(
        ROAR,
        &[
            (ROUTING_MODE, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            (stage_param(0, SHAPER), &[0.0, 2.0, 4.0, 8.0, 10.0, 11.0]),
            (stage_param(1, SHAPER), &[10.0, 3.0, 9.0, 5.0]),
            (stage_param(2, SHAPER), &[7.0, 6.0, 1.0]),
            (stage_param(0, FILTER), &[0.0, 4.0, 5.0, 6.0, 7.0, 8.0]),
            (stage_param(1, FILTER), &[8.0, 6.0, 2.0, 3.0]),
            (stage_param(2, FILTER), &[1.0, 4.0, 7.0]),
            (stage_param(0, AMOUNT), &[0.0, 60.0, 100.0]),
            (stage_param(1, AMOUNT), &[100.0, 20.0]),
            (stage_param(0, FREQUENCY), &[20_000.0, 600.0, 90.0]),
            (stage_param(1, RESONANCE), &[0.0, 80.0]),
            (stage_param(2, LEVEL), &[1.0, 0.0, 2.0]),
            (stage_param(0, PRE), &[1.0, 0.0]),
            (DRIVE, &[0.0, 18.0, 36.0]),
            (TONE_AMOUNT, &[-100.0, 100.0, 0.0]),
            (COLOR_COMPENSATION, &[1.0, 0.0]),
            (LOW_CROSSOVER, &[50.0, 2_000.0]),
            (HIGH_CROSSOVER, &[500.0, 16_000.0]),
            (OUTPUT_GAIN, &[0.0, 1.0, 3.9]),
            (DRY_WET, &[100.0, 40.0]),
        ],
    );
}

// Spec: drum-bus R1, R15
#[test]
fn drum_buss_passes_under_rtsan() {
    use bl_devices::drum_bus::*;
    run_lo_fi_device(
        DRUM_BUSS,
        &[
            (TRIM, &[0.0, 0.5, 1.0]),
            (COMP, &[0.0, 1.0]),
            (TYPE, &[0.0, 1.0, 2.0]),
            (DRIVE, &[0.0, 60.0, 100.0]),
            (CRUNCH, &[0.0, 100.0]),
            (DAMP, &[0.0, 50.0, 100.0]),
            (TRANSIENTS, &[-100.0, 0.0, 100.0]),
            (BOOM, &[0.0, 50.0, 100.0]),
            (FREQ, &[20.0, 60.0, 200.0]),
            (FORCE_TO_NOTE, &[0.0, 1.0]),
            (DECAY, &[10.0, 200.0, 2_000.0]),
            (BOOM_AUDITION, &[0.0, 1.0]),
            (DRY_WET, &[0.0, 100.0]),
            (OUTPUT, &[0.0, 1.0]),
        ],
    );
}

/// Multiband Dynamics (WP-5.22) on a track that plays, with its sidechain unused, while
/// its band switches, detector, knee, Time, Amount, solos and thresholds
/// change.
#[test]
fn triband_passes_under_rtsan() {
    use bl_devices::multiband_dynamics::{
        ABOVE_RATIO, ABOVE_THRESHOLD, AMOUNT, BELOW_RATIO, DETECTOR, HIGH, HIGH_ON, LOW, LOW_ON,
        MID, MULTIBAND_DYNAMICS, SOFT_KNEE, SOLO, TIME, band_param,
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
    tone.params.insert(test_tone::LEVEL, -10.0);
    let mut triband = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: MULTIBAND_DYNAMICS.to_owned(),
        },
    );
    for band in [LOW, MID, HIGH] {
        triband
            .params
            .insert(band_param(band, ABOVE_THRESHOLD), -30.0);
        triband.params.insert(band_param(band, ABOVE_RATIO), 4.0);
        triband.params.insert(band_param(band, BELOW_RATIO), 0.5);
    }
    let triband_id = triband.id;
    for (index, device) in [tone, triband].into_iter().enumerate() {
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
        device: triband_id,
        param,
        value,
    };
    for round in 0..2_000u64 {
        // A new setting every few blocks, so changes land mid-stream: the
        // band switches (the crossovers' topology), the detector, the
        // knee, Time, Amount, a solo and a ratio.
        let step = round / 12;
        let command = match round % 12 {
            0 => Some(set(LOW_ON, (step % 2) as f64)),
            2 => Some(set(HIGH_ON, (step / 2 % 2) as f64)),
            4 => Some(set(DETECTOR, (step % 2) as f64)),
            5 => Some(set(SOFT_KNEE, (step % 2) as f64)),
            6 => Some(set(TIME, 25.0 + (step % 8) as f64 * 100.0)),
            8 => Some(set(AMOUNT, (step % 5) as f64 * 50.0)),
            9 => Some(set(
                band_param(step as usize % 3, SOLO),
                (step / 3 % 2) as f64,
            )),
            10 => Some(set(band_param(MID, ABOVE_RATIO), 1.0 + (step % 6) as f64)),
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
        "2,000 blocks of Multiband Dynamics under RealtimeSanitizer: {} parameter changes, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}

// Spec: phaser-flanger R13, chorus-ensemble R8
#[test]
fn the_modulation_devices_pass_under_rtsan() {
    use bl_devices::{chorus_ensemble as chorus, phaser_flanger as pf};
    run_lo_fi_device(
        pf::PHASER_FLANGER,
        &[
            (pf::MODE, &[0.0, 1.0, 2.0]),
            (pf::AMOUNT, &[0.0, 50.0, 100.0]),
            (pf::FEEDBACK, &[0.0, 50.0, 80.0]),
            (pf::FEEDBACK_INVERT, &[0.0, 1.0]),
            (pf::WARMTH, &[0.0, 80.0]),
            (pf::NOTCHES, &[2.0, 4.0, 8.0]),
            (pf::FLANGER_TIME, &[0.5, 2.0, 10.0]),
            (pf::DOUBLER_TIME, &[10.0, 30.0]),
            (pf::SAFE_BASS, &[20.0, 200.0]),
        ],
    );
    run_lo_fi_device(
        chorus::CHORUS_ENSEMBLE,
        &[
            (chorus::MODE, &[0.0, 1.0, 2.0]),
            (chorus::RATE, &[0.5, 2.0, 10.0]),
            (chorus::AMOUNT, &[10.0, 70.0]),
            (chorus::FEEDBACK, &[0.0, 60.0]),
            (chorus::WIDTH, &[0.0, 100.0, 200.0]),
            (chorus::HP_FREQ, &[20.0, 300.0]),
            (chorus::CHORUS_TIME, &[0.0, 2.0, 4.0]),
            (chorus::CHORUS_TAPS, &[0.0, 1.0]),
            (chorus::VIBRATO_SHAPE, &[0.0, 100.0]),
        ],
    );
}

// Spec: gated-delay R8, pitch-hack R8
#[test]
fn gated_delay_and_pitch_hack_pass_under_rtsan() {
    use bl_devices::{gated_delay as gated, pitch_hack as hack};
    run_lo_fi_device(
        gated::GATED_DELAY,
        &[
            (gated::step(1), &[0.0, 1.0]),
            (gated::step(7), &[1.0, 0.0]),
            (gated::STEPS, &[3.0, 16.0, 11.0]),
            (gated::RATE, &[0.0, 4.0, 11.0]),
            (gated::DELAY_STEPS, &[1.0, 16.0]),
            (gated::FEEDBACK, &[0.0, 90.0]),
            (gated::RANDOM, &[0.0, 50.0, 100.0]),
            (gated::DRY_ON_RESTS, &[0.0, 1.0]),
            (gated::INPUT, &[0.0, 1.0, 3.98]),
            (gated::DRY_WET, &[0.0, 100.0]),
        ],
    );
    run_lo_fi_device(
        hack::PITCH_HACK,
        &[
            (hack::SYNC, &[0.0, 1.0]),
            (hack::DIVISION, &[0.0, 5.0, 13.0]),
            (hack::TIME, &[10.0, 250.0, 2_000.0]),
            (hack::TRANSPOSE, &[-24.0, 7.0, 24.0]),
            (hack::CENTS, &[-50.0, 50.0]),
            (hack::REVERSE, &[0.0, 50.0, 100.0]),
            (hack::RANDOM, &[0.0, 50.0, 100.0]),
            (hack::FEEDBACK, &[0.0, 100.0]),
            (hack::INPUT, &[0.0, 1.0]),
            (hack::DRY_WET, &[0.0, 100.0]),
        ],
    );
}

// Spec: echo R18
#[test]
fn echo_passes_under_rtsan() {
    use bl_devices::echo::*;
    run_lo_fi_device(
        ECHO,
        &[
            (CHANNEL_MODE, &[0.0, 1.0, 2.0]),
            (LINK, &[0.0, 1.0]),
            (line_param(0, SYNC), &[0.0, 1.0]),
            (line_param(0, SYNC_MODE), &[0.0, 1.0, 2.0, 3.0]),
            (line_param(0, NOTES), &[0.0, 3.0, 10.0]),
            (line_param(1, TIME), &[1.0, 100.0, 5_000.0]),
            (line_param(1, OFFSET), &[-33.0, 0.0, 33.0]),
            (INPUT, &[0.0, 1.0, 3.98]),
            (DRIVE, &[0.0, 1.0]),
            (FEEDBACK, &[0.0, 60.0, 100.0]),
            (FEEDBACK_INVERT, &[0.0, 1.0]),
            (FILTER, &[0.0, 1.0]),
            (HP_FREQ, &[20.0, 500.0, 5_000.0]),
            (LP_RES, &[0.0, 100.0]),
            (WAVE, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]),
            (LFO_SYNC, &[0.0, 1.0]),
            (LFO_RATE, &[0.0, 5.0, 9.0]),
            (MOD_DELAY, &[0.0, 100.0]),
            (MOD_X4, &[0.0, 1.0]),
            (MOD_FILTER, &[0.0, 100.0]),
            (ENV_MIX, &[0.0, 50.0, 100.0]),
            (GATE, &[0.0, 1.0]),
            (DUCKING, &[0.0, 1.0]),
            (NOISE, &[0.0, 1.0]),
            (NOISE_MORPH, &[0.0, 100.0]),
            (WOBBLE, &[0.0, 1.0]),
            (REPITCH, &[0.0, 1.0]),
            (REVERB, &[0.0, 100.0]),
            (REVERB_LOCATION, &[0.0, 1.0, 2.0]),
            (DECAY, &[0.2, 10.0]),
            (STEREO, &[0.0, 100.0, 200.0]),
            (EQUAL_LOUDNESS, &[0.0, 1.0]),
            (DRY_WET, &[0.0, 50.0, 100.0]),
        ],
    );
}

// Spec: resonators R11
#[test]
fn resonators_passes_under_rtsan() {
    use bl_devices::resonators::*;
    run_lo_fi_device(
        RESONATORS,
        &[
            (FILTER_TYPE, &[0.0, 1.0, 2.0, 3.0]),
            (FILTER_FREQ, &[50.0, 1_000.0, 18_000.0]),
            (MODE, &[0.0, 1.0]),
            (DECAY, &[0.01, 1.0, 30.0]),
            (CONST, &[0.0, 1.0]),
            (COLOR, &[-100.0, 0.0, 100.0]),
            (USE_SCALE, &[0.0, 1.0]),
            (WIDTH, &[0.0, 100.0]),
            (GAIN, &[0.0, 1.0, 3.9]),
            (DRY_WET, &[0.0, 50.0, 100.0]),
            (voice_param(0, PITCH), &[0.0, 60.0, 84.0]),
            (voice_param(0, FINE), &[-50.0, 50.0]),
            (voice_param(1, PITCH), &[-24.0, 12.0, 24.0]),
            (voice_param(3, ON), &[1.0, 0.0]),
            (voice_param(4, ON), &[1.0, 0.0]),
            (voice_param(2, LEVEL), &[0.0, 1.0]),
        ],
    );
}

// Spec: auto-pan-tremolo R15
#[test]
fn auto_pan_tremolo_passes_under_rtsan() {
    use bl_devices::auto_pan_tremolo::*;
    run_lo_fi_device(
        AUTO_PAN_TREMOLO,
        &[
            (MODE, &[0.0, 1.0]),
            (WAVE, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]),
            (TIME_MODE, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]),
            (RATE_HZ, &[0.1, 1.0, 10.0]),
            (TIME_SECS, &[0.1, 1.0, 10.0]),
            (SYNCED_DIVISION, &[0.0, 3.0, 5.0]),
            (STEREO_MODE, &[0.0, 1.0]),
            (PHASE, &[0.0, 90.0, 180.0]),
            (SPIN, &[-50.0, 0.0, 50.0]),
            (PHASE_OFFSET, &[-180.0, 0.0, 180.0]),
            (INVERT, &[0.0, 1.0]),
            (HARMONIC, &[0.0, 1.0]),
            (VINTAGE, &[0.0, 1.0]),
            (AMOUNT, &[0.0, 50.0, 100.0]),
            (SHAPE, &[-100.0, 0.0, 100.0]),
            (ATTACK_TIME, &[0.0, 50.0, 200.0]),
            (DYNAMIC_FM, &[-100.0, 0.0, 100.0]),
        ],
    );
}

// Spec: spectrum-analyser R9
#[test]
fn spectrum_passes_under_rtsan() {
    use bl_devices::spectrum_analyser::*;
    run_lo_fi_device(
        SPECTRUM,
        &[
            (BLOCK, &[0.0, 1.0, 2.0, 3.0, 4.0]),
            (CHANNEL, &[0.0, 1.0, 2.0]),
            (REFRESH, &[10.0, 50.0, 100.0]),
            (AVG, &[1.0, 4.0, 8.0]),
            (MAX, &[0.0, 1.0]),
            (GRAPH, &[0.0, 1.0]),
            (SCALE_X, &[0.0, 1.0, 2.0]),
        ],
    );
}

// Spec: device-framework R46
#[test]
fn the_test_spectrogram_passes_under_rtsan() {
    use bl_devices::testing::{SPECTROGRAM, SPECTROGRAM_FREEZE};
    run_lo_fi_device(SPECTROGRAM, &[(SPECTROGRAM_FREEZE, &[0.0, 1.0])]);
}

// Spec: tuner R8
#[test]
fn tuner_passes_under_rtsan() {
    use bl_devices::tuner::*;
    run_lo_fi_device(
        TUNER,
        &[
            (REFERENCE, &[415.0, 432.0, 440.0, 442.0]),
            (HERTZ_CENTS, &[0.0, 1.0]),
            (CLASSIC_MODE, &[0.0, 1.0]),
            (AUTO, &[0.0, 1.0]),
            (SPELLING_SHARPS, &[0.0, 1.0]),
            (SPELLING_FLATS, &[0.0, 1.0]),
            (SPELLING_BOTH, &[0.0, 1.0]),
        ],
    );
}

// Spec: oscilloscope R8
#[test]
fn oscilloscope_passes_under_rtsan() {
    use bl_devices::oscilloscope::*;
    run_lo_fi_device(
        OSCILLOSCOPE,
        &[
            (TIME, &[5.0, 20.0, 50.0, 100.0]),
            (GAIN, &[0.5, 1.0, 2.0]),
            (FREEZE, &[0.0, 1.0]),
            (DIVISIONS, &[2.0, 4.0, 8.0]),
            (TRIGGER, &[-50.0, 0.0, 50.0]),
        ],
    );
}

// Spec: pitchloop89 R9
#[test]
fn pitchloop89_passes_under_rtsan() {
    use bl_devices::pitchloop89::*;
    run_lo_fi_device(
        PITCHLOOP89,
        &[
            (BANDWIDTH_A, &[0.0, 2.0, 5.0]),
            (BANDWIDTH_B, &[1.0, 3.0, 4.0]),
            (SPEED_A, &[-200.0, -100.0, 50.0, 100.0, 200.0]),
            (SPEED_B, &[200.0, 100.0, -50.0]),
            (DELAY_A, &[0.0, 50.0, 100.0]),
            (HOLD_A, &[0.0, 1.0]),
            (HOLD_B, &[1.0, 0.0]),
            (FEEDBACK_A, &[0.0, 60.0, 100.0]),
            (LOW_PASS_A, &[200.0, 4_000.0, 20_000.0]),
            (HIGH_PASS_B, &[20.0, 500.0, 5_000.0]),
            (ROUTING, &[0.0, 1.0, 2.0]),
            (VIBRATO_DEPTH, &[0.0, 50.0, 100.0]),
            (POSITION_WAVE, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            (POSITION_DEPTH, &[0.0, 40.0, 100.0]),
            (RANDOM, &[1.0, 0.0]),
            (INPUT, &[0.0, 1.0, 3.98]),
        ],
    );
}

// Spec: vector-delay R7
#[test]
fn vector_delay_passes_under_rtsan() {
    use bl_devices::vector_delay::*;
    run_lo_fi_device(
        VECTOR_DELAY,
        &[
            (TAPS, &[1.0, 8.0, 4.0]),
            (GRAVITY, &[-100.0, 0.0, 100.0]),
            (MAGNET, &[100.0, 0.0, -100.0]),
            (FLOW, &[0.0, 100.0]),
            (SPEED, &[0.0, 100.0, 400.0]),
            (FRICTION, &[0.0, 20.0, 100.0]),
            (FEEDBACK, &[0.0, 30.0, 100.0]),
            (SYNC, &[0.0, 1.0]),
            (DIVISION, &[0.0, 4.0, 12.0]),
            (MAX_TIME, &[50.0, 1_000.0, 4_000.0]),
            (RESET, &[1.0, 0.0]),
            (tap_x(0), &[0.0, 0.5, 1.0]),
            (tap_y(3), &[1.0, 0.0, 0.5]),
        ],
    );
}
