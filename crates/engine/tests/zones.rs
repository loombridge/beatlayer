//! Rack zones in the engine (WP-5.3; the racks spec, R30 to R34 and R37):
//! each chain's key, velocity and chain select zones filter the notes that
//! reach it, a note-off follows its note-on, the chain selector gates an
//! audio effect rack's chains and scales their outputs across their fades,
//! and the engine publishes which chains are processing. Every block runs
//! as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{INSTRUMENT, SEMITONES, THROUGH, TRANSPOSE};
use bl_engine::{Block, Node, NoteEvent, NoteKind, TransportCommand};
use bl_model::{
    AutomationTarget, CHAIN_SELECTOR, ChainId, Clip, ClipData, Command, Device, DeviceId,
    DeviceKind, DeviceRole, FILTER_MIDI_CTRL, MidiData, Note, RackKind, TrackId, TrackKind, Zone,
    ZoneKind,
};
use common::{Rig, dc, effect_in, in_chain, rack, track};

const BEAT: i64 = 24_000;

/// Ticks for a number of beats.
fn beats(beats: f64) -> Ticks {
    Ticks((beats * TICKS_PER_QUARTER as f64).round() as i64)
}

/// What a chain's instrument heard: which chain, the key the clip played,
/// what it did, and its velocity as 0 to 127.
type Heard = (usize, u8, NoteKind, u8);

/// How far each chain's transpose moves the keys, which tells the chains'
/// notes apart in the probe.
const SHIFT: u8 = 40;

/// A MIDI track with an instrument rack of `chains` chains, each a
/// transposition by its number times [`SHIFT`] and an instrument.
fn instrument_rack(chains: usize) -> (Rig, TrackId, DeviceId, Vec<ChainId>) {
    let mut rig = Rig::new(4096);
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    let (rack, ids) = rack(&mut rig.editor, midi, RackKind::Instrument, chains);
    for (n, chain) in ids.iter().enumerate() {
        in_chain(
            &mut rig.editor,
            *chain,
            TRANSPOSE,
            DeviceRole::MidiEffect,
            &[(SEMITONES, f64::from(SHIFT) * n as f64)],
        );
        in_chain(
            &mut rig.editor,
            *chain,
            INSTRUMENT,
            DeviceRole::Instrument,
            &[],
        );
    }
    (rig, midi, rack, ids)
}

/// A clip of notes `(key, start beat, length in beats, velocity)` on `track`.
fn notes(rig: &mut Rig, track: TrackId, notes: &[(u8, f64, f64, u8)]) {
    let end = notes.iter().map(|n| n.1 + n.2).fold(1.0, f64::max).ceil();
    let data =
        MidiData::new(notes.iter().map(|&(key, at, length, velocity)| {
            Note::new(key, beats(at), beats(length), velocity)
        }));
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(id, Ticks::ZERO, beats(end), ClipData::Midi(data)),
        })
        .unwrap();
}

fn zone(rig: &mut Rig, chain: ChainId, kind: ZoneKind, zone: Zone) {
    rig.editor
        .apply(Command::SetChainZone { chain, kind, zone })
        .unwrap();
}

/// What the instruments heard so far, by chain.
fn listen(rig: &Rig) -> Vec<Heard> {
    rig.probe
        .recorded_notes()
        .iter()
        .map(|n| {
            (
                usize::from(n.event.key / SHIFT),
                n.event.key % SHIFT,
                n.event.kind,
                (n.event.velocity * 127.0).round() as u8,
            )
        })
        .collect()
}

/// The notes a chain heard, as (key, kind, velocity).
fn of(heard: &[Heard], chain: usize) -> Vec<(u8, NoteKind, u8)> {
    heard
        .iter()
        .filter(|h| h.0 == chain)
        .map(|h| (h.1, h.2, h.3))
        .collect()
}

fn on(key: u8, velocity: u8) -> (u8, NoteKind, u8) {
    (key, NoteKind::On, velocity)
}

fn off(key: u8, velocity: u8) -> (u8, NoteKind, u8) {
    (key, NoteKind::Off, velocity)
}

fn play(rig: &mut Rig, frames: usize, size: usize) {
    rig.sender.transport(TransportCommand::Play);
    rig.render(frames, size);
}

