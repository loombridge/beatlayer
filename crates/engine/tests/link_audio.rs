//! Link Audio in the engine (WP-7.16, `docs/specs/link-audio.md` R4 to R8):
//! a track whose input is a peer's channel plays the frames the peer sent on
//! the beat they were sent on plus the Latency, drops and counts what comes
//! too late, and the tracks the engine offers are staged after their mixers
//! and sent each block with the moment it is heard. The session and the
//! channels are stand-ins ([`testing::link_session`], [`testing::link_audio`])
//! a test drives, so no test waits for time; every block runs as a
//! real-time section under the allocation trap, at blocks of 1, 61, 512 and
//! 4,096 frames. The session plays 120 BPM, 24,000 frames to the beat, with
//! its clock a number the test sets for each block's first frame.

use crate::common;
use bl_core::time::{Bpm, TempoMap};
use bl_engine::testing::{self, LinkAudioPeers, LinkPeers};
use bl_engine::{LinkChannel, LinkFollower, frame_at};
use bl_model::{Command, CountIn, TrackId, TrackKind};
use common::{RATE, Rig, device, track};
use serde_json::json;

/// The block sizes every test runs at.
const BLOCKS: [usize; 4] = [1, 61, 512, 4096];

/// Frames to the beat.
const BEAT: u64 = 24_000;

/// The Latency the tests set: 20 ms.
const LATENCY: u64 = 960;

fn micros(frames: u64) -> i64 {
    (frames as f64 * 1e6 / RATE.as_f64()).round() as i64
}

/// A rig with an audio track that monitors a peer's channel, following a
/// session, with Link Audio installed.
struct Host {
    rig: Rig,
    peers: LinkPeers,
    link: LinkAudioPeers,
    channel: std::sync::Arc<LinkChannel>,
    track: TrackId,
    block: usize,
    done: u64,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Host {
    fn new(block: usize) -> Self {
        Self::offering(block, |_| Vec::new())
    }

    /// A host that also offers the tracks `offer` picks, made after the
    /// track that takes the channel.
    fn offering(block: usize, offer: impl FnOnce(&mut Rig) -> Vec<TrackId>) -> Self {
        let mut rig = Rig::new(block.max(512));
        rig.editor
            .apply(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
            })
            .unwrap();
        rig.editor
            .apply(Command::SetMetronomeCountIn {
                count_in: CountIn::None,
            })
            .unwrap();
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        for (id, args) in [
            (
                "routing.set_input",
                json!({ "track": a, "input": {
                    "type": "link_audio", "peer": "Studio", "channel": "Kick"
                } }),
            ),
            (
                "routing.set_monitor",
                json!({ "track": a, "monitor": "in" }),
            ),
        ] {
            bl_model::registry::run(&mut rig.editor, id, args).unwrap();
        }
        let offered = offer(&mut rig);
        rig.sync();
        let (session, peers) = testing::link_session(120.0);
        rig.follow(LinkFollower::new(session));
        let (link_audio, link) = testing::link_audio(
            RATE.hz(),
            block.max(512),
            &[("Studio", "Kick")],
            &offered,
        );
        rig.link_audio(link_audio);
        let channel = link.channel(0);
        let mut host = Self {
            rig,
            peers,
            link,
            channel,
            track: a,
            block,
            done: 0,
            left: Vec::new(),
            right: Vec::new(),
        };
        // The follower and Link Audio arrive in the first block.
        host.run(block as u64);
        host.left.clear();
        host.right.clear();
        host
    }

    /// Runs the engine up to frame `to`, setting Link's clock at each
    /// block's start.
    fn run_to(&mut self, to: u64) {
        while self.done < to {
            let n = self.block.min((to - self.done) as usize);
            self.peers.set_clock(micros(self.done));
            self.rig.block(n);
            let out = self.rig.processor.output();
            self.left.extend_from_slice(out.channel(0));
            self.right.extend_from_slice(out.channel(1));
            self.done += n as u64;
        }
    }

    fn run(&mut self, frames: u64) {
        self.run_to(self.done + frames);
    }

