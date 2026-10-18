//! Racks in the engine (WP-5.1; the racks spec, R3 to R6, R14 and R15): a
//! rack's chains run in its track's task on buffers of their own and are
//! summed in place, each through its own mixer; a MIDI effect rack merges
//! its chains' notes; chain edits make only the nodes they add. Every block
//! runs as a real-time section under the allocation trap, at block sizes
//! 1, 64, 512 and 4,096.

bl_rt::install_trap!();

mod common;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{GAIN, GAIN_PARAM, INSTRUMENT, SEMITONES, THROUGH, TRANSPOSE};
use bl_engine::{Block, Node, NoteEvent, NoteKind, TransportCommand};
use bl_model::{
    AutomationTarget, Chain, ChainId, Command, Device, DeviceId, DeviceRole, MixerParam, ParamId,
    RackKind, TrackId, TrackKind,
};
use common::{Rig, dc, device, effect_in, in_chain, rack, rack_in, track};

const SIZES: [usize; 4] = [1, 64, 512, 4096];
/// Frames enough for a 20 ms glide at 48 kHz to land, in the blocks of any
/// size.
const SETTLE: usize = 4096;

/// A rig with an audio track holding a DC source of `value` and a rack of
/// `kind` with `chains` chains after it.
fn setup(value: f64, kind: RackKind, chains: usize) -> (Rig, TrackId, DeviceId, Vec<ChainId>) {
    let mut rig = Rig::new(4096);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, value);
    let (rack, chains) = rack(&mut rig.editor, audio, kind, chains);
    (rig, audio, rack, chains)
}

/// The level a rig settles at, which must be the same on every frame.
fn settled(rig: &mut Rig, size: usize) -> f32 {
    rig.render(SETTLE, size);
    let out = rig.render(size.max(64), size);
    assert!(out.iter().all(|s| *s == out[0]), "{out:?}");
    out[0]
}

// Spec: racks R3
#[test]
fn every_chain_takes_the_racks_input_and_the_output_is_their_sum() {
    for size in SIZES {
        let (mut rig, _, _, chains) = setup(1.0, RackKind::AudioEffect, 2);
        effect_in(&mut rig.editor, chains[0], GAIN, &[(GAIN_PARAM, 0.5)]);
        effect_in(&mut rig.editor, chains[1], GAIN, &[(GAIN_PARAM, 0.25)]);
        rig.sync();
        assert_eq!(settled(&mut rig, size), 0.75, "block size {size}");
    }
}

// Spec: racks R3
#[test]
fn an_empty_chain_is_a_dry_path_and_an_empty_rack_passes_through() {
    for size in SIZES {
        let (mut rig, _, _, chains) = setup(1.0, RackKind::AudioEffect, 2);
        effect_in(&mut rig.editor, chains[1], GAIN, &[(GAIN_PARAM, 0.5)]);
        rig.sync();
        assert_eq!(settled(&mut rig, size), 1.5, "the dry chain plus the wet");

        let (mut rig, _, _, _) = setup(1.0, RackKind::AudioEffect, 0);
        rig.sync();
        assert_eq!(settled(&mut rig, size), 1.0, "a rack with no chains");
    }
}

