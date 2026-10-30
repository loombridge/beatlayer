//! Link in the engine (WP-7.14, `docs/specs/link.md` R2 to R13): a
//! [`LinkFollower`] over a stand-in session, whose peers and clock a test
//! drives. Every block runs as a real-time section under the allocation
//! trap, at blocks of 1, 61, 512 and 4,096 frames. The song's position at
//! each frame is read from `testing::POSITION`, which says it in frames at
//! the tempo map's tempo (here 120 BPM, so 24,000 to the beat), and −1 while
//! stopped.

use crate::common;
use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::testing::{self, LinkPeers};
use bl_engine::{LinkFollower, SessionCommand, TransportCommand};
use bl_model::{Command, CountIn, LaunchQuantization, LaunchSettings, TrackKind};
use common::{RATE, Rig, device, track};

/// The block sizes every test runs at.
const BLOCKS: [usize; 4] = [1, 61, 512, 4096];

/// Frames to the beat in the song, which the map plays at 120 BPM.
const BEAT: f64 = 24_000.0;

fn micros(frames: u64) -> i64 {
    (frames as f64 * 1e6 / RATE.as_f64()).round() as i64
}

/// A rig with one track whose output is the song position of each frame, or
/// −1 while stopped, following a Link session at `bpm`.
struct Host {
    rig: Rig,
    peers: LinkPeers,
    block: usize,
    done: u64,
    out: Vec<f32>,
}

impl Host {
    /// A host with one peer in the session.
    fn new(block: usize, bpm: f64) -> Self {
        Self::with(block, bpm, 1, |_| {})
    }

    /// A host alone in the session.
    fn alone(block: usize, bpm: f64) -> Self {
        Self::with(block, bpm, 0, |_| {})
    }

    fn with(block: usize, bpm: f64, peers: u64, build: impl FnOnce(&mut Rig)) -> Self {
        let mut rig = Rig::new(block);
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
        let a = track(&mut rig.editor, TrackKind::Midi, None);
        device(&mut rig.editor, a, testing::POSITION, &[]);
        build(&mut rig);
        rig.sync();
        let (session, link) = testing::link_session(bpm);
        link.set_peers(peers);
        rig.follow(LinkFollower::new(session));
        let mut host = Self {
            rig,
            peers: link,
            block,
            done: 0,
            out: Vec::new(),
        };
        // The follower is asked in the block it arrives in, ahead of what
        // follows it: one block runs before the test starts.
        host.run(block as u64);
        host
    }

    /// A peer sets the tempo now, on the frame the engine has got to.
    fn peer_tempo(&self, bpm: f64) {
        self.peers.set_clock(micros(self.done));
        self.peers.peer_tempo(bpm);
    }

    /// Shifts the session's beats so its phase is 0 now.
    fn align(&self) {
        self.peers
            .shift_beat(-self.peers.beat_at(micros(self.done)));
    }

    /// Runs the engine up to frame `to` of its clock, in blocks, setting
    /// Link's clock at each block's start.
    fn run_to(&mut self, to: u64) {
        while self.done < to {
            let n = self.block.min((to - self.done) as usize);
            self.peers.set_clock(micros(self.done));
            self.rig.block(n);
            self.out
                .extend_from_slice(self.rig.processor.output().channel(0));
            self.done += n as u64;
        }
    }

    fn run(&mut self, frames: u64) {
        self.run_to(self.done + frames);
    }

    /// The first frame, from `from`, the song plays on.
    fn started(&self, from: usize) -> Option<usize> {
        (from..self.out.len()).find(|&i| self.out[i] >= 0.0)
    }

    /// How far the song is from the session's beat at frame `i`, in frames
    /// of the session's tempo.
    fn error(&self, i: usize) -> f64 {
        self.error_in(i, 4.0)
    }

