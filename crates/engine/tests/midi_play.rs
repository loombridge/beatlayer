//! A monitored MIDI track's devices play its input (WP-2.2,
//! `docs/specs/midi-io.md` R12 and R26): timestamped input reaches the
//! track's first device on its exact frames, merged with its clips' notes;
//! a track that doesn't monitor sends its devices nothing; and every change
//! that stops monitoring ends the notes the devices hold. Every block is a
//! real-time section under the allocation trap.

use crate::common;
use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::testing::{self, RecordedNote};
use bl_engine::{NoInput, NoteExpression, NoteKind, TransportCommand};
use bl_model::{Clip, ClipData, Command, Input, MidiData, Monitor, Note, TrackId, TrackKind};
use common::{Rig, device, track};
use rtrb::{Producer, RingBuffer};

/// The block sizes every test runs at.
const SIZES: [usize; 4] = [1, 100, 512, 4096];

const KEYBOARD: MidiSource = MidiSource::ComputerKeyboard;

/// A rig playing from frame 0, with a MIDI queue and exact block times.
struct Play {
    rig: Rig,
    queue: Producer<MidiInputEvent>,
    start: Instant,
    size: usize,
    blocks: u64,
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

    /// Starts the transport and processes the first block, which locks the
    /// block clock.
    fn play(&mut self) {
        self.rig.sender.transport(TransportCommand::Play);
        self.block();
    }

    /// The start of the block `n` blocks after the first, on an exact clock.
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

    /// Processes blocks until the one that starts at `frame` has run.
    fn run_to(&mut self, frame: u64) {
        while self.blocks * (self.size as u64) <= frame {
            self.block();
        }
    }

    /// Queues `message` to land on engine frame `frame`: stamped in the
    /// block before, where R15 places it one block later. Frames in the
    /// first block can't be reached.
    fn send(&mut self, source: MidiSource, frame: u64, message: MidiMessage) {
        let size = self.size as u64;
        assert!(frame >= size, "frame {frame} is in the first block");
        // 100 ns in, so rounding to whole nanoseconds can't move the message
        // to the block before.
        let time = self.block_time(frame / size - 1)
            + Duration::from_nanos((frame % size) * 1_000_000_000 / 48_000 + 100);
        self.queue
            .push(MidiInputEvent {
                source,
                time,
                message,
            })
            .unwrap();
    }

    /// Every note the instruments recorded so far, as `(frame, kind, key)`.
    fn heard(&self) -> Vec<(i64, NoteKind, u8)> {
        notes(self.rig.probe.recorded_notes())
    }
}

fn notes(recorded: Vec<RecordedNote>) -> Vec<(i64, NoteKind, u8)> {
    recorded
        .into_iter()
        .map(|n| (n.frame, n.event.kind, n.event.key))
        .collect()
}

fn on(key: u8) -> MidiMessage {
    MidiMessage::note_on(0, key, 100)
}

fn off(key: u8) -> MidiMessage {
    MidiMessage::note_off(0, key)
}

// Spec: midi-io R12
#[test]
fn a_monitored_tracks_devices_play_its_input_on_the_exact_frames() {
    for size in SIZES {
        let mut play = Play::new(size);
        play.instrument();
        play.play();
        let release = MidiMessage::from_bytes(&[0x80, 60, 33]).unwrap();
        let pressure = MidiMessage::from_bytes(&[0xA0, 60, 127]).unwrap();
        // 2 * 4096 is the first frame every size can reach.
        play.send(KEYBOARD, 8192 + 7, MidiMessage::note_on(0, 60, 127));
        play.send(KEYBOARD, 8192 + 50, pressure);
        play.send(KEYBOARD, 8192 + 99, release);
        play.run_to(8192 + 5000);

        let recorded = play.rig.probe.recorded_notes();
        let frames: Vec<_> = recorded.iter().map(|n| n.frame).collect();
        assert_eq!(frames, [8199, 8242, 8291], "block size {size}");
        assert_eq!(recorded[0].event.kind, NoteKind::On);
        assert_eq!(recorded[0].event.velocity, 1.0);
        assert_eq!(
            recorded[1].event.kind,
            NoteKind::Expression {
                expression: NoteExpression::Pressure,
                value: 1.0
            }
        );
        assert_eq!(recorded[2].event.kind, NoteKind::Off);
        assert_eq!(recorded[2].event.velocity, 33.0 / 127.0, "release velocity");
    }
}