// Spec: racks R5
#[test]
fn chain_volume_pan_activator_and_solo_shape_the_sum() {
    for size in [64, 4096] {
        let (mut rig, _, _, chains) = setup(1.0, RackKind::AudioEffect, 3);
        for chain in &chains {
            effect_in(&mut rig.editor, *chain, THROUGH, &[]);
        }
        rig.sync();
        assert_eq!(settled(&mut rig, size), 3.0, "three chains at unity");

        // Volume is a gain that reaches −∞ dB.
        rig.apply(Command::SetChainVolume {
            chain: chains[0],
            volume: 0.5,
        });
        assert_eq!(settled(&mut rig, size), 2.5);
        rig.apply(Command::SetChainVolume {
            chain: chains[0],
            volume: 0.0,
        });
        assert_eq!(settled(&mut rig, size), 2.0, "−∞ dB adds nothing");

        // Pan follows the track mixer's constant-power law: centred is
        // unity, and hard left the left channel's +3 dB.
        rig.apply(Command::SetChainPan {
            chain: chains[1],
            pan: -1.0,
        });
        rig.render(SETTLE, size);
        rig.block(size);
        let left = rig.processor.output().channel(0)[0];
        let right = rig.processor.output().channel(1)[0];
        assert!(
            (left - (1.0 + std::f32::consts::SQRT_2)).abs() < 1e-5,
            "{left}"
        );
        assert_eq!(
            right, 1.0,
            "the other chain, centred, is unity on the right"
        );
        rig.apply(Command::SetChainPan {
            chain: chains[1],
            pan: 0.0,
        });

        // A chain that's off adds nothing.
        rig.apply(Command::SetChainEnabled {
            chain: chains[2],
            enabled: false,
        });
        assert_eq!(settled(&mut rig, size), 1.0);
        rig.apply(Command::SetChainEnabled {
            chain: chains[2],
            enabled: true,
        });
        rig.apply(Command::SetChainVolume {
            chain: chains[0],
            volume: 1.0,
        });
        assert_eq!(settled(&mut rig, size), 3.0);

        // While any chain is soloed the rack's unsoloed chains add nothing;
        // a switched-off chain stays off even when soloed.
        rig.apply(Command::SetChainSolo {
            chain: chains[1],
            solo: true,
        });
        assert_eq!(settled(&mut rig, size), 1.0);
        rig.apply(Command::SetChainSolo {
            chain: chains[2],
            solo: true,
        });
        assert_eq!(settled(&mut rig, size), 2.0);
        rig.apply(Command::SetChainEnabled {
            chain: chains[2],
            enabled: false,
        });
        assert_eq!(settled(&mut rig, size), 1.0);
        rig.apply(Command::SetChainSolo {
            chain: chains[1],
            solo: false,
        });
        rig.apply(Command::SetChainSolo {
            chain: chains[2],
            solo: false,
        });
        rig.apply(Command::SetChainEnabled {
            chain: chains[2],
            enabled: true,
        });
        assert_eq!(settled(&mut rig, size), 3.0);
    }
}

// Spec: racks R5
#[test]
fn a_solo_inside_one_rack_leaves_every_other_rack_and_track_as_it_is() {
    let mut rig = Rig::new(64);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    let other = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    dc(&mut rig.editor, other, 10.0);
    let (_, first) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 2);
    let (_, second) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 2);
    for chain in first.iter().chain(&second) {
        effect_in(&mut rig.editor, *chain, THROUGH, &[]);
    }
    rig.sync();
    // Each rack doubles the signal through its two chains: 1 → 2 → 4, plus
    // the other track's 10.
    assert_eq!(settled(&mut rig, 64), 14.0);
    rig.apply(Command::SetChainSolo {
        chain: first[0],
        solo: true,
    });
    // The first rack passes one chain (1), the second still doubles it.
    assert_eq!(settled(&mut rig, 64), 12.0);
}

