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
    Action, Collector, Compiler, EngineConfig, Registry, ScheduledCommand, TransportCommand, When,
    channel,
};
use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};

// Without `--cfg bl_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "bl-rt wasn't built with --cfg bl_rtsan; run `cargo xtask rtsan`"
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

    let mut wraps = 0;
    let mut last = processor.position();
    for round in 0..2_000u64 {
        let at = processor.clock() + 37;
        let command = match round % 40 {
            0 => Some(TransportCommand::Locate(Ticks::QUARTER * 3)),
            1 => Some(TransportCommand::Play),
            15 => Some(TransportCommand::Stop),
            16 => Some(TransportCommand::Continue),
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
