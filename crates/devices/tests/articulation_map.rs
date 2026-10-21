//! Articulation Map (WP-6.47; the articulation map spec): a trigger makes
//! its articulation current and sends its outputs, the notes that follow go
//! out on its channel, and the device's own controls work. Every block is a
//! real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_devices::articulation_map::{
    ARTICULATION, ARTICULATION_MAP, PASS_TRIGGERS, articulation_map_type,
};
use bl_engine::{NoteEvent, NoteKind};
use bl_model::{Articulation, ArticulationOutput, Command, Trigger};
use common::chain::{BEAT_FRAMES, Chain, note};
use common::{device, node, notes_through_events, registry, set};

/// A block of frames long.
const FRAMES: usize = 2048;

/// A note-on of `key` on frame `offset`.
fn on(offset: u32, key: u8, channel: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::On,
        channel,
        key,
        velocity: 0.5,
        note_id: None,
    }
}

/// A note-off of `key` on frame `offset`.
fn off(offset: u32, key: u8, channel: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::Off,
        channel,
        key,
        velocity: 0.0,
        note_id: None,
    }
}

/// A device of the Articulation Map type holding `articulations`.
fn map(articulations: Vec<Articulation>) -> bl_model::Device {
    let mut device = device(&articulation_map_type(), &[]);
    device.set_articulations(articulations);
    device
}

/// The articulation the tests use: a keyswitch on key 36, a keyswitch note
/// and a controller, and channel 5.
fn articulation() -> Articulation {
    Articulation {
        name: "Staccato".to_owned(),
        trigger: Trigger::Key { key: 36 },
        outputs: vec![
            ArticulationOutput::Key {
                key: 24,
                velocity: 100,
            },
            ArticulationOutput::Controller {
                controller: 1,
                value: 64,
            },
        ],
        channel: Some(5),
    }
}

// Spec: articulation-map R7, R8, R9, R10. A trigger makes its articulation
// current, sends its outputs before the note on the same frame, and the
// notes that follow go out on the articulation's channel. The trigger
// itself doesn't reach the next device.
#[test]
fn a_trigger_sends_its_outputs_before_the_notes_on_the_same_frame() {
    let mut node = node(&registry(), &map(vec![articulation()]), FRAMES);
    let out = notes_through_events(node.as_mut(), &[on(0, 36, 0), on(0, 60, 0)], &[], FRAMES);
    let kinds: Vec<(u32, NoteKind, u8, u8)> = out
        .iter()
        .map(|note| (note.offset, note.kind, note.key, note.channel))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (0, NoteKind::On, 24, 4),
            (
                0,
                NoteKind::Control {
                    controller: 1,
                    value: 64.0 / 127.0
                },
                0,
                4
            ),
            (0, NoteKind::On, 60, 4),
            (960, NoteKind::Off, 24, 4),
        ],
        "{kinds:?}"
    );
}

// Spec: articulation-map R8. With Pass Triggers on, the trigger's note
// reaches the next device as it came.
#[test]
fn pass_triggers_lets_the_trigger_through() {
    let mut device = map(vec![articulation()]);
    device.params.insert(PASS_TRIGGERS, 1.0);
    let mut node = node(&registry(), &device, FRAMES);
    let out = notes_through_events(node.as_mut(), &[on(0, 36, 0)], &[], FRAMES);
    let keys: Vec<(u32, u8)> = out
        .iter()
        .filter(|note| note.kind == NoteKind::On)
        .map(|note| (note.offset, note.key))
        .collect();
    assert_eq!(
        keys,
        vec![(0, 24), (0, 36)],
        "the trigger passes on: {keys:?}"
    );
    let _ = &mut node;
}

// Spec: articulation-map R10. A note that is sounding keeps the channel it
// started on for its note-off, whatever becomes current while it is held.
#[test]
fn a_note_held_across_a_change_ends_on_its_own_channel() {
    let mut second = articulation();
    second.name = "Legato".to_owned();
    second.trigger = Trigger::Key { key: 37 };
    second.channel = Some(6);
    let mut node = node(&registry(), &map(vec![articulation(), second]), FRAMES);
    let out = notes_through_events(
        node.as_mut(),
        &[on(0, 60, 0), on(10, 37, 0), off(100, 60, 0)],
        &[],
        FRAMES,
    );
    let notes: Vec<(u32, NoteKind, u8, u8)> = out
        .iter()
        .map(|note| (note.offset, note.kind, note.key, note.channel))
        .collect();
    assert_eq!(
        notes,
        vec![
            (0, NoteKind::On, 60, 4),
            (10, NoteKind::On, 24, 5),
            (
                10,
                NoteKind::Control {
                    controller: 1,
                    value: 64.0 / 127.0
                },
                0,
                5
            ),
            (100, NoteKind::Off, 60, 4),
            (970, NoteKind::Off, 24, 5),
        ],
        "{notes:?}"
    );
}

