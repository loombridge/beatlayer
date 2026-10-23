//! Probability groups' audio-thread work under RealtimeSanitizer (WP-6.2):
//! a looped clip of Play All and Play One groups played while the groups'
//! chances change, notes are deactivated and the playhead is located into
//! the middle of the loop. RTSan stops the process at the first allocation,
//! free, lock, or blocking call in any of it, and the allocation trap,
//! installed as in every Beatlayer test binary, fails the test at any heap
//! use it sees.

bl_rt::install_trap!();

use bl_core::time::{SampleRate, Ticks};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, EngineConfig, Registry, TransportCommand, channel};
use bl_model::{
    Clip, ClipData, Command, Device, DeviceKind, Editor, GroupKind, MidiData, Note, NoteGroup,
    TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

/// A looped one-beat clip of two groups, one of each kind, over eight notes.
fn grouped(chance: f64, off: bool) -> MidiData {
    let notes = (0..8u8).map(|place| {
        let mut note = Note::new(
            60 + place,
            Ticks::QUARTER / 8 * i64::from(place),
            Ticks::QUARTER / 16,
            100,
        );
        note.probability = chance;
        note.mute = off && place == 1;
        note.group = Some(u32::from(place / 4) + 1);
        note
    });
    let mut midi = MidiData::new(notes);
    midi.groups = vec![
        NoteGroup {
            id: 1,
            kind: GroupKind::PlayAll,
        },
        NoteGroup {
            id: 2,
            kind: GroupKind::PlayOne,
        },
    ];
    midi.looping = true;
    midi.loop_end = Some(Ticks::QUARTER);
    midi
}

#[test]
fn grouped_notes_play_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 256);
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
            device_type: TEST_TONE.to_owned(),
        },
    );
    device.params.insert(test_tone::LEVEL, -60.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE * 2,
                ClipData::Midi(grouped(0.5, false)),
            ),
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    for round in 0..1_000u64 {
        // The groups' chance and a deactivated member change now and then:
        // each sends the track a new list with its groups made afresh.
        if round % 25 == 0 {
            let data = grouped((round % 5) as f64 / 4.0, round % 50 == 0);
            editor
                .apply(Command::SetClipData {
                    clip,
                    data: ClipData::Midi(data),
                })
                .unwrap();
            sender.send_update(compiler.sync(editor.document()));
        }
        if round % 100 == 50 {
            sender.transport(TransportCommand::Locate(
                Ticks::QUARTER * (round % 7) as i64 / 3,
            ));
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
    assert_eq!(state.counters.data_unrouted, 0);
    assert!(state.counters.data_delivered > 20);
}