// Spec: racks R6
#[test]
fn chain_mixer_changes_glide_without_a_new_program() {
    let (mut rig, _, _, chains) = setup(1.0, RackKind::AudioEffect, 1);
    effect_in(&mut rig.editor, chains[0], THROUGH, &[]);
    rig.sync();
    assert_eq!(
        settled(&mut rig, 64),
        1.0,
        "unity and centre pass bit for bit"
    );
    let synced = rig.apply(Command::SetChainVolume {
        chain: chains[0],
        volume: 0.0,
    });
    assert!(!synced.program, "a mixer change sends no new program");
    assert_eq!(synced.params, 1);
    // Along a straight line over 20 ms: 960 frames at 48 kHz.
    let out = rig.render(1024, 64);
    assert!(out[0] < 1.0 && out[0] > 0.99, "{}", out[0]);
    assert!((out[480] - 0.5).abs() < 0.01, "{}", out[480]);
    assert_eq!(out[960], 0.0);
    assert_eq!(out[1023], 0.0);
    for pair in out[..960].windows(2) {
        assert!(pair[1] <= pair[0], "the glide only falls");
    }
    // The same on any block size.
    let (mut again, _, _, chains) = setup(1.0, RackKind::AudioEffect, 1);
    effect_in(&mut again.editor, chains[0], THROUGH, &[]);
    again.sync();
    settled(&mut again, 1);
    again.apply(Command::SetChainVolume {
        chain: chains[0],
        volume: 0.0,
    });
    assert_eq!(again.render(1024, 1), out);

    // A chain's controls are automation targets, and a node follows its
    // envelope.
    let (mut rig, audio, _, chains) = setup(1.0, RackKind::AudioEffect, 1);
    effect_in(&mut rig.editor, chains[0], THROUGH, &[]);
    for (at, value) in [(Ticks::ZERO, 1.0), (Ticks(TICKS_PER_QUARTER), 0.0)] {
        rig.editor
            .apply(Command::SetAutomationBreakpoint {
                track: audio,
                target: AutomationTarget::Chain {
                    chain: chains[0],
                    param: MixerParam::Volume,
                },
                at,
                value,
            })
            .unwrap();
    }
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(24_000, 64);
    assert_eq!(out[0], 1.0);
    assert!((out[12_000] - 0.5).abs() < 1e-3, "{}", out[12_000]);
    assert!(out[23_999] < 1e-3);
}

// Spec: racks R3, R15
#[test]
fn a_nested_rack_renders_the_same_at_every_block_size() {
    let render = |size: usize| {
        let (mut rig, _, _, chains) = setup(0.5, RackKind::AudioEffect, 2);
        effect_in(&mut rig.editor, chains[0], GAIN, &[(GAIN_PARAM, 2.0)]);
        let (_, inner) = rack_in(&mut rig.editor, chains[0], RackKind::AudioEffect, 2);
        effect_in(&mut rig.editor, inner[0], GAIN, &[(GAIN_PARAM, 0.5)]);
        effect_in(&mut rig.editor, inner[1], THROUGH, &[]);
        let (_, deepest) = rack_in(&mut rig.editor, inner[1], RackKind::AudioEffect, 2);
        effect_in(&mut rig.editor, deepest[0], GAIN, &[(GAIN_PARAM, 3.0)]);
        effect_in(&mut rig.editor, chains[1], GAIN, &[(GAIN_PARAM, 0.25)]);
        rig.sync();
        // A change part-way through, so the glides are in the render.
        let mut out = rig.render(3000, size);
        rig.apply(Command::SetChainVolume {
            chain: inner[0],
            volume: 0.25,
        });
        rig.apply(Command::SetChainPan {
            chain: chains[1],
            pan: 0.5,
        });
        out.extend(rig.render(3000, size));
        out
    };
    let golden = render(64);
    // The settled values: chain 0 is gain 2 on the DC .5, then the inner
    // rack, whose chain 0 is ×0.5 and ×0.25 (after the change) and whose
    // chain 1 is a pass-through into a rack of ×3 and a dry chain; chain 1
    // of the outer rack is ×0.25, panned to the right (a gain less than
    // unity on the left).
    let inner_0 = 1.0 * 0.5 * 0.25;
    let inner_1 = 1.0 * 3.0 + 1.0;
    let second = 0.5 * 0.25 * bl_engine::mixer::pan_gains(0.5)[0];
    let expected = (inner_0 + inner_1 + second) as f32;
    let last = *golden.last().unwrap();
    assert!((last - expected).abs() < 1e-5, "{last} against {expected}");
}

