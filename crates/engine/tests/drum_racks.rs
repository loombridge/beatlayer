//! Drum racks in the engine (WP-5.6; the drum racks spec, R2 to R9 and
//! R28): notes reach the drum chains that answer them, at their Play notes,
//! on the frames they fall on; a choke ends the others of its group on its
//! frame; sends feed return chains after the chains' mixers, and the return
//! chains sum into the rack's output or reach a return track. Every block
//! runs as a real-time section under the allocation trap, at block sizes
//! 1, 64, 512 and 4,096.

bl_rt::install_trap!();

mod common;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{DC, GAIN, GAIN_PARAM, INSTRUMENT, VALUE};
use bl_engine::{NoteKind, TransportCommand};
use bl_model::{ChainId, Command, DeviceRole, Receive, TrackId, TrackKind};
use common::{Rig, drum_rack, effect_in, in_chain, track};

const SIZES: [usize; 4] = [1, 64, 512, 4096];
/// Frames enough for a 20 ms glide at 48 kHz to land, in the blocks of any
/// size.
const SETTLE: usize = 4096;
/// A beat at 120 BPM and 48 kHz.
const BEAT: i64 = 24_000;

/// A MIDI track holding a drum rack of `drums` chains from note 36 with a
/// test instrument in each, and a clip of `notes` as `(key, beat, beats)`.
fn kit(size: usize, drums: usize, notes: &[(u8, i64, i64)]) -> (Rig, TrackId, Vec<ChainId>) {
    let mut rig = Rig::new(size.max(256));
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    let (_, chains, _) = drum_rack(&mut rig.editor, midi, drums, 0);
    for chain in &chains {
        in_chain(
            &mut rig.editor,
            *chain,
            INSTRUMENT,
            DeviceRole::Instrument,
            &[],
        );
    }
    let beat = Ticks(TICKS_PER_QUARTER);
    let notes = notes
        .iter()
        .map(|&(key, at, length)| bl_model::Note::new(key, beat * at, beat * length, 100));
    let clip = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: midi,
            clip: bl_model::Clip::new(
                clip,
                Ticks::ZERO,
                beat * 8,
                bl_model::ClipData::Midi(bl_model::MidiData::new(notes)),
            ),
        })
        .unwrap();
    rig.sync();
    (rig, midi, chains)
}

/// What the instruments heard of two seconds of playing: frame, kind, key,
/// instrument by instrument in the order they were made.
fn heard(rig: &mut Rig, size: usize) -> Vec<(i64, NoteKind, u8)> {
    rig.sender.transport(TransportCommand::Play);
    rig.render(96_000, size);
    rig.probe
        .recorded_notes()
        .iter()
        .map(|n| (n.frame, n.event.kind, n.event.key))
        .collect()
}

fn set(rig: &mut Rig, command: Command) {
    rig.apply(command);
}

// Spec: drum-racks R2, R28
#[test]
fn a_note_reaches_its_chains_at_their_play_note() {
    for size in SIZES {
        let (mut rig, _, chains) = kit(size, 3, &[(36, 0, 1), (37, 1, 1)]);
        set(
            &mut rig,
            Command::SetChainPlay {
                chain: chains[0],
                play: 62,
            },
        );
        // The third chain answers the first one's note too.
        set(
            &mut rig,
            Command::SetChainReceive {
                chain: chains[2],
                receive: Receive::Note(36),
            },
        );
        let got = heard(&mut rig, size);
        assert_eq!(
            got,
            vec![
                (0, NoteKind::On, 62),
                (BEAT, NoteKind::Off, 62),
                (BEAT, NoteKind::On, 60),
                (2 * BEAT, NoteKind::Off, 60),
                (0, NoteKind::On, 60),
                (BEAT, NoteKind::Off, 60),
            ],
            "block size {size}: chain 0 plays 62 for 36, chain 1 answers 37, chain 2 answers 36"
        );
    }
}

// Spec: drum-racks R2, R28
#[test]
fn all_notes_passes_every_note_as_it_came() {
    for size in SIZES {
        let (mut rig, _, chains) = kit(size, 2, &[(36, 0, 1), (50, 1, 1)]);
        set(
            &mut rig,
            Command::SetChainReceive {
                chain: chains[1],
                receive: Receive::AllNotes,
            },
        );
        let got = heard(&mut rig, size);
        assert_eq!(
            got,
            vec![
                (0, NoteKind::On, 60),
                (BEAT, NoteKind::Off, 60),
                (0, NoteKind::On, 36),
                (BEAT, NoteKind::Off, 36),
                (BEAT, NoteKind::On, 50),
                (2 * BEAT, NoteKind::Off, 50),
            ],
            "block size {size}"
        );
    }
}

