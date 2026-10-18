//! MIDI out on a track's MIDI To (WP-5.34, `docs/specs/external-devices.md`
//! R3 to R6): what reaches the end of a track's chain leaves for its port on
//! its channel, stamped with the moment its frame is heard at the audio
//! interface's output, and every note gets its note-off. Every block runs as
//! a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiOutputEvent, MidiSource, PortId};
use bl_engine::testing::{DELAY, DELAY_FRAMES};
use bl_engine::{NoInput, TransportCommand};
use bl_model::{Command, Input, MidiTo, Monitor, TrackId, TrackKind};
use common::{Rig, device, track};
use rtrb::{Consumer, Producer, RingBuffer};

/// A rig with a MIDI queue in and one out, a clock that calls each block
/// exactly on time, and a MIDI track playing the computer keyboard's notes
/// to the port `Synth` on channel 3.
struct Out {
    rig: Rig,
    input: Producer<MidiInputEvent>,
    output: Consumer<MidiOutputEvent>,
    start: Instant,
    block: usize,
    blocks: u64,
    keys: TrackId,
}

/// The length of `frames` frames at 48 kHz.
fn frames(frames: u64) -> Duration {
    Duration::from_nanos(frames * 1_000_000_000 / 48_000)
}

fn port(name: &str) -> PortId {
    PortId::from_name(name)
}

fn to(name: &str, channel: u8) -> Option<MidiTo> {
    Some(MidiTo::Port {
        name: name.to_owned(),
        channel,
    })
}

impl Out {
    /// A track `Keys` sending to `Synth`, in blocks of `block` frames, with
    /// the device reporting `latency` frames of output latency.
    fn new(block: usize, latency: u32) -> Self {
        let mut rig = Rig::with_config(block, |config| config.output_latency = latency);
        let (input, consumer) = RingBuffer::new(4096);
        assert!(rig.processor.set_midi_input(consumer).is_none());
        let (producer, output) = RingBuffer::new(4096);
        assert!(rig.processor.set_midi_output(producer).is_none());
        let keys = track(&mut rig.editor, TrackKind::Midi, None);
        rig.sync();
        rig.apply(Command::SetInput {
            track: keys,
            input: Input::ComputerKeyboard,
            channel: None,
        });
        rig.apply(Command::SetMonitor {
            track: keys,
            monitor: Monitor::In,
        });
        rig.apply(Command::SetMidiOutput {
            track: keys,
            to: to("Synth", 3),
        });
        Self {
            rig,
            input,
            output,
            start: Instant::now(),
            block,
            blocks: 0,
            keys,
        }
    }

    fn block_time(&self, n: u64) -> Instant {
        self.start + frames(n * self.block as u64)
    }

    /// Processes the next block, called exactly on time.
    fn run(&mut self) {
        let now = self.block_time(self.blocks);
        let (processor, block) = (&mut self.rig.processor, self.block);
        bl_rt::rt_test!({
            processor.process_at(block, &NoInput, now);
        });
        self.blocks += 1;
    }

    /// Sends a message that arrived at `time`.
    fn send(&mut self, time: Instant, message: MidiMessage) {
        self.input
            .push(MidiInputEvent {
                source: MidiSource::ComputerKeyboard,
                time,
                message,
            })
            .unwrap();
    }

    /// Sends a message that arrived during the previous block, `i` hundred
    /// microseconds in, so the block that runs next plays it, the messages
    /// in the order sent.
    fn send_now(&mut self, i: u32, message: MidiMessage) {
        let time = self.block_time(self.blocks) - frames(self.block as u64)
            + Duration::from_micros(100) * i;
        self.send(time, message);
    }

    /// Everything sent to ports since the last call.
    fn sent(&mut self) -> Vec<MidiOutputEvent> {
        let mut sent = Vec::new();
        while let Ok(event) = self.output.pop() {
            sent.push(event);
        }
        sent
    }
}

/// How far `a` is from `b`, in nanoseconds.
fn apart(a: Instant, b: Instant) -> u128 {
    a.max(b).duration_since(a.min(b)).as_nanos()
}