/// A MIDI track with a clip of notes, a MIDI effect rack, and the test
/// instrument after it.
fn midi_rack(chains: usize) -> (Rig, TrackId, Vec<ChainId>) {
    midi_rack_in(Rig::new(256), chains)
}

/// As [`midi_rack`], in `rig`.
fn midi_rack_in(mut rig: Rig, chains: usize) -> (Rig, TrackId, Vec<ChainId>) {
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    let (_, chains) = rack(&mut rig.editor, midi, RackKind::MidiEffect, chains);
    device(&mut rig.editor, midi, INSTRUMENT, &[]);
    let beat = Ticks(TICKS_PER_QUARTER);
    let notes = [
        bl_model::Note::new(60, Ticks::ZERO, beat, 100),
        bl_model::Note::new(64, beat, beat, 100),
    ];
    let clip = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: midi,
            clip: bl_model::Clip::new(
                clip,
                Ticks::ZERO,
                beat * 4,
                bl_model::ClipData::Midi(bl_model::MidiData::new(notes)),
            ),
        })
        .unwrap();
    (rig, midi, chains)
}

/// What the instrument heard of a second of playing: frame, kind, key.
fn heard(rig: &mut Rig, size: usize) -> Vec<(i64, NoteKind, u8)> {
    rig.sender.transport(TransportCommand::Play);
    rig.render(48_000, size);
    rig.probe
        .recorded_notes()
        .iter()
        .map(|n| (n.frame, n.event.kind, n.event.key))
        .collect()
}

/// What the instrument hears through a MIDI effect rack of one chain that
/// leaves the notes as they are: the notes in the order the clip plays them.
fn one_chain(size: usize) -> Vec<(i64, NoteKind, u8)> {
    let (mut rig, _, chains) = midi_rack(1);
    in_chain(
        &mut rig.editor,
        chains[0],
        TRANSPOSE,
        DeviceRole::MidiEffect,
        &[(SEMITONES, 0.0)],
    );
    rig.sync();
    heard(&mut rig, size)
}

/// `base`, sent through chains that shift it by `shifts`, merged: on each
/// frame the first chain's notes, then the second's.
fn merged(base: &[(i64, NoteKind, u8)], shifts: &[u8]) -> Vec<(i64, NoteKind, u8)> {
    let mut frames: Vec<i64> = base.iter().map(|n| n.0).collect();
    frames.dedup();
    frames
        .into_iter()
        .flat_map(|frame| {
            shifts.iter().flat_map(move |shift| {
                base.iter()
                    .filter(move |n| n.0 == frame)
                    .map(move |&(frame, kind, key)| (frame, kind, key + shift))
            })
        })
        .collect()
}

// Spec: racks R4
#[test]
fn a_midi_effect_racks_chains_merge_in_time_then_list_order() {
    for size in [64, 256] {
        let base = one_chain(size);
        assert_eq!(
            base.len(),
            3,
            "the first note on and off, and the second on"
        );
        let (mut rig, _, chains) = midi_rack(2);
        for (chain, semitones) in chains.iter().zip([1.0, 12.0]) {
            in_chain(
                &mut rig.editor,
                *chain,
                TRANSPOSE,
                DeviceRole::MidiEffect,
                &[(SEMITONES, semitones)],
            );
        }
        rig.sync();
        // Both chains send each note at the same frame: the higher chain in
        // the list first.
        assert_eq!(
            heard(&mut rig, size),
            merged(&base, &[1, 12]),
            "block size {size}"
        );
    }
}