// Spec: midi-io R12
#[test]
fn input_merges_with_a_playing_clips_notes_in_frame_order() {
    for size in SIZES {
        let mut play = Play::new(size);
        // Armed on Auto, the track monitors and its clips play too; on In
        // they would be silent.
        let id = play.instrument();
        play.rig.apply(Command::SetMonitor {
            track: id,
            monitor: Monitor::Auto,
        });
        play.rig.apply(Command::SetArm {
            track: id,
            arm: true,
        });
        // Key 60 plays beats 0 to 1 (frames 0 to 24,000 at 120 BPM), key 64
        // from beat 2 (frame 48,000).
        let beat = Ticks(TICKS_PER_QUARTER);
        let clip = play.rig.editor.new_id();
        play.rig
            .editor
            .apply(Command::CreateClip {
                track: id,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    beat * 4,
                    ClipData::Midi(MidiData::new([
                        Note::new(60, Ticks::ZERO, beat, 100),
                        Note::new(64, beat * 2, beat, 100),
                    ])),
                ),
            })
            .unwrap();
        play.rig.sync();
        play.play();
        // On the frame the clip's note ends, a note starts; on the frame the
        // clip's next note starts, the input's note ends.
        play.send(KEYBOARD, 24_000, on(72));
        play.send(KEYBOARD, 48_000, off(72));
        play.run_to(60_000);

        assert_eq!(
            play.heard(),
            [
                (0, NoteKind::On, 60),
                (24_000, NoteKind::Off, 60),
                (24_000, NoteKind::On, 72),
                (48_000, NoteKind::Off, 72),
                (48_000, NoteKind::On, 64),
            ],
            "block size {size}"
        );
    }
}

// Spec: midi-io R10, R12
#[test]
fn a_track_that_doesnt_monitor_sends_its_devices_nothing() {
    for size in SIZES {
        let mut play = Play::new(size);
        let off_track = play.instrument();
        play.rig.apply(Command::SetMonitor {
            track: off_track,
            monitor: Monitor::Off,
        });
        let unarmed = track(&mut play.rig.editor, TrackKind::Midi, None);
        device(&mut play.rig.editor, unarmed, testing::INSTRUMENT, &[]);
        play.rig.sync();
        play.play();
        play.send(KEYBOARD, 8192, on(60));
        play.run_to(8192 + 5000);

        assert_eq!(play.heard(), [], "block size {size}");
        // The input still reaches both tracks (R12).
        for id in [off_track, unarmed] {
            assert!(!play.rig.processor.midi_input(id).unwrap().monitored);
        }
        assert_eq!(play.rig.processor.midi_counters().delivered, 2);

        // Arming the Auto track starts it playing.
        play.rig.apply(Command::SetArm {
            track: unarmed,
            arm: true,
        });
        play.send(KEYBOARD, 16_384 + 1, on(61));
        play.run_to(16_384 + 5000);
        assert_eq!(
            play.heard(),
            [(16_385, NoteKind::On, 61)],
            "block size {size}"
        );
    }
}

// Spec: step-recording R2, R3
#[test]
fn a_preview_plays_on_its_own_track_with_the_song_stopped_and_is_never_input() {
    for size in SIZES {
        let mut play = Play::new(size);
        let previewed = play.instrument();
        play.rig.apply(Command::SetMonitor {
            track: previewed,
            monitor: Monitor::Off,
        });
        let other = play.instrument();
        play.rig.apply(Command::SetMonitor {
            track: other,
            monitor: Monitor::Off,
        });
        // The song stays stopped: one block locks the block clock.
        play.block();
        let preview = MidiSource::Preview(previewed.0);
        play.send(preview, 8192 + 7, MidiMessage::note_on(0, 64, 100));
        play.send(preview, 8192 + 2000, MidiMessage::note_off(0, 64));
        play.run_to(8192 + 5000);

        // The track doesn't monitor and the transport is stopped, yet its
        // device heard the note and, on release, its note-off; the other
        // track's device heard nothing. (A stopped song's frames are offsets
        // in the block, so only the notes are compared.)
        let heard: Vec<_> = play
            .heard()
            .into_iter()
            .map(|(_, kind, key)| (kind, key))
            .collect();
        assert_eq!(
            heard,
            [(NoteKind::On, 64), (NoteKind::Off, 64)],
            "block size {size}"
        );
        // It reached no track's input: recording and Capture MIDI take
        // nothing from it, and the other track played nothing.
        for id in [previewed, other] {
            assert!(play.rig.processor.midi_input(id).unwrap().events.is_empty());
        }
    }
}

