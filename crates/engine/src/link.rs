//! A Link session as a follower (the Link spec, R2 to R13): its tempo,
//! phase, and start and stop, reached through a [`Session`] the audio host
//! implements over `bl-link`, so the engine builds no C++ and its tests
//! stand in for the session ([`testing::LinkSession`](crate::testing)).
//!
//! Once per block the follower captures the session, works out the moment
//! the block is heard on Link's clock, and answers for the block: the
//! tempo to play at, a jump to put the song back in phase, the frame a
//! waiting start begins on, and the starts and stops a peer asked for. It
//! writes the session back only in a block that changed something.

use bl_core::time::{TICKS_PER_QUARTER, Ticks};

use crate::follower::{Ask, Follow, Follower};
use crate::message::TransportCommand;
use crate::midi::BlockClock;

/// What a block reads from and writes to a Link session, on the audio
/// thread: no allocation, lock or blocking call, except that
/// [`commit`](Self::commit) may signal a condition variable. Times are
/// microseconds on Link's clock; the quantum and beats are in quarter notes.
pub trait Session: Send {
    /// Whether Link is on.
    fn enabled(&self) -> bool;
    /// How many other peers are in the session.
    fn peers(&self) -> u64;
    /// Whether Start Stop Sync is on.
    fn start_stop_sync(&self) -> bool;
    /// Link's clock now.
    fn clock_micros(&self) -> i64;
    /// Takes a snapshot of the session, which the calls below read and
    /// change until the next.
    fn capture(&mut self);
    /// Writes the snapshot to the session.
    fn commit(&mut self);
    /// The session's tempo.
    fn tempo(&self) -> f64;
    /// Sets the tempo, from `at` on.
    fn set_tempo(&mut self, bpm: f64, at: i64);
    /// The session's phase at `time` within `quantum`.
    fn phase_at_time(&self, time: i64, quantum: f64) -> f64;
    /// The session's beat at `time`.
    fn beat_at_time(&self, time: i64, quantum: f64) -> f64;
    /// The time the session reaches `beat`.
    fn time_at_beat(&self, beat: f64, quantum: f64) -> i64;
    /// Maps `beat` to `time` as a launch: at once with no peers, else at the
    /// next time of the same phase.
    fn request_beat_at_time(&mut self, beat: f64, time: i64, quantum: f64);
    /// Whether the session's start and stop state is playing.
    fn playing(&self) -> bool;
    /// When the session's last start or stop takes effect.
    fn time_for_playing(&self) -> i64;
    /// Sets the start and stop state, taking effect at `at`.
    fn set_playing(&mut self, playing: bool, at: i64);
    /// Tells the main thread a peer set the tempo to `bpm`, which becomes
    /// the set's tempo (R3). It must not allocate or lock.
    fn peer_tempo(&mut self, bpm: f64);
}

/// A tempo this far from the last one, in BPM, is a change: Link keeps its
/// tempo as a whole number of microseconds a beat, so it reads back a little
/// off what was set.
const TEMPO_EPS: f64 = 0.005;

/// How many tempos the follower told the main thread about it remembers, so
/// the set's tempo arriving later isn't taken for Beatlayer's own change.
const PEER_TEMPOS: usize = 16;

/// The song's phase may differ from the session's by this much, in quarter
/// notes, before the song jumps instead of slewing: a sixteenth note (R6).
const JUMP: f64 = 0.25;

/// The most the song's speed is adjusted to close a phase difference (R6).
const MAX_SLEW: f64 = 0.02;

/// How long a phase difference takes to close, in seconds.
const CLOSE_SECONDS: f64 = 0.5;

/// A Link session as a [`Follower`].
pub struct LinkFollower<S> {
    session: S,
    clock: BlockClock,
    clock_rate: f64,
    /// How many quarter notes the session moves in a frame.
    frame_beats: f64,
    active: bool,
    /// The session's tempo as last seen or written.
    tempo: f64,
    /// The tempo the set asked for as last seen: what is Beatlayer's own
    /// when it changes.
    own: f64,
    peer_tempos: [f64; PEER_TEMPOS],
    peer_next: usize,
    sync: bool,
    session_playing: bool,
    playing: bool,
    /// A start or stop sent to the engine, and the engine-clock frame by
    /// which it should have shown.
    expect: Option<(bool, u64)>,
    /// Whether this start went to the session already, when its frame came.
    announced: bool,
}