// Spec: racks R4, R5
#[test]
fn a_midi_chain_that_is_off_passes_only_its_note_offs() {
    let base = one_chain(256);
    let (mut rig, _, chains) = midi_rack(2);
    for (chain, semitones) in chains.iter().zip([1.0, 12.0]) {
        in_chain(
            &mut rig.editor,
            *chain,
            TRANSPOSE,
            DeviceRole::MidiEffect,
            &[(SEMITONES, semitones)],
        );
    }
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    // The first note starts on both chains, then the second chain goes off:
    // it ends its note and starts no other.
    rig.render(1000, 100);
    rig.apply(Command::SetChainEnabled {
        chain: chains[1],
        enabled: false,
    });
    rig.render(48_000 - 1000, 100);
    let heard: Vec<_> = rig
        .probe
        .recorded_notes()
        .iter()
        .map(|n| (n.frame, n.event.kind, n.event.key))
        .collect();
    let mut expected = merged(&base, &[1, 12]);
    // The second chain's keys are 12 above the first's: its note starts
    // after the switch-off are the ones dropped.
    expected
        .retain(|&(frame, kind, key)| !(frame >= 1000 && kind == NoteKind::On && key == 64 + 12));
    assert_eq!(heard, expected);
}

/// A test MIDI effect that forwards its notes and sends a control change of
/// its controller behind each note-on (WP-5.44).
struct Controller(u8);

impl Node for Controller {
    fn process(&mut self, block: &mut Block<'_>) {
        for &note in block.notes() {
            block.send_note(note);
            if note.kind == NoteKind::On {
                block.send_note(NoteEvent {
                    kind: NoteKind::Control {
                        controller: self.0,
                        value: 1.0,
                    },
                    key: 0,
                    ..note
                });
            }
        }
    }
}

// Spec: racks R4, device-framework R39
#[test]
fn a_midi_effect_racks_chains_merge_their_messages_with_their_notes() {
    for size in SIZES {
        let rig = Rig::with_registry(4096, |registry| {
            registry.add_native("test.controller.1", |_, _| Box::new(Controller(1)));
            registry.add_native("test.controller.2", |_, _| Box::new(Controller(2)));
        });
        let (mut rig, _, chains) = midi_rack_in(rig, 2);
        for (chain, controller) in chains
            .iter()
            .zip(["test.controller.1", "test.controller.2"])
        {
            in_chain(
                &mut rig.editor,
                *chain,
                controller,
                DeviceRole::MidiEffect,
                &[],
            );
        }
        rig.sync();
        let heard: Vec<_> = heard(&mut rig, size)
            .into_iter()
            .map(|(frame, kind, _)| (frame, kind))
            .collect();
        let control = |controller| NoteKind::Control {
            controller,
            value: 1.0,
        };
        let beat = heard
            .iter()
            .find(|h| h.0 > 0)
            .expect("the first note-off")
            .0;
        // On each frame the first chain's note and message, then the
        // second's.
        assert_eq!(
            heard,
            [
                (0, NoteKind::On),
                (0, control(1)),
                (0, NoteKind::On),
                (0, control(2)),
                (beat, NoteKind::Off),
                (beat, NoteKind::On),
                (beat, control(1)),
                (beat, NoteKind::Off),
                (beat, NoteKind::On),
                (beat, control(2)),
            ],
            "block size {size}"
        );
    }
}

// Spec: racks R4
#[test]
fn an_instrument_racks_chains_all_take_the_notes() {
    let mut rig = Rig::new(256);
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    let (_, chains) = rack(&mut rig.editor, midi, RackKind::Instrument, 2);
    for (chain, semitones) in chains.iter().zip([0.0, 7.0]) {
        in_chain(
            &mut rig.editor,
            *chain,
            TRANSPOSE,
            DeviceRole::MidiEffect,
            &[(SEMITONES, semitones)],
        );
        in_chain(
            &mut rig.editor,
            *chain,
            INSTRUMENT,
            DeviceRole::Instrument,
            &[],
        );
    }
    let beat = Ticks(TICKS_PER_QUARTER);
    let clip = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: midi,
            clip: bl_model::Clip::new(
                clip,
                Ticks::ZERO,
                beat * 4,
                bl_model::ClipData::Midi(bl_model::MidiData::new([bl_model::Note::new(
                    60,
                    Ticks::ZERO,
                    beat,
                    100,
                )])),
            ),
        })
        .unwrap();
    rig.sync();
    let keys: Vec<(NoteKind, u8)> = heard(&mut rig, 256)
        .into_iter()
        .map(|(_, kind, key)| (kind, key))
        .collect();
    // One instrument per chain, each after its own chain's MIDI effect; the
    // probe reads one instrument's notes, then the next's.
    assert_eq!(
        keys,
        vec![
            (NoteKind::On, 60),
            (NoteKind::Off, 60),
            (NoteKind::On, 67),
            (NoteKind::Off, 67)
        ]
    );
}