/// Holds a note on `track` and returns the frame the next block starts at
/// once the edit `change` is applied, then the notes heard after it.
fn held_then(
    size: usize,
    prepare: impl Fn(&mut Play, TrackId),
    change: impl Fn(&mut Play, TrackId),
) -> (Play, Vec<(i64, NoteKind, u8)>) {
    let mut play = Play::new(size);
    let id = play.instrument();
    prepare(&mut play, id);
    play.play();
    play.send(KEYBOARD, 8192, on(60));
    play.send(KEYBOARD, 8192 + 3, MidiMessage::note_on(0, 62, 90));
    play.send(KEYBOARD, 8192 + 5, off(62));
    play.run_to(8192 + 100);
    assert_eq!(play.heard().len(), 3);
    change(&mut play, id);
    // The edit lands at the next block's first frame.
    let next = play.blocks * size as u64;
    play.run_to(next + 2 * size as u64);
    let heard = play.heard();
    assert_eq!(
        heard,
        [(next as i64, NoteKind::Off, 60)],
        "block size {size}: only the held note ends, on the first frame"
    );
    // Nothing ends twice, and later input isn't played.
    play.send(KEYBOARD, next + 4 * size as u64, off(60));
    play.run_to(next + 8 * size as u64);
    assert_eq!(play.heard(), [], "block size {size}");
    (play, heard)
}

// Spec: midi-io R26
#[test]
fn switching_monitoring_off_ends_the_held_notes() {
    for size in SIZES {
        held_then(
            size,
            |_, _| {},
            |play, id| {
                play.rig.apply(Command::SetMonitor {
                    track: id,
                    monitor: Monitor::Off,
                });
            },
        );
    }
}

// Spec: midi-io R26
#[test]
fn disarming_a_track_on_auto_ends_the_held_notes() {
    for size in SIZES {
        held_then(
            size,
            |play, id| {
                play.rig.apply(Command::SetMonitor {
                    track: id,
                    monitor: Monitor::Auto,
                });
                play.rig.apply(Command::SetArm {
                    track: id,
                    arm: true,
                });
            },
            |play, id| {
                play.rig.apply(Command::SetArm {
                    track: id,
                    arm: false,
                });
            },
        );
    }
}

// Spec: midi-io R26
#[test]
fn changing_the_input_or_its_channel_ends_the_held_notes() {
    for size in SIZES {
        held_then(
            size,
            |_, _| {},
            |play, id| {
                play.rig.apply(Command::SetInput {
                    track: id,
                    input: Input::AllIns,
                    channel: None,
                });
                play.rig.apply(Command::SetInput {
                    track: id,
                    input: Input::Port {
                        name: "Keys".to_owned(),
                    },
                    channel: Some(2),
                });
            },
        );
        // Another input, same monitoring.
        held_then(
            size,
            |_, _| {},
            |play, id| {
                play.rig.apply(Command::SetInput {
                    track: id,
                    input: Input::None,
                    channel: None,
                });
            },
        );
    }
}

// Spec: midi-io R26
#[test]
fn an_edit_that_keeps_the_input_keeps_the_held_notes_sounding() {
    let mut play = Play::new(512);
    let id = play.instrument();
    play.play();
    play.send(KEYBOARD, 8192, on(60));
    play.run_to(8192 + 600);
    // A new device is a new program, but the route is the same.
    device(&mut play.rig.editor, id, testing::GAIN, &[]);
    play.rig.sync();
    play.send(KEYBOARD, 10_240, off(60));
    play.run_to(10_240 + 1000);
    assert_eq!(
        play.heard(),
        [(8192, NoteKind::On, 60), (10_240, NoteKind::Off, 60)]
    );
}