    /// As [`Host::error`], within a quantum of `quantum` quarter notes.
    fn error_in(&self, i: usize, quantum: f64) -> f64 {
        let song = f64::from(self.out[i]) / BEAT;
        let session = self.peers.beat_at(micros(i as u64));
        let mut diff = (session - song).rem_euclid(quantum);
        if diff > quantum / 2.0 {
            diff -= quantum;
        }
        diff * 60.0 / self.peers.tempo() * RATE.as_f64()
    }
}

fn quantised(step: LaunchQuantization) -> SessionCommand {
    SessionCommand::SetQuantization(LaunchSettings {
        quantization: step,
        enabled: true,
        ..LaunchSettings::default()
    })
}

#[test]
fn a_peers_tempo_change_is_played_from_the_next_block_and_the_phase_holds() {
    // Spec: link R3, R6. Playing in phase with a session at 120 BPM; a peer
    // sets 144; the next block plays at 144, and the song's phase stays on
    // the session's to within a frame at a steady tempo.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        host.align();
        host.rig.sender.transport(TransportCommand::Play);
        host.run_to(20_000);
        let at = host.out.len();
        host.peer_tempo(144.0);
        host.run(block as u64);
        let after = &host.out[at..];
        assert!(
            host.out[block..at].iter().all(|&s| s >= 0.0),
            "plays from the start"
        );
        // 144 BPM against the map's 120: 1.2 song frames to the frame.
        let speed = (after[after.len() - 1] - after[0]) / (after.len() - 1).max(1) as f32;
        if after.len() > 1 {
            assert!((speed - 1.2).abs() < 1e-3, "block {block}: speed {speed}");
        }
        host.run_to(120_000);
        for i in [at + 2 * block, 60_000, 90_000, 119_999] {
            let error = host.error(i);
            assert!(
                error.abs() < 1.5,
                "block {block}: {error} frames off at {i}"
            );
        }
        assert_eq!(host.peers.told(), vec![144.0]);
    }
}

#[test]
fn drift_is_closed_without_a_jump_and_a_big_difference_jumps_once() {
    // Spec: link R6. A session that moves a tenth of a beat is caught up by
    // playing up to 2 % faster; one that moves a whole beat is jumped to.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        host.align();
        host.rig.sender.transport(TransportCommand::Play);
        host.run_to(30_000);
        assert!(host.error(29_999).abs() < 1.5);
        host.peers.shift_beat(0.1);
        let from = host.out.len();
        host.run(480_000);
        let played = &host.out[from..];
        // The song's position is a 32-bit number, too coarse to take a
        // frame's step from, so the speed is read over a thousand frames.
        let speeds: Vec<f32> = played
            .windows(1001)
            .map(|w| (w[1000] - w[0]) / 1000.0)
            .collect();
        assert!(
            speeds.iter().all(|&speed| speed > 0.99 && speed < 1.03),
            "block {block}: no jump while catching up"
        );
        assert!(
            speeds.iter().copied().fold(0.0_f32, f32::max) > 1.015,
            "block {block}: it played faster to catch up"
        );
        let last = host.out.len() - 1;
        assert!(host.error(last).abs() < 1.5, "block {block}: caught up");

        let mut host = Host::new(block, 120.0);
        host.align();
        host.rig.sender.transport(TransportCommand::Play);
        host.run_to(30_000);
        host.peers.shift_beat(1.0);
        let from = host.out.len();
        host.run(2 * block as u64 + 100);
        let jumps = host.out[from - 1..]
            .windows(2)
            .filter(|w| (w[1] - w[0]).abs() > 100.0)
            .count();
        assert_eq!(jumps, 1, "block {block}: one jump to the session's beat");
        let last = host.out.len() - 1;
        assert!(host.error(last).abs() < 1.5, "block {block}: in phase");
    }
}

/// The frame a start waits for: the first from `from` on where the
/// session's phase within `quantum` is `phase`.
fn due(peers: &LinkPeers, from: u64, phase: f64, quantum: f64) -> f64 {
    let here = peers.phase_at(micros(from), quantum);
    let mut ahead = (phase - here).rem_euclid(quantum);
    if ahead > quantum - 1e-9 {
        ahead = 0.0;
    }
    from as f64 + ahead * 60.0 / peers.tempo() * RATE.as_f64()
}

