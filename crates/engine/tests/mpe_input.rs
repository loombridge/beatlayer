//! MPE input in the engine (WP-6.17, `docs/specs/mpe.md` R10, R11 and R13,
//! `docs/specs/midi-io.md` R27 and R28): an input port whose MPE switch is on
//! reaches a monitored track's devices as notes with IDs of their own and
//! per-note expressions, on the exact frames, at any block size, with every
//! block a real-time section under the allocation trap.

bl_rt::install_trap!();

use crate::common;

use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource, PortId};
use bl_engine::testing;
use bl_engine::{NoInput, NoteExpression, NoteKind, TransportCommand};
use bl_model::{Command, Input, Monitor, TrackId, TrackKind};
use common::{Rig, device, track};
use rtrb::{Producer, RingBuffer};

/// The block sizes every test runs at.
const SIZES: [usize; 4] = [1, 100, 512, 4096];

/// What a device heard: the frame, what it was, the key, the channel and the
/// note's ID.
type Heard = (i64, NoteKind, u8, u8, Option<u32>);

struct Play {
    rig: Rig,
    queue: Producer<MidiInputEvent>,
    start: Instant,
    size: usize,
    blocks: u64,
}

fn mpe() -> MidiSource {
    MidiSource::MpePort(PortId::from_name("Roli"))
}

fn plain() -> MidiSource {
    MidiSource::Port(PortId::from_name("Roli"))
}

impl Play {
    fn new(size: usize) -> Self {
        let mut rig = Rig::new(size);
        let (queue, consumer) = RingBuffer::new(4096);
        rig.processor.set_midi_input(consumer);
        Self {
            rig,
            queue,
            start: Instant::now(),
            size,
            blocks: 0,
        }
    }

    /// A MIDI track with a test instrument, monitoring its input.
    fn instrument(&mut self) -> TrackId {
        let id = track(&mut self.rig.editor, TrackKind::Midi, None);
        device(&mut self.rig.editor, id, testing::INSTRUMENT, &[]);
        self.rig.apply(Command::SetMonitor {
            track: id,
            monitor: Monitor::In,
        });
        id
    }

    fn play(&mut self) {
        self.rig.sender.transport(TransportCommand::Play);
        self.block();
    }

    fn block_time(&self, n: u64) -> Instant {
        let frames = n * self.size as u64;
        self.start + Duration::from_nanos(frames * 1_000_000_000 / 48_000)
    }

    fn block(&mut self) {
        let now = self.block_time(self.blocks);
        let processor = &mut self.rig.processor;
        bl_rt::rt_test!({
            processor.process_at(self.size, &NoInput, now);
        });
        self.blocks += 1;
    }

    fn run_to(&mut self, frame: u64) {
        while self.blocks * (self.size as u64) <= frame {
            self.block();
        }
    }

    /// Queues `bytes` to land on engine frame `frame` (as the other input
    /// tests do).
    fn send(&mut self, source: MidiSource, frame: u64, bytes: &[u8]) {
        let size = self.size as u64;
        assert!(frame >= size, "frame {frame} is in the first block");
        let time = self.block_time(frame / size - 1)
            + Duration::from_nanos((frame % size) * 1_000_000_000 / 48_000 + 100);
        self.queue
            .push(MidiInputEvent {
                source,
                time,
                message: MidiMessage::from_bytes(bytes).unwrap(),
            })
            .unwrap();
    }

    fn heard(&self) -> Vec<Heard> {
        self.rig
            .probe
            .recorded_notes()
            .into_iter()
            .map(|n| {
                (
                    n.frame,
                    n.event.kind,
                    n.event.key,
                    n.event.channel,
                    n.event.note_id,
                )
            })
            .collect()
    }
}

fn expression(expression: NoteExpression, value: f32) -> NoteKind {
    NoteKind::Expression { expression, value }
}

/// Whether two lists of what a device heard are the same, expression values
/// to within what 14 bits of bend and `f32` arithmetic leave.
fn same(heard: &[NoteKind], wanted: &[NoteKind]) -> bool {
    heard.len() == wanted.len()
        && heard.iter().zip(wanted).all(|pair| match pair {
            (
                NoteKind::Expression {
                    expression: a,
                    value: x,
                },
                NoteKind::Expression {
                    expression: b,
                    value: y,
                },
            ) => a == b && (x - y).abs() < 1e-4,
            (a, b) => a == b,
        })
}

/// The first frame every block size can reach.
const FROM: u64 = 8192;

/// 14-bit pitch bend bytes for a bend `up` of the range, from −1 to 1.
fn bend(channel: u8, up: f32) -> [u8; 3] {
    let raw = if up >= 0.0 {
        8192 + (up * 8191.0).round() as i32
    } else {
        8192 + (up * 8192.0).round() as i32
    };
    [0xE0 | channel, (raw & 0x7F) as u8, (raw >> 7) as u8]
}