fn made(rig: &Rig) -> u64 {
    rig.probe.made.load(std::sync::atomic::Ordering::Relaxed)
}

/// The nodes the program just swapped in has that no node of the one before
/// supplied.
fn new_nodes(rig: &mut Rig) -> usize {
    rig.block(64);
    let program = rig.processor.program().unwrap();
    program.node_count() - program.migration_count()
}

// Spec: racks R14
#[test]
fn grouping_and_chain_edits_make_only_new_nodes() {
    let mut rig = Rig::new(64);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    let a = device(&mut rig.editor, audio, THROUGH, &[]);
    let b = device(&mut rig.editor, audio, GAIN, &[(GAIN_PARAM, 0.5)]);
    let c = device(&mut rig.editor, audio, THROUGH, &[]);
    rig.sync();
    rig.block(64);
    assert_eq!(made(&rig), 4);

    // Group wraps two devices in a rack: the rack's one chain is the only
    // thing new to the set, with its mixer, its zones and the rack's chain
    // selector (WP-5.3): no device's node.
    let rack_id: DeviceId = rig.editor.new_id();
    let chain: ChainId = rig.editor.new_id();
    let grouped =
        Device::rack(rack_id, RackKind::AudioEffect).with_chain(Chain::new(chain, "Chain"));
    assert!(
        rig.apply(Command::GroupDevices {
            devices: vec![a, b],
            rack: grouped,
        })
        .program
    );
    assert_eq!(
        new_nodes(&mut rig),
        3,
        "only the chain's mixer and zones and the rack's selector are new"
    );
    assert_eq!(made(&rig), 4, "no device node was made anew");
    assert_eq!(settled(&mut rig, 64), 0.5);

    // Ungroup makes none.
    assert!(
        rig.apply(Command::UngroupDevice { device: rack_id })
            .program
    );
    assert_eq!(new_nodes(&mut rig), 0);
    assert_eq!(made(&rig), 4);

    // A rack with a chain makes the chain's mixer, its zones and the rack's
    // selector; a chain added with a device makes its mixer, its zones and
    // the device.
    let rack_id: DeviceId = rig.editor.new_id();
    let first: ChainId = rig.editor.new_id();
    rig.apply(Command::InsertDevice {
        track: audio,
        index: 4,
        device: Device::rack(rack_id, RackKind::AudioEffect).with_chain(Chain::new(first, "A")),
    });
    assert_eq!(new_nodes(&mut rig), 3);
    let second: ChainId = rig.editor.new_id();
    let tail: DeviceId = rig.editor.new_id();
    rig.apply(Command::AddChain {
        rack: rack_id,
        index: 1,
        chain: Chain::new(second, "B").with_devices(vec![Device::new(
            tail,
            "Tail",
            bl_model::DeviceKind::Native {
                device_type: THROUGH.to_owned(),
            },
        )]),
    });
    assert_eq!(new_nodes(&mut rig), 3);
    assert_eq!(made(&rig), 5);

    // Moving a device into a rack, between racks and out again, and a chain
    // within its rack make none: every device keeps its node.
    for command in [
        Command::MoveDeviceToChain {
            device: c,
            chain: first,
            index: 0,
        },
        Command::MoveDeviceToChain {
            device: c,
            chain: second,
            index: 0,
        },
        Command::MoveDevice {
            device: c,
            track: audio,
            index: 0,
        },
        Command::MoveChain {
            chain: first,
            rack: rack_id,
            index: 1,
        },
    ] {
        rig.apply(command.clone());
        assert_eq!(new_nodes(&mut rig), 0, "{command:?}");
    }
    assert_eq!(made(&rig), 5);
    // Edits that leave the layout as it is send no program.
    for command in [
        Command::SetChainEnabled {
            chain: second,
            enabled: false,
        },
        Command::SetChainVolume {
            chain: second,
            volume: 0.5,
        },
        Command::SetChainSolo {
            chain: first,
            solo: true,
        },
        Command::RenameChain {
            chain: first,
            name: "Dry".to_owned(),
        },
        Command::SetDeviceParam {
            device: tail,
            param: ParamId(0),
            value: 0.5,
        },
    ] {
        assert!(!rig.apply(command.clone()).program, "{command:?}");
    }
    // Deleting a chain frees its nodes off the audio thread, and only its.
    rig.apply(Command::RemoveChain { chain: second });
    rig.block(64);
    rig.collector.collect();
    assert_eq!(
        rig.probe.dropped.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "only the removed chain's device goes"
    );
}

