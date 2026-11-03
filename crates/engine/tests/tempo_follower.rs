//! Tempo Follower in the engine (WP-9.13; `docs/specs/tempo-follower.md` R6
//! to R10, R20): a [`TempoFollower`] over a recorded tracker output, which a
//! test replays at blocks of 1, 61 and 512 frames. Every block runs as a
//! real-time section under the allocation trap. A block hears what the
//! recording gave from the frame its first frame is, so each change sits on
//! a frame that every block size starts a block on.

use crate::common;
use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::testing::{self, recorded_tracker};
use bl_engine::{DeviceInput, LinkFollower, TempoFollower, TransportCommand};
use bl_model::{
    Clip, ClipData, Command, CountIn, MetronomeRhythm, MetronomeSound, MidiData, Note, TrackKind,
};
use common::{Rig, device, track};

const BAR: Ticks = Ticks(TICKS_PER_QUARTER * 4);

/// The block sizes every test runs at.
const BLOCKS: [usize; 3] = [1, 61, 512];

/// A frame every block size starts a block on: 1, 61 and 512 share their
/// product.
const ALIGNED: u64 = 61 * 512;

/// The device's input: silence on two channels, as long as the longest block.
struct Silence([Vec<f32>; 2]);

impl Silence {
    fn new() -> Self {
        Self([vec![0.0; 4096], vec![0.0; 4096]])
    }
}

impl DeviceInput for Silence {
    fn channels(&self) -> usize {
        2
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.0[index]
    }
}

fn tempo(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).unwrap())
}

const NOTES: [(u8, i64, i64); 6] = [
    (60, 0, 2),
    (62, 4, 4),
    (64, 6, 1),
    (65, 8, 3),
    (67, 16, 4),
    (69, 22, 2),
];

/// A rig with an audio input, under `map`, whose track's notes sound as
/// pulses and whose clip holds `NOTES`, with the metronome clicking.
fn rig(map: TempoMap) -> Rig {
    let mut rig = Rig::with_input(4096, 2);
    rig.editor.apply(Command::SetTempo { tempo: map }).unwrap();
    rig.editor
        .apply(Command::SetMetronomeEnabled { enabled: true })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeCountIn {
            count_in: CountIn::None,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeSound {
            sound: MetronomeSound::Classic,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeRhythm {
            rhythm: MetronomeRhythm::Quarter,
        })
        .unwrap();
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, a, testing::NOTE_PULSE, &[]);
    let notes: Vec<Note> = NOTES
        .iter()
        .map(|&(key, start, length)| {
            Note::new(
                key,
                Ticks::SIXTEENTH * start,
                Ticks::SIXTEENTH * length,
                100,
            )
        })
        .collect();
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: a,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                BAR * 4,
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
    rig.sync();
    rig
}

/// Renders `total` frames of `rig` in blocks of `block`, hearing silence.
fn render(rig: &mut Rig, total: usize, block: usize) -> Vec<f32> {
    rig.render_with_input(total, block, &Silence::new())
}

/// The frames where the output isn't zero.
fn nonzero(out: &[f32]) -> Vec<usize> {
    (0..out.len()).filter(|&i| out[i] != 0.0).collect()
}

fn same(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: lengths");
    if let Some(at) = (0..got.len()).find(|&i| got[i] != want[i]) {
        panic!(
            "{what}: frame {at} is {} where {} is wanted; the frames that sound: {:?} against {:?}",
            got[at],
            want[at],
            &nonzero(got)[..nonzero(got).len().min(12)],
            &nonzero(want)[..nonzero(want).len().min(12)],
        );
    }
}

#[test]
fn the_song_plays_the_tempo_the_tracker_gives_on_every_frame() {
    // Spec: tempo-follower R6. A set at 120 BPM whose tracker says 140 plays
    // what the same set at 140 BPM plays: notes and clicks on their frames.
    for block in BLOCKS {
        let mut reference = rig(tempo(140.0));
        reference.sender.transport(TransportCommand::Play);
        let want = render(&mut reference, 110_000, block);
        assert!(nonzero(&want).len() > 12, "the reference sounds");

        let (tracker, _) = recorded_tracker(&[(0, 140.0, true)]);
        let mut rig = rig(tempo(120.0));
        rig.lead(TempoFollower::new(tracker));
        rig.sender.transport(TransportCommand::Play);
        let got = render(&mut rig, 110_000, block);
        same(&got, &want, &format!("at blocks of {block}"));

        let mut alone = self::rig(tempo(120.0));
        alone.sender.transport(TransportCommand::Play);
        assert_ne!(
            render(&mut alone, 110_000, block),
            want,
            "the map isn't played"
        );
    }
}

