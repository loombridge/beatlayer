//! Link in the app (the Link spec, R1, R3, R8, R12, R15): the shell's handle
//! on the Link instance, the session the engine follows it through, and the
//! tempo a peer sets, which the main thread writes into the set.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bl_engine::Session;

/// A peer's tempo, as often as the main thread writes it into the set while
/// it moves (the Link spec, R3): Beatlayer's own rate.
pub const WRITE_EVERY: Duration = Duration::from_millis(100);

/// The last tempo a peer set, which the audio thread leaves for the main
/// thread without allocating or locking.
#[derive(Debug, Default)]
pub struct PeerTempo {
    bits: AtomicU64,
    generation: AtomicU64,
}

impl PeerTempo {
    pub(crate) fn set(&self, bpm: f64) {
        self.bits.store(bpm.to_bits(), Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// How many times a peer set the tempo, and the last one.
    pub(crate) fn read(&self) -> (u64, f64) {
        let generation = self.generation.load(Ordering::Acquire);
        (
            generation,
            f64::from_bits(self.bits.load(Ordering::Relaxed)),
        )
    }
}

/// The Link session as the engine's follower reads it: `bl-link`'s audio
/// handle, and the place a peer's tempo goes for the main thread.
pub struct LinkSession {
    audio: bl_link::Audio,
    peer: Arc<PeerTempo>,
}

impl LinkSession {
    /// A session over `audio`, reporting peers' tempos to `peer`.
    #[must_use]
    pub fn new(audio: bl_link::Audio, peer: Arc<PeerTempo>) -> Self {
        Self { audio, peer }
    }
}

impl Session for LinkSession {
    fn enabled(&self) -> bool {
        self.audio.is_enabled()
    }

    fn peers(&self) -> u64 {
        self.audio.peers()
    }

    fn start_stop_sync(&self) -> bool {
        self.audio.start_stop_sync()
    }

    fn clock_micros(&self) -> i64 {
        self.audio.clock_micros()
    }

    fn capture(&mut self) {
        self.audio.capture();
    }

    fn commit(&mut self) {
        self.audio.commit();
    }

    fn tempo(&self) -> f64 {
        self.audio.tempo()
    }

    fn set_tempo(&mut self, bpm: f64, at: i64) {
        self.audio.set_tempo(bpm, at);
    }

    fn phase_at_time(&self, time: i64, quantum: f64) -> f64 {
        self.audio.phase_at_time(time, quantum)
    }

    fn beat_at_time(&self, time: i64, quantum: f64) -> f64 {
        self.audio.beat_at_time(time, quantum)
    }

    fn time_at_beat(&self, beat: f64, quantum: f64) -> i64 {
        self.audio.time_at_beat(beat, quantum)
    }

    fn request_beat_at_time(&mut self, beat: f64, time: i64, quantum: f64) {
        self.audio.request_beat_at_time(beat, time, quantum);
    }

    fn playing(&self) -> bool {
        self.audio.is_playing()
    }

    fn time_for_playing(&self) -> i64 {
        self.audio.time_for_is_playing()
    }

    fn set_playing(&mut self, playing: bool, at: i64) {
        self.audio.set_is_playing(playing, at);
    }

    fn peer_tempo(&mut self, bpm: f64) {
        self.peer.set(bpm);
    }
}

/// A tempo the audio thread leaves for the main thread, taken as often as
/// the main thread writes it into the set while it moves.
#[derive(Debug, Default)]
pub struct TempoWrites {
    /// How many tempos the main thread has taken.
    taken: u64,
    /// The tempo that waits for the next write.
    waiting: Option<f64>,
    /// When the tempo was last written into the set.
    written: Option<Instant>,
}

impl TempoWrites {
    /// Takes the tempo `peer` last got, and returns the tempo to write into
    /// the set now: at most once every [`WRITE_EVERY`] while it moves, the
    /// last one after it stops.
    pub fn hear(&mut self, peer: &PeerTempo, now: Instant) -> Option<f64> {
        let (generation, bpm) = peer.read();
        if generation != self.taken {
            self.taken = generation;
            self.waiting = Some(bpm);
        }
        let due = self
            .written
            .is_none_or(|at| now.saturating_duration_since(at) >= WRITE_EVERY);
        if due && let Some(bpm) = self.waiting.take() {
            self.written = Some(now);
            return Some(bpm);
        }
        None
    }
}

/// What the app holds of Link: the instance, once Link has been on, and what
/// the engine tells the main thread of the session.
#[derive(Default)]
pub struct LinkHost {
    link: Option<bl_link::Link>,
    peer: Arc<PeerTempo>,
    writes: TempoWrites,
}

impl std::fmt::Debug for LinkHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkHost")
            .field("on", &self.is_enabled())
            .finish_non_exhaustive()
    }
}

impl LinkHost {
    /// The instance's handle, for the audio host, once there is one.
    #[must_use]
    pub fn handle(&self) -> Option<(bl_link::Handle, Arc<PeerTempo>)> {
        self.link
            .as_ref()
            .map(|link| (link.handle(), Arc::clone(&self.peer)))
    }

    /// Whether Link is on.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.link.as_ref().is_some_and(bl_link::Link::is_enabled)
    }

    /// Makes the instance if there is none, whose session starts at `bpm`,
    /// and returns whether it was made.
    pub fn make(&mut self, bpm: f64) -> bool {
        if self.link.is_some() {
            return false;
        }
        self.link = Some(bl_link::Link::new(bpm));
        true
    }

    /// Switches Link on or off; on, a session that doesn't exist yet starts
    /// at `bpm`.
    pub fn enable(&self, on: bool, bpm: f64) {
        if let Some(link) = &self.link {
            link.enable(on, bpm);
        }
    }

    /// Switches Start Stop Sync.
    pub fn set_start_stop_sync(&self, on: bool) {
        if let Some(link) = &self.link {
            link.set_start_stop_sync(on);
        }
    }

    /// How many other peers are in the session; none while Link is off.
    #[must_use]
    pub fn peers(&self) -> u64 {
        match &self.link {
            Some(link) if link.is_enabled() => link.peers(),
            _ => 0,
        }
    }

    /// The session as the main thread reads it, for a quantum of `quantum`
    /// quarter notes.
    #[must_use]
    pub fn snapshot(&self, quantum: f64) -> Option<bl_link::Snapshot> {
        self.link.as_ref().map(|link| link.snapshot(quantum))
    }

    /// Takes what Link has told the main thread, and the tempo a peer set
    /// since, returning the tempo to write into the set now: at most once
    /// every [`WRITE_EVERY`] while it moves, the last one after it stops.
    pub fn hear(&mut self, now: Instant) -> Option<f64> {
        if let Some(link) = &self.link {
            // The notifications only wake the main thread: the peer count
            // and tempo are read when they're wanted.
            for _ in link.events() {}
        }
        self.writes.hear(&self.peer, now)
    }
}
