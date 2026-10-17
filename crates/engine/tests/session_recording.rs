//! Session recording in the engine (WP-3.11): a take starts and ends on the
//! launch quantisation grid, or at once from the playhead while the song is
//! stopped, after any count-in; it ignores the Arrangement's loop; the
//! engine publishes the recording slots; and a recorded clip's slot is
//! launched where the take ends. Every block runs as a real-time section
//! under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_core::time::{Frames, Ticks};
use bl_engine::{
    LoopRegion, MidiRecordTrack, NoInput, RecordedMidi, RecordingPlan, SessionCommand,
    SessionTarget, SlotState, TakeBounds, TransportCommand,
};
use bl_model::{Command, CountIn, Monitor, SceneId, TrackId, TrackKind};
use common::{Rig, track};
use rtrb::{Consumer, Producer, RingBuffer};

const BLOCK: usize = 256;

/// A 4/4 bar at 120 BPM and 48 kHz, in frames and in blocks.
const BAR_FRAMES: i64 = 96_000;
const BAR_BLOCKS: u64 = 375;

/// The length of `frames` frames at 48 kHz.
fn frames(frames: u64) -> Duration {
    Duration::from_nanos(frames * 1_000_000_000 / 48_000)
}

/// A rig with a MIDI queue and an armed MIDI track, and the time its first
/// block starts.
struct Take {
    rig: Rig,
    queue: Producer<MidiInputEvent>,
    start: Instant,
    blocks: u64,
    track: TrackId,
    scene: SceneId,
    ring: Consumer<RecordedMidi>,
}

impl Take {
    fn new(setup: impl FnOnce(&mut Rig)) -> Self {
        let mut rig = Rig::new(BLOCK);
        let (queue, consumer) = RingBuffer::new(256);
        assert!(rig.processor.set_midi_input(consumer).is_none());
        setup(&mut rig);
        let track = track(&mut rig.editor, TrackKind::Midi, None);
        let scene = rig.editor.document().scenes().next().unwrap().id;
        rig.sync();
        rig.apply(Command::SetArm { track, arm: true });
        rig.apply(Command::SetMonitor {
            track,
            monitor: Monitor::Off,
        });
        let (producer, ring) = RingBuffer::new(64);
        let plan = RecordingPlan {
            tracks: Vec::new(),
            midi: vec![MidiRecordTrack {
                track,
                producer,
                dropped: Default::default(),
            }],
            delay: 0,
            session: vec![SessionTarget {
                track,
                scene,
                overdub: false,
            }],
            launches: None,
        };
        rig.sender.record(rig.collector.handle().wrap(plan));
        Self {
            rig,
            queue,
            start: Instant::now(),
            blocks: 0,
            track,
            scene,
            ring,
        }
    }

    /// Processes `count` blocks, called exactly on time.
    fn blocks(&mut self, count: u64) {
        for _ in 0..count {
            let now = self.start + frames(self.blocks * BLOCK as u64);
            let processor = &mut self.rig.processor;
            bl_rt::rt_test!({
                processor.process_at(BLOCK, &NoInput, now);
            });
            self.blocks += 1;
        }
    }

    /// Sends a message that arrives `after` frames into block `block`.
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

    fn session(&mut self, command: SessionCommand) {
        self.rig.sender.session(command);
    }

    fn take(&mut self) -> Option<TakeBounds> {
        self.rig.monitor.state().take
    }

    fn slot(&mut self) -> Option<SlotState> {
        self.rig
            .monitor
            .state()
            .session
            .iter()
            .find(|slot| slot.track == self.track && slot.scene == self.scene)
            .map(|slot| slot.state)
    }

    fn messages(&mut self) -> Vec<(i64, MidiMessage)> {
        self.rig.collector.collect();
        let mut messages = Vec::new();
        while let Ok(m) = self.ring.pop() {
            messages.push((m.frame.get(), m.message));
        }
        messages
    }
}