#[test]
fn with_peers_play_waits_for_the_phase_of_the_insert_marker() {
    // Spec: link R7. Play at a moment the session's phase isn't the start
    // position's starts on the frame where it is, for each quantisation.
    let steps = [
        (LaunchQuantization::Bar, 4.0),
        (LaunchQuantization::Quarter, 1.0),
        (LaunchQuantization::EighthTriplet, 1.0 / 3.0),
        (LaunchQuantization::None, 1.0),
    ];
    // Where Play is pressed, in frames, and where the insert marker is, in
    // quarter notes.
    let presses = [
        (0_u64, 0.0),
        (1_234, 0.0),
        (30_017, 1.5),
        (77_777, 2.75),
        (123_457, 5.0),
    ];
    for block in BLOCKS {
        for (step, quantum) in steps {
            for (press, marker) in presses {
                let mut host = Host::new(block, 123.0);
                host.rig.sender.session(quantised(step));
                host.rig.sender.transport(TransportCommand::Locate(Ticks(
                    (marker * TICKS_PER_QUARTER as f64) as i64,
                )));
                host.run_to(press);
                let press = host.done;
                host.rig.sender.transport(TransportCommand::Play);
                host.run(1);
                let state = host.rig.monitor.state();
                assert!(state.waiting_start && !state.playing, "{step:?}: it waits");
                host.run_to(press + 400_000);
                let phase = (marker).rem_euclid(quantum);
                let want = due(&host.peers, press, phase, quantum);
                let got = host.started(0).expect("it started") as f64;
                assert!(
                    (got - want).abs() <= 1.5,
                    "block {block}, {step:?}, press {press}, marker {marker}: started on {got}, due on {want}"
                );
                assert!(want > press as f64 - 1.0);
                // It plays from the marker.
                let first = f64::from(host.out[got as usize]);
                let from = marker * BEAT;
                assert!(
                    (first - from).abs() <= 1.5,
                    "plays from the marker: {first}"
                );
                // The phase of the song is the session's.
                let at = got as usize + 20_000;
                let error = host.error_in(at, quantum);
                assert!(error.abs() < 2.5, "in phase: {error}");
            }
        }
    }
}

#[test]
fn stop_cancels_a_start_that_waits_and_alone_it_starts_at_once() {
    // Spec: link R7.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        host.rig
            .sender
            .session(quantised(LaunchQuantization::Bars2));
        host.run_to(1_000);
        host.rig.sender.transport(TransportCommand::Play);
        host.run(5_000);
        assert!(host.rig.monitor.state().waiting_start);
        host.rig.sender.transport(TransportCommand::Stop);
        host.run(400_000);
        assert!(host.started(0).is_none(), "block {block}: it never started");
        assert!(!host.rig.monitor.state().waiting_start);

        let mut host = Host::alone(block, 120.0);
        host.run_to(1_000);
        let press = host.done as usize;
        host.rig.sender.transport(TransportCommand::Play);
        host.run(10);
        assert_eq!(
            host.started(0),
            Some(press),
            "block {block}: alone it starts at once"
        );
        assert!(!host.rig.monitor.state().waiting_start);
    }
}

/// Frames, from the start of the engine clock, the first one at or after
/// `from` on which the song is stopped again.
fn stopped(host: &Host, from: usize) -> Option<usize> {
    (from..host.out.len()).find(|&i| host.out[i] < 0.0)
}