// Spec: racks R31
#[test]
fn a_note_passes_key_then_velocity_then_chain_select() {
    for size in [64, 512, 4096] {
        let (mut rig, midi, rack_id, chains) = instrument_rack(4);
        // A new chain passes everything while the selector is 0; the others
        // each stop some notes.
        zone(&mut rig, chains[1], ZoneKind::Key, Zone::new(20, 24));
        zone(&mut rig, chains[2], ZoneKind::Velocity, Zone::new(1, 60));
        zone(&mut rig, chains[3], ZoneKind::Select, Zone::new(1, 1));
        notes(
            &mut rig,
            midi,
            &[(20, 0.0, 0.5, 100), (28, 1.0, 0.5, 100), (20, 2.0, 0.5, 40)],
        );
        rig.sync();
        play(&mut rig, 3 * BEAT as usize, size);
        let heard = listen(&rig);
        assert_eq!(
            of(&heard, 0),
            [
                on(20, 100),
                off(20, 64),
                on(28, 100),
                off(28, 64),
                on(20, 40),
                off(20, 64)
            ],
            "block size {size}"
        );
        assert_eq!(
            of(&heard, 1),
            [on(20, 100), off(20, 64), on(20, 40), off(20, 64)],
            "the key zone stops key 28, at block size {size}"
        );
        assert_eq!(
            of(&heard, 2),
            [on(20, 40), off(20, 64)],
            "the velocity zone stops velocity 100, at block size {size}"
        );
        assert_eq!(of(&heard, 3), [], "the chain select zone doesn't hold 0");

        // The selector moves to 1: the last chain takes every note and the
        // first none.
        rig.apply(Command::SetDeviceParam {
            device: rack_id,
            param: CHAIN_SELECTOR,
            value: 1.0,
        });
        rig.sender.transport(TransportCommand::Stop);
        rig.render(64, size);
        let _ = rig.probe.recorded_notes();
        play(&mut rig, 3 * BEAT as usize, size);
        let heard = listen(&rig);
        assert_eq!(of(&heard, 0), [], "the first chain's zone is 0 alone");
        assert_eq!(of(&heard, 3).len(), 6);
    }
}

// Spec: racks R31
#[test]
fn a_note_off_reaches_the_chains_its_note_on_did_after_the_zone_moves() {
    for size in [1, 64, 4096] {
        let (mut rig, midi, _, chains) = instrument_rack(2);
        zone(&mut rig, chains[1], ZoneKind::Key, Zone::new(100, 110));
        notes(&mut rig, midi, &[(20, 0.0, 1.0, 100), (20, 1.0, 1.0, 100)]);
        rig.sync();
        rig.sender.transport(TransportCommand::Play);
        rig.render(BEAT as usize / 2, size);
        // The held note's chain no longer holds it, and the other chain now
        // does.
        rig.apply(Command::SetChainZone {
            chain: chains[0],
            kind: ZoneKind::Key,
            zone: Zone::new(100, 110),
        });
        rig.apply(Command::SetChainZone {
            chain: chains[1],
            kind: ZoneKind::Key,
            zone: Zone::new(0, 127),
        });
        rig.render(2 * BEAT as usize, size);
        let heard = listen(&rig);
        assert_eq!(
            of(&heard, 0),
            [on(20, 100), off(20, 64)],
            "the first note ends where it started, the second never starts, at block size {size}"
        );
        assert_eq!(
            of(&heard, 1),
            [on(20, 100), off(20, 64)],
            "the second note starts on the other chain and ends there, and the first note's \
             end doesn't reach it, at block size {size}"
        );
    }
}

/// A test MIDI effect on the track that sends a control change of its
/// controller ahead of each note-on, so a rack's input holds messages.
struct Controller;

impl Node for Controller {
    fn process(&mut self, block: &mut Block<'_>) {
        for &note in block.notes() {
            if note.kind == NoteKind::On {
                block.send_note(NoteEvent {
                    kind: NoteKind::Control {
                        controller: 1,
                        value: 1.0,
                    },
                    key: 0,
                    ..note
                });
            }
            block.send_note(note);
        }
    }
}