// Spec: articulation-map R11. A keyswitch sent again while its last
// note-on is still sounding ends that note first, on the same frame.
#[test]
fn a_repeated_key_output_ends_the_earlier_one_first() {
    let mut node = node(&registry(), &map(vec![articulation()]), FRAMES);
    let out = notes_through_events(node.as_mut(), &[on(0, 36, 0), on(100, 36, 0)], &[], FRAMES);
    let notes: Vec<(u32, NoteKind, u8)> = out
        .iter()
        .filter(|note| note.key == 24)
        .map(|note| (note.offset, note.kind, note.key))
        .collect();
    assert_eq!(
        notes,
        vec![
            (0, NoteKind::On, 24),
            (100, NoteKind::Off, 24),
            (100, NoteKind::On, 24),
            (1060, NoteKind::Off, 24),
        ],
        "{notes:?}"
    );
}

// Spec: articulation-map R6. The Articulation control's value makes the
// articulation it names current, on the frame its value changes.
#[test]
fn the_articulation_control_makes_its_articulation_current() {
    let mut second = articulation();
    second.name = "Legato".to_owned();
    second.trigger = Trigger::Key { key: 37 };
    second.outputs = vec![ArticulationOutput::Key {
        key: 25,
        velocity: 100,
    }];
    let mut node = node(&registry(), &map(vec![articulation(), second]), FRAMES);
    let out = notes_through_events(
        node.as_mut(),
        &[on(0, 60, 0)],
        &[set(100, ARTICULATION, 2.0)],
        FRAMES,
    );
    let notes: Vec<(u32, u8)> = out
        .iter()
        .filter(|note| note.kind == NoteKind::On)
        .map(|note| (note.offset, note.key))
        .collect();
    assert_eq!(notes, vec![(0, 60), (100, 25)], "{notes:?}");
}

// Spec: articulation-map R13. A device switched off ends the keyswitch it
// has sounding.
#[test]
fn a_device_switched_off_ends_its_sounding_keyswitch() {
    // A block shorter than the keyswitch's 20 ms, so the note-off is still
    // sounding when the device is switched off.
    let mut node = node(&registry(), &map(vec![articulation()]), 512);
    let out = notes_through_events(node.as_mut(), &[on(0, 36, 0)], &[], 512);
    let keyswitches = out
        .iter()
        .filter(|note| note.key == 24)
        .map(|note| (note.offset, note.kind))
        .collect::<Vec<_>>();
    assert_eq!(keyswitches, vec![(0, NoteKind::On)]);
    let mut released = bl_engine::NoteBuffer::with_capacity(16);
    node.release(&mut released);
    let offs: Vec<(u32, u8)> = released
        .notes()
        .iter()
        .map(|note| (note.offset, note.key))
        .collect();
    assert_eq!(offs, vec![(0, 24)], "{offs:?}");
}

// Spec: articulation-map R12. Resend on Start sends the current
// articulation's outputs again on the first frame the transport plays.
#[test]
fn resend_on_start_sends_the_outputs_again_when_the_transport_plays() {
    let mut node = node(&registry(), &map(vec![articulation()]), FRAMES);
    // The transport is stopped: nothing is sent.
    let stopped = notes_through_events(node.as_mut(), &[], &[], FRAMES);
    assert!(stopped.is_empty(), "{stopped:?}");
    // The transport plays: the outputs are sent on the first frame.
    let mut data = vec![0.0f32; 2 * FRAMES];
    let block = bl_engine::Block::new(
        &mut data,
        2,
        FRAMES,
        FRAMES,
        &[],
        bl_engine::TransportInfo {
            playing: true,
            position: bl_core::time::Frames(0),
        },
        common::RATE,
    );
    let mut out = bl_engine::NoteBuffer::with_capacity(64);
    let mut block = block.with_notes(&[], &mut out);
    bl_rt::rt_test!({ node.process(&mut block) });
    let keyswitches: Vec<(u32, u8)> = out
        .notes()
        .iter()
        .map(|note| (note.offset, note.key))
        .collect();
    assert_eq!(
        keyswitches,
        vec![(0, 24), (0, 0), (960, 24)],
        "{keyswitches:?}"
    );
}

