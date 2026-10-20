//! A clock tests drive by hand, for tests that play into a song.
//!
//! The null device can pace its blocks by hand (`Pacing::Manual`), and then
//! the device, the MIDI stamps, and the engine's block-start estimate must
//! read one clock that advances by each block's length rather than the
//! system's. A [`TestClock`] is that clock: a test advances it, and every
//! reader sees the same time however busy the machine is. Production code
//! reads `Instant::now`.
//!
//! Reading it on the audio thread allocates nothing and takes no lock: the
//! time is one relaxed atomic load from the instant the clock started.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A clock tests drive with [`advance`](Self::advance). Clones share it.
#[derive(Clone, Debug)]
pub struct TestClock {
    /// The instant the clock started. Every time it gives is this plus
    /// `elapsed_ns`.
    base: Instant,
    elapsed_ns: Arc<AtomicU64>,
}

impl TestClock {
    /// A clock at zero, started now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            elapsed_ns: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The time the clock reads now.
    #[must_use]
    pub fn now(&self) -> Instant {
        self.base + self.elapsed()
    }

    /// How long the clock has been advanced by.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.elapsed_ns.load(Ordering::Relaxed))
    }

    /// Moves the clock on by `by`.
    pub fn advance(&self, by: Duration) {
        let by = u64::try_from(by.as_nanos()).unwrap_or(u64::MAX);
        let _ = self
            .elapsed_ns
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |elapsed| {
                Some(elapsed.saturating_add(by))
            });
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advancing_moves_every_clone_and_reading_keeps_the_spacing() {
        let clock = TestClock::new();
        let same = clock.clone();
        assert_eq!(clock.elapsed(), Duration::ZERO);
        let start = clock.now();
        clock.advance(Duration::from_millis(250));
        assert_eq!(clock.elapsed(), Duration::from_millis(250));
        assert_eq!(same.elapsed(), Duration::from_millis(250));
        assert_eq!(clock.now(), start + Duration::from_millis(250));
        clock.advance(Duration::from_micros(500));
        assert_eq!(same.now(), start + Duration::from_micros(250_500));
    }
}
