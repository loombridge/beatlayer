//! Routing and sidechains under RealtimeSanitizer (T13): Audio From and To
//! between tracks, taps (Pre FX, Post FX, Post Mixer), monitoring switches
//! (In, Auto, Off), armed state, sends-only, resampling tracks, loop breaking,
//! and device sidechains with External, SC Gain, Mix, and Listen switches,
//! driven while the engine plays and swaps programs. RTSan stops the process
//! at the first allocation, free, lock, or blocking call on the audio thread,
//! and the allocation trap fails the test at any heap use it sees.

bl_rt::install_trap!();

use std::sync::Arc;

use bl_core::time::SampleRate;
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::testing::{self, KEY, Probe};
use bl_engine::{Collector, Compiler, EngineConfig, TransportCommand, channel};
use bl_model::{
    Chain, ChainId, Command, Device, DeviceId, DeviceKind, Editor, Input, Monitor, Output,
    RackKind, SendTap, Tap, TrackId, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

fn track(editor: &mut Editor, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind,
            name: String::new(),
            parent,
            index: 0,
        })
        .unwrap();
    id
}

fn tone(editor: &mut Editor, track: TrackId, hz: f64) {
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    device.params.insert(test_tone::FREQUENCY, hz);
    device.params.insert(test_tone::LEVEL, -20.0);
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
}

