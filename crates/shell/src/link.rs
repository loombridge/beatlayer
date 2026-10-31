//! Link in the app (the Link spec, R1, R3, R8, R12, R15): the shell's handle
//! on the Link instance, the session the engine follows it through, and the
//! tempo a peer sets, which the main thread writes into the set.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bl_engine::Session;

use crate::link_audio::{AudioSettings, LinkAudioHost};

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
    fn set(&self, bpm: f64) {
        self.bits.store(bpm.to_bits(), Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// How many times a peer set the tempo, and the last one.
    fn read(&self) -> (u64, f64) {
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
    settings: Arc<AudioSettings>,
}

impl LinkSession {
    /// A session over `audio`, reporting peers' tempos to `peer`, with
    /// Sync to Incoming Audio off.
    #[must_use]
    pub fn new(audio: bl_link::Audio, peer: Arc<PeerTempo>) -> Self {
        Self::with_settings(audio, peer, Arc::default())
    }

    /// A session that also delays the song by what `settings` say (the Link
    /// Audio spec, R8).
    #[must_use]
    pub fn with_settings(
        audio: bl_link::Audio,
        peer: Arc<PeerTempo>,
        settings: Arc<AudioSettings>,
    ) -> Self {
        Self {
            audio,
            peer,
            settings,
        }
    }
}

/// What the audio host needs of Link to give an engine its follower and its
/// Link Audio: the instance's handle, and what the audio thread shares with
/// the main thread.
#[derive(Clone)]
pub struct LinkHandles {
    /// Makes the audio thread's handles.
    pub link: bl_link::Handle,
    /// Where the follower leaves a peer's tempo.
    pub peer: Arc<PeerTempo>,
    /// The Latency and Sync to Incoming Audio.
    pub settings: Arc<AudioSettings>,
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

    fn audio_delay(&self) -> i64 {
        if self.audio.audio_enabled() {
            self.settings.delay_micros()
        } else {
            0
        }
    }
}

/// What the app holds of Link: the instance, once Link has been on, and what
/// the engine tells the main thread of the session.
#[derive(Default)]
pub struct LinkHost {
    link: Option<bl_link::Link>,
    peer: Arc<PeerTempo>,
    /// Link Audio's sinks and sources, and the channels the peers offer.
    pub(crate) audio: LinkAudioHost,
    /// How many peer tempos the main thread has taken.
    taken: u64,
    /// The tempo a peer set that waits for the next write.
    waiting: Option<f64>,
    /// When the tempo was last written into the set.
    written: Option<Instant>,
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
    pub fn handle(&self) -> Option<LinkHandles> {
        self.link.as_ref().map(|link| LinkHandles {
            link: link.handle(),
            peer: Arc::clone(&self.peer),
            settings: Arc::clone(&self.audio.settings),
        })
    }

    /// The Link instance, once there is one.
    #[must_use]
    pub fn instance(&self) -> Option<&bl_link::Link> {
        self.link.as_ref()
    }

    /// Whether Link Audio is switched on in the instance.
    #[must_use]
    pub fn audio_enabled(&self) -> bool {
        self.link.as_ref().is_some_and(bl_link::Link::audio_enabled)
    }

    /// Switches Link Audio and gives it the name other peers see.
    pub fn set_audio(&self, on: bool, name: &str) {
        if let Some(link) = &self.link {
            link.set_audio_name(name);
            link.set_audio_enabled(on);
        }
    }

    /// Sets the name other peers see the app by.
    pub fn set_audio_name(&self, name: &str) {
        if let Some(link) = &self.link {
            link.set_audio_name(name);
        }
    }

    /// Brings Link Audio's sinks and sources to `wanted` (made when
    /// `fingerprint` was told of the set), returning the plan the audio
    /// thread should have now if it changed.
    pub fn update_audio(
        &mut self,
        fingerprint: u64,
        wanted: &crate::link_audio::Wanted,
    ) -> Option<Arc<crate::link_audio::Plan>> {
        let Self { link, audio, .. } = self;
        link.as_ref()
            .and_then(|link| audio.update(link, fingerprint, wanted))
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
            // and tempo are read when they're wanted; a change of the
            // channels is noted for Link Audio's next update.
            for event in link.events() {
                if event == bl_link::Event::Channels {
                    self.audio.channels_changed();
                }
            }
        }
        let (generation, bpm) = self.peer.read();
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