impl<S: Session> LinkFollower<S> {
    /// A follower over `session`.
    #[must_use]
    pub fn new(session: S) -> Self {
        Self {
            session,
            clock: BlockClock::new(bl_core::time::SampleRate::HZ_48000),
            clock_rate: 0.0,
            frame_beats: 0.0,
            active: false,
            tempo: 0.0,
            own: 0.0,
            peer_tempos: [0.0; PEER_TEMPOS],
            peer_next: 0,
            sync: false,
            session_playing: false,
            playing: false,
            expect: None,
            announced: false,
        }
    }

    /// The moment the block starting at `ask.clock` is heard on Link's
    /// clock, in microseconds: Link's clock read now, smoothed as the MIDI
    /// clock's callback times are, plus the output latency (R13).
    fn heard(&mut self, ask: &Ask) -> i64 {
        if self.clock_rate != ask.rate {
            self.clock = BlockClock::new(
                bl_core::time::SampleRate::new(ask.rate as u32)
                    .unwrap_or(bl_core::time::SampleRate::HZ_48000),
            );
            self.clock_rate = ask.rate;
        }
        let now = self.session.clock_micros() as f64 * 1000.0;
        let (start, _) = self.clock.advance(now, ask.clock);
        let latency = f64::from(ask.latency) / ask.rate * 1e6;
        (start / 1000.0 + latency).round() as i64
    }

    /// The engine-clock frame the block's moment `heard` plus `time - heard`
    /// microseconds falls on: never before the block's first frame.
    fn frame_at(ask: &Ask, heard: i64, time: i64) -> u64 {
        let frames = (time - heard) as f64 * ask.rate / 1e6;
        ask.clock + frames.round().max(0.0) as u64
    }

    /// The time, not before `from`, at which the session's phase within the
    /// quantum equals `phase` (R7).
    fn time_of_phase(&self, from: i64, phase: f64, quantum: f64) -> i64 {
        let here = self.session.phase_at_time(from, quantum);
        let mut ahead = (phase - here).rem_euclid(quantum);
        // A phase passed by less than a frame is the one a block before
        // rounded to a frame it didn't hold: it isn't a whole quantum away.
        if ahead > quantum - self.frame_beats {
            ahead = 0.0;
        }
        let beat = self.session.beat_at_time(from, quantum);
        self.session.time_at_beat(beat + ahead, quantum)
    }

    /// Remembers a tempo told to the main thread.
    fn remember(&mut self, bpm: f64) {
        self.peer_tempos[self.peer_next] = bpm;
        self.peer_next = (self.peer_next + 1) % PEER_TEMPOS;
    }

    /// Whether `bpm` is a tempo told to the main thread coming back as the
    /// set's. It and every tempo told before it are forgotten then, so a
    /// later change to the same tempo by hand is Beatlayer's own.
    fn told(&mut self, bpm: f64) -> bool {
        let back = (1..=PEER_TEMPOS).find(|back| {
            let told = self.peer_tempos[(self.peer_next + PEER_TEMPOS - back) % PEER_TEMPOS];
            (told - bpm).abs() < TEMPO_EPS
        });
        let Some(back) = back else {
            return false;
        };
        for older in back..=PEER_TEMPOS {
            self.peer_tempos[(self.peer_next + PEER_TEMPOS - older) % PEER_TEMPOS] = 0.0;
        }
        true
    }

    /// Takes the state a block starts from when Link comes on.
    fn begin(&mut self, ask: &Ask) {
        self.active = true;
        self.tempo = self.session.tempo();
        self.own = ask.leader.unwrap_or(ask.tempo);
        self.peer_tempos = [0.0; PEER_TEMPOS];
        self.sync = self.session.start_stop_sync();
        self.session_playing = self.session.playing();
        self.playing = ask.playing;
        self.expect = None;
        self.announced = false;
    }
}

