//! Clip envelopes' audio-thread work under RealtimeSanitizer (WP-3.15): a
//! clip's envelopes modulating its track's fader and one of its sends while
//! the clip plays, with the envelopes edited, moved, unlinked and cleared as
//! the render runs. RTSan stops the process at the first allocation, free,
//! lock, or blocking call in any of it, and the allocation trap, installed
//! as in every Beatlayer test binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, EngineConfig, Registry, TransportCommand, channel};
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, Device, DeviceKind, Editor, MidiData,
    MixerParam, TrackId, TrackKind, Vector,
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

/// The engine's test tone on `track`, quiet enough to leave running.
fn tone(editor: &mut Editor, track: TrackId) {
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    device.params.insert(test_tone::FREQUENCY, 330.0);
    device.params.insert(test_tone::LEVEL, -60.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
}

/// A looping two-bar MIDI clip on `track` whose loop brace is its first bar,
/// with a linked envelope ramping the fader from 0 to 1 across the bar and
/// one ramping the send to `to`.
fn clip_with_envelopes(editor: &mut Editor, track: TrackId, to: TrackId) -> ClipId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE * 2,
                ClipData::Midi(MidiData {
                    notes: Vector::new(),
                    start: Ticks::ZERO,
                    end: None,
                    looping: true,
                    loop_start: Ticks::ZERO,
                    loop_end: Some(Ticks::WHOLE),
                    signature: None,
                }),
            ),
        })
        .unwrap();
    let targets = [
        AutomationTarget::Mixer {
            param: MixerParam::Volume,
        },
        AutomationTarget::Send { to },
    ];
    for target in targets {
        editor
            .apply(Command::SetClipEnvelopeBreakpoint {
                clip: id,
                target,
                at: Ticks::ZERO,
                value: 0.0,
            })
            .unwrap();
        editor
            .apply(Command::SetClipEnvelopeBreakpoint {
                clip: id,
                target,
                at: Ticks::WHOLE,
                value: 1.0,
            })
            .unwrap();
    }
    id
}

fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

#[test]
fn clip_envelopes_pass_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    let effects = track(&mut editor, TrackKind::Return);
    let lead = track(&mut editor, TrackKind::Midi);
    tone(&mut editor, lead);
    editor
        .apply(Command::SetSend {
            track: lead,
            to: effects,
            level: 0.5,
        })
        .unwrap();
    let clip = clip_with_envelopes(&mut editor, lead, effects);
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    for round in 0..600u64 {
        // An envelope edit every few blocks: a breakpoint set and moved, the
        // Linked switch, the breakpoint deleted, and, every 240 rounds, the
        // whole envelope cleared and drawn again — each one sends the track's
        // nodes their lists afresh.
        let command = match round % 24 {
            0 => Some(Command::SetClipEnvelopeBreakpoint {
                clip,
                target: volume(),
                at: Ticks::WHOLE / 2,
                value: (round % 5) as f64 / 4.0,
            }),
            6 => Some(Command::MoveClipEnvelopeBreakpoint {
                clip,
                target: volume(),
                at: Ticks::WHOLE / 2,
                to: Ticks::WHOLE / 4,
            }),
            12 => Some(Command::SetClipEnvelopeLinked {
                clip,
                target: volume(),
                linked: round % 48 != 12,
            }),
            18 => Some(Command::DeleteClipEnvelopeBreakpoint {
                clip,
                target: volume(),
                at: Ticks::WHOLE / 4,
            }),
            21 if round % 240 == 21 => Some(Command::DeleteClipEnvelope {
                clip,
                target: volume(),
            }),
            23 if round % 240 == 23 => Some(Command::SetClipEnvelopeBreakpoint {
                clip,
                target: volume(),
                at: Ticks::ZERO,
                value: 0.0,
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
    assert_eq!(state.counters.events_unrouted, 0);
    assert_eq!(state.counters.migration_mismatches, 0);
    eprintln!(
        "600 blocks of clip envelopes under RealtimeSanitizer: {} parameter changes, peak \
         engine load {:.1}%",
        state.counters.events_applied,
        state.cpu_load_max * 100.0
    );
}
