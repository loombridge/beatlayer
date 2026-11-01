//! Tempo Follower in the engine (the Tempo Follower spec, R6 to R10, R20):
//! the song's tempo following what a tracker hears in the audio device's
//! input.
//!
//! The tracker itself runs off the audio thread (`bl-analysis`); the audio
//! host reaches it through [`Tracker`], so the engine builds no analysis and
//! its tests stand in for the tracker with a recorded output
//! ([`testing::recorded_tracker`](crate::testing)). A [`TempoFollower`] over
//! a tracker is a [`Leader`]: each block the processor hands it the device's
//! input to copy, then asks it, as it asks any follower, for the tempo to
//! play at. It leads before a Link session is asked, which then sees the
//! tempo it leads with and shares it (the Link spec, R26).

use crate::follower::{Ask, Follow, Follower};
use crate::input::DeviceInput;

/// What a tracker gives (the Tempo Follower spec, R14).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    /// The tempo, in BPM.
    pub bpm: f64,
    /// Whether it is sure enough to follow: its confidence is at or above
    /// the one R14 names. Under it the follower keeps the last confident
    /// tempo.
    pub confident: bool,
}

/// The tracker as the audio thread reaches it: no allocation, lock or
/// blocking call in any of these.
pub trait Tracker: Send {
    /// Takes the first `frames` frames of the device's input for the
    /// tracker, which copies the channel it listens to (R11).
    fn listen(&mut self, input: &dyn DeviceInput, frames: usize);

    /// What the tracker last gave.
    fn heard(&self) -> Heard;

    /// Tells the main thread the tempo the song plays at now, which becomes
    /// the set's tempo (R6).
    fn given(&mut self, bpm: f64);
}

/// A tempo this far from the last one given, in BPM, is a change.
const TEMPO_EPS: f64 = 0.005;

/// A follower that also listens to the device's input: Tempo Follower.
pub trait Leader: Follower {
    /// Takes this block's device input before the follower is asked (R11).
    fn listen(&mut self, input: &dyn DeviceInput, frames: usize);
}

/// The tempo a [`Tracker`] hears as a [`Follower`].
pub struct TempoFollower<T> {
    tracker: T,
    /// The last confident tempo, kept while the tracker isn't (R14).
    tempo: Option<f64>,
    /// The tempo last told to the main thread.
    given: f64,
}

impl<T: Tracker> TempoFollower<T> {
    /// A follower over `tracker`, which follows nothing until the tracker is
    /// confident.
    #[must_use]
    pub fn new(tracker: T) -> Self {
        Self {
            tracker,
            tempo: None,
            given: 0.0,
        }
    }
}

impl<T: Tracker> Leader for TempoFollower<T> {
    fn listen(&mut self, input: &dyn DeviceInput, frames: usize) {
        self.tracker.listen(input, frames);
    }
}

impl<T: Tracker> Follower for TempoFollower<T> {
    fn block(&mut self, ask: &Ask, answer: &mut Follow) {
        let heard = self.tracker.heard();
        if heard.confident && heard.bpm.is_finite() {
            self.tempo = Some(heard.bpm);
        }
        // Spec: tempo-follower R6, R7. Before the tracker is first confident
        // the song plays its own tempo; after, the tempo it gave, once per
        // block, with the song's beat where it is.
        let Some(bpm) = self.tempo else {
            return;
        };
        answer.tempo(bpm);
        if (bpm - self.given).abs() > TEMPO_EPS {
            self.given = bpm;
            self.tracker.given(bpm);
        }
        // Spec: tempo-follower R8. A followed tempo that differs from the
        // map's takes a tempo envelope's place, as a hand move does.
        if ask.varies && !ask.overridden && (bpm - ask.tempo).abs() > TEMPO_EPS {
            answer.override_tempo();
        }
    }
}