// Spec: articulation-map R6. An automated Articulation lane makes the
// articulation it names current on its breakpoints' frames.
#[test]
fn an_automated_articulation_lane_switches_on_its_breakpoints_frames() {
    use bl_core::time::{Frames, TempoMap, Ticks};
    use bl_engine::{Collector, Compiler, EngineConfig, NodeKey, TransportInfo, Update};
    use bl_model::{AutomationTarget, Editor, TrackKind};

    let mut second = articulation();
    second.name = "Legato".to_owned();
    second.trigger = Trigger::Key { key: 37 };
    second.outputs = vec![ArticulationOutput::Key {
        key: 25,
        velocity: 100,
    }];
    second.channel = Some(6);

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
    let id = editor.new_id();
    let mut device = articulation_map_type().device(id);
    device.set_articulations(vec![articulation(), second]);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let target = AutomationTarget::Device {
        device: id,
        param: ARTICULATION,
    };
    editor
        .apply(Command::SetAutomationBreakpoint {
            track,
            target,
            at: Ticks::QUARTER,
            value: 1.0,
        })
        .unwrap();
    editor
        .apply(Command::SetAutomationBreakpoint {
            track,
            target,
            at: Ticks::QUARTER * 2 + Ticks::QUARTER / 2,
            value: 2.0,
        })
        .unwrap();

    // The envelope the compiler sends the device, as a node gets it.
    let collector = Collector::new();
    let config = EngineConfig::new(common::RATE, 48_000);
    let mut compiler = Compiler::new(config, registry(), collector.handle());
    let update: Update = compiler.sync(editor.document());
    let data = update
        .data
        .into_iter()
        .find(|(key, _)| matches!(key, NodeKey::Device(device) if *device == id))
        .expect("the device's node is sent its automation")
        .1;
    let tempo: TempoMap = editor.document().settings().tempo.clone();
    let mut device = articulation_map_type().device(id);
    device.set_articulations(vec![articulation(), {
        let mut second = articulation();
        second.name = "Legato".to_owned();
        second.trigger = Trigger::Key { key: 37 };
        second.outputs = vec![ArticulationOutput::Key {
            key: 25,
            velocity: 100,
        }];
        second.channel = Some(6);
        second
    }]);
    let mut node = node(&registry(), &device, 48_000);
    node.receive_data(data);

    // A block from the second beat: the lane's second breakpoint is in it.
    let mut out = bl_engine::NoteBuffer::with_capacity(64);
    let mut data = vec![0.0f32; 2 * 48_000];
    let mut block = bl_engine::Block::new(
        &mut data,
        2,
        48_000,
        48_000,
        &[],
        TransportInfo {
            playing: true,
            position: Frames(24_000),
        },
        common::RATE,
    )
    .with_tempo(&tempo)
    .with_notes(&[], &mut out);
    bl_rt::rt_test!({ node.process(&mut block) });
    let keyswitches: Vec<(u32, u8)> = out
        .notes()
        .iter()
        .filter(|note| note.kind == bl_engine::NoteKind::On)
        .map(|note| (note.offset, note.key))
        .collect();
    assert_eq!(
        keyswitches,
        vec![(0, 24), (18_000, 25)],
        "the lane's value reaches 2 halfway between its breakpoints, and the Articulation \
         control is a stepped parameter, so its value is 2 from then on: {keyswitches:?}"
    );
}

// Spec: articulation-map R14. The device sends everything on its own
// frame, at any block size.
#[test]
fn the_device_sends_on_its_frame_at_any_block_size() {
    for size in [1usize, 100, 512, 4096] {
        let mut chain = Chain::new(4096);
        let device = chain.effect(ARTICULATION_MAP, &[]);
        chain.apply(Command::SetArticulation {
            device,
            index: 0,
            name: Some("Staccato".to_owned()),
            trigger: Some(Trigger::Key { key: 36 }),
            outputs: Some(vec![
                ArticulationOutput::Key {
                    key: 24,
                    velocity: 100,
                },
                ArticulationOutput::Controller {
                    controller: 1,
                    value: 64,
                },
            ]),
            channel: Some(Some(5)),
        });
        // The trigger and the chord on the same frame, a beat in, so the
        // resend on the first played frame doesn't overlap them.
        chain.clip(2, [note(36, 1, 1), note(60, 1, 1)]);
        chain.sync();
        chain.play();
        chain.run(BEAT_FRAMES as usize * 2, size);
        let heard = chain.recorded();
        let first: Vec<(i64, bl_engine::NoteKind, u8, u8)> = heard
            .iter()
            .map(|note| {
                (
                    note.frame,
                    note.event.kind,
                    note.event.key,
                    note.event.channel,
                )
            })
            .collect();
        let at = |key: u8, kind: bl_engine::NoteKind| {
            first
                .iter()
                .find(|(frame, k, k2, _)| *frame == BEAT_FRAMES && *k == kind && *k2 == key)
        };
        let keyswitch = at(24, bl_engine::NoteKind::On);
        assert_eq!(
            keyswitch.map(|(_, _, _, channel)| *channel),
            Some(4),
            "the keyswitch comes first at size {size}: {first:?}"
        );
        let chord = at(60, bl_engine::NoteKind::On);
        assert_eq!(
            chord.map(|(_, _, _, channel)| *channel),
            Some(4),
            "the chord is on the articulation's channel at size {size}: {first:?}"
        );
        assert!(
            !first.iter().any(|(_, _, key, _)| *key == 36),
            "the trigger is swallowed at size {size}: {first:?}"
        );
    }
}
