//! Recording the Session into the Arrangement, in the engine (WP-3.13):
//! while an Arrangement take runs, every Session clip that starts, restarts,
//! stops or returns to the Arrangement is logged on its exact song frame,
//! and a take that begins with clips playing logs a start for each. Every
//! block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use bl_core::time::{Frames, Ticks};
use bl_engine::{
    LaunchLog, RecordingPlan, SessionCommand, SessionEvent, TransportCommand, testing,
};
use bl_model::{Clip, ClipData, Command, MidiData, Note, SceneId, TrackId, TrackKind};
use common::{Rig, device, track};
use rtrb::{Consumer, RingBuffer};

const BLOCK: usize = 256;
/// A 4/4 bar at 120 BPM and 48 kHz.
const BAR_FRAMES: i64 = 96_000;
const BAR: Ticks = Ticks(Ticks::WHOLE.get() * 4);

/// A rig with two MIDI tracks and a clip in each of three slots, and the
/// frames played so far.
struct Song {
    rig: Rig,
    played: i64,
    log: Consumer<SessionEvent>,
    dropped: Arc<AtomicU64>,
    a: TrackId,
    b: TrackId,
    scenes: [SceneId; 2],
}

impl Song {
    fn new() -> Self {
        let mut rig = Rig::new(BLOCK);
        let scenes: Vec<SceneId> = rig
            .editor
            .document()
            .scenes()
            .map(|s| s.id)
            .take(2)
            .collect();
        let mut tracks = Vec::new();
        for _ in 0..2 {
            let t = track(&mut rig.editor, TrackKind::Midi, None);
            device(&mut rig.editor, t, testing::INSTRUMENT, &[]);
            tracks.push(t);
        }
        for (t, scene) in [
            (tracks[0], scenes[0]),
            (tracks[0], scenes[1]),
            (tracks[1], scenes[0]),
        ] {
            let id = rig.editor.new_id();
            let note = Note::new(60, Ticks::ZERO, Ticks::QUARTER, 100);
            let clip = Clip::new(id, Ticks::ZERO, BAR, ClipData::Midi(MidiData::new([note])));
            rig.editor
                .apply(Command::CreateSlotClip {
                    track: t,
                    scene,
                    clip,
                })
                .unwrap();
        }
        rig.sync();
        let (producer, log) = RingBuffer::new(64);
        let dropped = Arc::new(AtomicU64::new(0));
        let plan = RecordingPlan {
            tracks: Vec::new(),
            midi: Vec::new(),
            delay: 0,
            session: Vec::new(),
            launches: Some(LaunchLog {
                producer,
                dropped: Arc::clone(&dropped),
            }),
        };
        rig.sender.record(rig.collector.handle().wrap(plan));
        Self {
            rig,
            played: 0,
            log,
            dropped,
            a: tracks[0],
            b: tracks[1],
            scenes: [scenes[0], scenes[1]],
        }
    }

    /// Plays on to song frame `to`, a multiple of the block.
    fn play_to(&mut self, to: i64) {
        while self.played < to {
            self.rig.block(BLOCK);
            self.played += BLOCK as i64;
        }
    }

    fn launch(&mut self, track: TrackId, scene: usize) {
        let scene = self.scenes[scene];
        self.rig
            .sender
            .session(SessionCommand::Launch { track, scene });
    }

    fn events(&mut self) -> Vec<(TrackId, usize, i64, Option<i64>)> {
        self.rig.collector.collect();
        let mut events = Vec::new();
        while let Ok(e) = self.log.pop() {
            let scene = self.scenes.iter().position(|&s| s == e.scene).unwrap();
            events.push((e.track, scene, e.at.get(), e.launched.map(Frames::get)));
        }
        events
    }
}

// Spec: session-to-arrangement R1, R2. A clip already playing when the
// take begins is logged as started at the take's first frame, with the frame
// it launched on; a launch lands on its grid line; launching B while A plays
// ends A and starts B on one frame; a stop ends the clip on its grid line;
// Back to Arrangement ends what still plays where it was pressed.
#[test]
fn the_launcher_logs_each_start_and_end_on_its_frame() {
    let mut song = Song::new();
    let (a, b) = (song.a, song.b);
    // B's clip plays before the take, launched on the first frame.
    song.launch(b, 0);
    song.play_to(20 * BLOCK as i64);
    song.rig
        .sender
        .transport(TransportCommand::SetRecording(true));
    song.play_to(BAR_FRAMES / 2);
    let take_start = song.rig.monitor.state().take_so_far.unwrap().start.get();
    // A launch mid-bar lands on the next bar line.
    song.launch(a, 0);
    song.play_to(BAR_FRAMES * 3 / 2);
    // A second launch on the same track ends the first where it starts.
    song.launch(a, 1);
    song.play_to(BAR_FRAMES * 5 / 2);
    song.rig.sender.session(SessionCommand::StopTrack(a));
    song.play_to(BAR_FRAMES * 7 / 2);
    let back = song.played;
    song.rig.sender.session(SessionCommand::BackToArrangement);
    song.play_to(back + 2 * BLOCK as i64);
    assert_eq!(
        song.events(),
        vec![
            (b, 0, take_start, Some(0)),
            (a, 0, BAR_FRAMES, Some(BAR_FRAMES)),
            (a, 0, 2 * BAR_FRAMES, None),
            (a, 1, 2 * BAR_FRAMES, Some(2 * BAR_FRAMES)),
            (a, 1, 3 * BAR_FRAMES, None),
            (b, 0, back, None),
        ]
    );
    assert_eq!(song.dropped.load(std::sync::atomic::Ordering::Relaxed), 0);
}

// Spec: session-to-arrangement R1. Starts and ends before the take begins
// are not logged: the take logs the clips playing when it starts instead.
#[test]
fn nothing_is_logged_before_the_take_begins() {
    let mut song = Song::new();
    let a = song.a;
    song.launch(a, 0);
    song.play_to(2 * BAR_FRAMES);
    song.rig.sender.session(SessionCommand::StopTrack(a));
    song.play_to(4 * BAR_FRAMES);
    assert_eq!(song.events(), vec![]);
}