// Spec: racks R14
#[test]
fn grouping_keeps_a_devices_state_running() {
    // A DC that keeps counting through a group is the same node: a counter
    // would show it, but its audio is enough: a delay's tail rings on.
    use bl_engine::testing::{DELAY, DELAY_FRAMES};
    let mut rig = Rig::new(64);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    let delay = device(&mut rig.editor, audio, DELAY, &[(DELAY_FRAMES, 200.0)]);
    rig.sync();
    // The delay holds 200 frames of the DC's 1.0 once it has run for a
    // while; swap the source off so only what the delay holds remains.
    rig.render(1000, 64);
    let rack_id: DeviceId = rig.editor.new_id();
    let chain: ChainId = rig.editor.new_id();
    rig.apply(Command::GroupDevices {
        devices: vec![delay],
        rack: Device::rack(rack_id, RackKind::AudioEffect).with_chain(Chain::new(chain, "Chain")),
    });
    let out = rig.render(300, 64);
    assert!(
        out.iter().all(|s| *s == 1.0),
        "the delay's line carried on through the regrouping"
    );
}

// Spec: racks R7. A rack that is off passes its input through, and what is
// set inside it meanwhile is there when it comes back on.
#[test]
fn a_switched_off_rack_passes_through_and_takes_changes_while_off() {
    let (mut rig, _, rack_id, chains) = setup(1.0, RackKind::AudioEffect, 2);
    let gain = effect_in(&mut rig.editor, chains[0], GAIN, &[(GAIN_PARAM, 0.5)]);
    effect_in(&mut rig.editor, chains[1], THROUGH, &[]);
    rig.sync();
    assert_eq!(settled(&mut rig, 64), 1.5);
    rig.apply(Command::SetDeviceEnabled {
        device: rack_id,
        enabled: false,
    });
    assert_eq!(settled(&mut rig, 64), 1.0, "the input, untouched");
    // Changes to a device, and to a chain's mixer, while it is off.
    rig.apply(Command::SetDeviceParam {
        device: gain,
        param: GAIN_PARAM,
        value: 2.0,
    });
    rig.apply(Command::SetChainVolume {
        chain: chains[1],
        volume: 0.25,
    });
    assert_eq!(settled(&mut rig, 64), 1.0);
    rig.apply(Command::SetDeviceEnabled {
        device: rack_id,
        enabled: true,
    });
    assert_eq!(
        settled(&mut rig, 64),
        2.25,
        "2 from the gain, .25 from the dry chain"
    );
}