// Spec: racks R31
#[test]
fn chain_selector_filters_midi_ctrl_filters_a_control_change() {
    let message = |heard: &[Heard], chain| {
        heard
            .iter()
            .filter(|h| h.0 == chain && matches!(h.2, NoteKind::Control { .. }))
            .count()
    };
    for size in [64, 4096] {
        let mut rig = Rig::with_registry(4096, |registry| {
            registry.add_native("test.controller", |_, _| Box::new(Controller));
        });
        let midi = track(&mut rig.editor, TrackKind::Midi, None);
        let controller = rig.editor.new_id();
        rig.editor
            .apply(Command::InsertDevice {
                track: midi,
                index: 0,
                device: Device::new(
                    controller,
                    "",
                    DeviceKind::Native {
                        device_type: "test.controller".to_owned(),
                    },
                )
                .with_role(DeviceRole::MidiEffect),
            })
            .unwrap();
        let (rack_id, chains) = rack(&mut rig.editor, midi, RackKind::Instrument, 2);
        for (n, chain) in chains.iter().enumerate() {
            in_chain(
                &mut rig.editor,
                *chain,
                TRANSPOSE,
                DeviceRole::MidiEffect,
                &[(SEMITONES, f64::from(SHIFT) * n as f64)],
            );
            in_chain(
                &mut rig.editor,
                *chain,
                INSTRUMENT,
                DeviceRole::Instrument,
                &[],
            );
        }
        // The second chain's chain select zone doesn't hold the selector.
        zone(&mut rig, chains[1], ZoneKind::Select, Zone::new(5, 5));
        notes(&mut rig, midi, &[(20, 0.0, 0.5, 100)]);
        rig.sync();
        play(&mut rig, BEAT as usize, size);
        let first = listen(&rig);
        assert_eq!(message(&first, 0), 1, "a message reaches both chains");
        assert_eq!(message(&first, 1), 1, "when the switch is off");

        rig.apply(Command::SetDeviceParam {
            device: rack_id,
            param: FILTER_MIDI_CTRL,
            value: 1.0,
        });
        rig.sender.transport(TransportCommand::Stop);
        rig.render(64, size);
        let _ = rig.probe.recorded_notes();
        play(&mut rig, BEAT as usize, size);
        let second = listen(&rig);
        assert_eq!(message(&second, 0), 1);
        assert_eq!(
            message(&second, 1),
            0,
            "the zone stops it with the switch on"
        );
    }
}

/// R32's share at `value` of a zone from `low` to `high` with fades of
/// `fade_low` and `fade_high` values, worked out as the rule says and not by
/// the model's own function: the distance from the silent edge over the
/// fade's span.
fn share(low: u8, high: u8, fade_low: u8, fade_high: u8, value: u8) -> Option<f64> {
    if value < low || value > high {
        return None;
    }
    Some(if fade_low > 0 && value - low < fade_low {
        f64::from(value - low) / f64::from(fade_low)
    } else if fade_high > 0 && high - value < fade_high {
        f64::from(high - value) / f64::from(fade_high)
    } else {
        1.0
    })
}

/// The velocity a note of `velocity` reaches a chain with, scaled by the
/// shares (R32): rounded, and never below 1.
fn scaled(velocity: u8, shares: &[f64]) -> u8 {
    let share: f64 = shares.iter().product();
    ((f64::from(velocity) * share).round() as u8).max(1)
}

// Spec: racks R32
#[test]
fn fades_scale_velocity_in_a_straight_line() {
    // Every key and velocity of a zone with fades: the zone's own, and the
    // two together. A property over the fades the zones can have.
    let fades = [(0, 0), (1, 0), (0, 1), (7, 9), (17, 17), (34, 0), (0, 34)];
    for (fade_low, fade_high) in fades {
        let (mut rig, midi, _, chains) = instrument_rack(1);
        zone(
            &mut rig,
            chains[0],
            ZoneKind::Key,
            Zone {
                low: 5,
                high: 39,
                fade_low,
                fade_high,
            },
        );
        let played: Vec<_> = (0..SHIFT)
            .map(|key| (key, f64::from(key) * 0.25, 0.125, 100))
            .collect();
        notes(&mut rig, midi, &played);
        rig.sync();
        play(&mut rig, 11 * BEAT as usize, 512);
        let heard: Vec<_> = of(&listen(&rig), 0)
            .into_iter()
            .filter(|n| n.1 == NoteKind::On)
            .collect();
        let expected: Vec<_> = (0..SHIFT)
            .filter_map(|key| {
                share(5, 39, fade_low, fade_high, key).map(|s| on(key, scaled(100, &[s])))
            })
            .collect();
        assert_eq!(heard, expected, "key fades {fade_low} and {fade_high}");
    }

    let (mut rig, midi, _, chains) = instrument_rack(1);
    zone(
        &mut rig,
        chains[0],
        ZoneKind::Velocity,
        Zone {
            low: 11,
            high: 120,
            fade_low: 20,
            fade_high: 30,
        },
    );
    let played: Vec<_> = (1..=127u8)
        .map(|velocity| (10, f64::from(velocity) * 0.25, 0.125, velocity))
        .collect();
    notes(&mut rig, midi, &played);
    rig.sync();
    play(&mut rig, 33 * BEAT as usize, 4096);
    let heard: Vec<_> = of(&listen(&rig), 0)
        .into_iter()
        .filter(|n| n.1 == NoteKind::On)
        .collect();
    let expected: Vec<_> = (1..=127u8)
        .filter_map(|v| share(11, 120, 20, 30, v).map(|s| on(10, scaled(v, &[s]))))
        .collect();
    assert_eq!(heard, expected, "velocity fades");

    // A note in both a key fade and a velocity fade takes both shares.
    let (mut rig, midi, _, chains) = instrument_rack(1);
    zone(
        &mut rig,
        chains[0],
        ZoneKind::Key,
        Zone {
            low: 5,
            high: 39,
            fade_low: 6,
            fade_high: 8,
        },
    );
    zone(
        &mut rig,
        chains[0],
        ZoneKind::Velocity,
        Zone {
            low: 11,
            high: 120,
            fade_low: 20,
            fade_high: 30,
        },
    );
    let played: Vec<_> = (0..SHIFT)
        .map(|key| (key, f64::from(key) * 0.25, 0.125, 25))
        .collect();
    notes(&mut rig, midi, &played);
    rig.sync();
    play(&mut rig, 11 * BEAT as usize, 512);
    let heard: Vec<_> = of(&listen(&rig), 0)
        .into_iter()
        .filter(|n| n.1 == NoteKind::On)
        .collect();
    let velocity = share(11, 120, 20, 30, 25).unwrap();
    let expected: Vec<_> = (0..SHIFT)
        .filter_map(|key| share(5, 39, 6, 8, key).map(|s| on(key, scaled(25, &[s, velocity]))))
        .collect();
    assert_eq!(heard, expected);
}