// Spec: drum-racks R2
#[test]
fn a_note_off_follows_its_note_on() {
    // The chain's Receive changes while the note is held: the note-off still
    // reaches it.
    let (mut rig, _, chains) = kit(64, 1, &[(36, 0, 4)]);
    rig.sender.transport(TransportCommand::Play);
    rig.render(BEAT as usize, 64);
    set(
        &mut rig,
        Command::SetChainReceive {
            chain: chains[0],
            receive: Receive::Note(90),
        },
    );
    rig.render(5 * BEAT as usize, 64);
    let got: Vec<_> = rig
        .probe
        .recorded_notes()
        .iter()
        .map(|n| (n.frame, n.event.kind, n.event.key))
        .collect();
    assert_eq!(
        got,
        vec![(0, NoteKind::On, 60), (4 * BEAT, NoteKind::Off, 60)]
    );
}

// Spec: drum-racks R4, R28
#[test]
fn a_choke_group_fades_the_others_within_five_ms() {
    for size in SIZES {
        // An open hi-hat on 46 from the start; the closed one on 42 a beat
        // later; a snare on 38 in no group.
        let (mut rig, _, chains) = kit(size, 3, &[(46, 0, 3), (42, 1, 1), (38, 1, 1)]);
        for (chain, note, group) in [(0, 46, Some(1)), (1, 42, Some(1)), (2, 38, None)] {
            set(
                &mut rig,
                Command::SetChainReceive {
                    chain: chains[chain],
                    receive: Receive::Note(note),
                },
            );
            set(
                &mut rig,
                Command::SetChainChoke {
                    chain: chains[chain],
                    choke: group,
                },
            );
        }
        set(
            &mut rig,
            Command::SetChainPlay {
                chain: chains[1],
                play: 61,
            },
        );
        let got = heard(&mut rig, size);
        // The open hi-hat is choked on the closed one's frame: the choke
        // arrives ahead of the note-off the clip sends later, at the key the
        // open chain played. The closed one is not choked, nor the snare.
        let open: Vec<_> = got.iter().take(3).copied().collect();
        assert_eq!(
            open,
            vec![
                (0, NoteKind::On, 60),
                (BEAT, NoteKind::Choke, 60),
                (3 * BEAT, NoteKind::Off, 60),
            ],
            "block size {size}"
        );
        let rest: Vec<_> = got.iter().skip(3).copied().collect();
        assert_eq!(
            rest,
            vec![
                (BEAT, NoteKind::On, 61),
                (2 * BEAT, NoteKind::Off, 61),
                (BEAT, NoteKind::On, 60),
                (2 * BEAT, NoteKind::Off, 60),
            ],
            "block size {size}"
        );
    }
}

// Spec: drum-racks R4
#[test]
fn a_choke_goes_out_on_the_channel_its_note_came_on() {
    use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
    use std::time::{Duration, Instant};
    const BLOCK: usize = 512;
    let (mut rig, midi, chains) = kit(BLOCK, 2, &[]);
    for (chain, note) in [(0, 46), (1, 42)] {
        set(
            &mut rig,
            Command::SetChainReceive {
                chain: chains[chain],
                receive: Receive::Note(note),
            },
        );
        set(
            &mut rig,
            Command::SetChainChoke {
                chain: chains[chain],
                choke: Some(1),
            },
        );
    }
    set(
        &mut rig,
        Command::SetArm {
            track: midi,
            arm: true,
        },
    );
    let (mut queue, consumer) = rtrb::RingBuffer::new(16);
    assert!(rig.processor.set_midi_input(consumer).is_none());
    // A pad controller on MIDI channel 10: the open hi-hat, then the
    // closed one a block later.
    let start = Instant::now();
    let at = |block: u32| start + Duration::from_nanos(u64::from(block) * BLOCK as u64 * 20_833);
    for (block, key) in [(0, 46), (1, 42)] {
        queue
            .push(MidiInputEvent {
                source: MidiSource::ComputerKeyboard,
                time: at(block) + Duration::from_millis(1),
                message: MidiMessage::note_on(9, key, 100),
            })
            .unwrap();
    }
    for block in 0..4 {
        let processor = &mut rig.processor;
        bl_rt::rt_test!({
            processor.process_at(BLOCK, &bl_engine::NoInput, at(block));
        });
    }
    let choke: Vec<_> = rig
        .probe
        .recorded_notes()
        .iter()
        .filter(|n| n.event.kind == NoteKind::Choke)
        .map(|n| (n.event.channel, n.event.key))
        .collect();
    assert_eq!(choke, vec![(9, 60)]);
}

