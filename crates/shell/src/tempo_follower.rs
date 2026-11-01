//! Tempo Follower in the app (the Tempo Follower spec, R3, R6, R10, R11): the
//! tracker's ring and thread for the audio device's stream, the engine's end
//! of it, and the tempo the engine leaves for the main thread to write into
//! the set.

use std::sync::Arc;
use std::time::Instant;

use bl_analysis::tempo_tracker::{self, Copier, Estimate, Running, Shared};
use bl_engine::{DeviceInput, Heard, Tracker};
use bl_model::Channel;

use crate::link::{PeerTempo, TempoWrites};

/// The tracker's channel for a routing `channel`, which counts from 1.
fn tracked(channel: Channel) -> tempo_tracker::Channel {
    match channel {
        Channel::Mono(c) => tempo_tracker::Channel::Mono(usize::from(c.saturating_sub(1))),
        Channel::Pair(c) => tempo_tracker::Channel::Pair(usize::from(c.saturating_sub(1))),
    }
}

/// Whether the audio device's `inputs` channels hold `channel` (R4).
#[must_use]
pub fn reachable(channel: Channel, inputs: u16) -> bool {
    match channel {
        Channel::Mono(c) => c >= 1 && c <= inputs,
        Channel::Pair(c) => c >= 1 && c < inputs,
    }
}

/// The engine's end of the tracker: it copies the chosen input channel each
/// block, reads what the tracker last gave, and leaves the tempo it played
/// for the main thread. Nothing in it allocates, locks or blocks.
pub struct EngineTracker {
    copier: Copier,
    shared: Arc<Shared>,
    given: Arc<PeerTempo>,
}

impl Tracker for EngineTracker {
    fn listen(&mut self, input: &dyn DeviceInput, frames: usize) {
        self.copier
            .copy_planar(input.channels(), frames, |c| input.channel(c));
    }

    fn heard(&self) -> Heard {
        let Estimate {
            bpm, confidence, ..
        } = self.shared.estimate();
        Heard {
            bpm,
            confident: confidence >= tempo_tracker::CONFIDENT,
        }
    }

    fn given(&mut self, bpm: f64) {
        self.given.set(bpm);
    }
}

/// What the main thread holds of a running tracker: its thread, what it
/// publishes and the tempo the engine played.
pub struct Tracking {
    _thread: Running,
    shared: Arc<Shared>,
    given: Arc<PeerTempo>,
    writes: TempoWrites,
}

impl Tracking {
    /// Starts a tracker for the device's stream at `rate` Hz, for a set at
    /// `bpm`, listening to `channel`: its thread, and the end the engine
    /// takes.
    #[must_use]
    pub fn start(rate: u32, bpm: f64, channel: Channel) -> (Self, EngineTracker) {
        let (copier, tracker) = tempo_tracker::ring(rate, bpm, tracked(channel));
        let shared = tracker.shared();
        let given = Arc::new(PeerTempo::default());
        let engine = EngineTracker {
            copier,
            shared: Arc::clone(&shared),
            given: Arc::clone(&given),
        };
        let thread = Self {
            _thread: tracker.spawn(),
            shared,
            given,
            writes: TempoWrites::default(),
        };
        (thread, engine)
    }

    /// What the tracker last gave.
    #[must_use]
    pub fn estimate(&self) -> Estimate {
        self.shared.estimate()
    }

    /// How many blocks of input the copy dropped.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.shared.dropped()
    }

    /// The tempo the engine played last, once it has played one.
    #[must_use]
    pub fn followed(&self) -> Option<f64> {
        let (generation, bpm) = self.given.read();
        (generation > 0).then_some(bpm)
    }

    /// The tempo to write into the set now: at most once every
    /// [`crate::link::WRITE_EVERY`] while it moves, the last one after it
    /// stops (R6).
    pub fn hear(&mut self, now: Instant) -> Option<f64> {
        self.writes.hear(&self.given, now)
    }
}