// Spec: mpe R11, midi-io R27. Notes on member channels 2 to 4 reach the
// devices with IDs of their own, their channels' bend, slide and pressure
// before them as starting values and during them as expressions on their
// frames; the master channel's messages are channel messages.
#[test]
fn notes_on_member_channels_arrive_as_notes_with_expressions_of_their_own() {
    for size in SIZES {
        let mut play = Play::new(size);
        play.instrument();
        play.play();
        // Channel 2's values before its note: bend a quarter up (12
        // semitones at the range of 48), slide 100, pressure 20.
        play.send(mpe(), FROM, &bend(1, 0.25));
        play.send(mpe(), FROM + 1, &[0xB1, 74, 100]);
        play.send(mpe(), FROM + 2, &[0xD1, 20]);
        play.send(mpe(), FROM + 10, &[0x91, 60, 100]);
        play.send(mpe(), FROM + 11, &[0x92, 64, 90]);
        play.send(mpe(), FROM + 12, &[0x93, 67, 80]);
        // During them: channel 3's bend and pressure, channel 4's slide.
        play.send(mpe(), FROM + 50, &bend(2, -0.5));
        play.send(mpe(), FROM + 60, &[0xD2, 127]);
        play.send(mpe(), FROM + 70, &[0xB3, 74, 5]);
        // The master channel.
        play.send(mpe(), FROM + 80, &[0xB0, 1, 127]);
        play.send(mpe(), FROM + 90, &[0x81, 60, 64]);
        play.run_to(FROM + 5000);

        let heard = play.heard();
        let at = |frame: u64| frame as i64;
        // Frame by frame: three note-ons with three different IDs.
        let ons: Vec<_> = heard
            .iter()
            .filter(|h| h.1 == NoteKind::On)
            .map(|h| (h.0, h.2, h.3))
            .collect();
        assert_eq!(
            ons,
            [
                (at(FROM + 10), 60, 1),
                (at(FROM + 11), 64, 2),
                (at(FROM + 12), 67, 3)
            ],
            "block size {size}"
        );
        let ids: Vec<_> = heard
            .iter()
            .filter(|h| h.1 == NoteKind::On)
            .map(|h| h.4.unwrap())
            .collect();
        assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
        // Channel 2's note starts with the values its channel held: after its
        // note-on, on its frame.
        let first: Vec<_> = heard
            .iter()
            .filter(|h| h.4 == Some(ids[0]) && h.0 == at(FROM + 10))
            .map(|h| h.1)
            .collect();
        assert!(
            same(
                &first,
                &[
                    NoteKind::On,
                    expression(NoteExpression::Bend, 0.25),
                    expression(NoteExpression::Slide, 100.0 / 127.0),
                    expression(NoteExpression::Pressure, 20.0 / 127.0),
                ]
            ),
            "block size {size}: {first:?}"
        );
        // Channel 3's note takes channel 3's bend on its frame, and
        // pressure; channel 4's takes its slide.
        let of = |id: u32| -> Vec<_> {
            heard
                .iter()
                .filter(|h| h.4 == Some(id) && matches!(h.1, NoteKind::Expression { .. }))
                .map(|h| (h.0, h.1, h.2))
                .collect()
        };
        let second = of(ids[1]);
        assert_eq!(
            second.iter().map(|e| (e.0, e.2)).collect::<Vec<_>>(),
            [(at(FROM + 50), 64), (at(FROM + 60), 64)],
            "block size {size}"
        );
        assert!(same(
            &second.iter().map(|e| e.1).collect::<Vec<_>>(),
            &[
                expression(NoteExpression::Bend, -0.5),
                expression(NoteExpression::Pressure, 1.0)
            ]
        ));
        let third = of(ids[2]);
        assert_eq!(
            third.iter().map(|e| (e.0, e.2)).collect::<Vec<_>>(),
            [(at(FROM + 70), 67)],
            "block size {size}"
        );
        assert!(same(
            &[third[0].1],
            &[expression(NoteExpression::Slide, 5.0 / 127.0)]
        ));
        // The master channel's controller is a channel message, with no note.
        let master: Vec<_> = heard
            .iter()
            .filter(|h| matches!(h.1, NoteKind::Control { .. }))
            .map(|h| (h.0, h.3, h.4))
            .collect();
        assert_eq!(master, [(at(FROM + 80), 0, None)], "block size {size}");
        // Channel 2's note-off ends the note with its own ID.
        let off: Vec<_> = heard
            .iter()
            .filter(|h| h.1 == NoteKind::Off)
            .map(|h| (h.0, h.2, h.3, h.4))
            .collect();
        assert_eq!(
            off,
            [(at(FROM + 90), 60, 1, Some(ids[0]))],
            "block size {size}"
        );
    }
}