// Spec: external-devices R3
#[test]
fn notes_and_controllers_leave_on_the_chosen_channel() {
    let mut out = Out::new(256, 0);
    out.run();
    let messages = [
        // Everything arrives on channel 1 (0) or 10 (9) and leaves on 3.
        [0x90, 60, 100],
        [0xB9, 1, 64],
        [0xE0, 127, 127],
        [0xA5, 60, 80],
        [0x80, 60, 64],
    ];
    for (i, bytes) in messages.into_iter().enumerate() {
        out.send_now(i as u32, MidiMessage::from_bytes(&bytes).unwrap());
    }
    out.send_now(5, MidiMessage::from_bytes(&[0xD1, 50]).unwrap());
    // A system message reaches no track, and so leaves for no port.
    out.send_now(6, MidiMessage::from_bytes(&[0xF8]).unwrap());
    out.run();
    out.run();
    let sent: Vec<[u8; 3]> = out.sent().iter().map(|e| e.message.padded()).collect();
    assert_eq!(
        sent,
        vec![
            [0x92, 60, 100],
            [0xB2, 1, 64],
            [0xE2, 127, 127],
            [0xA2, 60, 80],
            [0x82, 60, 64],
            [0xD2, 50, 0],
        ]
    );
    assert_eq!(out.rig.processor.midi_counters().sent, 6);
}

// Spec: external-devices R3
#[test]
fn every_message_is_for_the_port_the_track_names() {
    let mut out = Out::new(128, 0);
    out.run();
    out.send(out.block_time(0), MidiMessage::note_on(0, 60, 100));
    out.run();
    out.run();
    let sent = out.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].port, port("Synth"));
    // No Output sends nothing.
    out.rig.apply(Command::SetMidiOutput {
        track: out.keys,
        to: None,
    });
    out.run();
    out.send_now(0, MidiMessage::note_on(0, 61, 100));
    out.run();
    out.run();
    assert!(
        out.sent()
            .iter()
            .all(|e| e.message.is_note_off() && e.message.data1() == 60),
        "only the first note's note-off, for the old route"
    );
}

// Spec: external-devices R4
#[test]
fn each_message_is_stamped_with_the_moment_its_frame_is_heard() {
    const LATENCY: u32 = 300;
    const COMPENSATION: u32 = 1000;
    for block in [1, 100, 512, 4096] {
        for compensated in [false, true] {
            let mut out = Out::new(block, LATENCY);
            if compensated {
                // A track through a device that adds 1,000 frames: the keys
                // track's audio is held back that long on its way into
                // Main, so its MIDI is too.
                let slow = track(&mut out.rig.editor, TrackKind::Audio, None);
                device(
                    &mut out.rig.editor,
                    slow,
                    DELAY,
                    &[(DELAY_FRAMES, f64::from(COMPENSATION))],
                );
                out.rig.sync();
            }
            let extra = if compensated { COMPENSATION } else { 0 };
            // Block 0 locks the clock; what arrives during block 3 plays in
            // block 4, one block after it arrived.
            for _ in 0..3 {
                out.run();
            }
            let window = out.block_time(3);
            let arrivals: Vec<(u8, Instant)> = (0..8u8)
                .map(|j| {
                    let after = frames(block as u64 * u64::from(j) / 8);
                    (60 + j, window + after)
                })
                .collect();
            for &(key, time) in &arrivals {
                out.send(time, MidiMessage::note_on(0, key, 100));
            }
            for _ in 0..4 {
                out.run();
            }
            let sent = out.sent();
            assert_eq!(sent.len(), arrivals.len(), "block {block}");
            let one_frame = frames(1).as_nanos() + 1_000;
            for (key, arrival) in arrivals {
                let event = sent
                    .iter()
                    .find(|e| e.message.data1() == key)
                    .unwrap_or_else(|| panic!("note {key} left"));
                let expected = arrival + frames(block as u64 + u64::from(LATENCY + extra));
                let error = apart(event.time, expected);
                assert!(
                    error <= one_frame,
                    "block {block}, compensated {compensated}: note {key} is {error} ns \
                     from its moment"
                );
            }
        }
    }
}