impl<S: Session> Follower for LinkFollower<S> {
    fn block(&mut self, ask: &Ask, answer: &mut Follow) {
        if !self.session.enabled() {
            self.active = false;
            return;
        }
        // Spec: metronome R10. No count-in plays while Link is on.
        answer.no_count_in();
        self.session.capture();
        let heard = self.heard(ask);
        if !self.active {
            self.begin(ask);
        }
        let peers = self.session.peers();
        let quantum = ask.quantum;
        let mut dirty = false;
        self.frame_beats = self.session.tempo() / 60.0 / ask.rate;

        // Spec: link R3, R4. The tempo is shared both ways: a change in the
        // session that Beatlayer didn't make is a peer's; a change of the
        // set's tempo is Beatlayer's, the envelope's included until a peer's
        // change takes its place.
        let own = ask.leader.unwrap_or(ask.tempo);
        let session_tempo = self.session.tempo();
        let overridden = ask.overridden;
        if ask.leader.is_none() && (session_tempo - self.tempo).abs() > TEMPO_EPS {
            self.tempo = session_tempo;
            self.own = own;
            self.session.peer_tempo(session_tempo);
            self.remember(session_tempo);
            if ask.varies && !overridden {
                answer.override_tempo();
            }
        } else if (own - self.own).abs() > TEMPO_EPS {
            self.own = own;
            let told = self.told(own);
            if !overridden && (own - self.tempo).abs() > TEMPO_EPS && !told {
                self.session.set_tempo(own, heard);
                self.tempo = own;
                dirty = true;
            }
        }

        let leads = peers > 0 && ask.leader.is_none();
        if leads {
            // Spec: link R6. The song keeps the session's phase: slewed up
            // to 2 % faster or slower, jumped past a sixteenth.
            let mut factor = 1.0;
            if ask.playing && !ask.waiting {
                let session_phase = self.session.phase_at_time(heard, quantum);
                let mut error = (session_phase - ask.phase).rem_euclid(quantum);
                if error >= quantum / 2.0 {
                    error -= quantum;
                }
                if error.abs() > JUMP {
                    let to = ask.position + error * TICKS_PER_QUARTER as f64;
                    answer.jump(ask.clock, Ticks(to.round() as i64));
                } else {
                    factor = (1.0 + error * 60.0 / (self.tempo * CLOSE_SECONDS))
                        .clamp(1.0 - MAX_SLEW, 1.0 + MAX_SLEW);
                }
            }
            answer.tempo(self.tempo * factor);
        }

        // Spec: link R7. With peers, a start waits for the session's phase.
        let mut released = None;
        if peers > 0 {
            answer.decides_starts();
            if ask.waiting {
                let time = self.time_of_phase(heard, ask.start_phase, quantum);
                let frame = Self::frame_at(ask, heard, time);
                answer.start_at(frame);
                if frame < ask.clock + ask.frames as u64 {
                    released = Some(time);
                }
            }
        }

        // Spec: link R8. A start whose frame comes in this block goes to
        // the session at that moment.
        let sync = self.session.start_stop_sync();
        if let Some(time) = released
            && sync
            && !self.announced
        {
            self.session.set_playing(true, time);
            self.session_playing = true;
            self.announced = true;
            dirty = true;
        }

        // A peer's start or stop, with Start Stop Sync on both sides.
        let session_playing = self.session.playing();
        if sync != self.sync {
            // Switching it on while the session plays doesn't start
            // Beatlayer.
            self.sync = sync;
            self.session_playing = session_playing;
        } else if session_playing != self.session_playing {
            self.session_playing = session_playing;
            if sync {
                let at = self.session.time_for_playing().max(heard);
                if session_playing {
                    if !ask.playing && !ask.waiting {
                        let frame = if peers > 0 {
                            let time = self.time_of_phase(at, ask.start_phase, quantum);
                            Self::frame_at(ask, heard, time)
                        } else {
                            ask.clock
                        };
                        answer.command(frame, TransportCommand::PlayFromInsertMarker);
                        self.expect = Some((true, frame + ask.rate as u64));
                    }
                } else if ask.playing || ask.waiting {
                    let frame = Self::frame_at(ask, heard, self.session.time_for_playing());
                    answer.command(frame, TransportCommand::Stop);
                    self.expect = Some((false, frame + ask.rate as u64));
                }
            }
        }

        // Beatlayer's own start or stop.
        if let Some((_, by)) = self.expect
            && ask.clock > by
        {
            self.expect = None;
        }
        if ask.playing != self.playing {
            self.playing = ask.playing;
            if matches!(self.expect, Some((shown, _)) if shown == ask.playing) {
                self.expect = None;
            } else if ask.playing {
                if peers == 0 {
                    // Alone, the session's phase is the song's from here.
                    self.session.request_beat_at_time(ask.phase, heard, quantum);
                    dirty = true;
                }
                if sync && !self.announced {
                    self.session.set_playing(true, heard);
                    self.session_playing = true;
                    dirty = true;
                }
            } else if sync {
                self.session.set_playing(false, heard);
                self.session_playing = false;
                dirty = true;
            }
            self.announced = false;
        }

        if dirty {
            self.session.commit();
        }
    }
}
