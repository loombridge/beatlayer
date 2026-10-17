//! WP-2.14's acceptance in the engine: an armed MIDI track's note messages
//! are copied into a ring, stamped with the song frame they landed on, from
//! the frame playback starts on (after any count-in) to the end of the take.
//!
//! - Notes played during a take are in the ring on the exact sample the
//!   input placed them on, whether or not the track monitors.
//! - A count-in of one bar delays the take by a bar, and notes played during
//!   it are not recorded.
//! - Stopping during the count-in records nothing.
//! - A ring too full for a part's notes ends the take and counts them.
//! - Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_core::time::{Frames, Ticks};
use bl_engine::{
    MidiRecordTrack, NoInput, RecordedMidi, RecordingPlan, TakeBounds, TransportCommand,
};
use bl_model::{Command, CountIn, Monitor, TrackId, TrackKind};
use common::{Rig, track};
use rtrb::{Consumer, Producer, RingBuffer};

const BLOCK: usize = 256;

/// The length of `frames` frames at 48 kHz.
fn frames(frames: u64) -> Duration {
    Duration::from_nanos(frames * 1_000_000_000 / 48_000)
}

/// A rig with a MIDI queue, an armed MIDI track, and the time its first
/// block starts.
struct Midi {
    rig: Rig,
    queue: Producer<MidiInputEvent>,
    start: Instant,
    blocks: u64,
    track: TrackId,
}

impl Midi {
    fn new(setup: impl FnOnce(&mut Rig)) -> Self {
        let mut rig = Rig::new(BLOCK);
        let (queue, consumer) = RingBuffer::new(256);
        assert!(rig.processor.set_midi_input(consumer).is_none());
        setup(&mut rig);
        let track = track(&mut rig.editor, TrackKind::Midi, None);
        rig.sync();
        rig.apply(Command::SetArm { track, arm: true });
        // The track doesn't hear its input, and the take records it all
        // the same.
        rig.apply(Command::SetMonitor {
            track,
            monitor: Monitor::Off,
        });
        Self {
            rig,
            queue,
            start: Instant::now(),
            blocks: 0,
            track,
        }
    }