    /// The first frame, from `from`, whose left channel passes 0.01.
    fn heard(&self, from: usize) -> Option<usize> {
        (from..self.left.len()).find(|&i| self.left[i].abs() > 0.01)
    }

    /// A click as a peer sends it for the beat `beat`: where it plays is the
    /// beat plus the Latency, on the frame the session's clock reaches.
    fn click(&self, beat: u64, level: f32) {
        let at = frame_at(500_000 * beat as i64 + micros(LATENCY), RATE.hz());
        self.channel.push(at, &[level], &[level]);
    }
}

// Spec: link-audio R5, R6
#[test]
fn a_peers_buffers_play_on_their_beat_plus_the_latency() {
    for block in BLOCKS {
        let mut host = Host::new(block);
        host.click(1, 0.5);
        host.click(2, 0.5);
        host.run(3 * BEAT);
        // The first block ran before the log began, so frames count from
        // there: the click for beat 1 plays `LATENCY` after it.
        let first = host.heard(0).expect("the first click") as u64 + host.done
            - host.left.len() as u64;
        assert!(
            first.abs_diff(BEAT + LATENCY) <= 1,
            "block {block}: the first click played on {first}"
        );
        let second = host.heard(host.heard(0).unwrap() + 1).expect("the second") as u64
            + host.done
            - host.left.len() as u64;
        assert!(
            second.abs_diff(2 * BEAT + LATENCY) <= 1,
            "block {block}: the second click played on {second}"
        );
        assert_eq!(host.channel.dropped(), 0);
    }
}

// Spec: link-audio R5
#[test]
fn a_channel_plays_on_both_sides_and_a_mono_one_as_identical_sides() {
    let mut host = Host::new(512);
    // A channel with a different sample on each side, and one mono.
    let at = frame_at(500_000 + micros(LATENCY), RATE.hz());
    host.channel.push(at, &[0.5], &[0.25]);
    host.channel.push(at + 10, &[0.5], &[0.5]);
    host.run(2 * BEAT);
    let i = host.heard(0).unwrap();
    assert!(host.right[i] > 0.01 && host.left[i] > host.right[i], "stereo");
    let j = i + 10;
    assert!(host.left[j] > 0.01);
    assert_eq!(host.left[j], host.right[j], "a mono channel is the same on both sides");
}

// Spec: link-audio R6
#[test]
fn a_late_buffer_is_dropped_counted_and_silent() {
    for block in BLOCKS {
        let mut host = Host::new(block);
        host.run(2 * BEAT);
        // The click's frame passed 0.5 s ago.
        host.click(1, 0.5);
        assert_eq!(host.channel.dropped(), 1, "block {block}");
        host.run(BEAT);
        assert_eq!(host.heard(0), None, "block {block}: late audio is silent");
        // And one that is on time still plays.
        host.click(5, 0.5);
        host.run(3 * BEAT);
        assert!(host.heard(0).is_some(), "block {block}");
        assert_eq!(host.channel.dropped(), 1);
    }
}

// Spec: link-audio R5, R10
#[test]
fn a_channel_that_is_not_there_or_a_link_audio_that_is_off_is_silent() {
    let mut host = Host::new(512);
    host.click(1, 0.5);
    host.link.set_active(false);
    host.run(2 * BEAT);
    assert_eq!(host.heard(0), None, "off");

    // A track naming a peer that isn't in the session hears nothing, and
    // keeps the names.
    let mut host = Host::new(512);
    bl_model::registry::run(
        &mut host.rig.editor,
        "routing.set_input",
        json!({ "track": host.track, "input": {
            "type": "link_audio", "peer": "Gone", "channel": "Kick"
        } }),
    )
    .unwrap();
    host.rig.sync();
    host.click(1, 0.5);
    host.run(2 * BEAT);
    assert_eq!(host.heard(0), None, "a missing peer");
    assert_eq!(
        host.rig.editor.document().track(host.track).unwrap().routing().input,
        bl_model::Input::LinkAudio {
            peer: "Gone".into(),
            channel: "Kick".into()
        }
    );
}

