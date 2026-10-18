//! WP-3.14's acceptance in the engine: the notes a monitoring MIDI track's
//! input receives are logged for Capture MIDI, on their exact samples, with
//! the engine clock and, while the song plays, the song frame.
//!
//! - Notes are logged with the transport stopped, stamped with the clock
//!   only, and with it running, stamped with the song frame too.
//! - Only monitoring tracks are logged, and only notes and the controllers
//!   a clip keeps.
//! - A full ring drops the note and counts it.
//! - Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_core::time::{Frames, SampleRate, Ticks};
use bl_engine::capture::{self, Timing};
use bl_engine::testing;
use bl_engine::{CAPTURE_RING, CapturedMidi, NoInput, NoteKind, TransportCommand};
use bl_model::{Clip, ClipData, Command, MidiData, Monitor, TrackId, TrackKind};
use common::{Rig, device, track};
use rtrb::{Producer, RingBuffer};

const BLOCK: usize = 256;

/// The length of `frames` frames at 48 kHz.
fn frames(frames: u64) -> Duration {
    Duration::from_nanos(frames * 1_000_000_000 / 48_000)
}

struct Midi {
    rig: Rig,
    queue: Producer<MidiInputEvent>,
    start: Instant,
    blocks: u64,
    track: TrackId,
}

impl Midi {
    fn new(monitor: Monitor) -> Self {
        let mut rig = Rig::new(BLOCK);
        let (queue, consumer) = RingBuffer::new(CAPTURE_RING * 2);
        assert!(rig.processor.set_midi_input(consumer).is_none());
        let track = track(&mut rig.editor, TrackKind::Midi, None);
        rig.sync();
        rig.apply(Command::SetMonitor { track, monitor });
        Self {
            rig,
            queue,
            start: Instant::now(),
            blocks: 0,
            track,
        }
    }

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

    fn captured(&mut self) -> Vec<CapturedMidi> {
        self.rig.monitor.captured()
    }
}

// Spec: capture-midi R1; midi-controller-data R15. With the transport
// stopped a note or controller is logged on the engine-clock frame it
// landed on, with no song frame. A message no clip keeps, such as All Notes
// Off, is not logged.
#[test]
fn notes_played_with_the_transport_stopped_are_logged_on_their_exact_samples() {
    let mut midi = Midi::new(Monitor::In);
    let (on, off) = (
        MidiMessage::note_on(0, 60, 100),
        MidiMessage::note_off(0, 60),
    );
    let controller = MidiMessage::from_bytes(&[0xB0, 1, 64]).unwrap();
    let all_notes_off = MidiMessage::from_bytes(&[0xB0, 123, 0]).unwrap();
    midi.block();
    midi.send(0, 70, controller);
    midi.send(0, 80, all_notes_off);
    midi.send(1, 100, on);
    midi.block();
    midi.send(2, 40, off);
    midi.block();
    midi.block();
    midi.block();

    // Arrived 100 frames into block 1, placed in block 2; and so on.
    let track = midi.track;
    assert_eq!(
        midi.captured(),
        vec![
            CapturedMidi {
                track,
                clock: BLOCK as u64 + 70,
                song: None,
                message: controller,
            },
            CapturedMidi {
                track,
                clock: 2 * BLOCK as u64 + 100,
                song: None,
                message: on,
            },
            CapturedMidi {
                track,
                clock: 3 * BLOCK as u64 + 40,
                song: None,
                message: off,
            },
        ],
        "the notes and the controller, not All Notes Off"
    );
    assert!(midi.captured().is_empty(), "taken once");
}

// Spec: capture-midi R1. While the song plays the song frame comes too.
#[test]
fn notes_played_while_the_song_plays_carry_their_song_frame() {
    let mut midi = Midi::new(Monitor::In);
    midi.rig
        .sender
        .transport(TransportCommand::SetInsertMarker(Ticks::QUARTER * 2));
    midi.rig.sender.transport(TransportCommand::Play);
    midi.block();
    let on = MidiMessage::note_on(0, 62, 90);
    midi.send(1, 30, on);
    for _ in 0..3 {
        midi.block();
    }
    // Playback starts on two quarter notes (1 second) in the first block.
    let track = midi.track;
    assert_eq!(
        midi.captured(),
        vec![CapturedMidi {
            track,
            clock: 2 * BLOCK as u64 + 30,
            song: Some(Frames(48_000 + 2 * BLOCK as i64 + 30)),
            message: on,
        }]
    );
}

