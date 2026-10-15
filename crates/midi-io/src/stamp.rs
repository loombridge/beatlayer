//! Mapping a MIDI system's own timestamps onto the engine's clock.

use std::time::{Duration, Instant};

/// How far the estimated offset may rise per second of input when no
/// message arrives sooner than it expects: 200 µs a second (200 ppm), more
/// than two crystal clocks drift apart.
const DRIFT_PER_SECOND_NS: f64 = 200_000.0;

/// A stamp that maps to more than this before the message arrived means the
/// system's clock jumped (a restarted queue, a stamp of 0): the clock starts
/// over from the message's arrival.
const MAX_AGE: Duration = Duration::from_millis(50);

/// Maps a MIDI system's timestamps onto [`Instant`]s, the clock the engine
/// places messages with (`docs/specs/midi-io.md`, R13).
///
/// Core MIDI and the ALSA sequencer stamp each message with the time it
/// arrived at the system, on a clock of their own; `midir` passes them on
/// in microseconds. Those stamps keep the spacing of messages that the
/// system hands over together, which arrival times on the handler's thread
/// lose. Each stamp is a fixed offset away from the [`Instant`] it stands
/// for; the smallest gap seen between a stamp and its arrival is that
/// offset plus the system's shortest delivery delay, so the clock keeps the
/// smallest gap, letting it rise slowly in case the two clocks drift apart.
///
/// A mapped time is never after the message's arrival, and never more than
/// 50 ms before it.
#[derive(Clone, Copy, Debug, Default)]
pub struct StampClock {
    /// What arrivals are measured from.
    base: Option<Instant>,
    /// Arrival minus stamp, in nanoseconds, at its smallest.
    offset: Option<f64>,
    /// The arrival that last updated the offset, in nanoseconds after
    /// `base`.
    last: f64,
}

impl StampClock {
    /// A clock that hasn't seen a message yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// When a message stamped `stamp_us` (microseconds on the system's
    /// clock) that arrived at `arrival` reached the system.
    pub fn map(&mut self, stamp_us: u64, arrival: Instant) -> Instant {
        let base = *self.base.get_or_insert(arrival);
        let Some(since_base) = arrival.checked_duration_since(base) else {
            // An arrival before the first one: start over from it.
            *self = Self::default();
            return self.map(stamp_us, arrival);
        };
        let now = since_base.as_nanos() as f64;
        let stamp = stamp_us as f64 * 1_000.0;
        let gap = now - stamp;
        let offset = match self.offset {
            Some(offset) => {
                let allowed = offset + (now - self.last).max(0.0) * DRIFT_PER_SECOND_NS / 1e9;
                gap.min(allowed)
            }
            None => gap,
        };
        self.offset = Some(offset);
        self.last = now;
        let mapped = stamp + offset;
        if mapped >= now {
            return arrival;
        }
        let age = Duration::from_nanos((now - mapped) as u64);
        if age > MAX_AGE {
            self.offset = Some(gap);
            return arrival;
        }
        arrival - age
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: midi-io R13
    #[test]
    fn stamps_keep_the_spacing_that_arrival_times_lose() {
        let mut clock = StampClock::new();
        let start = Instant::now();
        // The system's clock reads 1,000 s when ours reads `start`; each
        // message takes 0.2 ms to be delivered, but these three were handed
        // over together 3 ms after the first was stamped.
        let system = |ms: f64| (1_000_000_000.0 + ms * 1_000.0) as u64;
        let at = |ms: f64| start + Duration::from_secs_f64(ms / 1_000.0);
        assert_eq!(clock.map(system(0.0), at(0.2)), at(0.2));
        let late = at(3.2);
        let first = clock.map(system(1.0), late);
        let second = clock.map(system(2.0), late);
        let third = clock.map(system(3.0), late);
        let ms = |t: Instant| t.duration_since(start).as_secs_f64() * 1_000.0;
        assert!((ms(first) - 1.2).abs() < 0.01, "{}", ms(first));
        assert!((ms(second) - 2.2).abs() < 0.01, "{}", ms(second));
        assert!((ms(third) - 3.2).abs() < 0.01, "{}", ms(third));
    }

    #[test]
    fn a_jump_in_the_systems_clock_starts_over_from_the_arrival() {
        let mut clock = StampClock::new();
        let start = Instant::now();
        clock.map(5_000_000, start);
        // The system's clock went back to 0 (a restarted queue).
        let arrival = start + Duration::from_millis(10);
        assert_eq!(clock.map(1_000, arrival), arrival);
        // From then on it follows the new clock: stamped 4 ms after the
        // last, this message reached the system 4 ms after it, though it
        // arrived 5 ms after.
        let next = arrival + Duration::from_millis(5);
        let mapped = clock.map(5_000, next);
        let expected = arrival + Duration::from_millis(4);
        let error = mapped.max(expected).duration_since(mapped.min(expected));
        assert!(error < Duration::from_micros(10), "{error:?}");
        // A stamp ahead of its arrival is taken as the arrival.
        let ahead = next + Duration::from_millis(1);
        assert_eq!(clock.map(1_000_000, ahead), ahead);
    }
}