// Spec: session-recording R3. While the song plays, the take starts on the
// next grid line and ends on the next one after the stop is asked for, and
// the notes in between are stamped with the song frames they landed on.
#[test]
fn a_take_starts_and_ends_on_grid_lines() {
    let mut take = Take::new(|_| {});
    // The song is stopped, so Record starts it at once, from the playhead.
    take.session(SessionCommand::Record);
    take.blocks(100);
    assert_eq!(
        take.slot(),
        Some(SlotState::Recording),
        "recording is shown at once"
    );
    let (on, off) = (
        MidiMessage::note_on(0, 60, 100),
        MidiMessage::note_off(0, 60),
    );
    take.send(BAR_BLOCKS + 5, 100, on);
    take.send(BAR_BLOCKS + 15, 40, off);
    take.blocks(BAR_BLOCKS + 50 - 100);
    assert_eq!(take.take(), None, "the take hasn't ended");
    take.session(SessionCommand::StopRecording);
    take.blocks(2 * BAR_BLOCKS);

    // The stop, asked for in the second bar, waited for the line at bar 2.
    assert_eq!(
        take.take(),
        Some(TakeBounds {
            start: Frames(0),
            end: Frames(2 * BAR_FRAMES),
            wrap: None,
        })
    );
    assert_eq!(
        take.messages(),
        vec![
            // A message is placed in the block after the one it arrived in.
            ((BAR_BLOCKS as i64 + 6) * BLOCK as i64 + 100, on),
            ((BAR_BLOCKS as i64 + 16) * BLOCK as i64 + 40, off),
        ]
    );
    assert_eq!(
        take.slot(),
        Some(SlotState::Playing),
        "the recorded clip's slot loops on from where the take ended"
    );
}

// Spec: session-recording R3. With the song already playing, a recording
// asked for mid-bar waits for the next bar line.
#[test]
fn a_recording_asked_for_mid_bar_waits_for_the_line() {
    let mut take = Take::new(|_| {});
    take.rig.sender.transport(TransportCommand::Play);
    take.blocks(100);
    take.session(SessionCommand::Record);
    let on = MidiMessage::note_on(0, 62, 90);
    take.send(BAR_BLOCKS + 10, 100, on);
    take.blocks(BAR_BLOCKS + 20);
    take.session(SessionCommand::StopRecording);
    take.blocks(BAR_BLOCKS + 10);
    let bounds = take.take().expect("a take");
    assert_eq!(bounds.start, Frames(BAR_FRAMES));
    assert_eq!(bounds.end, Frames(2 * BAR_FRAMES));
    assert_eq!(
        take.messages(),
        vec![((BAR_BLOCKS as i64 + 11) * BLOCK as i64 + 100, on)]
    );
}

// Spec: session-recording R6. A count-in runs before a recording that
// starts the transport, and the take starts when it ends.
#[test]
fn a_count_in_delays_the_take_by_a_bar() {
    let mut take = Take::new(|rig| {
        rig.editor
            .apply(Command::SetMetronomeEnabled { enabled: true })
            .unwrap();
        rig.editor
            .apply(Command::SetMetronomeCountIn {
                count_in: CountIn::Bars1,
            })
            .unwrap();
    });
    take.session(SessionCommand::Record);
    take.blocks(BAR_BLOCKS + 20);
    assert!(
        take.rig.monitor.state().playing,
        "recording starts the transport"
    );
    take.session(SessionCommand::StopRecording);
    take.blocks(BAR_BLOCKS + 5);
    let bounds = take.take().expect("a take");
    assert_eq!(
        bounds.start,
        Frames::ZERO,
        "the take starts as the count-in ends"
    );
    assert_eq!(bounds.end, Frames(BAR_FRAMES));
}

// Spec: session-recording R3. The Arrangement's loop and punch points
// don't touch a Session take: it runs on through the loop's wraps and its
// stamps run on with it.
#[test]
fn the_arrangement_loop_does_not_break_a_take() {
    let mut take = Take::new(|_| {});
    take.rig
        .sender
        .transport(TransportCommand::SetLoop(Some(LoopRegion {
            start: Ticks::ZERO,
            end: Ticks::WHOLE,
        })));
    take.session(SessionCommand::Record);
    // Three bars, one bar of loop: the playhead wraps twice.
    let on = MidiMessage::note_on(0, 64, 80);
    take.send(2 * BAR_BLOCKS + 7, 3, on);
    take.blocks(2 * BAR_BLOCKS + 20);
    take.session(SessionCommand::StopRecording);
    take.blocks(BAR_BLOCKS);
    assert_eq!(
        take.take(),
        Some(TakeBounds {
            start: Frames::ZERO,
            end: Frames(3 * BAR_FRAMES),
            wrap: None,
        })
    );
    assert_eq!(
        take.messages(),
        vec![((2 * BAR_BLOCKS as i64 + 8) * BLOCK as i64 + 3, on)]
    );
}