#[test]
fn a_change_of_the_tracked_tempo_is_played_from_the_block_it_reaches() {
    // Spec: tempo-follower R6. 120 BPM for the first stretch, then 150: the
    // same as a follower that makes that change on that frame.
    for block in BLOCKS {
        let at = 2 * ALIGNED;
        let (tracker, told) = recorded_tracker(&[(0, 120.0, true), (at, 150.0, true)]);
        let mut rig = rig(tempo(100.0));
        rig.lead(TempoFollower::new(tracker));
        rig.sender.transport(TransportCommand::Play);
        let got = render(&mut rig, 160_000, block);

        let mut reference = self::rig(tempo(100.0));
        reference.follow(testing::stand_in(move |ask, answer| {
            answer.tempo(if ask.clock < at { 120.0 } else { 150.0 });
        }));
        reference.sender.transport(TransportCommand::Play);
        let want = render(&mut reference, 160_000, block);
        same(&got, &want, &format!("at blocks of {block}"));
        // The main thread is told once for each tempo, not once a block.
        assert_eq!(told.all(), vec![120.0, 150.0], "block {block}");
    }
}

#[test]
fn the_last_confident_tempo_is_kept_and_none_is_followed_before_the_first() {
    // Spec: tempo-follower R6; the tracker's R14. Not confident at first: the
    // song plays its own 100 BPM. Confident at 130, then not: 130 stays.
    for block in BLOCKS {
        let (tracker, told) = recorded_tracker(&[
            (0, 90.0, false),
            (ALIGNED, 130.0, true),
            (2 * ALIGNED, 70.0, false),
        ]);
        let mut rig = rig(tempo(100.0));
        rig.lead(TempoFollower::new(tracker));
        rig.sender.transport(TransportCommand::Play);
        let got = render(&mut rig, 3 * ALIGNED as usize, block);

        let mut reference = self::rig(tempo(100.0));
        reference.follow(testing::stand_in(|ask, answer| {
            if ask.clock >= ALIGNED {
                answer.tempo(130.0);
            }
        }));
        reference.sender.transport(TransportCommand::Play);
        let want = render(&mut reference, 3 * ALIGNED as usize, block);
        same(&got, &want, &format!("at blocks of {block}"));
        assert_eq!(told.all(), vec![130.0]);
    }
}

#[test]
fn the_tempo_is_followed_while_stopped_and_play_starts_at_it() {
    // Spec: tempo-follower R7, R10. The tracker is heard before Play: the
    // engine reports the followed tempo, and Play starts at once at it.
    for block in BLOCKS {
        let (tracker, _) = recorded_tracker(&[(0, 140.0, true)]);
        let mut rig = rig(tempo(120.0));
        rig.lead(TempoFollower::new(tracker));
        render(&mut rig, 2 * ALIGNED as usize, block);
        assert_eq!(rig.monitor.state().following, Some(140.0), "block {block}");
        assert!(!rig.monitor.state().playing);

        rig.sender.transport(TransportCommand::Play);
        let got = render(&mut rig, 110_000, block);
        let mut reference = self::rig(tempo(140.0));
        reference.sender.transport(TransportCommand::Play);
        let want = render(&mut reference, 110_000, block);
        // Played at once: the notes sit where a 140 BPM song's do.
        same(&got, &want, &format!("at blocks of {block}"));
    }
}