// Spec: capture-midi R1. A track that doesn't monitor logs nothing.
#[test]
fn a_track_that_does_not_monitor_logs_nothing() {
    let mut midi = Midi::new(Monitor::Off);
    midi.block();
    midi.send(0, 10, MidiMessage::note_on(0, 60, 100));
    midi.block();
    midi.block();
    assert!(midi.captured().is_empty());
}

// Spec: capture-midi R1. A ring that is not drained takes only its capacity
// and counts the notes it had no room for.
#[test]
fn a_full_ring_drops_notes_and_counts_them() {
    let mut midi = Midi::new(Monitor::In);
    let spare = 10;
    let total = CAPTURE_RING + spare;
    let mut sent = 0;
    // 100 notes a block, each block's notes spread over its frames.
    let mut block = 0;
    while sent < total {
        for i in 0..100.min(total - sent) {
            midi.send(block, i as u64, MidiMessage::note_on(0, 60, 100));
            sent += 1;
        }
        block += 1;
    }
    for _ in 0..block + 2 {
        midi.block();
    }
    let counters = midi.rig.processor.midi_counters();
    assert_eq!(midi.captured().len(), CAPTURE_RING);
    assert_eq!(counters.capture_dropped, spare as u64);
}

// Spec: capture-midi R2, R3 (the acceptance's render-level test). A phrase
// played on exact samples, captured and placed as a clip, plays back on
// those samples: each note-on and note-off falls within a frame of where it
// was played, counted from the first note.
#[test]
fn a_captured_phrase_plays_back_on_the_samples_it_was_played_on() {
    let mut played = Midi::new(Monitor::In);
    // Note-ons 0, 24,000, 30,000 and 60,000 frames from the first, each a
    // sixth of a second long; the first lands in block 1.
    let starts = [300_u64, 24_300, 30_300, 60_300];
    played.block();
    for (i, &start) in starts.iter().enumerate() {
        let key = 60 + i as u8;
        let (block, after) = (start / BLOCK as u64, start % BLOCK as u64);
        played.send(block, after, MidiMessage::note_on(0, key, 90 + i as u8));
        let end = start + 8_000;
        played.send(
            end / BLOCK as u64,
            end % BLOCK as u64,
            MidiMessage::note_off(0, key),
        );
    }
    for _ in 0..(70_000 / BLOCK) {
        played.block();
    }
    let messages = played.captured();
    assert_eq!(messages.len(), 8);

    // What the shell does with a stopped transport: pair, fit, place.
    let notes = capture::pair(&messages);
    let rate = SampleRate::HZ_48000;
    let onsets: Vec<f64> = notes
        .iter()
        .map(|n| (n.on - notes[0].on) as f64 / rate.as_f64())
        .collect();
    let fit = capture::fit(&onsets, 120.0);
    assert_eq!(fit.set, None, "the played tempo is the song's");
    let timing = Timing::Stopped {
        from: notes[0].on,
        bpm: fit.bpm,
        rate,
    };
    let placed = capture::place(&notes, timing, |_| Ticks::ZERO, 0, Ticks::WHOLE);

    // Render the clip on a fresh rig and read the instrument's notes.
    let mut rig = Rig::new(BLOCK);
    let trk = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, trk, testing::INSTRUMENT, &[]);
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: trk,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                placed.loop_end,
                ClipData::Midi(MidiData::new(placed.notes)),
            ),
        })
        .unwrap();
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    for _ in 0..(3 * 48_000 / BLOCK) {
        rig.processor.process(BLOCK);
    }
    let rendered = rig.probe.recorded_notes();
    assert_eq!(rendered.len(), 8, "{rendered:?}");
    let mut expected: Vec<(i64, NoteKind, u8)> = Vec::new();
    for note in &notes {
        expected.push(((note.on - notes[0].on) as i64, NoteKind::On, note.key));
        expected.push(((note.off - notes[0].on) as i64, NoteKind::Off, note.key));
    }
    expected.sort_by_key(|&(frame, ..)| frame);
    for (heard, (frame, kind, key)) in rendered.iter().zip(expected) {
        assert!(
            (heard.frame - frame).abs() <= 1,
            "key {key}: played at {frame}, rendered at {}",
            heard.frame
        );
        assert_eq!((heard.event.kind, heard.event.key), (kind, key));
    }
    let velocities: Vec<f32> = rendered
        .iter()
        .filter(|n| n.event.kind == NoteKind::On)
        .map(|n| n.event.velocity)
        .collect();
    assert_eq!(
        velocities,
        [90.0 / 127.0, 91.0 / 127.0, 92.0 / 127.0, 93.0 / 127.0]
    );
}