// Spec: racks R33
#[test]
fn the_chain_selector_gates_an_audio_effect_racks_inputs_and_lets_tails_ring() {
    /// A one-pole smoother with a long memory: a tail that rings on after
    /// its input stops.
    struct Leak(f32);
    impl Node for Leak {
        fn process(&mut self, block: &mut Block<'_>) {
            for c in 0..block.channels() {
                let mut y = self.0;
                for sample in block.channel_mut(c) {
                    y = y * 0.999 + *sample * 0.001;
                    *sample = y;
                }
                if c == 0 {
                    self.0 = y;
                }
            }
        }
    }
    for size in [1, 64, 4096] {
        let mut rig = Rig::with_registry(4096, |registry| {
            registry.add_native("test.leak", |_, _| Box::new(Leak(0.0)));
        });
        let audio = track(&mut rig.editor, TrackKind::Audio, None);
        dc(&mut rig.editor, audio, 1.0);
        let (rack_id, chains) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 2);
        effect_in(&mut rig.editor, chains[0], "test.leak", &[]);
        zone(&mut rig, chains[1], ZoneKind::Select, Zone::new(1, 1));
        rig.sync();
        // Selected, the leaking chain settles at the input.
        rig.render(30_000, 4096);
        let out = rig.render(64, size);
        assert!((out[63] - 1.0).abs() < 1e-3, "{}", out[63]);

        rig.apply(Command::SetDeviceParam {
            device: rack_id,
            param: CHAIN_SELECTOR,
            value: 1.0,
        });
        let out = rig.render(8_000, size);
        // The dry chain opens over 20 ms while the leaking chain's input
        // closes; what it sounds decays on its own, with no fade to silence
        // it: the sum is above the dry chain's 1 while the tail rings.
        assert!(out[2_000] > 1.1, "{} at block size {size}", out[2_000]);
        assert!(out[7_999] < out[2_000]);
        let out = rig.render(40_000, size);
        assert!((out[39_999] - 1.0).abs() < 1e-3, "{}", out[39_999]);
    }
}