    /// Records the track's notes into a ring of `capacity` messages.
    fn record(&mut self, capacity: usize) -> (Consumer<RecordedMidi>, Arc<AtomicU64>) {
        let (producer, consumer) = RingBuffer::new(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let plan = RecordingPlan {
            tracks: Vec::new(),
            midi: vec![MidiRecordTrack {
                track: self.track,
                producer,
                dropped: Arc::clone(&dropped),
            }],
            delay: 0,
            session: Vec::new(),
            launches: None,
        };
        self.rig
            .sender
            .record(self.rig.collector.handle().wrap(plan));
        (consumer, dropped)
    }

    /// Processes the next block, called exactly on time, as a real-time
    /// section.
    fn block(&mut self) {
        let now = self.start + frames(self.blocks * BLOCK as u64);
        let processor = &mut self.rig.processor;
        bl_rt::rt_test!({
            processor.process_at(BLOCK, &NoInput, now);
        });
        self.blocks += 1;
    }

    /// Sends a message that arrived `after` frames into block `block`.
    fn send(&mut self, block: u64, after: u64, message: MidiMessage) {
        let time = self.start + frames(block * BLOCK as u64 + after);
        self.queue
            .push(MidiInputEvent {
                source: MidiSource::ComputerKeyboard,
                time,
                message,
            })
            .unwrap();
    }
}

fn drain(ring: &mut Consumer<RecordedMidi>) -> Vec<(i64, MidiMessage)> {
    let mut messages = Vec::new();
    while let Ok(m) = ring.pop() {
        messages.push((m.frame.get(), m.message));
    }
    messages
}

// Spec: midi-io R12, R15 (WP-2.14). A note lands in the block after the one
// it arrived in, on the sample its time gives, and the take stamps it with
// the song frame: the sample it was played on, to the sample.
#[test]
fn notes_played_during_a_take_are_on_their_exact_samples() {
    let mut midi = Midi::new(|_| {});
    let (mut ring, _) = midi.record(64);
    // Playback starts on two quarter notes (1 second).
    midi.rig
        .sender
        .transport(TransportCommand::SetInsertMarker(Ticks::QUARTER * 2));
    midi.rig
        .sender
        .transport(TransportCommand::SetRecording(true));
    midi.rig.sender.transport(TransportCommand::Play);
    let (on, off) = (
        MidiMessage::note_on(0, 60, 100),
        MidiMessage::note_off(0, 60),
    );
    let controller = MidiMessage::from_bytes(&[0xB0, 1, 64]).unwrap();
    midi.block();
    midi.send(0, 70, controller);
    midi.send(1, 100, on);
    midi.block();
    midi.send(2, 40, off);
    for _ in 0..4 {
        midi.block();
    }
    midi.rig.sender.transport(TransportCommand::Stop);
    midi.block();

    let state = midi.rig.monitor.state();
    assert_eq!(
        state.take,
        Some(TakeBounds {
            start: Frames(48_000),
            end: Frames(48_000 + 6 * BLOCK as i64),
            wrap: None,
        })
    );
    midi.rig.collector.collect();
    assert!(ring.is_abandoned(), "the plan went with the take");
    assert_eq!(
        drain(&mut ring),
        vec![
            // Arrived 100 frames into block 1, placed in block 2.
            (48_000 + 2 * BLOCK as i64 + 100, on),
            // Arrived 40 frames into block 2, placed in block 3.
            (48_000 + 3 * BLOCK as i64 + 40, off),
        ],
        "the notes, not the controller"
    );
}

// Spec: metronome R5 (WP-2.14). Recording starts when the count-in ends, so
// a one-bar count-in puts the take a bar later, and a note played during it
// isn't recorded.
#[test]
fn a_count_in_of_one_bar_starts_recording_one_bar_later() {
    let mut midi = Midi::new(|rig| {
        rig.editor
            .apply(Command::SetMetronomeEnabled { enabled: true })
            .unwrap();
        rig.editor
            .apply(Command::SetMetronomeCountIn {
                count_in: CountIn::Bars1,
            })
            .unwrap();
    });
    let (mut ring, _) = midi.record(64);
    midi.rig
        .sender
        .transport(TransportCommand::SetRecording(true));
    midi.rig.sender.transport(TransportCommand::Play);
    let (early, on) = (
        MidiMessage::note_on(0, 62, 90),
        MidiMessage::note_on(0, 64, 80),
    );
    // A 4/4 bar at 120 BPM is 96,000 frames: 375 blocks.
    let bar = 96_000 / BLOCK as u64;
    midi.block();
    midi.send(0, 10, early);
    for _ in 0..bar + 5 {
        midi.block();
    }
    midi.send(bar + 5, 100, on);
    for _ in 0..3 {
        midi.block();
    }
    midi.rig.sender.transport(TransportCommand::Stop);
    midi.block();

    assert_eq!(
        midi.rig.monitor.state().take.map(|take| take.start),
        Some(Frames::ZERO),
        "the take starts at the insert marker, a bar of blocks in"
    );
    // Block `bar + 6` is the 7th the song plays (block `bar` is the first).
    assert_eq!(drain(&mut ring), vec![(6 * BLOCK as i64 + 100, on)]);
}

// Spec: metronome R5 (WP-2.14). Stop during the count-in ends the recording
// before the take began: nothing is recorded and the take never existed.
#[test]
fn stopping_during_the_count_in_records_nothing() {
    let mut midi = Midi::new(|rig| {
        rig.editor
            .apply(Command::SetMetronomeCountIn {
                count_in: CountIn::Bars1,
            })
            .unwrap();
        rig.editor
            .apply(Command::SetMetronomeEnabled { enabled: true })
            .unwrap();
    });
    let (mut ring, _) = midi.record(64);
    midi.rig
        .sender
        .transport(TransportCommand::SetRecording(true));
    midi.rig.sender.transport(TransportCommand::Play);
    midi.block();
    midi.send(0, 10, MidiMessage::note_on(0, 62, 90));
    for _ in 0..10 {
        midi.block();
    }
    midi.rig.sender.transport(TransportCommand::Stop);
    midi.block();
    midi.rig.collector.collect();
    let state = midi.rig.monitor.state();
    assert!(!state.recording);
    assert_eq!(state.take, None, "no take began");
    assert!(ring.is_abandoned());
    assert_eq!(drain(&mut ring), vec![]);
}

// Spec: recording R5 (WP-2.14). A ring too full for a part's notes ends the
// take before them and counts them: nothing is dropped silently.
#[test]
fn a_full_ring_ends_the_take_and_counts_its_dropped_messages() {
    let mut midi = Midi::new(|_| {});
    let (mut ring, dropped) = midi.record(1);
    midi.rig
        .sender
        .transport(TransportCommand::SetRecording(true));
    midi.rig.sender.transport(TransportCommand::Play);
    midi.block();
    midi.send(0, 10, MidiMessage::note_on(0, 60, 100));
    midi.send(0, 20, MidiMessage::note_on(0, 64, 100));
    midi.block();
    midi.block();
    let state = midi.rig.monitor.state();
    assert!(!state.recording, "the take ended when its ring filled");
    assert_eq!(dropped.load(Ordering::Relaxed), 2);
    midi.rig.collector.collect();
    assert_eq!(drain(&mut ring), vec![]);
}