// Spec: session-recording R5. Stop ends the take where the playhead is,
// and the slot is not launched.
#[test]
fn stop_ends_the_take_without_launching_the_slot() {
    let mut take = Take::new(|_| {});
    take.session(SessionCommand::Record);
    take.blocks(40);
    take.rig.sender.transport(TransportCommand::Stop);
    take.blocks(2);
    let bounds = take.take().expect("a take");
    assert_eq!(bounds.start, Frames::ZERO);
    assert_eq!(bounds.end, Frames(40 * BLOCK as i64));
    assert_eq!(take.slot(), None, "nothing plays or records");
    take.rig.sender.transport(TransportCommand::Play);
    take.blocks(BAR_BLOCKS + 10);
    assert_eq!(take.slot(), None, "and Play doesn't start the slot later");
}

// Spec: session-recording R4. The launch button of a recording slot ends
// the recording and loops the clip; a clip stop ends it without looping.
#[test]
fn a_launch_of_the_recording_slot_ends_it_and_a_stop_does_not_loop() {
    let mut launched = Take::new(|_| {});
    launched.session(SessionCommand::Record);
    launched.blocks(20);
    launched.session(SessionCommand::Launch {
        track: launched.track,
        scene: launched.scene,
    });
    launched.blocks(BAR_BLOCKS);
    assert_eq!(launched.take().map(|t| t.end), Some(Frames(BAR_FRAMES)));
    assert_eq!(launched.slot(), Some(SlotState::Playing));

    let mut stopped = Take::new(|_| {});
    stopped.session(SessionCommand::Record);
    stopped.blocks(20);
    stopped.session(SessionCommand::StopTrack(stopped.track));
    stopped.blocks(BAR_BLOCKS);
    assert_eq!(stopped.take().map(|t| t.end), Some(Frames(BAR_FRAMES)));
    assert_eq!(stopped.slot(), None);
}

// Spec: session-recording R3. With quantisation off, the take starts and
// ends on the frames of the commands.
#[test]
fn without_quantisation_a_take_runs_from_press_to_press() {
    let mut take = Take::new(|_| {});
    take.rig
        .sender
        .session(SessionCommand::SetQuantization(bl_model::LaunchSettings {
            quantization: bl_model::LaunchQuantization::Bar,
            enabled: false,
        }));
    take.rig.sender.transport(TransportCommand::Play);
    take.blocks(10);
    take.session(SessionCommand::Record);
    take.blocks(30);
    take.session(SessionCommand::StopRecording);
    take.blocks(5);
    assert_eq!(
        take.take(),
        Some(TakeBounds {
            start: Frames(10 * BLOCK as i64),
            end: Frames(40 * BLOCK as i64),
            wrap: None,
        })
    );
}

// Spec: session-recording R4. The slot launches on the frame the take ends
// on, so a clip that reaches the program afterwards plays in step with it:
// from that frame, and round again a loop later.
#[test]
fn a_recorded_clip_plays_in_step_with_where_the_take_ended() {
    use bl_core::time::Bpm;
    use bl_model::{Clip, ClipData, MidiData, Note};

    let mut take = Take::new(|_| {});
    common::device(
        &mut take.rig.editor,
        take.track,
        bl_engine::testing::INSTRUMENT,
        &[],
    );
    take.rig.apply(Command::SetTempo {
        tempo: bl_core::time::TempoMap::constant(Bpm::new(120.0).unwrap()),
    });
    take.session(SessionCommand::Record);
    take.blocks(20);
    take.session(SessionCommand::StopRecording);
    // The take ends on the bar line; the shell makes the clip a poll later.
    take.blocks(BAR_BLOCKS - 20 + 2);
    let bar = Ticks::WHOLE;
    let id = take.rig.editor.new_id();
    let clip = Clip::new(
        id,
        Ticks::ZERO,
        bar,
        ClipData::Midi(MidiData {
            looping: true,
            ..MidiData::new([Note::new(60, Ticks::QUARTER, Ticks::QUARTER, 100)])
        }),
    );
    take.rig.apply(Command::CreateSlotClip {
        track: take.track,
        scene: take.scene,
        clip,
    });
    take.blocks(2 * BAR_BLOCKS);
    let on: Vec<i64> = take
        .rig
        .probe
        .recorded_notes()
        .into_iter()
        .filter(|note| note.event.kind == bl_engine::NoteKind::On)
        .map(|note| note.frame)
        .collect();
    // A quarter into each bar after the take ended at bar 1.
    assert_eq!(on, vec![BAR_FRAMES + 24_000, 2 * BAR_FRAMES + 24_000]);
}