// Spec: drum-racks R4
#[test]
fn a_chain_does_not_choke_itself() {
    let (mut rig, _, chains) = kit(64, 1, &[(36, 0, 1), (36, 2, 1)]);
    set(
        &mut rig,
        Command::SetChainChoke {
            chain: chains[0],
            choke: Some(3),
        },
    );
    let got = heard(&mut rig, 64);
    assert!(got.iter().all(|n| n.1 != NoteKind::Choke), "{got:?}");
}

/// A MIDI track holding a drum rack of `drums` chains that each carry a DC
/// source of 1, and `returns` return chains.
fn audio_kit(drums: usize, returns: usize) -> (Rig, TrackId, Vec<ChainId>, Vec<ChainId>) {
    let mut rig = Rig::new(4096);
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    let (_, chains, returns) = drum_rack(&mut rig.editor, midi, drums, returns);
    for chain in &chains {
        effect_in(&mut rig.editor, *chain, DC, &[(VALUE, 1.0)]);
    }
    (rig, midi, chains, returns)
}

/// The level a rig settles at, which must be the same on every frame.
fn settled(rig: &mut Rig, size: usize) -> f32 {
    rig.render(SETTLE, size);
    let out = rig.render(size.max(64), size);
    assert!(out.iter().all(|s| *s == out[0]), "{out:?}");
    out[0]
}

fn send(rig: &mut Rig, chain: ChainId, to: ChainId, level: f64) {
    set(rig, Command::SetChainSend { chain, to, level });
}

// Spec: drum-racks R6
#[test]
fn sends_are_post_fader_and_start_silent() {
    for size in SIZES {
        let (mut rig, _, chains, returns) = audio_kit(2, 1);
        effect_in(&mut rig.editor, returns[0], GAIN, &[(GAIN_PARAM, 0.5)]);
        rig.sync();
        assert_eq!(
            settled(&mut rig, size),
            2.0,
            "block size {size}: a send from −∞ dB feeds nothing"
        );
        send(&mut rig, chains[0], returns[0], 0.5);
        assert_eq!(
            settled(&mut rig, size),
            2.0 + 0.5 * 0.5,
            "block size {size}: half of chain 0 into the return, which halves it"
        );
        // Post-fader: the chain's volume scales the send.
        set(
            &mut rig,
            Command::SetChainVolume {
                chain: chains[0],
                volume: 0.5,
            },
        );
        assert_eq!(
            settled(&mut rig, size),
            0.5 + 1.0 + 0.5 * 0.5 * 0.5,
            "block size {size}"
        );
    }
}

// Spec: drum-racks R7
#[test]
fn a_return_chain_sums_its_sends() {
    for size in SIZES {
        let (mut rig, _, chains, returns) = audio_kit(2, 2);
        effect_in(&mut rig.editor, returns[0], GAIN, &[(GAIN_PARAM, 0.5)]);
        rig.sync();
        send(&mut rig, chains[0], returns[0], 1.0);
        send(&mut rig, chains[1], returns[0], 0.5);
        send(&mut rig, chains[1], returns[1], 1.0);
        // Return 0 takes 1 + 0.5 and halves it; return 1 is dry.
        assert_eq!(
            settled(&mut rig, size),
            2.0 + 0.75 + 1.0,
            "block size {size}"
        );
    }
}

// Spec: drum-racks R9
#[test]
fn soloed_chains_keep_their_returns() {
    for size in SIZES {
        let (mut rig, _, chains, returns) = audio_kit(2, 1);
        effect_in(&mut rig.editor, returns[0], GAIN, &[(GAIN_PARAM, 0.5)]);
        rig.sync();
        send(&mut rig, chains[0], returns[0], 1.0);
        send(&mut rig, chains[1], returns[0], 1.0);
        assert_eq!(settled(&mut rig, size), 2.0 + 1.0, "block size {size}");
        set(
            &mut rig,
            Command::SetChainSolo {
                chain: chains[0],
                solo: true,
            },
        );
        // Chain 1 is silent, and so is its send; the return stays audible
        // with chain 0's alone.
        assert_eq!(settled(&mut rig, size), 1.0 + 0.5, "block size {size}");
    }
}