// Spec: link-audio R4
#[test]
fn an_offered_track_is_sent_after_its_mixer_each_block_with_the_moment_it_is_heard() {
    for block in BLOCKS {
        let mut offered = None;
        let mut host = Host::offering(block, |rig| {
            let b = track(&mut rig.editor, TrackKind::Audio, None);
            device(&mut rig.editor, b, testing::DC, &[(bl_model::ParamId(0), 0.25)]);
            offered = Some(b);
            vec![b]
        });
        let b = offered.unwrap();
        host.run(2 * block as u64);
        let sent = host.link.sent();
        let mine: Vec<_> = sent.iter().filter(|s| s.track == b).collect();
        // The block the host began with, and the two that followed.
        assert!(mine.len() >= 3, "block {block}: {} sent", mine.len());
        for (n, one) in mine.iter().enumerate() {
            assert_eq!(one.left.len(), block);
            assert_eq!(one.right.len(), block);
            assert!(one.left.iter().all(|&s| (s - 0.25).abs() < 1e-3 || n == 0));
            if n > 0 {
                let expected = micros((n as u64 - 1) * block as u64 + block as u64);
                assert!(
                    (one.moment - expected).abs() <= 2,
                    "block {block}: block {n} sent for {} not {expected}",
                    one.moment
                );
            }
        }
        assert!(mine.last().unwrap().left.iter().all(|&s| s != 0.0));
    }
}

// Spec: link-audio R4, R1
#[test]
fn nothing_is_sent_while_link_audio_is_off() {
    let mut offered = None;
    let mut host = Host::offering(512, |rig| {
        let b = track(&mut rig.editor, TrackKind::Audio, None);
        offered = Some(b);
        vec![b]
    });
    let before = host.link.sent().len();
    host.link.set_active(false);
    host.run(2048);
    assert_eq!(host.link.sent().len(), before);
    host.link.set_active(true);
    host.run(2048);
    assert!(host.link.sent().len() > before);
    assert!(offered.is_some());
}

// Spec: link-audio R5; recording R1. An armed track records what its peer's
// channel plays, as it records Ext. In: the frames it heard, stereo.
#[test]
fn an_armed_track_records_what_a_peers_channel_plays() {
    use bl_engine::{RecordTrack, RecordingPlan, TransportCommand};
    use bl_model::Channel;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    let mut host = Host::new(256);
    bl_model::registry::run(
        &mut host.rig.editor,
        "mixer.set_arm",
        json!({ "track": host.track, "arm": true }),
    )
    .unwrap();
    host.rig.sync();
    host.peers
        .shift_beat(-host.peers.beat_at(micros(host.done)));
    // The channel's frames count the frame they play on: a ramp of them.
    let ramp: Vec<f32> = (0..40_000).map(|i| i as f32).collect();
    host.channel.push(0, &ramp, &ramp);

    let (producer, mut ring) = rtrb::RingBuffer::new(1 << 14);
    let plan = RecordingPlan {
        tracks: vec![RecordTrack {
            track: host.track,
            channel: Channel::Pair(1),
            producer,
            dropped: Arc::new(AtomicU64::new(0)),
        }],
        midi: Vec::new(),
        delay: 0,
        session: Vec::new(),
        launches: None,
    };
    host.rig
        .sender
        .record(host.rig.collector.handle().wrap(plan));
    host.rig.sender.transport(TransportCommand::SetRecording(true));
    host.rig.sender.transport(TransportCommand::Play);
    let began = host.done;
    host.run(4 * 256);
    host.rig.sender.transport(TransportCommand::Stop);
    host.run(256);
    host.rig.collector.collect();

    let mut samples = Vec::new();
    while let Ok(sample) = ring.pop() {
        samples.push(sample);
    }
    assert_eq!(samples.len(), 2 * 4 * 256, "four blocks, stereo");
    let first = samples[0];
    assert!(
        (first - began as f32).abs() <= 1.0,
        "the take starts with the frame heard as it began: {first} against {began}"
    );
    for (j, pair) in samples.chunks(2).enumerate() {
        assert_eq!(pair[0], pair[1], "both sides");
        assert_eq!(pair[0], first + j as f32, "frame {j}");
    }
}