#[test]
fn a_peers_start_and_stop_act_with_start_stop_sync_on() {
    // Spec: link R8. A peer's start starts Beatlayer from its insert marker
    // on the frame the session's phase gives; a peer's stop stops it at the
    // stop's moment.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        host.peers.set_start_stop_sync(true);
        host.rig.sender.session(quantised(LaunchQuantization::Bar));
        host.rig
            .sender
            .transport(TransportCommand::Locate(Ticks(TICKS_PER_QUARTER * 2)));
        host.run_to(10_000);
        let at = host.done;
        host.peers.peer_start(micros(at));
        host.run_to(at + 300_000);
        let want = due(&host.peers, at, 2.0, 4.0);
        let got = host.started(0).expect("a peer's start starts it") as f64;
        assert!(
            (got - want).abs() <= 1.5,
            "block {block}: started on {got}, due on {want}"
        );
        assert!(
            (f64::from(host.out[got as usize]) - 2.0 * BEAT).abs() <= 1.5,
            "from the marker"
        );

        // A stop 5,000 frames on stops the song on that frame.
        let stop = host.done + 5_000;
        host.peers.peer_stop(micros(stop));
        host.run_to(stop + 5_000);
        let at = stopped(&host, got as usize).expect("a peer's stop stops it");
        assert!(
            at.abs_diff(stop as usize) <= 1,
            "block {block}: stopped on {at}, not {stop}"
        );
    }
}

#[test]
fn without_start_stop_sync_a_peers_start_is_not_acted_on_and_ours_is_not_sent() {
    // Spec: link R8.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        host.peers.set_start_stop_sync(false);
        host.run_to(5_000);
        host.peers.peer_start(micros(host.done));
        host.run(200_000);
        assert!(
            host.started(0).is_none(),
            "block {block}: a peer's start isn't ours"
        );

        host.peers.peer_stop(micros(host.done));
        host.align();
        host.rig
            .sender
            .session(quantised(LaunchQuantization::Quarter));
        host.rig.sender.transport(TransportCommand::Play);
        host.run(100_000);
        assert!(host.started(0).is_some());
        assert!(!host.peers.playing(), "block {block}: our start stays ours");
    }
}

#[test]
fn our_start_and_stop_go_to_the_session_when_it_is_time_and_turning_the_sync_on_does_not_start() {
    // Spec: link R8.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        host.peers.set_start_stop_sync(true);
        host.rig.sender.session(quantised(LaunchQuantization::Bar));
        host.run_to(10_000);
        host.rig.sender.transport(TransportCommand::Play);
        host.run(400_000);
        let first = host.started(0).expect("it started");
        assert!(
            host.peers.playing(),
            "block {block}: our start reached the session"
        );
        let sent = host.peers.playing_time();
        assert!(
            (frames_of(sent) - first as f64).abs() <= 1.5,
            "block {block}: sent for {sent} µs, started on {first}"
        );
        host.rig.sender.transport(TransportCommand::Stop);
        host.run(5_000);
        assert!(
            !host.peers.playing(),
            "block {block}: our stop reached the session"
        );

        // The session plays while the sync is off here; turning it on
        // doesn't start us.
        host.peers.set_start_stop_sync(false);
        host.run(1_000);
        host.peers.peer_start(micros(host.done));
        host.run(1_000);
        host.peers.set_start_stop_sync(true);
        let from = host.out.len();
        host.run(300_000);
        assert!(
            host.started(from).is_none(),
            "block {block}: it didn't start"
        );
    }
}

fn frames_of(micros: i64) -> f64 {
    micros as f64 * RATE.as_f64() / 1e6
}

#[test]
fn no_count_in_plays_while_link_is_on() {
    // Spec: metronome R10. One bar of count-in, 96,000 frames at 120 BPM:
    // with Link on playback starts at once; without it the song waits.
    for block in BLOCKS {
        let with = |link: bool| {
            let mut host = Host::with(block, 120.0, 0, |rig| {
                rig.editor
                    .apply(Command::SetMetronomeCountIn {
                        count_in: CountIn::Bars1,
                    })
                    .unwrap();
                rig.editor
                    .apply(Command::SetMetronomeEnabled { enabled: true })
                    .unwrap();
            });
            host.peers.enable(link);
            host.run_to(10_000);
            host.rig.sender.transport(TransportCommand::Play);
            host.run(block as u64);
            host.rig.monitor.state().counting_in
        };
        assert!(!with(true), "block {block}: no count-in under Link");
        assert!(
            with(false),
            "block {block}: the count-in plays without Link"
        );
    }
}