// Spec: drum-racks R8
#[test]
fn a_return_chain_feeds_the_rack_or_a_return_track() {
    for size in SIZES {
        let (mut rig, _, chains, returns) = audio_kit(2, 1);
        effect_in(&mut rig.editor, returns[0], GAIN, &[(GAIN_PARAM, 0.5)]);
        rig.sync();
        send(&mut rig, chains[0], returns[0], 1.0);
        let in_rack = settled(&mut rig, size);
        assert_eq!(in_rack, 2.0 + 0.5, "block size {size}");
        // Sent to a return track with half its volume: the rack's own
        // output loses it, and the return adds half of it at Main.
        let ret = track(&mut rig.editor, TrackKind::Return, None);
        set(
            &mut rig,
            Command::SetVolume {
                track: ret,
                volume: 0.5,
            },
        );
        set(
            &mut rig,
            Command::SetChainAudioTo {
                chain: returns[0],
                track: Some(ret),
            },
        );
        assert_eq!(
            settled(&mut rig, size),
            2.0 + 0.5 * 0.5,
            "block size {size}"
        );
        // And back into the rack.
        set(
            &mut rig,
            Command::SetChainAudioTo {
                chain: returns[0],
                track: None,
            },
        );
        assert_eq!(settled(&mut rig, size), in_rack, "block size {size}");
    }
}

// Spec: drum-racks R28. A kit of four pads with a return chain, a pan, a
// delay on one pad and a send from each, renders the same to the bit at
// every block size.
#[test]
fn a_kit_renders_the_same_at_every_block_size() {
    use bl_engine::testing::{DELAY, DELAY_FRAMES};
    let render = |size: usize| {
        let (mut rig, _, chains, returns) = audio_kit(4, 1);
        effect_in(&mut rig.editor, chains[2], DELAY, &[(DELAY_FRAMES, 123.0)]);
        effect_in(&mut rig.editor, returns[0], GAIN, &[(GAIN_PARAM, 0.5)]);
        effect_in(&mut rig.editor, returns[0], DELAY, &[(DELAY_FRAMES, 77.0)]);
        for (n, chain) in chains.iter().enumerate() {
            rig.editor
                .apply(Command::SetChainSend {
                    chain: *chain,
                    to: returns[0],
                    level: 0.25 * (n as f64 + 1.0),
                })
                .unwrap();
            rig.editor
                .apply(Command::SetChainPan {
                    chain: *chain,
                    pan: n as f64 / 4.0 - 0.3,
                })
                .unwrap();
        }
        rig.sync();
        rig.render(12_288, size)
    };
    let golden = render(64);
    assert!(golden.iter().any(|s| *s != 0.0));
    for size in [1, 512, 4096] {
        assert_eq!(render(size), golden, "block size {size}");
    }
}

// Spec: drum-racks R10. A return chain's taps carry its signal at their
// point, and a drum chain offers none.
#[test]
fn a_return_chains_taps_carry_its_signal() {
    use serde_json::{Value, json};
    let run = |rig: &mut Rig, id: &str, args: Value| {
        bl_model::registry::run(&mut rig.editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
    };
    for size in [1, 64, 512] {
        let (mut rig, source, chains, returns) = audio_kit(1, 1);
        let tapper = track(&mut rig.editor, TrackKind::Audio, None);
        effect_in(&mut rig.editor, returns[0], GAIN, &[(GAIN_PARAM, 0.5)]);
        for command in [
            Command::SetChainSend {
                chain: chains[0],
                to: returns[0],
                level: 1.0,
            },
            Command::SetChainVolume {
                chain: returns[0],
                volume: 0.5,
            },
        ] {
            rig.editor.apply(command).unwrap();
        }
        run(
            &mut rig,
            "routing.set_monitor",
            json!({ "track": tapper, "monitor": "in" }),
        );
        rig.sync();
        assert_eq!(settled(&mut rig, size), 1.25, "the rack's sum, untapped");
        for (tap, heard) in [("pre_fx", 1.0), ("post_fx", 0.5), ("post_mixer", 0.25)] {
            run(
                &mut rig,
                "routing.set_input",
                json!({ "track": tapper, "input":
                    { "type": "track", "track": source, "tap": tap, "chain": returns[0] } }),
            );
            rig.sync();
            assert_eq!(
                settled(&mut rig, size),
                1.25 + heard,
                "{tap}, block size {size}"
            );
        }
    }
}
