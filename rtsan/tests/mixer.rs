//! The mixer's audio-thread work under RealtimeSanitizer (WP-1.6): track
//! mixers gliding to new volumes, pans, and activator states; outputs and
//! sends gliding as solo, Solo in Place, and send levels change; pre-fader
//! sends reading the copy of a chain's output; a program swap when a
//! return's Pre/Post switch moves, with every mixer and route migrating; and
//! the meters published after every block. RTSan stops the process at the
//! first allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::SampleRate;
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, EngineConfig, Registry, TransportCommand, channel};
use bl_model::{Command, Device, DeviceKind, Editor, SendTap, TrackId, TrackKind};

// Without `--cfg bl_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "bl-rt wasn't built with --cfg bl_rtsan; run `cargo xtask rtsan`"
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
    device.params.insert(test_tone::LEVEL, -60.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
}

#[test]
fn the_mixer_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let pre = track(&mut editor, TrackKind::Return, None);
    let post = track(&mut editor, TrackKind::Return, None);
    let group = track(&mut editor, TrackKind::Group, None);
    let inner = track(&mut editor, TrackKind::Audio, Some(group));
    tone(&mut editor, inner, 330.0);
    let outer = track(&mut editor, TrackKind::Midi, None);
    tone(&mut editor, outer, 440.0);
    for (from, to, level) in [(inner, post, 0.5), (outer, pre, 1.0), (group, post, 0.25)] {
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
            track: pre,
            tap: SendTap::Pre,
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut programs = 1;
    for round in 0..2_000u64 {
        // A new mixer edit every few blocks, so glides run into each other.
        let command = match round % 24 {
            0 => Some(Command::SetVolume {
                track: outer,
                volume: (round % 7) as f64 / 6.0,
            }),
            3 => Some(Command::SetPan {
                track: group,
                pan: ((round % 5) as f64 - 2.0) / 2.0,
            }),
            6 => Some(Command::SetSolo {
                track: inner,
                solo: round % 48 == 6,
            }),
            9 => Some(Command::SoloExclusive { track: post }),
            10 => Some(Command::SetSolo {
                track: post,
                solo: false,
            }),
            12 => Some(Command::SetSoloInPlace {
                enabled: round % 48 == 12,
            }),
            15 => Some(Command::SetSend {
                track: inner,
                to: post,
                level: (round % 3) as f64 / 2.0,
            }),
            18 => Some(Command::SetMute {
                track: outer,
                mute: round % 48 == 18,
            }),
            // Now and then the Pre/Post switch: a program swap.
            21 if round % 240 == 21 => Some(Command::SetSendTap {
                track: post,
                tap: if round % 480 == 21 {
                    SendTap::Pre
                } else {
                    SendTap::Post
                },
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
    assert!(programs > 5, "{programs} programs");
    assert_eq!(state.meters.len(), 6);
    eprintln!(
        "2,000 blocks of the mixer under RealtimeSanitizer: {} parameter changes, {} swaps, \
         peak engine load {:.1}%",
        state.counters.events_applied,
        state.swaps,
        state.cpu_load_max * 100.0
    );
}