#[test]
fn the_envelope_is_sent_and_a_peers_change_overrides_it_until_it_is_re_enabled() {
    // Spec: link R4. A tempo envelope from 120 to 140 BPM over four bars:
    // its changes go to the session as ours; a peer's change takes its
    // place, lights Re-Enable Automation, and the envelope and the session
    // don't fight over the tempo; Re-Enable Automation gives it back.
    let ramp = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (Ticks(TICKS_PER_QUARTER * 16), Bpm::new(140.0).unwrap()),
    ])
    .unwrap();
    for block in BLOCKS {
        let mut host = Host::with(block, 120.0, 1, |rig| {
            rig.editor
                .apply(Command::SetTempo {
                    tempo: ramp.clone(),
                })
                .unwrap();
        });
        host.align();
        host.rig.sender.transport(TransportCommand::Play);
        host.run(120_000);
        let envelope = |host: &mut Host| {
            let ticks = host.rig.monitor.state().position_ticks;
            ramp.bpm_at(ticks).get()
        };
        let want = envelope(&mut host);
        assert!(want > 121.0, "the song is on the ramp: {want}");
        assert!(
            (host.peers.tempo() - want).abs() < 0.4,
            "the envelope's tempo went to the session"
        );
        assert!(host.peers.commits() > 0);
        assert!(!host.rig.monitor.state().tempo_overridden);

        host.peer_tempo(100.0);
        host.run(2 * block as u64);
        assert!(
            host.rig.monitor.state().tempo_overridden,
            "block {block}: Re-Enable Automation lights"
        );
        assert_eq!(host.peers.told(), vec![100.0]);
        let commits = host.peers.commits();
        host.run(96_000);
        assert_eq!(
            host.peers.commits(),
            commits,
            "block {block}: the envelope sends nothing now"
        );
        assert!(
            (host.peers.tempo() - 100.0).abs() < 1e-9,
            "block {block}: the peer's tempo stays"
        );

        host.rig.sender.session(SessionCommand::ClearSceneOverride);
        host.run(block as u64 + 100);
        assert!(!host.rig.monitor.state().tempo_overridden);
        host.run(1_000);
        let want = envelope(&mut host);
        assert!(
            (host.peers.tempo() - want).abs() < 0.4,
            "block {block}: the envelope is the session's again"
        );
    }
}

#[test]
fn a_tempo_set_by_hand_goes_to_the_session_though_a_peer_set_it_before() {
    // Spec: link R3. A peer sets 133 and then 120, and the set follows
    // both; setting 133 by hand afterwards is Beatlayer's own change.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        let set_tempo = |host: &mut Host, bpm: f64| {
            host.rig
                .editor
                .apply(Command::SetTempo {
                    tempo: TempoMap::constant(Bpm::new(bpm).unwrap()),
                })
                .unwrap();
            host.rig.sync();
            host.run(2 * block as u64);
        };
        for bpm in [133.0, 120.0] {
            host.peer_tempo(bpm);
            host.run(2 * block as u64);
            set_tempo(&mut host, bpm);
        }
        assert_eq!(host.peers.told(), vec![133.0, 120.0]);
        set_tempo(&mut host, 133.0);
        assert!(
            (host.peers.tempo() - 133.0).abs() < 1e-9,
            "block {block}: the session has {}",
            host.peers.tempo()
        );
    }
}

#[test]
fn the_session_only_hears_a_block_that_changed_something() {
    // Spec: link R12. A song that plays in phase at a steady tempo writes
    // nothing to the session.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        host.align();
        host.rig.sender.transport(TransportCommand::Play);
        host.run(10);
        let commits = host.peers.commits();
        host.run(200_000);
        assert_eq!(host.peers.commits(), commits, "block {block}");
    }
}

