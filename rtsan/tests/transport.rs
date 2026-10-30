//! The transport's audio-thread work under RealtimeSanitizer (WP-1.5):
//! play, stop, continue, the insert marker, transport commands on exact
//! frames, the Arrangement loop wrapping inside a tempo ramp, a tempo map
//! swapped in during playback, and the beat tone, which reads the tempo
//! map on every frame. RTSan stops the process at the first allocation,
//! free, lock, or blocking call in any of it, and the allocation trap,
//! installed as in every Beatlayer test binary, fails the test at any heap
//! use it sees.

bl_rt::install_trap!();

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::nodes::{BEAT_TONE, beat_tone};
use bl_engine::{
    Action, Collector, Compiler, EngineConfig, Registry, ScheduledCommand, SessionCommand,
    TransportCommand, When, channel,
};
use bl_model::{
    Command, Device, DeviceKind, Editor, LaunchQuantization, LaunchSettings, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn the_transport_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 512);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
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
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: BEAT_TONE.to_owned(),
        },
    );
    device.params.insert(beat_tone::LEVEL, -60.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let bpm = |bpm| Bpm::new(bpm).unwrap();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::from_breakpoints([
                (Ticks::ZERO, bpm(100.0)),
                (Ticks::WHOLE * 4, bpm(160.0)),
            ])
            .unwrap(),
        })
        .unwrap();
    editor
        .apply(Command::SetLoop {
            start: Ticks::QUARTER * 3,
            length: Ticks::SIXTEENTH,
        })
        .unwrap();
    editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    // A quantised locate resolves the next line on the audio thread from the
    // engine's own playhead, with nothing allocated or locked (the transport
    // spec, R22).
    sender.session(SessionCommand::SetQuantization(LaunchSettings {
        quantization: LaunchQuantization::Bar,
        enabled: true,
        ..LaunchSettings::default()
    }));

    let mut wraps = 0;
    let mut last = processor.position();
    for round in 0..2_000u64 {
        let at = processor.clock() + 37;
        let command = match round % 40 {
            0 => Some(TransportCommand::Locate(Ticks::QUARTER * 3)),
            1 => Some(TransportCommand::Play),
            15 => Some(TransportCommand::Stop),
            16 => Some(TransportCommand::Continue),
            20 => Some(TransportCommand::QuantizedLocate(Ticks::WHOLE * 2)),
            // A held scrub loop beside the Arrangement loop (the transport
            // spec, R23): pressed and released while the song plays or is stopped.
            5 | 33 => Some(TransportCommand::Scrub {
                point: Ticks::WHOLE * 2 + Ticks::QUARTER,
                held: true,
            }),
            10 | 35 => Some(TransportCommand::Scrub {
                point: Ticks::WHOLE * 2 + Ticks::QUARTER,
                held: false,
            }),
            25 => Some(TransportCommand::InsertMarkerToPlayhead),
            30 => Some(TransportCommand::PlayStop),
            31 => Some(TransportCommand::Stop),
            32 => Some(TransportCommand::PlayStop),
            _ => None,
        };
        if let Some(command) = command {
            sender.transport_at(command, When::Frame(at));
        }
        if round % 400 == 200 {
            // A new tempo map during playback: a program swap that moves the
            // loop's frames.
            let tempo = if round % 800 == 200 { 140.0 } else { 90.0 };
            editor
                .apply(Command::SetTempo {
                    tempo: TempoMap::constant(bpm(tempo)),
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        if round % 100 == 50 {
            sender.schedule(ScheduledCommand {
                id: round,
                at: Ticks::QUARTER * 3 + Ticks::SIXTEENTH / 2,
                action: Action::Transport(TransportCommand::Locate(Ticks::QUARTER * 3)),
            });
        }
        assert!(sender.flush());

        bl_rt::rt_test!({
            processor.process(512);
        });
        if processor.playing() && processor.position() < last {
            wraps += 1;
        }
        last = processor.position();
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.transport_overflowed, 0);
    assert!(wraps > 10, "{wraps} wraps");
    eprintln!(
        "2,000 blocks of the transport under RealtimeSanitizer: {wraps} jumps back, {} swaps, \
         peak engine load {:.1}%",
        state.swaps,
        state.cpu_load_max * 100.0
    );
}

/// The song speed and Phase Nudge on the audio thread (WP-7.30; the
/// transport spec, R24 to R27): a followed tempo that changes every few
/// blocks over a tempo map that ramps, jumps the follower asks for, starts
/// that wait for the follower's frame, the loop wrapping at a speed other
/// than one, and a nudge pressed and released, beside notes, the metronome
/// and automation placed in the song. RTSan stops the process at the first
/// allocation, lock or blocking call.
#[test]
fn the_song_speed_and_phase_nudge_pass_under_rtsan() {
    use bl_engine::NudgeDirection;
    use bl_model::{Clip, ClipData, MidiData, Note};

    let config = EngineConfig::new(SampleRate::HZ_48000, 512);
    let mut collector = Collector::new();
    let probe = std::sync::Arc::new(bl_engine::testing::Probe::default());
    let mut compiler = Compiler::new(
        config,
        bl_engine::testing::registry(&probe),
        collector.handle(),
    );
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
    let device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: bl_engine::testing::NOTE_PULSE.to_owned(),
        },
    );
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let notes: Vec<Note> = (0..32)
        .map(|i| {
            Note::new(
                48 + (i % 24) as u8,
                Ticks::SIXTEENTH * i,
                Ticks::SIXTEENTH,
                100,
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
                Ticks::WHOLE * 2,
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    editor
        .apply(Command::SetMetronomeEnabled { enabled: true })
        .unwrap();
    let bpm = |bpm| Bpm::new(bpm).unwrap();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::from_breakpoints([
                (Ticks::ZERO, bpm(100.0)),
                (Ticks::WHOLE * 2, bpm(160.0)),
            ])
            .unwrap(),
        })
        .unwrap();
    editor
        .apply(Command::SetLoop {
            start: Ticks::ZERO,
            length: Ticks::WHOLE,
        })
        .unwrap();
    editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));

    // From 20 BPM to 999, a new tempo every three blocks, a jump now and then,
    // a start that waits, and the follower going now and then.
    let follower: Box<dyn bl_engine::Follower> =
        Box::new(bl_engine::testing::stand_in(move |ask, answer| {
            let block = ask.clock / 512;
            if block % 97 < 90 {
                answer.tempo(20.0 + ((block / 3) * 37 % 980) as f64);
            }
            if block % 200 < 120 {
                answer.decides_starts();
                answer.start_at(ask.clock + 700);
            }
            if block % 53 == 0 {
                answer.jump(ask.clock + 100, Ticks::QUARTER * 2);
            }
        }));
    sender.follower(Some(collector.handle().wrap(follower)));

    let mut moved = 0;
    for round in 0..3_000u64 {
        let command = match round % 60 {
            0 => Some(TransportCommand::Play),
            12 => Some(TransportCommand::PhaseNudge {
                direction: NudgeDirection::Up,
                held: true,
            }),
            14 => Some(TransportCommand::PhaseNudge {
                direction: NudgeDirection::Down,
                held: true,
            }),
            20 => Some(TransportCommand::PhaseNudge {
                direction: NudgeDirection::Up,
                held: false,
            }),
            22 => Some(TransportCommand::PhaseNudge {
                direction: NudgeDirection::Down,
                held: false,
            }),
            45 => Some(TransportCommand::Stop),
            _ => None,
        };
        if let Some(command) = command {
            sender.transport(command);
        }
        assert!(sender.flush());
        let before = processor.position();
        bl_rt::rt_test!({
            processor.process(512);
        });
        if processor.position() != before {
            moved += 1;
        }
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert_eq!(state.counters.transport_overflowed, 0);
    assert!(moved > 500, "{moved} blocks moved the song");
    eprintln!(
        "3,000 blocks of the song speed and Phase Nudge under RealtimeSanitizer: {moved} \
         blocks moved the song, peak engine load {:.1}%",
        state.cpu_load_max * 100.0
    );
}