#[test]
fn taking_follow_away_gives_the_set_its_tempo_with_the_beat_where_it_was() {
    // Spec: tempo-follower R9. Following 140 over a 120 BPM map, then not:
    // the next block plays at 120 BPM from where the song's beat was.
    for block in BLOCKS {
        let (tracker, _) = recorded_tracker(&[(0, 140.0, true)]);
        let mut rig = rig(tempo(120.0));
        rig.lead(TempoFollower::new(tracker));
        rig.sender.transport(TransportCommand::Play);
        let part = ALIGNED as usize;
        let mut got = render(&mut rig, part, block);
        rig.lead_none();
        got.extend(render(&mut rig, 60_000, block));
        assert_eq!(rig.monitor.state().following, None, "block {block}");

        // The reference follows 140 for the same frames, then plays 120.
        let mut reference = self::rig(tempo(120.0));
        reference.follow(testing::stand_in(move |ask, answer| {
            if ask.clock < part as u64 {
                answer.tempo(140.0);
            }
        }));
        reference.sender.transport(TransportCommand::Play);
        let want = render(&mut reference, part + 60_000, block);
        same(&got, &want, &format!("at blocks of {block}"));
    }
}

#[test]
fn a_followed_tempo_takes_a_tempo_envelopes_place() {
    // Spec: tempo-follower R8. A ramp from 120 to 140 over four bars:
    // before the tracker is confident the envelope plays and nothing is
    // overridden; once it is, Re-Enable Automation lights.
    let ramp = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (Ticks(TICKS_PER_QUARTER * 16), Bpm::new(140.0).unwrap()),
    ])
    .unwrap();
    for block in BLOCKS {
        let (tracker, _) = recorded_tracker(&[(0, 100.0, false), (ALIGNED, 100.0, true)]);
        let mut rig = rig(ramp.clone());
        rig.lead(TempoFollower::new(tracker));
        rig.sender.transport(TransportCommand::Play);
        render(&mut rig, ALIGNED as usize, block);
        assert!(
            !rig.monitor.state().tempo_overridden,
            "block {block}: the envelope plays"
        );
        render(&mut rig, 2 * block, block);
        assert!(
            rig.monitor.state().tempo_overridden,
            "block {block}: Re-Enable Automation lights"
        );
        assert_eq!(rig.monitor.state().following, Some(100.0));
    }
}

#[test]
fn with_link_on_the_session_gets_the_followed_tempo() {
    // Spec: tempo-follower R20; link R26. A song in a session at 120 BPM
    // whose tracker becomes confident at 140: the session's tempo becomes
    // 140 as Beatlayer's own change, not a peer's.
    for block in BLOCKS {
        let (tracker, _) = recorded_tracker(&[(0, 120.0, false), (ALIGNED, 140.0, true)]);
        let mut rig = rig(tempo(120.0));
        let (session, peers) = testing::link_session(120.0);
        peers.set_peers(1);
        rig.follow(LinkFollower::new(session));
        rig.lead(TempoFollower::new(tracker));
        rig.sender.transport(TransportCommand::Play);
        render(&mut rig, 2 * ALIGNED as usize, block);
        assert!(
            (peers.tempo() - 140.0).abs() < 0.01,
            "block {block}: the session is at {}",
            peers.tempo()
        );
        assert!(peers.told().is_empty(), "the followed tempo isn't a peer's");
        assert_eq!(rig.monitor.state().following, Some(140.0));
    }
}

#[test]
fn phase_nudge_does_nothing_while_the_tracker_leads() {
    // Spec: tempo-follower R21; transport R29. A held nudge changes nothing
    // while Tempo Follower gives the tempo: the song plays what it plays
    // with no nudge.
    use bl_engine::NudgeDirection;
    for block in BLOCKS {
        let play = |nudge: bool| {
            let (tracker, _) = recorded_tracker(&[(0, 140.0, true)]);
            let mut rig = rig(tempo(120.0));
            rig.lead(TempoFollower::new(tracker));
            rig.sender.transport(TransportCommand::Play);
            let mut out = render(&mut rig, ALIGNED as usize, block);
            if nudge {
                rig.sender.transport(TransportCommand::PhaseNudge {
                    direction: NudgeDirection::Up,
                    held: true,
                });
            }
            out.extend(render(&mut rig, 60_000, block));
            assert_eq!(rig.monitor.state().phase_nudge, None);
            out
        };
        same(&play(true), &play(false), &format!("at blocks of {block}"));
    }
}