#[test]
fn a_clock_that_leads_gives_the_session_its_tempo_and_the_phase_still_decides_our_starts() {
    // Spec: link R10. With EXT on and a clock at 130 BPM leading, the
    // session gets 130 as our own change; a Play waits for the session's
    // phase.
    use crate::clock_in::{BLOCKS, Host as Clock, clock_times};
    for block in BLOCKS {
        let mut clock = Clock::new(block, 90.0);
        let (session, peers) = testing::link_session(90.0);
        peers.set_peers(1);
        clock.rig.follow(LinkFollower::new(session));
        clock.clocks(&clock_times(10e6, 40.0, |_| 130.0));
        let run_to = |clock: &mut Clock, ns: f64| {
            while clock.block_ns(clock.blocks) <= ns {
                peers.set_clock((clock.block_ns(clock.blocks) / 1000.0) as i64);
                clock.run();
            }
        };
        run_to(&mut clock, 4e9);
        assert!(
            (peers.tempo() - 130.0).abs() < 0.5,
            "block {block}: the session has {}",
            peers.tempo()
        );
        assert!(
            peers.told().is_empty(),
            "block {block}: the clock's tempo isn't a peer's"
        );
        let following = clock.rig.monitor.state().following.unwrap();
        assert!(
            (following - 130.0).abs() < 0.5,
            "the clock leads: {following}"
        );

        clock
            .rig
            .sender
            .session(quantised(LaunchQuantization::Bars2));
        clock.send(TransportCommand::Play);
        run_to(&mut clock, 4e9 + 100e6);
        let state = clock.rig.monitor.state();
        assert!(
            state.waiting_start && !state.playing,
            "block {block}: Link's phase holds the start"
        );
    }
}

#[test]
fn a_peer_joining_makes_no_jump_and_a_start_by_a_timed_command_waits_too() {
    // Spec: link R2, R9. A peer that joins a session Beatlayer plays in, in
    // phase with it, changes nothing; a start sent for a frame, as Tap
    // Tempo's is, waits for the session's phase as any other.
    for block in BLOCKS {
        let mut host = Host::alone(block, 120.0);
        host.align();
        host.rig.sender.transport(TransportCommand::Play);
        host.run_to(30_000);
        let from = host.out.len();
        host.peers.set_peers(1);
        host.run(100_000);
        let steps = host.out[from..].windows(2).map(|w| w[1] - w[0]);
        assert!(
            steps.clone().all(|step| step.abs() < 4.0 && step > -1.0)
                && host.error(from).abs() < 1.5,
            "block {block}: no jump when a peer joins"
        );

        let mut host = Host::new(block, 123.0);
        host.rig.sender.session(quantised(LaunchQuantization::Bar));
        host.run_to(10_000);
        let at = host.done + 500;
        host.rig
            .sender
            .transport_at(TransportCommand::Play, bl_engine::When::Frame(at));
        host.run_to(at + 400_000);
        let want = due(&host.peers, at, 0.0, 4.0);
        let got = host.started(0).expect("it started") as f64;
        assert!(
            (got - want).abs() <= 1.5,
            "block {block}: started on {got}, due on {want}"
        );
    }
}

#[test]
fn jitter_in_when_the_callback_runs_does_not_move_the_beat() {
    // Spec: link R13. Link's clock is read with up to 2 ms of jitter at each
    // block; the song stays within half a millisecond of the session's beat,
    // as the block clock smooths what the reads say.
    let mut random = 0x9E37_79B9_7F4A_7C15_u64;
    let mut jitter = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        ((random >> 11) as f64 / (1u64 << 52) as f64 - 1.0) * 2_000.0
    };
    for block in [61, 512] {
        let mut host = Host::new(block, 120.0);
        host.align();
        host.rig.sender.transport(TransportCommand::Play);
        while host.done < 300_000 {
            let n = block.min(300_000 - host.done as usize);
            host.peers.set_clock(micros(host.done) + jitter() as i64);
            host.rig.block(n);
            host.out
                .extend_from_slice(host.rig.processor.output().channel(0));
            host.done += n as u64;
        }
        let worst = (100_000..300_000)
            .map(|i| host.error(i).abs())
            .fold(0.0, f64::max);
        // Half a millisecond is 24 frames.
        assert!(worst < 24.0, "block {block}: {worst} frames off");
    }
}
