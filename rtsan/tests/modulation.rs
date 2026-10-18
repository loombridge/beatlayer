//! Relative modulation's audio-thread work under RealtimeSanitizer
//! (WP-5.9): a clip's modulation envelopes on a track's volume, pan and
//! send, over the track's own volume automation, while the clip plays and
//! the engine publishes what each played. RTSan stops the process at the
//! first allocation, free, lock, or blocking call in any of it, and the
//! allocation trap, installed as in every Beatlayer test binary, fails the
//! test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, EngineConfig, Registry, TransportCommand, channel};
use bl_model::{
    AutomationTarget, Clip, ClipData, Command, Device, DeviceKind, Editor, MidiData, MixerParam,
    TrackId, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

fn track(editor: &mut Editor, kind: TrackKind) -> TrackId {
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
}

#[test]
fn modulated_blocks_run_clean_under_the_sanitizer() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let effects = track(&mut editor, TrackKind::Return);
    let lead = track(&mut editor, TrackKind::Midi);
    let mut tone = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    tone.params.insert(test_tone::LEVEL, -60.0);
    editor
        .apply(Command::InsertDevice {
            track: lead,
            index: 0,
            device: tone,
        })
        .unwrap();
    editor
        .apply(Command::SetSend {
            track: lead,
            to: effects,
            level: 0.5,
        })
        .unwrap();
    let volume = AutomationTarget::Mixer {
        param: MixerParam::Volume,
    };
    for (at, value) in [(Ticks::ZERO, 1.0), (Ticks::WHOLE * 2, 0.25)] {
        editor
            .apply(Command::SetAutomationBreakpoint {
                track: lead,
                target: volume,
                at,
                value,
            })
            .unwrap();
    }
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: lead,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE * 2,
                ClipData::Midi(MidiData::default()),
            ),
        })
        .unwrap();
    for target in [
        volume,
        AutomationTarget::Mixer {
            param: MixerParam::Pan,
        },
        AutomationTarget::Send { to: effects },
    ] {
        for (at, value) in [(Ticks::ZERO, 0.25), (Ticks::WHOLE * 2, 1.0)] {
            editor
                .apply(Command::SetClipEnvelopeBreakpoint {
                    kind: bl_model::EnvelopeKind::Modulation,
                    clip,
                    target,
                    at,
                    value,
                })
                .unwrap();
        }
    }
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut published = 0;
    for round in 0..600u64 {
        assert!(sender.flush());
        bl_rt::rt_test!({
            processor.process(256);
        });
        published = published.max(monitor.state().modulated.len());
        if round % 50 == 0 {
            collector.collect();
        }
    }
    assert_eq!(published, 3, "volume, pan and the send played modulated");
    let state = monitor.state().clone();
    assert_eq!(state.counters.events_overflowed, 0);
    assert_eq!(state.counters.migration_mismatches, 0);
    eprintln!(
        "600 modulated blocks under RealtimeSanitizer: peak engine load {:.1}%",
        state.cpu_load_max * 100.0
    );
}