// Spec: mpe R10. An MPE Configuration Message for an upper zone and a pitch
// bend sensitivity of 24 change the reading.
#[test]
fn a_configuration_message_and_a_bend_sensitivity_change_the_reading() {
    for size in SIZES {
        let mut play = Play::new(size);
        play.instrument();
        play.play();
        // An upper zone of 5 member channels: channels 11 to 15 under master 16.
        play.send(mpe(), FROM, &[0xBF, 101, 0]);
        play.send(mpe(), FROM, &[0xBF, 100, 6]);
        play.send(mpe(), FROM, &[0xBF, 6, 5]);
        // Channel 15, a member, sets its zone's range to 24 semitones.
        play.send(mpe(), FROM + 1, &[0xBE, 101, 0]);
        play.send(mpe(), FROM + 1, &[0xBE, 100, 0]);
        play.send(mpe(), FROM + 1, &[0xBE, 6, 24]);
        play.send(mpe(), FROM + 10, &[0x9E, 60, 100]);
        // A full bend up on channel 15 is 24 semitones: half of 48.
        play.send(mpe(), FROM + 20, &bend(14, 1.0));
        // Channel 16 is the upper zone's master.
        play.send(mpe(), FROM + 30, &[0xBF, 1, 9]);
        // A lower zone's channel keeps the default range of 48 semitones.
        play.send(mpe(), FROM + 40, &[0x91, 62, 100]);
        play.send(mpe(), FROM + 50, &bend(1, 1.0));
        play.run_to(FROM + 5000);
        let heard = play.heard();
        let bends: Vec<_> = heard
            .iter()
            .filter_map(|h| match h.1 {
                NoteKind::Expression {
                    expression: NoteExpression::Bend,
                    value,
                } => Some((h.0, h.2, value)),
                _ => None,
            })
            .collect();
        assert_eq!(
            bends.iter().map(|b| (b.0, b.1)).collect::<Vec<_>>(),
            [(FROM as i64 + 20, 60), (FROM as i64 + 50, 62)],
            "block size {size}"
        );
        assert!((bends[0].2 - 0.5).abs() < 1e-4, "24 of 48 semitones");
        assert!((bends[1].2 - 1.0).abs() < 1e-4, "48 of 48 semitones");
        let controls: Vec<_> = heard
            .iter()
            .filter(|h| matches!(h.1, NoteKind::Control { .. }))
            .map(|h| (h.0, h.3))
            .collect();
        assert_eq!(controls, [(FROM as i64 + 30, 15)], "block size {size}");
        // The configuration's controllers aren't passed on.
        assert!(
            heard
                .iter()
                .all(|h| !matches!(h.1, NoteKind::Control { controller, .. } if controller != 1))
        );
    }
}

// Spec: midi-io R27. The same messages from a port that isn't MPE are
// ordinary ones: channel messages and notes with no ID.
#[test]
fn a_port_that_is_not_mpe_is_read_as_before() {
    let mut play = Play::new(512);
    play.instrument();
    play.play();
    play.send(plain(), FROM, &[0x91, 60, 100]);
    play.send(plain(), FROM + 10, &bend(1, 0.5));
    play.send(plain(), FROM + 20, &[0x81, 60, 64]);
    play.run_to(FROM + 2000);
    let heard = play.heard();
    assert_eq!(
        heard
            .iter()
            .map(|h| (h.0, h.2, h.3, h.4))
            .collect::<Vec<_>>(),
        [
            (FROM as i64, 60, 1, None),
            (FROM as i64 + 10, 0, 1, None),
            (FROM as i64 + 20, 60, 1, None)
        ]
    );
    assert!(matches!(heard[1].1, NoteKind::PitchBend { .. }));
}

// Spec: midi-io R28. A track whose input is a port with MPE on takes every
// channel of it, whatever channel it had chosen.
#[test]
fn a_track_takes_every_channel_of_an_mpe_port() {
    let mut play = Play::new(512);
    let id = play.instrument();
    play.rig.apply(Command::SetInput {
        track: id,
        input: Input::Port {
            name: "Roli".to_owned(),
        },
        channel: Some(5),
    });
    play.play();
    play.send(mpe(), FROM, &[0x91, 60, 100]);
    play.send(mpe(), FROM + 10, &[0x92, 64, 100]);
    play.run_to(FROM + 2000);
    let ons = play
        .heard()
        .into_iter()
        .filter(|h| h.1 == NoteKind::On)
        .count();
    assert_eq!(ons, 2, "channels 2 and 3 reach a track set to channel 5");
    // The same port with MPE off keeps to the track's channel.
    play.send(plain(), FROM + 3000, &[0x91, 62, 100]);
    play.run_to(FROM + 5000);
    let ons = play
        .heard()
        .into_iter()
        .filter(|h| h.1 == NoteKind::On)
        .count();
    assert_eq!(ons, 0, "channel 2 isn't the track's channel 5");
}

// Spec: mpe R11. Switching monitoring off ends an MPE note by its own ID.
#[test]
fn switching_monitoring_off_ends_an_mpe_note_by_its_id() {
    let mut play = Play::new(512);
    let id = play.instrument();
    play.play();
    play.send(mpe(), FROM, &[0x91, 60, 100]);
    play.run_to(FROM + 1000);
    play.rig.apply(Command::SetMonitor {
        track: id,
        monitor: Monitor::Off,
    });
    play.run_to(FROM + 3000);
    let heard = play.heard();
    let on = heard.iter().find(|h| h.1 == NoteKind::On).unwrap();
    let off = heard.iter().find(|h| h.1 == NoteKind::Off).unwrap();
    assert_eq!((off.2, off.3, off.4), (60, 1, on.4));
}