// Spec: external-devices R6
#[test]
fn every_note_on_gets_its_note_off() {
    // Stop.
    let mut out = Out::new(128, 0);
    out.rig.sender.transport(TransportCommand::Play);
    out.run();
    out.send(out.block_time(0), MidiMessage::note_on(0, 60, 100));
    out.send(out.block_time(0), MidiMessage::note_on(0, 64, 100));
    out.run();
    out.run();
    assert_eq!(out.sent().len(), 2);
    out.rig.sender.transport(TransportCommand::Stop);
    assert!(out.rig.sender.flush());
    out.run();
    out.run();
    let offs = out.sent();
    let mut keys: Vec<u8> = offs.iter().map(|e| e.message.data1()).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec![60, 64]);
    assert!(
        offs.iter()
            .all(|e| e.message.is_note_off() && e.message.channel() == Some(2))
    );
    // A note already ended is ended once.
    out.rig.sender.transport(TransportCommand::Play);
    out.run();
    out.send_now(0, MidiMessage::note_on(0, 70, 100));
    out.run();
    out.send_now(0, MidiMessage::note_off(0, 70));
    out.run();
    out.run();
    out.rig.sender.transport(TransportCommand::Stop);
    assert!(out.rig.sender.flush());
    out.run();
    out.run();
    let sent = out.sent();
    assert_eq!(
        sent.iter().filter(|e| e.message.is_note_off()).count(),
        1,
        "{sent:?}"
    );

    // A reroute ends the notes where they started: the old port and
    // channel.
    let mut out = Out::new(128, 0);
    out.run();
    out.send(out.block_time(0), MidiMessage::note_on(0, 60, 100));
    out.run();
    out.run();
    out.sent();
    out.rig.apply(Command::SetMidiOutput {
        track: out.keys,
        to: to("Other", 9),
    });
    out.run();
    out.run();
    let sent = out.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].port, port("Synth"));
    assert_eq!(sent[0].message.padded(), [0x82, 60, 64]);

    // A new channel on the same port is a reroute too.
    out.send_now(0, MidiMessage::note_on(0, 61, 100));
    out.run();
    out.run();
    out.sent();
    out.rig.apply(Command::SetMidiOutput {
        track: out.keys,
        to: to("Other", 10),
    });
    out.run();
    let sent = out.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].port, port("Other"));
    assert_eq!(sent[0].message.padded(), [0x88, 61, 64]);

    // Deleting the track ends them too.
    out.send_now(0, MidiMessage::note_on(0, 62, 100));
    out.run();
    out.run();
    out.sent();
    out.rig.apply(Command::DeleteTrack { track: out.keys });
    out.run();
    let sent = out.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].message.padded(), [0x89, 62, 64]);

    // A stream that stops ends every held note.
    let mut out = Out::new(128, 0);
    out.run();
    out.send(out.block_time(0), MidiMessage::note_on(0, 60, 100));
    out.send(out.block_time(0), MidiMessage::note_on(0, 61, 100));
    out.run();
    out.run();
    out.sent();
    let _carry = out.rig.processor.park();
    let offs = out.sent();
    let mut keys: Vec<u8> = offs.iter().map(|e| e.message.data1()).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec![60, 61]);
    assert!(offs.iter().all(|e| e.message.is_note_off()));
}

// Spec: external-devices R5
#[test]
fn a_full_queue_drops_the_message_and_counts_it() {
    let mut rig = Rig::new(128);
    let (input, consumer) = RingBuffer::new(64);
    rig.processor.set_midi_input(consumer);
    let (producer, mut output) = RingBuffer::new(2);
    rig.processor.set_midi_output(producer);
    let keys = track(&mut rig.editor, TrackKind::Midi, None);
    rig.sync();
    rig.apply(Command::SetInput {
        track: keys,
        input: Input::ComputerKeyboard,
        channel: None,
    });
    rig.apply(Command::SetMonitor {
        track: keys,
        monitor: Monitor::In,
    });
    rig.apply(Command::SetMidiOutput {
        track: keys,
        to: to("Synth", 1),
    });
    let mut input = input;
    let start = Instant::now();
    let run = |rig: &mut Rig, n: u64| {
        let processor = &mut rig.processor;
        let now = start + frames(n * 128);
        bl_rt::rt_test!({
            processor.process_at(128, &NoInput, now);
        });
    };
    run(&mut rig, 0);
    for key in 60..65 {
        input
            .push(MidiInputEvent {
                source: MidiSource::ComputerKeyboard,
                time: start + frames(10),
                message: MidiMessage::note_on(0, key, 100),
            })
            .unwrap();
    }
    run(&mut rig, 1);
    run(&mut rig, 2);
    let mut taken = 0;
    while output.pop().is_ok() {
        taken += 1;
    }
    assert_eq!(taken, 2);
    let counters = rig.processor.midi_counters();
    assert_eq!((counters.sent, counters.out_dropped), (2, 3));
}