fn key(editor: &mut Editor, track: TrackId) -> DeviceId {
    let id = editor.new_id();
    let device = Device::new(
        id,
        "Test Sidechain",
        DeviceKind::Native {
            device_type: KEY.to_owned(),
        },
    )
    .with_sidechain();
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

#[test]
fn routing_and_sidechains_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let probe = Arc::new(Probe::default());
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, testing::registry(&probe), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let r1 = track(&mut editor, TrackKind::Return, None);
    let r2 = track(&mut editor, TrackKind::Return, None);
    let group = track(&mut editor, TrackKind::Group, None);
    let inner = track(&mut editor, TrackKind::Audio, Some(group));
    tone(&mut editor, inner, 660.0);

    let a = track(&mut editor, TrackKind::Audio, None);
    tone(&mut editor, a, 220.0);

    let b = track(&mut editor, TrackKind::Audio, None);
    tone(&mut editor, b, 330.0);

    let c = track(&mut editor, TrackKind::Audio, None);
    tone(&mut editor, c, 440.0);
    let gate_c = key(&mut editor, c);

    let d = track(&mut editor, TrackKind::Audio, None);
    tone(&mut editor, d, 550.0);
    let gate_d = key(&mut editor, d);

    // A track with an audio effect rack of two chains, the first holding a
    // rack of its own, and a track that takes their taps (the racks spec,
    // R38 to R41).
    let racked = track(&mut editor, TrackKind::Audio, None);
    tone(&mut editor, racked, 770.0);
    let (rack, nested): (DeviceId, DeviceId) = (editor.new_id(), editor.new_id());
    let (outer, inner_chain, other): (ChainId, ChainId, ChainId) =
        (editor.new_id(), editor.new_id(), editor.new_id());
    editor
        .apply(Command::InsertDevice {
            track: racked,
            index: 1,
            device: Device::rack(rack, RackKind::AudioEffect)
                .with_chain(Chain::new(outer, "Outer").with_devices(vec![
                    Device::rack(nested, RackKind::AudioEffect)
                        .with_chain(Chain::new(inner_chain, "Inner")),
                ]))
                .with_chain(Chain::new(other, "Other")),
        })
        .unwrap();
    let tapper = track(&mut editor, TrackKind::Audio, None);
    editor
        .apply(Command::SetMonitor {
            track: tapper,
            monitor: Monitor::In,
        })
        .unwrap();

    let resampler = track(&mut editor, TrackKind::Audio, None);
    editor
        .apply(Command::SetInput {
            track: resampler,
            input: Input::Resampling,
            channel: None,
        })
        .unwrap();
    editor
        .apply(Command::SetMonitor {
            track: resampler,
            monitor: Monitor::In,
        })
        .unwrap();

    for (from, to, level) in [(inner, r1, 0.5), (a, r2, 0.25), (resampler, r1, 0.1)] {
        editor
            .apply(Command::SetSend {
                track: from,
                to,
                level,
            })
            .unwrap();
    }
    editor
        .apply(Command::SetSendTap {
            track: r1,
            tap: SendTap::Pre,
        })
        .unwrap();

    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let take = |tap, chain| Command::SetInput {
        track: tapper,
        input: Input::Track {
            track: racked,
            tap,
            chain: Some(chain),
        },
        channel: None,
    };
    let mut programs = 1;
    for round in 0..2_000u64 {
        let command = match round % 96 {
            0 => Some(Command::SetOutput {
                track: a,
                output: Output::Track { track: b },
            }),
            2 => Some(Command::SetMonitor {
                track: b,
                monitor: Monitor::In,
            }),
            4 => Some(Command::SetSidechainInput {
                device: gate_c,
                input: Input::Track {
                    track: a,
                    tap: Tap::PostFx,
                    chain: None,
                },
            }),
            6 => Some(Command::SetSidechainEnabled {
                device: gate_c,
                enabled: true,
            }),
            8 => Some(Command::SetSidechainGain {
                device: gate_c,
                gain: 1.0 + (round % 7) as f64 * 0.5,
            }),
            10 => Some(Command::SetSidechainMix {
                device: gate_c,
                mix: ((round % 5) as f64) * 0.25,
            }),
            12 => Some(Command::SetSidechainListen {
                device: gate_c,
                listen: true,
            }),
            14 => Some(Command::SetInput {
                track: b,
                input: Input::Track {
                    track: a,
                    tap: Tap::PreFx,
                    chain: None,
                },
                channel: None,
            }),
            16 => Some(Command::SetMonitor {
                track: b,
                monitor: Monitor::Auto,
            }),
            18 => Some(Command::SetArm {
                track: b,
                arm: true,
            }),
            20 => Some(Command::SetSidechainInput {
                device: gate_c,
                input: Input::Track {
                    track: b,
                    tap: Tap::PostMixer,
                    chain: None,
                },
            }),
            22 => Some(Command::SetSidechainListen {
                device: gate_c,
                listen: false,
            }),
            24 => Some(Command::SetSidechainGain {
                device: gate_c,
                gain: 4.0,
            }),
            26 => Some(Command::SetInput {
                track: b,
                input: Input::Track {
                    track: a,
                    tap: Tap::PostMixer,
                    chain: None,
                },
                channel: None,
            }),
            28 => Some(Command::SetMonitor {
                track: b,
                monitor: Monitor::Off,
            }),
            30 => Some(Command::SetOutput {
                track: a,
                output: Output::SendsOnly,
            }),
            32 => Some(Command::SetSidechainInput {
                device: gate_d,
                input: Input::Track {
                    track: inner,
                    tap: Tap::PostFx,
                    chain: None,
                },
            }),
            34 => Some(Command::SetSidechainEnabled {
                device: gate_d,
                enabled: true,
            }),
            36 => Some(Command::SetSidechainMix {
                device: gate_d,
                mix: 0.75,
            }),
            38 => Some(Command::SetSolo {
                track: b,
                solo: true,
            }),
            40 => Some(Command::SoloExclusive { track: a }),
            42 => Some(Command::SoloExclusive { track: c }),
            44 => Some(Command::SetSolo {
                track: c,
                solo: false,
            }),
            46 => Some(Command::SetSidechainInput {
                device: gate_c,
                input: Input::Track {
                    track: c,
                    tap: Tap::PreFx,
                    chain: None,
                },
            }),
            48 => Some(Command::SetOutput {
                track: a,
                output: Output::Track { track: b },
            }),
            50 => Some(Command::SetInput {
                track: a,
                input: Input::Track {
                    track: b,
                    tap: Tap::PostMixer,
                    chain: None,
                },
                channel: None,
            }),
            52 => Some(Command::SetOutput {
                track: group,
                output: Output::Track { track: inner },
            }),
            54 => Some(Command::SetOutput {
                track: group,
                output: Output::Default,
            }),
            56 => Some(Command::SetInput {
                track: a,
                input: Input::None,
                channel: None,
            }),
            58 => Some(Command::SetSidechainInput {
                device: gate_c,
                input: Input::Track {
                    track: c,
                    tap: Tap::PostFx,
                    chain: None,
                },
            }),
            60 => Some(Command::SetSidechainInput {
                device: gate_c,
                input: Input::None,
            }),
            62 => Some(Command::SetSidechainEnabled {
                device: gate_c,
                enabled: false,
            }),
            64 => Some(Command::SetOutput {
                track: a,
                output: Output::Main,
            }),
            66 => Some(Command::SetArm {
                track: b,
                arm: false,
            }),
            68 => Some(Command::SetInput {
                track: b,
                input: Input::None,
                channel: None,
            }),
            70 => Some(Command::SetSidechainInput {
                device: gate_d,
                input: Input::Track {
                    track: r1,
                    tap: Tap::PostMixer,
                    chain: None,
                },
            }),
            72 => Some(Command::SetSidechainGain {
                device: gate_d,
                gain: 0.5,
            }),
            74 => Some(Command::SetSidechainMix {
                device: gate_d,
                mix: 0.25,
            }),
            76 => Some(Command::SetSidechainListen {
                device: gate_d,
                listen: true,
            }),
            78 => Some(Command::SetSidechainListen {
                device: gate_d,
                listen: false,
            }),
            80 => Some(Command::SetSidechainEnabled {
                device: gate_d,
                enabled: false,
            }),
            82 => Some(Command::SetSidechainInput {
                device: gate_d,
                input: Input::None,
            }),
            84 => Some(take(Tap::PreFx, outer)),
            86 => Some(take(Tap::PostFx, other)),
            88 => Some(take(Tap::PostMixer, inner_chain)),
            90 => Some(take(Tap::PostFx, inner_chain)),
            92 => Some(Command::SetDeviceEnabled {
                device: nested,
                enabled: false,
            }),
            94 => Some(Command::SetDeviceEnabled {
                device: nested,
                enabled: true,
            }),
            _ => None,
        };

        if let Some(command) = command {
            editor.apply(command).unwrap();
            let update = compiler.sync(editor.document());
            programs += u64::from(update.program.is_some());
            sender.send_update(update);
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
    assert_eq!(state.counters.migration_mismatches, 0);
    assert_eq!(state.swaps, programs);
    assert!(programs > 20, "{programs} programs");
    eprintln!(
        "2,000 blocks of routing and sidechains under RealtimeSanitizer: {} parameter changes, \
         {} swaps, peak engine load {:.1}%",
        state.counters.events_applied,
        state.swaps,
        state.cpu_load_max * 100.0
    );
}