// Spec: racks R34
#[test]
fn chain_select_fades_crossfade_the_outputs() {
    // Two chains over the selector's 0 to 10: one fades out, the other in.
    let mut rig = Rig::new(4096);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    let (rack_id, chains) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 2);
    for chain in &chains {
        effect_in(&mut rig.editor, *chain, THROUGH, &[]);
    }
    zone(
        &mut rig,
        chains[0],
        ZoneKind::Select,
        Zone {
            low: 0,
            high: 10,
            fade_low: 0,
            fade_high: 10,
        },
    );
    zone(
        &mut rig,
        chains[1],
        ZoneKind::Select,
        Zone {
            low: 0,
            high: 10,
            fade_low: 10,
            fade_high: 0,
        },
    );
    // The selector, automated from 0 to 10 over a beat.
    for (at, value) in [(Ticks::ZERO, 0.0), (beats(1.0), 10.0)] {
        rig.editor
            .apply(Command::SetAutomationBreakpoint {
                track: audio,
                target: AutomationTarget::Device {
                    device: rack_id,
                    param: CHAIN_SELECTOR,
                },
                at,
                value,
            })
            .unwrap();
    }
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(BEAT as usize, 64);
    // The two shares always add to the input, and the input is DC: a
    // crossfade that never dips.
    for (at, sample) in out.iter().enumerate().skip(2_000) {
        assert!((sample - 1.0).abs() < 1e-4, "frame {at}: {sample}");
    }
    // With the second chain off, the first shows its own share of the
    // selector as it passes through its fade.
    rig.sender.transport(TransportCommand::Stop);
    rig.apply(Command::SetChainEnabled {
        chain: chains[1],
        enabled: false,
    });
    rig.render(4_800, 4096);
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(BEAT as usize, 64);
    for fraction in [0.2f64, 0.5, 0.8] {
        let at = (fraction * BEAT as f64) as usize;
        let selector = (fraction * 10.0).round();
        let expected = (10.0 - selector) / 10.0;
        assert!(
            (f64::from(out[at]) - expected).abs() < 0.06,
            "frame {at}: {} against {expected}",
            out[at]
        );
    }
}

// Spec: racks R37
#[test]
fn the_engine_publishes_the_chains_each_rack_is_processing() {
    let (mut rig, midi, _, chains) = instrument_rack(2);
    zone(&mut rig, chains[1], ZoneKind::Key, Zone::new(100, 110));
    notes(&mut rig, midi, &[(20, 0.0, 1.0, 100)]);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    rig.render(BEAT as usize / 2, 256);
    // Only the chain a held note reached is processing.
    assert_eq!(rig.monitor.state().processing, [chains[0]]);
    rig.render(BEAT as usize, 256);
    assert_eq!(rig.monitor.state().processing, [], "the note has ended");

    // An audio effect rack's chains are processing while the selector is in
    // their chain select zones.
    let mut rig = Rig::new(256);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    let (rack_id, chains) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 2);
    zone(&mut rig, chains[1], ZoneKind::Select, Zone::new(1, 1));
    rig.sync();
    rig.render(256, 256);
    assert_eq!(rig.monitor.state().processing, [chains[0]]);
    rig.apply(Command::SetDeviceParam {
        device: rack_id,
        param: CHAIN_SELECTOR,
        value: 1.0,
    });
    rig.render(256, 256);
    assert_eq!(rig.monitor.state().processing, [chains[1]]);
}

// Spec: racks R36
#[test]
fn a_zone_edit_sends_parameter_changes_and_no_program() {
    let (mut rig, midi, rack_id, chains) = instrument_rack(2);
    notes(&mut rig, midi, &[(20, 0.0, 1.0, 100)]);
    rig.sync();
    rig.render(64, 64);
    let sent = rig.apply(Command::SetChainZone {
        chain: chains[0],
        kind: ZoneKind::Velocity,
        zone: Zone {
            low: 10,
            high: 100,
            fade_low: 5,
            fade_high: 0,
        },
    });
    assert!(!sent.program, "no new program");
    assert_eq!(sent.params, 3, "the three numbers that changed");
    let sent = rig.apply(Command::SetDeviceParam {
        device: rack_id,
        param: CHAIN_SELECTOR,
        value: 3.0,
    });
    assert!(!sent.program);
    assert_eq!(sent.params, 1);
    rig.render(64, 64);
    assert_eq!(rig.monitor.state().counters.events_unrouted, 0);
}

// Spec: racks R30
#[test]
fn a_drum_racks_selector_stays_at_zero() {
    let mut rig = Rig::new(256);
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    let (rack_id, chains) = rack(&mut rig.editor, midi, RackKind::Drum, 1);
    in_chain(
        &mut rig.editor,
        chains[0],
        INSTRUMENT,
        DeviceRole::Instrument,
        &[],
    );
    // The chain's Receive note (drum racks spec, R1), which it answers.
    notes(&mut rig, midi, &[(36, 0.0, 0.5, 100)]);
    rig.apply(Command::SetDeviceParam {
        device: rack_id,
        param: CHAIN_SELECTOR,
        value: 9.0,
    });
    play(&mut rig, BEAT as usize, 256);
    assert_eq!(listen(&rig).len(), 2, "the note on and off still arrive");
}
